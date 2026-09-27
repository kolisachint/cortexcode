//! Ports `test/suite/agent-session-runtime.test.ts`,
//! `test/agent-session-runtime-events.test.ts` and
//! `test/agent-session-branching.test.ts` (live-model in TS; faux here).
//!
//! The extension factories become a recording [`ExtensionHooks`]; the faux
//! provider's models reach the model registry through `models.json` (TS
//! registers them with an extension's `registerProvider`, 12.3). Not ported
//! here: the `message_end` replacement case and the stale extension-ctx check
//! (both need the extension runtime, 12.3).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

use cortexcode_ai_provider_faux::{
    faux_assistant_message, register_faux_provider, FauxMessageOptions, FauxModelDefinition,
    FauxProvider, FauxProviderRegistration, FauxResponseStep, RegisterFauxProviderOptions,
};
use cortexcode_ai_types::ThinkingLevel;
use cortexcode_code_agent_session::runtime::{
    create_agent_session_runtime, AgentSessionRuntime, ChangeDirectoryResult, CreatedRuntime,
    ForkResult, NewSessionRequest, RuntimeError, RuntimeFactory, RuntimeRequest,
};
use cortexcode_code_agent_session::{
    create_agent_session, AgentSessionServices, BaseTools, CreateAgentSessionOptions,
    ExtensionHooks, ForkPosition, PromptOptions, SessionEvent, SessionEventFuture,
    SessionEventResult, SessionShutdownReason, SessionStartReason, SessionSwitchReason,
    StaticResourceLoader,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_session::{default_session_dir, SessionManager};
use cortexcode_code_settings::SettingsManager;

/// Keep default session dirs (and the agent dir) out of the real home.
fn isolate_agent_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-runtime-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir);
    });
}

struct FauxAuth(String);

impl AuthLookup for FauxAuth {
    fn api_key(&self, provider: &str) -> Option<String> {
        (provider == self.0).then(|| "faux-key".to_string())
    }
}

/// A recorded session event, in the TS test's `toEqual` shape.
#[derive(Debug, Clone, PartialEq)]
enum Recorded {
    Start(SessionStartReason, Option<String>),
    BeforeSwitch(SessionSwitchReason, Option<String>),
    BeforeFork(String, ForkPosition),
    Shutdown(SessionShutdownReason, Option<String>),
}

type CancelFn = Box<dyn Fn(&SessionEvent) -> bool + Send + Sync>;

/// `hoo.on(...)` handlers: record the listed event types, cancel on demand.
struct Recorder {
    handles: Vec<&'static str>,
    events: Arc<Mutex<Vec<Recorded>>>,
    cancel: CancelFn,
}

impl ExtensionHooks for Recorder {
    fn has_handlers(&self, event_type: &str) -> bool {
        self.handles.contains(&event_type)
    }

    fn emit_session_event(&self, event: SessionEvent) -> SessionEventFuture {
        let mut result = SessionEventResult::default();
        if self.handles.contains(&event.event_type()) {
            let recorded = match &event {
                SessionEvent::Start(start) => Some(Recorded::Start(
                    start.reason,
                    start.previous_session_file.clone(),
                )),
                SessionEvent::BeforeSwitch {
                    reason,
                    target_session_file,
                } => Some(Recorded::BeforeSwitch(*reason, target_session_file.clone())),
                SessionEvent::BeforeFork { entry_id, position } => {
                    Some(Recorded::BeforeFork(entry_id.clone(), *position))
                }
                SessionEvent::Shutdown {
                    reason,
                    target_session_file,
                } => Some(Recorded::Shutdown(*reason, target_session_file.clone())),
                _ => None,
            };
            if let Some(recorded) = recorded {
                self.events.lock().unwrap().push(recorded);
            }
            result.cancel = (self.cancel)(&event);
        }
        Box::pin(async move { result })
    }
}

#[derive(Clone)]
struct Setup {
    faux: Arc<FauxProvider>,
    agent_dir: PathBuf,
    extensions: Option<Arc<dyn ExtensionHooks>>,
    bootstrap_model: bool,
}

fn write_models_json(dir: &Path, faux: &FauxProvider) {
    let model = faux.get_model();
    let models: Vec<_> = faux
        .models()
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id, "name": m.name, "reasoning": m.reasoning, "input": m.input,
                "contextWindow": m.context_window, "maxTokens": m.max_tokens,
            })
        })
        .collect();
    let config = serde_json::json!({"providers": {model.provider.clone(): {
        "baseUrl": model.base_url, "apiKey": "faux-key", "api": faux.api(), "models": models,
    }}});
    std::fs::write(dir.join("models.json"), config.to_string()).unwrap();
}

fn factory(setup: Setup) -> RuntimeFactory {
    Arc::new(move |request: RuntimeRequest| {
        let setup = setup.clone();
        Box::pin(async move {
            let services = AgentSessionServices {
                cwd: request.cwd.clone(),
                agent_dir: setup.agent_dir.clone(),
                auth: Arc::new(FauxAuth(setup.faux.get_model().provider)),
                settings: Arc::new(Mutex::new(SettingsManager::in_memory(Default::default()))),
                model_registry: Arc::new(ModelRegistry::create(
                    setup.agent_dir.join("models.json"),
                )),
                resource_loader: Arc::new(StaticResourceLoader::default()),
                diagnostics: vec![],
            };
            let created = create_agent_session(
                &services,
                request.session_manager,
                CreateAgentSessionOptions {
                    model: setup.bootstrap_model.then(|| setup.faux.get_model()),
                    base_tools: Some(BaseTools::Override(vec![])),
                    extensions: setup.extensions.clone(),
                    stream_fn: Some(Arc::new(setup.faux.stream_fn())),
                    session_start_event: request.session_start_event,
                    ..Default::default()
                },
            );
            Ok(CreatedRuntime {
                session: created.session,
                diagnostics: services.diagnostics.clone(),
                services,
                model_fallback_message: created.model_fallback_message,
            })
        })
    })
}

struct TestRuntime {
    runtime: AgentSessionRuntime,
    faux: Arc<FauxProvider>,
    registration: FauxProviderRegistration,
    temp: tempfile::TempDir,
    events: Arc<Mutex<Vec<Recorded>>>,
}

impl TestRuntime {
    fn dir(&self) -> PathBuf {
        self.temp.path().canonicalize().unwrap()
    }

    fn events(&self) -> Vec<Recorded> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    async fn prompt(&self, text: &str) {
        self.runtime
            .session()
            .prompt(text, PromptOptions::default())
            .await
            .unwrap();
    }

    fn session_file(&self) -> Option<String> {
        self.runtime
            .session()
            .session_file()
            .map(|p| p.to_string_lossy().into_owned())
    }

    async fn finish(mut self) {
        self.runtime.dispose().await;
        self.registration.unregister();
    }
}

#[derive(Default)]
struct Options {
    handles: Vec<&'static str>,
    cancel: Option<CancelFn>,
    in_memory: bool,
    bootstrap_model: Option<bool>,
}

fn faux_models() -> Vec<FauxModelDefinition> {
    vec![
        FauxModelDefinition {
            id: "faux-1".into(),
            reasoning: Some(true),
            ..Default::default()
        },
        FauxModelDefinition {
            id: "faux-2".into(),
            reasoning: Some(false),
            ..Default::default()
        },
    ]
}

fn text(t: &str) -> FauxResponseStep {
    FauxResponseStep::Message(faux_assistant_message(t, FauxMessageOptions::default()))
}

async fn create_runtime(options: Options) -> TestRuntime {
    isolate_agent_dir();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let registration = register_faux_provider(RegisterFauxProviderOptions {
        models: faux_models(),
        ..Default::default()
    });
    let faux = registration.provider().clone();
    faux.set_responses(vec![text("one"), text("two"), text("three")]);
    write_models_json(&dir, &faux);

    let events = Arc::new(Mutex::new(Vec::new()));
    let recorder = Recorder {
        handles: options.handles,
        events: events.clone(),
        cancel: options.cancel.unwrap_or_else(|| Box::new(|_| false)),
    };
    let setup = Setup {
        faux: faux.clone(),
        agent_dir: dir.clone(),
        extensions: Some(Arc::new(recorder)),
        bootstrap_model: options.bootstrap_model.unwrap_or(true),
    };
    let dir_str = dir.to_string_lossy().into_owned();
    let session_manager = if options.in_memory {
        SessionManager::in_memory(dir_str)
    } else {
        SessionManager::create(dir_str, None)
    };
    let runtime = create_agent_session_runtime(
        factory(setup),
        RuntimeRequest {
            cwd: dir.clone(),
            agent_dir: dir,
            session_manager,
            session_start_event: None,
        },
    )
    .await
    .unwrap();
    runtime.session().bind_extensions().await;
    TestRuntime {
        runtime,
        faux,
        registration,
        temp,
        events,
    }
}

/// `(role, user text)` for each message.
fn message_shape(runtime: &AgentSessionRuntime) -> Vec<(String, Option<String>)> {
    runtime
        .session()
        .messages()
        .iter()
        .map(|m| {
            let text = match m {
                cortexcode_agent_types::AgentMessage::User(u) => Some(
                    cortexcode_code_agent_session::stats::extract_user_message_text(&u.content),
                ),
                _ => None,
            };
            (m.role().to_string(), text)
        })
        .collect()
}

const LIFECYCLE: [&str; 4] = [
    "session_before_switch",
    "session_before_fork",
    "session_shutdown",
    "session_start",
];

#[tokio::test(flavor = "multi_thread")]
async fn moves_the_runtime_to_another_directory_and_starts_a_session_there() {
    let mut t = create_runtime(Options::default()).await;
    let other = tempfile::tempdir().unwrap();
    let other_dir = other.path().canonicalize().unwrap();

    t.prompt("hello").await;
    let original_file = t.session_file().unwrap();
    let original_id = t.runtime.session().session_id();

    let result = t.runtime.change_directory(&other_dir).await.unwrap();
    t.runtime.session().bind_extensions().await;

    assert_eq!(
        result,
        ChangeDirectoryResult {
            cancelled: false,
            cwd: other_dir.clone()
        }
    );
    assert_eq!(t.runtime.cwd(), other_dir);
    assert_ne!(t.runtime.session().session_id(), original_id);
    assert_eq!(
        t.runtime.session().session_manager().cwd(),
        other_dir.to_string_lossy()
    );
    assert!(t.runtime.session().messages().is_empty());
    assert_ne!(t.session_file().unwrap(), original_file);
    assert!(Path::new(&original_file).exists());
    assert_eq!(
        t.runtime.session().session_manager().session_dir(),
        default_session_dir(&other_dir.to_string_lossy())
    );
    assert_ne!(t.dir(), other_dir);
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_a_directory_that_is_missing_or_is_a_file_without_touching_the_session() {
    let mut t = create_runtime(Options::default()).await;
    let session_id = t.runtime.session().session_id();
    let file = t.dir().join("not-a-directory.txt");
    std::fs::write(&file, "").unwrap();

    let missing = t
        .runtime
        .change_directory(&t.dir().join("does-not-exist"))
        .await
        .unwrap_err();
    assert!(matches!(missing, RuntimeError::ChangeDirectory { .. }));
    let not_dir = t.runtime.change_directory(&file).await.unwrap_err();
    assert!(not_dir.to_string().contains("Not a directory"), "{not_dir}");
    assert_eq!(t.runtime.session().session_id(), session_id);
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn is_a_no_op_when_the_target_is_the_current_directory() {
    let mut t = create_runtime(Options::default()).await;
    let session_id = t.runtime.session().session_id();
    let dir = t.dir();
    let result = t.runtime.change_directory(&dir).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(t.runtime.session().session_id(), session_id);
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_session_before_switch_and_session_start_for_new_and_resume_flows() {
    let mut t = create_runtime(Options {
        handles: vec!["session_before_switch", "session_shutdown", "session_start"],
        ..Default::default()
    })
    .await;
    assert_eq!(
        t.events(),
        [Recorded::Start(SessionStartReason::Startup, None)]
    );

    t.prompt("hello").await;
    let original_file = t.session_file();
    assert!(original_file.is_some());

    let result = t
        .runtime
        .new_session(NewSessionRequest::default())
        .await
        .unwrap();
    assert!(!result.cancelled);
    t.runtime.session().bind_extensions().await;
    assert!(t.runtime.session().messages().is_empty());
    let second_file = t.session_file();
    assert_eq!(
        t.events(),
        [
            Recorded::BeforeSwitch(SessionSwitchReason::New, None),
            Recorded::Shutdown(SessionShutdownReason::New, second_file.clone()),
            Recorded::Start(SessionStartReason::New, original_file.clone()),
        ]
    );
    assert!(second_file.is_some());

    let result = t
        .runtime
        .switch_session(Path::new(original_file.as_ref().unwrap()), None)
        .await
        .unwrap();
    assert!(!result.cancelled);
    t.runtime.session().bind_extensions().await;
    assert_eq!(
        t.events(),
        [
            Recorded::BeforeSwitch(SessionSwitchReason::Resume, original_file.clone()),
            Recorded::Shutdown(SessionShutdownReason::Resume, original_file.clone()),
            Recorded::Start(SessionStartReason::Resume, second_file),
        ]
    );
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn honors_session_before_switch_cancellation_for_new_and_resume() {
    let cancel_reason: Arc<Mutex<Option<SessionSwitchReason>>> = Arc::default();
    let reason = cancel_reason.clone();
    let mut t = create_runtime(Options {
        handles: vec!["session_before_switch", "session_start"],
        cancel: Some(Box::new(move |event| match event {
            SessionEvent::BeforeSwitch { reason: r, .. } => *reason.lock().unwrap() == Some(*r),
            _ => false,
        })),
        ..Default::default()
    })
    .await;
    t.prompt("hello").await;
    let original_file = t.session_file();

    *cancel_reason.lock().unwrap() = Some(SessionSwitchReason::New);
    let result = t
        .runtime
        .new_session(NewSessionRequest::default())
        .await
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(t.session_file(), original_file);
    assert_eq!(
        t.events().last(),
        Some(&Recorded::BeforeSwitch(SessionSwitchReason::New, None))
    );

    let other = tempfile::tempdir().unwrap();
    let mut other_session = SessionManager::create(
        other.path().to_string_lossy(),
        Some(other.path().join("sessions")),
    );
    other_session.append_message(cortexcode_agent_types::AgentMessage::User(
        cortexcode_ai_types::UserMessage {
            content: vec![cortexcode_ai_types::Content::text("other")].into(),
            timestamp: cortexcode_ai_types::now_ms(),
        },
    ));
    let other_file = other_session.session_file().unwrap().to_path_buf();
    *cancel_reason.lock().unwrap() = Some(SessionSwitchReason::Resume);
    let result = t.runtime.switch_session(&other_file, None).await.unwrap();
    assert!(result.cancelled);
    assert_eq!(t.session_file(), original_file);
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_session_before_fork_and_session_start_and_honors_cancellation() {
    let cancel_next: Arc<Mutex<bool>> = Arc::default();
    let cancel = cancel_next.clone();
    let mut t = create_runtime(Options {
        handles: LIFECYCLE.to_vec(),
        cancel: Some(Box::new(move |event| {
            matches!(event, SessionEvent::BeforeFork { .. })
                && std::mem::take(&mut *cancel.lock().unwrap())
        })),
        ..Default::default()
    })
    .await;
    t.events();
    t.prompt("hello").await;
    let user_message = t.runtime.session().get_user_messages_for_forking()[0].clone();
    let previous_file = t.session_file();

    let result = t
        .runtime
        .fork(&user_message.entry_id, ForkPosition::Before)
        .await
        .unwrap();
    assert_eq!(
        result,
        ForkResult {
            cancelled: false,
            selected_text: Some("hello".into())
        }
    );
    t.runtime.session().bind_extensions().await;
    assert_eq!(
        t.events(),
        [
            Recorded::BeforeFork(user_message.entry_id.clone(), ForkPosition::Before),
            Recorded::Shutdown(SessionShutdownReason::Fork, t.session_file()),
            Recorded::Start(SessionStartReason::Fork, previous_file),
        ]
    );

    *cancel_next.lock().unwrap() = true;
    let result = t
        .runtime
        .fork(&user_message.entry_id, ForkPosition::Before)
        .await
        .unwrap();
    assert_eq!(
        result,
        ForkResult {
            cancelled: true,
            selected_text: None
        }
    );
    assert_eq!(
        t.events(),
        [Recorded::BeforeFork(
            user_message.entry_id.clone(),
            ForkPosition::Before
        )]
    );

    *cancel_next.lock().unwrap() = true;
    let result = t
        .runtime
        .fork("missing-entry", ForkPosition::At)
        .await
        .unwrap();
    assert!(result.cancelled);
    assert_eq!(
        t.events(),
        [Recorded::BeforeFork(
            "missing-entry".into(),
            ForkPosition::At
        )]
    );
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicates_the_current_active_branch_when_forking_at_the_current_position() {
    for in_memory in [false, true] {
        let mut t = create_runtime(Options {
            in_memory,
            ..Default::default()
        })
        .await;
        t.prompt("hello").await;
        t.prompt("again").await;
        let before = message_shape(&t.runtime);
        let previous_file = t.session_file();
        assert_eq!(previous_file.is_none(), in_memory);
        let leaf = t
            .runtime
            .session()
            .session_manager()
            .leaf_id()
            .unwrap()
            .to_string();

        let result = t.runtime.fork(&leaf, ForkPosition::At).await.unwrap();
        assert_eq!(result, ForkResult::default());
        if in_memory {
            assert!(t.session_file().is_none());
        } else {
            assert_ne!(t.session_file(), previous_file);
        }
        assert_eq!(message_shape(&t.runtime), before);
        t.finish().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_forking_with_an_invalid_entry_id() {
    let mut t = create_runtime(Options::default()).await;
    let error = t
        .runtime
        .fork("missing-entry", ForkPosition::Before)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Invalid entry ID for forking");
    t.finish().await;
}

/// A second runtime over the same faux provider, rooted at `cwd`.
async fn other_runtime(t: &TestRuntime, cwd: &Path) -> AgentSessionRuntime {
    let setup = Setup {
        faux: t.faux.clone(),
        agent_dir: t.dir(),
        extensions: None,
        bootstrap_model: false,
    };
    create_agent_session_runtime(
        factory(setup),
        RuntimeRequest {
            cwd: cwd.to_path_buf(),
            agent_dir: t.dir(),
            session_manager: SessionManager::create(cwd.to_string_lossy(), None),
            session_start_event: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn updates_the_runtime_session_cwd_on_cross_cwd_session_replacement() {
    let mut t = create_runtime(Options::default()).await;
    let second = tempfile::tempdir().unwrap();
    let second_dir = second.path().canonicalize().unwrap();
    let mut other = other_runtime(&t, &second_dir).await;
    other
        .session()
        .set_model(t.faux.get_model())
        .expect("faux model");
    other
        .session()
        .prompt("other", PromptOptions::default())
        .await
        .unwrap();
    let other_file = other.session().session_file().unwrap();

    t.runtime.switch_session(&other_file, None).await.unwrap();
    assert_eq!(
        Path::new(t.runtime.session().session_manager().cwd())
            .canonicalize()
            .unwrap(),
        second_dir
    );
    assert_eq!(t.runtime.cwd().canonicalize().unwrap(), second_dir);
    other.dispose().await;
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restores_model_and_thinking_state_from_the_destination_session() {
    let mut t = create_runtime(Options {
        bootstrap_model: Some(false),
        ..Default::default()
    })
    .await;
    let other_dir = t.dir().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let mut other = other_runtime(&t, &other_dir).await;
    other
        .session()
        .set_model(t.faux.get_model_by_id("faux-2").unwrap())
        .unwrap();
    other.session().set_thinking_level(ThinkingLevel::Off);
    other
        .session()
        .prompt("hello", PromptOptions::default())
        .await
        .unwrap();
    let target = other.session().session_file().unwrap();

    t.runtime.switch_session(&target, None).await.unwrap();
    assert_eq!(t.runtime.session().model().unwrap().id, "faux-2");
    assert_eq!(t.runtime.session().thinking_level(), ThinkingLevel::Off);
    other.dispose().await;
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_before_session_invalidate_after_session_shutdown_and_before_rebind_session() {
    let phases: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let p0 = phases.clone();
    let mut t = create_runtime(Options {
        handles: vec!["session_shutdown"],
        cancel: Some(Box::new(move |event| {
            if matches!(event, SessionEvent::Shutdown { .. }) {
                p0.lock().unwrap().push("session_shutdown");
            }
            false
        })),
        ..Default::default()
    })
    .await;
    let old_session = t.runtime.session().clone();
    let (p1, p2) = (phases.clone(), phases.clone());
    t.runtime
        .set_before_session_invalidate(Some(Box::new(move || {
            p1.lock().unwrap().push("beforeSessionInvalidate");
        })));
    t.runtime.set_rebind_session(Some(Box::new(move |_| {
        p2.lock().unwrap().push("rebindSession");
    })));

    t.runtime
        .new_session(NewSessionRequest::default())
        .await
        .unwrap();
    assert_eq!(
        *phases.lock().unwrap(),
        [
            "session_shutdown",
            "beforeSessionInvalidate",
            "rebindSession"
        ]
    );
    assert_ne!(old_session.session_id(), t.runtime.session().session_id());
    t.runtime.set_before_session_invalidate(None);
    t.runtime.set_rebind_session(None);
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_emits_session_shutdown_and_keeps_the_active_tools() {
    let t = create_runtime(Options {
        handles: vec!["session_shutdown"],
        ..Default::default()
    })
    .await;
    let tools = t.runtime.session().get_active_tool_names();
    t.runtime.session().reload().await;
    assert_eq!(
        t.events(),
        [Recorded::Shutdown(SessionShutdownReason::Reload, None)]
    );
    assert_eq!(t.runtime.session().get_active_tool_names(), tools);
    t.finish().await;
}

// ---- test/agent-session-branching.test.ts ----

#[tokio::test(flavor = "multi_thread")]
async fn should_allow_forking_from_single_message() {
    let mut t = create_runtime(Options::default()).await;
    t.prompt("Say hello").await;
    let user_messages = t.runtime.session().get_user_messages_for_forking();
    assert_eq!(user_messages.len(), 1);
    assert_eq!(user_messages[0].text, "Say hello");

    let result = t
        .runtime
        .fork(&user_messages[0].entry_id, ForkPosition::Before)
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.selected_text.as_deref(), Some("Say hello"));
    assert!(t.runtime.session().messages().is_empty());
    let file = t.session_file().unwrap();
    assert!(!Path::new(&file).exists());
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn should_support_in_memory_forking_in_no_session_mode() {
    let mut t = create_runtime(Options {
        in_memory: true,
        ..Default::default()
    })
    .await;
    assert!(t.session_file().is_none());
    t.prompt("Say hi").await;
    let user_messages = t.runtime.session().get_user_messages_for_forking();
    assert_eq!(user_messages.len(), 1);
    assert!(!t.runtime.session().messages().is_empty());

    let result = t
        .runtime
        .fork(&user_messages[0].entry_id, ForkPosition::Before)
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.selected_text.as_deref(), Some("Say hi"));
    assert!(t.runtime.session().messages().is_empty());
    assert!(t.session_file().is_none());
    t.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn should_fork_from_middle_of_conversation() {
    let mut t = create_runtime(Options::default()).await;
    t.prompt("Say one").await;
    t.prompt("Say two").await;
    t.prompt("Say three").await;
    let user_messages = t.runtime.session().get_user_messages_for_forking();
    assert_eq!(user_messages.len(), 3);

    let result = t
        .runtime
        .fork(&user_messages[1].entry_id, ForkPosition::Before)
        .await
        .unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.selected_text.as_deref(), Some("Say two"));
    let roles: Vec<String> = message_shape(&t.runtime)
        .into_iter()
        .map(|(role, _)| role)
        .collect();
    assert_eq!(roles, ["user", "assistant"]);
    t.finish().await;
}

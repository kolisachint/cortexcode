//! RPC session commands on [`RuntimeHost`]: `new_session`, `switch_session`,
//! `fork` and `clone` replace the runtime's session and RPC mode follows it
//! (rpc.test.ts "should create new session", on the faux provider; the other
//! commands' semantics come from rpc-mode.ts and agent-session-runtime.ts).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use cortexcode_ai_provider_faux::{
    faux_assistant_message, register_faux_provider, FauxProvider, FauxProviderRegistration,
    FauxResponseStep, RegisterFauxProviderOptions,
};
use cortexcode_code_agent_session::{
    create_agent_session, create_agent_session_runtime, AgentSessionServices, BaseTools,
    CreateAgentSessionOptions, CreatedRuntime, RuntimeFactory, RuntimeRequest,
    StaticResourceLoader,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_rpc::client::{RpcClient, RpcClientOptions};
use cortexcode_code_rpc::{run_rpc_mode, serialize_json_line, RuntimeHost};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

/// Keep default session dirs (and the agent dir) out of the real home.
fn isolate_agent_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-rpc-host-test-{}", std::process::id()));
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

/// Every runtime gets a session on the faux model, in-memory settings.
fn factory(faux: Arc<FauxProvider>, agent_dir: PathBuf) -> RuntimeFactory {
    Arc::new(move |request: RuntimeRequest| {
        let faux = faux.clone();
        let agent_dir = agent_dir.clone();
        Box::pin(async move {
            let model = faux.get_model();
            let services = AgentSessionServices {
                cwd: request.cwd.clone(),
                agent_dir,
                auth: Arc::new(FauxAuth(model.provider.clone())),
                settings: Arc::new(Mutex::new(SettingsManager::in_memory(Default::default()))),
                model_registry: Arc::new(ModelRegistry::in_memory()),
                resource_loader: Arc::new(StaticResourceLoader::default()),
                diagnostics: vec![],
            };
            let created = create_agent_session(
                &services,
                request.session_manager,
                CreateAgentSessionOptions {
                    model: Some(model),
                    base_tools: Some(BaseTools::Override(vec![])),
                    stream_fn: Some(Arc::new(faux.stream_fn())),
                    session_start_event: request.session_start_event,
                    ..Default::default()
                },
            );
            Ok(CreatedRuntime {
                session: created.session,
                diagnostics: vec![],
                services,
                model_fallback_message: created.model_fallback_message,
            })
        })
    })
}

struct Setup {
    client: RpcClient,
    _registration: FauxProviderRegistration,
    _temp: tempfile::TempDir,
}

/// RPC mode on a runtime with a persisted session, a client attached over pipes.
async fn setup(replies: &[&str]) -> Setup {
    isolate_agent_dir();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let registration = register_faux_provider(RegisterFauxProviderOptions::default());
    let faux = registration.provider().clone();
    faux.set_responses(
        replies
            .iter()
            .map(|t| FauxResponseStep::Message(faux_assistant_message(*t, Default::default())))
            .collect(),
    );
    let session_manager = SessionManager::create(dir.to_string_lossy(), Some(dir.join("sessions")));
    let runtime = create_agent_session_runtime(
        factory(faux, dir.clone()),
        RuntimeRequest {
            cwd: dir.clone(),
            agent_dir: dir,
            session_manager,
            session_start_event: None,
        },
    )
    .await
    .unwrap();

    let (client_writer, agent_reader) = tokio::io::duplex(1 << 16);
    let (agent_writer, client_reader) = tokio::io::duplex(1 << 16);
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut agent_writer = agent_writer;
        while let Some(line) = line_rx.recv().await {
            if agent_writer.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(run_rpc_mode(
        Arc::new(RuntimeHost::new(runtime)),
        agent_reader,
        Arc::new(move |value: &Value| {
            let _ = line_tx.send(serialize_json_line(value));
        }),
    ));
    let client = RpcClient::attach(RpcClientOptions::default(), client_reader, client_writer);
    Setup {
        client,
        _registration: registration,
        _temp: temp,
    }
}

const WAIT: Duration = Duration::from_secs(10);

async fn state(s: &Setup) -> Value {
    s.client.get_state().await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn should_create_new_session() {
    let s = setup(&["hi", "again"]).await;
    s.client.prompt_and_wait("Hello", None, WAIT).await.unwrap();
    let before = state(&s).await;
    assert_eq!(before["messageCount"], 2);

    let result = s.client.new_session(None).await.unwrap();
    assert_eq!(result, serde_json::json!({"cancelled": false}));
    let after = state(&s).await;
    assert_eq!(after["messageCount"], 0);
    assert_ne!(after["sessionFile"], before["sessionFile"]);
    assert_ne!(after["sessionId"], before["sessionId"]);

    // Events come from the new session.
    let events = s.client.prompt_and_wait("Again", None, WAIT).await.unwrap();
    assert_eq!(events.last().unwrap()["type"], "agent_end");
    assert_eq!(
        s.client.get_last_assistant_text().await.unwrap().as_deref(),
        Some("again")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn switch_session_reopens_an_earlier_session() {
    let s = setup(&["first reply"]).await;
    s.client.prompt_and_wait("First", None, WAIT).await.unwrap();
    let first = state(&s).await;
    s.client.new_session(None).await.unwrap();
    assert_eq!(state(&s).await["messageCount"], 0);

    let result = s
        .client
        .switch_session(first["sessionFile"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(result, serde_json::json!({"cancelled": false}));
    let switched = state(&s).await;
    assert_eq!(switched["sessionFile"], first["sessionFile"]);
    assert_eq!(switched["messageCount"], 2);
    assert_eq!(
        s.client.get_last_assistant_text().await.unwrap().as_deref(),
        Some("first reply")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fork_branches_before_a_user_message_and_returns_its_text() {
    let s = setup(&["one", "two"]).await;
    s.client.prompt_and_wait("First", None, WAIT).await.unwrap();
    s.client
        .prompt_and_wait("Second", None, WAIT)
        .await
        .unwrap();
    let original = state(&s).await;
    let forkable = s.client.get_fork_messages().await.unwrap();
    assert_eq!(forkable.len(), 2);
    assert_eq!(forkable[1]["text"], "Second");

    let result = s
        .client
        .fork(forkable[1]["entryId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        result,
        serde_json::json!({"text": "Second", "cancelled": false})
    );
    let forked = state(&s).await;
    assert_eq!(forked["messageCount"], 2);
    assert_ne!(forked["sessionFile"], original["sessionFile"]);

    let err = s.client.fork("missing").await.unwrap_err();
    assert_eq!(err.0, "Invalid entry ID for forking");
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_duplicates_the_current_branch() {
    let s = setup(&["one"]).await;
    // A fresh session's leaf (its model change) is not on disk yet, so the
    // persisted fork cannot find it (hoocode answers the same).
    let err = s.client.clone_session().await.unwrap_err();
    assert!(
        err.0.starts_with("Entry ") && err.0.ends_with(" not found"),
        "{err}"
    );

    s.client.prompt_and_wait("First", None, WAIT).await.unwrap();
    let original = state(&s).await;
    let result = s.client.clone_session().await.unwrap();
    assert_eq!(result, serde_json::json!({"cancelled": false}));
    let cloned = state(&s).await;
    assert_eq!(cloned["messageCount"], 2);
    assert_ne!(cloned["sessionFile"], original["sessionFile"]);
    assert_eq!(
        s.client.get_last_assistant_text().await.unwrap().as_deref(),
        Some("one")
    );
}

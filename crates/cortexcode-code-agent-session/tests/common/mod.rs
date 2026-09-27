//! `test/suite/harness.ts`: an AgentSession on the faux provider.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use cortexcode_agent_core::{Agent, AgentOptions};
use cortexcode_agent_types::{AgentMessage, AgentState, AgentToolResult, AgentTools};
use cortexcode_ai_provider_faux::{
    register_faux_provider, FauxModelDefinition, FauxProvider, FauxProviderRegistration,
    FauxResponseStep, RegisterFauxProviderOptions,
};
use cortexcode_ai_types::{Content, Message};
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionConfig, AgentSessionEvent, BaseTools, ExtensionHooks, ResourceLoader,
    SessionSubscription, StaticResourceLoader,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::{ToolDefinition, ToolError};

/// Auth for the given providers only.
pub struct TestAuth(pub Vec<String>);

impl AuthLookup for TestAuth {
    fn api_key(&self, provider: &str) -> Option<String> {
        self.0
            .iter()
            .any(|p| p == provider)
            .then(|| "faux-key".to_string())
    }
}

/// Builds the session manager from the harness temp dir.
pub type MakeSessionManager = Box<dyn FnOnce(&std::path::Path) -> SessionManager>;

#[derive(Default)]
pub struct HarnessOptions {
    pub tools: Vec<ToolDefinition>,
    pub custom_tools: Vec<ToolDefinition>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub resource_loader: Option<Arc<dyn ResourceLoader>>,
    pub extensions: Option<Arc<dyn ExtensionHooks>>,
    pub without_auth: bool,
    pub models: Vec<FauxModelDefinition>,
    pub allowed_tool_names: Option<Vec<String>>,
    pub disallowed_tool_names: Option<Vec<String>>,
    pub initial_active_tool_names: Option<Vec<String>>,
    /// Defaults to an in-memory manager rooted at the temp dir.
    pub session_manager: Option<MakeSessionManager>,
}

pub struct Harness {
    pub session: AgentSession,
    pub faux: Arc<FauxProvider>,
    pub events: Arc<Mutex<Vec<AgentSessionEvent>>>,
    pub temp_dir: tempfile::TempDir,
    _subscription: SessionSubscription,
    registration: FauxProviderRegistration,
}

impl Harness {
    pub fn new(options: HarnessOptions) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        // Registered on the API registry so compaction summaries reach it too.
        let registration = register_faux_provider(RegisterFauxProviderOptions {
            models: options.models,
            ..Default::default()
        });
        let faux = registration.provider().clone();
        let model = faux.get_model();
        let agent = Arc::new(Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: "You are a test assistant.".into(),
                model: model.clone(),
                thinking_level: cortexcode_ai_types::ThinkingLevel::Off,
                tools: AgentTools::new(vec![]),
                messages: vec![],
                is_streaming: false,
                streaming_message: None,
                pending_tool_calls: Default::default(),
                error_message: None,
            }),
            stream_fn: Some(Arc::new(faux.stream_fn())),
            convert_to_llm: Some(Arc::new(|messages: Vec<AgentMessage>| {
                Ok(cortexcode_agent_harness::messages::convert_to_llm(
                    &messages,
                ))
            })),
            api_key: Some("faux-key".into()),
            ..Default::default()
        }));
        let auth = if options.without_auth {
            TestAuth(vec![])
        } else {
            TestAuth(vec![model.provider.clone()])
        };
        let session = AgentSession::new(AgentSessionConfig {
            agent,
            session_manager: match options.session_manager {
                Some(make) => make(temp_dir.path()),
                None => SessionManager::in_memory(temp_dir.path().to_string_lossy()),
            },
            settings: Arc::new(Mutex::new(SettingsManager::in_memory(options.settings))),
            cwd: temp_dir.path().to_path_buf(),
            scoped_models: vec![],
            resource_loader: options
                .resource_loader
                .unwrap_or_else(|| Arc::new(StaticResourceLoader::default())),
            custom_tools: options.custom_tools,
            model_registry: Arc::new(ModelRegistry::in_memory()),
            auth: Arc::new(auth),
            initial_active_tool_names: options.initial_active_tool_names,
            allowed_tool_names: options.allowed_tool_names,
            disallowed_tool_names: options.disallowed_tool_names,
            base_tools: BaseTools::Override(options.tools),
            extensions: options.extensions,
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let subscription = session.subscribe(move |event| sink.lock().unwrap().push(event.clone()));
        Self {
            session,
            faux,
            events,
            temp_dir,
            _subscription: subscription,
            registration,
        }
    }

    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        self.faux.set_responses(responses);
    }

    pub fn pending_response_count(&self) -> usize {
        self.faux.get_pending_response_count()
    }

    pub fn roles(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .map(|m| m.role().to_string())
            .collect()
    }

    pub fn user_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .filter_map(|m| match m {
                AgentMessage::User(u) => Some(text_of(&u.content.blocks())),
                _ => None,
            })
            .collect()
    }

    pub fn assistant_texts(&self) -> Vec<String> {
        self.session
            .messages()
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Assistant(a) => Some(text_of(&a.content)),
                _ => None,
            })
            .collect()
    }

    pub fn entry_types(&self) -> Vec<String> {
        self.session
            .session_manager()
            .entries()
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.session.dispose();
        self.registration.unregister();
    }
}

/// `getMessageText`: text blocks joined with newlines.
pub fn text_of(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text of an LLM message.
pub fn message_text(message: &Message) -> String {
    match message {
        Message::User(u) => text_of(&u.content.blocks()),
        Message::Assistant(a) => text_of(&a.content),
        Message::ToolResult(t) => text_of(&t.content),
    }
}

/// A tool from a plain function of its arguments.
pub fn tool<F>(name: &str, execute: F) -> ToolDefinition
where
    F: Fn(serde_json::Value) -> AgentToolResult + Send + Sync + 'static,
{
    ToolDefinition {
        name: name.into(),
        label: name.into(),
        description: format!("{name} tool"),
        prompt_snippet: None,
        prompt_guidelines: vec![],
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, params, _signal, _update, _ctx| {
            Ok::<_, ToolError>(execute(params))
        }),
    }
}

pub fn text_result(text: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::text(text)],
        details: serde_json::json!({}),
        terminate: false,
    }
}

/// `createWaitingHarness`'s `wait` tool: blocks until released.
pub struct WaitTool {
    pub definition: ToolDefinition,
    release: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl WaitTool {
    pub fn new() -> Self {
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let gate = release.clone();
        let definition = tool("wait", move |_| {
            let (lock, cvar) = &*gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cvar.wait(released).unwrap();
            }
            text_result("released")
        });
        Self {
            definition,
            release,
        }
    }

    pub fn release(&self) {
        let (lock, cvar) = &*self.release;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }
}

/// Resolves once the session emits `tool_execution_start` for `tool_name`.
pub fn wait_for_tool_start(
    session: &AgentSession,
    tool_name: &str,
) -> (tokio::sync::oneshot::Receiver<()>, SessionSubscription) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let name = tool_name.to_string();
    let subscription = session.subscribe(move |event| {
        if let AgentSessionEvent::Agent(cortexcode_agent_types::AgentEvent::ToolExecutionStart {
            tool_name,
            ..
        }) = event
        {
            if *tool_name == name {
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        }
    });
    (rx, subscription)
}

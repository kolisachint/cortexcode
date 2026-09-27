//! rpc-prompt-response-semantics.test.ts: the `prompt` command answers once,
//! after preflight, whether the prompt was sent, queued or rejected.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cortexcode_agent_core::{Agent, AgentOptions};
use cortexcode_agent_types::{AgentMessage, AgentState, AgentTools};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, register_faux_provider, FauxProviderRegistration, FauxResponseStep,
    RegisterFauxProviderOptions,
};
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionConfig, BaseTools, StaticResourceLoader,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_rpc::{RpcMode, SingleSessionHost};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use serde_json::{json, Value};

struct TestAuth(Option<String>);

impl AuthLookup for TestAuth {
    fn api_key(&self, provider: &str) -> Option<String> {
        (self.0.as_deref() == Some(provider)).then(|| "test-key".to_string())
    }
}

struct Rpc {
    mode: Arc<RpcMode>,
    lines: Arc<Mutex<Vec<Value>>>,
    session: AgentSession,
    _registration: FauxProviderRegistration,
    _temp: tempfile::TempDir,
}

impl Rpc {
    /// `startRpcMode({ withAuth, responseDelayMs })` on the faux provider.
    fn start(with_auth: bool, response_delay: Duration) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let registration = register_faux_provider(RegisterFauxProviderOptions::default());
        let faux = registration.provider().clone();
        let model = faux.get_model();
        faux.set_responses(
            (0..3)
                .map(|_| {
                    FauxResponseStep::async_factory(move |_, _, _, _| async move {
                        tokio::time::sleep(response_delay).await;
                        Ok(faux_assistant_message("done", Default::default()))
                    })
                })
                .collect(),
        );
        let agent = Arc::new(Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: "Test".into(),
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
            api_key: Some("test-key".into()),
            ..Default::default()
        }));
        let session = AgentSession::new(AgentSessionConfig {
            agent,
            session_manager: SessionManager::in_memory(temp.path().to_string_lossy()),
            settings: Arc::new(Mutex::new(SettingsManager::in_memory(Default::default()))),
            cwd: temp.path().to_path_buf(),
            scoped_models: vec![],
            resource_loader: Arc::new(StaticResourceLoader::default()),
            custom_tools: vec![],
            model_registry: Arc::new(ModelRegistry::in_memory()),
            auth: Arc::new(TestAuth(with_auth.then(|| model.provider.clone()))),
            initial_active_tool_names: None,
            allowed_tool_names: None,
            disallowed_tool_names: None,
            base_tools: BaseTools::Override(vec![]),
            extensions: None,
            session_start_event: None,
        });
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        let mode = RpcMode::new(
            Arc::new(SingleSessionHost::new(session.clone())),
            Arc::new(move |value: &Value| sink.lock().unwrap().push(value.clone())),
        );
        Self {
            mode,
            lines,
            session,
            _registration: registration,
            _temp: temp,
        }
    }

    async fn send(&self, command: Value) {
        self.mode.handle_line(&command.to_string()).await;
    }

    fn prompt_responses(&self, id: &str) -> Vec<Value> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter(|v| v["id"] == id && v["type"] == "response" && v["command"] == "prompt")
            .cloned()
            .collect()
    }

    /// `vi.waitFor`: poll until `n` prompt responses for `id` arrived.
    async fn wait_for_prompt_responses(&self, id: &str, n: usize) -> Vec<Value> {
        for _ in 0..200 {
            let responses = self.prompt_responses(id);
            if responses.len() >= n {
                return responses;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.prompt_responses(id)
    }

    async fn cleanup(self) {
        if self.session.is_streaming() {
            self.session.abort().await;
        }
        self.session.dispose();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_one_failure_response_when_prompt_preflight_rejects() {
    let rpc = Rpc::start(false, Duration::ZERO);
    rpc.send(json!({"id": "b1", "type": "prompt", "message": "Hello"}))
        .await;
    let responses = rpc.wait_for_prompt_responses("b1", 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rpc.prompt_responses("b1").len(), 1);
    let response = &responses[0];
    assert_eq!(response["success"], false);
    let error = response["error"].as_str().unwrap();
    assert!(
        error.contains("\n\nUse /login to log into a provider via OAuth or API key. See:"),
        "{error}"
    );
    assert!(error.starts_with("No API key found for "), "{error}");
    rpc.cleanup().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_one_success_response_when_prompt_preflight_succeeds() {
    let rpc = Rpc::start(true, Duration::ZERO);
    rpc.send(json!({"id": "b2", "type": "prompt", "message": "Hello"}))
        .await;
    let responses = rpc.wait_for_prompt_responses("b2", 1).await;
    assert_eq!(
        responses,
        [json!({"id": "b2", "type": "response", "command": "prompt", "success": true})]
    );
    rpc.cleanup().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_one_success_response_when_prompt_is_queued_during_streaming() {
    let rpc = Rpc::start(true, Duration::from_millis(100));
    rpc.send(json!({"id": "b3-start", "type": "prompt", "message": "Start"}))
        .await;
    assert_eq!(rpc.wait_for_prompt_responses("b3-start", 1).await.len(), 1);

    rpc.lines.lock().unwrap().clear();
    rpc.send(json!({
        "id": "b3",
        "type": "prompt",
        "message": "Queue this",
        "streamingBehavior": "followUp",
    }))
    .await;
    let responses = rpc.wait_for_prompt_responses("b3", 1).await;
    assert_eq!(
        responses,
        [json!({"id": "b3", "type": "response", "command": "prompt", "success": true})]
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(rpc.prompt_responses("b3").len(), 1);
    rpc.cleanup().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn responses_echo_ids_and_unknown_commands_have_none() {
    let rpc = Rpc::start(true, Duration::ZERO);
    rpc.send(json!({"id": 7, "type": "set_steering_mode", "mode": "all"}))
        .await;
    rpc.send(json!({"id": "x", "type": "nope"})).await;
    rpc.mode.handle_line("not json").await;
    rpc.send(json!({"type": "set_session_name", "name": "  "}))
        .await;
    let lines = rpc.lines.lock().unwrap().clone();
    assert_eq!(
        lines[0],
        json!({"id": 7, "type": "response", "command": "set_steering_mode", "success": true})
    );
    assert_eq!(
        lines[1],
        json!({"type": "response", "command": "nope", "success": false, "error": "Unknown command: nope"})
    );
    assert_eq!(lines[2]["command"], "parse");
    assert!(lines[2]["error"]
        .as_str()
        .unwrap()
        .starts_with("Failed to parse command: "));
    assert_eq!(
        lines[3],
        json!({"type": "response", "command": "set_session_name", "success": false, "error": "Session name cannot be empty"})
    );
    rpc.cleanup().await;
}

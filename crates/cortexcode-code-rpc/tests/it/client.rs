//! rpc-client-clone.test.ts and rpc.test.ts: the client against RPC mode.
//!
//! rpc.test.ts runs against a live Anthropic model; here the client drives
//! [`run_rpc_mode`] in-process over a pipe, on the faux provider.

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
use cortexcode_code_rpc::client::{get_data, RpcClient, RpcClientOptions};
use cortexcode_code_rpc::{run_rpc_mode, serialize_json_line, SingleSessionHost};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TestAuth(String);

impl AuthLookup for TestAuth {
    fn api_key(&self, provider: &str) -> Option<String> {
        (provider == self.0).then(|| "test-key".to_string())
    }
}

struct Setup {
    client: RpcClient,
    session: AgentSession,
    _registration: FauxProviderRegistration,
    _temp: tempfile::TempDir,
}

/// An RPC-mode session on the faux provider, with a client attached over pipes.
fn setup(replies: &[&str]) -> Setup {
    let temp = tempfile::tempdir().unwrap();
    let registration = register_faux_provider(RegisterFauxProviderOptions::default());
    let faux = registration.provider().clone();
    let model = faux.get_model();
    faux.set_responses(
        replies
            .iter()
            .map(|t| FauxResponseStep::Message(faux_assistant_message(*t, Default::default())))
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
        auth: Arc::new(TestAuth(model.provider.clone())),
        initial_active_tool_names: None,
        allowed_tool_names: None,
        disallowed_tool_names: None,
        base_tools: BaseTools::Override(vec![]),
        extensions: None,
        session_start_event: None,
    });

    // client stdin -> agent input; agent output -> client stdout.
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
    let host = Arc::new(SingleSessionHost::new(session.clone()));
    tokio::spawn(run_rpc_mode(
        host,
        agent_reader,
        Arc::new(move |value: &Value| {
            let _ = line_tx.send(serialize_json_line(value));
        }),
    ));
    let client = RpcClient::attach(RpcClientOptions::default(), client_reader, client_writer);
    Setup {
        client,
        session,
        _registration: registration,
        _temp: temp,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn should_get_state() {
    let s = setup(&[]);
    let state = s.client.get_state().await.unwrap();
    assert_eq!(
        state["model"]["provider"],
        s.session.model().unwrap().provider
    );
    assert_eq!(state["isStreaming"], false);
    assert_eq!(state["messageCount"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_and_wait_collects_events_up_to_agent_end() {
    let s = setup(&["hello"]);
    let events = s
        .client
        .prompt_and_wait(
            "Reply with just the word 'hello'",
            None,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let message_ends = events.iter().filter(|e| e["type"] == "message_end").count();
    assert!(message_ends >= 2, "user + assistant: {events:?}");
    assert_eq!(events.last().unwrap()["type"], "agent_end");
    assert_eq!(
        s.client.get_last_assistant_text().await.unwrap().as_deref(),
        Some("hello")
    );
    let messages = s.client.get_messages().await.unwrap();
    assert_eq!(messages.len(), 2);
    let stats = s.client.get_session_stats().await.unwrap();
    assert_eq!(stats["userMessages"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn commands_round_trip_and_errors_reject() {
    let s = setup(&[]);
    s.client.set_steering_mode("all").await.unwrap();
    s.client.set_follow_up_mode("all").await.unwrap();
    s.client.set_auto_compaction(false).await.unwrap();
    let state = s.client.get_state().await.unwrap();
    assert_eq!(state["steeringMode"], "all");
    assert_eq!(state["followUpMode"], "all");
    assert_eq!(state["autoCompactionEnabled"], false);

    s.client.set_session_name("my session").await.unwrap();
    assert_eq!(
        s.client.get_state().await.unwrap()["sessionName"],
        "my session"
    );
    let err = s.client.set_session_name("  ").await.unwrap_err();
    assert_eq!(err.0, "Session name cannot be empty");

    let err = s.client.set_model("nope", "missing").await.unwrap_err();
    assert_eq!(err.0, "Model not found: nope/missing");
    assert!(s.client.get_fork_messages().await.unwrap().is_empty());
    assert_eq!(s.client.get_last_assistant_text().await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn bash_runs_through_the_session() {
    let s = setup(&[]);
    let result = s.client.bash("echo hi").await.unwrap();
    assert_eq!(result["output"], "hi\n");
    assert_eq!(result["exitCode"], 0);
    assert_eq!(result["cancelled"], false);
}

/// rpc-client-clone.test.ts: `clone()` sends `{"type":"clone"}` and returns `data`.
#[tokio::test(flavor = "multi_thread")]
async fn sends_the_clone_rpc_command() {
    let (client_writer, mut agent_reader) = tokio::io::duplex(4096);
    let (mut agent_writer, client_reader) = tokio::io::duplex(4096);
    let client = RpcClient::attach(RpcClientOptions::default(), client_reader, client_writer);
    let agent = tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let n = agent_reader.read(&mut buf).await.unwrap();
        let command: Value = serde_json::from_slice(&buf[..n]).unwrap();
        let response = json!({
            "id": command["id"],
            "type": "response",
            "command": "clone",
            "success": true,
            "data": {"cancelled": false},
        });
        agent_writer
            .write_all(serialize_json_line(&response).as_bytes())
            .await
            .unwrap();
        command
    });
    let result = client.clone_session().await.unwrap();
    assert_eq!(result, json!({"cancelled": false}));
    assert_eq!(
        agent.await.unwrap(),
        json!({"type": "clone", "id": "req_1"})
    );
}

#[test]
fn get_data_returns_data_or_the_error() {
    assert_eq!(
        get_data(&json!({"type": "response", "success": true, "data": {"a": 1}})).unwrap(),
        json!({"a": 1})
    );
    assert_eq!(
        get_data(&json!({"type": "response", "success": false, "error": "boom"}))
            .unwrap_err()
            .0,
        "boom"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_agent_fails_pending_and_later_requests() {
    let (client_writer, agent_reader) = tokio::io::duplex(4096);
    let (agent_writer, client_reader) = tokio::io::duplex(4096);
    let client = RpcClient::attach(RpcClientOptions::default(), client_reader, client_writer);
    drop(agent_reader);
    drop(agent_writer);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let err = client.get_state().await.unwrap_err();
    assert!(err.0.starts_with("Agent process has exited"), "{err}");
    let err = client
        .wait_for_idle(Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(err.0.starts_with("Agent process already exited"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn start_reports_a_process_that_exits_immediately() {
    let mut client = RpcClient::new(RpcClientOptions {
        executable: Some("sh".into()),
        prefix_args: vec!["-c".into(), "echo boom >&2; exit 3".into(), "sh".into()],
        ..Default::default()
    });
    let err = client.start().await.unwrap_err();
    assert!(
        err.0
            .starts_with("Agent process exited immediately with code 3"),
        "{err}"
    );
    assert!(err.0.contains("boom"), "{err}");
    let mut missing = RpcClient::new(RpcClientOptions {
        executable: Some("/nonexistent/cortex".into()),
        ..Default::default()
    });
    assert!(missing.start().await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn spawned_agent_answers_over_stdio_and_stops() {
    // A stand-in agent: answers every command with success, echoing its id.
    let script = r#"while IFS= read -r line; do id=$(printf '%s' "$line" | sed 's/.*"id":"\([^"]*\)".*/\1/'); printf '{"id":"%s","type":"response","command":"get_state","success":true,"data":{"isStreaming":false}}\n' "$id"; done"#;
    let mut client = RpcClient::new(RpcClientOptions {
        executable: Some("sh".into()),
        prefix_args: vec!["-c".into(), script.into(), "sh".into()],
        ..Default::default()
    });
    client.start().await.unwrap();
    let state = client.get_state().await.unwrap();
    assert_eq!(state, json!({"isStreaming": false}));
    client.stop().await;
    assert!(client.get_state().await.is_err());
}

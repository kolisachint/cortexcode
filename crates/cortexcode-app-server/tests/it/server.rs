//! The app-server end to end on the faux provider: a client talks JSON over
//! an in-memory line transport, exactly as over stdio.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use cortexcode_agent_types::{AgentToolResult, PermissionGate};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_tool_call, register_faux_provider, FauxProvider,
    FauxProviderRegistration, FauxResponseStep, RegisterFauxProviderOptions,
};
use cortexcode_ai_types::{Content, StopReason};
use cortexcode_app_server::{AppServer, SavedSession, ServerConfig, SessionFactory};
use cortexcode_code_agent_session::{
    create_agent_session, AgentSession, AgentSessionServices, BaseTools, CreateAgentSessionOptions,
    StaticResourceLoader,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_tool_api::ToolDefinition;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(10);

/// Every message any client received: `(request method for responses, message)`.
static RECORDED: Mutex<Vec<(Option<String>, Value)>> = Mutex::new(Vec::new());

/// Request id → method, for tagging responses.
static METHODS: Mutex<Vec<(i64, String)>> = Mutex::new(Vec::new());

fn record(value: &Value) {
    let method = value["id"].as_i64().and_then(|id| {
        METHODS
            .lock()
            .unwrap()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, m)| m.clone())
    });
    let method = if value.get("method").is_some() {
        None
    } else {
        method
    };
    RECORDED.lock().unwrap().push((method, value.clone()));
}

/// Keep the global hoo-config (and anything else in the agent dir) out of
/// the real home. Shared by all tests in this binary.
fn isolate_agent_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir =
            std::env::temp_dir().join(format!("cortex-app-server-test-{}", std::process::id()));
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

/// A `bash` tool that doesn't run anything: it echoes its command.
fn fake_bash() -> ToolDefinition {
    ToolDefinition {
        ordered_start: false,
        background_when: None,
        name: "bash".into(),
        label: "bash".into(),
        description: "bash tool".into(),
        prompt_snippet: None,
        prompt_guidelines: vec![],
        parameters: json!({"type": "object", "properties": {"command": {"type": "string"}}}),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(|_id, params, _signal, _update, _ctx| {
            let command = params["command"].as_str().unwrap_or_default().to_string();
            Ok(AgentToolResult {
                content: vec![Content::text(format!("ran {command}"))],
                details: json!({}),
                terminate: false,
            })
        }),
    }
}

struct Factory {
    faux: Arc<FauxProvider>,
    dir: PathBuf,
}

impl Factory {
    fn session(&self, manager: SessionManager, gate: Arc<dyn PermissionGate>) -> AgentSession {
        let model = self.faux.get_model();
        let services = AgentSessionServices {
            cwd: self.dir.clone(),
            agent_dir: self.dir.clone(),
            auth: Arc::new(FauxAuth(model.provider.clone())),
            settings: Arc::new(Mutex::new(SettingsManager::in_memory(Default::default()))),
            model_registry: Arc::new(ModelRegistry::in_memory()),
            resource_loader: Arc::new(StaticResourceLoader::default()),
            diagnostics: vec![],
        };
        create_agent_session(
            &services,
            manager,
            CreateAgentSessionOptions {
                model: Some(model),
                base_tools: Some(BaseTools::Override(vec![])),
                custom_tools: vec![fake_bash()],
                stream_fn: Some(Arc::new(self.faux.stream_fn())),
                permission_gate: Some(gate),
                ..Default::default()
            },
        )
        .session
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }
}

impl SessionFactory for Factory {
    fn create(
        &self,
        _model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String> {
        let manager = SessionManager::create(self.dir.to_string_lossy(), Some(self.sessions_dir()));
        Ok(self.session(manager, gate))
    }

    fn open(
        &self,
        path: &Path,
        _model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String> {
        let manager = SessionManager::open(path, Some(self.sessions_dir()), None);
        Ok(self.session(manager, gate))
    }

    fn models(&self) -> Vec<(String, String, bool, bool)> {
        vec![
            ("p/in-scope".into(), "In scope".into(), true, false),
            ("p/out-of-scope".into(), "Out of scope".into(), false, true),
        ]
    }

    fn list(&self) -> Vec<SavedSession> {
        let mut out: Vec<SavedSession> =
            cortexcode_code_session::manager::list_sessions(self.sessions_dir(), None)
                .unwrap_or_default()
                .into_iter()
                .map(|s| SavedSession {
                    id: s.id,
                    path: s.path,
                    cwd: s.cwd,
                    name: s.name,
                    preview: s.first_message,
                    created: s.created.timestamp(),
                    modified: s.modified.timestamp(),
                })
                .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.modified));
        out
    }
}

/// One client over an in-memory line transport.
struct Client {
    tx: tokio::io::DuplexStream,
    rx: mpsc::UnboundedReceiver<Value>,
    next_id: i64,
    /// Messages read while waiting for something else.
    backlog: Vec<Value>,
}

impl Client {
    fn connect(server: &AppServer) -> Self {
        let (client_side, server_side) = tokio::io::duplex(1 << 20);
        let (server_read, server_write) = tokio::io::split(server_side);
        tokio::spawn(cortexcode_app_server::transport::serve_lines(
            server.handler(),
            server_read,
            server_write,
        ));
        let (client_read, client_write) = tokio::io::split(client_side);
        let (line_tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(client_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let value: Value = serde_json::from_str(&line).expect("server sent invalid JSON");
                assert!(
                    value.get("jsonrpc").is_none(),
                    "no jsonrpc field on the wire"
                );
                record(&value);
                if line_tx.send(value).is_err() {
                    break;
                }
            }
        });
        // Re-join the halves the test writes on.
        let (tx, mut pipe_out) = tokio::io::duplex(1 << 20);
        let mut client_write = client_write;
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut pipe_out, &mut client_write).await;
        });
        Client {
            tx,
            rx,
            next_id: 100,
            backlog: vec![],
        }
    }

    async fn send(&mut self, value: Value) {
        let mut line = serde_json::to_string(&value).unwrap();
        line.push('\n');
        self.tx.write_all(line.as_bytes()).await.unwrap();
    }

    /// The next message matching `pred` (earlier non-matching ones are kept).
    async fn next_where(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(pos) = self.backlog.iter().position(&pred) {
            return self.backlog.remove(pos);
        }
        loop {
            let value = tokio::time::timeout(WAIT, self.rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out waiting for {what}; backlog: {:#?}", self.backlog)
                })
                .expect("server closed");
            if pred(&value) {
                return value;
            }
            self.backlog.push(value);
        }
    }

    async fn notification(&mut self, method: &str) -> Value {
        let m = method.to_string();
        self.next_where(method, move |v| v["method"] == m && v.get("id").is_none())
            .await["params"]
            .clone()
    }

    async fn server_request(&mut self, method: &str) -> Value {
        let m = method.to_string();
        self.next_where(method, move |v| v["method"] == m && v.get("id").is_some())
            .await
    }

    /// Send a request; its full response (`result` or `error`).
    async fn call(&mut self, method: &str, params: Value) -> Value {
        static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1_000);
        self.next_id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let id = self.next_id;
        METHODS.lock().unwrap().push((id, method.to_string()));
        self.send(json!({"id": id, "method": method, "params": params}))
            .await;
        self.next_where(method, move |v| v["id"] == id && v.get("method").is_none())
            .await
    }

    async fn ok(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params).await;
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    async fn initialize(&mut self) {
        self.ok(
            "initialize",
            json!({"clientInfo": {"name": "test", "version": "1"}, "capabilities": {"experimentalApi": true}}),
        )
        .await;
        self.send(json!({"method": "initialized"})).await;
    }

    fn drain_backlog(&mut self) -> Vec<Value> {
        while let Ok(v) = self.rx.try_recv() {
            self.backlog.push(v);
        }
        std::mem::take(&mut self.backlog)
    }
}

struct Setup {
    server: AppServer,
    faux: Arc<FauxProvider>,
    _registration: FauxProviderRegistration,
    _temp: tempfile::TempDir,
}

/// A server in a temp workspace. `ask_bash`: the workspace's mode asks
/// before bash (like hoobot's `discord` mode).
fn setup(ask_bash: bool) -> Setup {
    isolate_agent_dir();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().canonicalize().unwrap();
    let mode = if ask_bash { "ask" } else { "trusting" };
    let auto_allow: Vec<&str> = if ask_bash {
        vec!["read"]
    } else {
        vec!["read", "bash"]
    };
    std::fs::create_dir_all(dir.join(".cortexcode")).unwrap();
    std::fs::write(
        dir.join(".cortexcode/hoo-config.json"),
        json!({"active_mode": mode, "modes": {mode: {"auto_allow": auto_allow}}}).to_string(),
    )
    .unwrap();
    let registration = register_faux_provider(RegisterFauxProviderOptions::default());
    let faux = registration.provider().clone();
    let server = AppServer::new(
        ServerConfig {
            version: "9.9.9".into(),
            home: dir.to_string_lossy().into_owned(),
            cwd: dir.clone(),
        },
        Arc::new(Factory {
            faux: faux.clone(),
            dir: dir.clone(),
        }),
    );
    Setup {
        server,
        faux,
        _registration: registration,
        _temp: temp,
    }
}

fn reply(text: &str) -> FauxResponseStep {
    FauxResponseStep::Message(faux_assistant_message(text, Default::default()))
}

fn bash_call(command: &str) -> FauxResponseStep {
    FauxResponseStep::Message(faux_assistant_message(
        vec![faux_tool_call(
            "bash",
            json!({"command": command}),
            Some("call-1".into()),
        )],
        cortexcode_ai_provider_faux::FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    ))
}

async fn start_thread(client: &mut Client) -> String {
    let result = client.ok("thread/start", json!({})).await;
    let id = result["thread"]["id"].as_str().unwrap().to_string();
    let started = client.notification("thread/started").await;
    assert_eq!(started["thread"]["id"], json!(id));
    id
}

async fn start_turn(client: &mut Client, thread: &str, text: &str) -> String {
    let result = client
        .ok("turn/start", json!({"threadId": thread, "input": [{"type": "text", "text": text, "text_elements": []}]}))
        .await;
    assert_eq!(result["turn"]["status"], json!("inProgress"));
    result["turn"]["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_before_initialize_are_rejected() {
    let s = setup(false);
    let mut c = Client::connect(&s.server);
    let r = c.call("thread/list", json!({})).await;
    assert_eq!(r["error"]["code"], json!(-32600));
    let init = c
        .ok(
            "initialize",
            json!({"clientInfo": {"name": "t", "version": "1"}}),
        )
        .await;
    assert_eq!(init["userAgent"], json!("hoocode/9.9.9"));
    assert!(init["codexHome"].is_string() && init["platformOs"].is_string());
    let r = c
        .call(
            "initialize",
            json!({"clientInfo": {"name": "t", "version": "1"}}),
        )
        .await;
    assert_eq!(r["error"]["code"], json!(-32600));
    let r = c.call("fuzzyFileSearch", json!({})).await;
    assert_eq!(r["error"]["code"], json!(-32601));
    let r = c.call("account/read", json!({})).await;
    assert_eq!(r["result"]["requiresOpenaiAuth"], json!(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_list_leaves_out_hidden_models_unless_asked() {
    let s = setup(false);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let ids = |r: &Value| -> Vec<String> {
        r["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect()
    };
    let scoped = c.ok("model/list", json!({})).await;
    assert_eq!(ids(&scoped), vec!["p/in-scope"]);
    let all = c.ok("model/list", json!({"includeHidden": true})).await;
    assert_eq!(ids(&all), vec!["p/in-scope", "p/out-of-scope"]);
    assert_eq!(all["data"][1]["hidden"], json!(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_streams_items_and_completes() {
    let s = setup(false);
    s.faux.set_responses(vec![reply("hello there")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let turn = start_turn(&mut c, &thread, "hi").await;

    let started = c.notification("turn/started").await;
    assert_eq!(started["turn"]["id"], json!(turn));
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
    let items = completed["turn"]["items"].as_array().unwrap();
    assert_eq!(items[0]["type"], json!("userMessage"));
    assert_eq!(items.last().unwrap()["type"], json!("agentMessage"));
    assert_eq!(items.last().unwrap()["text"], json!("hello there"));

    let rest = c.drain_backlog();
    let deltas: String = rest
        .iter()
        .filter(|v| v["method"] == "item/agentMessage/delta")
        .map(|v| v["params"]["delta"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(deltas, "hello there");
    assert!(rest.iter().any(|v| v["method"] == "item/started"
        && v["params"]["item"]["type"] == "agentMessage"
        && v["params"]["startedAtMs"].is_i64()));

    // A second turn on the same thread works.
    s.faux.set_responses(vec![reply("again")]);
    start_turn(&mut c, &thread, "more").await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bash_asks_and_accept_runs_it() {
    let s = setup(true);
    s.faux.set_responses(vec![bash_call("ls"), reply("listed")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let turn = start_turn(&mut c, &thread, "list").await;

    let request = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    assert_eq!(request["params"]["command"], json!("ls"));
    assert_eq!(request["params"]["threadId"], json!(thread));
    assert_eq!(request["params"]["turnId"], json!(turn));
    assert_eq!(request["params"]["itemId"], json!("call-1"));
    c.send(json!({"id": request["id"], "result": {"decision": "accept"}}))
        .await;

    let resolved = c.notification("serverRequest/resolved").await;
    assert_eq!(resolved["requestId"], request["id"]);
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
    let cmd = completed["turn"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["type"] == "commandExecution")
        .unwrap()
        .clone();
    assert_eq!(cmd["status"], json!("completed"));
    assert_eq!(cmd["aggregatedOutput"], json!("ran ls"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decline_blocks_the_tool_and_the_turn_continues() {
    let s = setup(true);
    s.faux
        .set_responses(vec![bash_call("rm -rf /"), reply("ok, I won't")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    start_turn(&mut c, &thread, "clean up").await;
    let request = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.send(json!({"id": request["id"], "result": {"decision": "decline"}}))
        .await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
    let items = completed["turn"]["items"].as_array().unwrap();
    let cmd = items
        .iter()
        .find(|i| i["type"] == "commandExecution")
        .unwrap();
    assert_eq!(cmd["status"], json!("declined"));
    assert_eq!(items.last().unwrap()["text"], json!("ok, I won't"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_denies_and_interrupts() {
    let s = setup(true);
    s.faux
        .set_responses(vec![bash_call("ls"), reply("should not run")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    start_turn(&mut c, &thread, "x").await;
    let request = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.send(json!({"id": request["id"], "result": {"decision": "cancel"}}))
        .await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accept_for_session_skips_later_prompts() {
    let s = setup(true);
    s.faux
        .set_responses(vec![bash_call("ls"), bash_call("pwd"), reply("done")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    start_turn(&mut c, &thread, "x").await;
    let request = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.send(json!({"id": request["id"], "result": {"decision": "acceptForSession"}}))
        .await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
    let rest = c.drain_backlog();
    assert!(
        !rest
            .iter()
            .any(|v| v["method"] == "item/commandExecution/requestApproval"),
        "second bash call must not ask"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_denies_open_approval() {
    let s = setup(true);
    s.faux
        .set_responses(vec![bash_call("sleep 100"), reply("never")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let turn = start_turn(&mut c, &thread, "x").await;
    let request = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.ok(
        "turn/interrupt",
        json!({"threadId": thread, "turnId": turn}),
    )
    .await;
    let resolved = c.notification("serverRequest/resolved").await;
    assert_eq!(resolved["requestId"], request["id"]);
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_clients_share_a_thread_and_first_answer_wins() {
    let s = setup(true);
    s.faux.set_responses(vec![bash_call("ls"), reply("done")]);
    let mut a = Client::connect(&s.server);
    let mut b = Client::connect(&s.server);
    a.initialize().await;
    b.initialize().await;
    let thread = start_thread(&mut a).await;
    let resumed = b.ok("thread/resume", json!({"threadId": thread})).await;
    assert_eq!(resumed["thread"]["id"], json!(thread));
    assert_eq!(resumed["sandbox"], json!({"type": "dangerFullAccess"}));

    start_turn(&mut a, &thread, "x").await;
    let ra = a
        .server_request("item/commandExecution/requestApproval")
        .await;
    let rb = b
        .server_request("item/commandExecution/requestApproval")
        .await;
    assert_eq!(ra["id"], rb["id"], "one id for every subscriber");

    b.send(json!({"id": rb["id"], "result": {"decision": "accept"}}))
        .await;
    // The two connections are independent, so A's answer is only "late"
    // once B's has been taken: wait for that, then A's is ignored.
    let resolved = a.notification("serverRequest/resolved").await;
    assert_eq!(resolved["requestId"], ra["id"]);
    a.send(json!({"id": ra["id"], "result": {"decision": "decline"}}))
        .await;
    let resolved = b.notification("serverRequest/resolved").await;
    assert_eq!(resolved["requestId"], ra["id"]);
    for c in [&mut a, &mut b] {
        let completed = c.notification("turn/completed").await;
        let cmd = completed["turn"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "commandExecution")
            .unwrap()
            .clone();
        assert_eq!(cmd["status"], json!("completed"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reconnecting_client_gets_the_open_approval() {
    let s = setup(true);
    s.faux.set_responses(vec![bash_call("ls"), reply("done")]);
    let mut a = Client::connect(&s.server);
    a.initialize().await;
    let thread = start_thread(&mut a).await;
    // Keep the thread loaded while A goes away.
    let mut keeper = Client::connect(&s.server);
    keeper.initialize().await;
    keeper
        .ok("thread/resume", json!({"threadId": thread}))
        .await;
    start_turn(&mut a, &thread, "x").await;
    let first = a
        .server_request("item/commandExecution/requestApproval")
        .await;
    drop(a);

    let mut b = Client::connect(&s.server);
    b.initialize().await;
    let resumed = b.ok("thread/resume", json!({"threadId": thread})).await;
    assert_eq!(resumed["thread"]["status"]["type"], json!("active"));
    let replayed = b
        .server_request("item/commandExecution/requestApproval")
        .await;
    assert_eq!(replayed["id"], first["id"]);
    b.send(json!({"id": replayed["id"], "result": {"decision": "accept"}}))
        .await;
    let completed = b.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn steer_needs_the_active_turn_id() {
    let s = setup(false);
    s.faux.set_responses(vec![
        FauxResponseStep::async_factory(|_, _, _, _| async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(faux_assistant_message("first", Default::default()))
        }),
        reply("after steer"),
    ]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let turn = start_turn(&mut c, &thread, "x").await;
    let bad = c
        .call("turn/steer", json!({"threadId": thread, "expectedTurnId": "nope", "input": [{"type": "text", "text": "y"}]}))
        .await;
    assert!(bad.get("error").is_some());
    let again = c
        .call(
            "turn/start",
            json!({"threadId": thread, "input": [{"type": "text", "text": "z"}]}),
        )
        .await;
    assert!(again.get("error").is_some(), "one turn at a time");
    let good = c
        .ok("turn/steer", json!({"threadId": thread, "expectedTurnId": turn, "input": [{"type": "text", "text": "also this"}]}))
        .await;
    assert_eq!(good["turnId"], json!(turn));
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
    let texts: Vec<&str> = completed["turn"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["type"] == "agentMessage")
        .map(|i| i["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, vec!["first", "after steer"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn list_read_and_resume_from_disk() {
    let s = setup(false);
    s.faux.set_responses(vec![reply("saved answer")]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    start_turn(&mut c, &thread, "remember me").await;
    c.notification("turn/completed").await;
    let unsub = c
        .ok("thread/unsubscribe", json!({"threadId": thread}))
        .await;
    assert_eq!(unsub["status"], json!("unsubscribed"));

    let list = c.ok("thread/list", json!({"limit": 10})).await;
    let entry = list["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == json!(thread))
        .unwrap()
        .clone();
    assert_eq!(entry["status"]["type"], json!("notLoaded"));
    assert_eq!(entry["preview"], json!("remember me"));
    assert!(list["nextCursor"].is_null());
    assert!(entry.get("projectId").is_some());

    let read = c
        .ok(
            "thread/read",
            json!({"threadId": thread, "includeTurns": true}),
        )
        .await;
    let turns = read["thread"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["items"][1]["text"], json!("saved answer"));

    let resumed = c.ok("thread/resume", json!({"threadId": thread})).await;
    assert_eq!(resumed["thread"]["status"]["type"], json!("idle"));
    assert_eq!(resumed["thread"]["turns"].as_array().unwrap().len(), 1);
    s.faux.set_responses(vec![reply("still here")]);
    start_turn(&mut c, &thread, "again").await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("completed"));
}

// ---------------------------------------------------------------------------
// Schema conformance against Codex's own JSON schemas, generated at test time
// (`scripts/codex-schema.sh`, into target/; never checked in).

fn schema_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/codex-schema")
}

/// Definition name in the v2 bundle, or a standalone file, per message.
fn schema_for(kind: &str, method: &str) -> Option<&'static str> {
    Some(match (kind, method) {
        ("notification", "thread/started") => "ThreadStartedNotification",
        ("notification", "thread/status/changed") => "ThreadStatusChangedNotification",
        ("notification", "turn/started") => "TurnStartedNotification",
        ("notification", "turn/completed") => "TurnCompletedNotification",
        ("notification", "item/started") => "ItemStartedNotification",
        ("notification", "item/completed") => "ItemCompletedNotification",
        ("notification", "item/agentMessage/delta") => "AgentMessageDeltaNotification",
        ("notification", "serverRequest/resolved") => "ServerRequestResolvedNotification",
        ("notification", "error") => "ErrorNotification",
        ("response", "thread/start") => "ThreadStartResponse",
        ("response", "thread/resume") => "ThreadResumeResponse",
        ("response", "thread/list") => "ThreadListResponse",
        ("response", "thread/read") => "ThreadReadResponse",
        ("response", "thread/unsubscribe") => "ThreadUnsubscribeResponse",
        ("response", "thread/loaded/list") => "ThreadLoadedListResponse",
        ("response", "turn/start") => "TurnStartResponse",
        ("response", "turn/steer") => "TurnSteerResponse",
        ("response", "turn/interrupt") => "TurnInterruptResponse",
        ("response", "account/read") => "GetAccountResponse",
        ("response", "model/list") => "ModelListResponse",
        ("response", "configRequirements/read") => "ConfigRequirementsReadResponse",
        ("response", "skills/list") => "SkillsListResponse",
        ("response", "initialize") => "file:v1/InitializeResponse.json",
        ("request", "item/commandExecution/requestApproval") => {
            "file:CommandExecutionRequestApprovalParams.json"
        }
        ("request", "item/fileChange/requestApproval") => {
            "file:FileChangeRequestApprovalParams.json"
        }
        _ => return None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a codex binary; run scripts/codex-schema.sh, then --ignored"]
async fn schema_conformance() {
    let dir = schema_dir();
    let bundle_path = dir.join("codex_app_server_protocol.v2.schemas.json");
    if !bundle_path.exists() {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/codex-schema.sh");
        let _ = std::process::Command::new(script).status();
    }
    let Ok(bundle) = std::fs::read_to_string(&bundle_path) else {
        eprintln!("skipping: no Codex schemas at {}", dir.display());
        return;
    };
    let bundle: Value = serde_json::from_str(&bundle).unwrap();

    // A scenario that touches every message type we send.
    let s = setup(true);
    s.faux.set_responses(vec![
        reply("hello"),
        bash_call("ls"),
        reply("ran it"),
        bash_call("rm x"),
        reply("ok"),
        bash_call("sleep 9"),
        FauxResponseStep::Message(faux_assistant_message(
            "",
            cortexcode_ai_provider_faux::FauxMessageOptions {
                stop_reason: Some(StopReason::Error),
                error_message: Some("boom".into()),
                ..Default::default()
            },
        )),
    ]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    for (method, params) in [
        ("account/read", json!({"refreshToken": false})),
        ("model/list", json!({"includeHidden": true})),
        ("configRequirements/read", json!({})),
        ("skills/list", json!({"cwds": ["/x"]})),
        ("thread/loaded/list", json!({})),
    ] {
        c.ok(method, params).await;
    }
    let thread = start_thread(&mut c).await;
    start_turn(&mut c, &thread, "hi").await;
    c.notification("turn/completed").await;
    start_turn(&mut c, &thread, "run ls").await;
    let r = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.send(json!({"id": r["id"], "result": {"decision": "accept"}}))
        .await;
    c.notification("turn/completed").await;
    start_turn(&mut c, &thread, "remove").await;
    let r = c
        .server_request("item/commandExecution/requestApproval")
        .await;
    c.send(json!({"id": r["id"], "result": {"decision": "decline"}}))
        .await;
    c.notification("turn/completed").await;
    let turn = start_turn(&mut c, &thread, "wait").await;
    c.server_request("item/commandExecution/requestApproval")
        .await;
    c.ok(
        "turn/interrupt",
        json!({"threadId": thread, "turnId": turn}),
    )
    .await;
    c.notification("turn/completed").await;
    start_turn(&mut c, &thread, "fail").await;
    c.notification("turn/completed").await;
    c.ok("thread/list", json!({"limit": 5})).await;
    c.ok(
        "thread/read",
        json!({"threadId": thread, "includeTurns": true}),
    )
    .await;
    c.ok("thread/resume", json!({"threadId": thread})).await;
    c.ok("thread/unsubscribe", json!({"threadId": thread}))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let recorded = RECORDED.lock().unwrap().clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for (response_to, message) in &recorded {
        let (kind, method, payload) = if let Some(m) = message["method"].as_str() {
            if message.get("id").is_some() {
                ("request", m.to_string(), &message["params"])
            } else {
                ("notification", m.to_string(), &message["params"])
            }
        } else if message.get("result").is_some() {
            let Some(m) = response_to else { continue };
            ("response", m.clone(), &message["result"])
        } else {
            continue; // error responses: shape checked by the protocol crate
        };
        let Some(name) = schema_for(kind, &method) else {
            failures.push(format!("{kind} {method}: no schema mapping"));
            continue;
        };
        seen.insert(format!("{kind} {method}"));
        let schema = match name.strip_prefix("file:") {
            Some(file) => {
                serde_json::from_str(&std::fs::read_to_string(dir.join(file)).unwrap()).unwrap()
            }
            None => json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "$ref": format!("#/definitions/{name}"),
                "definitions": bundle["definitions"],
            }),
        };
        let validator = jsonschema::draft7::new(&schema)
            .unwrap_or_else(|e| panic!("bad schema for {name}: {e}"));
        for error in validator.iter_errors(payload) {
            failures.push(format!(
                "{kind} {method} ({name}) at {}: {error}",
                error.instance_path
            ));
        }
    }
    eprintln!("checked {} messages; types: {seen:#?}", recorded.len());
    for expected in [
        "notification error",
        "notification item/agentMessage/delta",
        "notification serverRequest/resolved",
        "request item/commandExecution/requestApproval",
        "response initialize",
        "response thread/resume",
    ] {
        assert!(
            seen.contains(expected),
            "scenario never produced {expected}"
        );
    }
    assert!(
        failures.is_empty(),
        "{} schema failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turn_start_rejects_an_unknown_model() {
    let s = setup(false);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let r = c
        .call(
            "turn/start",
            json!({"threadId": thread, "model": "no-such-model", "input": [{"type": "text", "text": "x"}]}),
        )
        .await;
    assert_eq!(r["error"]["code"], json!(-32602));
    // The current model by name is fine.
    s.faux.set_responses(vec![reply("ok")]);
    let current = s.faux.get_model().id;
    c.ok(
        "turn/start",
        json!({"threadId": thread, "model": current, "input": [{"type": "text", "text": "x"}]}),
    )
    .await;
    assert_eq!(
        c.notification("turn/completed").await["turn"]["status"],
        json!("completed")
    );
}

/// Two tool calls in one message: interrupting during the first approval
/// must not leave the second one asking (and the interrupt hanging).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_with_two_pending_tool_calls_does_not_hang() {
    let s = setup(true);
    s.faux.set_responses(vec![
        FauxResponseStep::Message(faux_assistant_message(
            vec![
                faux_tool_call("bash", json!({"command": "one"}), Some("c1".into())),
                faux_tool_call("bash", json!({"command": "two"}), Some("c2".into())),
            ],
            cortexcode_ai_provider_faux::FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )),
        reply("never"),
    ]);
    let mut c = Client::connect(&s.server);
    c.initialize().await;
    let thread = start_thread(&mut c).await;
    let turn = start_turn(&mut c, &thread, "x").await;
    c.server_request("item/commandExecution/requestApproval")
        .await;
    c.ok(
        "turn/interrupt",
        json!({"threadId": thread, "turnId": turn}),
    )
    .await;
    let completed = c.notification("turn/completed").await;
    assert_eq!(completed["turn"]["status"], json!("interrupted"));
    let asked: Vec<Value> = c
        .drain_backlog()
        .into_iter()
        .filter(|v| v["method"] == "item/commandExecution/requestApproval")
        .collect();
    assert!(asked.is_empty(), "asked again after interrupt: {asked:?}");
}

//! `modes/rpc/rpc-mode.ts`: headless operation over JSON lines.
//!
//! Commands arrive on stdin (`{"id"?, "type": "...", ...}`), responses
//! (`{"id"?, "type": "response", "command", "success", "data"? | "error"}`) and
//! every session event go to stdout, one JSON object per line
//! (`rpc-types.ts`, `docs/rpc.md`).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use cortexcode_ai_types::{ImageContent, ThinkingLevel};
use cortexcode_code_agent_session::{
    compaction_result_json, AgentSession, AgentSessionRuntime, CycleDirection, ForkPosition,
    InputSource, NewSessionRequest, PreflightResult, PromptOptions, SessionSubscription,
    StreamingBehavior,
};
use cortexcode_code_settings::QueueMode;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::jsonl::JsonlLineReader;

/// A boxed future, for [`RpcHost`] methods.
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Outcome of a session-replacing command (`{ cancelled }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChange {
    pub cancelled: bool,
}

/// Outcome of `fork` (`{ selectedText, cancelled }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkChange {
    pub selected_text: Option<String>,
    pub cancelled: bool,
}

/// The `AgentSessionRuntime` surface RPC mode uses: the current session and the
/// operations that replace it.
pub trait RpcHost: Send + Sync {
    /// The current session (it changes after new/switch/fork).
    fn session(&self) -> AgentSession;
    /// `newSession({ parentSession })`.
    fn new_session(
        &self,
        parent_session: Option<String>,
    ) -> HostFuture<'_, Result<SessionChange, String>>;
    /// `switchSession(sessionPath)`.
    fn switch_session(&self, session_path: String)
        -> HostFuture<'_, Result<SessionChange, String>>;
    /// `fork(entryId, { position: at ? "at" : "before" })`.
    fn fork(&self, entry_id: String, at: bool) -> HostFuture<'_, Result<ForkChange, String>>;
    /// `dispose()`.
    fn dispose(&self) -> HostFuture<'_, ()>;
}

/// The CLI's host: an `AgentSessionRuntime` whose session new/switch/fork
/// replace.
pub struct RuntimeHost {
    runtime: tokio::sync::Mutex<AgentSessionRuntime>,
    /// The runtime's current session, readable without waiting on a
    /// replacement in flight.
    current: Mutex<AgentSession>,
}

impl RuntimeHost {
    pub fn new(runtime: AgentSessionRuntime) -> Self {
        let current = Mutex::new(runtime.session().clone());
        Self {
            runtime: tokio::sync::Mutex::new(runtime),
            current,
        }
    }

    fn sync_current(&self, runtime: &AgentSessionRuntime) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = runtime.session().clone();
    }
}

impl RpcHost for RuntimeHost {
    fn session(&self) -> AgentSession {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    fn new_session(
        &self,
        parent_session: Option<String>,
    ) -> HostFuture<'_, Result<SessionChange, String>> {
        Box::pin(async move {
            let mut runtime = self.runtime.lock().await;
            let result = runtime
                .new_session(NewSessionRequest {
                    parent_session,
                    setup: None,
                })
                .await;
            self.sync_current(&runtime);
            let result = result.map_err(|e| e.to_string())?;
            Ok(SessionChange {
                cancelled: result.cancelled,
            })
        })
    }
    fn switch_session(
        &self,
        session_path: String,
    ) -> HostFuture<'_, Result<SessionChange, String>> {
        Box::pin(async move {
            let mut runtime = self.runtime.lock().await;
            let result = runtime
                .switch_session(std::path::Path::new(&session_path), None)
                .await;
            self.sync_current(&runtime);
            let result = result.map_err(|e| e.to_string())?;
            Ok(SessionChange {
                cancelled: result.cancelled,
            })
        })
    }
    fn fork(&self, entry_id: String, at: bool) -> HostFuture<'_, Result<ForkChange, String>> {
        Box::pin(async move {
            let position = if at {
                ForkPosition::At
            } else {
                ForkPosition::Before
            };
            let mut runtime = self.runtime.lock().await;
            let result = runtime.fork(&entry_id, position).await;
            self.sync_current(&runtime);
            let result = result.map_err(|e| e.to_string())?;
            Ok(ForkChange {
                selected_text: result.selected_text,
                cancelled: result.cancelled,
            })
        })
    }
    fn dispose(&self) -> HostFuture<'_, ()> {
        Box::pin(async move { self.runtime.lock().await.dispose().await })
    }
}

/// A host with one fixed session: session-replacing commands fail. For
/// embedding RPC mode without a runtime factory.
pub struct SingleSessionHost {
    session: AgentSession,
}

impl SingleSessionHost {
    pub fn new(session: AgentSession) -> Self {
        Self { session }
    }
}

const NO_RUNTIME: &str = "This command needs a session runtime, which this host does not have";

impl RpcHost for SingleSessionHost {
    fn session(&self) -> AgentSession {
        self.session.clone()
    }
    fn new_session(&self, _: Option<String>) -> HostFuture<'_, Result<SessionChange, String>> {
        Box::pin(async { Err(NO_RUNTIME.to_string()) })
    }
    fn switch_session(&self, _: String) -> HostFuture<'_, Result<SessionChange, String>> {
        Box::pin(async { Err(NO_RUNTIME.to_string()) })
    }
    fn fork(&self, _: String, _: bool) -> HostFuture<'_, Result<ForkChange, String>> {
        Box::pin(async { Err(NO_RUNTIME.to_string()) })
    }
    fn dispose(&self) -> HostFuture<'_, ()> {
        let session = self.session.clone();
        Box::pin(async move { session.dispose() })
    }
}

/// Where RPC output lines go (`writeRawStdout(serializeJsonLine(obj))`).
pub type RpcOutput = Arc<dyn Fn(&Value) + Send + Sync>;

/// `{ id, type: "response", command, success: true, data? }`; `id` is left
/// out when the command had none (`JSON.stringify` drops `undefined`).
pub fn success(id: Option<&Value>, command: &str, data: Option<Value>) -> Value {
    let mut map = Map::new();
    if let Some(id) = id {
        map.insert("id".into(), id.clone());
    }
    map.insert("type".into(), "response".into());
    map.insert("command".into(), command.into());
    map.insert("success".into(), true.into());
    if let Some(data) = data {
        map.insert("data".into(), data);
    }
    Value::Object(map)
}

/// `{ id, type: "response", command, success: false, error }`.
pub fn error(id: Option<&Value>, command: &str, message: &str) -> Value {
    let mut map = Map::new();
    if let Some(id) = id {
        map.insert("id".into(), id.clone());
    }
    map.insert("type".into(), "response".into());
    map.insert("command".into(), command.into());
    map.insert("success".into(), false.into());
    map.insert("error".into(), message.into());
    Value::Object(map)
}

/// A command's fields (`RpcCommand`), read loosely as hoocode does.
struct Command<'a> {
    value: &'a Value,
}

impl Command<'_> {
    fn str(&self, key: &str) -> Result<String, String> {
        match self.value.get(key) {
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err(format!("Missing or invalid \"{key}\"")),
        }
    }
    fn opt_str(&self, key: &str) -> Option<String> {
        self.value
            .get(key)
            .and_then(Value::as_str)
            .map(String::from)
    }
    fn bool(&self, key: &str) -> Result<bool, String> {
        self.value
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("Missing or invalid \"{key}\""))
    }
    fn images(&self) -> Result<Vec<ImageContent>, String> {
        match self.value.get("images") {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(images) => serde_json::from_value(images.clone()).map_err(|e| e.to_string()),
        }
    }
}

fn parse_queue_mode(mode: &str) -> Result<QueueMode, String> {
    QueueMode::parse(mode).ok_or_else(|| format!("Invalid mode: {mode}"))
}

fn parse_thinking_level(level: &str) -> Result<ThinkingLevel, String> {
    Ok(match level {
        "off" => ThinkingLevel::Off,
        "minimal" => ThinkingLevel::Minimal,
        "low" => ThinkingLevel::Low,
        "medium" => ThinkingLevel::Medium,
        "high" => ThinkingLevel::High,
        "xhigh" => ThinkingLevel::XHigh,
        other => return Err(format!("Invalid thinking level: {other}")),
    })
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The RPC command loop over one [`RpcHost`].
pub struct RpcMode {
    host: Arc<dyn RpcHost>,
    output: RpcOutput,
    subscription: Mutex<Option<SessionSubscription>>,
}

impl RpcMode {
    /// Start forwarding the host session's events to `output`.
    pub fn new(host: Arc<dyn RpcHost>, output: RpcOutput) -> Arc<Self> {
        let mode = Arc::new(Self {
            host,
            output,
            subscription: Mutex::new(None),
        });
        mode.rebind_session();
        mode
    }

    fn session(&self) -> AgentSession {
        self.host.session()
    }

    fn output(&self, value: &Value) {
        (self.output)(value);
    }

    /// `rebindSession`: subscribe to the (possibly new) current session.
    pub fn rebind_session(&self) {
        let output = self.output.clone();
        let subscription = self
            .session()
            .subscribe(move |event| output(&event.to_json()));
        *self.subscription.lock().unwrap_or_else(|e| e.into_inner()) = Some(subscription);
    }

    /// `handleInputLine`: parse one line and run it. Commands that wait on the
    /// model, a compaction, a bash command or a session switch run in the
    /// background (hoocode does not await them before reading the next line);
    /// the rest finish before this returns, so they apply in order.
    pub async fn handle_line(self: &Arc<Self>, line: &str) {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                self.output(&error(
                    None,
                    "parse",
                    &format!("Failed to parse command: {e}"),
                ));
                return;
            }
        };
        // Extension UI responses: no dialogs are pending until the extension
        // runner exists (ledger 12.3).
        if parsed.get("type").and_then(Value::as_str) == Some("extension_ui_response") {
            return;
        }
        let command_type = parsed
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if matches!(
            command_type.as_str(),
            "prompt"
                | "abort"
                | "new_session"
                | "compact"
                | "bash"
                | "switch_session"
                | "fork"
                | "clone"
        ) {
            let this = self.clone();
            tokio::spawn(async move { this.run_command(parsed, command_type).await });
        } else {
            self.clone().run_command(parsed, command_type).await;
        }
    }

    async fn run_command(self: Arc<Self>, parsed: Value, command_type: String) {
        let id = parsed.get("id");
        match self.handle_command(id, &command_type, &parsed).await {
            Ok(Some(response)) => self.output(&response),
            Ok(None) => {}
            Err(message) => self.output(&error(id, &command_type, &message)),
        }
    }

    /// `handleCommand`: the response, `None` when it is sent later (prompt),
    /// or the error message.
    async fn handle_command(
        self: &Arc<Self>,
        id: Option<&Value>,
        command_type: &str,
        value: &Value,
    ) -> Result<Option<Value>, String> {
        let command = Command { value };
        let session = self.session();
        let ok = |data: Option<Value>| Ok(Some(success(id, command_type, data)));
        match command_type {
            // Prompting
            "prompt" => {
                let message = command.str("message")?;
                let streaming_behavior = match command.opt_str("streamingBehavior").as_deref() {
                    Some("steer") => Some(StreamingBehavior::Steer),
                    Some("followUp") => Some(StreamingBehavior::FollowUp),
                    _ => None,
                };
                // The response is sent once preflight accepts the prompt; a
                // rejection surfaces as the error below.
                let accepted = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let preflight = {
                    let this = self.clone();
                    let accepted = accepted.clone();
                    let id = id.cloned();
                    PreflightResult(Arc::new(move |did_succeed| {
                        if did_succeed {
                            accepted.store(true, std::sync::atomic::Ordering::SeqCst);
                            this.output(&success(id.as_ref(), "prompt", None));
                        }
                    }))
                };
                let options = PromptOptions {
                    images: command.images()?,
                    streaming_behavior,
                    source: InputSource::Rpc,
                    preflight_result: Some(preflight),
                    ..Default::default()
                };
                if let Err(e) = session.prompt(&message, options).await {
                    if !accepted.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(e.to_string());
                    }
                }
                Ok(None)
            }
            "steer" => {
                session
                    .steer(&command.str("message")?, &command.images()?)
                    .map_err(|e| e.to_string())?;
                ok(None)
            }
            "follow_up" => {
                session
                    .follow_up(&command.str("message")?, &command.images()?)
                    .map_err(|e| e.to_string())?;
                ok(None)
            }
            "abort" => {
                session.abort().await;
                ok(None)
            }
            "new_session" => {
                let change = self
                    .host
                    .new_session(command.opt_str("parentSession"))
                    .await?;
                if !change.cancelled {
                    self.rebind_session();
                }
                ok(Some(json!({"cancelled": change.cancelled})))
            }

            // State
            "get_state" => {
                let mut state = Map::new();
                if let Some(model) = session.model() {
                    state.insert("model".into(), to_json(&model));
                }
                state.insert(
                    "thinkingLevel".into(),
                    session.thinking_level().as_str().into(),
                );
                state.insert("isStreaming".into(), session.is_streaming().into());
                state.insert("isCompacting".into(), session.is_compacting().into());
                state.insert(
                    "steeringMode".into(),
                    session.steering_mode().as_str().into(),
                );
                state.insert(
                    "followUpMode".into(),
                    session.follow_up_mode().as_str().into(),
                );
                if let Some(file) = session.session_file() {
                    state.insert(
                        "sessionFile".into(),
                        file.to_string_lossy().into_owned().into(),
                    );
                }
                state.insert("sessionId".into(), session.session_id().into());
                if let Some(name) = session.session_name() {
                    state.insert("sessionName".into(), name.into());
                }
                state.insert(
                    "autoCompactionEnabled".into(),
                    session.auto_compaction_enabled().into(),
                );
                state.insert("messageCount".into(), session.messages().len().into());
                state.insert(
                    "pendingMessageCount".into(),
                    session.pending_message_count().into(),
                );
                ok(Some(Value::Object(state)))
            }

            // Model
            "set_model" => {
                let provider = command.str("provider")?;
                let model_id = command.str("modelId")?;
                let Some(model) = session
                    .get_available_models()
                    .into_iter()
                    .find(|m| m.provider == provider && m.id == model_id)
                else {
                    return Ok(Some(error(
                        id,
                        "set_model",
                        &format!("Model not found: {provider}/{model_id}"),
                    )));
                };
                session
                    .set_model(model.clone())
                    .map_err(|e| e.to_string())?;
                ok(Some(to_json(&model)))
            }
            "cycle_model" => match session.cycle_model(CycleDirection::Forward) {
                None => ok(Some(Value::Null)),
                Some(result) => ok(Some(json!({
                    "model": to_json(&result.model),
                    "thinkingLevel": result.thinking_level.as_str(),
                    "isScoped": result.is_scoped,
                }))),
            },
            "get_available_models" => {
                let models: Vec<Value> =
                    session.get_available_models().iter().map(to_json).collect();
                ok(Some(json!({ "models": models })))
            }

            // Thinking
            "set_thinking_level" => {
                session.set_thinking_level(parse_thinking_level(&command.str("level")?)?);
                ok(None)
            }
            "cycle_thinking_level" => match session.cycle_thinking_level(CycleDirection::Forward) {
                None => ok(Some(Value::Null)),
                Some(level) => ok(Some(json!({ "level": level.as_str() }))),
            },

            // Queue modes
            "set_steering_mode" => {
                session.set_steering_mode(parse_queue_mode(&command.str("mode")?)?);
                ok(None)
            }
            "set_follow_up_mode" => {
                session.set_follow_up_mode(parse_queue_mode(&command.str("mode")?)?);
                ok(None)
            }

            // Compaction
            "compact" => {
                let instructions = command.opt_str("customInstructions");
                let result = session
                    .compact(instructions.as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                ok(Some(compaction_result_json(&result)))
            }
            "set_auto_compaction" => {
                session.set_auto_compaction_enabled(command.bool("enabled")?);
                ok(None)
            }

            // Retry
            "set_auto_retry" => {
                session.set_auto_retry_enabled(command.bool("enabled")?);
                ok(None)
            }
            "abort_retry" => {
                session.abort_retry();
                ok(None)
            }

            // Bash
            "bash" => {
                let bash_command = command.str("command")?;
                let result = tokio::task::spawn_blocking(move || {
                    session.execute_bash(&bash_command, None, false, None)
                })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
                let mut data = Map::new();
                data.insert("output".into(), result.output.into());
                if let Some(code) = result.exit_code {
                    data.insert("exitCode".into(), code.into());
                }
                data.insert("cancelled".into(), result.cancelled.into());
                data.insert("truncated".into(), result.truncated.into());
                if let Some(path) = result.full_output_path {
                    data.insert("fullOutputPath".into(), path.into());
                }
                ok(Some(Value::Object(data)))
            }
            "abort_bash" => {
                session.abort_bash();
                ok(None)
            }

            // Session
            "get_session_stats" => ok(Some(to_json(&session.get_session_stats()))),
            "export_html" => Err("HTML export is not supported by cortex yet".into()),
            "switch_session" => {
                let change = self
                    .host
                    .switch_session(command.str("sessionPath")?)
                    .await?;
                if !change.cancelled {
                    self.rebind_session();
                }
                ok(Some(json!({"cancelled": change.cancelled})))
            }
            "fork" => {
                let change = self.host.fork(command.str("entryId")?, false).await?;
                if !change.cancelled {
                    self.rebind_session();
                }
                let mut data = Map::new();
                if let Some(text) = change.selected_text {
                    data.insert("text".into(), Value::String(text));
                }
                data.insert("cancelled".into(), Value::Bool(change.cancelled));
                ok(Some(Value::Object(data)))
            }
            "clone" => {
                let leaf = session.session_manager().leaf_id().map(String::from);
                let Some(leaf) = leaf else {
                    return Ok(Some(error(
                        id,
                        "clone",
                        "Cannot clone session: no current entry selected",
                    )));
                };
                let change = self.host.fork(leaf, true).await?;
                if !change.cancelled {
                    self.rebind_session();
                }
                ok(Some(json!({"cancelled": change.cancelled})))
            }
            "get_fork_messages" => ok(Some(
                json!({ "messages": to_json(&session.get_user_messages_for_forking()) }),
            )),
            "get_last_assistant_text" => {
                ok(Some(json!({ "text": session.get_last_assistant_text() })))
            }
            "set_session_name" => {
                let name = command.str("name")?;
                let name = name.trim();
                if name.is_empty() {
                    return Ok(Some(error(
                        id,
                        "set_session_name",
                        "Session name cannot be empty",
                    )));
                }
                session.set_session_name(name);
                ok(None)
            }

            // Messages
            "get_messages" => ok(Some(json!({ "messages": to_json(&session.messages()) }))),

            // Commands: extension commands (none until 12.3), prompt templates, skills.
            "get_commands" => {
                let commands: Vec<Value> = session
                    .resource_loader()
                    .slash_commands()
                    .into_iter()
                    .map(|c| {
                        let mut map = Map::new();
                        map.insert("name".into(), c.name.into());
                        if let Some(description) = c.description {
                            map.insert("description".into(), description.into());
                        }
                        map.insert("source".into(), c.source.into());
                        map.insert("sourceInfo".into(), c.source_info);
                        Value::Object(map)
                    })
                    .collect();
                ok(Some(json!({ "commands": commands })))
            }

            other => Ok(Some(error(
                None,
                other,
                &format!("Unknown command: {other}"),
            ))),
        }
    }
}

/// `runRpcMode`: read commands from `input` until it ends, then dispose the
/// host. Returns the process exit code.
pub async fn run_rpc_mode<R: AsyncRead + Unpin>(
    host: Arc<dyn RpcHost>,
    mut input: R,
    output: RpcOutput,
) -> i32 {
    let mode = RpcMode::new(host.clone(), output);
    let mut reader = JsonlLineReader::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match input.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for line in reader.push(&buf[..n]) {
                    mode.handle_line(&line).await;
                }
            }
        }
    }
    if let Some(line) = reader.finish() {
        mode.handle_line(&line).await;
    }
    // `shutdown()`: stdin ended.
    *mode.subscription.lock().unwrap_or_else(|e| e.into_inner()) = None;
    host.dispose().await;
    0
}

//! `modes/rpc/rpc-client.ts`: spawn the agent in RPC mode and drive it with
//! typed commands.
//!
//! Responses are matched to requests by `id` (`req_<n>`); every other line is
//! an event for the [`RpcClient::on_event`] listeners.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cortexcode_ai_types::ImageContent;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::jsonl::{serialize_json_line, JsonlLineReader};

/// How long a command waits for its response.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
/// `waitForIdle` / `collectEvents` default.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// `RpcClientOptions`.
#[derive(Debug, Clone, Default)]
pub struct RpcClientOptions {
    /// The agent executable. Defaults to the running binary (`cortex`).
    pub executable: Option<PathBuf>,
    /// Arguments before the RPC ones (e.g. a script for an interpreter).
    pub prefix_args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Added to the inherited environment.
    pub env: Vec<(String, String)>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Extra CLI arguments.
    pub args: Vec<String>,
}

/// A failed command, a dead process or a timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcClientError(pub String);

impl std::fmt::Display for RpcClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RpcClientError {}

type Result<T> = std::result::Result<T, RpcClientError>;
type Listener = Arc<dyn Fn(&Value) + Send + Sync>;
type ExitListener = Arc<dyn Fn(Option<i32>) + Send + Sync>;

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>,
    listeners: Mutex<Vec<(u64, Listener)>>,
    exit_listeners: Mutex<Vec<(u64, ExitListener)>>,
    next_listener: AtomicU64,
    stderr: Mutex<String>,
    exited: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn stderr(&self) -> String {
        lock(&self.stderr).clone()
    }

    /// `handleLine`: a response to a pending request, else an event.
    fn handle_line(&self, line: &str) {
        let Ok(data) = serde_json::from_str::<Value>(line) else {
            return; // non-JSON lines are ignored
        };
        if data["type"] == "response" {
            if let Some(id) = data["id"].as_str() {
                if let Some(sender) = lock(&self.pending).remove(id) {
                    let _ = sender.send(Ok(data));
                    return;
                }
            }
        }
        let listeners: Vec<Listener> = lock(&self.listeners)
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(&data);
        }
    }

    /// `handleExit`: fail every in-flight request and tell the exit listeners.
    fn handle_exit(&self, code: Option<i32>) {
        self.exited.store(true, Ordering::SeqCst);
        let message = format!(
            "Agent process exited (code {}). Stderr: {}",
            code.map_or_else(|| "null".to_string(), |c| c.to_string()),
            self.stderr()
        );
        for (_, sender) in lock(&self.pending).drain() {
            let _ = sender.send(Err(RpcClientError(message.clone())));
        }
        let listeners: Vec<_> = lock(&self.exit_listeners)
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(code);
        }
    }
}

/// Unsubscribes a listener when dropped or [`Subscription::cancel`]led.
pub struct Subscription {
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl Subscription {
    pub fn cancel(mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

/// Sends commands to the agent (a spawned process, or any writer in tests).
type Sink = Arc<tokio::sync::Mutex<Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>>>;

/// `RpcClient`.
pub struct RpcClient {
    options: RpcClientOptions,
    shared: Arc<Shared>,
    stdin: Sink,
    child: Option<Arc<tokio::sync::Mutex<Child>>>,
    exit_rx: Option<tokio::sync::watch::Receiver<bool>>,
    request_id: AtomicU64,
}

impl RpcClient {
    pub fn new(options: RpcClientOptions) -> Self {
        Self {
            options,
            shared: Arc::default(),
            stdin: Arc::default(),
            child: None,
            exit_rx: None,
            request_id: AtomicU64::new(0),
        }
    }

    /// `start()`: spawn `<executable> [prefix args] --mode rpc [--provider] [--model] [args]`.
    pub async fn start(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Err(RpcClientError("Client already started".into()));
        }
        let mut args: Vec<String> = vec!["--mode".into(), "rpc".into()];
        if let Some(provider) = &self.options.provider {
            args.extend(["--provider".into(), provider.clone()]);
        }
        if let Some(model) = &self.options.model {
            args.extend(["--model".into(), model.clone()]);
        }
        args.extend(self.options.args.iter().cloned());
        let executable = match &self.options.executable {
            Some(executable) => executable.clone(),
            None => std::env::current_exe().map_err(|e| RpcClientError(e.to_string()))?,
        };
        let mut command = Command::new(executable);
        command
            .args(&self.options.prefix_args)
            .args(&args)
            .envs(self.options.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &self.options.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|e| RpcClientError(e.to_string()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self.stdin.lock().await = stdin.map(|s: ChildStdin| Box::new(s) as _);

        // Collect stderr for debugging.
        if let Some(mut stderr) = stderr {
            let shared = self.shared.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    eprint!("{text}");
                    lock(&shared.stderr).push_str(&text);
                }
            });
        }

        let child = Arc::new(tokio::sync::Mutex::new(child));
        let (exit_tx, exit_rx) = tokio::sync::watch::channel(false);
        if let Some(stdout) = stdout {
            let shared = self.shared.clone();
            let child = child.clone();
            tokio::spawn(async move {
                read_lines(stdout, &shared).await;
                // stdout closed: the child is exiting.
                let code = child.lock().await.wait().await.ok().and_then(|s| s.code());
                shared.handle_exit(code);
                let _ = exit_tx.send(true);
            });
        }
        self.child = Some(child.clone());
        self.exit_rx = Some(exit_rx);

        // Wait a moment for the process to initialize.
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(Some(status)) = child.lock().await.try_wait() {
            return Err(RpcClientError(format!(
                "Agent process exited immediately with code {}. Stderr: {}",
                status
                    .code()
                    .map_or_else(|| "null".into(), |c| c.to_string()),
                self.shared.stderr()
            )));
        }
        Ok(())
    }

    /// Drive an agent over any stream pair (in-process tests, custom transports).
    pub fn attach<R, W>(options: RpcClientOptions, reader: R, writer: W) -> Self
    where
        R: tokio::io::AsyncRead + Send + Unpin + 'static,
        W: tokio::io::AsyncWrite + Send + Unpin + 'static,
    {
        let client = Self::new(options);
        let shared = client.shared.clone();
        let (exit_tx, exit_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            read_lines(reader, &shared).await;
            shared.handle_exit(None);
            let _ = exit_tx.send(true);
        });
        if let Ok(mut sink) = client.stdin.try_lock() {
            *sink = Some(Box::new(writer));
        }
        Self {
            exit_rx: Some(exit_rx),
            ..client
        }
    }

    /// `stop()`: terminate the process (SIGKILL after a second).
    pub async fn stop(&mut self) {
        *self.stdin.lock().await = None;
        let Some(child) = self.child.take() else {
            return;
        };
        #[cfg(unix)]
        if let Some(pid) = child.lock().await.id() {
            // SAFETY: kill(2) on our own child's pid (hoocode sends SIGTERM).
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        let exited = match self.exit_rx.as_mut() {
            Some(rx) => tokio::time::timeout(Duration::from_secs(1), rx.wait_for(|e| *e))
                .await
                .is_ok(),
            None => false,
        };
        if !exited {
            let _ = child.lock().await.start_kill();
        }
        lock(&self.shared.pending).clear();
    }

    /// `onEvent`: every non-response line.
    pub fn on_event(&self, listener: impl Fn(&Value) + Send + Sync + 'static) -> Subscription {
        let id = self.shared.next_listener.fetch_add(1, Ordering::SeqCst);
        lock(&self.shared.listeners).push((id, Arc::new(listener)));
        let shared = self.shared.clone();
        Subscription {
            cancel: Some(Box::new(move || {
                lock(&shared.listeners).retain(|(i, _)| *i != id);
            })),
        }
    }

    /// `onExit`.
    pub fn on_exit(&self, listener: impl Fn(Option<i32>) + Send + Sync + 'static) -> Subscription {
        let id = self.shared.next_listener.fetch_add(1, Ordering::SeqCst);
        lock(&self.shared.exit_listeners).push((id, Arc::new(listener)));
        let shared = self.shared.clone();
        Subscription {
            cancel: Some(Box::new(move || {
                lock(&shared.exit_listeners).retain(|(i, _)| *i != id);
            })),
        }
    }

    /// `getStderr()`.
    pub fn stderr(&self) -> String {
        self.shared.stderr()
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    /// `prompt`: returns once the agent accepted it; events follow.
    pub async fn prompt(&self, message: &str, images: Option<&[ImageContent]>) -> Result<()> {
        self.send_ok(with_images(
            json!({"type": "prompt", "message": message}),
            images,
        ))
        .await
    }

    pub async fn steer(&self, message: &str, images: Option<&[ImageContent]>) -> Result<()> {
        self.send_ok(with_images(
            json!({"type": "steer", "message": message}),
            images,
        ))
        .await
    }

    pub async fn follow_up(&self, message: &str, images: Option<&[ImageContent]>) -> Result<()> {
        self.send_ok(with_images(
            json!({"type": "follow_up", "message": message}),
            images,
        ))
        .await
    }

    pub async fn abort(&self) -> Result<()> {
        self.send_ok(json!({"type": "abort"})).await
    }

    /// `{ cancelled }`.
    pub async fn new_session(&self, parent_session: Option<&str>) -> Result<Value> {
        let mut command = json!({"type": "new_session"});
        if let Some(parent) = parent_session {
            command["parentSession"] = parent.into();
        }
        self.send_data(command).await
    }

    /// `RpcSessionState`.
    pub async fn get_state(&self) -> Result<Value> {
        self.send_data(json!({"type": "get_state"})).await
    }

    /// The selected `Model`.
    pub async fn set_model(&self, provider: &str, model_id: &str) -> Result<Value> {
        self.send_data(json!({"type": "set_model", "provider": provider, "modelId": model_id}))
            .await
    }

    /// `{ model, thinkingLevel, isScoped }` or `null`.
    pub async fn cycle_model(&self) -> Result<Value> {
        self.send_data(json!({"type": "cycle_model"})).await
    }

    /// The `models` array.
    pub async fn get_available_models(&self) -> Result<Vec<Value>> {
        let data = self
            .send_data(json!({"type": "get_available_models"}))
            .await?;
        Ok(array(&data, "models"))
    }

    pub async fn set_thinking_level(&self, level: &str) -> Result<()> {
        self.send_ok(json!({"type": "set_thinking_level", "level": level}))
            .await
    }

    /// `{ level }` or `null`.
    pub async fn cycle_thinking_level(&self) -> Result<Value> {
        self.send_data(json!({"type": "cycle_thinking_level"}))
            .await
    }

    /// `"all" | "one-at-a-time"`.
    pub async fn set_steering_mode(&self, mode: &str) -> Result<()> {
        self.send_ok(json!({"type": "set_steering_mode", "mode": mode}))
            .await
    }

    pub async fn set_follow_up_mode(&self, mode: &str) -> Result<()> {
        self.send_ok(json!({"type": "set_follow_up_mode", "mode": mode}))
            .await
    }

    /// `CompactionResult`.
    pub async fn compact(&self, custom_instructions: Option<&str>) -> Result<Value> {
        let mut command = json!({"type": "compact"});
        if let Some(instructions) = custom_instructions {
            command["customInstructions"] = instructions.into();
        }
        self.send_data(command).await
    }

    pub async fn set_auto_compaction(&self, enabled: bool) -> Result<()> {
        self.send_ok(json!({"type": "set_auto_compaction", "enabled": enabled}))
            .await
    }

    pub async fn set_auto_retry(&self, enabled: bool) -> Result<()> {
        self.send_ok(json!({"type": "set_auto_retry", "enabled": enabled}))
            .await
    }

    pub async fn abort_retry(&self) -> Result<()> {
        self.send_ok(json!({"type": "abort_retry"})).await
    }

    /// `BashResult`.
    pub async fn bash(&self, command: &str) -> Result<Value> {
        self.send_data(json!({"type": "bash", "command": command}))
            .await
    }

    pub async fn abort_bash(&self) -> Result<()> {
        self.send_ok(json!({"type": "abort_bash"})).await
    }

    /// `SessionStats`.
    pub async fn get_session_stats(&self) -> Result<Value> {
        self.send_data(json!({"type": "get_session_stats"})).await
    }

    /// `{ path }`.
    pub async fn export_html(&self, output_path: Option<&str>) -> Result<Value> {
        let mut command = json!({"type": "export_html"});
        if let Some(path) = output_path {
            command["outputPath"] = path.into();
        }
        self.send_data(command).await
    }

    /// `{ cancelled }`.
    pub async fn switch_session(&self, session_path: &str) -> Result<Value> {
        self.send_data(json!({"type": "switch_session", "sessionPath": session_path}))
            .await
    }

    /// `{ text, cancelled }`.
    pub async fn fork(&self, entry_id: &str) -> Result<Value> {
        self.send_data(json!({"type": "fork", "entryId": entry_id}))
            .await
    }

    /// `{ cancelled }`.
    pub async fn clone_session(&self) -> Result<Value> {
        self.send_data(json!({"type": "clone"})).await
    }

    /// `[{ entryId, text }]`.
    pub async fn get_fork_messages(&self) -> Result<Vec<Value>> {
        let data = self.send_data(json!({"type": "get_fork_messages"})).await?;
        Ok(array(&data, "messages"))
    }

    pub async fn get_last_assistant_text(&self) -> Result<Option<String>> {
        let data = self
            .send_data(json!({"type": "get_last_assistant_text"}))
            .await?;
        Ok(data["text"].as_str().map(String::from))
    }

    pub async fn set_session_name(&self, name: &str) -> Result<()> {
        self.send_ok(json!({"type": "set_session_name", "name": name}))
            .await
    }

    /// `AgentMessage[]`.
    pub async fn get_messages(&self) -> Result<Vec<Value>> {
        let data = self.send_data(json!({"type": "get_messages"})).await?;
        Ok(array(&data, "messages"))
    }

    /// `RpcSlashCommand[]`.
    pub async fn get_commands(&self) -> Result<Vec<Value>> {
        let data = self.send_data(json!({"type": "get_commands"})).await?;
        Ok(array(&data, "commands"))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// `waitForIdle`: until the next `agent_end`.
    pub async fn wait_for_idle(&self, timeout: Duration) -> Result<()> {
        self.collect_events(timeout).await.map(|_| ())
    }

    /// `collectEvents`: every event up to and including the next `agent_end`.
    pub async fn collect_events(&self, timeout: Duration) -> Result<Vec<Value>> {
        let (subscription, exit, rx) = self.event_collector()?;
        let result = self.finish_collecting(rx, timeout).await;
        drop((subscription, exit));
        result
    }

    /// `promptAndWait`: prompt, then the events up to `agent_end`.
    pub async fn prompt_and_wait(
        &self,
        message: &str,
        images: Option<&[ImageContent]>,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        // Listen before sending so no event is missed.
        let (subscription, exit, rx) = self.event_collector()?;
        self.prompt(message, images).await?;
        let result = self.finish_collecting(rx, timeout).await;
        drop((subscription, exit));
        result
    }

    #[allow(clippy::type_complexity)]
    fn event_collector(
        &self,
    ) -> Result<(
        Subscription,
        Subscription,
        mpsc::UnboundedReceiver<std::result::Result<Value, Option<i32>>>,
    )> {
        if self.shared.exited.load(Ordering::SeqCst) {
            return Err(RpcClientError(format!(
                "Agent process already exited. Stderr: {}",
                self.shared.stderr()
            )));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let events = tx.clone();
        let subscription = self.on_event(move |event| {
            let _ = events.send(Ok(event.clone()));
        });
        let exit = self.on_exit(move |code| {
            let _ = tx.send(Err(code));
        });
        Ok((subscription, exit, rx))
    }

    async fn finish_collecting(
        &self,
        mut rx: mpsc::UnboundedReceiver<std::result::Result<Value, Option<i32>>>,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        let mut events = Vec::new();
        let collect = async {
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(event) => {
                        let done = event["type"] == "agent_end";
                        events.push(event);
                        if done {
                            return Ok(());
                        }
                    }
                    Err(code) => {
                        return Err(RpcClientError(format!(
                            "Agent process exited (code {}) before becoming idle. Stderr: {}",
                            code.map_or_else(|| "null".to_string(), |c| c.to_string()),
                            self.shared.stderr()
                        )))
                    }
                }
            }
            Ok(())
        };
        match tokio::time::timeout(timeout, collect).await {
            Ok(Ok(())) => Ok(events),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(RpcClientError(format!(
                "Timeout collecting events. Stderr: {}",
                self.shared.stderr()
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Internal
    // -----------------------------------------------------------------------

    /// `send`: write the command with a fresh `req_<n>` id, wait for its response.
    pub async fn send(&self, command: Value) -> Result<Value> {
        if self.shared.exited.load(Ordering::SeqCst) {
            return Err(RpcClientError(format!(
                "Agent process has exited. Stderr: {}",
                self.shared.stderr()
            )));
        }
        let command_type = command["type"].as_str().unwrap_or_default().to_string();
        let id = format!("req_{}", self.request_id.fetch_add(1, Ordering::SeqCst) + 1);
        let mut full = match command {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        full.insert("id".into(), id.clone().into());
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.pending).insert(id.clone(), tx);
        {
            let mut sink = self.stdin.lock().await;
            let Some(stdin) = sink.as_mut() else {
                lock(&self.shared.pending).remove(&id);
                return Err(RpcClientError("Client not started".into()));
            };
            let line = serialize_json_line(&Value::Object(full));
            if let Err(e) = async {
                stdin.write_all(line.as_bytes()).await?;
                stdin.flush().await
            }
            .await
            {
                lock(&self.shared.pending).remove(&id);
                return Err(RpcClientError(e.to_string()));
            }
        }
        match tokio::time::timeout(RESPONSE_TIMEOUT, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcClientError(format!(
                "Agent process has exited. Stderr: {}",
                self.shared.stderr()
            ))),
            Err(_) => {
                lock(&self.shared.pending).remove(&id);
                Err(RpcClientError(format!(
                    "Timeout waiting for response to {command_type}. Stderr: {}",
                    self.shared.stderr()
                )))
            }
        }
    }

    async fn send_ok(&self, command: Value) -> Result<()> {
        let response = self.send(command).await?;
        get_data(&response).map(|_| ())
    }

    async fn send_data(&self, command: Value) -> Result<Value> {
        let response = self.send(command).await?;
        get_data(&response)
    }
}

/// `getData`: the response's `data`, or its `error` as the error.
pub fn get_data(response: &Value) -> Result<Value> {
    if response["success"] != true {
        return Err(RpcClientError(
            response["error"].as_str().unwrap_or_default().to_string(),
        ));
    }
    Ok(response.get("data").cloned().unwrap_or(Value::Null))
}

fn with_images(mut command: Value, images: Option<&[ImageContent]>) -> Value {
    if let Some(images) = images {
        command["images"] = serde_json::to_value(images).unwrap_or_default();
    }
    command
}

fn array(data: &Value, key: &str) -> Vec<Value> {
    data[key].as_array().cloned().unwrap_or_default()
}

async fn read_lines<R: tokio::io::AsyncRead + Unpin>(mut reader: R, shared: &Shared) {
    let mut lines = JsonlLineReader::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for line in lines.push(&buf[..n]) {
                    shared.handle_line(&line);
                }
            }
        }
    }
    if let Some(line) = lines.finish() {
        shared.handle_line(&line);
    }
}

//! The server: connections, loaded threads, turns and approvals.
//!
//! Threads are hoocode sessions. Each loaded thread has a set of subscribed
//! connections; every notification for it goes to all of them. Approval
//! requests also go to all of them under one id, and the first answer wins.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cortexcode_agent_types::{
    AgentEvent, AgentMessage, AgentToolCall, PermissionDecision, PermissionGate,
};
use cortexcode_ai_types::{AssistantMessageEvent, Content, ImageContent, StopReason};
use cortexcode_app_server_protocol::jsonrpc::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
};
use cortexcode_app_server_protocol::{
    self as proto, methods, ApprovalDecision, ApprovalResponse, Message, RequestId, SandboxPolicy,
    Thread, ThreadActiveFlag, ThreadItem, ThreadStatus, Turn, TurnError, TurnStatus, UserInput,
};
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionEvent, PromptOptions, SessionSubscription,
};
use cortexcode_code_permissions::{evaluate, Verdict};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::items::{self, ToolOutcome};
use crate::transport::{ConnectionId, MessageHandler};

/// Builds and finds sessions. The CLI implements this with hoocode's real
/// session setup; tests use the faux provider.
pub trait SessionFactory: Send + Sync + 'static {
    /// A new session in the server's workspace. `gate` must be installed as
    /// the session's permission gate.
    fn create(
        &self,
        model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String>;
    /// Open a saved session file.
    fn open(
        &self,
        path: &Path,
        model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String>;
    /// Saved sessions, newest first.
    fn list(&self) -> Vec<SavedSession>;
    /// Models for `model/list`: `(id, display name, is default)`.
    fn models(&self) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    /// The model a client names (`model` on `turn/start`), if usable.
    fn resolve_model(&self, _name: &str) -> Option<cortexcode_ai_types::Model> {
        None
    }
}

/// A saved session as `thread/list` sees it.
#[derive(Debug, Clone)]
pub struct SavedSession {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub name: Option<String>,
    pub preview: String,
    /// Unix seconds.
    pub created: i64,
    /// Unix seconds.
    pub modified: i64,
}

/// Server settings.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `userAgent` prefix and `cliVersion`.
    pub version: String,
    /// `codexHome` in `initialize`.
    pub home: String,
    /// The workspace every thread runs in.
    pub cwd: PathBuf,
}

/// The app-server. Cheap to clone.
#[derive(Clone)]
pub struct AppServer {
    inner: Arc<Inner>,
}

struct Inner {
    config: ServerConfig,
    factory: Arc<dyn SessionFactory>,
    state: Mutex<State>,
    next_connection: AtomicU64,
    next_request: AtomicI64,
    runtime: tokio::runtime::Handle,
}

#[derive(Default)]
struct State {
    connections: HashMap<ConnectionId, Connection>,
    threads: HashMap<String, Arc<LoadedThread>>,
    /// Server request id → the thread and the pending approval.
    requests: HashMap<RequestId, String>,
}

struct Connection {
    tx: mpsc::UnboundedSender<Value>,
    /// This connection's requests, handled one at a time in arrival order
    /// (clients pipeline `initialize` and the requests after it).
    requests: mpsc::UnboundedSender<(RequestId, String, Option<Value>)>,
    initialized: bool,
    opt_out: HashSet<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

type ReplyResult = Result<Value, (i64, String)>;

fn ok<T: Serialize>(value: T) -> ReplyResult {
    serde_json::to_value(value).map_err(|e| (INTERNAL_ERROR, e.to_string()))
}

fn parse<T: DeserializeOwned>(params: Option<Value>) -> Result<T, (i64, String)> {
    serde_json::from_value(params.unwrap_or(Value::Object(Default::default())))
        .map_err(|e| (INVALID_PARAMS, format!("invalid params: {e}")))
}

// ---------------------------------------------------------------------------
// Loaded threads

struct LoadedThread {
    id: String,
    session: AgentSession,
    server: Weak<Inner>,
    state: Mutex<ThreadState>,
    _subscription: Mutex<Option<SessionSubscription>>,
}

#[derive(Default)]
struct ThreadState {
    subscribers: HashSet<ConnectionId>,
    turn: Option<ActiveTurn>,
    approvals: HashMap<RequestId, PendingApproval>,
    /// Tools approved with `acceptForSession`.
    session_grants: HashSet<String>,
    created: i64,
}

struct ActiveTurn {
    id: String,
    started_ms: i64,
    items: Vec<ThreadItem>,
    /// The agent-message item being streamed, if any.
    message_item: Option<String>,
    /// Tool call id → (tool name, arguments).
    tools: HashMap<String, (String, Value)>,
    /// Tool calls the user declined.
    declined: HashSet<String>,
    interrupted: bool,
    cancel_requested: bool,
    error: Option<String>,
}

impl ActiveTurn {
    /// Interrupted or cancelled: no more approvals may be asked.
    fn stopping(&self) -> bool {
        self.interrupted || self.cancel_requested
    }
}

struct PendingApproval {
    /// The full request message, replayed to new subscribers.
    message: Value,
    answer: oneshot::Sender<ApprovalDecision>,
}

impl LoadedThread {
    fn server(&self) -> Option<Arc<Inner>> {
        self.server.upgrade()
    }

    fn status(&self) -> ThreadStatus {
        let state = lock(&self.state);
        if state.turn.is_none() {
            return ThreadStatus::Idle;
        }
        let flags = if state.approvals.is_empty() {
            vec![]
        } else {
            vec![ThreadActiveFlag::WaitingOnApproval]
        };
        ThreadStatus::Active {
            active_flags: flags,
        }
    }

    fn notify<T: Serialize>(&self, method: &str, params: &T) {
        let Some(server) = self.server() else { return };
        let subscribers: Vec<ConnectionId> =
            lock(&self.state).subscribers.iter().copied().collect();
        server.send_to(
            &subscribers,
            &proto::notification(method, params),
            Some(method),
        );
    }

    fn notify_status(&self) {
        self.notify(
            methods::THREAD_STATUS_CHANGED,
            &proto::ThreadStatusChangedNotification {
                thread_id: self.id.clone(),
                status: self.status(),
            },
        );
    }

    /// Deny every open approval (interrupt, unload, shutdown).
    fn deny_all_approvals(&self) {
        let pending: Vec<(RequestId, PendingApproval)> =
            lock(&self.state).approvals.drain().collect();
        if let Some(server) = self.server() {
            let mut state = lock(&server.state);
            for (id, _) in &pending {
                state.requests.remove(id);
            }
        }
        for (id, approval) in pending {
            let _ = approval.answer.send(ApprovalDecision::Decline);
            self.notify(
                methods::SERVER_REQUEST_RESOLVED,
                &proto::ServerRequestResolvedNotification {
                    thread_id: self.id.clone(),
                    request_id: id,
                },
            );
        }
    }

    // --- session events → notifications -------------------------------

    fn on_event(&self, event: &AgentSessionEvent) {
        let AgentSessionEvent::Agent(event) = event else {
            return;
        };
        match event {
            AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => {
                if let AssistantMessageEvent::TextDelta { delta, .. } =
                    assistant_message_event.as_ref()
                {
                    self.on_text_delta(delta);
                }
            }
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant(message),
            } => {
                let mut thinking = Vec::new();
                let mut text = String::new();
                for block in &message.content {
                    match block {
                        Content::Thinking(t) if !t.thinking.is_empty() => {
                            thinking.push(t.thinking.clone())
                        }
                        Content::Text(t) => text.push_str(&t.text),
                        _ => {}
                    }
                }
                if !thinking.is_empty() {
                    let item = ThreadItem::Reasoning {
                        id: new_id(),
                        summary: vec![],
                        content: thinking,
                    };
                    self.start_item(item.clone());
                    self.complete_item(item);
                }
                let streamed = self.with_turn(|t| t.message_item.take()).flatten();
                if !text.is_empty() || streamed.is_some() {
                    let id = match streamed {
                        Some(id) => id,
                        None => {
                            let id = new_id();
                            self.start_item(ThreadItem::AgentMessage {
                                id: id.clone(),
                                text: String::new(),
                            });
                            id
                        }
                    };
                    self.complete_item(ThreadItem::AgentMessage { id, text });
                }
                if matches!(message.stop_reason, StopReason::Error) {
                    let error = message
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "model error".into());
                    self.with_turn(|t| t.error = Some(error));
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                self.with_turn(|t| {
                    t.tools
                        .insert(tool_call_id.clone(), (tool_name.clone(), args.clone()))
                });
                let cwd = self.session.cwd().to_string_lossy().into_owned();
                self.start_item(items::tool_item_started(
                    tool_call_id,
                    tool_name,
                    args,
                    &cwd,
                ));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let (args, declined) = self
                    .with_turn(|t| {
                        let args = t
                            .tools
                            .remove(tool_call_id)
                            .map(|(_, a)| a)
                            .unwrap_or(Value::Null);
                        (args, t.declined.remove(tool_call_id))
                    })
                    .unwrap_or((Value::Null, false));
                let cwd = self.session.cwd().to_string_lossy().into_owned();
                let outcome = if declined {
                    ToolOutcome::Declined
                } else {
                    ToolOutcome::Done {
                        result,
                        is_error: *is_error,
                    }
                };
                self.complete_item(items::tool_item_completed(
                    tool_call_id,
                    tool_name,
                    &args,
                    &cwd,
                    outcome,
                ));
            }
            AgentEvent::AgentEnd { messages } => {
                let aborted = messages.iter().rev().find_map(|m| match m {
                    AgentMessage::Assistant(a) => {
                        Some(matches!(a.stop_reason, StopReason::Aborted))
                    }
                    _ => None,
                });
                if aborted == Some(true) {
                    self.with_turn(|t| t.interrupted = true);
                }
            }
            _ => {}
        }
    }

    fn with_turn<R>(&self, f: impl FnOnce(&mut ActiveTurn) -> R) -> Option<R> {
        lock(&self.state).turn.as_mut().map(f)
    }

    fn turn_id(&self) -> Option<String> {
        lock(&self.state).turn.as_ref().map(|t| t.id.clone())
    }

    fn on_text_delta(&self, delta: &str) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        let (item_id, is_new) = match self.with_turn(|t| match &t.message_item {
            Some(id) => (id.clone(), false),
            None => {
                let id = new_id();
                t.message_item = Some(id.clone());
                (id, true)
            }
        }) {
            Some(v) => v,
            None => return,
        };
        if is_new {
            self.start_item(ThreadItem::AgentMessage {
                id: item_id.clone(),
                text: String::new(),
            });
        }
        self.notify(
            methods::AGENT_MESSAGE_DELTA,
            &proto::AgentMessageDeltaNotification {
                thread_id: self.id.clone(),
                turn_id,
                item_id,
                delta: delta.to_string(),
            },
        );
    }

    fn start_item(&self, item: ThreadItem) {
        let Some(turn_id) = self.turn_id() else {
            return;
        };
        self.notify(
            methods::ITEM_STARTED,
            &proto::ItemStartedNotification {
                item,
                thread_id: self.id.clone(),
                turn_id,
                started_at_ms: now_ms(),
            },
        );
    }

    fn complete_item(&self, item: ThreadItem) {
        let Some(turn_id) = self.with_turn(|t| {
            t.items.push(item.clone());
            t.id.clone()
        }) else {
            return;
        };
        self.notify(
            methods::ITEM_COMPLETED,
            &proto::ItemCompletedNotification {
                item,
                thread_id: self.id.clone(),
                turn_id,
                completed_at_ms: now_ms(),
            },
        );
    }

    /// End the active turn and send `turn/completed`.
    fn finish_turn(&self, failure: Option<String>) {
        let Some(turn) = lock(&self.state).turn.take() else {
            return;
        };
        // Approvals can't outlive their turn.
        self.deny_all_approvals();
        let error = failure.or(turn.error);
        let status = if turn.interrupted || turn.cancel_requested {
            TurnStatus::Interrupted
        } else if error.is_some() {
            TurnStatus::Failed
        } else {
            TurnStatus::Completed
        };
        let turn_error = error.map(|message| TurnError {
            message,
            additional_details: None,
        });
        if status == TurnStatus::Failed {
            if let Some(error) = &turn_error {
                self.notify(
                    methods::ERROR,
                    &proto::ErrorNotification {
                        error: error.clone(),
                        will_retry: false,
                        thread_id: self.id.clone(),
                        turn_id: turn.id.clone(),
                    },
                );
            }
        }
        let completed_ms = now_ms();
        self.notify(
            methods::TURN_COMPLETED,
            &proto::TurnCompletedNotification {
                thread_id: self.id.clone(),
                turn: Turn {
                    id: turn.id,
                    items: turn.items,
                    status,
                    error: turn_error,
                    started_at: Some(turn.started_ms / 1000),
                    completed_at: Some(completed_ms / 1000),
                    duration_ms: Some(completed_ms - turn.started_ms),
                },
            },
        );
        self.notify_status();
        if let Some(server) = self.server() {
            server.unload_if_unused(&self.id);
        }
    }

    // --- approvals ------------------------------------------------------

    /// Ask every subscriber; block until one answers or the turn ends.
    fn ask(&self, call: &AgentToolCall) -> ApprovalDecision {
        let Some(server) = self.server() else {
            return ApprovalDecision::Decline;
        };
        let Some(turn_id) = self.turn_id() else {
            return ApprovalDecision::Decline;
        };
        let request_id = RequestId::Integer(server.next_request.fetch_add(1, Ordering::SeqCst));
        let started_at_ms = now_ms();
        let cwd = self.session.cwd().to_string_lossy().into_owned();
        let message = match items::tool_kind(&call.name) {
            items::ToolKind::FileChange => proto::request(
                request_id.clone(),
                methods::FILE_CHANGE_REQUEST_APPROVAL,
                &proto::FileChangeRequestApprovalParams {
                    thread_id: self.id.clone(),
                    turn_id,
                    item_id: call.id.clone(),
                    started_at_ms,
                    reason: Some(format!(
                        "Allow: {}",
                        cortexcode_code_permissions::describe_tool(&call.name, &call.arguments)
                    )),
                    grant_root: None,
                },
            ),
            kind => proto::request(
                request_id.clone(),
                methods::COMMAND_EXECUTION_REQUEST_APPROVAL,
                &proto::CommandExecutionRequestApprovalParams {
                    thread_id: self.id.clone(),
                    turn_id,
                    item_id: call.id.clone(),
                    started_at_ms,
                    command: Some(if kind == items::ToolKind::Command {
                        call.arguments
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        cortexcode_code_permissions::describe_tool(&call.name, &call.arguments)
                    }),
                    cwd: Some(cwd),
                    reason: (kind == items::ToolKind::Dynamic)
                        .then(|| format!("hoocode tool `{}`", call.name)),
                },
            ),
        };
        let (tx, rx) = oneshot::channel();
        let subscribers: Vec<ConnectionId> = {
            let mut state = lock(&self.state);
            // Checked under the same lock interrupt takes to deny approvals,
            // so a call can't slip in after the interrupt and ask again.
            if state.turn.as_ref().is_none_or(ActiveTurn::stopping) {
                return ApprovalDecision::Decline;
            }
            state.approvals.insert(
                request_id.clone(),
                PendingApproval {
                    message: message.clone(),
                    answer: tx,
                },
            );
            state.subscribers.iter().copied().collect()
        };
        lock(&server.state)
            .requests
            .insert(request_id, self.id.clone());
        self.notify_status();
        server.send_to(&subscribers, &message, None);
        drop(server);

        let decision =
            tokio::task::block_in_place(|| rx.blocking_recv()).unwrap_or(ApprovalDecision::Decline);
        self.notify_status();
        decision
    }

    /// A client answered approval `id`. First answer wins.
    fn answer(&self, id: &RequestId, decision: ApprovalDecision) {
        let Some(approval) = lock(&self.state).approvals.remove(id) else {
            return;
        };
        let _ = approval.answer.send(decision);
        self.notify(
            methods::SERVER_REQUEST_RESOLVED,
            &proto::ServerRequestResolvedNotification {
                thread_id: self.id.clone(),
                request_id: id.clone(),
            },
        );
    }

    fn pending_approval_messages(&self) -> Vec<Value> {
        lock(&self.state)
            .approvals
            .values()
            .map(|a| a.message.clone())
            .collect()
    }

    fn to_thread(&self, include_turns: bool) -> Thread {
        let messages = self.session.messages();
        let cwd = self.session.cwd().to_string_lossy().into_owned();
        let model = self.session.model();
        let created = lock(&self.state).created;
        let mut turns = if include_turns {
            items::turns_from_messages(&messages, &cwd)
        } else {
            vec![]
        };
        // A running turn's id must match what clients were told.
        if include_turns {
            if let Some(active) = &lock(&self.state).turn {
                if let Some(last) = turns.last_mut() {
                    last.id = active.id.clone();
                    last.status = TurnStatus::InProgress;
                    last.completed_at = None;
                }
            }
        }
        Thread {
            id: self.id.clone(),
            session_id: self.id.clone(),
            preview: items::preview(&messages),
            ephemeral: false,
            model_provider: model
                .as_ref()
                .map(|m| m.provider.clone())
                .unwrap_or_default(),
            cwd,
            cli_version: self
                .server()
                .map(|s| s.config.version.clone())
                .unwrap_or_default(),
            source: "appServer".into(),
            status: self.status(),
            created_at: created,
            updated_at: now_ms() / 1000,
            turns: std::mem::take(&mut turns),
            path: self
                .session
                .session_file()
                .map(|p| p.to_string_lossy().into_owned()),
            name: self.session.session_name(),
            model: model.map(|m| m.id),
            project_id: None,
        }
    }
}

/// The permission gate the server installs in every session: hoocode's policy
/// decides whether to ask; asking goes to the thread's clients.
struct ServerGate {
    cwd: PathBuf,
    thread: Mutex<Weak<LoadedThread>>,
}

impl PermissionGate for ServerGate {
    fn request(&self, call: &AgentToolCall) -> PermissionDecision {
        let config = cortexcode_code_modes::config::read_merged_config(&self.cwd);
        match evaluate(&config, &self.cwd, &call.name, &call.arguments, true) {
            Verdict::Allow => PermissionDecision::Grant,
            Verdict::Block(reason) => PermissionDecision::Deny { reason },
            Verdict::Prompt => {
                let Some(thread) = lock(&self.thread).upgrade() else {
                    return deny();
                };
                if lock(&thread.state).session_grants.contains(&call.name) {
                    return PermissionDecision::Grant;
                }
                match thread.ask(call) {
                    ApprovalDecision::Accept | ApprovalDecision::AcceptWithAmendment => {
                        PermissionDecision::Grant
                    }
                    ApprovalDecision::AcceptForSession => {
                        lock(&thread.state).session_grants.insert(call.name.clone());
                        PermissionDecision::Grant
                    }
                    ApprovalDecision::Decline => {
                        thread.with_turn(|t| t.declined.insert(call.id.clone()));
                        deny()
                    }
                    ApprovalDecision::Cancel => {
                        thread.with_turn(|t| {
                            t.declined.insert(call.id.clone());
                            t.cancel_requested = true;
                        });
                        thread.deny_all_approvals();
                        let session = thread.session.clone();
                        if let Some(server) = thread.server() {
                            server.runtime.spawn(async move { session.abort().await });
                        }
                        deny()
                    }
                }
            }
        }
    }
}

fn deny() -> PermissionDecision {
    PermissionDecision::Deny {
        reason: "Denied by permission gate".into(),
    }
}

// ---------------------------------------------------------------------------
// Server

impl AppServer {
    /// Must be called inside a multi-thread tokio runtime.
    pub fn new(config: ServerConfig, factory: Arc<dyn SessionFactory>) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                factory,
                state: Mutex::new(State::default()),
                next_connection: AtomicU64::new(1),
                next_request: AtomicI64::new(1),
                runtime: tokio::runtime::Handle::current(),
            }),
        }
    }

    /// The transport-facing side.
    pub fn handler(&self) -> Arc<dyn MessageHandler> {
        Arc::new(self.clone())
    }

    /// Deny open approvals and stop running turns.
    pub async fn shutdown(&self) {
        let threads: Vec<Arc<LoadedThread>> =
            lock(&self.inner.state).threads.values().cloned().collect();
        for thread in threads {
            thread.deny_all_approvals();
            if thread.session.is_streaming() {
                let _ = tokio::time::timeout(Duration::from_secs(5), thread.session.abort()).await;
            }
        }
    }
}

impl MessageHandler for AppServer {
    fn connect(&self) -> (ConnectionId, mpsc::UnboundedReceiver<Value>) {
        let id = self.inner.next_connection.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::unbounded_channel();
        let (requests, mut queue) = mpsc::unbounded_channel::<(RequestId, String, Option<Value>)>();
        lock(&self.inner.state).connections.insert(
            id,
            Connection {
                tx,
                requests,
                initialized: false,
                opt_out: HashSet::new(),
            },
        );
        let inner = self.inner.clone();
        self.inner.runtime.spawn(async move {
            while let Some((request_id, method, params)) = queue.recv().await {
                let reply = inner.handle_request(id, &method, params).await;
                let mut after = Vec::new();
                let value = match reply {
                    Ok(result) => {
                        after = inner.after_response(&method, &result);
                        proto::response(request_id, &result)
                    }
                    Err((code, message)) => proto::error_response(Some(request_id), code, message),
                };
                inner.send_to(&[id], &value, None);
                for message in after {
                    inner.send_to(&[id], &message, None);
                }
            }
        });
        (id, rx)
    }

    fn receive(&self, connection: ConnectionId, message: Result<Value, String>) {
        let inner = self.inner.clone();
        let message = match message.and_then(Message::from_value) {
            Ok(m) => m,
            Err(e) => {
                inner.send_to(
                    &[connection],
                    &proto::error_response(None, proto::jsonrpc::PARSE_ERROR, e),
                    None,
                );
                return;
            }
        };
        match message {
            Message::Request { id, method, params } => {
                if let Some(conn) = lock(&inner.state).connections.get(&connection) {
                    let _ = conn.requests.send((id, method, params));
                }
            }
            Message::Notification { .. } => {}
            Message::Response { id, result } => inner.handle_answer(&id, Some(result)),
            Message::Error { id: Some(id), .. } => inner.handle_answer(&id, None),
            Message::Error { id: None, .. } => {}
        }
    }

    fn disconnect(&self, connection: ConnectionId) {
        let threads: Vec<Arc<LoadedThread>> = {
            let mut state = lock(&self.inner.state);
            state.connections.remove(&connection);
            state.threads.values().cloned().collect()
        };
        for thread in threads {
            let removed = lock(&thread.state).subscribers.remove(&connection);
            if removed {
                self.inner.unload_if_unused(&thread.id);
            }
        }
    }
}

impl Inner {
    fn send_to(&self, connections: &[ConnectionId], value: &Value, notification: Option<&str>) {
        let state = lock(&self.state);
        for id in connections {
            if let Some(conn) = state.connections.get(id) {
                if notification.is_some_and(|m| conn.opt_out.contains(m)) {
                    continue;
                }
                let _ = conn.tx.send(value.clone());
            }
        }
    }

    fn thread(&self, id: &str) -> Option<Arc<LoadedThread>> {
        lock(&self.state).threads.get(id).cloned()
    }

    /// Unload an idle thread nobody is subscribed to. Its session file stays.
    fn unload_if_unused(&self, id: &str) {
        let Some(thread) = self.thread(id) else {
            return;
        };
        {
            let state = lock(&thread.state);
            if !state.subscribers.is_empty() || state.turn.is_some() {
                return;
            }
        }
        lock(&self.state).threads.remove(id);
        thread.session.dispose();
    }

    fn handle_answer(&self, id: &RequestId, result: Option<Value>) {
        let Some(thread_id) = lock(&self.state).requests.remove(id) else {
            return;
        };
        let decision = result
            .and_then(|r| serde_json::from_value::<ApprovalResponse>(r).ok())
            .map(|r| r.decision)
            .unwrap_or(ApprovalDecision::Decline);
        if let Some(thread) = self.thread(&thread_id) {
            thread.answer(id, decision);
        }
    }

    async fn handle_request(
        self: &Arc<Self>,
        connection: ConnectionId,
        method: &str,
        params: Option<Value>,
    ) -> ReplyResult {
        if method == methods::INITIALIZE {
            return self.initialize(connection, parse(params)?);
        }
        let initialized = lock(&self.state)
            .connections
            .get(&connection)
            .is_some_and(|c| c.initialized);
        if !initialized {
            return Err((INVALID_REQUEST, "Not initialized".into()));
        }
        match method {
            methods::THREAD_START => self.thread_start(connection, parse(params)?).await,
            methods::THREAD_RESUME => self.thread_resume(connection, parse(params)?).await,
            methods::THREAD_LIST => self.thread_list(parse(params)?),
            methods::THREAD_READ => self.thread_read(parse(params)?),
            methods::THREAD_UNSUBSCRIBE => self.thread_unsubscribe(connection, parse(params)?),
            methods::TURN_START => self.turn_start(parse(params)?).await,
            methods::TURN_STEER => self.turn_steer(parse(params)?),
            methods::TURN_INTERRUPT => self.turn_interrupt(parse(params)?).await,
            methods::ACCOUNT_READ => ok(proto::GetAccountResponse {
                account: None,
                requires_openai_auth: false,
            }),
            methods::MODEL_LIST => ok(self.model_list()),
            methods::SKILLS_LIST => {
                let params: proto::SkillsListParams = parse(params)?;
                ok(proto::SkillsListResponse {
                    data: params
                        .cwds
                        .into_iter()
                        .map(|cwd| proto::SkillsListEntry {
                            cwd,
                            skills: vec![],
                            errors: vec![],
                        })
                        .collect(),
                })
            }
            methods::THREAD_LOADED_LIST => {
                let mut data: Vec<String> = lock(&self.state).threads.keys().cloned().collect();
                data.sort();
                ok(proto::ThreadLoadedListResponse {
                    data,
                    next_cursor: None,
                })
            }
            methods::CONFIG_REQUIREMENTS_READ => {
                ok(proto::ConfigRequirementsReadResponse::default())
            }
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }

    fn initialize(&self, connection: ConnectionId, params: proto::InitializeParams) -> ReplyResult {
        let mut state = lock(&self.state);
        let Some(conn) = state.connections.get_mut(&connection) else {
            return Err((INTERNAL_ERROR, "connection closed".into()));
        };
        if conn.initialized {
            return Err((INVALID_REQUEST, "Already initialized".into()));
        }
        conn.initialized = true;
        if let Some(caps) = params.capabilities {
            conn.opt_out = caps
                .opt_out_notification_methods
                .unwrap_or_default()
                .into_iter()
                .collect();
        }
        ok(proto::InitializeResponse {
            user_agent: format!("hoocode/{}", self.config.version),
            codex_home: self.config.home.clone(),
            platform_family: if cfg!(windows) { "windows" } else { "unix" }.into(),
            platform_os: std::env::consts::OS.into(),
        })
    }

    fn load(
        self: &Arc<Self>,
        session: impl FnOnce(Arc<dyn PermissionGate>) -> Result<AgentSession, String>,
        created: i64,
    ) -> Result<Arc<LoadedThread>, String> {
        let gate = Arc::new(ServerGate {
            cwd: self.config.cwd.clone(),
            thread: Mutex::new(Weak::new()),
        });
        let session = session(gate.clone())?;
        let id = session.session_id();
        if let Some(existing) = self.thread(&id) {
            session.dispose();
            return Ok(existing);
        }
        let thread = Arc::new(LoadedThread {
            id: id.clone(),
            session: session.clone(),
            server: Arc::downgrade(self),
            state: Mutex::new(ThreadState {
                created,
                ..Default::default()
            }),
            _subscription: Mutex::new(None),
        });
        *lock(&gate.thread) = Arc::downgrade(&thread);
        let weak = Arc::downgrade(&thread);
        let subscription = session.subscribe(move |event| {
            if let Some(thread) = weak.upgrade() {
                thread.on_event(event);
            }
        });
        *lock(&thread._subscription) = Some(subscription);
        lock(&self.state).threads.insert(id, thread.clone());
        Ok(thread)
    }

    fn subscribe(&self, thread: &LoadedThread, connection: ConnectionId) {
        lock(&thread.state).subscribers.insert(connection);
    }

    fn session_response(
        &self,
        thread: &LoadedThread,
        include_turns: bool,
    ) -> proto::ThreadSessionResponse {
        let model = thread.session.model();
        proto::ThreadSessionResponse {
            thread: thread.to_thread(include_turns),
            model: model.as_ref().map(|m| m.id.clone()).unwrap_or_default(),
            model_provider: model.map(|m| m.provider).unwrap_or_default(),
            service_tier: None,
            cwd: thread.session.cwd().to_string_lossy().into_owned(),
            instruction_sources: vec![],
            approval_policy: "on-request".into(),
            approvals_reviewer: "user".into(),
            sandbox: SandboxPolicy::DangerFullAccess,
            reasoning_effort: None,
        }
    }

    fn check_cwd(&self, cwd: Option<&str>) -> Result<(), (i64, String)> {
        let Some(cwd) = cwd else { return Ok(()) };
        let wanted = std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd));
        let ours =
            std::fs::canonicalize(&self.config.cwd).unwrap_or_else(|_| self.config.cwd.clone());
        if wanted == ours {
            Ok(())
        } else {
            Err((
                INVALID_PARAMS,
                format!(
                    "this server only serves {}; start one in {cwd} for that folder",
                    self.config.cwd.display()
                ),
            ))
        }
    }

    async fn thread_start(
        self: &Arc<Self>,
        connection: ConnectionId,
        params: proto::ThreadStartParams,
    ) -> ReplyResult {
        self.check_cwd(params.cwd.as_deref())?;
        let factory = self.factory.clone();
        let model = params.model.clone();
        let thread = self
            .load(
                |gate| factory.create(model.as_deref(), gate),
                now_ms() / 1000,
            )
            .map_err(|e| (INTERNAL_ERROR, e))?;
        self.subscribe(&thread, connection);
        let response = self.session_response(&thread, false);
        thread.notify(
            methods::THREAD_STARTED,
            &proto::ThreadStartedNotification {
                thread: response.thread.clone(),
            },
        );
        ok(response)
    }

    fn find_saved(&self, id: &str) -> Option<SavedSession> {
        self.factory.list().into_iter().find(|s| s.id == id)
    }

    async fn thread_resume(
        self: &Arc<Self>,
        connection: ConnectionId,
        params: proto::ThreadResumeParams,
    ) -> ReplyResult {
        let thread = match self.thread(&params.thread_id) {
            Some(thread) => thread,
            None => {
                let (path, created) = match (&params.path, self.find_saved(&params.thread_id)) {
                    (Some(path), saved) => {
                        (PathBuf::from(path), saved.map(|s| s.created).unwrap_or(0))
                    }
                    (None, Some(saved)) => (saved.path, saved.created),
                    (None, None) => {
                        return Err((INVALID_PARAMS, format!("no thread {}", params.thread_id)))
                    }
                };
                let factory = self.factory.clone();
                let model = params.model.clone();
                self.load(|gate| factory.open(&path, model.as_deref(), gate), created)
                    .map_err(|e| (INTERNAL_ERROR, e))?
            }
        };
        self.subscribe(&thread, connection);
        ok(self.session_response(&thread, true))
    }

    /// Messages a connection gets right after a successful response: open
    /// approvals on a thread it just resumed, so it can answer them.
    fn after_response(&self, method: &str, result: &Value) -> Vec<Value> {
        if method != methods::THREAD_RESUME {
            return vec![];
        }
        result["thread"]["id"]
            .as_str()
            .and_then(|id| self.thread(id))
            .map(|t| t.pending_approval_messages())
            .unwrap_or_default()
    }

    fn saved_to_thread(&self, saved: &SavedSession) -> Thread {
        if let Some(loaded) = self.thread(&saved.id) {
            return loaded.to_thread(false);
        }
        Thread {
            id: saved.id.clone(),
            session_id: saved.id.clone(),
            preview: saved.preview.clone(),
            ephemeral: false,
            model_provider: String::new(),
            cwd: saved.cwd.clone(),
            cli_version: self.config.version.clone(),
            source: "appServer".into(),
            status: ThreadStatus::NotLoaded,
            created_at: saved.created,
            updated_at: saved.modified,
            turns: vec![],
            path: Some(saved.path.to_string_lossy().into_owned()),
            name: saved.name.clone(),
            model: None,
            project_id: None,
        }
    }

    fn thread_list(&self, params: proto::ThreadListParams) -> ReplyResult {
        if params.archived == Some(true) {
            return ok(proto::ThreadListResponse {
                data: vec![],
                next_cursor: None,
                backwards_cursor: None,
            });
        }
        let saved = self.factory.list();
        let offset: usize = params
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let limit = params.limit.unwrap_or(50).clamp(1, 500) as usize;
        let page: Vec<Thread> = saved
            .iter()
            .skip(offset)
            .take(limit)
            .map(|s| self.saved_to_thread(s))
            .collect();
        let next = offset + page.len();
        ok(proto::ThreadListResponse {
            data: page,
            next_cursor: (next < saved.len()).then(|| next.to_string()),
            backwards_cursor: None,
        })
    }

    fn thread_read(&self, params: proto::ThreadReadParams) -> ReplyResult {
        if let Some(thread) = self.thread(&params.thread_id) {
            return ok(proto::ThreadReadResponse {
                thread: thread.to_thread(params.include_turns),
            });
        }
        let Some(saved) = self.find_saved(&params.thread_id) else {
            return Err((INVALID_PARAMS, format!("no thread {}", params.thread_id)));
        };
        let mut thread = self.saved_to_thread(&saved);
        if params.include_turns {
            let messages = read_messages(&saved.path);
            thread.turns = items::turns_from_messages(&messages, &saved.cwd);
        }
        ok(proto::ThreadReadResponse { thread })
    }

    fn thread_unsubscribe(
        &self,
        connection: ConnectionId,
        params: proto::ThreadUnsubscribeParams,
    ) -> ReplyResult {
        let Some(thread) = self.thread(&params.thread_id) else {
            return ok(proto::ThreadUnsubscribeResponse {
                status: proto::ThreadUnsubscribeStatus::NotLoaded,
            });
        };
        let removed = lock(&thread.state).subscribers.remove(&connection);
        if removed {
            self.unload_if_unused(&thread.id);
        }
        ok(proto::ThreadUnsubscribeResponse {
            status: if removed {
                proto::ThreadUnsubscribeStatus::Unsubscribed
            } else {
                proto::ThreadUnsubscribeStatus::NotSubscribed
            },
        })
    }

    fn loaded(&self, id: &str) -> Result<Arc<LoadedThread>, (i64, String)> {
        self.thread(id).ok_or_else(|| {
            (
                INVALID_PARAMS,
                format!("thread {id} is not loaded; call thread/resume first"),
            )
        })
    }

    async fn turn_start(self: &Arc<Self>, params: proto::TurnStartParams) -> ReplyResult {
        let thread = self.loaded(&params.thread_id)?;
        let text = items::input_text(&params.input);
        let images = input_images(&params.input)?;
        if text.trim().is_empty() && images.is_empty() {
            return Err((INVALID_PARAMS, "input is empty".into()));
        }
        // `model` switches this turn and the ones after it.
        if let Some(name) = params.model.as_deref() {
            let current = thread.session.model();
            let same = current
                .as_ref()
                .is_some_and(|m| name == m.id || name == format!("{}/{}", m.provider, m.id));
            if !same && !thread.session.is_streaming() {
                let model = self.factory.resolve_model(name).ok_or_else(|| {
                    (
                        INVALID_PARAMS,
                        format!("unknown or unavailable model {name}"),
                    )
                })?;
                thread
                    .session
                    .set_model(model)
                    .map_err(|e| (INVALID_PARAMS, e.to_string()))?;
            }
        }
        let turn_id = new_id();
        let started_ms = now_ms();
        {
            let mut state = lock(&thread.state);
            if state.turn.is_some() || thread.session.is_streaming() {
                return Err((
                    INVALID_REQUEST,
                    "a turn is already running; use turn/steer".into(),
                ));
            }
            state.turn = Some(ActiveTurn {
                id: turn_id.clone(),
                started_ms,
                items: vec![],
                message_item: None,
                tools: HashMap::new(),
                declined: HashSet::new(),
                interrupted: false,
                cancel_requested: false,
                error: None,
            });
        }
        let turn = Turn {
            id: turn_id.clone(),
            items: vec![],
            status: TurnStatus::InProgress,
            error: None,
            started_at: Some(started_ms / 1000),
            completed_at: None,
            duration_ms: None,
        };
        thread.notify(
            methods::TURN_STARTED,
            &proto::TurnStartedNotification {
                thread_id: thread.id.clone(),
                turn: turn.clone(),
            },
        );
        thread.notify_status();
        let user_item = ThreadItem::UserMessage {
            id: new_id(),
            content: params.input.clone(),
        };
        thread.start_item(user_item.clone());
        thread.complete_item(user_item);

        let run = thread.clone();
        self.runtime.spawn(async move {
            let options = PromptOptions {
                images,
                source: cortexcode_code_agent_session::InputSource::Rpc,
                ..Default::default()
            };
            let failure = run
                .session
                .prompt(&text, options)
                .await
                .err()
                .map(|e| e.to_string());
            run.finish_turn(failure);
        });
        ok(proto::TurnStartResponse { turn })
    }

    fn turn_steer(&self, params: proto::TurnSteerParams) -> ReplyResult {
        let thread = self.loaded(&params.thread_id)?;
        match thread.turn_id() {
            Some(id) if id == params.expected_turn_id => {}
            Some(id) => {
                return Err((
                    INVALID_REQUEST,
                    format!(
                        "expectedTurnId {} does not match the active turn {id}",
                        params.expected_turn_id
                    ),
                ))
            }
            None => return Err((INVALID_REQUEST, "no active turn to steer".into())),
        }
        let text = items::input_text(&params.input);
        let images = input_images(&params.input)?;
        thread
            .session
            .steer(&text, &images)
            .map_err(|e| (INVALID_REQUEST, e.to_string()))?;
        ok(proto::TurnSteerResponse {
            turn_id: params.expected_turn_id,
        })
    }

    async fn turn_interrupt(self: &Arc<Self>, params: proto::TurnInterruptParams) -> ReplyResult {
        let thread = self.loaded(&params.thread_id)?;
        if thread.turn_id().as_deref() != Some(params.turn_id.as_str()) {
            return Err((
                INVALID_REQUEST,
                format!("turn {} is not running", params.turn_id),
            ));
        }
        thread.with_turn(|t| t.interrupted = true);
        thread.deny_all_approvals();
        // Don't wait for the run to wind down: the client learns that from
        // `turn/completed`, and waiting here would block its other requests.
        let session = thread.session.clone();
        self.runtime.spawn(async move { session.abort().await });
        ok(proto::TurnInterruptResponse {})
    }

    fn model_list(&self) -> proto::ModelListResponse {
        let data = self
            .factory
            .models()
            .into_iter()
            .map(|(id, name, is_default)| proto::ModelInfo {
                id: id.clone(),
                model: id,
                display_name: name.clone(),
                description: name,
                hidden: false,
                is_default,
                default_reasoning_effort: "medium".into(),
                supported_reasoning_efforts: vec![proto::ReasoningEffortOption {
                    reasoning_effort: "medium".into(),
                    description: "Default".into(),
                }],
                input_modalities: vec!["text".into(), "image".into()],
            })
            .collect();
        proto::ModelListResponse {
            data,
            next_cursor: None,
        }
    }
}

/// Image inputs → hoocode images. Data URLs and local files only.
fn input_images(input: &[UserInput]) -> Result<Vec<ImageContent>, (i64, String)> {
    let mut images = Vec::new();
    for item in input {
        match item {
            UserInput::Image { url } => {
                let Some(rest) = url.strip_prefix("data:") else {
                    return Err((INVALID_PARAMS, "only data: image URLs are supported".into()));
                };
                let Some((mime, data)) = rest.split_once(";base64,") else {
                    return Err((INVALID_PARAMS, "image URL must be base64".into()));
                };
                images.push(ImageContent {
                    data: data.into(),
                    media_type: mime.into(),
                });
            }
            UserInput::LocalImage { path } => {
                let bytes =
                    std::fs::read(path).map_err(|e| (INVALID_PARAMS, format!("{path}: {e}")))?;
                let mime = match Path::new(path).extension().and_then(|e| e.to_str()) {
                    Some("png") => "image/png",
                    Some("jpg" | "jpeg") => "image/jpeg",
                    Some("gif") => "image/gif",
                    Some("webp") => "image/webp",
                    _ => return Err((INVALID_PARAMS, format!("{path}: unsupported image type"))),
                };
                images.push(ImageContent {
                    data: base64_encode(&bytes),
                    media_type: mime.into(),
                });
            }
            _ => {}
        }
    }
    Ok(images)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Messages of a saved session file.
fn read_messages(path: &Path) -> Vec<AgentMessage> {
    let manager = cortexcode_code_session::SessionManager::open(path, None, None);
    manager.build_context().messages
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}

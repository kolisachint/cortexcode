//! `core/subagent-pool.ts`: runs subagents as child processes with bounded
//! concurrency, a priority FIFO queue and automatic slot refill.
//!
//! Each child is `<executable> [prefix args] --mode json --session <file>
//! --task-id <id> ... <prompt>`. Its stdout carries progress events,
//! `message_end` (token usage) and `{"ping":true}` heartbeats; its success is a
//! verified `result.json` in `.hoocode/dispatch/<task_id>/`, not its exit code.
//!
//! Events (`PoolEvent::name`): `task_done`, `task_failed`, `task_stalled`,
//! `task_timeout`, `task_cancelled`, `budget_warning`, `budget_exceeded`
//! (advisory, never kills) and `task_progress` (coarse lifecycle events for
//! the UI).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cortexcode_ai_types::Model;
use cortexcode_code_resources::{
    load_agent_registry, AgentDefinition, AgentRegistry, AgentSource, LoadAgentRegistryOptions,
    MODEL_INHERIT,
};
use cortexcode_code_rpc::JsonlLineReader;
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;

use crate::agent_log::agent_log;
use crate::depth::{
    current_subagent_depth, resolve_max_subagent_depth, tool_allowlist_needs_mcp,
    DEFER_MCP_SCHEMAS_ENV, SUBAGENT_DEPTH_ENV, SUBAGENT_SKIP_MCP_ENV,
};
use crate::dispatch::DispatchEvaluator;
use crate::events::{classify_subagent_line, SubagentStdoutLine};
use crate::lifeguard::{kill_process_tree, LifeguardEvent, SubagentLifeguard};
use crate::model_categories::{resolve_model_reference, CategorySettings};
use crate::output_verifier::OutputVerifier;
use crate::result::write_file_atomic;
use crate::token_budget::{TokenBudget, TokenBudgetOptions};

/// Provider/model failures where retrying with the parent's model can recover.
static INHERITED_MODEL_FALLBACK_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)usage[_\s-]?limit|subscription|quota|rate.?limit|too many requests|429|insufficient|out of credit|credit balance|billing|payment required|402|model[^\n]*(not found|unavailable|not available|not supported|does not exist|invalid|unsupported)|no api key|no auth configured|authentication|unauthorized|forbidden|permission",
    )
    .expect("valid regex")
});

/// Hard cap on assistant turns when an agent does not set `maxTurns` (the
/// token budget only warns, so this is the guaranteed stop).
pub const DEFAULT_SUBAGENT_MAX_TURNS: u64 = 50;

/// Tail cap on the captured stdout/stderr per task.
const MAX_CAPTURED_STREAM_CHARS: usize = 256 * 1024;
/// Cap on one un-terminated stdout line (a runaway writer is dropped).
const MAX_SUBAGENT_EVENT_LINE_CHARS: usize = 8 * 1024 * 1024;
/// How long to wait for stdio to close after the child exits (grandchildren
/// may hold the pipes).
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

/// The env prefix cortex stamps on children (read back via any prefix).
const CHILD_ENV_PREFIX: &str = "CORTEXCODE_";

/// `SubagentPoolTask`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentPoolTask {
    pub task_id: String,
    pub agent_type: String,
    pub task: String,
    pub context: Option<String>,
    pub token_budget: Option<u64>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Session file to persist/continue (default: the task's dispatch dir).
    pub session_file: Option<PathBuf>,
    /// Internal: retry with the caller's model after the preferred one failed.
    pub use_inherited_model_fallback: bool,
}

/// Terminal status of a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultStatus {
    Complete,
    Partial,
    Failed,
    Stalled,
    Timeout,
    Cancelled,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Stalled => "stalled",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

/// `SubagentResult`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubagentResult {
    pub task_id: String,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Advisory: the task went over its token budget.
    pub budget_exceeded: Option<bool>,
    pub status: Option<ResultStatus>,
    /// The parsed `result.json`, when there is one.
    pub result_data: Option<Map<String, Value>>,
    /// This run used the inherited-model fallback.
    pub used_inherited_model_fallback: Option<bool>,
}

/// `TaskResult` of [`SubagentPool::dispatch`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskResult {
    /// The evaluator kept the task inline (depth guard).
    pub handled_inline: bool,
    pub task_id: Option<String>,
    pub agent_type: Option<String>,
    pub reason: Option<String>,
    pub result: Option<SubagentResult>,
    /// Milliseconds, when delegated.
    pub duration: Option<u64>,
}

/// `DispatchOptions`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DispatchOptions {
    /// Skip evaluation and use this agent type.
    pub force_agent: Option<String>,
    /// Context distilled from the calling agent.
    pub context: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Session file to persist/continue (resume).
    pub session_file: Option<PathBuf>,
    /// Caller-supplied task id (default: a generated `dispatch-…` id).
    pub task_id: Option<String>,
}

/// `SubagentPoolOptions`.
#[derive(Debug, Clone, Default)]
pub struct SubagentPoolOptions {
    /// The cortex executable (or a runtime when `prefix_args` names a script).
    pub executable: PathBuf,
    pub prefix_args: Vec<String>,
    /// Default 5.
    pub max_concurrency: Option<usize>,
    /// Default: the process cwd.
    pub cwd: Option<PathBuf>,
    /// Default: the process environment.
    pub env: Option<HashMap<String, String>>,
    pub default_token_budget: Option<u64>,
    /// Non-default skill paths forwarded to every child via `--skill`.
    pub skill_paths: Vec<String>,
    pub settings: Option<CategorySettings>,
    /// Available models for deriving model-category defaults (snapshot).
    pub available_models: Vec<Model>,
}

/// `get_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Queued,
    Done,
    Failed,
    Stalled,
    Timeout,
    Cancelled,
    Unknown,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stalled => "stalled",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// A pool event: hoocode's event name and payload.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolEvent {
    pub name: &'static str,
    pub data: Value,
}

/// Pool errors (hoocode's thrown/rejected messages).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolError(pub String);

impl std::fmt::Display for PoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PoolError {}

type Listener = Arc<dyn Fn(&PoolEvent) + Send + Sync>;
type Waiter = oneshot::Sender<Result<SubagentResult, PoolError>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KillReason {
    Stalled,
    Timeout,
    Cancelled,
}

impl KillReason {
    fn status(self) -> ResultStatus {
        match self {
            Self::Stalled => ResultStatus::Stalled,
            Self::Timeout => ResultStatus::Timeout,
            Self::Cancelled => ResultStatus::Cancelled,
        }
    }
}

/// `SubagentSlot`: a running child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSlot {
    pub pid: u32,
    pub agent_type: String,
    pub task_id: String,
    /// Epoch ms.
    pub spawned_at: u64,
    pub token_budget: u64,
}

#[derive(Default)]
struct PoolState {
    slots: HashMap<String, SubagentSlot>,
    queue: VecDeque<SubagentPoolTask>,
    completed: HashMap<String, SubagentResult>,
    waiters: HashMap<String, Waiter>,
    budgets: HashMap<String, Arc<Mutex<TokenBudget>>>,
    kill_reasons: HashMap<String, KillReason>,
    /// Terminal status, kept after `wait_for` consumes the result.
    task_status: HashMap<String, TaskStatus>,
    skill_paths: Vec<String>,
    registry: Option<Arc<AgentRegistry>>,
    disposed: bool,
}

struct PoolInner {
    id: u64,
    max_concurrency: usize,
    executable: PathBuf,
    prefix_args: Vec<String>,
    cwd: PathBuf,
    env: HashMap<String, String>,
    default_token_budget: u64,
    settings: Option<CategorySettings>,
    available_models: Vec<Model>,
    verifier: OutputVerifier,
    lifeguard: Arc<SubagentLifeguard>,
    state: Mutex<PoolState>,
    listeners: Mutex<Vec<Listener>>,
}

/// `SubagentPool`. Cheap to clone; must be created inside a tokio runtime.
#[derive(Clone)]
pub struct SubagentPool {
    inner: Arc<PoolInner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append to a capped capture buffer, keeping the most recent tail.
fn append_tail(current: &mut String, chunk: &str) {
    current.push_str(chunk);
    if current.len() > MAX_CAPTURED_STREAM_CHARS {
        let mut cut = current.len() - MAX_CAPTURED_STREAM_CHARS;
        while !current.is_char_boundary(cut) {
            cut += 1;
        }
        current.drain(..cut);
    }
}

/// Higher runs first: read-only investigation often unblocks other work.
fn priority_of(agent_type: &str) -> u8 {
    if agent_type == "explore" || agent_type == "plan" {
        2
    } else {
        1
    }
}

/// `Math.random().toString(36).slice(2, 8)`: six base-36 characters.
fn random_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (0..6)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            DIGITS[(seed % 36) as usize] as char
        })
        .collect()
}

impl SubagentPool {
    pub fn new(options: SubagentPoolOptions) -> Self {
        let cwd = options
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let lifeguard = SubagentLifeguard::new(&cwd);
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let inner = Arc::new(PoolInner {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            max_concurrency: options.max_concurrency.unwrap_or(5),
            executable: options.executable,
            prefix_args: options.prefix_args,
            env: options.env.unwrap_or_else(|| std::env::vars().collect()),
            default_token_budget: options.default_token_budget.unwrap_or(0),
            settings: options.settings,
            available_models: options.available_models,
            verifier: OutputVerifier::new(cwd.clone()),
            lifeguard: lifeguard.clone(),
            cwd,
            state: Mutex::new(PoolState {
                skill_paths: options.skill_paths,
                ..Default::default()
            }),
            listeners: Mutex::new(Vec::new()),
        });
        let weak: Weak<PoolInner> = Arc::downgrade(&inner);
        lifeguard.on_event(move |event| {
            let Some(inner) = weak.upgrade() else { return };
            let (task_id, pid, reason, name) = match event {
                LifeguardEvent::Stalled { task_id, pid } => {
                    (task_id, pid, KillReason::Stalled, "task_stalled")
                }
                LifeguardEvent::Timeout { task_id, pid } => {
                    (task_id, pid, KillReason::Timeout, "task_timeout")
                }
            };
            inner.state().kill_reasons.insert(task_id.clone(), reason);
            inner.emit(name, json!({"task_id": task_id, "pid": pid}));
        });
        Self { inner }
    }

    /// A process-unique id for this pool.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    /// Listen for pool events.
    pub fn on(&self, listener: impl Fn(&PoolEvent) + Send + Sync + 'static) {
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(listener));
    }

    /// Test hook: deliver a synthetic event to the listeners.
    #[doc(hidden)]
    pub fn emit_for_testing(&self, name: &'static str, data: Value) {
        self.inner.emit(name, data);
    }

    /// The lifeguard (tests inject stall verdicts through it).
    pub fn lifeguard(&self) -> &Arc<SubagentLifeguard> {
        &self.inner.lifeguard
    }

    /// Update the skill paths forwarded to new subagents.
    pub fn update_skill_paths(&self, paths: Vec<String>) {
        self.inner.state().skill_paths = paths;
    }

    /// External in-process load (background MCP tools) for the lifeguard.
    pub fn set_external_load(&self, count: i64) {
        self.inner.lifeguard.set_external_load(count);
    }

    /// Use this registry instead of loading one for the pool's cwd.
    pub fn set_registry(&self, registry: AgentRegistry) {
        self.inner.state().registry = Some(Arc::new(registry));
    }

    /// Queue a task; it runs when a slot is free.
    pub fn spawn(&self, task: SubagentPoolTask) -> Result<(), PoolError> {
        {
            let mut state = self.inner.state();
            if state.disposed {
                return Err(PoolError("SubagentPool has been disposed".into()));
            }
            if state.slots.contains_key(&task.task_id)
                || state.queue.iter().any(|t| t.task_id == task.task_id)
                || state.completed.contains_key(&task.task_id)
            {
                return Err(PoolError(format!("Duplicate task_id: {}", task.task_id)));
            }
            let p = priority_of(&task.agent_type);
            match state
                .queue
                .iter()
                .position(|t| priority_of(&t.agent_type) < p)
            {
                Some(idx) => state.queue.insert(idx, task),
                None => state.queue.push_back(task),
            }
        }
        self.inner.pull();
        Ok(())
    }

    /// Current status of a task.
    pub fn get_status(&self, task_id: &str) -> TaskStatus {
        let state = self.inner.state();
        if state.slots.contains_key(task_id) {
            return TaskStatus::Running;
        }
        if state.queue.iter().any(|t| t.task_id == task_id) {
            return TaskStatus::Queued;
        }
        if let Some(status) = state.task_status.get(task_id) {
            return *status;
        }
        match state.completed.get(task_id) {
            Some(result) => match result.status {
                Some(ResultStatus::Stalled) => TaskStatus::Stalled,
                Some(ResultStatus::Timeout) => TaskStatus::Timeout,
                Some(ResultStatus::Cancelled) => TaskStatus::Cancelled,
                _ if result.ok => TaskStatus::Done,
                _ => TaskStatus::Failed,
            },
            None => TaskStatus::Unknown,
        }
    }

    /// Cancel on the user's behalf. A queued task settles at once; a running
    /// task's process tree is killed and it settles `cancelled` on exit
    /// (unless it already wrote a valid result.json). False for unknown ids.
    pub fn cancel(&self, task_id: &str) -> bool {
        let mut state = self.inner.state();
        if let Some(idx) = state.queue.iter().position(|t| t.task_id == task_id) {
            state.queue.remove(idx);
            drop(state);
            let result = SubagentResult {
                task_id: task_id.to_string(),
                error: Some("cancelled before start".into()),
                status: Some(ResultStatus::Cancelled),
                ..Default::default()
            };
            self.inner
                .emit("task_cancelled", json!({"task_id": task_id}));
            self.inner.resolve_waiter(task_id, result);
            return true;
        }
        let Some(pid) = state.slots.get(task_id).map(|s| s.pid) else {
            return false;
        };
        state
            .kill_reasons
            .insert(task_id.to_string(), KillReason::Cancelled);
        drop(state);
        if pid > 0 {
            kill_process_tree(pid);
        }
        true
    }

    /// Wait for a task and take its result.
    pub async fn wait_for(&self, task_id: &str) -> Result<SubagentResult, PoolError> {
        let rx = {
            let mut state = self.inner.state();
            if state.disposed {
                return Err(PoolError("SubagentPool has been disposed".into()));
            }
            if let Some(result) = state.completed.remove(task_id) {
                return Ok(result);
            }
            let (tx, rx) = oneshot::channel();
            state.waiters.insert(task_id.to_string(), tx);
            rx
        };
        rx.await
            .unwrap_or_else(|_| Err(PoolError("SubagentPool disposed".into())))
    }

    /// The running child for a task.
    pub fn slot(&self, task_id: &str) -> Option<SubagentSlot> {
        self.inner.state().slots.get(task_id).cloned()
    }

    pub fn running_count(&self) -> usize {
        self.inner.state().slots.len()
    }

    pub fn queued_count(&self) -> usize {
        self.inner.state().queue.len()
    }

    /// Dispatch through the evaluator and wait: inline when the depth guard
    /// keeps it (and no agent is forced), else spawn and wait.
    pub async fn dispatch(
        &self,
        task: &str,
        options: DispatchOptions,
    ) -> Result<TaskResult, PoolError> {
        if self.inner.state().disposed {
            return Err(PoolError("SubagentPool has been disposed".into()));
        }
        let begin = self.begin_dispatch(task, options)?;
        let Some((task_id, agent_type, start)) = begin.spawned else {
            return Ok(TaskResult {
                handled_inline: true,
                reason: begin.reason,
                ..Default::default()
            });
        };
        let result = self.wait_for(&task_id).await?;
        Ok(TaskResult {
            handled_inline: false,
            task_id: Some(task_id),
            agent_type: Some(agent_type),
            reason: begin.reason,
            result: Some(result),
            duration: Some(now_ms().saturating_sub(start)),
        })
    }

    /// Fire-and-forget dispatch for background agents; poll with
    /// [`get_status`](Self::get_status) / [`collect`](Self::collect).
    pub fn dispatch_detached(
        &self,
        task: &str,
        options: DispatchOptions,
    ) -> Result<TaskResult, PoolError> {
        if self.inner.state().disposed {
            return Err(PoolError("SubagentPool has been disposed".into()));
        }
        let begin = self.begin_dispatch(task, options)?;
        Ok(match begin.spawned {
            None => TaskResult {
                handled_inline: true,
                reason: begin.reason,
                ..Default::default()
            },
            Some((task_id, agent_type, _)) => TaskResult {
                handled_inline: false,
                task_id: Some(task_id),
                agent_type: Some(agent_type),
                reason: begin.reason,
                ..Default::default()
            },
        })
    }

    fn begin_dispatch(&self, task: &str, options: DispatchOptions) -> Result<Begin, PoolError> {
        let analysis = DispatchEvaluator.evaluate_with_env(task, &self.inner.env);
        if options.force_agent.is_none() && !analysis.should_delegate {
            return Ok(Begin {
                reason: Some(analysis.reason),
                spawned: None,
            });
        }
        let agent_type = options
            .force_agent
            .clone()
            .unwrap_or_else(|| "general-purpose".into());
        let task_id = options
            .task_id
            .clone()
            .unwrap_or_else(|| format!("dispatch-{}-{}", now_ms(), random_suffix()));
        let reason = if options.force_agent.is_some() {
            "user_override".to_string()
        } else {
            analysis.reason
        };
        let complexity = analysis.estimated_complexity.as_str();
        let child_depth = current_subagent_depth(&self.inner.env) + 1;
        agent_log(&format!(
            "[DISPATCH] agent={agent_type} depth={child_depth} reason={reason} complexity={complexity} task_id={task_id}"
        ));
        self.inner.write_dispatch_log(
            &task_id,
            &agent_type,
            &reason,
            complexity,
            task,
            child_depth,
        );
        let start = now_ms();
        self.spawn(SubagentPoolTask {
            task_id: task_id.clone(),
            agent_type: agent_type.clone(),
            task: task.to_string(),
            context: options.context,
            model: options.model,
            provider: options.provider,
            session_file: options.session_file,
            cwd: Some(self.inner.cwd.clone()),
            ..Default::default()
        })?;
        Ok(Begin {
            reason: Some(reason),
            spawned: Some((task_id, agent_type, start)),
        })
    }

    /// Read a completed task's result without consuming it.
    pub fn collect(&self, task_id: &str) -> Option<SubagentResult> {
        self.inner.state().completed.get(task_id).cloned()
    }

    /// The persisted session file for a task.
    pub fn get_session_file(&self, task_id: &str, cwd: Option<&Path>) -> PathBuf {
        self.inner.session_file(task_id, cwd)
    }

    /// Continue a previously dispatched subagent's persisted session with a
    /// follow-up prompt, as the agent type it was dispatched with.
    pub async fn resume(
        &self,
        task_id: &str,
        prompt: &str,
        options: DispatchOptions,
    ) -> Result<TaskResult, PoolError> {
        if self.inner.state().disposed {
            return Err(PoolError("SubagentPool has been disposed".into()));
        }
        let session_file = self.get_session_file(task_id, None);
        if !session_file.exists() {
            return Err(PoolError(format!(
                "No resumable session for task \"{task_id}\" (expected {}).",
                session_file.display()
            )));
        }
        let agent_type = self
            .inner
            .read_dispatch_agent_type(task_id)
            .unwrap_or_else(|| "general-purpose".into());
        self.dispatch(
            prompt,
            DispatchOptions {
                force_agent: Some(agent_type),
                session_file: Some(session_file),
                ..options
            },
        )
        .await
    }

    /// Kill all running processes, clear the queue, fail pending waiters.
    pub fn dispose(&self) {
        let (pids, waiters) = {
            let mut state = self.inner.state();
            if state.disposed {
                return;
            }
            state.disposed = true;
            let pids: Vec<u32> = state.slots.values().map(|s| s.pid).collect();
            state.slots.clear();
            state.queue.clear();
            let waiters: Vec<Waiter> = state.waiters.drain().map(|(_, w)| w).collect();
            state.completed.clear();
            state.budgets.clear();
            state.kill_reasons.clear();
            state.task_status.clear();
            (pids, waiters)
        };
        // The whole process group: a subagent's own children must not outlive
        // the pool.
        for pid in pids.into_iter().filter(|p| *p > 0) {
            kill_process_tree(pid);
        }
        for waiter in waiters {
            let _ = waiter.send(Err(PoolError("SubagentPool disposed".into())));
        }
        self.inner.lifeguard.dispose();
        self.inner
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// `shouldRetryWithInheritedModel`: a failed built-in or project agent
    /// that pins its own model retries once with the caller's model.
    pub fn should_retry_with_inherited_model(
        &self,
        task: &SubagentPoolTask,
        result: &SubagentResult,
    ) -> bool {
        self.inner.should_retry_with_inherited_model(task, result)
    }

    /// `isInheritedModelFallbackError`.
    pub fn is_inherited_model_fallback_error(result: &SubagentResult) -> bool {
        is_inherited_model_fallback_error(result)
    }
}

struct Begin {
    reason: Option<String>,
    spawned: Option<(String, String, u64)>,
}

fn is_inherited_model_fallback_error(result: &SubagentResult) -> bool {
    let data = Value::Object(result.result_data.clone().unwrap_or_default()).to_string();
    let text = [result.error.as_deref().unwrap_or(""), &result.stderr, &data]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    INHERITED_MODEL_FALLBACK_ERROR.is_match(&text)
}

impl PoolInner {
    fn state(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(&self, name: &'static str, data: Value) {
        let listeners = self
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let event = PoolEvent { name, data };
        for listener in listeners {
            listener(&event);
        }
    }

    fn registry(&self) -> Arc<AgentRegistry> {
        if let Some(registry) = self.state().registry.clone() {
            return registry;
        }
        let registry = Arc::new(load_agent_registry(&LoadAgentRegistryOptions::new(
            self.cwd.to_string_lossy(),
        )));
        self.state().registry = Some(registry.clone());
        registry
    }

    fn definition(&self, agent_type: &str) -> Option<AgentDefinition> {
        if agent_type.is_empty() {
            return None;
        }
        self.registry().get(agent_type).cloned()
    }

    fn session_file(&self, task_id: &str, cwd: Option<&Path>) -> PathBuf {
        cortexcode_code_paths::dispatch_task_dir(cwd.unwrap_or(&self.cwd), task_id)
            .join("session.jsonl")
    }

    fn read_dispatch_agent_type(&self, task_id: &str) -> Option<String> {
        let path =
            cortexcode_code_paths::dispatch_task_dir(&self.cwd, task_id).join("dispatch-log.json");
        let parsed: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        parsed
            .get("agent_type")
            .and_then(Value::as_str)
            .map(String::from)
    }

    fn write_dispatch_log(
        &self,
        task_id: &str,
        agent_type: &str,
        reason: &str,
        complexity: &str,
        task: &str,
        depth: u32,
    ) {
        let log = json!({
            "timestamp": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            "task_id": task_id,
            "agent_type": agent_type,
            "depth": depth,
            "reason": reason,
            "complexity": complexity,
            "task": task,
        });
        let path =
            cortexcode_code_paths::dispatch_task_dir(&self.cwd, task_id).join("dispatch-log.json");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&log).unwrap_or_default());
    }

    fn write_output_json(&self, task_id: &str, result: &SubagentResult) {
        // JSON.stringify drops undefined fields.
        let mut output = Map::new();
        output.insert("task_id".into(), Value::from(result.task_id.clone()));
        output.insert("ok".into(), Value::from(result.ok));
        output.insert("exit_code".into(), json!(result.exit_code));
        if let Some(status) = result.status {
            output.insert("status".into(), Value::from(status.as_str()));
        }
        output.insert("stdout".into(), Value::from(result.stdout.clone()));
        output.insert("stderr".into(), Value::from(result.stderr.clone()));
        if let Some(error) = &result.error {
            output.insert("error".into(), Value::from(error.clone()));
        }
        if let Some(exceeded) = result.budget_exceeded {
            output.insert("budget_exceeded".into(), Value::from(exceeded));
        }
        if let Some(data) = &result.result_data {
            output.insert("result_data".into(), Value::Object(data.clone()));
        }
        let path = cortexcode_code_paths::dispatch_task_dir(&self.cwd, task_id).join("output.json");
        let text = serde_json::to_string_pretty(&Value::Object(output)).unwrap_or_default();
        let _ = write_file_atomic(&path, &text);
    }

    fn read_result_json(&self, task_id: &str, cwd: &Path) -> Option<Map<String, Value>> {
        let path = cortexcode_code_paths::dispatch_task_dir(cwd, task_id).join("result.json");
        match serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()? {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Start queued tasks while slots are free.
    fn pull(self: &Arc<Self>) {
        loop {
            let task = {
                let mut state = self.state();
                if state.disposed || state.slots.len() >= self.max_concurrency {
                    return;
                }
                match state.queue.pop_front() {
                    Some(task) => task,
                    None => return,
                }
            };
            self.start_task(task, false);
        }
    }

    /// The child's command line.
    fn build_args(&self, task: &SubagentPoolTask) -> Vec<String> {
        let cwd = task.cwd.as_deref().unwrap_or(&self.cwd);
        let session_file = task
            .session_file
            .clone()
            .unwrap_or_else(|| self.session_file(&task.task_id, Some(cwd)));
        let mut args = self.prefix_args.clone();
        args.extend([
            "--mode".into(),
            "json".into(),
            "--session".into(),
            session_file.to_string_lossy().into_owned(),
            "--task-id".into(),
            task.task_id.clone(),
        ]);

        let def = self.definition(&task.agent_type);
        if !task.agent_type.is_empty() {
            if let Some(prompt) = def.as_ref().map(|d| &d.prompt).filter(|p| !p.is_empty()) {
                args.extend(["--system-prompt".into(), prompt.clone()]);
            }
            // A `delegate: true` agent may dispatch itself, but only while the
            // child it becomes can still nest.
            let child_depth = current_subagent_depth(&self.env) + 1;
            let can_child_delegate = def.as_ref().and_then(|d| d.delegate) == Some(true)
                && child_depth < resolve_max_subagent_depth(None, &self.env);
            let mut tools = def.as_ref().and_then(|d| d.tools.clone());
            if can_child_delegate {
                if let Some(tools) = &mut tools {
                    for t in ["Task", "TaskOutput"] {
                        if !tools.iter().any(|x| x == t) {
                            tools.push(t.into());
                        }
                    }
                }
            }
            if let Some(tools) = tools.filter(|t| !t.is_empty()) {
                args.extend(["--tools".into(), tools.join(",")]);
            }
            if let Some(disallowed) = def
                .as_ref()
                .and_then(|d| d.disallowed_tools.as_ref())
                .filter(|t| !t.is_empty())
            {
                args.extend(["--disallowed-tools".into(), disallowed.join(",")]);
            }
            if can_child_delegate {
                args.push("--enable-subagents".into());
                if let Some(to) = def
                    .as_ref()
                    .and_then(|d| d.delegate_to.as_ref())
                    .filter(|t| !t.is_empty())
                {
                    args.extend(["--delegate-allow".into(), to.join(",")]);
                }
            }
        }

        // A definition's explicit model wins (unless `inherit`), else the
        // caller's; a category resolves to a concrete model or to nothing.
        let explicit = def
            .as_ref()
            .and_then(|d| d.model.clone())
            .filter(|m| !task.use_inherited_model_fallback && !m.is_empty() && m != MODEL_INHERIT);
        let raw = explicit.or_else(|| task.model.clone().filter(|m| !m.is_empty()));
        let model = raw.and_then(|m| {
            resolve_model_reference(
                &m,
                self.settings.as_ref(),
                Some(self.available_models.as_slice()),
            )
        });
        if let Some(model) = &model {
            args.extend(["--model".into(), model.clone()]);
        }
        // --provider would filter out a model id carrying another provider's
        // prefix, so it only goes along with a bare model id.
        if let Some(provider) = task.provider.as_ref().filter(|p| !p.is_empty()) {
            if model.as_ref().is_none_or(|m| !m.contains('/')) {
                args.extend(["--provider".into(), provider.clone()]);
            }
        }

        let max_turns = def
            .as_ref()
            .and_then(|d| d.max_turns)
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS);
        args.extend(["--max-turns".into(), max_turns.to_string()]);

        for path in self.state().skill_paths.clone() {
            args.extend(["--skill".into(), path]);
        }

        let context = task.context.as_deref().map(str::trim).unwrap_or("");
        args.push(if context.is_empty() {
            format!("Task: {}", task.task.trim())
        } else {
            format!(
                "Context from the calling agent:\n\n{context}\n\nTask: {}",
                task.task.trim()
            )
        });
        args
    }

    /// The child's environment: depth + 1, MCP skipped for an MCP-free
    /// allowlist, deferred MCP schemas off.
    fn child_env(&self, task: &SubagentPoolTask) -> HashMap<String, String> {
        let mut env = self.env.clone();
        let depth = current_subagent_depth(&self.env) + 1;
        for prefix in cortexcode_code_paths::ENV_PREFIXES {
            env.remove(&format!("{prefix}{SUBAGENT_DEPTH_ENV}"));
        }
        env.insert(
            format!("{CHILD_ENV_PREFIX}{SUBAGENT_DEPTH_ENV}"),
            depth.to_string(),
        );
        let def = self.definition(&task.agent_type);
        if !tool_allowlist_needs_mcp(def.as_ref().and_then(|d| d.tools.as_deref())) {
            env.insert(
                format!("{CHILD_ENV_PREFIX}{SUBAGENT_SKIP_MCP_ENV}"),
                "1".into(),
            );
        }
        for prefix in cortexcode_code_paths::ENV_PREFIXES {
            env.remove(&format!("{prefix}{DEFER_MCP_SCHEMAS_ENV}"));
        }
        env
    }

    fn budget_for(self: &Arc<Self>, task: &SubagentPoolTask) -> Arc<Mutex<TokenBudget>> {
        if let Some(budget) = self.state().budgets.get(&task.task_id) {
            return budget.clone();
        }
        let mut budget = TokenBudget::new(
            &task.task_id,
            &task.agent_type,
            TokenBudgetOptions {
                limit: task.token_budget,
                cwd: Some(task.cwd.clone().unwrap_or_else(|| self.cwd.clone())),
            },
        );
        let weak = Arc::downgrade(self);
        budget.on_warning(Box::new(move |data| {
            if let Some(inner) = weak.upgrade() {
                inner.emit("budget_warning", data.clone());
            }
        }));
        // Advisory: telemetry only; --max-turns is the hard stop.
        let weak = Arc::downgrade(self);
        budget.on_exceeded(Box::new(move |data| {
            if let Some(inner) = weak.upgrade() {
                inner.emit("budget_exceeded", data.clone());
            }
        }));
        let budget = Arc::new(Mutex::new(budget));
        self.state()
            .budgets
            .insert(task.task_id.clone(), budget.clone());
        budget
    }

    /// Start a task in a child process (one retry on a spawn failure).
    fn start_task(self: &Arc<Self>, task: SubagentPoolTask, is_retry: bool) {
        let budget = self.budget_for(&task);
        let cwd = task.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(self.build_args(&task))
            .current_dir(&cwd)
            .env_clear()
            .envs(self.child_env(&task))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // POSIX: the child leads its own process group so a kill reaches the
        // whole tree (its bash commands, nested subagents).
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) if !is_retry => return self.start_task(task, true),
            Err(_) => {
                self.emit(
                    "task_failed",
                    json!({"task_id": task.task_id, "error": "Spawn failed synchronously"}),
                );
                self.resolve_waiter(
                    &task.task_id,
                    SubagentResult {
                        task_id: task.task_id.clone(),
                        error: Some("Spawn failed synchronously".into()),
                        status: Some(ResultStatus::Failed),
                        ..Default::default()
                    },
                );
                self.state().budgets.remove(&task.task_id);
                self.pull();
                return;
            }
        };
        let pid = child.id().unwrap_or(0);
        let spawned_at = now_ms();
        self.state().slots.insert(
            task.task_id.clone(),
            SubagentSlot {
                pid,
                agent_type: task.agent_type.clone(),
                task_id: task.task_id.clone(),
                spawned_at,
                token_budget: task.token_budget.unwrap_or(self.default_token_budget),
            },
        );
        self.lifeguard.monitor(&task.task_id, &task.agent_type, pid);

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let captured = Arc::new(Mutex::new((String::new(), String::new())));

        let out_task = {
            let inner = self.clone();
            let captured = captured.clone();
            let budget = budget.clone();
            let task_id = task.task_id.clone();
            let agent_type = task.agent_type.clone();
            tokio::spawn(async move {
                let Some(mut stdout) = stdout else { return };
                let mut reader = JsonlLineReader::with_max_buffer(MAX_SUBAGENT_EVENT_LINE_CHARS);
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    // Any output is a heartbeat: a busy child is alive even
                    // before its ping line is parsed.
                    append_tail(
                        &mut captured.lock().unwrap_or_else(|e| e.into_inner()).0,
                        &String::from_utf8_lossy(&buf[..n]),
                    );
                    inner.lifeguard.record_heartbeat(&task_id);
                    for line in reader.push(&buf[..n]) {
                        inner.handle_stdout_line(&task_id, &agent_type, &budget, &line);
                    }
                }
                if let Some(line) = reader.finish() {
                    inner.handle_stdout_line(&task_id, &agent_type, &budget, &line);
                }
            })
        };
        let err_task = {
            let captured = captured.clone();
            tokio::spawn(async move {
                let Some(mut stderr) = stderr else { return };
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    let n = match stderr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    append_tail(
                        &mut captured.lock().unwrap_or_else(|e| e.into_inner()).1,
                        &String::from_utf8_lossy(&buf[..n]),
                    );
                }
            })
        };

        let inner = self.clone();
        tokio::spawn(async move {
            let status = child.wait().await;
            // Stdio may stay open in grandchildren: wait briefly for it.
            let _ = tokio::time::timeout(EXIT_STDIO_GRACE, async {
                let _ = out_task.await;
                let _ = err_task.await;
            })
            .await;
            inner.lifeguard.untrack(&task.task_id);
            let (stdout, stderr) = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let retry_scheduled = match status {
                Ok(status) => {
                    inner.settle(&task, &budget, spawned_at, status.code(), stdout, stderr)
                }
                Err(err) => {
                    inner.settle_error(&task, &budget, spawned_at, is_retry, err, stdout, stderr)
                }
            };
            if !retry_scheduled {
                inner.state().budgets.remove(&task.task_id);
            }
            inner.pull();
        });
    }

    fn handle_stdout_line(
        &self,
        task_id: &str,
        agent_type: &str,
        budget: &Arc<Mutex<TokenBudget>>,
        line: &str,
    ) {
        budget
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .process_line(line);
        match classify_subagent_line(line) {
            SubagentStdoutLine::Heartbeat => self.lifeguard.record_heartbeat(task_id),
            SubagentStdoutLine::Progress(event) => self.emit(
                "task_progress",
                json!({"task_id": task_id, "agent_type": agent_type, "event": event}),
            ),
            SubagentStdoutLine::Ignore => {}
        }
    }

    /// The child exited: settle the task. Returns whether a retry was queued.
    fn settle(
        self: &Arc<Self>,
        task: &SubagentPoolTask,
        budget: &Arc<Mutex<TokenBudget>>,
        spawned_at: u64,
        code: Option<i32>,
        stdout: String,
        stderr: String,
    ) -> bool {
        let task_id = task.task_id.as_str();
        let cwd = task.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let (tokens_used, budget_exceeded) = {
            let mut budget = budget.lock().unwrap_or_else(|e| e.into_inner());
            budget.flush();
            (budget.used(), budget.is_exceeded())
        };
        let kill_reason = {
            let mut state = self.state();
            state.slots.remove(task_id);
            state.kill_reasons.remove(task_id)
        };
        let duration = now_ms().saturating_sub(spawned_at);

        // Success is a verified result.json, not the exit code: a child that
        // finished its work can still be reaped before it exits on its own.
        let verification = self.verifier.verify(task_id, Some(&cwd));
        let mut cleanly_completed = code == Some(0) || verification.valid;
        if cleanly_completed && verification.valid {
            let failed = self
                .read_result_json(task_id, &cwd)
                .is_some_and(|rd| rd.get("status").and_then(Value::as_str) == Some("failed"));
            if failed {
                cleanly_completed = false;
            }
        }

        // Killed (reaped or cancelled) before producing a valid result.
        if let Some(reason) = kill_reason.filter(|_| !verification.valid) {
            let result = SubagentResult {
                task_id: task_id.to_string(),
                stdout,
                stderr,
                exit_code: code,
                status: Some(reason.status()),
                ..Default::default()
            };
            self.write_output_json(task_id, &result);
            let name = match reason {
                KillReason::Stalled => "task_stalled",
                KillReason::Timeout => "task_timeout",
                KillReason::Cancelled => "task_cancelled",
            };
            self.emit(
                name,
                json!({"task_id": task_id, "agent_type": task.agent_type, "duration": duration, "tokens_used": tokens_used}),
            );
            self.resolve_waiter(task_id, result);
            return false;
        }

        let mut result = SubagentResult {
            task_id: task_id.to_string(),
            ok: cleanly_completed,
            stdout,
            stderr,
            exit_code: code,
            budget_exceeded: Some(budget_exceeded),
            status: Some(if cleanly_completed {
                ResultStatus::Complete
            } else {
                ResultStatus::Failed
            }),
            used_inherited_model_fallback: Some(task.use_inherited_model_fallback),
            ..Default::default()
        };

        if result.ok {
            if !verification.valid {
                result.ok = false;
                result.error = verification.reason.clone();
                result.status = Some(ResultStatus::Failed);
                self.write_output_json(task_id, &result);
                self.emit(
                    "task_failed",
                    json!({"task_id": task_id, "agent_type": task.agent_type, "duration": duration, "tokens_used": tokens_used, "error": verification.reason}),
                );
                self.resolve_waiter(task_id, result);
                return false;
            }
            result.result_data = self.read_result_json(task_id, &cwd);
            // Clean success: the in-memory result carries result_data, so the
            // dispatch dir goes (resume only works for unsuccessful tasks).
            let _ =
                std::fs::remove_dir_all(cortexcode_code_paths::dispatch_task_dir(&cwd, task_id));
            self.emit(
                "task_done",
                json!({"task_id": task_id, "agent_type": task.agent_type, "duration": duration, "tokens_used": tokens_used, "status": "complete"}),
            );
            self.resolve_waiter(task_id, result);
            return false;
        }

        // Failure: keep the dispatch dir; surface the concrete cause.
        result.result_data = self.read_result_json(task_id, &cwd);
        if result.error.is_none() {
            result.error = Some(derive_failure_reason(&result));
        }
        if self.should_retry_with_inherited_model(task, &result) {
            agent_log(&format!(
                "[DISPATCH] agent={} task_id={task_id} preferred model failed; retrying with inherited model",
                task.agent_type
            ));
            self.cleanup_retry_artifacts(task);
            self.state().queue.push_front(SubagentPoolTask {
                use_inherited_model_fallback: true,
                ..task.clone()
            });
            return true;
        }
        self.write_output_json(task_id, &result);
        let error = result
            .error
            .clone()
            .unwrap_or_else(|| format!("Exited with code {}", js_code(code)));
        self.emit(
            "task_failed",
            json!({"task_id": task_id, "agent_type": task.agent_type, "duration": duration, "tokens_used": tokens_used, "error": error}),
        );
        self.resolve_waiter(task_id, result);
        false
    }

    /// Waiting on the child failed: retry once, else fail.
    #[allow(clippy::too_many_arguments)]
    fn settle_error(
        self: &Arc<Self>,
        task: &SubagentPoolTask,
        budget: &Arc<Mutex<TokenBudget>>,
        spawned_at: u64,
        is_retry: bool,
        err: std::io::Error,
        stdout: String,
        stderr: String,
    ) -> bool {
        self.state().slots.remove(&task.task_id);
        let tokens_used = {
            let mut budget = budget.lock().unwrap_or_else(|e| e.into_inner());
            budget.flush();
            budget.used()
        };
        if !is_retry {
            self.start_task(task.clone(), true);
            return true;
        }
        let error = err.to_string();
        let result = SubagentResult {
            task_id: task.task_id.clone(),
            stdout,
            stderr,
            error: Some(error.clone()),
            status: Some(ResultStatus::Failed),
            used_inherited_model_fallback: Some(task.use_inherited_model_fallback),
            ..Default::default()
        };
        self.write_output_json(&task.task_id, &result);
        self.emit(
            "task_failed",
            json!({"task_id": task.task_id, "agent_type": task.agent_type, "duration": now_ms().saturating_sub(spawned_at), "tokens_used": tokens_used, "error": error}),
        );
        self.resolve_waiter(&task.task_id, result);
        false
    }

    fn should_retry_with_inherited_model(
        &self,
        task: &SubagentPoolTask,
        result: &SubagentResult,
    ) -> bool {
        if task.use_inherited_model_fallback || task.session_file.is_some() {
            return false;
        }
        // Only the parent model is required (the provider may come from a
        // gateway default).
        if task.model.as_deref().is_none_or(str::is_empty) {
            return false;
        }
        let Some(def) = self.definition(&task.agent_type) else {
            return false;
        };
        if !matches!(def.source, AgentSource::Builtin | AgentSource::Project) {
            return false;
        }
        if def
            .model
            .as_deref()
            .is_none_or(|m| m.is_empty() || m == MODEL_INHERIT)
        {
            return false;
        }
        is_inherited_model_fallback_error(result)
    }

    fn cleanup_retry_artifacts(&self, task: &SubagentPoolTask) {
        let cwd = task.cwd.clone().unwrap_or_else(|| self.cwd.clone());
        let dir = cortexcode_code_paths::dispatch_task_dir(&cwd, &task.task_id);
        let session = task
            .session_file
            .clone()
            .unwrap_or_else(|| self.session_file(&task.task_id, Some(&cwd)));
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(dir.join("result.json"));
        let _ = std::fs::remove_file(dir.join("output.json"));
    }

    fn resolve_waiter(&self, task_id: &str, result: SubagentResult) {
        let mut state = self.state();
        let status = match result.status {
            Some(ResultStatus::Stalled) => TaskStatus::Stalled,
            Some(ResultStatus::Timeout) => TaskStatus::Timeout,
            Some(ResultStatus::Cancelled) => TaskStatus::Cancelled,
            _ if result.ok => TaskStatus::Done,
            _ => TaskStatus::Failed,
        };
        state.task_status.insert(task_id.to_string(), status);
        if let Some(waiter) = state.waiters.remove(task_id) {
            drop(state);
            let _ = waiter.send(Ok(result));
            return;
        }
        state.completed.insert(task_id.to_string(), result);
    }
}

/// `String(code)` for an exit code (`null` when killed by a signal).
fn js_code(code: Option<i32>) -> String {
    code.map_or_else(|| "null".into(), |c| c.to_string())
}

/// The child's result.json summary, else the stderr tail, else the exit code.
fn derive_failure_reason(result: &SubagentResult) -> String {
    let summary = result
        .result_data
        .as_ref()
        .and_then(|d| d.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if !summary.is_empty() {
        return summary.to_string();
    }
    let lines: Vec<&str> = result
        .stderr
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    if !tail.is_empty() {
        return tail;
    }
    format!("Exited with code {}", js_code(result.exit_code))
}

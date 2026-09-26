//! `core/task-store.ts`: a minimal in-process task store.
//!
//! Tasks (TodoWrite plan items, subagent delegations, MCP calls) and the
//! agents that own them, with change listeners for the task panel. Mutations
//! inside [`TaskStore::batch`] notify once, when the outermost batch ends.
//! Listeners run outside the store's lock, so they may read the store.

use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// `TaskStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Done,
    Failed,
    /// A user-initiated stop, distinct from `Failed`.
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }
}

/// `TaskSource`: what kind of background work owns a task (unset: the main
/// agent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSource {
    Subagent,
    Mcp,
}

/// `TaskAgentKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAgentKind {
    Main,
    Subagent,
    Role,
}

/// `TaskAgentState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAgentState {
    Active,
    Running,
    Done,
    Queued,
    Idle,
    Waiting,
    Failed,
    Cancelled,
}

/// Token and cost totals for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AgentStats {
    pub input: f64,
    pub output: f64,
    pub cost: f64,
}

/// `TaskAgent`: a group header in the panel's grouped views.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskAgent {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub kind: TaskAgentKind,
    pub state: Option<TaskAgentState>,
    pub handoff: Option<String>,
    /// Live activity of a running subagent; `""` clears it.
    pub activity: Option<String>,
    pub stats: Option<AgentStats>,
}

/// Token and cost usage attributed to a task.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TaskUsage {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cost: f64,
}

/// `Task`.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub status: TaskStatus,
    pub source: Option<TaskSource>,
    /// The row's `[tag]`: subagent type or MCP server name.
    pub subagent_mode: Option<String>,
    /// Id of the owning [`TaskAgent`].
    pub agent: Option<String>,
    /// The task that spawned this one (subagent trees).
    pub parent_task_id: Option<u64>,
    /// The TodoWrite plan item this run works on (display link only).
    pub linked_task_id: Option<u64>,
    /// A short warning cue.
    pub note: Option<String>,
    /// Canonical TodoWrite item content (the title flips with status).
    pub todo_content: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub usage: Option<TaskUsage>,
}

/// `CreateTaskOptions`.
#[derive(Debug, Clone, Default)]
pub struct CreateTaskOptions {
    pub source: Option<TaskSource>,
    pub subagent_mode: Option<String>,
    pub agent: Option<String>,
    pub parent_task_id: Option<u64>,
    pub linked_task_id: Option<u64>,
}

/// `TaskPatch`: `None` leaves a field alone, except `note`, where
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub status: Option<TaskStatus>,
    pub source: Option<TaskSource>,
    pub subagent_mode: Option<String>,
    pub agent: Option<String>,
    pub usage: Option<TaskUsage>,
    pub note: Option<Option<String>>,
    pub parent_task_id: Option<u64>,
    pub linked_task_id: Option<u64>,
    pub todo_content: Option<String>,
}

/// `TaskAgentPatch`.
#[derive(Debug, Clone, Default)]
pub struct TaskAgentPatch {
    pub name: Option<String>,
    pub role: Option<String>,
    pub kind: Option<TaskAgentKind>,
    pub state: Option<TaskAgentState>,
    pub handoff: Option<String>,
    pub activity: Option<String>,
    pub stats: Option<AgentStats>,
}

/// `taskOwnerId`: the explicit agent, else `subagent` for subagent-sourced
/// work, else `main`.
pub fn task_owner_id(agent: Option<&str>, source: Option<TaskSource>) -> String {
    match (agent, source) {
        (Some(agent), _) => agent.to_owned(),
        (None, Some(TaskSource::Subagent)) => "subagent".into(),
        _ => "main".into(),
    }
}

impl Task {
    pub fn owner_id(&self) -> String {
        task_owner_id(self.agent.as_deref(), self.source)
    }
}

type Listener = Arc<dyn Fn() + Send + Sync>;

struct State {
    tasks: Vec<Task>,
    agents: Vec<TaskAgent>,
    next_id: u64,
    batch_depth: usize,
    mutation_count: u64,
    listeners: Vec<(u64, Listener)>,
    next_listener: u64,
}

/// `TaskStore`.
pub struct TaskStore {
    state: Mutex<State>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn apply_agent_patch(agent: &mut TaskAgent, patch: TaskAgentPatch) {
    if let Some(name) = patch.name {
        agent.name = name;
    }
    if patch.role.is_some() {
        agent.role = patch.role;
    }
    if let Some(kind) = patch.kind {
        agent.kind = kind;
    }
    if patch.state.is_some() {
        agent.state = patch.state;
    }
    if patch.activity.is_some() {
        agent.activity = patch.activity;
    }
    if patch.handoff.is_some() {
        agent.handoff = patch.handoff;
    }
    if patch.stats.is_some() {
        agent.stats = patch.stats;
    }
}

/// Unsubscribes its listener when called (`subscribe`'s return value).
pub struct Subscription<'a> {
    store: &'a TaskStore,
    id: u64,
}

impl Subscription<'_> {
    pub fn unsubscribe(self) {
        self.store.lock().listeners.retain(|(id, _)| *id != self.id);
    }
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                tasks: Vec::new(),
                agents: Vec::new(),
                next_id: 1,
                batch_depth: 0,
                mutation_count: 0,
                listeners: Vec::new(),
                next_listener: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bump the version and, outside a batch, notify (with the lock released).
    fn emit(&self, mut state: MutexGuard<'_, State>) {
        state.mutation_count += 1;
        if state.batch_depth > 0 {
            return;
        }
        let listeners: Vec<Listener> = state.listeners.iter().map(|(_, l)| l.clone()).collect();
        drop(state);
        for listener in listeners {
            listener();
        }
    }

    /// `version`: bumped on every mutation, batched or not.
    pub fn version(&self) -> u64 {
        self.lock().mutation_count
    }

    /// `batch`: listeners are notified once, when the outermost batch ends.
    pub fn batch<T>(&self, f: impl FnOnce(&Self) -> T) -> T {
        self.lock().batch_depth += 1;
        struct Guard<'a>(&'a TaskStore);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                let mut state = self.0.lock();
                state.batch_depth -= 1;
                if state.batch_depth == 0 {
                    // As in hoocode, the flush itself counts as a mutation.
                    self.0.emit(state);
                }
            }
        }
        let _guard = Guard(self);
        f(self)
    }

    /// `create`: a pending task (a blank title becomes `(untitled task)`).
    pub fn create(&self, title: &str, options: CreateTaskOptions) -> Task {
        let mut state = self.lock();
        let now = now_ms();
        let title = title.trim();
        let task = Task {
            id: state.next_id,
            title: if title.is_empty() {
                "(untitled task)".into()
            } else {
                title.to_owned()
            },
            status: TaskStatus::Pending,
            source: options.source,
            subagent_mode: options.subagent_mode,
            agent: options.agent,
            parent_task_id: options.parent_task_id,
            linked_task_id: options.linked_task_id,
            note: None,
            todo_content: None,
            created_at: now,
            updated_at: now,
            usage: None,
        };
        state.next_id += 1;
        state.tasks.push(task.clone());
        self.emit(state);
        task
    }

    /// `update`; unknown ids are ignored.
    pub fn update(&self, id: u64, patch: TaskPatch) {
        let mut state = self.lock();
        let Some(task) = state.tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        if let Some(title) = patch.title {
            task.title = title;
        }
        if let Some(status) = patch.status {
            task.status = status;
        }
        if patch.source.is_some() {
            task.source = patch.source;
        }
        if patch.subagent_mode.is_some() {
            task.subagent_mode = patch.subagent_mode;
        }
        if patch.agent.is_some() {
            task.agent = patch.agent;
        }
        if patch.parent_task_id.is_some() {
            task.parent_task_id = patch.parent_task_id;
        }
        if patch.linked_task_id.is_some() {
            task.linked_task_id = patch.linked_task_id;
        }
        if patch.todo_content.is_some() {
            task.todo_content = patch.todo_content;
        }
        if patch.usage.is_some() {
            task.usage = patch.usage;
        }
        if let Some(note) = patch.note {
            task.note = note;
        }
        task.updated_at = now_ms();
        self.emit(state);
    }

    /// `upsertAgent`: merge into an agent with the same id, or create it.
    pub fn upsert_agent(
        &self,
        id: &str,
        name: &str,
        kind: TaskAgentKind,
        patch: TaskAgentPatch,
    ) -> TaskAgent {
        let mut state = self.lock();
        let agent = match state.agents.iter_mut().find(|a| a.id == id) {
            Some(existing) => {
                apply_agent_patch(
                    existing,
                    TaskAgentPatch {
                        name: Some(name.to_owned()),
                        kind: Some(kind),
                        ..patch
                    },
                );
                existing.clone()
            }
            None => {
                let created = TaskAgent {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    role: patch.role,
                    kind,
                    state: patch.state,
                    handoff: patch.handoff,
                    activity: patch.activity,
                    stats: patch.stats,
                };
                state.agents.push(created.clone());
                created
            }
        };
        self.emit(state);
        agent
    }

    /// `patchAgent`; unknown ids are ignored.
    pub fn patch_agent(&self, id: &str, patch: TaskAgentPatch) {
        let mut state = self.lock();
        let Some(agent) = state.agents.iter_mut().find(|a| a.id == id) else {
            return;
        };
        apply_agent_patch(agent, patch);
        self.emit(state);
    }

    /// `addAgentStats`: add a usage delta (totals start at zero).
    pub fn add_agent_stats(&self, id: &str, delta: AgentStats) {
        let mut state = self.lock();
        let Some(agent) = state.agents.iter_mut().find(|a| a.id == id) else {
            return;
        };
        let stats = agent.stats.get_or_insert_with(AgentStats::default);
        stats.input += delta.input;
        stats.output += delta.output;
        stats.cost += delta.cost;
        self.emit(state);
    }

    pub fn agents(&self) -> Vec<TaskAgent> {
        self.lock().agents.clone()
    }

    /// `remove`; unknown ids are ignored.
    pub fn remove(&self, id: u64) {
        let mut state = self.lock();
        let Some(index) = state.tasks.iter().position(|t| t.id == id) else {
            return;
        };
        state.tasks.remove(index);
        self.emit(state);
    }

    /// `arrange`: permute the given tasks into this relative order within
    /// their existing slots; ignored unless every id resolves.
    pub fn arrange(&self, ids: &[u64]) {
        let mut state = self.lock();
        let ordered: Vec<Task> = ids
            .iter()
            .filter_map(|id| state.tasks.iter().find(|t| t.id == *id).cloned())
            .collect();
        if ordered.len() != ids.len() {
            return;
        }
        let slots: Vec<usize> = state
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| ids.contains(&t.id))
            .map(|(i, _)| i)
            .collect();
        if slots.len() != ordered.len() {
            return;
        }
        for (slot, task) in slots.into_iter().zip(ordered) {
            state.tasks[slot] = task;
        }
        self.emit(state);
    }

    /// `reset`: drop finished tasks (a new user message arrived); numbering
    /// restarts once nothing active remains. Agents with stats survive.
    pub fn reset(&self) {
        let mut state = self.lock();
        let active: Vec<Task> = state
            .tasks
            .iter()
            .filter(|t| t.status.is_active())
            .cloned()
            .collect();
        if active.len() == state.tasks.len() && state.next_id == 1 {
            return;
        }
        let has_stats = |a: &TaskAgent| {
            a.stats
                .is_some_and(|s| s.input > 0.0 || s.output > 0.0 || s.cost > 0.0)
        };
        if active.is_empty() {
            state.next_id = 1;
            state
                .agents
                .retain(|a| a.kind != TaskAgentKind::Subagent && has_stats(a));
        } else {
            let live: Vec<String> = active.iter().map(Task::owner_id).collect();
            state.agents.retain(|a| {
                live.contains(&a.id) || (a.kind != TaskAgentKind::Subagent && has_stats(a))
            });
        }
        state.tasks = active;
        self.emit(state);
    }

    /// `list`: every task, in panel order.
    pub fn list(&self) -> Vec<Task> {
        self.lock().tasks.clone()
    }

    /// `clear`: wipe everything and restart numbering (tests).
    pub fn clear(&self) {
        let mut state = self.lock();
        state.tasks.clear();
        state.agents.clear();
        state.next_id = 1;
        self.emit(state);
    }

    /// `subscribe`.
    pub fn subscribe(&self, listener: impl Fn() + Send + Sync + 'static) -> Subscription<'_> {
        let mut state = self.lock();
        let id = state.next_listener;
        state.next_listener += 1;
        state.listeners.push((id, Arc::new(listener)));
        Subscription { store: self, id }
    }
}

static GLOBAL: LazyLock<TaskStore> = LazyLock::new(TaskStore::new);

/// The shared, process-wide store (`taskStore`).
pub fn task_store() -> &'static TaskStore {
    &GLOBAL
}

//! `core/subagent-inbox.ts`: the notify-and-pull bookkeeping behind background
//! `Task` dispatch and the `TaskOutput` tool.
//!
//! A background dispatch gives the parent a compact notification
//! ("explore#1 finished"); the body waits here, keyed by task id, until the
//! model pulls it with TaskOutput. Lifecycle per task:
//!
//! ```text
//! running ──▶ done (body kept) ──collect──▶ collected (body dropped)
//!         ├─▶ failed / stalled / timeout / cancelled
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::Notify;

use crate::pool::{ResultStatus, SubagentPool, TaskResult};

/// `TaskLifecycle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLifecycle {
    Running,
    Done,
    Failed,
    Stalled,
    Timeout,
    Cancelled,
    Collected,
}

impl TaskLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stalled => "stalled",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Collected => "collected",
        }
    }

    /// Still doing work.
    fn is_outstanding(self) -> bool {
        self == Self::Running
    }
}

/// `InboxRecord`.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxRecord {
    pub task_id: String,
    /// Friendly handle shown to the model, e.g. `explore#1`.
    pub label: String,
    pub agent_type: String,
    pub lifecycle: TaskLifecycle,
    /// Epoch ms.
    pub started_at: u64,
    pub ended_at: Option<u64>,
    /// The tool the subagent is running, from the pool's progress stream.
    pub last_activity: Option<String>,
    /// First line of the result, kept after the body is collected.
    pub summary_line: Option<String>,
    /// The full summary, only while `done` and uncollected.
    pub body: Option<String>,
    pub error: Option<String>,
}

/// Settled records kept for late pulls (running records are never pruned).
const MAX_SETTLED: usize = 50;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// First non-empty line, capped at `max` characters (UTF-16 units in JS).
fn first_line(text: &str, max: usize) -> String {
    let line = text
        .trim()
        .split('\n')
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let units: Vec<u16> = line.encode_utf16().collect();
    if units.len() > max {
        format!("{}\u{2026}", String::from_utf16_lossy(&units[..max - 1]))
    } else {
        line.to_string()
    }
}

fn fail_lifecycle(status: Option<ResultStatus>) -> TaskLifecycle {
    match status {
        Some(ResultStatus::Stalled) => TaskLifecycle::Stalled,
        Some(ResultStatus::Timeout) => TaskLifecycle::Timeout,
        Some(ResultStatus::Cancelled) => TaskLifecycle::Cancelled,
        _ => TaskLifecycle::Failed,
    }
}

/// "Currently running X" from a forwarded progress event.
fn activity_from_event(event: &Value) -> Option<String> {
    match event.get("type").and_then(Value::as_str) {
        Some("tool_execution_start") => event
            .get("toolName")
            .and_then(Value::as_str)
            .map(String::from),
        Some("turn_end") => Some("thinking".into()),
        _ => None,
    }
}

#[derive(Default)]
struct InboxState {
    records: HashMap<String, InboxRecord>,
    /// Insertion order, for listing and pruning.
    order: Vec<String>,
    label_counters: HashMap<String, u64>,
    observed_pools: HashSet<u64>,
}

impl InboxState {
    fn get(&self, handle: &str) -> Option<&InboxRecord> {
        self.records
            .get(handle)
            .or_else(|| self.records.values().find(|r| r.label == handle))
    }

    fn get_mut(&mut self, handle: &str) -> Option<&mut InboxRecord> {
        if self.records.contains_key(handle) {
            return self.records.get_mut(handle);
        }
        self.records.values_mut().find(|r| r.label == handle)
    }

    fn list(&self) -> Vec<InboxRecord> {
        self.order
            .iter()
            .filter_map(|id| self.records.get(id).cloned())
            .collect()
    }

    fn outstanding_count(&self) -> usize {
        self.records
            .values()
            .filter(|r| r.lifecycle.is_outstanding())
            .count()
    }

    /// Drop the oldest settled records past the cap.
    fn prune(&mut self) {
        let mut settled = self
            .order
            .iter()
            .filter(|id| {
                self.records
                    .get(*id)
                    .is_some_and(|r| !r.lifecycle.is_outstanding())
            })
            .count();
        if settled <= MAX_SETTLED {
            return;
        }
        let mut kept = Vec::new();
        for id in std::mem::take(&mut self.order) {
            let is_settled = self
                .records
                .get(&id)
                .is_some_and(|r| !r.lifecycle.is_outstanding());
            if is_settled && settled > MAX_SETTLED {
                self.records.remove(&id);
                settled -= 1;
                continue;
            }
            kept.push(id);
        }
        self.order = kept;
    }
}

/// `SubagentInbox`.
#[derive(Default)]
pub struct SubagentInbox {
    state: Mutex<InboxState>,
    /// Woken whenever a record settles (the wait helpers).
    settled: Notify,
}

impl SubagentInbox {
    fn state(&self) -> MutexGuard<'_, InboxState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notify_settle(&self) {
        self.settled.notify_waiters();
    }

    /// Resolve once the task settles (at once if settled or unknown), bounded
    /// by `timeout`.
    pub async fn wait_for(&self, handle: &str, timeout: Duration) -> Option<InboxRecord> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.settled.notified();
            let current = self.get(handle);
            match &current {
                Some(rec) if rec.lifecycle.is_outstanding() => {}
                _ => return current,
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.get(handle);
            }
        }
    }

    /// Resolve once nothing is outstanding (the swarm barrier), bounded by
    /// `timeout`.
    pub async fn wait_for_all(&self, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.settled.notified();
            if self.state().outstanding_count() == 0 {
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return;
            }
        }
    }

    /// The next label for an agent type (`explore#1`, `explore#2`, ...).
    pub fn next_label(&self, agent_type: &str) -> String {
        let mut state = self.state();
        let n = state
            .label_counters
            .entry(agent_type.to_string())
            .or_insert(0);
        *n += 1;
        format!("{agent_type}#{n}")
    }

    /// Track a pool's `task_progress` so running records carry a live
    /// activity. Idempotent per pool.
    pub fn observe(self: &Arc<Self>, pool: &SubagentPool) {
        if !self.state().observed_pools.insert(pool.id()) {
            return;
        }
        let inbox = Arc::downgrade(self);
        pool.on(move |event| {
            if event.name != "task_progress" {
                return;
            }
            let Some(inbox) = inbox.upgrade() else { return };
            let Some(task_id) = event.data.get("task_id").and_then(Value::as_str) else {
                return;
            };
            let mut state = inbox.state();
            let Some(rec) = state.records.get_mut(task_id) else {
                return;
            };
            if rec.lifecycle != TaskLifecycle::Running {
                return;
            }
            if let Some(activity) = activity_from_event(&event.data["event"]) {
                rec.last_activity = Some(activity);
            }
        });
    }

    /// Register a freshly dispatched background task as running.
    pub fn start(&self, task_id: &str, label: &str, agent_type: &str) -> InboxRecord {
        let rec = InboxRecord {
            task_id: task_id.to_string(),
            label: label.to_string(),
            agent_type: agent_type.to_string(),
            lifecycle: TaskLifecycle::Running,
            started_at: now_ms(),
            ended_at: None,
            last_activity: None,
            summary_line: None,
            body: None,
            error: None,
        };
        let mut state = self.state();
        state.records.insert(task_id.to_string(), rec.clone());
        state.order.push(task_id.to_string());
        state.prune();
        rec
    }

    /// Settle a task from its dispatch result: the body on success, the
    /// reason on failure.
    pub fn finish(&self, task_id: &str, result: &TaskResult) -> Option<InboxRecord> {
        let rec = {
            let mut state = self.state();
            let rec = state.records.get_mut(task_id)?;
            rec.ended_at = Some(now_ms());
            rec.last_activity = None;
            let r = result.result.as_ref();
            if r.is_some_and(|r| r.ok) {
                let summary = r
                    .and_then(|r| r.result_data.as_ref())
                    .and_then(|d| d.get("summary"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("(subagent returned no output)")
                    .to_string();
                rec.lifecycle = TaskLifecycle::Done;
                rec.summary_line = Some(first_line(&summary, 120));
                rec.body = Some(summary);
            } else {
                let status = r.and_then(|r| r.status);
                rec.lifecycle = fail_lifecycle(status);
                let error = r
                    .and_then(|r| r.error.clone())
                    .unwrap_or_else(|| match status {
                        Some(status) => format!("subagent {}", status.as_str()),
                        None => "unknown error".into(),
                    });
                rec.summary_line = Some(error.clone());
                rec.error = Some(error);
            }
            rec.clone()
        };
        self.notify_settle();
        Some(rec)
    }

    /// Settle a task that never produced a result (a failed dispatch).
    pub fn fail(
        &self,
        task_id: &str,
        reason: &str,
        lifecycle: TaskLifecycle,
    ) -> Option<InboxRecord> {
        let rec = {
            let mut state = self.state();
            let rec = state.records.get_mut(task_id)?;
            rec.ended_at = Some(now_ms());
            rec.last_activity = None;
            rec.lifecycle = lifecycle;
            rec.error = Some(reason.to_string());
            rec.summary_line = Some(reason.to_string());
            rec.clone()
        };
        self.notify_settle();
        Some(rec)
    }

    /// A record by task id or label.
    pub fn get(&self, handle: &str) -> Option<InboxRecord> {
        self.state().get(handle).cloned()
    }

    /// Read a done task's body once and mark it collected (the body is
    /// dropped so it is not re-fed to the model).
    pub fn collect(&self, handle: &str) -> Option<(InboxRecord, String)> {
        let mut state = self.state();
        let rec = state.get_mut(handle)?;
        if rec.lifecycle != TaskLifecycle::Done {
            return None;
        }
        let body = rec.body.take()?;
        rec.lifecycle = TaskLifecycle::Collected;
        let mut record = rec.clone();
        record.body = Some(body.clone());
        Some((record, body))
    }

    /// All records, oldest first.
    pub fn list(&self) -> Vec<InboxRecord> {
        self.state().list()
    }

    /// Records still doing work.
    pub fn outstanding(&self) -> Vec<InboxRecord> {
        self.list()
            .into_iter()
            .filter(|r| r.lifecycle.is_outstanding())
            .collect()
    }

    /// Test/teardown helper.
    pub fn clear(&self) {
        let mut state = self.state();
        state.records.clear();
        state.order.clear();
        state.label_counters.clear();
    }
}

static INBOX: LazyLock<Arc<SubagentInbox>> = LazyLock::new(Arc::default);

/// The process-wide inbox shared by the Task and TaskOutput tools.
pub fn subagent_inbox() -> &'static Arc<SubagentInbox> {
    &INBOX
}

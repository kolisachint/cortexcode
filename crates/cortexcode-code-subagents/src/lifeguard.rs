//! `core/lifeguard.ts`: watches running subagent processes for heartbeats and
//! hard timeouts, and sweeps stale dispatch directories.
//!
//! A child must print `{"ping":true}` periodically (any stdout counts, see the
//! pool). A child silent past the load-scaled threshold is killed with its
//! whole process group and reported `stalled`; one past its hard timeout is
//! reported `timeout`. Under load (several subagents, background MCP tools)
//! both budgets widen, up to a ceiling, so a starved parent does not reap
//! healthy children.
//!
//! Deviation: hoocode also hooks the parent's SIGINT/SIGTERM to shut children
//! down gracefully; here the host calls [`SubagentLifeguard::graceful_shutdown`]
//! from its own signal handling, since a library must not take over signals.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;

use crate::agent_log::agent_log;

/// Base hard timeout per agent type (explore's for any other type).
fn base_timeout_ms(agent_type: &str) -> u64 {
    match agent_type {
        "edit" | "test" => 10 * 60 * 1000,
        "review" => 8 * 60 * 1000,
        _ => 5 * 60 * 1000,
    }
}

const HEARTBEAT_MISS_THRESHOLD_MS: u64 = 60_000;
const HEARTBEAT_CHECK_INTERVAL_MS: u64 = 5_000;
const PARENT_SHUTDOWN_GRACE_MS: u64 = 5_000;
/// Each additional concurrent task adds this fraction to both budgets.
const LOAD_TOLERANCE_PER_PROCESS: f64 = 0.5;
/// Ceiling on the load multiplier: a stuck child is still reaped eventually.
const MAX_LOAD_MULTIPLIER: f64 = 4.0;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Kill a process group (the pool spawns children as group leaders), falling
/// back to the single process (`killProcessTree`).
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let pid = pid as libc::pid_t;
        // SAFETY: plain kill(2) calls.
        unsafe {
            if libc::kill(-pid, libc::SIGKILL) != 0 {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .spawn();
    }
}

#[cfg(unix)]
fn terminate_group(pid: u32) {
    // SAFETY: plain kill(2) call.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_group(pid: u32) {
    kill_process_tree(pid);
}

/// What the lifeguard reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifeguardEvent {
    Stalled { task_id: String, pid: u32 },
    Timeout { task_id: String, pid: u32 },
}

type Listener = Arc<dyn Fn(&LifeguardEvent) + Send + Sync>;

#[derive(Debug, Clone)]
struct Monitored {
    pid: u32,
    agent_type: String,
}

#[derive(Default)]
struct State {
    processes: HashMap<String, Monitored>,
    last_heartbeat: HashMap<String, u64>,
    timeouts: HashMap<String, JoinHandle<()>>,
    started_at: HashMap<String, u64>,
    base_timeout_ms: HashMap<String, u64>,
    last_check_at: u64,
    external_load: u64,
    /// Reaped (kill sent) but not yet exited: not re-reported each tick.
    reaping: HashSet<String>,
    disposed: bool,
}

impl State {
    fn load_multiplier(&self) -> f64 {
        let concurrent = self.processes.len() as u64 + self.external_load;
        let mult = 1.0 + (concurrent.saturating_sub(1)) as f64 * LOAD_TOLERANCE_PER_PROCESS;
        mult.min(MAX_LOAD_MULTIPLIER)
    }
}

/// `SubagentLifeguard`.
pub struct SubagentLifeguard {
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
    check_task: Mutex<Option<JoinHandle<()>>>,
}

impl SubagentLifeguard {
    /// Sweep stale dispatch dirs under `cwd` and start the heartbeat check.
    /// Must run inside a tokio runtime.
    pub fn new(cwd: impl AsRef<Path>) -> Arc<Self> {
        sweep_old_agents(cwd.as_ref());
        let guard = Arc::new(Self {
            state: Mutex::new(State {
                last_check_at: now_ms(),
                ..Default::default()
            }),
            listeners: Mutex::new(Vec::new()),
            check_task: Mutex::new(None),
        });
        let weak = Arc::downgrade(&guard);
        let task = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_millis(HEARTBEAT_CHECK_INTERVAL_MS));
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(guard) = weak.upgrade() else { break };
                guard.check_heartbeats();
            }
        });
        *guard.check_task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
        guard
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Listen for `stalled` / `timeout`.
    pub fn on_event(&self, listener: impl Fn(&LifeguardEvent) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(listener));
    }

    fn emit(&self, event: LifeguardEvent) {
        let listeners = self
            .listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for listener in listeners {
            listener(&event);
        }
    }

    /// Count of external in-process tasks (background MCP tools) sharing the
    /// parent; widens the budgets like extra subagents. Negative is 0.
    pub fn set_external_load(&self, count: i64) {
        self.state().external_load = count.max(0) as u64;
    }

    /// Start monitoring a child. The caller reports its exit with
    /// [`untrack`](Self::untrack).
    pub fn monitor(self: &Arc<Self>, task_id: &str, agent_type: &str, pid: u32) {
        let mut state = self.state();
        if state.disposed {
            return;
        }
        let now = now_ms();
        state.processes.insert(
            task_id.to_string(),
            Monitored {
                pid,
                agent_type: agent_type.to_string(),
            },
        );
        state.last_heartbeat.insert(task_id.to_string(), now);
        let base = base_timeout_ms(agent_type);
        state.started_at.insert(task_id.to_string(), now);
        state.base_timeout_ms.insert(task_id.to_string(), base);
        let delay = (base as f64 * state.load_multiplier()).round() as u64;
        let handle = self.arm_timeout(task_id, delay);
        if let Some(old) = state.timeouts.insert(task_id.to_string(), handle) {
            old.abort();
        }
    }

    fn arm_timeout(self: &Arc<Self>, task_id: &str, delay_ms: u64) -> JoinHandle<()> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if let Some(guard) = weak.upgrade() {
                guard.handle_timeout(&task_id);
            }
        })
    }

    /// Record a heartbeat for a monitored task.
    pub fn record_heartbeat(&self, task_id: &str) {
        let mut state = self.state();
        if state.processes.contains_key(task_id) {
            state.last_heartbeat.insert(task_id.to_string(), now_ms());
        }
    }

    /// The last heartbeat (epoch ms), if monitored.
    pub fn last_heartbeat_at(&self, task_id: &str) -> Option<u64> {
        self.state().last_heartbeat.get(task_id).copied()
    }

    pub fn is_monitoring(&self, task_id: &str) -> bool {
        self.state().processes.contains_key(task_id)
    }

    /// Test hook: backdate a task's last heartbeat.
    #[doc(hidden)]
    pub fn set_last_heartbeat_for_testing(&self, task_id: &str, at_ms: u64) {
        self.state()
            .last_heartbeat
            .insert(task_id.to_string(), at_ms);
    }

    /// Test hook: backdate the last heartbeat check (event-loop lag).
    #[doc(hidden)]
    pub fn set_last_check_for_testing(&self, at_ms: u64) {
        self.state().last_check_at = at_ms;
    }

    /// Test hook: replace a task's hard timeout with a short one.
    #[doc(hidden)]
    pub fn set_timeout_for_testing(self: &Arc<Self>, task_id: &str, delay: Duration) {
        let handle = self.arm_timeout(task_id, delay.as_millis() as u64);
        if let Some(old) = self.state().timeouts.insert(task_id.to_string(), handle) {
            old.abort();
        }
    }

    /// Test hook: deliver an event as if the lifeguard had decided it (no kill).
    #[doc(hidden)]
    pub fn inject_event_for_testing(&self, event: LifeguardEvent) {
        self.emit(event);
    }

    /// Reap every task silent past the load-scaled threshold (run every 5s).
    pub fn check_heartbeats(&self) {
        let now = now_ms();
        let stalled: Vec<String> = {
            let mut state = self.state();
            // Forgive the parent's own starvation (the check ran late).
            let loop_lag = now
                .saturating_sub(state.last_check_at)
                .saturating_sub(HEARTBEAT_CHECK_INTERVAL_MS);
            state.last_check_at = now;
            let threshold =
                HEARTBEAT_MISS_THRESHOLD_MS as f64 * state.load_multiplier() + loop_lag as f64;
            state
                .processes
                .keys()
                .filter(|id| !state.reaping.contains(*id))
                .filter(|id| {
                    state
                        .last_heartbeat
                        .get(*id)
                        .is_some_and(|last| now.saturating_sub(*last) as f64 > threshold)
                })
                .cloned()
                .collect()
        };
        for task_id in stalled {
            self.handle_stalled(&task_id);
        }
    }

    fn handle_stalled(&self, task_id: &str) {
        let (monitored, line) = {
            let mut state = self.state();
            let Some(monitored) = state.processes.get(task_id).cloned() else {
                return;
            };
            if !state.reaping.insert(task_id.to_string()) {
                return;
            }
            let silent = state
                .last_heartbeat
                .get(task_id)
                .map_or(-1, |last| now_ms().saturating_sub(*last) as i64);
            let line = format!(
                "[LIFEGUARD] stalled task_id={task_id} agent={} silent_ms={silent} concurrent={} load_mult={:.2} base_threshold_ms={HEARTBEAT_MISS_THRESHOLD_MS}",
                monitored.agent_type,
                state.processes.len(),
                state.load_multiplier(),
            );
            (monitored, line)
        };
        agent_log(&line);
        kill_tree(monitored.pid);
        self.emit(LifeguardEvent::Stalled {
            task_id: task_id.to_string(),
            pid: monitored.pid,
        });
    }

    fn handle_timeout(self: &Arc<Self>, task_id: &str) {
        let monitored = {
            let mut state = self.state();
            let Some(monitored) = state.processes.get(task_id).cloned() else {
                return;
            };
            if state.reaping.contains(task_id) {
                return;
            }
            // Under load, re-arm rather than kill, up to base * MAX_LOAD_MULTIPLIER.
            let now = now_ms();
            let started = state.started_at.get(task_id).copied().unwrap_or(now);
            let base = state
                .base_timeout_ms
                .get(task_id)
                .copied()
                .unwrap_or_else(|| base_timeout_ms(&monitored.agent_type));
            let elapsed = now.saturating_sub(started);
            let ceiling = (base as f64 * MAX_LOAD_MULTIPLIER) as u64;
            let mult = state.load_multiplier();
            if mult > 1.0 && elapsed < ceiling {
                let remaining = ceiling - elapsed;
                let next = ((base as f64 * mult).round() as u64)
                    .min(HEARTBEAT_CHECK_INTERVAL_MS.max(remaining));
                drop(state);
                let handle = self.arm_timeout(task_id, next);
                self.state().timeouts.insert(task_id.to_string(), handle);
                return;
            }
            state.reaping.insert(task_id.to_string());
            state.timeouts.remove(task_id);
            monitored
        };
        kill_tree(monitored.pid);
        self.emit(LifeguardEvent::Timeout {
            task_id: task_id.to_string(),
            pid: monitored.pid,
        });
    }

    /// The child exited: stop monitoring it.
    pub fn untrack(&self, task_id: &str) {
        let mut state = self.state();
        if let Some(timeout) = state.timeouts.remove(task_id) {
            timeout.abort();
        }
        state.processes.remove(task_id);
        state.last_heartbeat.remove(task_id);
        state.started_at.remove(task_id);
        state.base_timeout_ms.remove(task_id);
        state.reaping.remove(task_id);
    }

    /// The parent is shutting down: SIGTERM every child's process group, then
    /// kill the trees after a grace period.
    pub fn graceful_shutdown(self: &Arc<Self>) {
        let pids: Vec<u32> = self.state().processes.values().map(|m| m.pid).collect();
        for pid in pids.iter().filter(|p| **p > 0) {
            terminate_group(*pid);
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(PARENT_SHUTDOWN_GRACE_MS)).await;
            if let Some(guard) = weak.upgrade() {
                let pids: Vec<u32> = guard.state().processes.values().map(|m| m.pid).collect();
                for pid in pids {
                    kill_tree(pid);
                }
            }
        });
    }

    /// Kill all monitored processes and stop monitoring.
    pub fn dispose(&self) {
        let pids: Vec<u32> = {
            let mut state = self.state();
            if state.disposed {
                return;
            }
            state.disposed = true;
            for (_, timeout) in state.timeouts.drain() {
                timeout.abort();
            }
            let pids = state.processes.values().map(|m| m.pid).collect();
            state.processes.clear();
            state.last_heartbeat.clear();
            state.started_at.clear();
            state.base_timeout_ms.clear();
            state.reaping.clear();
            pids
        };
        if let Some(task) = self
            .check_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
        for pid in pids {
            kill_tree(pid);
        }
        self.listeners
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Drop for SubagentLifeguard {
    fn drop(&mut self) {
        if let Some(task) = self
            .check_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

/// A pid of 0 means the spawn never produced a process: nothing to kill.
fn kill_tree(pid: u32) {
    if pid > 0 {
        kill_process_tree(pid);
    }
}

/// Remove dispatch dirs older than 24 hours whose `pid` file names no live
/// process.
fn sweep_old_agents(cwd: &Path) {
    let dispatch_dir = cortexcode_code_paths::dispatch_root(cwd);
    let Ok(entries) = std::fs::read_dir(&dispatch_dir) else {
        return;
    };
    let cutoff = Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age > cutoff);
        if old && !has_running_pid(&path) {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

fn has_running_pid(dir: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join("pid")) else {
        return false;
    };
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let Ok(pid) = digits.parse::<i64>() else {
        return false;
    };
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

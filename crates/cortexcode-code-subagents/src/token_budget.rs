//! `core/token-budget.ts`: cumulative token usage for one subagent task, read
//! from the child's `message_end` events.
//!
//! Advisory only: it never stops a subagent (the hard stop is `--max-turns`
//! and the lifeguard's per-agent deadline). `budget_warning` fires once at 80%
//! of the budget, `budget_exceeded` once at 100%; the state persists to the
//! task's `budget.json` on every usage event.
//!
//! Deviation: hoocode accumulates `usage.totalTokens`, which is the *context
//! size* of the turn — the whole conversation resent each request, not the work
//! done. Summing it is quadratic: N turns of a ~10k context report ~10k·N²/2.
//! The recorded runs report 1 654 795 against a 35 000 budget (47x) when their
//! real generated total is 4 111.
//!
//! Note that fixing the arithmetic is not enough. The delta of `totalTokens`
//! still counts the fixed system-prompt + tool-schema floor, which is 25k-62k
//! on turn 1 alone — it crosses a 35k budget at turn 1 or 2 in 7 of the 8
//! recorded runs. So `used` counts what the subagent *generated*
//! (`output` + `cacheWrite`, which are per-request figures and need no
//! baseline), and the context size is reported separately as `peak_context`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Default budget for an agent type (custom agents get 35 000).
pub fn get_default_budget(agent_type: &str) -> u64 {
    match agent_type {
        "explore" | "plan" => 35_000,
        "general-purpose" => 60_000,
        _ => 35_000,
    }
}

/// `TokenBudget` options.
#[derive(Debug, Clone, Default)]
pub struct TokenBudgetOptions {
    pub limit: Option<u64>,
    pub cwd: Option<PathBuf>,
}

/// A budget event listener (`budget_warning` / `budget_exceeded` payloads).
pub type BudgetListener = Box<dyn FnMut(&Value) + Send>;

/// `TokenBudget`.
pub struct TokenBudget {
    task_id: String,
    agent_type: String,
    limit: u64,
    cwd: PathBuf,
    used: u64,
    /// Largest `usage.totalTokens` seen: this run's peak context size. Reported
    /// in `budget.json` for diagnostics; never compared against the budget.
    peak_context: u64,
    warned: bool,
    exceeded: bool,
    stdout_buffer: String,
    warning_threshold: u64,
    exceeded_threshold: u64,
    warning_listeners: Vec<BudgetListener>,
    exceeded_listeners: Vec<BudgetListener>,
}

impl TokenBudget {
    pub fn new(task_id: &str, agent_type: &str, options: TokenBudgetOptions) -> Self {
        let limit = options
            .limit
            .unwrap_or_else(|| get_default_budget(agent_type));
        Self {
            task_id: task_id.to_string(),
            agent_type: agent_type.to_string(),
            limit,
            cwd: options
                .cwd
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into())),
            used: 0,
            peak_context: 0,
            warned: false,
            exceeded: false,
            stdout_buffer: String::new(),
            // Math.floor(limit * 0.8)
            warning_threshold: (limit as f64 * 0.8).floor() as u64,
            exceeded_threshold: limit,
            warning_listeners: Vec::new(),
            exceeded_listeners: Vec::new(),
        }
    }

    /// `on("budget_warning", ...)`.
    pub fn on_warning(&mut self, listener: BudgetListener) {
        self.warning_listeners.push(listener);
    }

    /// `on("budget_exceeded", ...)`.
    pub fn on_exceeded(&mut self, listener: BudgetListener) {
        self.exceeded_listeners.push(listener);
    }

    /// One complete JSONL line (the pool's line reader handles framing).
    pub fn process_line(&mut self, line: &str) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            self.parse_line(trimmed);
        }
    }

    /// A raw stdout chunk, buffered until newlines.
    pub fn process_stdout(&mut self, chunk: &str) {
        self.stdout_buffer.push_str(chunk);
        while let Some(end) = self.stdout_buffer.find('\n') {
            let line = self.stdout_buffer[..end].trim_end().to_string();
            self.stdout_buffer.drain(..=end);
            if !line.is_empty() {
                self.parse_line(&line);
            }
        }
    }

    /// Process the remaining buffered stdout (the stream ended).
    pub fn flush(&mut self) {
        let rest = self.stdout_buffer.trim().to_string();
        if !rest.is_empty() {
            self.parse_line(&rest);
            self.stdout_buffer.clear();
        }
    }

    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// This run's peak context size, in tokens. Diagnostic only.
    pub fn peak_context(&self) -> u64 {
        self.peak_context
    }

    pub fn is_warned(&self) -> bool {
        self.warned
    }

    pub fn is_exceeded(&self) -> bool {
        self.exceeded
    }

    /// Persist the state to `budget.json`. Best-effort.
    pub fn persist(&self) {
        let last_updated = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let state = json!({
            "task_id": self.task_id,
            "agent_type": self.agent_type,
            "budget": self.limit,
            "used": self.used,
            "peak_context": self.peak_context,
            "warned": self.warned,
            "exceeded": self.exceeded,
            "last_updated": last_updated,
        });
        let path =
            cortexcode_code_paths::dispatch_task_dir(&self.cwd, &self.task_id).join("budget.json");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            &path,
            serde_json::to_string_pretty(&state).unwrap_or_default(),
        );
    }

    fn parse_line(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if event.get("type").and_then(Value::as_str) != Some("message_end") {
            return;
        }
        let Some(message) = event.get("message").filter(|m| m.is_object()) else {
            return;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        let Some(usage) = message.get("usage").filter(|u| u.is_object()) else {
            return;
        };
        // What the subagent generated this turn: `output` and `cacheWrite` are
        // per-request figures, so they sum directly.
        let generated = usage
            .get("output")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            .max(0.0)
            + usage
                .get("cacheWrite")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(0.0);
        if generated > 0.0 {
            self.used += generated as u64;
        }
        // Context size for this turn, tracked separately: it is what a reader
        // of budget.json actually wants to see grow, and it is not what the
        // budget is measured against.
        if let Some(total) = usage.get("totalTokens").and_then(Value::as_f64) {
            if total > 0.0 {
                self.peak_context = self.peak_context.max(total as u64);
            }
        }
        if self.used == 0 {
            // A provider that reports neither (or an empty usage block) must
            // not re-fire the thresholds on every event.
            return;
        }

        if !self.warned && self.used >= self.warning_threshold {
            self.warned = true;
            let payload = json!({
                "task_id": self.task_id,
                "message": "You are near token limit. Summarize and write result.json now.",
                "used": self.used,
                "limit": self.limit,
            });
            for listener in &mut self.warning_listeners {
                listener(&payload);
            }
        }
        if !self.exceeded && self.used >= self.exceeded_threshold {
            self.exceeded = true;
            let payload = json!({
                "task_id": self.task_id,
                "used": self.used,
                "limit": self.limit,
            });
            for listener in &mut self.exceeded_listeners {
                listener(&payload);
            }
        }
        self.persist();
    }
}

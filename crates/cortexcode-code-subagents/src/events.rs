//! `core/subagent-events.ts` plus `classifySubagentLine` from
//! `core/subagent-pool.ts`: which child stdout events cross the pipe and what
//! the parent does with each line.
//!
//! A subagent child writes only the progress events (forwarded to the UI as
//! `task_progress`) and `message_end` (token usage for the parent's
//! [`TokenBudget`](crate::token_budget::TokenBudget)); the per-delta firehose
//! is dropped at the source. The result travels via `result.json` and
//! liveness via periodic `{"ping":true}` lines.

use serde_json::{Map, Value};

/// `SUBAGENT_PROGRESS_EVENTS` (and the pool's `FORWARDED_SUBAGENT_EVENTS`).
pub const SUBAGENT_PROGRESS_EVENTS: &[&str] =
    &["turn_end", "tool_execution_start", "tool_execution_end"];

/// `SUBAGENT_STDOUT_EVENT_TYPES`: what the child writes.
pub const SUBAGENT_STDOUT_EVENT_TYPES: &[&str] = &[
    "turn_end",
    "tool_execution_start",
    "tool_execution_end",
    "message_end",
];

/// `FORWARDED_SUBAGENT_EVENTS`: the pool forwards exactly the progress set.
pub const FORWARDED_SUBAGENT_EVENTS: &[&str] = SUBAGENT_PROGRESS_EVENTS;

/// `SubagentStdoutLine`: the action for one child stdout line.
#[derive(Debug, Clone, PartialEq)]
pub enum SubagentStdoutLine {
    Heartbeat,
    Progress(Map<String, Value>),
    Ignore,
}

/// Classify one complete JSONL line from a subagent's stdout.
pub fn classify_subagent_line(line: &str) -> SubagentStdoutLine {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return SubagentStdoutLine::Ignore;
    }
    let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(trimmed) else {
        return SubagentStdoutLine::Ignore;
    };
    if parsed.get("ping") == Some(&Value::Bool(true)) {
        return SubagentStdoutLine::Heartbeat;
    }
    match parsed.get("type").and_then(Value::as_str) {
        Some(t) if FORWARDED_SUBAGENT_EVENTS.contains(&t) => SubagentStdoutLine::Progress(parsed),
        _ => SubagentStdoutLine::Ignore,
    }
}

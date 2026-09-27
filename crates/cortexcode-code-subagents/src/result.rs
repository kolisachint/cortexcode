//! `core/subagent-result.ts`: a subagent's `result.json` audit file.
//!
//! A child run with `--task-id <id>` writes
//! `.hoocode/dispatch/<task_id>/result.json`, which the parent pool verifies
//! against a fixed schema ([`OutputVerifier`](crate::output_verifier::OutputVerifier)).
//! The file is derived from the finished session, so subagents never write it
//! themselves.

use std::collections::HashMap;
use std::path::Path;

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, StopReason};
use cortexcode_code_task_store::{Task, TaskSource, TaskStatus, TaskUsage};
use serde_json::{json, Map, Value};

/// `SubagentUsage`.
pub type SubagentUsage = TaskUsage;

/// `SubagentTaskNode`: one node of a subagent's task subtree, propagated to
/// the parent so nested delegation is visible above the process boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentTaskNode {
    pub id: u64,
    pub title: String,
    pub status: TaskStatus,
    /// Origin of the task (unset: the subagent's own TodoWrite plan).
    pub source: Option<TaskSource>,
    pub subagent_mode: Option<String>,
    pub usage: Option<SubagentUsage>,
    pub children: Vec<SubagentTaskNode>,
}

/// `status` of a result file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultStatus {
    Complete,
    Partial,
    Failed,
}

impl ResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

/// `SubagentResultFile`.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentResultFile {
    pub summary: String,
    pub files_changed: Vec<String>,
    pub confidence: f64,
    pub status: ResultStatus,
    /// Token and cost usage (extra field; ignored by the verifier).
    pub usage: Option<SubagentUsage>,
    /// The child's own task subtree (extra field; ignored by the verifier).
    pub task_tree: Option<Vec<SubagentTaskNode>>,
}

/// A JSON number as JS holds it: integral values print without `.0`.
pub(crate) fn js_number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

fn usage_json(usage: &SubagentUsage) -> Value {
    json!({
        "input": js_number(usage.input),
        "output": js_number(usage.output),
        "cacheRead": js_number(usage.cache_read),
        "cacheWrite": js_number(usage.cache_write),
        "cost": js_number(usage.cost),
    })
}

fn source_str(source: TaskSource) -> &'static str {
    match source {
        TaskSource::Subagent => "subagent",
        TaskSource::Mcp => "mcp",
    }
}

impl SubagentTaskNode {
    /// The node in `JSON.stringify` shape (unset fields omitted).
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("id".into(), Value::from(self.id));
        map.insert("title".into(), Value::from(self.title.clone()));
        map.insert("status".into(), Value::from(self.status.as_str()));
        if let Some(source) = self.source {
            map.insert("source".into(), Value::from(source_str(source)));
        }
        if let Some(mode) = &self.subagent_mode {
            map.insert("subagentMode".into(), Value::from(mode.clone()));
        }
        if let Some(usage) = &self.usage {
            map.insert("usage".into(), usage_json(usage));
        }
        map.insert(
            "children".into(),
            Value::Array(self.children.iter().map(Self::to_json).collect()),
        );
        Value::Object(map)
    }
}

impl SubagentResultFile {
    /// The file's JSON (field order as hoocode writes it).
    pub fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("summary".into(), Value::from(self.summary.clone()));
        map.insert("files_changed".into(), json!(self.files_changed));
        map.insert("confidence".into(), js_number(self.confidence));
        map.insert("status".into(), Value::from(self.status.as_str()));
        if let Some(usage) = &self.usage {
            map.insert("usage".into(), usage_json(usage));
        }
        if let Some(tree) = &self.task_tree {
            map.insert(
                "task_tree".into(),
                Value::Array(tree.iter().map(SubagentTaskNode::to_json).collect()),
            );
        }
        Value::Object(map)
    }
}

/// `buildTaskForest`: link a flat task list into a forest via
/// `parent_task_id`, keeping store order among siblings.
pub fn build_task_forest(tasks: &[Task]) -> Vec<SubagentTaskNode> {
    let mut children_by_parent: HashMap<u64, Vec<&Task>> = HashMap::new();
    let mut roots = Vec::new();
    for task in tasks {
        match task.parent_task_id {
            None => roots.push(task),
            Some(parent) => children_by_parent.entry(parent).or_default().push(task),
        }
    }
    fn to_node(task: &Task, children: &HashMap<u64, Vec<&Task>>) -> SubagentTaskNode {
        SubagentTaskNode {
            id: task.id,
            title: task.title.clone(),
            status: task.status,
            source: task.source,
            subagent_mode: task.subagent_mode.clone(),
            usage: task.usage,
            children: children
                .get(&task.id)
                .map(|kids| kids.iter().map(|k| to_node(k, children)).collect())
                .unwrap_or_default(),
        }
    }
    roots
        .into_iter()
        .map(|t| to_node(t, &children_by_parent))
        .collect()
}

/// Tools that mutate files; their `path`/`file_path` argument is a changed file.
const MUTATING_TOOLS: &[&str] = &["edit", "write"];

/// Distinct file paths touched by edit/write tool calls, in first-seen order.
fn collect_changed_files(messages: &[AgentMessage]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for message in messages {
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        for content in &assistant.content {
            let Content::ToolCall(call) = content else {
                continue;
            };
            if !MUTATING_TOOLS.contains(&call.name.as_str()) {
                continue;
            }
            let path = call
                .arguments
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| call.arguments.get("file_path").and_then(Value::as_str));
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                if !files.iter().any(|f| f == path) {
                    files.push(path.to_string());
                }
            }
        }
    }
    files
}

/// The last non-empty assistant text, trimmed.
fn derive_summary(messages: &[AgentMessage]) -> String {
    for message in messages.iter().rev() {
        let AgentMessage::Assistant(assistant) = message else {
            continue;
        };
        let text: String = assistant
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    String::new()
}

fn last_assistant(messages: &[AgentMessage]) -> Option<&cortexcode_ai_types::AssistantMessage> {
    match messages.last() {
        Some(AgentMessage::Assistant(assistant)) => Some(assistant),
        _ => None,
    }
}

/// Failed when the final assistant message errored or was aborted.
fn derive_failed(messages: &[AgentMessage]) -> bool {
    last_assistant(messages)
        .is_some_and(|a| matches!(a.stop_reason, StopReason::Error | StopReason::Aborted))
}

/// The final assistant message's error text when the run ended in an error.
fn derive_error_message(messages: &[AgentMessage]) -> Option<String> {
    let assistant = last_assistant(messages)?;
    if assistant.stop_reason != StopReason::Error {
        return None;
    }
    let trimmed = assistant.error_message.as_deref()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `BuildSubagentResultOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildSubagentResultOptions {
    /// The run stopped at its turn cap: report a usable partial result.
    pub reached_max_turns: bool,
}

fn or_else(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

/// `buildSubagentResult`: the `result.json` payload for a finished session.
/// The verifier needs a non-empty summary and confidence >= 0.5, so every
/// branch yields both.
pub fn build_subagent_result(
    messages: &[AgentMessage],
    usage: Option<SubagentUsage>,
    options: BuildSubagentResultOptions,
) -> SubagentResultFile {
    let files_changed = collect_changed_files(messages);
    if options.reached_max_turns {
        return SubagentResultFile {
            summary: or_else(
                derive_summary(messages),
                "Reached the turn limit before completing. Returning partial findings.",
            ),
            files_changed,
            confidence: 0.6,
            status: ResultStatus::Partial,
            usage,
            task_tree: None,
        };
    }
    if derive_failed(messages) {
        // Prefer the provider/model error over partial assistant text.
        let summary = match derive_error_message(messages) {
            Some(error) => format!("Task failed: {error}"),
            None => or_else(derive_summary(messages), "Task failed."),
        };
        return SubagentResultFile {
            summary,
            files_changed,
            confidence: 0.5,
            status: ResultStatus::Failed,
            usage,
            task_tree: None,
        };
    }
    SubagentResultFile {
        summary: or_else(
            derive_summary(messages),
            "Task completed with no textual summary.",
        ),
        files_changed,
        confidence: 0.9,
        status: ResultStatus::Complete,
        usage,
        task_tree: None,
    }
}

/// `writeFileAtomicSync`: temp file in the same directory, then rename.
pub(crate) fn write_file_atomic(path: &Path, data: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    std::io::Write::write_all(&mut tmp, data.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// `writeSubagentResult`: write `result.json` for a task. Best-effort.
pub fn write_subagent_result(cwd: &Path, task_id: &str, result: &SubagentResultFile) {
    let path = cortexcode_code_paths::dispatch_task_dir(cwd, task_id).join("result.json");
    // Atomic: the parent may read the file the moment it appears, or kill the
    // child mid-write; a torn file would turn a success into a failure.
    let text = serde_json::to_string_pretty(&result.to_json()).unwrap_or_default();
    let _ = write_file_atomic(&path, &text);
}

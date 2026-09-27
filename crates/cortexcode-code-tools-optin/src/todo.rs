//! `core/tools/todo.ts`: the TodoWrite tool over the task store.

use std::sync::Arc;

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::{Content, TextContent};
use cortexcode_code_task_store::{Task, TaskPatch, TaskStatus, TaskStore};
use cortexcode_code_tool_api::{ToolDefinition, ToolError};
use serde_json::{json, Value};

/// `TODO_WRITE_TOOL_NAME`.
pub const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

/// One incoming item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub content: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: TodoStatus,
    pub active_form: Option<String>,
}

/// The model-facing status vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            _ => return None,
        })
    }

    fn task_status(self) -> TaskStatus {
        match self {
            Self::Pending => TaskStatus::Pending,
            Self::InProgress => TaskStatus::InProgress,
            Self::Completed => TaskStatus::Done,
        }
    }
}

fn glyph(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "[ ]",
        TaskStatus::InProgress => "[~]",
        TaskStatus::Done => "[x]",
        TaskStatus::Failed => "[!]",
        TaskStatus::Cancelled => "[-]",
    }
}

/// The active form while in progress, otherwise the content.
fn display_title(item: &TodoItem) -> String {
    if item.status == TodoStatus::InProgress {
        if let Some(active) = item
            .active_form
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return active.to_owned();
        }
    }
    item.content.trim().to_owned()
}

/// A root task the main agent owns (a plan item).
fn is_main_plan_task(task: &Task) -> bool {
    task.source.is_none() && task.agent.is_none() && task.parent_task_id.is_none()
}

/// `settleDanglingMainTasks`: at request end, settle plan items left
/// `in_progress` (unless delegated work is still running). Returns the count.
pub fn settle_dangling_main_tasks(store: &TaskStore, outcome: TaskStatus) -> usize {
    let all = store.list();
    if all
        .iter()
        .any(|t| !is_main_plan_task(t) && t.status.is_active())
    {
        return 0;
    }
    let dangling: Vec<u64> = all
        .iter()
        .filter(|t| is_main_plan_task(t) && t.status == TaskStatus::InProgress)
        .map(|t| t.id)
        .collect();
    if dangling.is_empty() {
        return 0;
    }
    store.batch(|store| {
        for id in &dangling {
            store.update(
                *id,
                TaskPatch {
                    status: Some(outcome),
                    ..Default::default()
                },
            );
        }
    });
    dangling.len()
}

/// The TypeBox schema hoocode sends for TodoWrite.
pub fn todo_write_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["todos"],
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["content", "status"],
                    "properties": {
                        "content": {"type": "string", "description": "The task, in imperative form (e.g. 'Add tests for the parser')."},
                        "status": {
                            "anyOf": [
                                {"type": "string", "const": "pending"},
                                {"type": "string", "const": "in_progress"},
                                {"type": "string", "const": "completed"}
                            ],
                            "description": "pending = not started, in_progress = actively being worked on, completed = finished."
                        },
                        "activeForm": {"type": "string", "description": "Optional present-tense form shown while the item is in_progress (e.g. 'Adding tests for the parser')."}
                    },
                    "additionalProperties": false
                },
                "description": "The complete todo list. This REPLACES the previous list on every call, so always send every item with its current status — omitting an item removes it."
            }
        },
        "additionalProperties": false
    })
}

fn parse_items(args: &Value) -> Result<Vec<TodoItem>, ToolError> {
    let Some(todos) = args.get("todos") else {
        return Ok(Vec::new());
    };
    let todos = todos.as_array().ok_or("todos must be an array")?;
    todos
        .iter()
        .map(|t| {
            Ok(TodoItem {
                content: t
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                status: t
                    .get("status")
                    .and_then(Value::as_str)
                    .and_then(TodoStatus::parse)
                    .ok_or("todo status must be pending, in_progress or completed")?,
                active_form: t
                    .get("activeForm")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

/// Reconcile the incoming list against the main plan tasks: by content
/// first, then leftover slots in order; drop the rest; keep list order.
fn reconcile(store: &TaskStore, todos: &[TodoItem]) {
    store.batch(|store| {
        let existing: Vec<Task> = store.list().into_iter().filter(is_main_plan_task).collect();
        let content = |item: &TodoItem| item.content.trim().to_owned();
        let mut matched = vec![false; existing.len()];
        let mut assigned: Vec<Option<usize>> = vec![None; todos.len()];
        for (i, item) in todos.iter().enumerate() {
            let wanted = content(item);
            if let Some(j) = existing.iter().enumerate().position(|(j, t)| {
                !matched[j] && t.todo_content.as_deref().unwrap_or(&t.title) == wanted
            }) {
                matched[j] = true;
                assigned[i] = Some(j);
            }
        }
        let leftovers: Vec<usize> = (0..existing.len()).filter(|j| !matched[*j]).collect();
        let mut next_leftover = 0;
        for slot in assigned.iter_mut().filter(|a| a.is_none()) {
            if let Some(&j) = leftovers.get(next_leftover) {
                *slot = Some(j);
                next_leftover += 1;
            }
        }
        let mut final_ids = Vec::with_capacity(todos.len());
        for (item, slot) in todos.iter().zip(&assigned) {
            let patch = TaskPatch {
                title: Some(display_title(item)),
                status: Some(item.status.task_status()),
                todo_content: Some(content(item)),
                ..Default::default()
            };
            match slot {
                Some(j) => {
                    store.update(existing[*j].id, patch);
                    final_ids.push(existing[*j].id);
                }
                None => {
                    let created = store.create(&display_title(item), Default::default());
                    store.update(
                        created.id,
                        TaskPatch {
                            title: None,
                            ..patch
                        },
                    );
                    final_ids.push(created.id);
                }
            }
        }
        for &j in &leftovers[next_leftover.min(leftovers.len())..] {
            store.remove(existing[j].id);
        }
        store.arrange(&final_ids);
    });
}

/// Which task store TodoWrite writes to.
#[derive(Clone)]
pub enum StoreRef {
    /// The process-wide store the task panel renders (hoocode's `taskStore`).
    Global,
    /// A private store (tests, embedders).
    Owned(Arc<TaskStore>),
}

impl StoreRef {
    pub fn get(&self) -> &TaskStore {
        match self {
            Self::Global => cortexcode_code_task_store::task_store(),
            Self::Owned(store) => store,
        }
    }
}

/// `createTodoWriteToolDefinition`.
pub fn create_todo_write_tool_definition(store: StoreRef) -> ToolDefinition {
    ToolDefinition { background_when: None,
        name: TODO_WRITE_TOOL_NAME.into(),
        label: TODO_WRITE_TOOL_NAME.into(),
        description: [
            "Maintain a structured todo list for the current task, shown live in the task panel.",
            "Write the full plan as todos before starting multi-step or non-trivial work, and keep it current; skip only trivial single-step tasks.",
            "Mark exactly ONE item in_progress at a time, and flip an item to completed immediately after finishing it.",
            "Each call sends the FULL list and REPLACES the previous one — include every item with its current status; omitting an item removes it.",
        ]
        .join("\n"),
        prompt_snippet: Some(
            "Plan and track multi-step work as a live todo list (use proactively; replaces the whole list each call)".into(),
        ),
        prompt_guidelines: vec![
            "Use TodoWrite proactively for multi-step or non-trivial work; skip trivial single-step tasks.".into(),
        ],
        parameters: todo_write_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, args, _signal, _on_update, _ctx| {
            let todos = parse_items(&args)?;
            reconcile(store.get(), &todos);
            let (mut pending, mut in_progress, mut completed) = (0, 0, 0);
            for t in &todos {
                match t.status {
                    TodoStatus::InProgress => in_progress += 1,
                    TodoStatus::Completed => completed += 1,
                    TodoStatus::Pending => pending += 1,
                }
            }
            let text = if todos.is_empty() {
                "Todo list cleared.".to_owned()
            } else {
                let lines: Vec<String> = todos
                    .iter()
                    .map(|t| format!("{} {}", glyph(t.status.task_status()), display_title(t)))
                    .collect();
                format!(
                    "Todos updated ({in_progress} in progress, {pending} pending, {completed} completed):\n{}",
                    lines.join("\n")
                )
            };
            Ok(AgentToolResult {
                content: vec![Content::Text(TextContent {
                    text_signature: None,
                    text,
                })],
                details: json!({
                    "total": todos.len(),
                    "pending": pending,
                    "inProgress": in_progress,
                    "completed": completed,
                }),
                terminate: false,
            })
        }),
    }
}

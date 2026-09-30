//! Port of the pin's `suite/regressions/79-stale-todo-settle.test.ts`, store
//! half: `settleDanglingMainTasks` over rows the real TodoWrite tool wrote.
//! The InteractiveMode half (outcome by stop reason, queued messages) is
//! `plan_settle_outcome` in cortexcode-code-tui-app's `tests/stale_todo_settle_ts.rs`.

use std::sync::Arc;

use cortexcode_code_task_store::{CreateTaskOptions, TaskPatch, TaskSource, TaskStatus, TaskStore};
use cortexcode_code_tool_api::ToolDefinition;
use cortexcode_code_tools_optin::*;
use serde_json::{json, Value};

fn setup() -> (Arc<TaskStore>, ToolDefinition) {
    let store = Arc::new(TaskStore::new());
    let tool = create_todo_write_tool_definition(StoreRef::Owned(store.clone()));
    (store, tool)
}

fn write(tool: &ToolDefinition, todos: Value) {
    let _ = (tool.execute)("call-79".into(), json!({"todos": todos}), None, None, None);
}

fn status_of(store: &TaskStore, title: &str) -> Option<TaskStatus> {
    store
        .list()
        .into_iter()
        .find(|t| t.todo_content.as_deref().unwrap_or(&t.title) == title)
        .map(|t| t.status)
}

fn external(
    store: &TaskStore,
    title: &str,
    source: TaskSource,
    mode: &str,
    status: TaskStatus,
) -> u64 {
    let task = store.create(
        title,
        CreateTaskOptions {
            source: Some(source),
            subagent_mode: Some(mode.into()),
            ..Default::default()
        },
    );
    store.update(
        task.id,
        TaskPatch {
            status: Some(status),
            ..Default::default()
        },
    );
    task.id
}

fn status_by_id(store: &TaskStore, id: u64) -> TaskStatus {
    store
        .list()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap()
        .status
}

#[test]
fn settles_the_in_progress_item_the_model_never_completed() {
    let (store, tool) = setup();
    write(
        &tool,
        json!([
            {"content": "Read the config", "status": "completed"},
            {"content": "Patch the parser", "status": "in_progress", "activeForm": "Patching the parser"},
        ]),
    );
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::InProgress)
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 1);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::Done)
    );
    assert_eq!(status_of(&store, "Read the config"), Some(TaskStatus::Done));
}

#[test]
fn leaves_pending_items_alone() {
    let (store, tool) = setup();
    write(
        &tool,
        json!([
            {"content": "Patch the parser", "status": "in_progress"},
            {"content": "Update the docs", "status": "pending"},
        ]),
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 1);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::Done)
    );
    assert_eq!(
        status_of(&store, "Update the docs"),
        Some(TaskStatus::Pending)
    );
}

#[test]
fn is_a_no_op_when_the_model_sent_the_final_call() {
    let (store, tool) = setup();
    write(
        &tool,
        json!([{"content": "Patch the parser", "status": "completed"}]),
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 0);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::Done)
    );
}

#[test]
fn never_touches_subagent_or_mcp_rows() {
    let (store, tool) = setup();
    write(
        &tool,
        json!([{"content": "Patch the parser", "status": "in_progress"}]),
    );
    let mcp = external(
        &store,
        "github: list issues",
        TaskSource::Mcp,
        "github",
        TaskStatus::Done,
    );
    let delegated = external(
        &store,
        "explore the module",
        TaskSource::Subagent,
        "explore",
        TaskStatus::Failed,
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Cancelled), 1);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::Cancelled)
    );
    assert_eq!(status_by_id(&store, mcp), TaskStatus::Done);
    assert_eq!(status_by_id(&store, delegated), TaskStatus::Failed);
}

#[test]
fn holds_off_while_a_delegated_task_is_still_running() {
    let (store, tool) = setup();
    write(
        &tool,
        json!([{"content": "Patch the parser", "status": "in_progress"}]),
    );
    let delegated = external(
        &store,
        "explore the module",
        TaskSource::Subagent,
        "explore",
        TaskStatus::InProgress,
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 0);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::InProgress)
    );
    store.update(
        delegated,
        TaskPatch {
            status: Some(TaskStatus::Done),
            ..Default::default()
        },
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 1);
    assert_eq!(
        status_of(&store, "Patch the parser"),
        Some(TaskStatus::Done)
    );
}

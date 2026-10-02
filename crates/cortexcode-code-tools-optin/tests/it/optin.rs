//! Port of hoocode `test/todo-tool.test.ts` and the autonomous-loop cases of
//! `test/ask-options-loop.test.ts` (v0.5.89; the `/loop` integration case
//! belongs with the loop extension), plus task-store behavior.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::{AbortSignal, Content};
use cortexcode_code_task_store::{TaskPatch, TaskStatus, TaskStore};
use cortexcode_code_tool_api::ToolDefinition;
use cortexcode_code_tools_optin::*;
use serde_json::{json, Value};

fn text(result: &AgentToolResult) -> String {
    match &result.content[0] {
        Content::Text(t) => t.text.clone(),
        _ => unreachable!(),
    }
}

fn store_and_tool() -> (Arc<TaskStore>, ToolDefinition) {
    let store = Arc::new(TaskStore::new());
    let tool = create_todo_write_tool_definition(StoreRef::Owned(store.clone()));
    (store, tool)
}

fn write(tool: &ToolDefinition, todos: Value) -> AgentToolResult {
    (tool.execute)(
        "todo-test".into(),
        json!({"todos": todos}),
        None,
        None,
        None,
    )
    .unwrap()
}

fn main_tasks(store: &TaskStore) -> Vec<(u64, String, TaskStatus)> {
    store
        .list()
        .into_iter()
        .filter(|t| t.owner_id() == "main")
        .map(|t| (t.id, t.title, t.status))
        .collect()
}

fn titles_statuses(store: &TaskStore) -> Vec<(String, TaskStatus)> {
    main_tasks(store)
        .into_iter()
        .map(|(_, t, s)| (t, s))
        .collect()
}

// --- todo-tool.test.ts ---

#[test]
fn creates_main_tasks_and_maps_statuses() {
    let (store, tool) = store_and_tool();
    write(
        &tool,
        json!([
            {"content": "First", "status": "completed"},
            {"content": "Second", "status": "in_progress", "activeForm": "Doing second"},
            {"content": "Third", "status": "pending"}
        ]),
    );
    assert_eq!(
        titles_statuses(&store),
        [
            ("First".to_string(), TaskStatus::Done),
            ("Doing second".to_string(), TaskStatus::InProgress),
            ("Third".to_string(), TaskStatus::Pending)
        ]
    );
}

#[test]
fn replaces_the_list_reconciling_update_add_remove() {
    let (store, tool) = store_and_tool();
    write(
        &tool,
        json!([{"content": "A", "status": "in_progress"}, {"content": "B", "status": "pending"}]),
    );
    let first_ids: Vec<u64> = main_tasks(&store).iter().map(|t| t.0).collect();
    write(
        &tool,
        json!([
            {"content": "A", "status": "completed"},
            {"content": "B", "status": "in_progress"},
            {"content": "C", "status": "pending"}
        ]),
    );
    assert_eq!(
        titles_statuses(&store),
        [
            ("A".to_string(), TaskStatus::Done),
            ("B".to_string(), TaskStatus::InProgress),
            ("C".to_string(), TaskStatus::Pending)
        ]
    );
    assert_eq!(
        main_tasks(&store)[..2]
            .iter()
            .map(|t| t.0)
            .collect::<Vec<_>>(),
        first_ids
    );

    write(&tool, json!([{"content": "A", "status": "completed"}]));
    assert_eq!(
        main_tasks(&store)
            .iter()
            .map(|t| t.1.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
}

#[test]
fn clears_on_an_empty_list_and_reports_counts() {
    let (store, tool) = store_and_tool();
    write(&tool, json!([{"content": "A", "status": "pending"}]));
    let result = write(&tool, json!([]));
    assert!(main_tasks(&store).is_empty());
    assert!(text(&result).contains("cleared"));

    let result = write(
        &tool,
        json!([
            {"content": "A", "status": "in_progress"},
            {"content": "B", "status": "pending"},
            {"content": "C", "status": "completed"}
        ]),
    );
    assert!(text(&result).contains("1 in progress, 1 pending, 1 completed"));
    assert_eq!(
        result.details,
        json!({"total": 3, "pending": 1, "inProgress": 1, "completed": 1})
    );
    assert_eq!(
        text(&result),
        "Todos updated (1 in progress, 1 pending, 1 completed):\n[~] A\n[ ] B\n[x] C"
    );
}

// --- beyond the TS file ---

#[test]
fn reorders_by_identity_keeping_ids_and_leaves_other_rows_alone() {
    let (store, tool) = store_and_tool();
    let delegated = store.create(
        "explore",
        cortexcode_code_task_store::CreateTaskOptions {
            source: Some(cortexcode_code_task_store::TaskSource::Subagent),
            ..Default::default()
        },
    );
    write(
        &tool,
        json!([{"content": "A", "status": "pending"}, {"content": "B", "status": "pending"}]),
    );
    let ids: Vec<(String, u64)> = main_tasks(&store).into_iter().map(|t| (t.1, t.0)).collect();
    write(
        &tool,
        json!([{"content": "B", "status": "pending"}, {"content": "A", "status": "pending"}]),
    );
    let after: Vec<(String, u64)> = main_tasks(&store).into_iter().map(|t| (t.1, t.0)).collect();
    assert_eq!(after, [ids[1].clone(), ids[0].clone()]);
    assert!(store.list().iter().any(|t| t.id == delegated.id));
    // A rename keeps the slot's id.
    write(
        &tool,
        json!([{"content": "B2", "status": "pending"}, {"content": "A", "status": "pending"}]),
    );
    assert_eq!(main_tasks(&store)[0].0, ids[1].1);
}

#[test]
fn settles_dangling_in_progress_items_unless_delegated_work_runs() {
    let (store, tool) = store_and_tool();
    write(
        &tool,
        json!([{"content": "A", "status": "in_progress"}, {"content": "B", "status": "pending"}]),
    );
    let run = store.create(
        "run",
        cortexcode_code_task_store::CreateTaskOptions {
            source: Some(cortexcode_code_task_store::TaskSource::Subagent),
            ..Default::default()
        },
    );
    store.update(
        run.id,
        TaskPatch {
            status: Some(TaskStatus::InProgress),
            ..Default::default()
        },
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 0);
    store.update(
        run.id,
        TaskPatch {
            status: Some(TaskStatus::Done),
            ..Default::default()
        },
    );
    assert_eq!(settle_dangling_main_tasks(&store, TaskStatus::Done), 1);
    assert_eq!(titles_statuses(&store)[0].1, TaskStatus::Done);
    assert_eq!(titles_statuses(&store)[1].1, TaskStatus::Pending);
}

#[test]
fn store_batches_notifications_and_resets_numbering() {
    let store = TaskStore::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let _sub = store.subscribe(move || {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    store.batch(|s| {
        let a = s.create("a", Default::default());
        s.create("b", Default::default());
        s.update(
            a.id,
            TaskPatch {
                status: Some(TaskStatus::Done),
                ..Default::default()
            },
        );
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.version(), 4);
    store.create("  ", Default::default());
    assert_eq!(store.list()[2].title, "(untitled task)");
    store.reset();
    assert_eq!(
        store
            .list()
            .iter()
            .map(|t| t.title.as_str())
            .collect::<Vec<_>>(),
        ["b", "(untitled task)"]
    );
    let b = store.list()[0].id;
    store.arrange(&[store.list()[1].id, b]);
    assert_eq!(store.list()[1].id, b);
    store.update(
        b,
        TaskPatch {
            status: Some(TaskStatus::Done),
            ..Default::default()
        },
    );
    let other = store.list()[0].id;
    store.update(
        other,
        TaskPatch {
            status: Some(TaskStatus::Failed),
            ..Default::default()
        },
    );
    store.reset();
    assert!(store.list().is_empty());
    assert_eq!(store.create("fresh", Default::default()).id, 1);
}

// --- ask-options-loop.test.ts ---

#[derive(Default)]
struct FakeHost {
    loop_active: Mutex<bool>,
    ask_calls: AtomicUsize,
    halts: Mutex<Vec<String>>,
}

impl AskOptionsHost for FakeHost {
    fn has_ui(&self) -> bool {
        true
    }
    fn ask_options(
        &self,
        _: &[AskQuestion],
        _: Option<AbortSignal>,
    ) -> Option<Vec<Option<String>>> {
        self.ask_calls.fetch_add(1, Ordering::SeqCst);
        Some(vec![Some("USER_PICKED".into())])
    }
    fn auto_loop_active(&self) -> bool {
        *self.loop_active.lock().unwrap()
    }
    fn halt_loop(&self, reason: &str) {
        self.halts.lock().unwrap().push(reason.into());
    }
}

fn ask(tool: &ToolDefinition, questions: Value) -> String {
    text(
        &(tool.execute)(
            "id".into(),
            json!({"questions": questions}),
            None,
            None,
            None,
        )
        .unwrap(),
    )
}

#[test]
fn ask_options_blocks_normally_auto_answers_in_a_loop_and_halts_without_defaults() {
    let host = Arc::new(FakeHost::default());
    let tool = create_ask_options_tool_definition(host.clone());
    let which = json!([{"question": "which?", "options": [{"label": "a"}, {"label": "b"}]}]);

    assert!(ask(&tool, which.clone()).contains("USER_PICKED"));
    assert_eq!(host.ask_calls.load(Ordering::SeqCst), 1);

    *host.loop_active.lock().unwrap() = true;
    let answer = ask(
        &tool,
        json!([{"question": "which store?", "options": [{"label": "a"}, {"label": "b", "recommended": true}]}]),
    );
    assert_eq!(host.ask_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        answer,
        "which store?\n  → b (auto-selected recommended default; autonomous loop, no user present)"
    );

    let answer = ask(
        &tool,
        json!([
            {"question": "safe one?", "options": [{"label": "a", "recommended": true}]},
            {"question": "risky one?", "options": [{"label": "x"}, {"label": "y"}]}
        ]),
    );
    assert_eq!(host.ask_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.halts.lock().unwrap().len(), 1);
    assert!(answer.contains("risky one?") && answer.contains("stopped"));

    *host.loop_active.lock().unwrap() = false;
    ask(&tool, which);
    assert_eq!(host.ask_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn ask_options_without_ui_or_questions() {
    let tool = create_ask_options_tool_definition(Arc::new(NoUi));
    assert_eq!(
        ask(&tool, json!([{"question": "q", "options": []}])),
        "Cannot ask the user: no interactive UI is available in this session. Proceed using your best judgement."
    );
    let tool = create_ask_options_tool_definition(Arc::new(FakeHost::default()));
    assert_eq!(ask(&tool, json!([])), "No questions were provided.");
}

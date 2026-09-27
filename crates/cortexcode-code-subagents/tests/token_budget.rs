//! token-budget.test.ts.

use std::sync::{Arc, Mutex};

use cortexcode_code_subagents::token_budget::*;
use serde_json::{json, Value};

fn end(total: u64) -> String {
    format!(
        "{}\n",
        json!({"type": "message_end", "message": {"role": "assistant", "usage": {"totalTokens": total}}})
    )
}

fn budget(limit: Option<u64>) -> (TokenBudget, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let b = TokenBudget::new(
        "t1",
        "explore",
        TokenBudgetOptions {
            limit,
            cwd: Some(dir.path().to_path_buf()),
        },
    );
    (b, dir)
}

#[test]
fn returns_default_budgets_per_agent_type() {
    assert_eq!(get_default_budget("explore"), 35000);
    assert_eq!(get_default_budget("plan"), 35000);
    assert_eq!(get_default_budget("general-purpose"), 60000);
    assert_eq!(get_default_budget("unknown"), 35000);
}

#[test]
fn accumulates_usage_from_message_end_events() {
    let (mut b, _d) = budget(None);
    assert_eq!(b.used(), 0);
    b.process_stdout(&end(100));
    assert_eq!(b.used(), 100);
    b.process_stdout(&end(200));
    assert_eq!(b.used(), 300);
}

#[test]
fn ignores_non_assistant_and_non_message_end_events() {
    let (mut b, _d) = budget(None);
    b.process_stdout(&format!(
        "{}\n",
        json!({"type": "message_end", "message": {"role": "user", "usage": {"totalTokens": 500}}})
    ));
    b.process_stdout(&format!(
        "{}\n",
        json!({"type": "message_update", "message": {"role": "assistant", "usage": {"totalTokens": 500}}})
    ));
    assert_eq!(b.used(), 0);
}

#[test]
fn handles_events_split_across_chunks_and_several_per_chunk() {
    let (mut b, _d) = budget(None);
    let event = end(150);
    b.process_stdout(&event[..20]);
    assert_eq!(b.used(), 0);
    b.process_stdout(&event[20..]);
    assert_eq!(b.used(), 150);
    b.process_stdout(&format!("{}{}", end(50), end(75)));
    assert_eq!(b.used(), 275);
}

#[test]
fn ignores_invalid_json_and_empty_lines() {
    let (mut b, _d) = budget(None);
    b.process_stdout("not json\n");
    b.process_stdout("\n\n");
    b.process_stdout(&end(42));
    assert_eq!(b.used(), 42);
}

#[test]
fn flush_processes_a_trailing_line_without_newline() {
    let (mut b, _d) = budget(None);
    b.process_stdout(end(99).trim_end());
    assert_eq!(b.used(), 0);
    b.flush();
    assert_eq!(b.used(), 99);
}

fn recorder() -> (Arc<Mutex<Vec<Value>>>, BudgetListener) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (
        seen,
        Box::new(move |v: &Value| sink.lock().unwrap().push(v.clone())),
    )
}

#[test]
fn emits_budget_warning_at_80_percent() {
    let (mut b, _d) = budget(Some(1000));
    let (seen, listener) = recorder();
    b.on_warning(listener);
    b.process_stdout(&end(800));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({
            "task_id": "t1",
            "message": "You are near token limit. Summarize and write result.json now.",
            "used": 800,
            "limit": 1000,
        })]
    );
    assert!(b.is_warned());
}

#[test]
fn emits_budget_exceeded_at_100_percent() {
    let (mut b, _d) = budget(Some(500));
    let (seen, listener) = recorder();
    b.on_exceeded(listener);
    b.process_stdout(&end(500));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![json!({"task_id": "t1", "used": 500, "limit": 500})]
    );
    assert!(b.is_exceeded());
}

#[test]
fn warns_and_exceeds_once() {
    let (mut b, _d) = budget(Some(100));
    let (warnings, w) = recorder();
    let (exceeded, e) = recorder();
    b.on_warning(w);
    b.on_exceeded(e);
    b.process_stdout(&end(80));
    b.process_stdout(&end(10));
    b.process_stdout(&end(10));
    b.process_stdout(&end(10));
    assert_eq!(warnings.lock().unwrap().len(), 1);
    assert_eq!(exceeded.lock().unwrap().len(), 1);
}

fn read_state(dir: &std::path::Path, task: &str) -> Value {
    let path = cortexcode_code_paths::dispatch_task_dir(dir, task).join("budget.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn persists_budget_state_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = TokenBudget::new(
        "persist-task",
        "general-purpose",
        TokenBudgetOptions {
            limit: None,
            cwd: Some(dir.path().to_path_buf()),
        },
    );
    b.process_stdout(&end(2500));
    let state = read_state(dir.path(), "persist-task");
    assert_eq!(state["task_id"], "persist-task");
    assert_eq!(state["agent_type"], "general-purpose");
    assert_eq!(state["budget"], 60000);
    assert_eq!(state["used"], 2500);
    assert_eq!(state["warned"], false);
    assert_eq!(state["exceeded"], false);
    assert!(state["last_updated"].as_u64().unwrap() > 0);
}

#[test]
fn updates_the_persisted_file_on_each_usage_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut b = TokenBudget::new(
        "persist-task2",
        "explore",
        TokenBudgetOptions {
            limit: Some(500),
            cwd: Some(dir.path().to_path_buf()),
        },
    );
    b.process_stdout(&end(200));
    let s1 = read_state(dir.path(), "persist-task2");
    assert_eq!(
        (
            s1["used"].clone(),
            s1["warned"].clone(),
            s1["exceeded"].clone()
        ),
        (json!(200), json!(false), json!(false))
    );
    b.process_stdout(&end(250));
    let s2 = read_state(dir.path(), "persist-task2");
    assert_eq!(
        (
            s2["used"].clone(),
            s2["warned"].clone(),
            s2["exceeded"].clone()
        ),
        (json!(450), json!(true), json!(false))
    );
    b.process_stdout(&end(100));
    let s3 = read_state(dir.path(), "persist-task2");
    assert_eq!(
        (
            s3["used"].clone(),
            s3["warned"].clone(),
            s3["exceeded"].clone()
        ),
        (json!(550), json!(true), json!(true))
    );
}

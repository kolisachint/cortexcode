//! token-budget.test.ts.
//!
//! Deviation: the tests build realistic usage blocks (`input`/`output`/
//! `cacheRead`/`cacheWrite`/`totalTokens`) instead of hoocode's `totalTokens`
//! only, because `used` now counts generated tokens rather than context size —
//! see the module docs on `TokenBudget`.

use std::sync::{Arc, Mutex};

use cortexcode_code_subagents::token_budget::*;
use serde_json::{json, Value};

/// One assistant turn: the run's context size, and what this turn generated.
fn turn(context: u64, output: u64) -> String {
    format!(
        "{}\n",
        json!({"type": "message_end", "message": {"role": "assistant", "usage": {
            "input": 0, "output": output, "cacheRead": context.saturating_sub(output),
            "cacheWrite": 0, "totalTokens": context
        }}})
    )
}

/// The 15 assistant turns of `dispatch-1790866463943-0ozuar`
/// (`hoobot/.cortexcode/dispatch`), as (context, output).
///
/// hoocode's accumulation reported 1_654_795 against a 35_000 budget. The turns
/// visible in that run's (256KB-capped) stdout sum to 1_047_206 of context;
/// the generated total is 4_111.
const RECORDED_TURNS: &[(u64, u64)] = &[
    (57780, 153),
    (61360, 178),
    (64704, 96),
    (68500, 496),
    (69678, 207),
    (69808, 79),
    (69949, 76),
    (70103, 92),
    (71554, 212),
    (72332, 207),
    (72529, 77),
    (72670, 78),
    (73189, 77),
    (73999, 190),
    (79051, 1893),
];

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
    b.process_stdout(&turn(100, 40));
    assert_eq!(b.used(), 40);
    b.process_stdout(&turn(200, 60));
    assert_eq!(b.used(), 100);
}

/// `used` counts what the subagent generated, not the size of the context it
/// kept re-reading.
///
/// hoocode summed `usage.totalTokens`, which is the whole conversation resent
/// each turn, so N turns reported ~context·N²/2. Every recorded run blew its
/// budget by 20-47x and reported `"exceeded": true` on all ten.
#[test]
fn does_not_accumulate_the_context_size() {
    let (mut b, _d) = budget(None);
    for (context, output) in RECORDED_TURNS {
        b.process_stdout(&turn(*context, *output));
    }
    // Sum of generated tokens across the recorded run.
    assert_eq!(b.used(), 4_111);
    // What hoocode's accumulation reported for the same turns.
    let quadratic: u64 = RECORDED_TURNS.iter().map(|(context, _)| *context).sum();
    assert_eq!(quadratic, 1_047_206);
    assert!(b.used() < quadratic / 100);
    // And it no longer trips the 35k default budget.
    assert!(!b.is_warned());
    assert!(!b.is_exceeded());
}

/// The context size is still reported — separately, as the peak. It is what a
/// reader of `budget.json` wants to watch, and it is not what the budget is
/// measured against.
#[test]
fn reports_the_peak_context_separately() {
    let (mut b, _d) = budget(None);
    for (context, output) in RECORDED_TURNS {
        b.process_stdout(&turn(*context, *output));
    }
    assert_eq!(b.peak_context(), 79_051);
    // Turn 1 alone is already past a 35k budget, which is why a delta of
    // totalTokens would still fire on turn 1 or 2.
    assert!(RECORDED_TURNS[0].0 > 35_000);
    assert!(!b.is_exceeded());
}

/// A provider that reports a context size but no generated tokens must not
/// re-fire the thresholds, and must not claim budget was consumed.
#[test]
fn a_turn_with_no_output_consumes_nothing() {
    let (mut b, _d) = budget(None);
    let no_output = format!(
        "{}\n",
        json!({"type": "message_end", "message": {"role": "assistant", "usage": {
            "input": 100, "output": 0, "cacheRead": 40_000,
            "cacheWrite": 0, "totalTokens": 40_100
        }}})
    );
    for _ in 0..5 {
        b.process_stdout(&no_output);
    }
    assert_eq!(b.used(), 0);
    assert_eq!(b.peak_context(), 40_100);
    assert!(!b.is_warned());
    assert!(!b.is_exceeded());
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
    let event = turn(150, 150);
    b.process_stdout(&event[..20]);
    assert_eq!(b.used(), 0);
    b.process_stdout(&event[20..]);
    assert_eq!(b.used(), 150);
    b.process_stdout(&format!("{}{}", turn(50, 50), turn(75, 75)));
    assert_eq!(b.used(), 275);
}

#[test]
fn ignores_invalid_json_and_empty_lines() {
    let (mut b, _d) = budget(None);
    b.process_stdout("not json\n");
    b.process_stdout("\n\n");
    b.process_stdout(&turn(42, 42));
    assert_eq!(b.used(), 42);
}

#[test]
fn flush_processes_a_trailing_line_without_newline() {
    let (mut b, _d) = budget(None);
    b.process_stdout(turn(99, 99).trim_end());
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
    b.process_stdout(&turn(800, 800));
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
    b.process_stdout(&turn(500, 500));
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
    b.process_stdout(&turn(80, 80));
    b.process_stdout(&turn(10, 10));
    b.process_stdout(&turn(10, 10));
    b.process_stdout(&turn(10, 10));
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
    b.process_stdout(&turn(2500, 2500));
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
    b.process_stdout(&turn(200, 200));
    let s1 = read_state(dir.path(), "persist-task2");
    assert_eq!(
        (
            s1["used"].clone(),
            s1["warned"].clone(),
            s1["exceeded"].clone()
        ),
        (json!(200), json!(false), json!(false))
    );
    b.process_stdout(&turn(250, 250));
    let s2 = read_state(dir.path(), "persist-task2");
    assert_eq!(
        (
            s2["used"].clone(),
            s2["warned"].clone(),
            s2["exceeded"].clone()
        ),
        (json!(450), json!(true), json!(false))
    );
    b.process_stdout(&turn(100, 100));
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

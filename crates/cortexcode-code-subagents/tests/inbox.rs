//! subagent-inbox.test.ts. The inbox is process-wide, so the cases share one
//! test (serialized) like the TS file's afterEach(clear).

use std::time::Duration;

use cortexcode_code_subagents::inbox::{subagent_inbox, TaskLifecycle};
use cortexcode_code_subagents::pool::{ResultStatus, SubagentResult, TaskResult};
use serde_json::json;

fn ok_result(task_id: &str, summary: &str) -> TaskResult {
    TaskResult {
        task_id: Some(task_id.into()),
        agent_type: Some("explore".into()),
        result: Some(SubagentResult {
            task_id: task_id.into(),
            ok: true,
            exit_code: Some(0),
            status: Some(ResultStatus::Complete),
            result_data: json!({"summary": summary, "files_changed": [], "confidence": 0.9, "status": "complete"})
                .as_object()
                .cloned(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn fail_result(task_id: &str, status: ResultStatus, error: &str) -> TaskResult {
    TaskResult {
        task_id: Some(task_id.into()),
        agent_type: Some("explore".into()),
        result: Some(SubagentResult {
            task_id: task_id.into(),
            exit_code: Some(1),
            status: Some(status),
            error: Some(error.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn subagent_inbox_lifecycle() {
    let inbox = subagent_inbox();

    // allocates monotonic per-agent labels
    inbox.clear();
    assert_eq!(inbox.next_label("explore"), "explore#1");
    assert_eq!(inbox.next_label("explore"), "explore#2");
    assert_eq!(inbox.next_label("plan"), "plan#1");

    // tracks a task from running to done and keeps the body until collected
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    assert_eq!(inbox.get("t1").unwrap().lifecycle, TaskLifecycle::Running);
    assert_eq!(inbox.outstanding().len(), 1);
    inbox.finish(
        "t1",
        &ok_result("t1", "Found the bug in foo.ts:42\nmore detail"),
    );
    let done = inbox.get("t1").unwrap();
    assert_eq!(done.lifecycle, TaskLifecycle::Done);
    assert_eq!(
        done.summary_line.as_deref(),
        Some("Found the bug in foo.ts:42")
    );
    assert!(inbox.outstanding().is_empty());
    let (_, body) = inbox.collect("t1").unwrap();
    assert!(body.contains("more detail"));
    assert_eq!(inbox.get("t1").unwrap().lifecycle, TaskLifecycle::Collected);
    assert!(inbox.collect("t1").is_none());
    assert_eq!(
        inbox.get("t1").unwrap().summary_line.as_deref(),
        Some("Found the bug in foo.ts:42")
    );

    // resolves a handle by task id or by label
    inbox.clear();
    inbox.start("abc123", "plan#1", "plan");
    assert_eq!(inbox.get("abc123").unwrap().label, "plan#1");
    assert_eq!(inbox.get("plan#1").unwrap().task_id, "abc123");
    assert!(inbox.get("nope").is_none());

    // maps failure status onto stalled/timeout/failed
    inbox.clear();
    inbox.start("f1", "explore#1", "explore");
    inbox.finish(
        "f1",
        &fail_result("f1", ResultStatus::Stalled, "no heartbeat"),
    );
    let failed = inbox.get("f1").unwrap();
    assert_eq!(failed.lifecycle, TaskLifecycle::Stalled);
    assert_eq!(failed.error.as_deref(), Some("no heartbeat"));
    assert!(inbox.collect("f1").is_none());

    // settles a task with no TaskResult via fail()
    inbox.clear();
    inbox.start("e1", "explore#1", "explore");
    inbox.fail("e1", "dispatch threw", TaskLifecycle::Failed);
    let e1 = inbox.get("e1").unwrap();
    assert_eq!(e1.lifecycle, TaskLifecycle::Failed);
    assert_eq!(e1.summary_line.as_deref(), Some("dispatch threw"));

    // the wait helpers resolve on settle, or at the timeout
    inbox.clear();
    inbox.start("w1", "explore#1", "explore");
    let waiter = tokio::spawn(async {
        subagent_inbox()
            .wait_for("explore#1", Duration::from_secs(5))
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    inbox.finish("w1", &ok_result("w1", "done"));
    assert_eq!(
        waiter.await.unwrap().unwrap().lifecycle,
        TaskLifecycle::Done
    );
    inbox.start("w2", "explore#2", "explore");
    let started = std::time::Instant::now();
    inbox.wait_for_all(Duration::from_millis(50)).await;
    assert!(started.elapsed() >= Duration::from_millis(50));
    inbox.fail("w2", "gone", TaskLifecycle::Failed);
    inbox.wait_for_all(Duration::from_secs(5)).await;

    // keeps at most 50 settled records, never pruning running ones
    inbox.clear();
    inbox.start("live", "explore#1", "explore");
    for i in 0..60 {
        let id = format!("s{i}");
        inbox.start(&id, &format!("x#{i}"), "explore");
        inbox.fail(&id, "x", TaskLifecycle::Failed);
    }
    inbox.start("last", "explore#2", "explore");
    let listed = inbox.list();
    assert!(listed.iter().any(|r| r.task_id == "live"));
    assert_eq!(
        listed
            .iter()
            .filter(|r| r.lifecycle == TaskLifecycle::Failed)
            .count(),
        50
    );
    inbox.clear();
}

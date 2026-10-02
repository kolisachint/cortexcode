//! The subagents half of the pin's `test/suite/subagent-spawn-audit.test.ts`:
//! lifeguard stall de-duplication, the JSONL reader's bounded buffer, atomic
//! result writes, the cumulative token budget across the inherited-model
//! retry, the process-group kill, and cancellation (pool and Task tool). The
//! roster and task panel cases are in
//! `cortexcode-code-tui-app/tests/subagent_spawn_audit_ts.rs`.
//!
//! Not ported: "survives a stream error without throwing and detaches
//! itself": a Node stream `error` event with no listener; the Rust reader is
//! fed bytes and has no stream to detach from.
#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use cortexcode_ai_types::AbortSignal;
use cortexcode_code_rpc::JsonlLineReader;
use cortexcode_code_subagents::inbox::subagent_inbox;
use cortexcode_code_subagents::instance::set_subagent_pool_for_testing;
use cortexcode_code_subagents::lifeguard::{LifeguardEvent, SubagentLifeguard};
use cortexcode_code_subagents::pool::{
    ResultStatus, SubagentPool, SubagentPoolOptions, SubagentPoolTask, TaskStatus,
};
use cortexcode_code_subagents::result::{write_subagent_result, SubagentResultFile};
use cortexcode_code_subagents::token_budget::{TokenBudget, TokenBudgetOptions};
use cortexcode_code_subagents::tools::create_task_tool_definition;
use cortexcode_code_task_store::{task_store, TaskAgentKind, TaskAgentState, TaskSource};
use cortexcode_code_tool_api::ToolContext;
use serde_json::{json, Value};

const DIR: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

/// One test at a time: the store, inbox and pool slot are process-wide.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn setup() -> tempfile::TempDir {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-spawn-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir);
    });
    tempfile::tempdir().unwrap()
}

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn now_ms() -> u64 {
    cortexcode_code_task_store::now_ms()
}

fn record(pool: &SubagentPool, name: &'static str) -> Arc<Mutex<Vec<Value>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    pool.on(move |event| {
        if event.name == name {
            sink.lock().unwrap().push(event.data.clone());
        }
    });
    seen
}

fn task(id: &str, agent: &str, text: &str) -> SubagentPoolTask {
    SubagentPoolTask {
        task_id: id.into(),
        agent_type: agent.into(),
        task: text.into(),
        ..Default::default()
    }
}

// --- lifeguard stall de-duplication ---------------------------------------------

#[tokio::test]
async fn emits_stalled_once_per_reap_even_when_the_check_ticks_again_before_exit() {
    let dir = setup();
    let guard = SubagentLifeguard::new(dir.path());
    let stalled = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = stalled.clone();
    guard.on_event(move |event| {
        if let LifeguardEvent::Stalled { task_id, .. } = event {
            sink.lock().unwrap().push(task_id.clone());
        }
    });
    let mut child = std::process::Command::new("sleep")
        .arg("10")
        .process_group(0)
        .spawn()
        .unwrap();
    guard.monitor("t1", "explore", child.id());
    guard.set_last_heartbeat_for_testing("t1", now_ms() - 70_000);
    // Three ticks before the exit is observed: one report.
    guard.check_heartbeats();
    guard.check_heartbeats();
    guard.check_heartbeats();
    assert_eq!(*stalled.lock().unwrap(), ["t1"]);
    let _ = child.kill();
    let _ = child.wait();
    guard.dispose();
}

// --- JSONL reader hardening --------------------------------------------------------

#[test]
fn drops_an_oversized_unterminated_line_instead_of_buffering_it_forever() {
    let mut reader = JsonlLineReader::with_max_buffer(16);
    let mut lines = Vec::new();
    lines.extend(reader.push("x".repeat(64).as_bytes()));
    lines.extend(reader.push(b"still the same giant line"));
    lines.extend(reader.push(b"\n"));
    lines.extend(reader.push(b"{\"ok\":true}\n"));
    assert_eq!(lines, ["{\"ok\":true}"]);
}

#[test]
fn flushes_the_final_partial_line_when_the_stream_closes() {
    let mut reader = JsonlLineReader::new();
    let mut lines = reader.push(b"{\"a\":1}\n{\"b\":2}");
    lines.extend(reader.finish());
    assert_eq!(lines, ["{\"a\":1}", "{\"b\":2}"]);
}

#[test]
fn reassembles_multi_byte_characters_split_across_chunks() {
    let mut reader = JsonlLineReader::new();
    let payload = "{\"emoji\":\"🚀\"}\n".as_bytes();
    // Split inside the 4-byte emoji sequence.
    let mid = payload.iter().position(|&b| b == 0x9a).unwrap();
    let mut lines = reader.push(&payload[..mid]);
    lines.extend(reader.push(&payload[mid..]));
    assert_eq!(lines, ["{\"emoji\":\"🚀\"}"]);
}

#[test]
fn feeds_token_budget_per_line() {
    let dir = setup();
    let mut budget = TokenBudget::new(
        "t1",
        "explore",
        TokenBudgetOptions {
            limit: Some(1000),
            cwd: Some(dir.path().to_path_buf()),
        },
    );
    for total in [400, 500] {
        budget.process_line(
            &json!({"type": "message_end", "message": {"role": "assistant", "usage": {
                "input": 0, "output": total, "cacheRead": 0, "cacheWrite": 0, "totalTokens": total
            }}})
            .to_string(),
        );
    }
    assert_eq!(budget.used(), 900);
    assert!(budget.is_warned());
    assert!(!budget.is_exceeded());
}

// --- atomic writes -----------------------------------------------------------------

#[test]
fn write_subagent_result_persists_atomically_under_the_dispatch_dir() {
    let dir = setup();
    let result = |summary: &str| SubagentResultFile {
        summary: summary.into(),
        files_changed: vec![],
        confidence: 0.9,
        status: cortexcode_code_subagents::result::ResultStatus::Complete,
        usage: None,
        task_tree: None,
    };
    write_subagent_result(dir.path(), "task-1", &result("first"));
    write_subagent_result(dir.path(), "task-1", &result("done"));
    let task_dir = dir.path().join(DIR).join("dispatch").join("task-1");
    let entries: Vec<String> = std::fs::read_dir(&task_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    // No temp file left behind, and the last write is what is there.
    assert_eq!(entries, ["result.json"]);
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(task_dir.join("result.json")).unwrap())
            .unwrap();
    assert_eq!(parsed["summary"], "done");
}

// --- pool reliability ---------------------------------------------------------------

#[tokio::test]
async fn keeps_one_cumulative_token_budget_across_the_inherited_model_retry() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path();
    let agents = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("pinned.md"),
        "---\nname: pinned\ndescription: agent with a pinned model.\nmodel: pinned-model\n---\nbody",
    )
    .unwrap();
    // Fails on its pinned model (a quota-style error, after reporting usage)
    // and succeeds on the inherited one; each attempt reports 300 tokens.
    let failed = json!({"summary": "Task failed: usage limit reached for pinned-model.", "files_changed": [], "confidence": 0.5, "status": "failed"});
    let ok = json!({"summary": "recovered on inherited model", "files_changed": [], "confidence": 0.9, "status": "complete"});
    let exe = script(
        cwd,
        "mock-quota-retry.sh",
        &format!(
            r#"tid=; model=; prev=; for a in "$@"; do [ "$prev" = "--task-id" ] && tid=$a; [ "$prev" = "--model" ] && model=$a; prev=$a; done
mkdir -p {DIR}/dispatch/$tid
echo '{{"type":"message_end","message":{{"role":"assistant","usage":{{"input":0,"output":300,"cacheRead":0,"cacheWrite":0,"totalTokens":300}}}}}}'
if [ "$model" = "pinned-model" ]; then printf '%s' '{failed}' > {DIR}/dispatch/$tid/result.json; exit 1; fi
printf '%s' '{ok}' > {DIR}/dispatch/$tid/result.json
exit 0"#
        ),
    );
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    let done = record(&pool, "task_done");
    let exceeded = record(&pool, "budget_exceeded");
    pool.spawn(SubagentPoolTask {
        model: Some("parent-model".into()),
        inherited_model: Some("parent-model".into()),
        token_budget: Some(500),
        ..task("retry-budget", "pinned", "do work")
    })
    .unwrap();
    let result = pool.wait_for("retry-budget").await.unwrap();
    assert!(result.ok, "{result:?}");
    assert_eq!(result.used_inherited_model_fallback, Some(true));
    // 300 from the failed attempt plus 300 from the retry: one budget.
    assert_eq!(done.lock().unwrap()[0]["tokens_generated"], 600);
    // The 500 cap is only crossed cumulatively: its listeners survived.
    let exceeded = exceeded.lock().unwrap();
    assert_eq!(exceeded.len(), 1);
    assert_eq!(exceeded[0]["used"], 600);
    pool.dispose();
}

#[tokio::test]
async fn a_stalled_run_retries_on_the_inherited_model_when_the_model_is_the_cause() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path();
    let agents = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("pinned.md"),
        "---\nname: pinned\ndescription: agent with a pinned model.\nmodel: pinned-model\n---\nbody",
    )
    .unwrap();
    // First attempt: hangs, and its stderr names a provider error — the model
    // being unreachable is the reason it never got going. Second attempt
    // (inherited model) succeeds.
    let ok = json!({"summary": "recovered on inherited model", "files_changed": [], "confidence": 0.9, "status": "complete"});
    let exe = script(
        cwd,
        "mock-stall-retry.sh",
        &format!(
            r#"tid=; model=; prev=; for a in "$@"; do [ "$prev" = "--task-id" ] && tid=$a; [ "$prev" = "--model" ] && model=$a; prev=$a; done
mkdir -p {DIR}/dispatch/$tid
if [ "$model" = "pinned-model" ]; then
  echo '400 Upstream request failed: This Go model requires Global regions.' >&2
  sleep 0.2
  exit 1
fi
printf '%s' '{ok}' > {DIR}/dispatch/$tid/result.json
exit 0"#
        ),
    );
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    pool.spawn(SubagentPoolTask {
        model: Some("parent-model".into()),
        inherited_model: Some("parent-model".into()),
        ..task("stall-retry", "pinned", "do work")
    })
    .unwrap();
    // Reap the first attempt as stalled once it is up.
    for _ in 0..100 {
        if pool.running_count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    pool.lifeguard()
        .inject_event_for_testing(LifeguardEvent::Stalled {
            task_id: "stall-retry".into(),
            pid: 0,
        });
    let result = pool.wait_for("stall-retry").await.unwrap();
    assert!(result.ok, "{result:?}");
    assert_eq!(result.used_inherited_model_fallback, Some(true));
    assert_eq!(result.status, Some(ResultStatus::Complete));
    pool.dispose();
}

/// A user cancellation is never retried on another model, whatever the child's
/// stderr says: they asked for it to stop.
#[tokio::test]
async fn a_cancelled_run_is_never_retried_on_another_model() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path();
    let agents = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("pinned.md"),
        "---\nname: pinned\ndescription: agent with a pinned model.\nmodel: pinned-model\n---\nbody",
    )
    .unwrap();
    let exe = script(
        cwd,
        "mock-cancel.sh",
        "echo '429 Go usage limit exceeded' >&2\nsleep 0.2",
    );
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    pool.spawn(SubagentPoolTask {
        model: Some("parent-model".into()),
        inherited_model: Some("parent-model".into()),
        ..task("cancel-once", "pinned", "do work")
    })
    .unwrap();
    for _ in 0..100 {
        if pool.running_count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(pool.cancel("cancel-once"));
    let result = pool.wait_for("cancel-once").await.unwrap();
    assert!(!result.ok);
    assert_eq!(result.status, Some(ResultStatus::Cancelled));
    assert_eq!(result.used_inherited_model_fallback, Some(false));
    // Nothing was requeued behind it.
    assert_eq!(pool.queued_count(), 0);
    pool.dispose();
}

/// A killed run settles with the status alone, exactly as hoocode does, so the
/// parent still reports `subagent <status>`. The diagnostic value lives in
/// `output.json` instead.
#[tokio::test]
async fn a_killed_run_reports_the_status_not_a_prose_cause() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path();
    let exe = script(cwd, "mock-silent.sh", "sleep 0.2");
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    pool.spawn(task("why", "explore", "work")).unwrap();
    for _ in 0..100 {
        if pool.running_count() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    pool.lifeguard()
        .inject_event_for_testing(LifeguardEvent::Stalled {
            task_id: "why".into(),
            pid: 0,
        });
    let result = pool.wait_for("why").await.unwrap();
    assert!(!result.ok);
    assert_eq!(result.status, Some(ResultStatus::Stalled));
    // hoocode leaves `error` unset on the kill path; the parent derives
    // "subagent stalled" from the status. Keep it that way.
    assert_eq!(result.error, None);
    pool.dispose();
}

#[tokio::test]
async fn dispose_kills_a_subagents_grandchildren_via_the_process_group() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path();
    let exe = script(
        cwd,
        "mock-nested.sh",
        "sleep 30 &\necho $! > grandchild.pid\nsleep 30",
    );
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    pool.spawn(task("nested", "explore", "spawn a grandchild"))
        .unwrap();
    let pid_file = cwd.join("grandchild.pid");
    for _ in 0..100 {
        if std::fs::read_to_string(&pid_file).is_ok_and(|s| !s.trim().is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    assert!(alive(pid));

    pool.dispose();

    let mut dead = false;
    for _ in 0..100 {
        if !alive(pid) {
            dead = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(dead, "grandchild {pid} survived the pool");
}

// --- cancellation ------------------------------------------------------------------

fn sleeping_pool(cwd: &Path) -> SubagentPool {
    SubagentPool::new(SubagentPoolOptions {
        executable: script(cwd, "mock-sleep.sh", "sleep 30"),
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    })
}

#[tokio::test]
async fn cancel_on_a_running_child_settles_it_as_cancelled() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let pool = sleeping_pool(dir.path());
    let cancelled = record(&pool, "task_cancelled");
    pool.spawn(task("c1", "explore", "work")).unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(pool.cancel("c1"));
    let result = pool.wait_for("c1").await.unwrap();
    assert!(!result.ok);
    assert_eq!(result.status, Some(ResultStatus::Cancelled));
    assert_eq!(pool.get_status("c1"), TaskStatus::Cancelled);
    let ids: Vec<Value> = cancelled
        .lock()
        .unwrap()
        .iter()
        .map(|d| d["task_id"].clone())
        .collect();
    assert_eq!(ids, [json!("c1")]);
    assert_eq!(pool.running_count(), 0);
    pool.dispose();
}

#[tokio::test]
async fn cancel_on_a_queued_task_settles_it_immediately_without_spawning() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let pool = sleeping_pool(dir.path());
    pool.spawn(task("blocker", "explore", "block")).unwrap();
    pool.spawn(task("queued", "explore", "wait")).unwrap();
    assert_eq!(pool.queued_count(), 1);
    assert!(pool.cancel("queued"));
    assert_eq!(pool.queued_count(), 0);
    let result = pool.wait_for("queued").await.unwrap();
    assert_eq!(result.status, Some(ResultStatus::Cancelled));
    assert_eq!(pool.get_status("queued"), TaskStatus::Cancelled);
    // Settled and unknown ids report false.
    assert!(!pool.cancel("queued"));
    assert!(!pool.cancel("nope"));
    pool.dispose();
}

#[tokio::test(flavor = "multi_thread")]
async fn task_tool_abort_marks_the_run_cancelled_across_store_roster_and_inbox() {
    let _serial = SERIAL.lock().await;
    let dir = setup();
    let cwd = dir.path().canonicalize().unwrap();
    task_store().clear();
    subagent_inbox().clear();
    let pool = sleeping_pool(&cwd);
    set_subagent_pool_for_testing(Some(pool.clone()));
    let tool = create_task_tool_definition(&cwd);
    let signal = AbortSignal::new();
    let abort = signal.clone();
    // general-purpose runs in the foreground, so the abort settles the call.
    let call = tokio::task::spawn_blocking(move || {
        let ctx = ToolContext {
            cwd: Some(cwd.clone()),
            ..Default::default()
        };
        (tool.execute)(
            "c1".into(),
            json!({"description": "long job", "prompt": "do the long job", "subagent_type": "general-purpose"}),
            Some(signal),
            None,
            Some(&ctx),
        )
        .map_err(|e| e.to_string())
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    abort.abort();
    let err = call.await.unwrap().unwrap_err();
    assert!(err.contains("cancelled by user"), "{err}");
    let task = task_store()
        .list()
        .into_iter()
        .find(|t| t.source == Some(TaskSource::Subagent))
        .unwrap();
    assert_eq!(
        task.status,
        cortexcode_code_task_store::TaskStatus::Cancelled
    );
    let row = task_store()
        .agents()
        .into_iter()
        .find(|a| a.kind == TaskAgentKind::Subagent)
        .unwrap();
    assert_eq!(row.state, Some(TaskAgentState::Cancelled));
    set_subagent_pool_for_testing(None);
    pool.dispose();
}

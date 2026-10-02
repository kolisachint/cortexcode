//! subagent-pool.test.ts, subagent-pool-protocol.test.ts,
//! subagent-pool-registry.test.ts, subagent-pool-inherited-model.test.ts and
//! the pool half of subagent-claude-agent.test.ts.
//!
//! The mock children are shell scripts (hoocode's are Node scripts); argv is
//! recorded NUL-separated.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use cortexcode_code_resources::{AgentDefinition, AgentRegistry, AgentSource};
use cortexcode_code_settings::ModelCategories;
use cortexcode_code_subagents::model_categories::CategorySettings;
use cortexcode_code_subagents::pool::*;
use serde_json::{json, Value};

const DIR: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

/// Keep the agent registry's user dirs out of the real home.
fn isolate_agent_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-pool-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir);
    });
}

fn setup() -> tempfile::TempDir {
    isolate_agent_dir();
    tempfile::tempdir().unwrap()
}

/// Sets `tid` to the `--task-id` argument.
const TASK_ID: &str =
    r#"tid=unknown; prev=; for a in "$@"; do [ "$prev" = "--task-id" ] && tid=$a; prev=$a; done"#;
const DONE_LINE: &str = r#"{"type":"done","reason":"stop","message":{"role":"assistant","content":[{"type":"text","text":"completed"}]}}"#;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn mock(dir: &Path, exit_code: i32, delay_ms: u64) -> PathBuf {
    let run = if exit_code == 0 {
        format!("echo '{DONE_LINE}'")
    } else {
        "echo 'mock error' >&2".to_string()
    };
    script(
        dir,
        "mock.sh",
        &format!(
            "sleep {}\necho '{{\"type\":\"start\",\"partial\":{{}}}}'\n{run}\nexit {exit_code}",
            delay_ms as f64 / 1000.0
        ),
    )
}

fn result_json(summary: &str, status: &str, confidence: f64) -> String {
    json!({"summary": summary, "files_changed": [], "confidence": confidence, "status": status})
        .to_string()
}

/// Records argv, writes a valid result.json for its task id, exits 0.
fn result_writer(dir: &Path, delay_ms: u64) -> PathBuf {
    script(
        dir,
        "mock-result.sh",
        &format!(
            "printf '%s\\0' \"$@\" > argv.bin\n{TASK_ID}\nsleep {}\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{}' > {DIR}/dispatch/$tid/result.json\necho '{DONE_LINE}'\nexit 0",
            delay_ms as f64 / 1000.0,
            result_json("background done", "complete", 0.9)
        ),
    )
}

fn argv_recorder(dir: &Path) -> PathBuf {
    script(
        dir,
        "mock-argv.sh",
        &format!("printf '%s\\0' \"$@\" > argv.bin\necho '{DONE_LINE}'\nexit 0"),
    )
}

fn read_argv(dir: &Path) -> Vec<String> {
    read_argv_file(&dir.join("argv.bin"))
}

fn read_argv_file(path: &Path) -> Vec<String> {
    let bytes = std::fs::read(path).unwrap();
    let mut argv: Vec<String> = bytes
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    argv.pop();
    argv
}

fn arg_after(argv: &[String], flag: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == flag)?;
    argv.get(i + 1).cloned()
}

fn write_valid_result(cwd: &Path, task_id: &str) {
    let dir = cortexcode_code_paths::dispatch_task_dir(cwd, task_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("result.json"),
        json!({"summary": "All files updated successfully", "files_changed": ["src/foo.ts"], "confidence": 0.95, "status": "complete"}).to_string(),
    )
    .unwrap();
}

fn pool(exe: PathBuf, max: usize, cwd: &Path) -> SubagentPool {
    SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        max_concurrency: Some(max),
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    })
}

fn task(id: &str, agent: &str, text: &str) -> SubagentPoolTask {
    SubagentPoolTask {
        task_id: id.into(),
        agent_type: agent.into(),
        task: text.into(),
        ..Default::default()
    }
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

fn env_with(pairs: &[(&str, &str)], remove: &[&str]) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for key in remove {
        for prefix in cortexcode_code_paths::ENV_PREFIXES {
            env.remove(&format!("{prefix}{key}"));
        }
    }
    for (k, v) in pairs {
        env.insert(k.to_string(), v.to_string());
    }
    env
}

#[tokio::test]
async fn spawns_a_subagent_and_waits_for_the_result() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 0), 2, dir.path());
    write_valid_result(dir.path(), "t1");
    p.spawn(task("t1", "explore", "hello")).unwrap();
    let result = p.wait_for("t1").await.unwrap();
    assert_eq!(result.task_id, "t1");
    assert!(result.ok);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout.contains("completed"));
    assert_eq!(p.running_count(), 0);
    assert_eq!(p.queued_count(), 0);
    p.dispose();
}

#[tokio::test]
async fn respects_max_concurrency() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 50), 2, dir.path());
    for id in ["t1", "t2", "t3"] {
        write_valid_result(dir.path(), id);
        p.spawn(task(id, "explore", id)).unwrap();
    }
    assert_eq!(p.running_count(), 2);
    assert_eq!(p.queued_count(), 1);
    for id in ["t1", "t2", "t3"] {
        p.wait_for(id).await.unwrap();
    }
    assert_eq!(p.running_count(), 0);
    assert_eq!(p.queued_count(), 0);
    p.dispose();
}

#[tokio::test]
async fn priority_ordering_explore_before_others_when_queued() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 50), 1, dir.path());
    let order = Arc::new(Mutex::new(Vec::new()));
    let sink = order.clone();
    p.on(move |event| {
        if event.name == "task_done" {
            sink.lock()
                .unwrap()
                .push(event.data["task_id"].as_str().unwrap().to_string());
        }
    });
    write_valid_result(dir.path(), "blocker");
    p.spawn(task("blocker", "edit", "block")).unwrap();
    for (id, agent) in [("t1", "doc"), ("t2", "explore"), ("t3", "edit")] {
        write_valid_result(dir.path(), id);
        p.spawn(task(id, agent, "x")).unwrap();
    }
    for id in ["blocker", "t2", "t3", "t1"] {
        p.wait_for(id).await.unwrap();
    }
    // The TS test records its own await order (always blocker, t2, t3, t1).
    // By priorityOf, explore runs first and doc/edit tie (FIFO), so the real
    // completion order is blocker, t2, t1, t3.
    assert_eq!(*order.lock().unwrap(), vec!["blocker", "t2", "t1", "t3"]);
    p.dispose();
}

#[tokio::test]
async fn emits_task_failed_after_retry_on_persistent_spawn_failure() {
    let dir = setup();
    let p = pool(dir.path().join("nonexistent-binary"), 2, dir.path());
    let failed = record(&p, "task_failed");
    p.spawn(task("t1", "explore", "hello")).unwrap();
    let result = p.wait_for("t1").await.unwrap();
    assert!(!result.ok);
    assert!(result.error.is_some());
    let failed = failed.lock().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["task_id"], "t1");
    p.dispose();
}

#[tokio::test]
async fn duplicate_task_id_is_an_error() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 0), 2, dir.path());
    write_valid_result(dir.path(), "t1");
    p.spawn(task("t1", "explore", "hello")).unwrap();
    assert_eq!(
        p.spawn(task("t1", "explore", "hello")).unwrap_err().0,
        "Duplicate task_id: t1"
    );
    p.dispose();
}

#[tokio::test]
async fn dispose_kills_running_processes_and_rejects_waiters() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 5000), 2, dir.path());
    p.spawn(task("t1", "explore", "hello")).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(p.running_count(), 1);
    let waiter = {
        let p = p.clone();
        tokio::spawn(async move { p.wait_for("t1").await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    p.dispose();
    assert_eq!(
        waiter.await.unwrap().unwrap_err().0,
        "SubagentPool disposed"
    );
    assert_eq!(p.running_count(), 0);
    assert_eq!(p.queued_count(), 0);
}

#[tokio::test]
async fn tracks_slot_metadata() {
    let dir = setup();
    let p = SubagentPool::new(SubagentPoolOptions {
        executable: mock(dir.path(), 0, 200),
        max_concurrency: Some(1),
        default_token_budget: Some(1000),
        cwd: Some(dir.path().to_path_buf()),
        ..Default::default()
    });
    write_valid_result(dir.path(), "t1");
    p.spawn(SubagentPoolTask {
        token_budget: Some(500),
        ..task("t1", "explore", "hello")
    })
    .unwrap();
    let slot = p.slot("t1").unwrap();
    assert_eq!(slot.agent_type, "explore");
    assert_eq!(slot.token_budget, 500);
    assert!(slot.pid > 0);
    assert!(p.wait_for("t1").await.unwrap().ok);
    p.dispose();
}

#[tokio::test]
async fn fails_and_emits_task_failed_when_output_verification_fails() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 0), 2, dir.path());
    let failed = record(&p, "task_failed");
    p.spawn(task("bad-output", "explore", "hello")).unwrap();
    let result = p.wait_for("bad-output").await.unwrap();
    assert!(!result.ok);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("result.json not found"));
    assert_eq!(result.exit_code, Some(0));
    let failed = failed.lock().unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["error"]
        .as_str()
        .unwrap()
        .contains("result.json not found"));
    // The dispatch dir stays for debugging, with output.json.
    assert!(
        cortexcode_code_paths::dispatch_task_dir(dir.path(), "bad-output")
            .join("output.json")
            .exists()
    );
    p.dispose();
}

#[tokio::test]
async fn emits_task_done_and_removes_the_dispatch_dir_on_success() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 0), 2, dir.path());
    let done = record(&p, "task_done");
    write_valid_result(dir.path(), "t1");
    let dispatch_dir = cortexcode_code_paths::dispatch_task_dir(dir.path(), "t1");
    assert!(dispatch_dir.exists());
    p.spawn(task("t1", "explore", "hello")).unwrap();
    let result = p.wait_for("t1").await.unwrap();
    assert!(result.ok);
    assert_eq!(result.result_data.as_ref().unwrap()["status"], "complete");
    assert!(!dispatch_dir.exists());
    let done = done.lock().unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["task_id"], "t1");
    assert_eq!(done[0]["status"], "complete");
    assert_eq!(p.get_status("t1"), TaskStatus::Done);
    p.dispose();
}

#[tokio::test]
async fn get_status_reports_running_queued_failed_and_unknown() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 5000), 1, dir.path());
    p.spawn(task("t1", "explore", "hello")).unwrap();
    p.spawn(task("t2", "explore", "world")).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(p.get_status("t1"), TaskStatus::Running);
    assert_eq!(p.get_status("t2"), TaskStatus::Queued);
    for id in ["dispatch-0000-abc", "1", "explore-1"] {
        assert_eq!(p.get_status(id), TaskStatus::Unknown);
    }
    p.dispose();

    let dir = setup();
    let p = pool(mock(dir.path(), 1, 0), 2, dir.path());
    p.spawn(task("t1", "explore", "hello")).unwrap();
    p.wait_for("t1").await.unwrap();
    assert_eq!(p.get_status("t1"), TaskStatus::Failed);
    p.dispose();
}

#[tokio::test]
async fn surfaces_the_child_failure_reason_from_result_json() {
    let dir = setup();
    let reason = "Task failed: Anthropic usage limit reached. Please try again later.";
    let exe = script(
        dir.path(),
        "mock-failing.sh",
        &format!(
            "{TASK_ID}\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{}' > {DIR}/dispatch/$tid/result.json\nexit 1",
            result_json(reason, "failed", 0.5)
        ),
    );
    let p = pool(exe, 1, dir.path());
    p.spawn(task("quota", "general-purpose", "do work"))
        .unwrap();
    let result = p.wait_for("quota").await.unwrap();
    assert!(!result.ok);
    assert!(result
        .error
        .as_deref()
        .unwrap()
        .contains("usage limit reached"));
    assert_eq!(result.result_data.unwrap()["status"], "failed");
    p.dispose();
}

#[tokio::test]
async fn falls_back_to_the_stderr_tail_without_result_json() {
    let dir = setup();
    let p = pool(mock(dir.path(), 1, 0), 1, dir.path());
    p.spawn(task("stderr-only", "explore", "hello")).unwrap();
    let result = p.wait_for("stderr-only").await.unwrap();
    assert!(!result.ok);
    assert_eq!(result.error.as_deref(), Some("mock error"));
    p.dispose();
}

#[tokio::test]
async fn passes_a_default_max_turns_cap_and_a_persisted_session() {
    let dir = setup();
    let p = pool(argv_recorder(dir.path()), 1, dir.path());
    write_valid_result(dir.path(), "mt-task");
    p.spawn(task("mt-task", "explore", "scan")).unwrap();
    p.wait_for("mt-task").await.unwrap();
    let argv = read_argv(dir.path());
    assert_eq!(
        arg_after(&argv, "--max-turns"),
        Some(DEFAULT_SUBAGENT_MAX_TURNS.to_string())
    );
    assert_eq!(
        arg_after(&argv, "--session").map(PathBuf::from),
        Some(cortexcode_code_paths::dispatch_task_dir(dir.path(), "mt-task").join("session.jsonl"))
    );
    assert!(!argv.contains(&"--no-session".to_string()));
    assert_eq!(&argv[..2], ["--mode", "json"]);
    assert_eq!(argv.last().map(String::as_str), Some("Task: scan"));
    p.dispose();
}

fn env_capture(dir: &Path) -> PathBuf {
    script(
        dir,
        "mock-env.sh",
        &format!(
            "printf '%s %s' \"${{CORTEXCODE_SUBAGENT_DEPTH-null}}\" \"${{HOOCODE_SUBAGENT_MAX_DEPTH-null}}\" > env.txt\necho '{DONE_LINE}'"
        ),
    )
}

#[tokio::test]
async fn stamps_depth_1_on_a_child_and_propagates_the_cap() {
    let dir = setup();
    let p = SubagentPool::new(SubagentPoolOptions {
        executable: env_capture(dir.path()),
        max_concurrency: Some(1),
        cwd: Some(dir.path().to_path_buf()),
        env: Some(env_with(
            &[("HOOCODE_SUBAGENT_MAX_DEPTH", "2")],
            &["SUBAGENT_DEPTH", "SUBAGENT_MAX_DEPTH"],
        )),
        ..Default::default()
    });
    write_valid_result(dir.path(), "d-root");
    p.spawn(task("d-root", "explore", "scan")).unwrap();
    p.wait_for("d-root").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("env.txt")).unwrap(),
        "1 2"
    );
    p.dispose();
}

#[tokio::test]
async fn increments_depth_across_nesting_levels() {
    let dir = setup();
    let p = SubagentPool::new(SubagentPoolOptions {
        executable: env_capture(dir.path()),
        max_concurrency: Some(1),
        cwd: Some(dir.path().to_path_buf()),
        env: Some(env_with(
            &[
                ("HOOCODE_SUBAGENT_DEPTH", "1"),
                ("HOOCODE_SUBAGENT_MAX_DEPTH", "2"),
            ],
            &["SUBAGENT_DEPTH", "SUBAGENT_MAX_DEPTH"],
        )),
        ..Default::default()
    });
    write_valid_result(dir.path(), "d-nested");
    p.spawn(task("d-nested", "explore", "scan")).unwrap();
    p.wait_for("d-nested").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("env.txt")).unwrap(),
        "2 2"
    );
    p.dispose();
}

fn write_project_agent(cwd: &Path, name: &str, content: &str) {
    let dir = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.md")), content).unwrap();
}

const ORCHESTRATOR: &str = "---\nname: orchestrator\ndescription: Breaks work into subtasks and delegates each.\ntools: read, SearchCodebase\ndelegate: true\n---\nDelegate subtasks via the Task tool.\n";

async fn run_with_cap(cwd: &Path, agent: &str, max_depth: &str) -> Vec<String> {
    let p = SubagentPool::new(SubagentPoolOptions {
        executable: argv_recorder(cwd),
        max_concurrency: Some(1),
        cwd: Some(cwd.to_path_buf()),
        env: Some(env_with(
            &[("HOOCODE_SUBAGENT_MAX_DEPTH", max_depth)],
            &["SUBAGENT_DEPTH", "SUBAGENT_MAX_DEPTH"],
        )),
        ..Default::default()
    });
    write_valid_result(cwd, "a-task");
    p.spawn(task("a-task", agent, "do a multi-part job"))
        .unwrap();
    p.wait_for("a-task").await.unwrap();
    p.dispose();
    read_argv(cwd)
}

#[tokio::test]
async fn a_delegate_agent_gets_task_tools_when_nesting_is_permitted() {
    let dir = setup();
    write_project_agent(dir.path(), "orchestrator", ORCHESTRATOR);
    let argv = run_with_cap(dir.path(), "orchestrator", "2").await;
    assert!(argv.contains(&"--enable-subagents".to_string()));
    let tools = arg_after(&argv, "--tools").unwrap();
    assert_eq!(tools, "read,SearchCodebase,Task,TaskOutput");
}

#[tokio::test]
async fn a_delegate_agent_gets_no_task_tools_at_the_default_cap() {
    let dir = setup();
    write_project_agent(dir.path(), "orchestrator", ORCHESTRATOR);
    let argv = run_with_cap(dir.path(), "orchestrator", "1").await;
    assert!(!argv.contains(&"--enable-subagents".to_string()));
    assert_eq!(arg_after(&argv, "--tools").unwrap(), "read,SearchCodebase");
}

#[tokio::test]
async fn forwards_a_scoped_delegate_list_as_delegate_allow() {
    let dir = setup();
    write_project_agent(
        dir.path(),
        "scoped",
        "---\nname: scoped\ndescription: delegates only to explore.\ntools: read, SearchCodebase\ndelegate: explore\n---\nbody",
    );
    let argv = run_with_cap(dir.path(), "scoped", "2").await;
    assert!(argv.contains(&"--enable-subagents".to_string()));
    assert_eq!(
        arg_after(&argv, "--delegate-allow").as_deref(),
        Some("explore")
    );
}

#[tokio::test]
async fn forwards_an_agents_disallowed_tools() {
    let dir = setup();
    write_project_agent(
        dir.path(),
        "limited",
        "---\nname: limited\ndescription: restricted agent.\ntools: read, SearchCodebase, bash\ndisallowedTools: bash\n---\nbody",
    );
    let argv = run_with_cap(dir.path(), "limited", "1").await;
    assert_eq!(
        arg_after(&argv, "--disallowed-tools").as_deref(),
        Some("bash")
    );
}

#[tokio::test]
async fn dispatch_detached_returns_a_handle_and_the_result_is_collectable() {
    let dir = setup();
    let p = pool(result_writer(dir.path(), 150), 1, dir.path());
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = Mutex::new(Some(tx));
    p.on(move |event| {
        if event.name == "task_done" {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(event.data["task_id"].as_str().unwrap().to_string());
            }
        }
    });
    let dispatched = p
        .dispatch_detached(
            "do background work",
            DispatchOptions {
                force_agent: Some("explore".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!dispatched.handled_inline);
    let id = dispatched.task_id.unwrap();
    assert!(id.starts_with("dispatch-"));
    assert!(matches!(
        p.get_status(&id),
        TaskStatus::Running | TaskStatus::Queued
    ));
    assert!(p.collect(&id).is_none());
    assert_eq!(rx.await.unwrap(), id);
    let result = p.collect(&id).unwrap();
    assert!(result.ok);
    assert_eq!(result.result_data.unwrap()["summary"], "background done");
    assert!(p.collect(&id).is_some());
    assert_eq!(p.get_status(&id), TaskStatus::Done);
    p.dispose();
}

#[tokio::test]
async fn resume_continues_the_original_session_file() {
    let dir = setup();
    let p = pool(result_writer(dir.path(), 0), 1, dir.path());
    let original = cortexcode_code_paths::dispatch_task_dir(dir.path(), "orig-task");
    std::fs::create_dir_all(&original).unwrap();
    let session = original.join("session.jsonl");
    std::fs::write(&session, "{\"type\":\"session\"}\n").unwrap();
    std::fs::write(
        original.join("dispatch-log.json"),
        r#"{"agent_type":"explore"}"#,
    )
    .unwrap();
    let resumed = p
        .resume(
            "orig-task",
            "follow-up instruction",
            DispatchOptions::default(),
        )
        .await
        .unwrap();
    assert!(!resumed.handled_inline);
    assert_eq!(resumed.agent_type.as_deref(), Some("explore"));
    let argv = read_argv(dir.path());
    assert_eq!(
        arg_after(&argv, "--session").map(PathBuf::from),
        Some(session)
    );
    assert_ne!(arg_after(&argv, "--task-id").as_deref(), Some("orig-task"));
    p.dispose();
}

#[tokio::test]
async fn resume_rejects_without_a_persisted_session() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 0), 1, dir.path());
    let err = p
        .resume("missing", "go", DispatchOptions::default())
        .await
        .unwrap_err();
    assert!(err.0.starts_with("No resumable session"), "{err}");
    p.dispose();
}

#[tokio::test]
async fn an_exceeded_token_budget_is_advisory() {
    let dir = setup();
    let exe = script(
        dir.path(),
        "mock-buster.sh",
        r#"echo '{"type":"message_end","message":{"role":"assistant","usage":{"totalTokens":1000}}}'"#,
    );
    let p = pool(exe, 1, dir.path());
    let exceeded = record(&p, "budget_exceeded");
    write_valid_result(dir.path(), "budget-task");
    p.spawn(SubagentPoolTask {
        token_budget: Some(500),
        ..task("budget-task", "explore", "big task")
    })
    .unwrap();
    let result = p.wait_for("budget-task").await.unwrap();
    assert!(result.ok);
    assert_eq!(result.status, Some(ResultStatus::Complete));
    assert_eq!(result.budget_exceeded, Some(true));
    assert_eq!(result.error, None);
    let exceeded = exceeded.lock().unwrap();
    assert_eq!(exceeded.len(), 1);
    assert_eq!(exceeded[0]["task_id"], "budget-task");
    assert_eq!(exceeded[0]["limit"], 500);
    p.dispose();
}

fn inject_stall(p: &SubagentPool, task_id: &str) {
    p.lifeguard().inject_event_for_testing(
        cortexcode_code_subagents::lifeguard::LifeguardEvent::Stalled {
            task_id: task_id.into(),
            pid: 0,
        },
    );
}

#[tokio::test]
async fn a_late_stall_does_not_clobber_a_clean_completion() {
    let dir = setup();
    let p = pool(result_writer(dir.path(), 150), 1, dir.path());
    p.spawn(task("race", "explore", "work")).unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    inject_stall(&p, "race");
    let result = p.wait_for("race").await.unwrap();
    assert!(result.ok);
    assert_eq!(result.status, Some(ResultStatus::Complete));
    assert_eq!(result.result_data.unwrap()["summary"], "background done");
    assert_eq!(p.get_status("race"), TaskStatus::Done);
    assert!(!cortexcode_code_paths::dispatch_task_dir(dir.path(), "race").exists());
    p.dispose();
}

#[tokio::test]
async fn a_genuine_stall_reports_stalled() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 150), 1, dir.path());
    let stalled = record(&p, "task_stalled");
    p.spawn(task("hung", "explore", "work")).unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    inject_stall(&p, "hung");
    let result = p.wait_for("hung").await.unwrap();
    assert!(!result.ok);
    assert_eq!(result.status, Some(ResultStatus::Stalled));
    assert_eq!(p.get_status("hung"), TaskStatus::Stalled);
    assert!(stalled
        .lock()
        .unwrap()
        .iter()
        .any(|e| e["task_id"] == "hung"));
    p.dispose();
}

#[tokio::test]
async fn cancel_settles_queued_and_running_tasks() {
    let dir = setup();
    let p = pool(mock(dir.path(), 0, 5000), 1, dir.path());
    let cancelled = record(&p, "task_cancelled");
    p.spawn(task("run", "explore", "a")).unwrap();
    p.spawn(task("wait", "explore", "b")).unwrap();
    assert!(p.cancel("wait"));
    let queued = p.wait_for("wait").await.unwrap();
    assert_eq!(queued.status, Some(ResultStatus::Cancelled));
    assert_eq!(queued.error.as_deref(), Some("cancelled before start"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(p.cancel("run"));
    let running = p.wait_for("run").await.unwrap();
    assert_eq!(running.status, Some(ResultStatus::Cancelled));
    assert_eq!(p.get_status("run"), TaskStatus::Cancelled);
    assert_eq!(cancelled.lock().unwrap().len(), 2);
    assert!(!p.cancel("nope"));
    p.dispose();
}

// subagent-pool-protocol.test.ts

fn protocol_mock(dir: &Path) -> PathBuf {
    let result = json!({
        "summary": "child-written summary", "files_changed": ["src/changed.ts"],
        "confidence": 0.9, "status": "complete",
        "usage": {"input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0, "cost": 0.001},
    });
    script(
        dir,
        "mock-protocol.sh",
        &format!(
            "{TASK_ID}\necho '{{\"ping\":true}}'\necho '{DONE_LINE}'\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{result}' > {DIR}/dispatch/$tid/result.json\nexit 0"
        ),
    )
}

#[tokio::test]
async fn reads_the_child_written_result_json() {
    let dir = setup();
    let p = pool(protocol_mock(dir.path()), 1, dir.path());
    p.spawn(task("proto-1", "explore", "hello")).unwrap();
    let result = p.wait_for("proto-1").await.unwrap();
    assert!(result.ok);
    assert_eq!(result.status, Some(ResultStatus::Complete));
    let data = result.result_data.unwrap();
    assert_eq!(data["summary"], "child-written summary");
    assert_eq!(data["files_changed"], json!(["src/changed.ts"]));
    p.dispose();
}

#[tokio::test]
async fn dispatch_surfaces_the_child_summary_and_usage() {
    let dir = setup();
    let p = pool(protocol_mock(dir.path()), 1, dir.path());
    let result = p
        .dispatch(
            "investigate the parser thoroughly",
            DispatchOptions {
                force_agent: Some("explore".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!result.handled_inline);
    let sub = result.result.unwrap();
    assert!(sub.ok);
    let data = sub.result_data.unwrap();
    assert_eq!(data["summary"], "child-written summary");
    assert_eq!(data["usage"]["input"], 100);
    p.dispose();
}

#[tokio::test]
async fn forwards_progress_events_and_drops_the_firehose() {
    let dir = setup();
    let exe = script(
        dir.path(),
        "mock-progress.sh",
        r#"echo '{"type":"turn_end"}'
echo '{"type":"message_update"}'
echo '{"type":"tool_execution_start","toolName":"read"}'"#,
    );
    let p = pool(exe, 1, dir.path());
    let progress = record(&p, "task_progress");
    write_valid_result(dir.path(), "prog");
    p.spawn(task("prog", "explore", "x")).unwrap();
    p.wait_for("prog").await.unwrap();
    let progress = progress.lock().unwrap();
    assert_eq!(
        progress
            .iter()
            .map(|e| e["event"]["type"].clone())
            .collect::<Vec<_>>(),
        vec![json!("turn_end"), json!("tool_execution_start")]
    );
    assert_eq!(progress[0]["agent_type"], "explore");
    p.dispose();
}

// subagent-pool-registry.test.ts / subagent-claude-agent.test.ts

fn argv_and_result_recorder(dir: &Path) -> PathBuf {
    script(
        dir,
        "mock-argv-result.sh",
        &format!(
            "printf '%s\\0' \"$@\" > argv.bin\n{TASK_ID}\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{}' > {DIR}/dispatch/$tid/result.json\necho '{DONE_LINE}'",
            result_json("audit complete", "complete", 0.95)
        ),
    )
}

async fn dispatch_forced(
    p: &SubagentPool,
    agent: &str,
    text: &str,
    model: Option<&str>,
) -> TaskResult {
    p.dispatch(
        text,
        DispatchOptions {
            force_agent: Some(agent.into()),
            model: model.map(String::from),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_project_agent_overrides_the_builtin_prompt_tools_and_model() {
    let dir = setup();
    write_project_agent(
        dir.path(),
        "explore",
        "---\nname: explore\ndescription: Project explore agent for testing.\ntools: Read, Glob\nmodel: pinned-model\n---\nPROJECT EXPLORE PROMPT",
    );
    let p = pool(argv_and_result_recorder(dir.path()), 5, dir.path());
    let result = dispatch_forced(&p, "explore", "investigate the module", None).await;
    assert!(result.result.unwrap().ok);
    let argv = read_argv(dir.path());
    assert_eq!(
        arg_after(&argv, "--system-prompt").as_deref(),
        Some("PROJECT EXPLORE PROMPT")
    );
    assert_eq!(
        arg_after(&argv, "--tools").as_deref(),
        Some("read,SearchCodebase")
    );
    assert_eq!(arg_after(&argv, "--model").as_deref(), Some("pinned-model"));
    p.dispose();
}

#[tokio::test]
async fn uses_the_builtin_agents_frontmatter_tool_allowlist() {
    let dir = setup();
    let p = pool(argv_and_result_recorder(dir.path()), 5, dir.path());
    dispatch_forced(&p, "explore", "run a read-only scan", None).await;
    let argv = read_argv(dir.path());
    assert_eq!(
        arg_after(&argv, "--tools").as_deref(),
        Some("read,SearchCodebase")
    );
    p.dispose();
}

#[tokio::test]
async fn spawns_a_claude_format_agent_with_its_prompt_tools_and_model() {
    let dir = setup();
    let agents = dir.path().join(".claude/agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("security-reviewer.md"),
        "---\nname: security-reviewer\ndescription: Use to audit code for injection, auth, and secret-handling flaws.\ntools: Read, Grep, Glob, Bash\nmodel: sonnet\n---\nYou are a security reviewer. Audit the given code for vulnerabilities and report findings ordered by severity.\n",
    )
    .unwrap();
    let p = pool(argv_and_result_recorder(dir.path()), 5, dir.path());
    let result = dispatch_forced(
        &p,
        "security-reviewer",
        "Audit src/login.ts for SQL injection",
        None,
    )
    .await;
    let sub = result.result.unwrap();
    assert!(sub.ok);
    assert_eq!(sub.result_data.unwrap()["summary"], "audit complete");
    let argv = read_argv(dir.path());
    assert!(arg_after(&argv, "--system-prompt")
        .unwrap()
        .contains("You are a security reviewer."));
    assert_eq!(
        arg_after(&argv, "--tools").as_deref(),
        Some("read,SearchCodebase,bash")
    );
    assert_eq!(arg_after(&argv, "--model").as_deref(), Some("sonnet"));
    p.dispose();
}

/// Rejects every model but `parent-model`; records each run's argv.
fn model_fallback_recorder(dir: &Path) -> PathBuf {
    script(
        dir,
        "mock-fallback.sh",
        &format!(
            r#"n=$(ls argv-*.bin 2>/dev/null | wc -l | tr -d ' ')
printf '%s\0' "$@" > argv-$n.bin
model=; prev=; for a in "$@"; do [ "$prev" = "--model" ] && model=$a; prev=$a; done
if [ "$model" != "parent-model" ]; then echo "No API key found for preferred model" >&2; exit 1; fi
{TASK_ID}
mkdir -p {DIR}/dispatch/$tid
printf '%s' '{}' > {DIR}/dispatch/$tid/result.json
echo '{DONE_LINE}'"#,
            result_json("ok", "complete", 0.9)
        ),
    )
}

async fn fallback_run(provider: Option<&str>) {
    let dir = setup();
    let p = SubagentPool::new(SubagentPoolOptions {
        executable: model_fallback_recorder(dir.path()),
        cwd: Some(dir.path().to_path_buf()),
        settings: Some(CategorySettings {
            model_categories: Some(ModelCategories {
                fast: Some("preferred-model".into()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
    let result = p
        .dispatch(
            "run a read-only scan",
            DispatchOptions {
                force_agent: Some("explore".into()),
                model: Some("parent-model".into()),
                provider: provider.map(String::from),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let sub = result.result.unwrap();
    assert!(sub.ok);
    assert_eq!(sub.used_inherited_model_fallback, Some(true));
    let first = read_argv_file(&dir.path().join("argv-0.bin"));
    let second = read_argv_file(&dir.path().join("argv-1.bin"));
    assert!(!dir.path().join("argv-2.bin").exists());
    assert_eq!(
        arg_after(&first, "--model").as_deref(),
        Some("preferred-model")
    );
    assert_eq!(
        arg_after(&second, "--model").as_deref(),
        Some("parent-model")
    );
    p.dispose();
}

#[tokio::test]
async fn retries_builtin_agents_with_the_inherited_model() {
    fallback_run(Some("parent-provider")).await;
}

#[tokio::test]
async fn falls_back_to_the_inherited_model_without_a_provider() {
    fallback_run(None).await;
}

// subagent-pool-inherited-model.test.ts

fn failed(error: &str) -> SubagentResult {
    SubagentResult {
        task_id: "t1".into(),
        exit_code: Some(1),
        status: Some(ResultStatus::Failed),
        error: Some(error.into()),
        ..Default::default()
    }
}

#[test]
fn fallback_error_matches_not_supported_and_unsupported_wording() {
    assert!(SubagentPool::is_inherited_model_fallback_error(&failed(
        "400 The requested model is not supported"
    )));
    assert!(SubagentPool::is_inherited_model_fallback_error(&failed(
        "model is unsupported by this provider"
    )));
    assert!(!SubagentPool::is_inherited_model_fallback_error(&failed(
        "syntax error in tool output"
    )));
}

fn agent(source: AgentSource, model: &str) -> AgentDefinition {
    AgentDefinition {
        name: "custom".into(),
        description: "A project agent".into(),
        tools: None,
        disallowed_tools: None,
        model: Some(model.into()),
        prompt: "do work".into(),
        source,
        file_path: None,
        max_turns: None,
        background: None,
        delegate: None,
        delegate_to: None,
        fork: None,
    }
}

#[tokio::test]
async fn should_retry_with_inherited_model_rules() {
    let t = SubagentPoolTask {
        model: Some("claude-opus-4-7".into()),
        ..task("t1", "custom", "do work")
    };
    let result = failed("400 The requested model is not supported");
    let check = |def: AgentDefinition, task: &SubagentPoolTask| {
        let p = SubagentPool::new(SubagentPoolOptions {
            executable: "true".into(),
            ..Default::default()
        });
        let mut registry = AgentRegistry::new();
        registry.register(def);
        p.set_registry(registry);
        let answer = p.should_retry_with_inherited_model(task, &result);
        p.dispose();
        answer
    };
    assert!(check(agent(AgentSource::Project, "claude-haiku-4-5"), &t));
    assert!(check(agent(AgentSource::Builtin, "claude-haiku-4-5"), &t));
    assert!(!check(agent(AgentSource::User, "claude-haiku-4-5"), &t));
    assert!(!check(agent(AgentSource::Project, "inherit"), &t));
    let no_parent = SubagentPoolTask {
        model: None,
        ..t.clone()
    };
    assert!(!check(
        agent(AgentSource::Project, "claude-haiku-4-5"),
        &no_parent
    ));
}

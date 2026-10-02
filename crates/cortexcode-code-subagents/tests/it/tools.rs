//! task-output.test.ts, subagent-progress-roster.test.ts and subagent.test.ts,
//! plus the Task tool's execute paths against a pool of shell-mock children
//! (the TS suite's fake pool is 10.9f's subagent-execution test).
//!
//! The inbox, task store and shared pool are process-wide, so each group
//! runs as one serialized test.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Duration;

use cortexcode_agent_types::{AgentToolCall, AgentToolResult};
use cortexcode_ai_types::Content;
use cortexcode_code_resources::agent_registry::EMBEDDED_AGENT_PROMPTS;
use cortexcode_code_resources::{parse_agent_definition, AgentSource};
use cortexcode_code_subagents::inbox::{subagent_inbox, TaskLifecycle};
use cortexcode_code_subagents::instance::{
    dispose_subagent_pool, get_subagent_pool, set_subagent_pool_for_testing,
};
use cortexcode_code_subagents::pool::{
    ResultStatus, SubagentPool, SubagentPoolOptions, SubagentResult, TaskResult,
};
use cortexcode_code_subagents::tools::*;
use cortexcode_code_task_store::{
    task_store, TaskAgentKind, TaskAgentPatch, TaskAgentState, TaskStatus,
};
use cortexcode_code_tool_api::{ToolContext, ToolDefinition};
use serde_json::{json, Value};

/// One test at a time touches the process-wide inbox, store and pool.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn isolate_agent_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-tools-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir);
    });
}

fn text(result: &AgentToolResult) -> String {
    match &result.content[0] {
        Content::Text(t) => t.text.clone(),
        other => panic!("{other:?}"),
    }
}

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

fn run(
    tool: &ToolDefinition,
    args: Value,
    ctx: Option<&ToolContext>,
) -> Result<AgentToolResult, String> {
    (tool.execute)("tc".into(), args, None, None, ctx).map_err(|e| e.to_string())
}

fn assert_subset(details: &Value, expected: Value) {
    for (k, v) in expected.as_object().unwrap() {
        assert_eq!(&details[k], v, "{k} in {details}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn task_output_tool() {
    let _serial = SERIAL.lock().await;
    let inbox = subagent_inbox();
    let tool = create_task_output_tool_definition();

    // lists all background subagents with status, without bodies
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    inbox.start("t2", "explore#2", "explore");
    inbox.finish(
        "t2",
        &ok_result(
            "t2",
            "Two: headline\nlong body that must not appear in the roster listing",
        ),
    );
    let r = run(&tool, json!({"list": true}), None).unwrap();
    let t = text(&r);
    for needle in ["explore#1", "running", "explore#2", "done", "Two: headline"] {
        assert!(t.contains(needle), "{needle} in {t}");
    }
    assert!(!t.contains("must not appear in the roster"));
    assert_subset(
        &r.details,
        json!({"status": "list", "ok": true, "outstanding": 1}),
    );

    // returns the body once, then reports it as already delivered
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    inbox.finish("t1", &ok_result("t1", "the full result body"));
    let first = run(&tool, json!({"task_id": "explore#1"}), None).unwrap();
    assert_eq!(text(&first), "the full result body");
    assert_subset(&first.details, json!({"status": "done", "ok": true}));
    let second = run(&tool, json!({"task_id": "explore#1"}), None).unwrap();
    assert!(text(&second).contains("already delivered"));
    assert_subset(&second.details, json!({"status": "collected", "ok": true}));

    // reports status (not an error) for a still-running task
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    let r = run(&tool, json!({"task_id": "explore#1"}), None).unwrap();
    assert!(text(&r).contains("still running"));
    assert_subset(&r.details, json!({"status": "running", "ok": true}));

    // never throws on an unknown handle
    let r = run(&tool, json!({"task_id": "nope#9"}), None).unwrap();
    assert!(text(&r).contains("No background task"));
    assert_subset(&r.details, json!({"status": "unknown", "ok": false}));

    // reports a failed task without throwing
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    inbox.fail("t1", "no heartbeat", TaskLifecycle::Stalled);
    let r = run(&tool, json!({"task_id": "explore#1"}), None).unwrap();
    assert!(text(&r).contains("stalled"));
    assert!(text(&r).contains("no heartbeat"));
    assert_subset(&r.details, json!({"status": "stalled", "ok": false}));

    // wait:true blocks until the task finishes, then returns its body
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        subagent_inbox().finish("t1", &ok_result("t1", "finished after waiting"));
    });
    let tool2 = tool.clone();
    let r = tokio::task::spawn_blocking(move || {
        run(
            &tool2,
            json!({"task_id": "explore#1", "wait": true, "timeout_ms": 2000}),
            None,
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(text(&r), "finished after waiting");
    assert_subset(&r.details, json!({"status": "done", "ok": true}));

    // wait:true with no task_id is a barrier for all outstanding tasks
    inbox.clear();
    inbox.start("t1", "explore#1", "explore");
    inbox.start("t2", "explore#2", "explore");
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(5)).await;
        subagent_inbox().finish("t1", &ok_result("t1", "one"));
        tokio::time::sleep(Duration::from_millis(10)).await;
        subagent_inbox().finish("t2", &ok_result("t2", "two"));
    });
    let tool3 = tool.clone();
    let r = tokio::task::spawn_blocking(move || {
        run(&tool3, json!({"wait": true, "timeout_ms": 2000}), None)
    })
    .await
    .unwrap()
    .unwrap();
    assert_subset(&r.details, json!({"status": "list", "outstanding": 0}));
    inbox.clear();
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_events_drive_roster_activity() {
    let _serial = SERIAL.lock().await;
    isolate_agent_dir();
    let store = task_store();
    let activity_of = |id: &str| {
        store
            .agents()
            .into_iter()
            .find(|a| a.id == id)
            .and_then(|a| a.activity)
    };
    let upsert = |id: &str, name: &str| {
        store.upsert_agent(
            id,
            name,
            TaskAgentKind::Subagent,
            TaskAgentPatch {
                state: Some(TaskAgentState::Running),
                ..Default::default()
            },
        );
    };
    let progress = |pool: &SubagentPool, id: &str, event: Value| {
        pool.emit_for_testing(
            "task_progress",
            json!({"task_id": id, "agent_type": "explore", "event": event}),
        )
    };
    let cwd = std::env::current_dir().unwrap();

    // tool start sets the activity, tool end clears it
    store.clear();
    upsert("run-1", "explore#1");
    let pool = get_subagent_pool(&cwd, &[]);
    progress(
        &pool,
        "run-1",
        json!({"type": "tool_execution_start", "toolName": "SearchCodebase"}),
    );
    assert_eq!(activity_of("run-1").as_deref(), Some("SearchCodebase"));
    progress(
        &pool,
        "run-1",
        json!({"type": "tool_execution_end", "toolName": "SearchCodebase"}),
    );
    assert_eq!(activity_of("run-1").as_deref(), Some(""));

    // thinking between turns; terminal events clear
    progress(
        &pool,
        "run-1",
        json!({"type": "tool_execution_start", "toolName": "bash"}),
    );
    assert_eq!(activity_of("run-1").as_deref(), Some("bash"));
    progress(&pool, "run-1", json!({"type": "turn_end"}));
    assert_eq!(activity_of("run-1").as_deref(), Some("thinking"));
    pool.emit_for_testing(
        "task_done",
        json!({"agent_type": "explore", "task_id": "run-1"}),
    );
    assert_eq!(activity_of("run-1").as_deref(), Some(""));

    // concurrent same-type runs keep separate rows
    upsert("run-2", "explore#2");
    progress(
        &pool,
        "run-1",
        json!({"type": "tool_execution_start", "toolName": "SearchCodebase"}),
    );
    progress(
        &pool,
        "run-2",
        json!({"type": "tool_execution_start", "toolName": "bash"}),
    );
    pool.emit_for_testing(
        "task_done",
        json!({"agent_type": "explore", "task_id": "run-2"}),
    );
    assert_eq!(activity_of("run-1").as_deref(), Some("SearchCodebase"));
    assert_eq!(activity_of("run-2").as_deref(), Some(""));

    // a run with no roster row is a no-op
    progress(
        &pool,
        "ghost-run",
        json!({"type": "tool_execution_start", "toolName": "SearchCodebase"}),
    );
    assert!(store.agents().iter().all(|a| a.id != "ghost-run"));
    dispose_subagent_pool();
    store.clear();
}

fn builtin_tools(name: &str) -> Vec<String> {
    let raw = EMBEDDED_AGENT_PROMPTS
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    parse_agent_definition(raw, AgentSource::Builtin, None, Some(name))
        .0
        .unwrap()
        .tools
        .unwrap_or_default()
}

#[test]
fn builtin_subagent_allowlists() {
    let mut names: Vec<&str> = EMBEDDED_AGENT_PROMPTS.iter().map(|(n, _)| *n).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "code-review",
            "explore",
            "general-purpose",
            "plan",
            "security-review"
        ]
    );
    for name in ["code-review", "security-review"] {
        let tools = builtin_tools(name);
        assert!(tools.contains(&"bash".to_string()));
        assert!(!tools.iter().any(|t| t == "edit" || t == "write"));
    }
    for name in ["plan", "explore"] {
        let tools = builtin_tools(name);
        assert!(
            !tools
                .iter()
                .any(|t| t == "edit" || t == "write" || t == "bash"),
            "{name}"
        );
    }
    for name in names {
        assert!(!builtin_tools(name).is_empty(), "{name}");
    }
    assert!(builtin_tools("general-purpose").contains(&"write".to_string()));
    let raw = EMBEDDED_AGENT_PROMPTS
        .iter()
        .find(|(n, _)| *n == "general-purpose")
        .unwrap()
        .1;
    let gp = parse_agent_definition(raw, AgentSource::Builtin, None, Some("general-purpose"))
        .0
        .unwrap();
    assert_eq!(gp.delegate, Some(true));
}

#[test]
fn resolve_fork_session_file_cases() {
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("parent.jsonl");
    std::fs::write(
        &parent,
        format!(
            "{}\n",
            json!({"type": "session", "version": 3, "id": "parent", "timestamp": "2026-01-01T00:00:00.000Z", "cwd": dir.path()})
        ),
    )
    .unwrap();
    assert_eq!(
        resolve_fork_session_file(None, Some(&parent), dir.path()),
        None
    );
    assert_eq!(
        resolve_fork_session_file(Some(true), None, dir.path()),
        None
    );
    std::env::set_var(
        "CORTEXCODE_CODING_AGENT_SESSION_DIR",
        dir.path().join("sessions"),
    );
    let forked = resolve_fork_session_file(Some(true), Some(&parent), dir.path()).unwrap();
    std::env::remove_var("CORTEXCODE_CODING_AGENT_SESSION_DIR");
    assert!(forked.exists());
    assert_ne!(forked, parent);
    let empty = dir.path().join("empty.jsonl");
    std::fs::write(&empty, "").unwrap();
    assert_eq!(
        resolve_fork_session_file(Some(true), Some(&empty), dir.path()),
        None
    );
}

#[test]
fn summarize_agent_description_cases() {
    let builtin_style = "Use this subagent ONLY when:\n- Reading or understanding code without changes\n- Scouting a codebase for plans or maps\n- Analyzing dependencies, imports, project structure\n\nDO NOT use for:\n- Writing or modifying code";
    assert_eq!(
        summarize_agent_description(builtin_style),
        "Reading or understanding code without changes; Scouting a codebase for plans or maps; Analyzing dependencies, imports, project structure"
    );
    assert_eq!(
        summarize_agent_description("Expert TypeScript reviewer"),
        "Expert TypeScript reviewer"
    );
    assert_eq!(
        summarize_agent_description("Reviews PRs for correctness and risk.\nMore detail here."),
        "Reviews PRs for correctness and risk."
    );
    assert_eq!(summarize_agent_description(""), "");
}

#[test]
fn build_task_main_prompt_contents() {
    isolate_agent_dir();
    let cwd = tempfile::tempdir().unwrap();
    let prompt = build_task_main_prompt(cwd.path());
    assert!(prompt.contains("<available_agents>"));
    assert!(!prompt.contains("- explore: "));
    assert!(prompt.contains("Dispatch independent subtasks in the same turn"));
    assert!(prompt.contains("Keep inline only trivial single-step edits"));
    assert!(!prompt.contains("Default to handling small, quick, or single-file work inline"));
    assert!(prompt.contains("mark the item in_progress BEFORE dispatching"));
    // Built-in explore/plan are background agents: the background block is in.
    assert!(prompt.contains("don't idle"));
    assert!(prompt.contains("TaskOutput(wait: true)"));
    assert!(!prompt.contains("DO NOT stop and wait"));
    assert!(prompt.contains("Delegate when you need only the final result"));
    assert!(!prompt.contains("WHEN TO USE:"));
    assert_eq!(prompt.matches("cannot see this conversation").count(), 1);
    assert_eq!(prompt.matches("final answer").count(), 2);
    assert!(!prompt.contains("When to delegate:"));
    assert!(!prompt.contains("Delegate proactively when work is self-contained"));
}

#[test]
fn format_duration_secs_like_hoocode() {
    assert_eq!(format_duration_secs(0.0), "0.0s");
    assert_eq!(format_duration_secs(3.21), "3.2s");
    assert_eq!(format_duration_secs(42.4), "42s");
    assert_eq!(format_duration_secs(125.0), "2m05s");
    assert_eq!(format_duration_secs(3720.0), "1h02m");
    assert_eq!(format_duration_secs(-1.0), "0.0s");
}

#[test]
fn the_task_tool_runs_in_the_background_per_agent_or_argument() {
    isolate_agent_dir();
    let cwd = tempfile::tempdir().unwrap();
    let tool = create_task_tool_definition(cwd.path());
    let predicate = tool.background_when.clone().unwrap();
    let call = |args: Value| AgentToolCall {
        id: "c".into(),
        name: "Task".into(),
        arguments: args,
    };
    // Built-in explore is a background agent; general-purpose is not.
    assert!(predicate(&call(json!({"subagent_type": "explore"}))));
    assert!(!predicate(&call(
        json!({"subagent_type": "general-purpose"})
    )));
    // A per-call argument overrides either way.
    assert!(!predicate(&call(
        json!({"subagent_type": "explore", "background": false})
    )));
    assert!(predicate(&call(
        json!({"subagent_type": "general-purpose", "background": true})
    )));
}

// The Task tool's execute paths, against a real pool of shell-mock children.

const DIR: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

fn mock_child(dir: &Path, summary: &str, status: &str, exit: i32) -> PathBuf {
    let path = dir.join("mock-child.sh");
    let result = json!({"summary": summary, "files_changed": [], "confidence": 0.9, "status": status,
        "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "cost": 0.01}});
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ntid=unknown; prev=; for a in \"$@\"; do [ \"$prev\" = \"--task-id\" ] && tid=$a; prev=$a; done\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{result}' > {DIR}/dispatch/$tid/result.json\nexit {exit}\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn install_pool(cwd: &Path, exe: PathBuf) -> SubagentPool {
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    set_subagent_pool_for_testing(Some(pool.clone()));
    pool
}

fn ctx(cwd: &Path) -> ToolContext {
    ToolContext {
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    }
}

async fn execute(
    tool: ToolDefinition,
    args: Value,
    ctx: ToolContext,
) -> Result<AgentToolResult, String> {
    tokio::task::spawn_blocking(move || run(&tool, args, Some(&ctx)))
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn task_tool_execute_paths() {
    let _serial = SERIAL.lock().await;
    isolate_agent_dir();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let tool = create_task_tool_definition(&cwd);
    task_store().clear();
    subagent_inbox().clear();

    // Foreground: the answer comes back inline and the task settles done.
    let pool = install_pool(&cwd, mock_child(&cwd, "Found it in main.rs", "complete", 0));
    let r = execute(
        tool.clone(),
        json!({"description": "Find the bug", "prompt": "Find the bug", "subagent_type": "general-purpose"}),
        ctx(&cwd),
    )
    .await
    .unwrap();
    assert_eq!(text(&r), "Found it in main.rs");
    assert_subset(
        &r.details,
        json!({"subagent_type": "general-purpose", "ok": true}),
    );
    let task = task_store()
        .list()
        .into_iter()
        .find(|t| t.title == "Find the bug")
        .unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.usage.unwrap().input, 10.0);
    let row = task_store()
        .agents()
        .into_iter()
        .find(|a| Some(&a.id) == task.agent.as_ref())
        .unwrap();
    assert_eq!(row.name, "general-purpose#1");
    assert_eq!(row.state, Some(TaskAgentState::Done));
    pool.dispose();

    // Background: a compact notification; the body waits in the inbox.
    let pool = install_pool(
        &cwd,
        mock_child(&cwd, "Mapped the module\nwith details", "complete", 0),
    );
    let r = execute(
        tool.clone(),
        json!({"description": "Map it", "prompt": "Map the module", "subagent_type": "explore"}),
        ctx(&cwd),
    )
    .await
    .unwrap();
    assert_eq!(
        text(&r),
        "explore#1 finished ✓ — Mapped the module.\nRead the full result with TaskOutput(\"explore#1\")."
    );
    assert_subset(&r.details, json!({"ok": true, "background": true}));
    let (_, body) = subagent_inbox().collect("explore#1").unwrap();
    assert_eq!(body, "Mapped the module\nwith details");
    pool.dispose();

    // A failed foreground run is an error carrying the child's reason.
    let pool = install_pool(&cwd, mock_child(&cwd, "Task failed: quota", "failed", 1));
    let err = execute(
        tool.clone(),
        json!({"description": "Do it", "prompt": "Do it", "subagent_type": "general-purpose"}),
        ctx(&cwd),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "Subagent (general-purpose) failed: Task failed: quota");
    pool.dispose();

    // An unknown agent is rejected with the available list.
    let err = execute(
        tool.clone(),
        json!({"description": "x", "prompt": "x", "subagent_type": "nope"}),
        ctx(&cwd),
    )
    .await
    .unwrap_err();
    assert!(
        err.starts_with("Unknown subagent_type: \"nope\". Available agents: "),
        "{err}"
    );
    assert!(err.contains("explore"));

    // An exhausted inherited provider skips the spawn.
    let model: cortexcode_ai_types::Model = serde_json::from_value(json!({
        "id": "m", "name": "m", "api": "openai-completions", "provider": "prov",
        "baseUrl": "http://x", "contextWindow": 1000, "maxTokens": 100,
    }))
    .unwrap();
    cortexcode_code_agent_session::provider_health::mark_provider_exhausted(
        "prov",
        "Usage limit reached",
    );
    let r = execute(
        tool.clone(),
        json!({"description": "Skip", "prompt": "x", "subagent_type": "explore"}),
        ToolContext {
            model: Some(model),
            ..ctx(&cwd)
        },
    )
    .await
    .unwrap();
    cortexcode_code_agent_session::provider_health::clear_provider_exhaustion("prov");
    assert!(text(&r).starts_with(
        "Did not dispatch subagent \"explore\": the \"prov\" provider appears exhausted"
    ));
    let skipped = task_store()
        .list()
        .into_iter()
        .find(|t| t.title == "Skip")
        .unwrap();
    assert_eq!(skipped.status, TaskStatus::Failed);
    assert_eq!(skipped.note.as_deref(), Some("prov exhausted"));

    set_subagent_pool_for_testing(None);
    task_store().clear();
    subagent_inbox().clear();
}

// suite/subagent-execution.test.ts: the cases the paths above do not cover.

fn argv_child(dir: &Path) -> PathBuf {
    let path = dir.join("mock-argv.sh");
    let result =
        json!({"summary": "ok", "files_changed": [], "confidence": 0.9, "status": "complete"});
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\0' \"$@\" > argv.bin\ntid=unknown; prev=; for a in \"$@\"; do [ \"$prev\" = \"--task-id\" ] && tid=$a; prev=$a; done\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{result}' > {DIR}/dispatch/$tid/result.json\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn model_arg(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(dir.join("argv.bin")).unwrap();
    let argv: Vec<String> = bytes
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    let i = argv.iter().position(|a| a == "--model")?;
    argv.get(i + 1).cloned()
}

/// A child that appends each attempt's `--model` to `models.txt`: fails with
/// `failed` on the first (pinned) model, succeeds on any other.
fn failing_then_ok_child(dir: &Path, failed: &Value) -> PathBuf {
    let path = dir.join("mock-fallback.sh");
    let ok = json!({"summary": "recovered", "files_changed": [], "confidence": 0.9, "status": "complete"});
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ntid=unknown; model=; prev=; for a in \"$@\"; do [ \"$prev\" = \"--task-id\" ] && tid=$a; [ \"$prev\" = \"--model\" ] && model=$a; prev=$a; done\nmkdir -p {DIR}/dispatch/$tid\necho \"$model\" >> models.txt\nif [ \"$model\" = \"pinned-model\" ]; then printf '%s' '{failed}' > {DIR}/dispatch/$tid/result.json; exit 1; fi\nprintf '%s' '{ok}' > {DIR}/dispatch/$tid/result.json\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Every `--model` the child was spawned with, in order.
fn model_arg_history(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("models.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn task_tool_passes_complexity_or_the_parent_model() {
    let _serial = SERIAL.lock().await;
    isolate_agent_dir();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    // An agent that inherits the model (built-ins pin a category, which wins).
    let agents = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("helper.md"),
        "---\nname: helper\ndescription: Inherits the model.\ntools: read\nmodel: inherit\n---\nHelp.",
    )
    .unwrap();
    let tool = create_task_tool_definition(&cwd);
    task_store().clear();
    let model: cortexcode_ai_types::Model = serde_json::from_value(json!({
        "id": "parent-model", "name": "p", "api": "openai-completions", "provider": "prov",
        "baseUrl": "http://x", "contextWindow": 1000, "maxTokens": 100,
    }))
    .unwrap();
    // `complexity` goes to the pool as the model; the pool resolves the tier.
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: argv_child(&cwd),
        cwd: Some(cwd.clone()),
        settings: Some(
            cortexcode_code_subagents::model_categories::CategorySettings {
                model_categories: Some(cortexcode_code_settings::ModelCategories {
                    fast: Some("fast-model".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ),
        ..Default::default()
    });
    set_subagent_pool_for_testing(Some(pool.clone()));
    let with_model = || ToolContext {
        model: Some(model.clone()),
        ..ctx(&cwd)
    };
    execute(
        tool.clone(),
        json!({"description": "quick read", "prompt": "read one file", "subagent_type": "helper", "complexity": "fast"}),
        with_model(),
    )
    .await
    .unwrap();
    assert_eq!(model_arg(&cwd).as_deref(), Some("fast-model"));
    // Without it, the parent's model.
    execute(
        tool.clone(),
        json!({"description": "do work", "prompt": "do some work", "subagent_type": "helper"}),
        with_model(),
    )
    .await
    .unwrap();
    assert_eq!(model_arg(&cwd).as_deref(), Some("parent-model"));
    pool.dispose();
    set_subagent_pool_for_testing(None);
    task_store().clear();
}

/// The inherited-model fallback has to actually switch models.
///
/// When the caller passes a `complexity` tier, `DispatchOptions::model` holds
/// the *category*, not a model. Falling back to it resolved the category again
/// and re-ran on the model that had just failed — the retry looked like it
/// happened and changed nothing.
#[tokio::test]
async fn the_inherited_model_fallback_switches_away_from_a_complexity_tier() {
    let _serial = SERIAL.lock().await;
    isolate_agent_dir();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    // An agent that pins a concrete model, so the fallback has something to
    // drop; the caller's `complexity` is what it will otherwise resolve to.
    let agents = cwd.join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("pinned.md"),
        "---\nname: pinned\ndescription: Pins a model.\ntools: read\nmodel: pinned-model\n---\nBody.",
    )
    .unwrap();
    let tool = create_task_tool_definition(&cwd);
    task_store().clear();
    let model: cortexcode_ai_types::Model = serde_json::from_value(json!({
        "id": "parent-model", "name": "p", "api": "openai-completions", "provider": "prov",
        "baseUrl": "http://x", "contextWindow": 1000, "maxTokens": 100,
    }))
    .unwrap();
    let failed = json!({
        "summary": "Task failed: This Go model requires Global regions.",
        "files_changed": [], "confidence": 0.5, "status": "failed"
    });
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: failing_then_ok_child(&cwd, &failed),
        cwd: Some(cwd.clone()),
        settings: Some(
            cortexcode_code_subagents::model_categories::CategorySettings {
                model_categories: Some(cortexcode_code_settings::ModelCategories {
                    fast: Some("tier-model".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ),
        ..Default::default()
    });
    set_subagent_pool_for_testing(Some(pool.clone()));
    let result = execute(
        tool,
        json!({"description": "quick read", "prompt": "read one file", "subagent_type": "pinned", "complexity": "fast"}),
        ToolContext {
            model: Some(model),
            ..ctx(&cwd)
        },
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    // Two attempts were recorded: the pinned model, then the parent's.
    let models = model_arg_history(&cwd);
    assert_eq!(models, vec!["pinned-model", "parent-model"]);
    pool.dispose();
    set_subagent_pool_for_testing(None);
    task_store().clear();
}

#[test]
fn a_project_agent_opting_into_background_runs_detached() {
    isolate_agent_dir();
    let dir = tempfile::tempdir().unwrap();
    let agents = dir.path().join(DIR).join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("watcher.md"),
        "---\nname: watcher\ndescription: A non-blocking background agent.\nbackground: true\n---\nBody.",
    )
    .unwrap();
    let tool = create_task_tool_definition(dir.path());
    let predicate = tool.background_when.clone().unwrap();
    let call = |agent: &str| AgentToolCall {
        id: "c".into(),
        name: "Task".into(),
        arguments: json!({"subagent_type": agent}),
    };
    assert!(predicate(&call("watcher")));
    assert!(predicate(&call("explore")));
    assert!(!predicate(&call("general-purpose")));
    assert!(!predicate(&call("does-not-exist")));
}

fn active_tool_names(with_task: bool) -> Vec<String> {
    use cortexcode_code_agent_session::{
        create_agent_session, AgentSessionServices, CreateAgentSessionOptions, StaticResourceLoader,
    };
    let dir = tempfile::tempdir().unwrap();
    let model: cortexcode_ai_types::Model = serde_json::from_value(json!({
        "id": "m", "name": "m", "api": "openai-completions", "provider": "prov",
        "baseUrl": "http://x", "contextWindow": 1000, "maxTokens": 100,
    }))
    .unwrap();
    let services = AgentSessionServices {
        cwd: dir.path().to_path_buf(),
        agent_dir: dir.path().to_path_buf(),
        auth: Arc::new(cortexcode_code_models::NoAuth),
        settings: Arc::new(std::sync::Mutex::new(
            cortexcode_code_settings::SettingsManager::in_memory(Default::default()),
        )),
        model_registry: Arc::new(cortexcode_code_models::ModelRegistry::in_memory()),
        resource_loader: Arc::new(StaticResourceLoader::default()),
        diagnostics: vec![],
    };
    let created = create_agent_session(
        &services,
        cortexcode_code_session::SessionManager::in_memory(dir.path().to_string_lossy()),
        CreateAgentSessionOptions {
            model: Some(model),
            custom_tools: if with_task {
                vec![create_task_tool_definition(dir.path())]
            } else {
                vec![]
            },
            ..Default::default()
        },
    );
    let names = created.session.get_active_tool_names();
    created.session.dispose();
    names
}

#[test]
fn the_task_tool_is_active_only_when_registered() {
    isolate_agent_dir();
    let on = active_tool_names(true);
    assert!(on.contains(&"Task".to_string()));
    assert!(on.contains(&"read".to_string()));
    let off = active_tool_names(false);
    assert!(!off.contains(&"Task".to_string()));
    assert!(off.contains(&"read".to_string()));
}

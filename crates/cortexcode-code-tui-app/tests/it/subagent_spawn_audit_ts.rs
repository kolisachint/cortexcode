//! The roster and task panel half of the pin's
//! `test/suite/subagent-spawn-audit.test.ts`: per-run roster identity for
//! concurrent same-type dispatches, per-run usage, and the panel's wall-clock
//! elapsed, orphan roots, cycle guard, clearable notes and re-render after a
//! mutation. The pool, lifeguard, JSONL and cancellation cases are in
//! `cortexcode-code-subagents/tests/subagent_spawn_audit_ts.rs`.
//!
//! The TS "deferred pool" is a real pool here over a shell child that waits
//! for a release file before writing its result.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::Duration;

use cortexcode_code_subagents::inbox::subagent_inbox;
use cortexcode_code_subagents::instance::set_subagent_pool_for_testing;
use cortexcode_code_subagents::pool::{SubagentPool, SubagentPoolOptions};
use cortexcode_code_subagents::tools::create_task_tool_definition;
use cortexcode_code_task_store::{
    advance_clock_for_tests, task_store, AgentStats, CreateTaskOptions, TaskAgent, TaskAgentKind,
    TaskAgentState, TaskPatch, TaskSource, TaskStatus, TaskUsage,
};
use cortexcode_code_tool_api::ToolContext;
use cortexcode_code_tui_theme::init_theme;
use cortexcode_code_tui_widgets::task_panel::{TaskPanelComponent, TaskPanelView};
use cortexcode_tui_render::Component;
use serde_json::json;

const DIR: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir =
            std::env::temp_dir().join(format!("cortex-spawn-audit-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir);
    });
    init_theme(Some("dark"), false);
    task_store().clear();
    subagent_inbox().clear();
    set_subagent_pool_for_testing(None);
    guard
}

fn strip(s: &str) -> String {
    cortexcode_tui_util::strip_vt_control_characters(s)
}

fn plain(panel: &mut TaskPanelComponent) -> Vec<String> {
    panel.render(120).iter().map(|l| strip(l)).collect()
}

fn sub(mode: &str) -> CreateTaskOptions {
    CreateTaskOptions {
        source: Some(TaskSource::Subagent),
        subagent_mode: Some(mode.into()),
        ..Default::default()
    }
}

fn update(id: u64, patch: TaskPatch) {
    task_store().update(id, patch);
}

fn subagent_rows() -> Vec<TaskAgent> {
    task_store()
        .agents()
        .into_iter()
        .filter(|a| a.kind == TaskAgentKind::Subagent)
        .collect()
}

/// A child that waits for `release-<task id>` in its cwd, then writes a
/// successful result (with usage) for its task.
fn deferred_pool(cwd: &Path) -> SubagentPool {
    use std::os::unix::fs::PermissionsExt;
    let result = json!({"summary": "done", "files_changed": [], "confidence": 0.9, "status": "complete",
        "usage": {"input": 1200, "output": 300, "cacheRead": 0, "cacheWrite": 0, "cost": 0.01}});
    let exe: PathBuf = cwd.join("deferred-child.sh");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\ntid=unknown; prev=; for a in \"$@\"; do [ \"$prev\" = \"--task-id\" ] && tid=$a; prev=$a; done\nwhile [ ! -f release-$tid ]; do sleep 0.05; done\nmkdir -p {DIR}/dispatch/$tid\nprintf '%s' '{result}' > {DIR}/dispatch/$tid/result.json\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    set_subagent_pool_for_testing(Some(pool.clone()));
    pool
}

/// Run a Task call on its own thread (it blocks until the dispatch settles).
fn start_task(
    cwd: &Path,
    id: &str,
    description: &str,
    prompt: &str,
) -> std::thread::JoinHandle<()> {
    let tool = create_task_tool_definition(cwd);
    let cwd = cwd.to_path_buf();
    let (id, args) = (
        id.to_string(),
        json!({"description": description, "prompt": prompt, "subagent_type": "explore"}),
    );
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _enter = handle.enter();
        let ctx = ToolContext {
            cwd: Some(cwd),
            ..Default::default()
        };
        (tool.execute)(id, args, None, None, Some(&ctx)).unwrap();
    })
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn agent_state(id: &str) -> Option<TaskAgentState> {
    task_store()
        .agents()
        .into_iter()
        .find(|a| a.id == id)
        .and_then(|a| a.state)
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

// --- per-run roster identity ---------------------------------------------------------

#[test]
fn gives_concurrent_same_type_dispatches_their_own_roster_rows_and_settles_them_independently() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let rt = runtime();
    let _enter = rt.enter();
    let pool = deferred_pool(&cwd);
    let first = start_task(&cwd, "c1", "scan a", "scan module a");
    let second = start_task(&cwd, "c2", "scan b", "scan module b");
    wait_until("two running rows", || {
        subagent_rows()
            .iter()
            .filter(|a| a.state == Some(TaskAgentState::Running))
            .count()
            == 2
    });

    let runs = subagent_rows();
    assert_ne!(runs[0].id, runs[1].id);
    // Rows are labeled like the inbox, not keyed by bare type.
    let mut names: Vec<String> = runs.iter().map(|a| a.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["explore#1", "explore#2"]);
    // Each task row points at its own run.
    let mut owners: Vec<String> = task_store()
        .list()
        .into_iter()
        .filter_map(|t| t.agent)
        .collect();
    owners.sort();
    let mut ids: Vec<String> = runs.iter().map(|a| a.id.clone()).collect();
    ids.sort();
    assert_eq!(owners, ids);

    // Finish only the first run: its row settles, the sibling keeps running.
    let first_id = runs
        .iter()
        .find(|a| a.name == "explore#1")
        .unwrap()
        .id
        .clone();
    let second_id = runs
        .iter()
        .find(|a| a.name == "explore#2")
        .unwrap()
        .id
        .clone();
    std::fs::write(cwd.join(format!("release-{first_id}")), "").unwrap();
    wait_until("the first row to settle", || {
        agent_state(&first_id) == Some(TaskAgentState::Done)
    });
    assert_eq!(agent_state(&second_id), Some(TaskAgentState::Running));

    std::fs::write(cwd.join(format!("release-{second_id}")), "").unwrap();
    wait_until("the second row to settle", || {
        agent_state(&second_id) == Some(TaskAgentState::Done)
    });
    let _ = (first.join(), second.join());
    set_subagent_pool_for_testing(None);
    pool.dispose();
}

#[test]
fn attributes_per_run_usage_to_the_runs_own_row_not_a_shared_type_row() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let rt = runtime();
    let _enter = rt.enter();
    let pool = deferred_pool(&cwd);
    let call = start_task(&cwd, "c1", "scan", "scan the repo");
    wait_until("a running row", || subagent_rows().len() == 1);
    let run_id = subagent_rows()[0].id.clone();
    std::fs::write(cwd.join(format!("release-{run_id}")), "").unwrap();
    call.join().unwrap();

    let row = task_store()
        .agents()
        .into_iter()
        .find(|a| a.id == run_id)
        .unwrap();
    assert_eq!(
        row.stats,
        Some(AgentStats {
            input: 1200.0,
            output: 300.0,
            cost: 0.01
        })
    );
    // No type-keyed row exists to swallow the stats.
    assert!(!task_store().agents().iter().any(|a| a.id == "explore"));
    set_subagent_pool_for_testing(None);
    pool.dispose();
}

// --- task panel audit fixes ------------------------------------------------------------

#[test]
fn advances_a_running_delegated_tasks_elapsed_time_from_the_wall_clock() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    let task = task_store().create("long build", sub("explore"));
    update(
        task.id,
        TaskPatch {
            status: Some(TaskStatus::InProgress),
            usage: Some(TaskUsage {
                input: 1000.0,
                output: 100.0,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    advance_clock_for_tests(90_000);
    let lines = plain(&mut panel);
    let row = lines.iter().find(|l| l.contains("long build")).unwrap();
    assert!(row.contains("1m30s"), "{row}");
    panel.dispose();
}

#[test]
fn renders_orphaned_children_as_roots_so_the_tab_count_matches_the_rows() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    // A main task keeps a second lens alive, so the tab strip renders.
    task_store().create("plan the work", CreateTaskOptions::default());
    let parent = task_store().create("parent run", sub("explore"));
    let child = task_store().create(
        "orphaned child",
        CreateTaskOptions {
            parent_task_id: Some(parent.id),
            ..sub("review")
        },
    );
    update(
        child.id,
        TaskPatch {
            status: Some(TaskStatus::Done),
            ..Default::default()
        },
    );
    task_store().remove(parent.id);
    let lines = plain(&mut panel);
    assert!(
        lines.iter().any(|l| l.contains("orphaned child")),
        "{lines:#?}"
    );
    assert!(lines[0].contains("subagents 1/1"), "{}", lines[0]);
    panel.dispose();
}

#[test]
fn terminates_on_a_parent_task_id_cycle_instead_of_walking_forever() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let a = task_store().create("node a", sub("explore"));
    let b = task_store().create(
        "node b",
        CreateTaskOptions {
            parent_task_id: Some(a.id),
            ..sub("explore")
        },
    );
    update(
        a.id,
        TaskPatch {
            parent_task_id: Some(b.id),
            ..Default::default()
        },
    );
    // Cycle members have no root, so they drop from the lens.
    let lines = plain(&mut panel);
    assert!(!lines
        .iter()
        .any(|l| l.contains("node a") || l.contains("node b")));
    panel.dispose();
}

#[test]
fn clears_a_task_note_when_the_next_update_omits_it_explicitly() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    let task = task_store().create("retry-prone work", sub("explore"));
    update(
        task.id,
        TaskPatch {
            status: Some(TaskStatus::InProgress),
            note: Some(Some("ran on inherited model".into())),
            ..Default::default()
        },
    );
    panel.set_view(TaskPanelView::Subagents);
    assert!(plain(&mut panel)
        .join("\n")
        .contains("⚠\u{fe0e} ran on inherited model"));
    // The finishing update clears the note, as finalizing a dispatch with no
    // warning does.
    update(
        task.id,
        TaskPatch {
            status: Some(TaskStatus::Done),
            note: Some(None),
            ..Default::default()
        },
    );
    assert!(!plain(&mut panel).join("\n").contains('⚠'));
    panel.dispose();
}

#[test]
fn re_renders_rows_after_a_store_mutation() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    let task = task_store().create("stable row", CreateTaskOptions::default());
    update(
        task.id,
        TaskPatch {
            status: Some(TaskStatus::Done),
            ..Default::default()
        },
    );
    assert!(plain(&mut panel).join("\n").contains("stable row"));
    update(
        task.id,
        TaskPatch {
            title: Some("renamed row".into()),
            ..Default::default()
        },
    );
    let after = plain(&mut panel).join("\n");
    assert!(after.contains("renamed row"));
    assert!(!after.contains("stable row"));
    panel.dispose();
}

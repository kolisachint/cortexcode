//! Port of the pin's `test/suite/subagent-visual-tie.test.ts`: per-agent
//! identity colors across the Task call, the task panel and TaskOutput;
//! wall-clock elapsed times; the TodoWrite ↔ dispatch link and the flat lens
//! nesting it drives; TodoWrite's reconcile by identity.
//!
//! The TS "instant pool" is a real pool here over a shell child that writes
//! a canned result. `vi.advanceTimersByTime` is
//! `cortexcode_code_task_store::advance_clock_for_tests`, which moves the
//! clock every elapsed display reads.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, Once};

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::Content;
use cortexcode_code_subagents::inbox::subagent_inbox;
use cortexcode_code_subagents::instance::set_subagent_pool_for_testing;
use cortexcode_code_subagents::pool::{SubagentPool, SubagentPoolOptions};
use cortexcode_code_subagents::tools::{
    create_task_output_tool_definition, create_task_tool_definition,
};
use cortexcode_code_task_store::{
    advance_clock_for_tests, task_store, CreateTaskOptions, TaskAgentKind, TaskAgentPatch,
    TaskAgentState, TaskPatch, TaskSource, TaskStatus, TaskUsage,
};
use cortexcode_code_tool_api::{ToolContext, ToolDefinition};
use cortexcode_code_tools_optin::todo::{create_todo_write_tool_definition, StoreRef};
use cortexcode_code_tui_theme::{agent_color_for, init_theme, theme, AGENT_COLOR_TOKENS};
use cortexcode_code_tui_widgets::task_panel::{TaskPanelComponent, TaskPanelView};
use cortexcode_code_tui_widgets::tool_execution::ToolResultView;
use cortexcode_code_tui_widgets::tools::subagent::{
    format_task_call, format_task_output_call, format_task_output_result,
};
use cortexcode_tui_render::Component;
use serde_json::{json, Value};

/// One test at a time: the store, inbox, pool and clock are process-wide.
fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("cortex-visual-tie-{}", std::process::id()));
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

fn fg_ansi(token: &str) -> String {
    theme().get_fg_ansi(token).to_string()
}

fn render(panel: &mut TaskPanelComponent) -> Vec<String> {
    panel.render(120)
}

fn find<'a>(lines: &'a [String], needle: &str) -> &'a String {
    lines
        .iter()
        .find(|l| strip(l).contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in {lines:?}"))
}

fn sub(mode: &str, agent: Option<&str>) -> CreateTaskOptions {
    CreateTaskOptions {
        source: Some(TaskSource::Subagent),
        subagent_mode: Some(mode.into()),
        agent: agent.map(str::to_string),
        ..Default::default()
    }
}

fn set_status(id: u64, status: TaskStatus) {
    task_store().update(
        id,
        TaskPatch {
            status: Some(status),
            ..Default::default()
        },
    );
}

fn running_agent(id: &str, name: &str, activity: Option<&str>) {
    task_store().upsert_agent(
        id,
        name,
        TaskAgentKind::Subagent,
        TaskAgentPatch {
            state: Some(TaskAgentState::Running),
            activity: activity.map(str::to_string),
            ..Default::default()
        },
    );
}

/// A shell child that writes a successful result for its `--task-id`.
fn instant_pool(cwd: &Path) -> SubagentPool {
    let dir = cortexcode_code_paths::CONFIG_DIR_NAME;
    let result =
        json!({"summary": "done", "files_changed": [], "confidence": 0.9, "status": "complete"});
    let exe: PathBuf = cwd.join("instant-child.sh");
    std::fs::write(
        &exe,
        format!(
            "#!/bin/sh\ntid=unknown; prev=; for a in \"$@\"; do [ \"$prev\" = \"--task-id\" ] && tid=$a; prev=$a; done\nmkdir -p {dir}/dispatch/$tid\nprintf '%s' '{result}' > {dir}/dispatch/$tid/result.json\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: exe,
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    });
    set_subagent_pool_for_testing(Some(pool.clone()));
    pool
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn execute(tool: &ToolDefinition, args: Value, cwd: &Path) -> AgentToolResult {
    let ctx = ToolContext {
        cwd: Some(cwd.to_path_buf()),
        ..Default::default()
    };
    (tool.execute)("c1".into(), args, None, None, Some(&ctx)).unwrap()
}

/// Dispatch "scan the repo" as an explore run through the Task tool.
fn dispatch_scan(cwd: &Path) {
    let rt = runtime();
    let _enter = rt.enter();
    let pool = instant_pool(cwd);
    let tool = create_task_tool_definition(cwd);
    execute(
        &tool,
        json!({"description": "scan repo", "prompt": "scan the repo", "subagent_type": "explore"}),
        cwd,
    );
    pool.dispose();
    set_subagent_pool_for_testing(None);
}

fn text_of(result: &AgentToolResult) -> String {
    match &result.content[0] {
        Content::Text(t) => t.text.clone(),
        other => panic!("{other:?}"),
    }
}

// --- identity colors -----------------------------------------------------------

#[test]
fn hashes_an_agent_type_to_a_stable_palette_token() {
    let _g = lock();
    let color = agent_color_for("explore");
    assert!(AGENT_COLOR_TOKENS.contains(&color));
    assert_eq!(agent_color_for("explore"), color);
    let distinct: std::collections::HashSet<&str> = [
        "explore",
        "plan",
        "general-purpose",
        "review",
        "edit",
        "doc",
    ]
    .into_iter()
    .map(agent_color_for)
    .collect();
    assert!(distinct.len() > 1);
}

#[test]
fn resolves_agent_palette_tokens_against_the_built_in_theme() {
    let _g = lock();
    for token in AGENT_COLOR_TOKENS {
        let _ = theme().fg(token, "x");
    }
    assert_ne!(fg_ansi("agent1"), fg_ansi("accent"));
}

#[test]
fn colors_the_task_call_line_by_agent_type() {
    let _g = lock();
    let line =
        format_task_call(&json!({"subagent_type": "explore", "description": "d", "prompt": "p"}));
    assert!(strip(&line).contains("Agent [explore]"), "{line:?}");
    assert!(line.contains(&fg_ansi(agent_color_for("explore"))));
}

#[test]
fn colors_panel_rows_and_roster_names_with_the_same_per_type_hue() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let run = task_store().create("scan the repo", sub("explore", Some("run-1")));
    running_agent("run-1", "explore#1", None);
    set_status(run.id, TaskStatus::InProgress);
    let lines = render(&mut panel);
    let row = find(&lines, "scan the repo");
    assert!(
        row.contains(&fg_ansi(agent_color_for("explore"))),
        "{row:?}"
    );
    panel.dispose();
}

#[test]
fn colors_task_output_call_target_and_roster_lines_by_agent_type() {
    let _g = lock();
    let explore = fg_ansi(agent_color_for("explore"));
    let call = format_task_output_call(&json!({"task_id": "explore#1"}));
    assert!(call.contains(&explore), "{call:?}");

    let roster = [
        "2 background subagents (1 running):",
        "- explore#1  running  34s  · grep",
        "- plan#1  done (uncollected)  1m10s — drafted the plan",
    ]
    .join("\n");
    let content = vec![Content::Text(cortexcode_ai_types::TextContent {
        text: roster,
        ..Default::default()
    })];
    let details = json!({"status": "list", "ok": true});
    let rendered = format_task_output_result(&ToolResultView {
        content: &content,
        details: &details,
    });
    assert!(rendered.contains(&explore));
    assert!(rendered.contains(&fg_ansi(agent_color_for("plan"))));
    assert!(strip(&rendered).contains("- explore#1  running  34s  · grep"));
}

// --- elapsed time --------------------------------------------------------------

#[test]
fn the_header_shows_the_wall_clock_span_not_the_sum_of_concurrent_runs() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    for name in ["run a", "run b"] {
        let t = task_store().create(name, sub("explore", None));
        task_store().update(
            t.id,
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
    }
    advance_clock_for_tests(30_000);
    let header = strip(&render(&mut panel)[0]);
    // Two runs over 30s of wall time is 30s, not 1m00s.
    assert!(header.contains("30s"), "{header}");
    assert!(!header.contains("1m00s"), "{header}");
    panel.dispose();
}

#[test]
fn each_running_row_carries_its_own_live_timer_next_to_the_activity() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let run = task_store().create("scan the repo", sub("explore", Some("run-1")));
    running_agent("run-1", "explore#1", Some("SearchCodebase"));
    set_status(run.id, TaskStatus::InProgress);
    advance_clock_for_tests(34_000);
    let lines = render(&mut panel);
    let row = strip(find(&lines, "scan the repo"));
    assert!(row.contains("⋯ SearchCodebase · 34s"), "{row}");
    panel.dispose();
}

#[test]
fn task_output_reports_elapsed_in_the_panels_format() {
    let _g = lock();
    subagent_inbox().start("t1", "explore#1", "explore");
    advance_clock_for_tests(94_000);
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let _enter = rt.enter();
    let tool = create_task_output_tool_definition();
    let text = text_of(&execute(&tool, json!({"list": true}), dir.path()));
    assert!(text.contains("1m34s"), "{text}");
    assert!(!text.contains("94s"), "{text}");
}

// --- todo ↔ subagent linkage ------------------------------------------------------

#[test]
fn links_a_dispatch_to_the_single_in_progress_plan_item() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    let todo = task_store().create("Wire the parser", CreateTaskOptions::default());
    set_status(todo.id, TaskStatus::InProgress);
    dispatch_scan(dir.path());
    let run = task_store()
        .list()
        .into_iter()
        .find(|t| t.source == Some(TaskSource::Subagent))
        .unwrap();
    assert_eq!(run.linked_task_id, Some(todo.id));
}

#[test]
fn records_no_link_when_several_plan_items_are_in_progress() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    for title in ["Step A", "Step B"] {
        let t = task_store().create(title, CreateTaskOptions::default());
        set_status(t.id, TaskStatus::InProgress);
    }
    dispatch_scan(dir.path());
    let run = task_store()
        .list()
        .into_iter()
        .find(|t| t.source == Some(TaskSource::Subagent))
        .unwrap();
    assert_eq!(run.linked_task_id, None);
}

#[test]
fn records_no_link_when_no_plan_item_is_in_progress() {
    let _g = lock();
    let dir = tempfile::tempdir().unwrap();
    task_store().create("Step A", CreateTaskOptions::default());
    dispatch_scan(dir.path());
    let run = task_store()
        .list()
        .into_iter()
        .find(|t| t.source == Some(TaskSource::Subagent))
        .unwrap();
    assert_eq!(run.linked_task_id, None);
}

#[test]
fn nests_a_linked_run_under_its_plan_item_in_the_flat_lens_and_counts_it() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    let todo = task_store().create("Wire the parser", CreateTaskOptions::default());
    set_status(todo.id, TaskStatus::InProgress);
    let other = task_store().create("Write docs", CreateTaskOptions::default());
    set_status(other.id, TaskStatus::Pending);
    let run = task_store().create(
        "scan the repo",
        CreateTaskOptions {
            linked_task_id: Some(todo.id),
            ..sub("explore", Some("run-1"))
        },
    );
    running_agent("run-1", "explore#1", Some("SearchCodebase"));
    set_status(run.id, TaskStatus::InProgress);
    // An unlinked run stays out of the flat lens entirely.
    let unlinked = task_store().create("free-floating run", sub("plan", None));
    set_status(unlinked.id, TaskStatus::InProgress);

    panel.set_view(TaskPanelView::Flat);
    let lines: Vec<String> = render(&mut panel).iter().map(|l| strip(l)).collect();
    let todo_idx = lines
        .iter()
        .position(|l| l.contains("Wire the parser"))
        .unwrap();
    let run_idx = lines
        .iter()
        .position(|l| l.contains("scan the repo"))
        .unwrap();
    assert!(todo_idx > 0);
    assert_eq!(run_idx, todo_idx + 1, "{lines:#?}");
    assert!(lines[run_idx].contains("└─"));
    assert!(lines[run_idx].contains("[explore]"));
    assert!(lines[run_idx].contains("⋯ SearchCodebase"));
    assert!(!lines.iter().any(|l| l.contains("free-floating run")));
    // The header counts what the lens shows: todo + linked run + pending todo.
    assert!(lines[0].contains("0/3"), "{}", lines[0]);

    // The run still appears in the subagents lens.
    panel.set_view(TaskPanelView::Subagents);
    let sa: Vec<String> = render(&mut panel).iter().map(|l| strip(l)).collect();
    assert!(sa.iter().any(|l| l.contains("scan the repo")));
    assert!(sa.iter().any(|l| l.contains("free-floating run")));
    panel.dispose();
}

#[test]
fn drops_a_dangling_link_from_the_flat_lens_without_losing_the_run() {
    let _g = lock();
    let mut panel = TaskPanelComponent::new();
    let todo = task_store().create("Old plan item", CreateTaskOptions::default());
    set_status(todo.id, TaskStatus::InProgress);
    let survivor = task_store().create("Still on the plan", CreateTaskOptions::default());
    set_status(survivor.id, TaskStatus::InProgress);
    let run = task_store().create(
        "scan the repo",
        CreateTaskOptions {
            linked_task_id: Some(todo.id),
            ..sub("explore", None)
        },
    );
    set_status(run.id, TaskStatus::InProgress);
    task_store().remove(todo.id);

    panel.set_view(TaskPanelView::Flat);
    let flat: Vec<String> = render(&mut panel).iter().map(|l| strip(l)).collect();
    assert!(flat.iter().any(|l| l.contains("Still on the plan")));
    assert!(!flat.iter().any(|l| l.contains("scan the repo")));
    panel.set_view(TaskPanelView::Subagents);
    let sa: Vec<String> = render(&mut panel).iter().map(|l| strip(l)).collect();
    assert!(sa.iter().any(|l| l.contains("scan the repo")));
    panel.dispose();
}

// --- TodoWrite reconcile by identity ----------------------------------------------

fn write_todos(todos: Value) {
    let tool = create_todo_write_tool_definition(StoreRef::Global);
    (tool.execute)("todo".into(), json!({"todos": todos}), None, None, None).unwrap();
}

fn plan_titles() -> Vec<String> {
    task_store()
        .list()
        .into_iter()
        .filter(|t| t.source.is_none() && t.agent.is_none() && t.parent_task_id.is_none())
        .map(|t| t.title)
        .collect()
}

#[test]
fn keeps_a_linked_run_on_its_plan_item_when_the_completed_head_is_dropped() {
    let _g = lock();
    write_todos(json!([
        {"content": "Set up parser", "status": "completed"},
        {"content": "Wire the parser", "status": "in_progress"},
    ]));
    let wire_id = task_store()
        .list()
        .into_iter()
        .find(|t| t.title == "Wire the parser")
        .unwrap()
        .id;
    let dir = tempfile::tempdir().unwrap();
    dispatch_scan(dir.path());
    let run = task_store()
        .list()
        .into_iter()
        .find(|t| t.source == Some(TaskSource::Subagent))
        .unwrap();
    assert_eq!(run.linked_task_id, Some(wire_id));

    // The model replaces the list, dropping the completed head.
    write_todos(json!([{"content": "Wire the parser", "status": "in_progress"}]));
    assert_eq!(plan_titles(), ["Wire the parser"]);
    let wire = task_store()
        .list()
        .into_iter()
        .find(|t| t.id == wire_id)
        .unwrap();
    assert_eq!(wire.title, "Wire the parser");

    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Flat);
    let lines: Vec<String> = render(&mut panel).iter().map(|l| strip(l)).collect();
    let todo_idx = lines
        .iter()
        .position(|l| l.contains("Wire the parser"))
        .unwrap();
    // The run row carries the dispatch description as its title.
    let run_idx = lines.iter().position(|l| l.contains("scan repo")).unwrap();
    assert!(todo_idx > 0);
    assert_eq!(run_idx, todo_idx + 1, "{lines:#?}");
    panel.dispose();
}

#[test]
fn matches_items_across_active_form_and_content_title_flips() {
    let _g = lock();
    write_todos(
        json!([{"content": "Add tests", "status": "in_progress", "activeForm": "Adding tests"}]),
    );
    let first = task_store().list().into_iter().next().unwrap();
    assert_eq!(first.title, "Adding tests");
    write_todos(json!([{"content": "Add tests", "status": "completed"}]));
    let task = task_store()
        .list()
        .into_iter()
        .find(|t| t.id == first.id)
        .unwrap();
    assert_eq!(task.title, "Add tests");
    assert_eq!(task.status, TaskStatus::Done);
}

#[test]
fn keeps_ids_on_reorder_and_renders_the_plan_in_the_new_order() {
    let _g = lock();
    write_todos(json!([
        {"content": "A", "status": "pending"},
        {"content": "B", "status": "pending"},
    ]));
    let ids: Vec<u64> = task_store().list().iter().map(|t| t.id).collect();
    write_todos(json!([
        {"content": "B", "status": "in_progress"},
        {"content": "A", "status": "pending"},
    ]));
    let by_title: std::collections::HashMap<String, u64> = task_store()
        .list()
        .into_iter()
        .map(|t| (t.title, t.id))
        .collect();
    assert_eq!(by_title["A"], ids[0]);
    assert_eq!(by_title["B"], ids[1]);
    assert_eq!(plan_titles(), ["B", "A"]);
}

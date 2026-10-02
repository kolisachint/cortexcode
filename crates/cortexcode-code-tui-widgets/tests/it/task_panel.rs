//! Port of `test/task-panel.test.ts` and `test/task-panel-team-focus.test.ts`.
//!
//! The store is process-global, so every test holds [`lock`] and starts from
//! a cleared store. Not ported: the two "warns on unknown id" store tests,
//! which assert `console.warn` output (the Rust store ignores unknown ids
//! without printing).

use std::sync::{Mutex, MutexGuard};

use cortexcode_code_task_store::{
    task_store, AgentStats, CreateTaskOptions, TaskAgentKind, TaskAgentPatch, TaskAgentState,
    TaskPatch, TaskSource, TaskStatus, TaskUsage,
};
use cortexcode_code_tui_keybindings::app_key_label;
use cortexcode_code_tui_theme::{init_theme, theme};
use cortexcode_code_tui_widgets::task_panel::{TaskPanelComponent, TaskPanelEvent, TaskPanelView};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_theme(Some("dark"), false);
    task_store().clear();
    guard
}

fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn render(panel: &mut TaskPanelComponent, width: u16) -> Vec<String> {
    panel.render(width)
}

fn plain(panel: &mut TaskPanelComponent, width: u16) -> Vec<String> {
    panel.render(width).iter().map(|l| strip(l)).collect()
}

fn create(title: &str) -> u64 {
    task_store().create(title, CreateTaskOptions::default()).id
}

fn create_with(title: &str, options: CreateTaskOptions) -> u64 {
    task_store().create(title, options).id
}

fn sub(mode: &str) -> CreateTaskOptions {
    CreateTaskOptions {
        source: Some(TaskSource::Subagent),
        subagent_mode: Some(mode.into()),
        ..Default::default()
    }
}

fn mcp(server: Option<&str>) -> CreateTaskOptions {
    CreateTaskOptions {
        source: Some(TaskSource::Mcp),
        subagent_mode: server.map(String::from),
        ..Default::default()
    }
}

fn mode(mode: &str) -> CreateTaskOptions {
    CreateTaskOptions {
        subagent_mode: Some(mode.into()),
        ..Default::default()
    }
}

fn owned(agent: &str) -> CreateTaskOptions {
    CreateTaskOptions {
        agent: Some(agent.into()),
        ..Default::default()
    }
}

fn set(id: u64, status: TaskStatus) {
    task_store().update(
        id,
        TaskPatch {
            status: Some(status),
            ..Default::default()
        },
    );
}

fn role(id: &str, name: &str, state: Option<TaskAgentState>) {
    task_store().upsert_agent(
        id,
        name,
        TaskAgentKind::Role,
        TaskAgentPatch {
            state,
            ..Default::default()
        },
    );
}

fn usage(input: f64, output: f64, cost: f64) -> TaskUsage {
    TaskUsage {
        input,
        output,
        cost,
        ..Default::default()
    }
}

fn find<'a>(lines: &'a [String], needle: &str) -> &'a String {
    lines
        .iter()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no row with {needle:?} in {lines:#?}"))
}

#[test]
fn collapses_to_empty_when_there_are_no_tasks() {
    let _g = lock();
    assert!(render(&mut TaskPanelComponent::new(), 120).is_empty());
}

#[test]
fn shows_active_subagent_tasks_with_status_icons_ids_and_mode_tags() {
    let _g = lock();
    let explore = create_with("SSE watch endpoint", mode("explore"));
    let edit = create_with("Auth refactor", mode("edit"));
    let plain_task = create("Init project");
    set(explore, TaskStatus::InProgress);
    set(edit, TaskStatus::Pending);
    set(plain_task, TaskStatus::InProgress);

    let lines = render(&mut TaskPanelComponent::new(), 120);
    assert_eq!(lines.len(), 3);
    let text = lines.join("\n");
    for s in [
        "SSE watch endpoint",
        "Auth refactor",
        "Init project",
        "[explore]",
        "[edit]",
        "◐",
        "○",
    ] {
        assert!(text.contains(s), "{s}");
    }
    for s in ["●", "⠋", "queued", "running…", "◆"] {
        assert!(!text.contains(s), "{s}");
    }
}

#[test]
fn each_lens_carries_the_owner_glyph_and_tag_for_its_own_rows() {
    let _g = lock();
    role("team:planner", "planner", Some(TaskAgentState::Running));
    let s = create_with("find the bug", sub("explore"));
    let m = create_with("fetch", mcp(Some("web")));
    let r = create_with("draft plan", owned("team:planner"));
    let p = create("init project");
    for id in [s, m, r, p] {
        set(id, TaskStatus::InProgress);
    }
    let mut panel = TaskPanelComponent::new();

    panel.set_view(TaskPanelView::Flat);
    let flat = render(&mut panel, 120);
    let plain_row = find(&flat, "init project");
    assert!(!plain_row.contains('◆') && !plain_row.contains('◇'));
    assert!(!flat.iter().any(|l| l.contains("find the bug")));
    assert!(!flat.iter().any(|l| l.contains("fetch")));

    panel.set_view(TaskPanelView::Subagents);
    let sa = render(&mut panel, 120);
    let sub_row = find(&sa, "find the bug");
    let mcp_row = find(&sa, "fetch");
    assert!(sub_row.contains('◇') && sub_row.contains("[explore]"));
    assert!(mcp_row.contains('⧉') && mcp_row.contains("[web]"));
    for row in &sa {
        assert_eq!(visible_width(row), 120);
    }

    panel.set_view(TaskPanelView::Teams);
    let teams = render(&mut panel, 120);
    assert!(teams
        .iter()
        .any(|l| l.contains('▸') && l.contains("planner")));
    assert!(teams.iter().any(|l| l.contains("draft plan")));
}

#[test]
fn an_mcp_row_without_a_recorded_server_falls_back_to_the_mcp_tag() {
    let _g = lock();
    let m = create_with("legacy call", mcp(None));
    set(m, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let text = render(&mut panel, 120).join("\n");
    assert!(text.contains('⧉') && text.contains("[MCP]"));
}

#[test]
fn subagents_tree_keeps_each_rows_mode_and_server_tag() {
    let _g = lock();
    let s = create_with("find the bug", sub("explore"));
    let m = create_with("fetch", mcp(Some("web")));
    set(s, TaskStatus::InProgress);
    set(m, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    assert!(find(&lines, "find the bug").contains("[explore]"));
    assert!(find(&lines, "fetch").contains("[web]"));
}

fn explore_agent(activity: &str) {
    task_store().upsert_agent(
        "explore",
        "explore",
        TaskAgentKind::Subagent,
        TaskAgentPatch {
            state: Some(TaskAgentState::Running),
            activity: Some(activity.into()),
            ..Default::default()
        },
    );
}

fn explore_run() -> u64 {
    let id = create_with(
        "trace the auth flow",
        CreateTaskOptions {
            agent: Some("explore".into()),
            ..sub("explore")
        },
    );
    set(id, TaskStatus::InProgress);
    id
}

#[test]
fn an_in_progress_subagent_row_shows_the_owners_live_tool_activity() {
    let _g = lock();
    explore_agent("SearchCodebase");
    explore_run();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    let row = find(&lines, "trace the auth flow");
    assert!(row.contains("⋯ SearchCodebase · "), "{row}");
    let after = row.split("⋯ SearchCodebase · ").nth(1).unwrap();
    assert!(after.starts_with(|c: char| c.is_ascii_digit()), "{row}");
}

#[test]
fn an_idle_in_progress_subagent_row_keeps_its_run_clock() {
    let _g = lock();
    explore_agent("");
    explore_run();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    let row = find(&lines, "trace the auth flow").trim_end().to_string();
    assert!(row.ends_with('s'), "{row}");
    let clock = row.rsplit(' ').next().unwrap();
    assert!(clock.trim_end_matches('s').parse::<f64>().is_ok(), "{row}");
    assert!(!row.contains('⋯') && !row.contains("running…"));
}

#[test]
fn an_in_progress_main_plan_row_carries_no_run_clock() {
    let _g = lock();
    let t = create("Patch the warning threshold");
    set(t, TaskStatus::InProgress);
    let lines = plain(&mut TaskPanelComponent::new(), 120);
    let row = find(&lines, "Patch the warning threshold");
    assert!(row.trim_end().ends_with("threshold"), "{row}");
}

#[test]
fn title_column_stays_aligned_across_single_and_double_digit_ids() {
    let _g = lock();
    for i in 1..=10 {
        let t = create(&format!("task-{i}"));
        set(t, TaskStatus::InProgress);
    }
    let lines = plain(&mut TaskPanelComponent::new(), 120);
    let row1 = lines
        .iter()
        .find(|l| l.contains("task-1") && !l.contains("task-10"))
        .unwrap();
    let row10 = find(&lines, "task-10");
    assert_eq!(row1.find("task-1"), row10.find("task-10"));
}

#[test]
fn completed_and_failed_tasks_stay_visible_with_their_status() {
    let _g = lock();
    create_with("Still running", mode("explore"));
    let d = create("Finished work");
    set(d, TaskStatus::Done);
    let f = create("Broken build");
    set(f, TaskStatus::Failed);
    let lines = render(&mut TaskPanelComponent::new(), 120);
    assert_eq!(lines.len(), 3);
    let text = lines.join("\n");
    for s in ["Still running", "Finished work", "Broken build", "✓", "✗"] {
        assert!(text.contains(s), "{s}");
    }
}

#[test]
fn a_task_note_renders_as_a_warning_cue_replacing_the_usage_stamp() {
    let _g = lock();
    let d = create("Audit reconnect path");
    task_store().update(
        d,
        TaskPatch {
            status: Some(TaskStatus::Done),
            usage: Some(usage(4000.0, 500.0, 0.0)),
            note: Some(Some("ran on inherited model".into())),
            ..Default::default()
        },
    );
    let s = create("Run suite");
    task_store().update(
        s,
        TaskPatch {
            status: Some(TaskStatus::Failed),
            note: Some(Some("anthropic exhausted".into())),
            ..Default::default()
        },
    );
    let lines = render(&mut TaskPanelComponent::new(), 120);
    let note_row = find(&lines, "Audit reconnect path");
    assert!(note_row.contains("\u{26a0}\u{fe0e} ran on inherited model"));
    assert!(!note_row.contains("4.5k"));
    assert!(find(&lines, "Run suite").contains("\u{26a0}\u{fe0e} anthropic exhausted"));
}

#[test]
fn long_titles_use_the_full_left_width() {
    let _g = lock();
    let a =
        create("A deliberately long task title that should fill the row up to the right column");
    set(a, TaskStatus::InProgress);
    let lines = render(&mut TaskPanelComponent::new(), 80);
    let row = lines
        .iter()
        .find(|l| strip(l).contains("deliberately long"))
        .unwrap();
    assert_eq!(visible_width(row), 80);
    assert!(strip(row).contains("should fill the row"), "{}", strip(row));
}

#[test]
fn finished_tasks_show_combined_token_usage_per_row() {
    let _g = lock();
    let d = create("Investigate flaky test");
    task_store().update(
        d,
        TaskPatch {
            status: Some(TaskStatus::Done),
            usage: Some(usage(9000.0, 1100.0, 0.02)),
            ..Default::default()
        },
    );
    let lines = render(&mut TaskPanelComponent::new(), 120);
    let row = find(&lines, "Investigate flaky test");
    assert!(row.contains("10k"));
    assert!(!row.contains('↑') && !row.contains('↓'));
}

#[test]
fn the_pane_carries_no_turn_token_or_cost_delta() {
    let _g = lock();
    let a = create_with("Explore module", sub("explore"));
    task_store().update(
        a,
        TaskPatch {
            status: Some(TaskStatus::Done),
            usage: Some(usage(3000.0, 500.0, 0.01)),
            ..Default::default()
        },
    );
    let b = create("Run tests");
    set(b, TaskStatus::Done);
    let header = strip(&render(&mut TaskPanelComponent::new(), 120)[0]);
    for s in ["turn", "$", "━", "WORKING", "REVIEWED"] {
        assert!(!header.contains(s), "{s}");
    }
}

fn plan_and_done_run() -> u64 {
    let plan = create("Plain work");
    set(plan, TaskStatus::InProgress);
    let s = create_with("trace the auth flow", sub("explore"));
    set(s, TaskStatus::Done);
    plan
}

#[test]
fn each_tab_carries_its_own_lens_count() {
    let _g = lock();
    plan_and_done_run();
    let header = strip(&render(&mut TaskPanelComponent::new(), 120)[0]);
    assert!(header.contains("tasks 0/1"), "{header}");
    assert!(header.contains("subagents 1/1"), "{header}");
}

#[test]
fn a_filled_tab_marks_the_selected_lens_only_while_it_has_live_work() {
    let _g = lock();
    let plan = plan_and_done_run();
    let bg = theme().get_bg_ansi("selectedBg").to_string();
    let mut panel = TaskPanelComponent::new();
    assert!(render(&mut panel, 120)[0].contains(&bg));
    set(plan, TaskStatus::Done);
    assert!(!render(&mut panel, 120)[0].contains(&bg));
}

#[test]
fn header_keeps_the_tab_strip_and_drops_the_cycle_hint_when_narrow() {
    let _g = lock();
    let plan = create("Explore module");
    set(plan, TaskStatus::Done);
    let s = create_with("trace the auth flow", sub("explore"));
    set(s, TaskStatus::Done);
    let header = render(&mut TaskPanelComponent::new(), 20)[0].clone();
    assert!(visible_width(&header) <= 20);
    assert!(!strip(&header).contains("cycle"));
    assert!(strip(&header).contains("tasks"));
}

#[test]
fn reset_clears_finished_tasks_and_restarts_numbering() {
    let _g = lock();
    let d = create("Finished work");
    set(d, TaskStatus::Done);
    let f = create("Broken build");
    set(f, TaskStatus::Failed);
    task_store().reset();
    let mut panel = TaskPanelComponent::new();
    assert!(render(&mut panel, 120).is_empty());
    let next = create("Fresh task");
    assert_eq!(next, 1);
    assert!(plain(&mut panel, 120).join("\n").contains("Fresh task"));
}

#[test]
fn reset_keeps_active_tasks_and_their_numbering() {
    let _g = lock();
    let running = create_with("Still running", mode("explore"));
    set(running, TaskStatus::InProgress);
    let d = create("Finished work");
    set(d, TaskStatus::Done);
    task_store().reset();
    let mut panel = TaskPanelComponent::new();
    let text = render(&mut panel, 120).join("\n");
    assert!(text.contains("Still running") && !text.contains("Finished work"));
    set(running, TaskStatus::Done);
    assert!(render(&mut panel, 120).join("\n").contains("Still running"));
    let next = create("Next task");
    assert!(next != 1 && next > running);
}

#[test]
fn shows_all_tasks_without_a_line_limit() {
    let _g = lock();
    for i in 0..6 {
        let t = create(&format!("Task {i}"));
        set(t, TaskStatus::InProgress);
    }
    let lines = render(&mut TaskPanelComponent::new(), 120);
    assert_eq!(lines.len(), 6);
    let text = lines.join("\n");
    assert!(text.contains("Task 0") && text.contains("Task 5"));
}

#[test]
fn the_header_exists_only_when_there_is_more_than_one_lens() {
    let _g = lock();
    let t = create("Plain work");
    set(t, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    assert!(strip(&render(&mut panel, 120)[0]).contains("Plain work"));
    assert_eq!(render(&mut panel, 120).len(), 1);

    create_with("find the bug", sub("explore"));
    let two = strip(&render(&mut panel, 120)[0]);
    assert!(
        two.contains("tasks 0/1") && two.contains("subagents 0/1"),
        "{two}"
    );
    assert!(!two.contains("teams"));
    assert!(
        two.contains(&format!(
            "{} cycle",
            // Display text: "option+l" on macOS.
            cortexcode_code_tui_keybindings::format_key_text(
                &app_key_label("app.tasks.cycleForward"),
                false
            )
        )),
        "{two}"
    );

    task_store().upsert_agent(
        "planner",
        "planner",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    assert!(strip(&render(&mut panel, 120)[0]).contains("teams 0/0"));
}

#[test]
fn cycle_view_advances_through_the_lenses_that_have_content() {
    let _g = lock();
    create("plan the work");
    create_with("find the bug", sub("explore"));
    task_store().upsert_agent(
        "planner",
        "planner",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    let mut panel = TaskPanelComponent::new();
    assert_eq!(panel.get_view(), TaskPanelView::Flat);
    assert_eq!(panel.cycle_view(true), TaskPanelView::Subagents);
    assert_eq!(panel.cycle_view(true), TaskPanelView::Teams);
    assert_eq!(panel.cycle_view(true), TaskPanelView::Flat);
}

#[test]
fn cycle_view_skips_empty_lenses() {
    let _g = lock();
    let t = create("Plain work");
    set(t, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    assert_eq!(panel.cycle_view(true), TaskPanelView::Flat);
    create_with("find the bug", sub("explore"));
    assert_eq!(panel.cycle_view(true), TaskPanelView::Subagents);
    assert_eq!(panel.cycle_view(true), TaskPanelView::Flat);
}

#[test]
fn a_selected_lens_that_empties_falls_back_to_flat_rendering() {
    let _g = lock();
    let t = create("Plain work");
    set(t, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Teams);
    let lines = plain(&mut panel, 120);
    assert!(lines.join("\n").contains("Plain work"));
    assert_eq!(lines.len(), 1);
    assert_eq!(panel.cycle_view(true), TaskPanelView::Flat);
}

#[test]
fn subagents_view_renders_a_recursive_task_tree() {
    let _g = lock();
    let explore = create_with("Explore module", sub("explore"));
    set(explore, TaskStatus::Done);
    let review = create_with(
        "Review findings",
        CreateTaskOptions {
            parent_task_id: Some(explore),
            ..sub("review")
        },
    );
    set(review, TaskStatus::Done);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 2);
    let explore_row = find(&lines, "Explore module");
    let review_row = find(&lines, "Review findings");
    assert!(!explore_row.contains("└─") && !explore_row.contains("├─"));
    assert!(explore_row.contains("[explore]"));
    assert!(review_row.contains("└─") && review_row.contains("[review]"));
    assert!(review_row.find("Review findings") > explore_row.find("Explore module"));
}

#[test]
fn subagents_view_draws_branch_connectors_for_siblings() {
    let _g = lock();
    let explore = create_with("Explore module", sub("explore"));
    set(explore, TaskStatus::Done);
    let child = |title: &str, m: &str| {
        let id = create_with(
            title,
            CreateTaskOptions {
                parent_task_id: Some(explore),
                ..sub(m)
            },
        );
        set(id, TaskStatus::Done);
    };
    child("First child", "edit");
    child("Second child", "review");
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    assert!(find(&lines, "First child").contains("├─"));
    assert!(find(&lines, "Second child").contains("└─"));
}

#[test]
fn subagents_tree_with_only_top_level_tasks_renders_flat() {
    let _g = lock();
    let a = create_with("Explore A", sub("explore"));
    let b = create_with("Explore B", sub("explore"));
    set(a, TaskStatus::InProgress);
    set(b, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 2);
    let (ra, rb) = (find(&lines, "Explore A"), find(&lines, "Explore B"));
    for row in [ra, rb] {
        assert!(!row.contains("└─") && !row.contains("├─"));
    }
    assert_eq!(ra.find("Explore A"), rb.find("Explore B"));
}

fn plan_sub_mcp(sub_status: TaskStatus, mcp_status: TaskStatus) {
    let plan = create("Write the plan");
    set(plan, TaskStatus::InProgress);
    let s = create_with("Explore the API", sub("explore"));
    set(s, sub_status);
    let m = create_with("Fetch the spec", mcp(Some("web")));
    set(m, mcp_status);
}

#[test]
fn the_tasks_and_subagents_lenses_split_work_by_ownership() {
    let _g = lock();
    plan_sub_mcp(TaskStatus::InProgress, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Flat);
    let flat = plain(&mut panel, 120).join("\n");
    assert!(flat.contains("Write the plan"));
    assert!(!flat.contains("Explore the API") && !flat.contains("Fetch the spec"));
    panel.set_view(TaskPanelView::Subagents);
    let sa = plain(&mut panel, 120).join("\n");
    assert!(!sa.contains("Write the plan"));
    assert!(sa.contains("Explore the API") && sa.contains("Fetch the spec"));
}

#[test]
fn each_tabs_count_is_scoped_to_its_own_lens() {
    let _g = lock();
    plan_sub_mcp(TaskStatus::Done, TaskStatus::Done);
    let mut panel = TaskPanelComponent::new();
    for view in [TaskPanelView::Flat, TaskPanelView::Subagents] {
        panel.set_view(view);
        let header = strip(&render(&mut panel, 120)[0]);
        assert!(header.contains("tasks 0/1"), "{header}");
        assert!(header.contains("subagents 2/2"), "{header}");
        assert!(!header.contains("0/3"));
    }
}

#[test]
fn an_empty_flat_lens_falls_through_to_the_subagents_tree() {
    let _g = lock();
    let s = create_with("Explore the API", sub("explore"));
    set(s, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    assert_eq!(panel.get_view(), TaskPanelView::Flat);
    assert!(plain(&mut panel, 120)
        .join("\n")
        .contains("Explore the API"));
}

#[test]
fn teams_view_renders_role_agents_with_states_and_handoff_arrows() {
    let _g = lock();
    task_store().upsert_agent(
        "planner",
        "planner",
        TaskAgentKind::Role,
        TaskAgentPatch {
            role: Some("architect".into()),
            state: Some(TaskAgentState::Done),
            handoff: Some("→ builder".into()),
            stats: Some(AgentStats {
                input: 1400.0,
                output: 260.0,
                cost: 0.004,
            }),
            ..Default::default()
        },
    );
    task_store().upsert_agent(
        "builder",
        "builder",
        TaskAgentKind::Role,
        TaskAgentPatch {
            role: Some("engineer".into()),
            state: Some(TaskAgentState::Active),
            ..Default::default()
        },
    );
    let draft = create_with("Draft the retry design", owned("planner"));
    set(draft, TaskStatus::Done);
    let imp = create_with("Implement withRetry()", owned("builder"));
    set(imp, TaskStatus::InProgress);

    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Teams);
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 6, "{lines:#?}");
    let planner = &lines[1];
    for s in [
        "▸ planner",
        "· architect",
        "[done]",
        "→ builder",
        "↑1.4k ↓260 · $0.004",
    ] {
        assert!(planner.contains(s), "{s} in {planner}");
    }
    assert!(lines
        .iter()
        .any(|l| l.contains("└──→") && l.contains("builder")));
    let builder = &lines[4];
    assert!(builder.contains("▸ builder") && builder.contains("[active]"));
    assert!(builder.trim_end().ends_with("0/1"), "{builder}");
    for line in render(&mut panel, 120) {
        assert_eq!(visible_width(&line), 120);
    }
}

#[test]
fn subagents_tree_shows_delegated_work_and_excludes_role_tasks() {
    let _g = lock();
    task_store().upsert_agent(
        "planner",
        "planner",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    let w = create_with(
        "worker task",
        CreateTaskOptions {
            agent: Some("worker".into()),
            ..sub("build")
        },
    );
    set(w, TaskStatus::InProgress);
    let p = create_with("planner task", owned("planner"));
    set(p, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let text = plain(&mut panel, 120).join("\n");
    assert!(!text.contains("planner task"));
    assert!(text.contains("worker task"));
}

#[test]
fn teams_view_hides_non_role_agent_groups() {
    let _g = lock();
    let store = task_store();
    store.upsert_agent(
        "main-ag",
        "main-ag",
        TaskAgentKind::Main,
        TaskAgentPatch::default(),
    );
    store.upsert_agent(
        "worker",
        "worker",
        TaskAgentKind::Subagent,
        TaskAgentPatch::default(),
    );
    store.upsert_agent(
        "architect",
        "architect",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    for (title, agent) in [
        ("main task", "main-ag"),
        ("worker task", "worker"),
        ("arch task", "architect"),
    ] {
        let id = create_with(title, owned(agent));
        set(id, TaskStatus::InProgress);
    }
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Teams);
    let text = plain(&mut panel, 120).join("\n");
    assert!(!text.contains("main task") && !text.contains("worker task"));
    assert!(text.contains("arch task") && text.contains("architect"));
}

#[test]
fn teams_view_renders_queued_and_idle_role_placeholders() {
    let _g = lock();
    task_store().upsert_agent(
        "queued-role",
        "queued-role",
        TaskAgentKind::Role,
        TaskAgentPatch {
            role: Some("pending".into()),
            state: Some(TaskAgentState::Queued),
            ..Default::default()
        },
    );
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Teams);
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 2);
    assert!(lines
        .iter()
        .any(|l| l.contains("queued-role") && l.contains("0/0")));

    task_store().clear();
    role("team:planner", "planner", Some(TaskAgentState::Idle));
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 2);
    assert!(lines
        .iter()
        .any(|l| l.contains("planner") && l.contains("[idle]") && l.contains("0/0")));
}

#[test]
fn empty_flat_view_falls_through_to_the_team_roster_at_startup() {
    let _g = lock();
    role("team:planner", "planner", Some(TaskAgentState::Idle));
    role("team:builder", "builder", Some(TaskAgentState::Idle));
    let mut panel = TaskPanelComponent::new();
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 3);
    assert!(lines.iter().any(|l| l.contains("planner")));
    assert!(lines.iter().any(|l| l.contains("builder")));
    let t = create("Init project");
    set(t, TaskStatus::InProgress);
    let flat = plain(&mut panel, 120);
    assert!(flat.iter().any(|l| l.contains("Init project")));
    assert!(!flat.iter().any(|l| l.contains("planner")));
}

#[test]
fn subagents_tree_renders_delegated_roots_without_a_roster() {
    let _g = lock();
    let s = create_with("find the bug", sub("explore"));
    set(s, TaskStatus::InProgress);
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Subagents);
    let lines = plain(&mut panel, 120);
    assert_eq!(lines.len(), 1);
    for s in ["find the bug", "[explore]", "◇"] {
        assert!(lines[0].contains(s), "{s}");
    }
}

#[test]
fn reset_drops_the_roster_with_the_tasks_but_keeps_owners_of_live_tasks() {
    let _g = lock();
    let store = task_store();
    for (id, state) in [
        ("explore", TaskAgentState::Running),
        ("review", TaskAgentState::Done),
    ] {
        store.upsert_agent(
            id,
            id,
            TaskAgentKind::Subagent,
            TaskAgentPatch {
                role: Some("subagent".into()),
                state: Some(state),
                ..Default::default()
            },
        );
    }
    let live = create_with(
        "Still running",
        CreateTaskOptions {
            source: Some(TaskSource::Subagent),
            agent: Some("explore".into()),
            ..Default::default()
        },
    );
    set(live, TaskStatus::InProgress);
    let done = create_with(
        "Finished work",
        CreateTaskOptions {
            source: Some(TaskSource::Subagent),
            agent: Some("review".into()),
            ..Default::default()
        },
    );
    set(done, TaskStatus::Done);
    store.reset();
    let ids: Vec<String> = store.agents().into_iter().map(|a| a.id).collect();
    assert_eq!(ids, vec!["explore"]);
    set(live, TaskStatus::Done);
    store.reset();
    assert!(store.agents().is_empty());
}

#[test]
fn add_agent_stats_accumulates_usage_across_dispatches() {
    let _g = lock();
    let store = task_store();
    store.upsert_agent(
        "explore",
        "explore",
        TaskAgentKind::Subagent,
        TaskAgentPatch::default(),
    );
    store.add_agent_stats(
        "explore",
        AgentStats {
            input: 1000.0,
            output: 200.0,
            cost: 0.01,
        },
    );
    store.add_agent_stats(
        "explore",
        AgentStats {
            input: 500.0,
            output: 100.0,
            cost: 0.005,
        },
    );
    let stats = store.agents()[0].stats.unwrap();
    assert_eq!((stats.input, stats.output), (1500.0, 300.0));
    assert!((stats.cost - 0.015).abs() < 1e-12);
    store.upsert_agent(
        "explore",
        "explore",
        TaskAgentKind::Subagent,
        TaskAgentPatch {
            state: Some(TaskAgentState::Running),
            ..Default::default()
        },
    );
    assert_eq!(store.agents()[0].stats.unwrap().input, 1500.0);
}

#[test]
fn reset_preserves_role_agents_with_stats_but_drops_settled_subagent_runs() {
    let _g = lock();
    let store = task_store();
    let dummy = create("dummy");
    set(dummy, TaskStatus::Done);
    store.upsert_agent(
        "team:planner",
        "planner",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    store.add_agent_stats(
        "team:planner",
        AgentStats {
            input: 100.0,
            output: 50.0,
            cost: 0.001,
        },
    );
    store.upsert_agent(
        "run-1",
        "explore#1",
        TaskAgentKind::Subagent,
        TaskAgentPatch {
            state: Some(TaskAgentState::Done),
            ..Default::default()
        },
    );
    store.add_agent_stats(
        "run-1",
        AgentStats {
            input: 200.0,
            output: 20.0,
            cost: 0.002,
        },
    );
    store.reset();
    assert!(store.agents().iter().any(|a| a.id == "team:planner"));
    assert!(!store.agents().iter().any(|a| a.id == "run-1"));

    store.upsert_agent(
        "team:zero",
        "zero",
        TaskAgentKind::Role,
        TaskAgentPatch::default(),
    );
    let dummy2 = create("dummy2");
    set(dummy2, TaskStatus::Done);
    store.reset();
    assert!(!store.agents().iter().any(|a| a.id == "team:zero"));
    assert!(store.agents().iter().any(|a| a.id == "team:planner"));
}

// --- task-panel-team-focus.test.ts ------------------------------------------

const DOWN: &str = "\x1b[B";
const UP: &str = "\x1b[A";

fn team() {
    role("team:planner", "planner", Some(TaskAgentState::Idle));
    role("team:coder", "coder", Some(TaskAgentState::Active));
}

#[test]
fn focused_panel_renders_the_roster_with_a_cursor_on_the_selected_role() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.focused = true;
    let lines = plain(&mut panel, 80);
    assert!(find(&lines, "planner").contains('▶'));
    assert!(find(&lines, "coder").contains('▸'));
    assert!(lines.join("\n").contains("n nudge"));
}

#[test]
fn unfocused_panel_keeps_the_plain_role_glyph_and_no_hints() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.set_view(TaskPanelView::Teams);
    let text = plain(&mut panel, 80).join("\n");
    assert!(!text.contains('▶') && !text.contains("n nudge"));
}

#[test]
fn up_and_down_move_the_cursor_and_clamp_at_the_edges() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.focused = true;
    assert_eq!(panel.focused_role().as_deref(), Some("planner"));
    for (key, expected) in [
        (DOWN, "coder"),
        (DOWN, "coder"),
        (UP, "planner"),
        (UP, "planner"),
    ] {
        panel.handle_input(key);
        assert_eq!(panel.focused_role().as_deref(), Some(expected));
    }
}

#[test]
fn n_nudges_and_a_attaches_the_focused_role_q_and_escape_exit() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.focused = true;
    panel.handle_input("n");
    panel.handle_input(DOWN);
    panel.handle_input("a");
    panel.handle_input("q");
    panel.handle_input("\x1b");
    assert_eq!(
        panel.take_events(),
        vec![
            TaskPanelEvent::Nudge("planner".into()),
            TaskPanelEvent::Attach("coder".into()),
            TaskPanelEvent::ExitFocus,
            TaskPanelEvent::ExitFocus,
        ]
    );
}

#[test]
fn the_cursor_follows_its_role_by_name_when_the_roster_reorders() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.focused = true;
    panel.handle_input(DOWN);
    assert_eq!(panel.focused_role().as_deref(), Some("coder"));
    task_store().clear();
    role("team:coder", "coder", Some(TaskAgentState::Active));
    role("team:planner", "planner", Some(TaskAgentState::Idle));
    assert_eq!(panel.focused_role().as_deref(), Some("coder"));
}

#[test]
fn an_emptied_roster_exits_focus_instead_of_trapping_the_keyboard() {
    let _g = lock();
    team();
    let mut panel = TaskPanelComponent::new();
    panel.focused = true;
    task_store().clear();
    panel.handle_input(DOWN);
    assert_eq!(panel.take_events(), vec![TaskPanelEvent::ExitFocus]);
}

//! `components/task-panel.ts`: the task ledger above the prompt.
//!
//! A state-coloured left rail (working / reviewed / stopped), a lens tab strip
//! when more than one lens has content, and the rows of the chosen lens:
//! - flat ("tasks"): the main agent's TodoWrite plan, each dispatched run
//!   nested under the plan item it works on;
//! - subagents: the delegated forest (`parentTaskId`), drawn with connectors;
//! - teams: role agents as groups, with handoff arrows and a focusable roster.
//!
//! The pin memoizes rows per store version; that is only an optimization and
//! is not reproduced. Its 1s run clock is [`TaskPanelComponent::ticking`]: the
//! app re-renders every second while it is true.

use std::collections::{HashMap, HashSet};

use cortexcode_code_agent_session::format::{format_duration_secs, format_tokens};
use cortexcode_code_task_store::{
    task_owner_id, task_store, Task, TaskAgent, TaskAgentKind, TaskAgentState, TaskSource,
    TaskStatus,
};
use cortexcode_code_tui_keybindings::{
    app_key_label, format_key_text, matches_app_key, raw_key_hint,
};
use cortexcode_code_tui_theme::{agent_color_for, theme};
use cortexcode_tui_keys::{get_keybindings, matches_key};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};

fn task_status_icon(status: TaskStatus) -> &'static str {
    // Hollow = not started; ● stays the chat's tool status dot.
    match status {
        TaskStatus::Pending => "○",
        TaskStatus::InProgress => "◐",
        TaskStatus::Done => "✓",
        TaskStatus::Failed => "✗",
        TaskStatus::Cancelled => "⊘",
    }
}

/// U+26A0 with VS15, so terminals draw it one cell wide.
const WARNING_GLYPH: &str = "⚠\u{fe0e}";
/// U+25B6 with VS15.
const SELECTED_GLYPH: &str = "▶\u{fe0e}";
/// Marker for MCP-sourced rows, which have no owning agent.
const MCP_SOURCE_GLYPH: &str = "⧉";
/// The thin left rail that groups the pane.
const RAIL: &str = "▎";
/// Indent under a group header, with a faint guide.
const GROUP_INDENT_PLAIN: &str = "│ ";

/// `TaskPanelView`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPanelView {
    Flat,
    Subagents,
    Teams,
}

impl TaskPanelView {
    fn label(self) -> &'static str {
        match self {
            Self::Flat => "tasks",
            Self::Subagents => "subagents",
            Self::Teams => "teams",
        }
    }
}

/// `TaskPanelDensity`: every row, or the counts alone on one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPanelDensity {
    Full,
    Summary,
}

/// The main agent's own TodoWrite plan item.
fn is_main_task(task: &Task) -> bool {
    task.source.is_none() && task.agent.is_none() && task.parent_task_id.is_none()
}

/// A dispatched subagent run or an MCP call.
fn is_delegated(task: &Task) -> bool {
    task.source.is_some()
}

/// The lenses with content, in cycle order.
fn available_views(tasks: &[Task], agents: &[TaskAgent]) -> Vec<TaskPanelView> {
    let mut views = Vec::new();
    if tasks.iter().any(is_main_task) {
        views.push(TaskPanelView::Flat);
    }
    let has_subagent_work = agents.iter().any(|a| a.kind == TaskAgentKind::Subagent)
        || tasks.iter().any(|t| t.source.is_some());
    if has_subagent_work {
        views.push(TaskPanelView::Subagents);
    }
    if agents.iter().any(|a| a.kind == TaskAgentKind::Role) {
        views.push(TaskPanelView::Teams);
    }
    views
}

fn agent_glyph(kind: TaskAgentKind) -> &'static str {
    match kind {
        TaskAgentKind::Main => "◆",
        TaskAgentKind::Subagent => "◇",
        TaskAgentKind::Role => "▸",
    }
}

fn agent_glyph_color(kind: TaskAgentKind) -> &'static str {
    match kind {
        TaskAgentKind::Role => "borderAccent",
        _ => "accent",
    }
}

fn agent_state_color(state: TaskAgentState) -> &'static str {
    match state {
        TaskAgentState::Active | TaskAgentState::Running => "warning",
        TaskAgentState::Done => "success",
        TaskAgentState::Queued | TaskAgentState::Idle | TaskAgentState::Cancelled => "dim",
        TaskAgentState::Waiting => "mdLink",
        TaskAgentState::Failed => "error",
    }
}

fn agent_state_label(state: TaskAgentState) -> &'static str {
    match state {
        TaskAgentState::Active => "active",
        TaskAgentState::Running => "running",
        TaskAgentState::Done => "done",
        TaskAgentState::Queued => "queued",
        TaskAgentState::Idle => "idle",
        TaskAgentState::Waiting => "waiting",
        TaskAgentState::Failed => "failed",
        TaskAgentState::Cancelled => "cancelled",
    }
}

/// The rail colour for the lens's overall state.
fn panel_state_color(tasks: &[Task]) -> &'static str {
    if tasks.iter().any(|t| t.status == TaskStatus::Failed) {
        "error"
    } else if tasks.iter().any(|t| t.status.is_active()) {
        "warning"
    } else {
        "success"
    }
}

fn task_status_color(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::InProgress => "warning",
        TaskStatus::Done => "success",
        TaskStatus::Failed => "error",
        // A user-initiated cancel is not an error.
        TaskStatus::Pending | TaskStatus::Cancelled => "dim",
    }
}

fn now_ms() -> u64 {
    cortexcode_code_task_store::now_ms()
}

/// Wall-clock time a delegated task occupied (running ones against now).
fn task_elapsed_secs(task: &Task, now: u64) -> f64 {
    let end = if task.status == TaskStatus::InProgress {
        now
    } else {
        task.updated_at
    };
    end.saturating_sub(task.created_at) as f64 / 1000.0
}

/// `"explore#2"` → `"explore"`.
fn agent_type_of_name(name: &str) -> &str {
    match name.find('#') {
        Some(idx) if idx > 0 => &name[..idx],
        _ => name,
    }
}

fn pad(n: usize) -> String {
    " ".repeat(n)
}

/// `formatLensTabs`: one tab per lens with its done/total, plus key hints
/// when they fit.
fn format_lens_tabs(
    tasks: &[Task],
    agents: &[TaskAgent],
    width: usize,
    view: TaskPanelView,
    tab_views: &[TaskPanelView],
    show_cycle_hint: bool,
    show_focus_hint: bool,
) -> String {
    let t = theme();
    let tabs: Vec<(String, String)> = tab_views
        .iter()
        .map(|&v| {
            let lens = filter_tasks_for_lens(tasks, agents, v);
            let done = lens.iter().filter(|t| t.status == TaskStatus::Done).count();
            let count = format!("{done}/{}", lens.len());
            let plain = format!(" {} {count} ", v.label());
            if v != view {
                return (plain.clone(), t.fg("dim", &plain));
            }
            let body = format!(
                " {} {} ",
                t.bold(&t.fg("accent", v.label())),
                t.fg("muted", &count)
            );
            let live = lens.iter().any(|t| t.status == TaskStatus::InProgress);
            (
                plain,
                if live {
                    t.bg("selectedBg", &body)
                } else {
                    body
                },
            )
        })
        .collect();
    let selected = tab_views
        .iter()
        .position(|&v| v == view)
        .and_then(|i| tabs.get(i))
        .or(tabs.first())
        .cloned();
    let strip = (
        tabs.iter().map(|t| t.0.as_str()).collect::<String>(),
        tabs.iter().map(|t| t.1.as_str()).collect::<String>(),
    );

    let mut parts: Vec<(String, String)> = Vec::new();
    if show_cycle_hint {
        let key = app_key_label("app.tasks.cycleForward");
        parts.push((
            format!("{} cycle", format_key_text(&key, false)),
            raw_key_hint(&key, "cycle"),
        ));
    }
    if show_focus_hint {
        let key = app_key_label("app.team.focus");
        parts.push((
            format!("{} focus", format_key_text(&key, false)),
            raw_key_hint(&key, "focus"),
        ));
    }
    let sep = "  ";
    let hint_plain = parts
        .iter()
        .map(|p| p.0.as_str())
        .collect::<Vec<_>>()
        .join(sep);
    let hint_styled = parts
        .iter()
        .map(|p| p.1.as_str())
        .collect::<Vec<_>>()
        .join(&t.fg("muted", sep));

    // Narrowing order: drop the hints, then the unselected tabs.
    if !hint_plain.is_empty() && visible_width(&strip.0) + 2 + visible_width(&hint_plain) <= width {
        let gap = width - visible_width(&strip.0) - visible_width(&hint_plain);
        return format!("{}{}{hint_styled}", strip.1, pad(gap));
    }
    for (plain, styled) in std::iter::once(strip).chain(selected.clone()) {
        if visible_width(&plain) <= width {
            return format!("{styled}{}", pad(width - visible_width(&plain)));
        }
    }
    truncate_to_width(
        &selected.map(|s| s.0).unwrap_or_default(),
        width,
        "…",
        false,
    )
}

/// Row options (`formatTaskLine`'s third argument).
#[derive(Default, Clone, Copy)]
struct LineOptions<'a> {
    grouped: bool,
    owner: Option<&'a TaskAgent>,
    tree_prefix: &'a str,
}

/// `formatTaskLine`: status icon, owner glyph where it disambiguates, origin
/// tag, title, and a right column (usage, live activity and run clock, or a
/// warning note), padded to the pane width.
fn format_task_line(task: &Task, width: usize, options: LineOptions<'_>, now: u64) -> String {
    let t = theme();
    let icon = t.fg(
        task_status_color(task.status),
        task_status_icon(task.status),
    );
    let grouped = options.grouped;
    let indent = if grouped {
        t.fg("borderMuted", GROUP_INDENT_PLAIN)
    } else {
        String::new()
    };

    let is_mcp = task.source == Some(TaskSource::Mcp);
    let owner_kind =
        options
            .owner
            .map(|o| o.kind)
            .unwrap_or(if task.source == Some(TaskSource::Subagent) {
                TaskAgentKind::Subagent
            } else {
                TaskAgentKind::Main
            });
    let show_owner_glyph = !grouped && (is_mcp || owner_kind != TaskAgentKind::Main);
    let source_glyph = if is_mcp {
        MCP_SOURCE_GLYPH
    } else {
        agent_glyph(owner_kind)
    };
    let agent_type_name = (!is_mcp && owner_kind == TaskAgentKind::Subagent)
        .then(|| task.subagent_mode.as_deref().or(task.agent.as_deref()))
        .flatten();
    let styled_source = if !show_owner_glyph {
        String::new()
    } else if let Some(name) = agent_type_name {
        t.fg(agent_color_for(name), source_glyph)
    } else {
        t.fg("dim", source_glyph)
    };

    let (tag, tag_color) = if is_mcp {
        (
            format!("[{}]", task.subagent_mode.as_deref().unwrap_or("MCP")),
            "mcp",
        )
    } else if grouped {
        (String::new(), "accent")
    } else if let Some(mode) = &task.subagent_mode {
        (format!("[{mode}]"), agent_color_for(mode))
    } else if let (TaskAgentKind::Role, Some(owner)) = (owner_kind, options.owner) {
        (format!("[{}]", owner.name), agent_color_for(&owner.name))
    } else {
        (String::new(), "accent")
    };
    let styled_tag = if tag.is_empty() {
        String::new()
    } else {
        format!("{} ", t.fg(tag_color, &tag))
    };
    let title = task.title.as_str();
    let styled_title = match task.status {
        TaskStatus::Done => t.fg("muted", title),
        TaskStatus::Pending => t.fg("dim", title),
        TaskStatus::Failed => t.fg("error", title),
        TaskStatus::Cancelled => t.fg("dim", &t.strikethrough(title)),
        TaskStatus::InProgress => t.bold(title),
    };

    let mut right_plain = String::new();
    let mut right_styled = String::new();
    match task.status {
        TaskStatus::Done | TaskStatus::Failed => {
            if let Some(usage) = &task.usage {
                let total = usage.input + usage.output;
                if total > 0.0 {
                    right_plain = format_tokens(total as u64);
                    right_styled = t.fg("muted", &right_plain);
                }
            }
        }
        TaskStatus::InProgress => {
            let activity = options
                .owner
                .and_then(|o| o.activity.as_deref())
                .filter(|a| !a.is_empty())
                .map(|a| format!("⋯ {a}"))
                .unwrap_or_default();
            let run_for = if is_delegated(task) {
                format_duration_secs(task_elapsed_secs(task, now))
            } else {
                String::new()
            };
            match (activity.is_empty(), run_for.is_empty()) {
                (false, false) => {
                    right_plain = format!("{activity} · {run_for}");
                    right_styled =
                        t.fg("warning", &activity) + &t.fg("dim", &format!(" · {run_for}"));
                }
                (false, true) => {
                    right_styled = t.fg("warning", &activity);
                    right_plain = activity;
                }
                (true, false) => {
                    right_styled = t.fg("dim", &run_for);
                    right_plain = run_for;
                }
                (true, true) => {}
            }
        }
        _ => {}
    }
    if let Some(note) = &task.note {
        right_plain = format!("{WARNING_GLYPH} {note}");
        right_styled = t.fg("warning", &right_plain);
    }

    let right_width = if right_plain.is_empty() {
        0
    } else {
        visible_width(&right_plain) + 1
    };
    let left_width = width.saturating_sub(right_width);
    let tree_prefix = if options.tree_prefix.is_empty() {
        String::new()
    } else {
        t.fg("borderMuted", options.tree_prefix)
    };
    let left_body = if grouped {
        format!("{indent}{icon} {styled_tag}{styled_title}")
    } else {
        let source = if styled_source.is_empty() {
            String::new()
        } else {
            format!("{styled_source} ")
        };
        format!("{tree_prefix}{icon} {source}{styled_tag}{styled_title}")
    };
    let left = truncate_to_width(&left_body, left_width, "…", false);
    if right_plain.is_empty() {
        let gap = width.saturating_sub(visible_width(&left));
        return format!("{left}{}", pad(gap));
    }
    let gap = width
        .saturating_sub(visible_width(&left) + visible_width(&right_plain))
        .max(1);
    format!("{left}{}{right_styled}", pad(gap))
}

/// One row of the flat or subagents lens: a task and its tree connector.
struct TreeRow<'a> {
    task: &'a Task,
    tree_prefix: String,
}

/// `subagentLensRows`: a depth-first walk of the delegated forest (orphans
/// become roots; a visited set guards against cycles).
fn subagent_lens_rows(tasks: &[Task]) -> Vec<TreeRow<'_>> {
    let ids: HashSet<u64> = tasks.iter().map(|t| t.id).collect();
    let mut children: HashMap<u64, Vec<&Task>> = HashMap::new();
    for task in tasks {
        if let Some(parent) = task.parent_task_id {
            children.entry(parent).or_default().push(task);
        }
    }
    let roots = tasks.iter().filter(|t| match t.parent_task_id {
        None => t.source.is_some(),
        Some(parent) => !ids.contains(&parent),
    });
    let mut rows = Vec::new();
    let mut visited = HashSet::new();
    fn walk<'a>(
        task: &'a Task,
        prefix: &str,
        is_last: bool,
        is_root: bool,
        children: &HashMap<u64, Vec<&'a Task>>,
        visited: &mut HashSet<u64>,
        rows: &mut Vec<TreeRow<'a>>,
    ) {
        if !visited.insert(task.id) {
            return;
        }
        let branch = if is_last { "└─ " } else { "├─ " };
        rows.push(TreeRow {
            task,
            tree_prefix: if is_root {
                String::new()
            } else {
                format!("{prefix}{branch}")
            },
        });
        let kids = children.get(&task.id).cloned().unwrap_or_default();
        let child_prefix = if is_root {
            String::new()
        } else {
            format!("{prefix}{}", if is_last { "   " } else { "│  " })
        };
        for (i, kid) in kids.iter().enumerate() {
            walk(
                kid,
                &child_prefix,
                i == kids.len() - 1,
                false,
                children,
                visited,
                rows,
            );
        }
    }
    for root in roots {
        walk(root, "", true, true, &children, &mut visited, &mut rows);
    }
    rows
}

/// `flatLensRows`: the plan, each linked subagent run nested under its item.
fn flat_lens_rows(tasks: &[Task]) -> Vec<TreeRow<'_>> {
    let mut linked: HashMap<u64, Vec<&Task>> = HashMap::new();
    for task in tasks {
        if let (Some(link), Some(TaskSource::Subagent)) = (task.linked_task_id, task.source) {
            linked.entry(link).or_default().push(task);
        }
    }
    let mut rows = Vec::new();
    for task in tasks.iter().filter(|t| is_main_task(t)) {
        rows.push(TreeRow {
            task,
            tree_prefix: String::new(),
        });
        let runs = linked.get(&task.id).cloned().unwrap_or_default();
        for (i, run) in runs.iter().enumerate() {
            rows.push(TreeRow {
                task: run,
                tree_prefix: if i == runs.len() - 1 {
                    "└─ "
                } else {
                    "├─ "
                }
                .to_string(),
            });
        }
    }
    rows
}

fn role_ids(agents: &[TaskAgent]) -> HashSet<&str> {
    agents
        .iter()
        .filter(|a| a.kind == TaskAgentKind::Role)
        .map(|a| a.id.as_str())
        .collect()
}

/// `filterTasksForLens`: exactly the tasks the lens shows.
fn filter_tasks_for_lens<'a>(
    tasks: &'a [Task],
    agents: &[TaskAgent],
    view: TaskPanelView,
) -> Vec<&'a Task> {
    match view {
        TaskPanelView::Flat => flat_lens_rows(tasks).into_iter().map(|r| r.task).collect(),
        TaskPanelView::Subagents => subagent_lens_rows(tasks)
            .into_iter()
            .map(|r| r.task)
            .collect(),
        TaskPanelView::Teams => {
            let roles = role_ids(agents);
            tasks
                .iter()
                .filter(|t| roles.contains(t.owner_id().as_str()))
                .collect()
        }
    }
}

/// Group metadata for an owner with no roster entry.
fn default_agent_meta(id: &str) -> TaskAgent {
    let main = id == "main";
    TaskAgent {
        id: id.to_string(),
        name: id.to_string(),
        role: Some(if main { "orchestrator" } else { "subagent" }.to_string()),
        kind: if main {
            TaskAgentKind::Main
        } else {
            TaskAgentKind::Subagent
        },
        state: None,
        handoff: None,
        activity: None,
        stats: None,
    }
}

struct Group<'a> {
    id: String,
    meta: TaskAgent,
    items: Vec<&'a Task>,
}

/// `groupTasks`: owner groups, main first, then roster order, then the rest.
fn group_tasks<'a>(tasks: &[&'a Task], agents: &[TaskAgent]) -> Vec<Group<'a>> {
    let mut groups: Vec<(String, Vec<&'a Task>)> = Vec::new();
    for task in tasks {
        let owner = task.owner_id();
        match groups.iter_mut().find(|(id, _)| *id == owner) {
            Some((_, items)) => items.push(task),
            None => groups.push((owner, vec![task])),
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if !order.iter().any(|o| o == id) {
            order.push(id.to_string());
        }
    };
    if groups.iter().any(|(id, _)| id == "main") {
        push("main");
    }
    for agent in agents {
        if groups.iter().any(|(id, _)| *id == agent.id) {
            push(&agent.id);
        }
    }
    for (id, _) in &groups {
        push(id);
    }
    order
        .into_iter()
        .map(|id| Group {
            meta: agents
                .iter()
                .find(|a| a.id == id)
                .cloned()
                .unwrap_or_else(|| default_agent_meta(&id)),
            items: groups
                .iter()
                .find(|(g, _)| *g == id)
                .map(|(_, items)| items.clone())
                .unwrap_or_default(),
            id,
        })
        .collect()
}

/// `formatGroupHeader`: glyph, name, role, `[state]`, activity, handoff, and
/// the agent's usage plus done/total on the right.
fn format_group_header(meta: &TaskAgent, items: &[&Task], width: usize, selected: bool) -> String {
    let t = theme();
    let identity_color = if meta.kind == TaskAgentKind::Main {
        agent_glyph_color(meta.kind)
    } else {
        agent_color_for(agent_type_of_name(&meta.name))
    };
    let glyph = if selected {
        t.fg("accent", SELECTED_GLYPH)
    } else {
        t.fg(identity_color, agent_glyph(meta.kind))
    };
    let name = if selected {
        t.bold(&t.fg("accent", &meta.name))
    } else if meta.kind == TaskAgentKind::Main {
        t.bold(&meta.name)
    } else {
        t.bold(&t.fg(identity_color, &meta.name))
    };
    let role = match &meta.role {
        Some(role) if !role.is_empty() => {
            if meta.kind == TaskAgentKind::Main {
                format!(" {}", t.fg("muted", role))
            } else {
                t.fg("dim", &format!(" · {role}"))
            }
        }
        _ => String::new(),
    };
    let state = meta
        .state
        .map(|s| {
            format!(
                " {}",
                t.fg(agent_state_color(s), &format!("[{}]", agent_state_label(s)))
            )
        })
        .unwrap_or_default();
    let activity = meta
        .activity
        .as_deref()
        .filter(|a| !a.is_empty())
        .map(|a| t.fg("dim", &format!(" ⋯ {a}")))
        .unwrap_or_default();
    let handoff = meta
        .handoff
        .as_deref()
        .filter(|h| !h.is_empty())
        .map(|h| format!(" {}", t.fg("dim", h)))
        .unwrap_or_default();

    let done = items
        .iter()
        .filter(|t| t.status == TaskStatus::Done)
        .count();
    let count_plain = format!("{done}/{}", items.len());
    let count = t.fg("muted", &done.to_string())
        + &t.fg("dim", "/")
        + &t.fg("muted", &items.len().to_string());
    let (mut right_plain, mut right_styled) = (count_plain.clone(), count.clone());
    if let Some(stats) = meta
        .stats
        .filter(|s| s.input > 0.0 || s.output > 0.0 || s.cost > 0.0)
    {
        let stats_plain = format!(
            "↑{} ↓{} · ${:.3}",
            format_tokens(stats.input as u64),
            format_tokens(stats.output as u64),
            stats.cost
        );
        right_plain = format!("{stats_plain}  {count_plain}");
        right_styled = format!("{}  {count}", t.fg("dim", &stats_plain));
    }
    let right_width = visible_width(&right_plain) + 1;
    let left = truncate_to_width(
        &format!("{glyph} {name}{role}{state}{activity}{handoff}"),
        width.saturating_sub(right_width),
        "…",
        false,
    );
    let gap = width
        .saturating_sub(visible_width(&left) + visible_width(&right_plain))
        .max(1);
    format!("{left}{}{right_styled}", pad(gap))
}

/// What a focused roster asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskPanelEvent {
    Nudge(String),
    Attach(String),
    ExitFocus,
}

/// `TaskPanelComponent`.
pub struct TaskPanelComponent {
    view: TaskPanelView,
    density: TaskPanelDensity,
    disposed: bool,
    ticking: bool,
    /// Team focus: role rows become a navigable list.
    pub focused: bool,
    selected_role: Option<String>,
    events: Vec<TaskPanelEvent>,
}

impl Default for TaskPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskPanelComponent {
    pub fn new() -> Self {
        Self {
            view: TaskPanelView::Flat,
            density: TaskPanelDensity::Full,
            disposed: false,
            ticking: false,
            focused: false,
            selected_role: None,
            events: Vec::new(),
        }
    }

    fn role_agents() -> Vec<TaskAgent> {
        task_store()
            .agents()
            .into_iter()
            .filter(|a| a.kind == TaskAgentKind::Role)
            .collect()
    }

    /// `focusedRole`: the cursor's role, clamped to the live roster.
    pub fn focused_role(&self) -> Option<String> {
        let roles = Self::role_agents();
        roles
            .iter()
            .find(|a| Some(&a.name) == self.selected_role.as_ref())
            .or(roles.first())
            .map(|a| a.name.clone())
    }

    /// What the focused roster asked for since the last call.
    pub fn take_events(&mut self) -> Vec<TaskPanelEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn get_view(&self) -> TaskPanelView {
        self.view
    }

    pub fn set_view(&mut self, view: TaskPanelView) {
        self.view = view;
    }

    /// `setDensity`: driven by the chrome, not by the pane.
    pub fn set_density(&mut self, density: TaskPanelDensity) {
        self.density = density;
    }

    /// `cycleView`: the next lens with content, wrapping; an emptied view
    /// snaps back to flat.
    pub fn cycle_view(&mut self, forward: bool) -> TaskPanelView {
        let store = task_store();
        let available = available_views(&store.list(), &store.agents());
        let len = available.len() as isize;
        let idx = available
            .iter()
            .position(|v| *v == self.view)
            .map_or(-1, |i| i as isize);
        self.view = if len == 0 {
            TaskPanelView::Flat
        } else {
            let step = if forward { 1 } else { -1 };
            available[((idx + step + len) % len) as usize]
        };
        self.view
    }

    /// A delegated row is live: re-render every second for its run clock.
    pub fn ticking(&self) -> bool {
        self.ticking
    }

    /// Stop the run clock. Call on teardown.
    pub fn dispose(&mut self) {
        self.disposed = true;
        self.ticking = false;
    }
}

impl Component for TaskPanelComponent {
    fn handle_input(&mut self, data: &str) {
        let roles = Self::role_agents();
        if roles.is_empty() {
            self.events.push(TaskPanelEvent::ExitFocus);
            return;
        }
        let kb = get_keybindings();
        let index = roles
            .iter()
            .position(|a| Some(&a.name) == self.selected_role.as_ref())
            .unwrap_or(0);
        if kb.matches(data, "tui.select.up") {
            self.selected_role = Some(roles[index.saturating_sub(1)].name.clone());
        } else if kb.matches(data, "tui.select.down") {
            self.selected_role = Some(roles[(index + 1).min(roles.len() - 1)].name.clone());
        } else if matches_app_key(data, "app.team.nudge") {
            if let Some(role) = self.focused_role() {
                self.events.push(TaskPanelEvent::Nudge(role));
            }
        } else if matches_app_key(data, "app.team.attach") {
            if let Some(role) = self.focused_role() {
                self.events.push(TaskPanelEvent::Attach(role));
            }
        } else if matches_key(data, "q") || kb.matches(data, "tui.select.cancel") {
            self.events.push(TaskPanelEvent::ExitFocus);
        }
    }

    fn render(&mut self, width: u16) -> Vec<String> {
        if self.disposed {
            return Vec::new();
        }
        let width = width as usize;
        let store = task_store();
        let tasks = store.list();
        let agents = store.agents();
        let now = now_ms();

        // Keep the stored view while it has content, else the first lens that
        // does; team focus always shows the roster.
        let available = available_views(&tasks, &agents);
        let mut view = if available.contains(&self.view) {
            self.view
        } else {
            available.first().copied().unwrap_or(TaskPanelView::Flat)
        };
        if self.focused {
            view = TaskPanelView::Teams;
        }
        let has_role_roster =
            view == TaskPanelView::Teams && agents.iter().any(|a| a.kind == TaskAgentKind::Role);
        if tasks.is_empty() && !has_role_roster {
            self.ticking = false;
            return Vec::new();
        }

        let lens: Vec<Task> = filter_tasks_for_lens(&tasks, &agents, view)
            .into_iter()
            .cloned()
            .collect();
        self.ticking = lens
            .iter()
            .any(|t| t.status == TaskStatus::InProgress && is_delegated(t));

        let t = theme();
        let gutter = format!("{} ", t.fg(panel_state_color(&lens), RAIL));
        let inner = width.saturating_sub(visible_width(RAIL) + 1);
        let tab_views: Vec<TaskPanelView> = if available.contains(&view) {
            available.clone()
        } else {
            available
                .iter()
                .copied()
                .chain(std::iter::once(view))
                .collect()
        };

        if self.density == TaskPanelDensity::Summary {
            return vec![
                gutter.clone()
                    + &format_lens_tabs(&tasks, &agents, inner, view, &tab_views, false, false),
            ];
        }

        let mut lines = Vec::new();
        if available.len() >= 2 || view == TaskPanelView::Teams {
            let show_cycle_hint = available.len() >= 2 && !self.focused;
            let show_focus_hint =
                view == TaskPanelView::Teams && !self.focused && !Self::role_agents().is_empty();
            lines.push(
                gutter.clone()
                    + &format_lens_tabs(
                        &tasks,
                        &agents,
                        inner,
                        view,
                        &tab_views,
                        show_cycle_hint,
                        show_focus_hint,
                    ),
            );
        }

        let agent_by_id: HashMap<&str, &TaskAgent> =
            agents.iter().map(|a| (a.id.as_str(), a)).collect();
        let tree_rows = match view {
            TaskPanelView::Flat => Some(flat_lens_rows(&tasks)),
            TaskPanelView::Subagents => Some(subagent_lens_rows(&tasks)),
            TaskPanelView::Teams => None,
        };
        if let Some(rows) = tree_rows {
            for row in rows {
                let owner_id = task_owner_id(row.task.agent.as_deref(), row.task.source);
                let options = LineOptions {
                    grouped: false,
                    owner: agent_by_id.get(owner_id.as_str()).copied(),
                    tree_prefix: &row.tree_prefix,
                };
                lines.push(gutter.clone() + &format_task_line(row.task, inner, options, now));
            }
            return lines;
        }

        // Teams: role groups (idle roles too), handoff connectors, focus hints.
        let roles = role_ids(&agents);
        let role_agents: Vec<TaskAgent> = agents
            .iter()
            .filter(|a| a.kind == TaskAgentKind::Role)
            .cloned()
            .collect();
        let team_tasks: Vec<&Task> = tasks
            .iter()
            .filter(|t| roles.contains(t.owner_id().as_str()))
            .collect();
        let mut groups = group_tasks(&team_tasks, &role_agents);
        for agent in &role_agents {
            if !groups.iter().any(|g| g.id == agent.id) {
                groups.push(Group {
                    id: agent.id.clone(),
                    meta: agent.clone(),
                    items: Vec::new(),
                });
            }
        }
        let cursor_role = if self.focused {
            self.focused_role()
        } else {
            None
        };
        for group in &groups {
            let selected = group.meta.kind == TaskAgentKind::Role
                && cursor_role.as_deref() == Some(group.meta.name.as_str());
            lines.push(
                gutter.clone() + &format_group_header(&group.meta, &group.items, inner, selected),
            );
            for task in &group.items {
                let options = LineOptions {
                    grouped: true,
                    ..Default::default()
                };
                lines.push(gutter.clone() + &format_task_line(task, inner, options, now));
            }
            if let Some(handoff) = &group.meta.handoff {
                if let Some(idx) = handoff.find("→ ") {
                    let next = handoff[idx + "→ ".len()..].trim();
                    if role_agents.iter().any(|a| a.name == next) {
                        let connector = format!("{GROUP_INDENT_PLAIN}  └──→ ");
                        let gap =
                            inner.saturating_sub(visible_width(&connector) + visible_width(next));
                        lines.push(format!(
                            "{gutter}{}{}{}",
                            t.fg("borderMuted", &connector),
                            t.fg("dim", next),
                            pad(gap)
                        ));
                    }
                }
            }
        }
        if self.focused {
            let sep = t.fg("muted", " · ");
            let hint = [
                raw_key_hint("↑/↓", "select"),
                raw_key_hint(&app_key_label("app.team.nudge"), "nudge"),
                raw_key_hint(&app_key_label("app.team.attach"), "attach"),
                raw_key_hint("q/esc", "back"),
            ]
            .join(&sep);
            lines.push(gutter.clone() + &truncate_to_width(&hint, inner, "…", false));
        }
        lines
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

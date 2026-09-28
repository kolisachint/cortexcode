//! The session picker (`components/session-selector.ts`): the sessions of the
//! current folder or of every folder, threaded by fork parent or searched,
//! with rename and delete.
//!
//! Loading is asynchronous in the pin. Here a loader receives a [`LoadSink`]
//! (`Send`, so it can finish on another thread) and the picker applies what
//! arrived in [`SessionSelectorComponent::poll`], which also expires the
//! header's timed status message. The list reports what a key did as
//! [`ListEvent`]s instead of calling back into its owner.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use cortexcode_code_paths::{canonicalize_path, home_dir};
use cortexcode_code_session::identity::{session_color_slot_for, session_slug_for};
use cortexcode_code_session::SessionInfo;
use cortexcode_code_tui_keybindings::{key_hint, key_text};
use cortexcode_code_tui_theme::{
    paint_selected_row, select_gutter, session_color_token, style_input, theme, SELECT_CURSOR,
};
use cortexcode_code_tui_widgets::brand::GIT_BRANCH_GLYPH;
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_tui_components::{Input, Spacer, Text};
use cortexcode_tui_keys::{get_keybindings, KeybindingsManager};
use cortexcode_tui_render::{Component, ComponentHandle, Container};
use cortexcode_tui_util::js_regex::is_js_space;
use cortexcode_tui_util::{truncate_to_width, visible_width};

use crate::session_selector_search::{
    filter_and_sort_sessions, has_session_name, NameFilter, SortMode,
};

/// Which sessions the picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    Current,
    All,
}

/// Branches that name no particular piece of work.
const DEFAULT_BRANCHES: [&str; 4] = ["main", "master", "trunk", "develop"];

fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

fn shorten_path(path: &str) -> String {
    let home = home_dir();
    let home = home.to_string_lossy();
    if path.is_empty() {
        return String::new();
    }
    match path.strip_prefix(&*home) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}

/// `formatSessionDate`: the age of `date` in one short unit.
pub fn format_session_date(date: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let diff_ms = (now - date).num_milliseconds() as f64;
    let mins = (diff_ms / 60_000.0).floor();
    let hours = (diff_ms / 3_600_000.0).floor();
    let days = (diff_ms / 86_400_000.0).floor();
    if mins < 1.0 {
        "now".to_string()
    } else if mins < 60.0 {
        format!("{mins}m")
    } else if hours < 24.0 {
        format!("{hours}h")
    } else if days < 7.0 {
        format!("{days}d")
    } else if days < 30.0 {
        format!("{}w", (days / 7.0).floor())
    } else if days < 365.0 {
        format!("{}mo", (days / 30.0).floor())
    } else {
        format!("{}y", (days / 365.0).floor())
    }
}

fn canonical(path: &Path) -> PathBuf {
    canonicalize_path(path)
}

// ---- header ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Info,
    Error,
}

struct SessionSelectorHeader {
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    loading: bool,
    load_progress: Option<(usize, usize)>,
    show_path: bool,
    confirming_delete: bool,
    status: Option<(StatusKind, String)>,
    status_deadline: Option<Instant>,
    show_rename_hint: bool,
}

impl SessionSelectorHeader {
    fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
        // Progress is scoped to the current load.
        self.load_progress = None;
    }

    fn set_status(&mut self, status: Option<(StatusKind, String)>, auto_hide: Option<Duration>) {
        self.status = status;
        self.status_deadline = match (&self.status, auto_hide) {
            (Some(_), Some(d)) if !d.is_zero() => Some(Instant::now() + d),
            _ => None,
        };
    }

    /// Expire a timed status; `true` when it went.
    fn poll(&mut self, now: Instant) -> bool {
        match self.status_deadline {
            Some(deadline) if now >= deadline => {
                self.status = None;
                self.status_deadline = None;
                true
            }
            _ => false,
        }
    }
}

impl Component for SessionSelectorHeader {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let t = theme();
        let title = match self.scope {
            SessionScope::Current => "Resume Session (Current Folder)",
            SessionScope::All => "Resume Session (All)",
        };
        let left_text = t.bold(title);

        let sort_label = match self.sort_mode {
            SortMode::Threaded => "Threaded",
            SortMode::Recent => "Recent",
            SortMode::Relevance => "Fuzzy",
        };
        let sort_text = t.fg("muted", "Sort: ") + &t.fg("accent", sort_label);
        let name_label = match self.name_filter {
            NameFilter::All => "All",
            NameFilter::Named => "Named",
        };
        let name_text = t.fg("muted", "Name: ") + &t.fg("accent", name_label);

        let scope_text = if self.loading {
            let progress = match self.load_progress {
                Some((loaded, total)) => format!("{loaded}/{total}"),
                None => "...".to_string(),
            };
            t.fg("muted", "○ Current Folder | ") + &t.fg("accent", &format!("Loading {progress}"))
        } else if self.scope == SessionScope::Current {
            t.fg("accent", "◉ Current Folder") + &t.fg("muted", " | ○ All")
        } else {
            t.fg("muted", "○ Current Folder | ") + &t.fg("accent", "◉ All")
        };

        let right_text = truncate_to_width(
            &format!("{scope_text}  {name_text}  {sort_text}"),
            width,
            "",
            false,
        );
        let available_left = width.saturating_sub(visible_width(&right_text) + 1);
        let left = truncate_to_width(&left_text, available_left, "", false);
        let spacing = width.saturating_sub(visible_width(&left) + visible_width(&right_text));

        let (hint1, hint2) = if self.confirming_delete {
            let confirm = format!(
                "Delete session? {} · {}",
                key_hint("tui.select.confirm", "confirm"),
                key_hint("tui.select.cancel", "cancel")
            );
            (
                t.fg("error", &truncate_to_width(&confirm, width, "…", false)),
                String::new(),
            )
        } else if let Some((kind, message)) = &self.status {
            let color = if *kind == StatusKind::Error {
                "error"
            } else {
                "accent"
            };
            (
                t.fg(color, &truncate_to_width(message, width, "…", false)),
                String::new(),
            )
        } else {
            let path_state = if self.show_path { "(on)" } else { "(off)" };
            let sep = t.fg("muted", " · ");
            let hint1 = key_hint("tui.input.tab", "scope")
                + &sep
                + &t.fg("muted", "re:<pattern> regex · \"phrase\" exact");
            let mut parts = vec![
                key_hint("app.session.toggleSort", "sort"),
                key_hint("app.session.toggleNamedFilter", "named"),
                key_hint("app.session.delete", "delete"),
                key_hint("app.session.togglePath", &format!("path {path_state}")),
            ];
            if self.show_rename_hint {
                parts.push(key_hint("app.session.rename", "rename"));
            }
            let hint2 = parts.join(&sep);
            (
                truncate_to_width(&hint1, width, "…", false),
                truncate_to_width(&hint2, width, "…", false),
            )
        };

        vec![
            format!("{left}{}{right_text}", " ".repeat(spacing)),
            hint1,
            hint2,
        ]
    }
}

// ---- list ------------------------------------------------------------------

struct TreeNode {
    session: SessionInfo,
    children: Vec<TreeNode>,
}

/// A row: a session with its place in the thread tree.
#[derive(Debug, Clone)]
struct FlatNode {
    session: SessionInfo,
    depth: usize,
    is_last: bool,
    /// For each ancestor level, whether more siblings follow it.
    ancestor_continues: Vec<bool>,
}

/// `buildSessionTree`: fork children under their parent, newest first.
fn build_session_tree(sessions: &[SessionInfo]) -> Vec<TreeNode> {
    let keys: Vec<PathBuf> = sessions.iter().map(|s| canonical(&s.path)).collect();
    let parent_of: Vec<Option<usize>> = sessions
        .iter()
        .map(|s| {
            let parent = s.parent_session_path.as_deref().filter(|p| !p.is_empty())?;
            let parent = canonical(Path::new(parent));
            // The last session with a path wins, as a Map keeps the last set.
            keys.iter().rposition(|k| *k == parent)
        })
        .collect();

    fn build(i: usize, sessions: &[SessionInfo], parent_of: &[Option<usize>]) -> TreeNode {
        let children = (0..sessions.len())
            .filter(|&j| parent_of[j] == Some(i) && j != i)
            .map(|j| build(j, sessions, parent_of))
            .collect();
        TreeNode {
            session: sessions[i].clone(),
            children,
        }
    }

    let mut roots: Vec<TreeNode> = (0..sessions.len())
        .filter(|&i| parent_of[i].is_none_or(|p| p == i))
        .map(|i| build(i, sessions, &parent_of))
        .collect();

    fn sort_nodes(nodes: &mut [TreeNode]) {
        nodes.sort_by(|a, b| b.session.modified.cmp(&a.session.modified));
        for node in nodes {
            sort_nodes(&mut node.children);
        }
    }
    sort_nodes(&mut roots);
    roots
}

/// `flattenSessionTree`.
fn flatten_session_tree(roots: &[TreeNode]) -> Vec<FlatNode> {
    fn walk(
        node: &TreeNode,
        depth: usize,
        ancestor_continues: Vec<bool>,
        is_last: bool,
        out: &mut Vec<FlatNode>,
    ) {
        out.push(FlatNode {
            session: node.session.clone(),
            depth,
            is_last,
            ancestor_continues: ancestor_continues.clone(),
        });
        let n = node.children.len();
        for (i, child) in node.children.iter().enumerate() {
            // Only non-root ancestors draw a continuation line.
            let continues = depth > 0 && !is_last;
            let mut next = ancestor_continues.clone();
            next.push(continues);
            walk(child, depth + 1, next, i == n - 1, out);
        }
    }
    let mut out = Vec::new();
    let n = roots.len();
    for (i, root) in roots.iter().enumerate() {
        walk(root, 0, Vec::new(), i == n - 1, &mut out);
    }
    out
}

/// What a key did to the list, for its owner to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListEvent {
    Select(PathBuf),
    Cancel,
    ToggleScope,
    ToggleSort,
    ToggleNameFilter,
    TogglePath(bool),
    DeleteConfirmationChange(Option<PathBuf>),
    DeleteSession(PathBuf),
    Rename(PathBuf),
    Error(String),
}

/// `SessionList`: the search line and one row per session.
pub struct SessionList {
    all_sessions: Vec<SessionInfo>,
    filtered: Vec<FlatNode>,
    selected_index: usize,
    search_input: Input,
    show_cwd: bool,
    sort_mode: SortMode,
    name_filter: NameFilter,
    keybindings: Rc<KeybindingsManager>,
    show_path: bool,
    confirming_delete_path: Option<PathBuf>,
    current_session_canonical: Option<PathBuf>,
    max_visible: usize,
    now: Option<DateTime<Utc>>,
}

impl SessionList {
    fn new(
        sort_mode: SortMode,
        name_filter: NameFilter,
        keybindings: Rc<KeybindingsManager>,
        current_session_file: Option<&Path>,
    ) -> Self {
        let mut search_input = Input::new();
        style_input(&mut search_input);
        let mut list = Self {
            all_sessions: Vec::new(),
            filtered: Vec::new(),
            selected_index: 0,
            search_input,
            show_cwd: false,
            sort_mode,
            name_filter,
            keybindings,
            show_path: false,
            confirming_delete_path: None,
            current_session_canonical: current_session_file.map(canonical),
            max_visible: 10,
            now: None,
        };
        list.filter_sessions("");
        list
    }

    /// The path of the highlighted session.
    pub fn selected_session_path(&self) -> Option<&Path> {
        self.filtered
            .get(self.selected_index)
            .map(|n| n.session.path.as_path())
    }

    /// Fix "now" for the age column (tests).
    pub fn set_now(&mut self, now: Option<DateTime<Utc>>) {
        self.now = now;
    }

    fn set_sort_mode(&mut self, sort_mode: SortMode) {
        self.sort_mode = sort_mode;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn set_name_filter(&mut self, name_filter: NameFilter) {
        self.name_filter = name_filter;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn set_sessions(&mut self, sessions: Vec<SessionInfo>, show_cwd: bool) {
        self.all_sessions = sessions;
        self.show_cwd = show_cwd;
        let query = self.search_input.get_value().to_string();
        self.filter_sessions(&query);
    }

    fn filter_sessions(&mut self, query: &str) {
        let name_filtered: Vec<SessionInfo> = match self.name_filter {
            NameFilter::All => self.all_sessions.clone(),
            NameFilter::Named => self
                .all_sessions
                .iter()
                .filter(|s| has_session_name(s))
                .cloned()
                .collect(),
        };
        if self.sort_mode == SortMode::Threaded && js_trim(query).is_empty() {
            self.filtered = flatten_session_tree(&build_session_tree(&name_filtered));
        } else {
            self.filtered =
                filter_and_sort_sessions(&name_filtered, query, self.sort_mode, NameFilter::All)
                    .into_iter()
                    .map(|session| FlatNode {
                        session,
                        depth: 0,
                        is_last: true,
                        ancestor_continues: Vec::new(),
                    })
                    .collect();
        }
        self.selected_index = self
            .selected_index
            .min(self.filtered.len().saturating_sub(1));
    }

    fn set_confirming_delete_path(&mut self, path: Option<PathBuf>, events: &mut Vec<ListEvent>) {
        self.confirming_delete_path = path.clone();
        events.push(ListEvent::DeleteConfirmationChange(path));
    }

    fn start_delete_confirmation(&mut self, events: &mut Vec<ListEvent>) {
        let Some(selected) = self.filtered.get(self.selected_index) else {
            return;
        };
        let path = selected.session.path.clone();
        if self.is_current_session_path(&path) {
            events.push(ListEvent::Error(
                "Cannot delete the currently active session".to_string(),
            ));
            return;
        }
        self.set_confirming_delete_path(Some(path), events);
    }

    fn is_current_session_path(&self, path: &Path) -> bool {
        self.current_session_canonical
            .as_ref()
            .is_some_and(|current| canonical(path) == *current)
    }

    fn build_tree_prefix(node: &FlatNode) -> String {
        if node.depth == 0 {
            return String::new();
        }
        let mut prefix: String = node
            .ancestor_continues
            .iter()
            .map(|&c| if c { "│  " } else { "   " })
            .collect();
        prefix.push_str(if node.is_last { "└─ " } else { "├─ " });
        prefix
    }

    fn search_value(&self) -> String {
        self.search_input.get_value().to_string()
    }

    fn forward_to_search(&mut self, data: &str) {
        self.search_input.handle_input(data);
        let query = self.search_value();
        self.filter_sessions(&query);
    }

    /// Handle a key; returns what it did.
    pub fn handle_key(&mut self, data: &str) -> Vec<ListEvent> {
        let kb = get_keybindings();
        let mut events = Vec::new();

        // A pending delete takes every key.
        if let Some(path) = self.confirming_delete_path.clone() {
            if kb.matches(data, "tui.select.confirm") {
                self.set_confirming_delete_path(None, &mut events);
                events.push(ListEvent::DeleteSession(path));
            } else if kb.matches(data, "tui.select.cancel") {
                self.set_confirming_delete_path(None, &mut events);
            }
            return events;
        }

        if kb.matches(data, "tui.input.tab") {
            events.push(ListEvent::ToggleScope);
            return events;
        }
        if kb.matches(data, "app.session.toggleSort") {
            events.push(ListEvent::ToggleSort);
            return events;
        }
        if self
            .keybindings
            .matches(data, "app.session.toggleNamedFilter")
        {
            events.push(ListEvent::ToggleNameFilter);
            return events;
        }
        if kb.matches(data, "app.session.togglePath") {
            self.show_path = !self.show_path;
            events.push(ListEvent::TogglePath(self.show_path));
            return events;
        }
        if kb.matches(data, "app.session.delete") {
            self.start_delete_confirmation(&mut events);
            return events;
        }
        if kb.matches(data, "app.session.rename") {
            if let Some(selected) = self.filtered.get(self.selected_index) {
                events.push(ListEvent::Rename(selected.session.path.clone()));
            }
            return events;
        }
        // Ctrl+Backspace deletes only with an empty query; otherwise it edits it.
        if kb.matches(data, "app.session.deleteNoninvasive") {
            if !self.search_input.get_value().is_empty() {
                self.forward_to_search(data);
                return events;
            }
            self.start_delete_confirmation(&mut events);
            return events;
        }

        let last = self.filtered.len().saturating_sub(1);
        if kb.matches(data, "tui.select.up") {
            self.selected_index = self.selected_index.saturating_sub(1);
        } else if kb.matches(data, "tui.select.down") {
            self.selected_index = (self.selected_index + 1).min(last);
        } else if kb.matches(data, "tui.select.pageUp") {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible);
        } else if kb.matches(data, "tui.select.pageDown") {
            self.selected_index = (self.selected_index + self.max_visible).min(last);
        } else if kb.matches(data, "tui.select.confirm") {
            if let Some(selected) = self.filtered.get(self.selected_index) {
                events.push(ListEvent::Select(selected.session.path.clone()));
            }
        } else if kb.matches(data, "tui.select.cancel") {
            events.push(ListEvent::Cancel);
        } else {
            self.forward_to_search(data);
        }
        events
    }
}

impl Component for SessionList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let w = width as usize;
        let mut lines = self.search_input.render(width);
        lines.push(String::new());

        if self.filtered.is_empty() {
            let message = if self.name_filter == NameFilter::Named {
                let toggle = key_text("app.session.toggleNamedFilter");
                if self.show_cwd {
                    format!("  No named sessions found. Press {toggle} to show all.")
                } else {
                    format!(
                        "  No named sessions in current folder. Press {toggle} to show all, or Tab to view all."
                    )
                }
            } else if self.show_cwd {
                "  No sessions found".to_string()
            } else {
                "  No sessions in current folder. Press Tab to view all.".to_string()
            };
            lines.push(t.fg("muted", &truncate_to_width(&message, w, "…", false)));
            return lines;
        }

        let len = self.filtered.len();
        let start = (self.selected_index as i64 - (self.max_visible / 2) as i64)
            .min(len as i64 - self.max_visible as i64)
            .max(0) as usize;
        let end = (start + self.max_visible).min(len);
        let now = self.now.unwrap_or_else(Utc::now);

        for i in start..end {
            let node = &self.filtered[i];
            let session = &node.session;
            let is_selected = i == self.selected_index;
            let is_confirming_delete =
                self.confirming_delete_path.as_deref() == Some(session.path.as_path());
            let is_current = self.is_current_session_path(&session.path);

            let prefix = Self::build_tree_prefix(node);

            let has_name = session.name.is_some();
            let display = session.name.as_deref().unwrap_or(&session.first_message);
            let normalized: String = display
                .chars()
                .map(|c| {
                    if (c as u32) < 0x20 || c == '\x7f' {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            let normalized = js_trim(&normalized);

            // A swatch in the session's own colour, as its chip resolves it.
            let slot = session
                .color
                .map(i64::from)
                .unwrap_or_else(|| i64::from(session_color_slot_for(&session.id)));
            let swatch = t.fill(session_color_token(slot), " ");

            // The branch, only where it says something the row does not.
            let branch = match &session.branch {
                Some(b)
                    if !has_name && !b.is_empty() && !DEFAULT_BRANCHES.contains(&b.as_str()) =>
                {
                    b.as_str()
                }
                _ => "",
            };
            let branch_part = if branch.is_empty() {
                String::new()
            } else {
                format!("{GIT_BRANCH_GLYPH} {branch}  ")
            };
            let styled_branch = if branch.is_empty() {
                String::new()
            } else {
                format!(
                    "{}{}  ",
                    t.fg("dim", &format!("{GIT_BRANCH_GLYPH} ")),
                    t.fg("muted", branch)
                )
            };

            let age = format_session_date(session.modified, now);
            let mut right = format!("{} {age}", session.message_count);
            if !has_name {
                right = format!("{} {right}", session_slug_for(&session.id));
            }
            if self.show_cwd && !session.cwd.is_empty() {
                right = format!("{} {right}", shorten_path(&session.cwd));
            }
            if self.show_path {
                right = format!("{} {right}", shorten_path(&session.path.to_string_lossy()));
            }

            let cursor = if is_selected {
                t.fg("accent", SELECT_CURSOR)
            } else {
                select_gutter()
            };

            let prefix_width = visible_width(&prefix) as i64;
            let right_width = visible_width(&right) as i64 + 2;
            let available =
                w as i64 - 4 - prefix_width - visible_width(&branch_part) as i64 - right_width;
            let truncated = truncate_to_width(normalized, available.max(10) as usize, "…", false);

            let color = if is_confirming_delete {
                Some("error")
            } else if is_current {
                Some("accent")
            } else if has_name {
                Some("warning")
            } else {
                None
            };
            let mut styled = match color {
                Some(c) => t.fg(c, &truncated),
                None => truncated,
            };
            if is_selected {
                styled = t.bold(&styled);
            }

            let left = format!(
                "{cursor}{swatch} {}{styled_branch}{styled}",
                t.fg("dim", &prefix)
            );
            let spacing =
                (w as i64 - visible_width(&left) as i64 - visible_width(&right) as i64).max(1);
            let styled_right = t.fg(if is_confirming_delete { "error" } else { "dim" }, &right);
            let line = truncate_to_width(
                &format!("{left}{}{styled_right}", " ".repeat(spacing as usize)),
                w,
                "...",
                false,
            );
            lines.push(if is_selected {
                paint_selected_row(&line, w)
            } else {
                line
            });
        }

        if start > 0 || end < len {
            let scroll = format!("  ({}/{})", self.selected_index + 1, len);
            lines.push(t.fg("muted", &truncate_to_width(&scroll, w, "", false)));
        }
        lines
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_key(data);
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.search_input.set_focused(focused);
    }
}

// ---- deleting --------------------------------------------------------------

enum DeleteMethod {
    Trash,
    Unlink,
}

/// `deleteSessionFile`: the `trash` CLI first, then a plain unlink.
fn delete_session_file(path: &Path) -> Result<DeleteMethod, String> {
    let lossy = path.to_string_lossy();
    let mut cmd = std::process::Command::new("trash");
    if lossy.starts_with('-') {
        cmd.arg("--");
    }
    let trash = cmd.arg(path).stdin(std::process::Stdio::null()).output();
    let trash_ok = matches!(&trash, Ok(out) if out.status.success());
    if trash_ok || !path.exists() {
        return Ok(DeleteMethod::Trash);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(DeleteMethod::Unlink),
        Err(err) => {
            let mut parts = Vec::new();
            match &trash {
                Err(e) => parts.push(e.to_string()),
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let stderr = stderr.trim();
                    if !stderr.is_empty() {
                        parts.push(stderr.lines().next().unwrap_or(stderr).to_string());
                    }
                }
            }
            let hint = (!parts.is_empty()).then(|| {
                let joined = parts.join(" · ");
                format!("trash: {}", joined.chars().take(200).collect::<String>())
            });
            Err(match hint {
                Some(h) => format!("{err} ({h})"),
                None => err.to_string(),
            })
        }
    }
}

// ---- loading ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadReason {
    Initial,
    Refresh,
    Toggle,
}

enum LoadMsg {
    Progress {
        scope: SessionScope,
        seq: Option<u64>,
        loaded: usize,
        total: usize,
    },
    Done {
        scope: SessionScope,
        seq: Option<u64>,
        reason: LoadReason,
        result: Result<Vec<SessionInfo>, String>,
    },
}

/// Where a loader reports: progress, then one result. It may finish on
/// another thread; the picker applies it in [`SessionSelectorComponent::poll`].
#[derive(Clone)]
pub struct LoadSink {
    tx: Sender<LoadMsg>,
    scope: SessionScope,
    seq: Option<u64>,
    reason: LoadReason,
}

impl LoadSink {
    /// `onProgress(loaded, total)`.
    pub fn progress(&self, loaded: usize, total: usize) {
        let _ = self.tx.send(LoadMsg::Progress {
            scope: self.scope,
            seq: self.seq,
            loaded,
            total,
        });
    }

    /// The loaded sessions, or why loading failed.
    pub fn finish(&self, result: Result<Vec<SessionInfo>, String>) {
        let _ = self.tx.send(LoadMsg::Done {
            scope: self.scope,
            seq: self.seq,
            reason: self.reason,
            result,
        });
    }
}

/// Starts loading one scope's sessions.
pub type SessionsLoader = Box<dyn FnMut(LoadSink)>;

/// Renames a session; the picker reloads afterwards.
pub type RenameSessionFn = Box<dyn FnMut(&Path, &str) -> Result<(), String>>;

#[derive(Default)]
pub struct SessionSelectorOptions {
    pub rename_session: Option<RenameSessionFn>,
    /// Defaults to whether renaming is possible.
    pub show_rename_hint: Option<bool>,
    /// The manager the named-filter key is read from (the app's by default).
    pub keybindings: Option<Rc<KeybindingsManager>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Rename,
}

/// `SessionSelectorComponent`.
pub struct SessionSelectorComponent {
    frame: InputFrame,
    header: Rc<RefCell<SessionSelectorHeader>>,
    session_list: Rc<RefCell<SessionList>>,
    scope: SessionScope,
    sort_mode: SortMode,
    name_filter: NameFilter,
    current_sessions: Option<Vec<SessionInfo>>,
    all_sessions: Option<Vec<SessionInfo>>,
    current_loader: SessionsLoader,
    all_loader: SessionsLoader,
    on_select: Box<dyn FnMut(PathBuf)>,
    on_cancel: Box<dyn FnMut()>,
    rename_session: Option<RenameSessionFn>,
    current_loading: bool,
    all_loading: bool,
    all_load_seq: u64,
    mode: Mode,
    rename_input: Rc<RefCell<Input>>,
    rename_submitted: Rc<RefCell<Option<String>>>,
    rename_target: Option<PathBuf>,
    tx: Sender<LoadMsg>,
    rx: Receiver<LoadMsg>,
    focused: bool,
}

impl SessionSelectorComponent {
    pub fn new(
        current_loader: SessionsLoader,
        all_loader: SessionsLoader,
        on_select: Box<dyn FnMut(PathBuf)>,
        on_cancel: Box<dyn FnMut()>,
        options: SessionSelectorOptions,
        current_session_file: Option<&Path>,
    ) -> Self {
        let keybindings = options.keybindings.unwrap_or_else(|| {
            Rc::new(
                cortexcode_code_tui_keybindings::AppKeybindingsManager::create(None).into_manager(),
            )
        });
        let can_rename = options.rename_session.is_some();
        let header = Rc::new(RefCell::new(SessionSelectorHeader {
            scope: SessionScope::Current,
            sort_mode: SortMode::Threaded,
            name_filter: NameFilter::All,
            loading: false,
            load_progress: None,
            show_path: false,
            confirming_delete: false,
            status: None,
            status_deadline: None,
            show_rename_hint: options.show_rename_hint.unwrap_or(can_rename),
        }));
        let session_list = Rc::new(RefCell::new(SessionList::new(
            SortMode::Threaded,
            NameFilter::All,
            keybindings,
            current_session_file,
        )));

        let mut rename_input = Input::new();
        style_input(&mut rename_input);
        let rename_submitted = Rc::new(RefCell::new(None));
        let sink = rename_submitted.clone();
        rename_input.on_submit = Some(Box::new(move |value: &str| {
            *sink.borrow_mut() = Some(value.to_string());
        }));

        let (tx, rx) = channel();
        let mut selector = Self {
            frame: InputFrame::new(InputFrameOptions {
                title: Some("sessions".to_string()),
                ..Default::default()
            }),
            header,
            session_list,
            scope: SessionScope::Current,
            sort_mode: SortMode::Threaded,
            name_filter: NameFilter::All,
            current_sessions: None,
            all_sessions: None,
            current_loader,
            all_loader,
            on_select,
            on_cancel,
            rename_session: options.rename_session,
            current_loading: false,
            all_loading: false,
            all_load_seq: 0,
            mode: Mode::List,
            rename_input: Rc::new(RefCell::new(rename_input)),
            rename_submitted,
            rename_target: None,
            tx,
            rx,
            focused: false,
        };
        let list: ComponentHandle = selector.session_list.clone();
        selector.build_base_layout(list, true);
        selector.load_scope(SessionScope::Current, LoadReason::Initial);
        selector
    }

    /// The list (tests drive it directly, as the pin's do).
    pub fn session_list(&self) -> Rc<RefCell<SessionList>> {
        self.session_list.clone()
    }

    fn build_base_layout(&mut self, content: ComponentHandle, show_header: bool) {
        self.frame.clear();
        if show_header {
            self.frame.add_child(self.header.clone());
            self.frame.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        }
        self.frame.add_child(content);
    }

    /// Apply loads that finished and expire the timed status. `true` when
    /// something changed (render again).
    pub fn poll(&mut self) -> bool {
        let mut changed = self.header.borrow_mut().poll(Instant::now());
        while let Ok(msg) = self.rx.try_recv() {
            changed = true;
            self.apply(msg);
        }
        changed
    }

    fn apply(&mut self, msg: LoadMsg) {
        match msg {
            LoadMsg::Progress {
                scope,
                seq,
                loaded,
                total,
            } => {
                if scope != self.scope || seq.is_some_and(|s| s != self.all_load_seq) {
                    return;
                }
                self.header.borrow_mut().load_progress = Some((loaded, total));
            }
            LoadMsg::Done {
                scope,
                seq,
                reason,
                result,
            } => self.finish_load(scope, seq, reason, result),
        }
    }

    fn finish_load(
        &mut self,
        scope: SessionScope,
        seq: Option<u64>,
        reason: LoadReason,
        result: Result<Vec<SessionInfo>, String>,
    ) {
        let show_cwd = scope == SessionScope::All;
        match scope {
            SessionScope::Current => self.current_loading = false,
            SessionScope::All => self.all_loading = false,
        }
        match result {
            Ok(sessions) => {
                match scope {
                    SessionScope::Current => self.current_sessions = Some(sessions.clone()),
                    SessionScope::All => self.all_sessions = Some(sessions.clone()),
                }
                if scope != self.scope || seq.is_some_and(|s| s != self.all_load_seq) {
                    return;
                }
                self.header.borrow_mut().set_loading(false);
                let empty = sessions.is_empty();
                self.session_list
                    .borrow_mut()
                    .set_sessions(sessions, show_cwd);
                if scope == SessionScope::All
                    && empty
                    && self.current_sessions.as_ref().is_none_or(Vec::is_empty)
                {
                    (self.on_cancel)();
                }
            }
            Err(message) => {
                if scope != self.scope || seq.is_some_and(|s| s != self.all_load_seq) {
                    return;
                }
                let mut header = self.header.borrow_mut();
                header.set_loading(false);
                header.set_status(
                    Some((
                        StatusKind::Error,
                        format!("Failed to load sessions: {message}"),
                    )),
                    Some(Duration::from_millis(4000)),
                );
                drop(header);
                if reason == LoadReason::Initial {
                    self.session_list
                        .borrow_mut()
                        .set_sessions(Vec::new(), show_cwd);
                }
            }
        }
    }

    fn load_scope(&mut self, scope: SessionScope, reason: LoadReason) {
        match scope {
            SessionScope::Current => self.current_loading = true,
            SessionScope::All => self.all_loading = true,
        }
        let seq = (scope == SessionScope::All).then(|| {
            self.all_load_seq += 1;
            self.all_load_seq
        });
        {
            let mut header = self.header.borrow_mut();
            header.scope = scope;
            header.set_loading(true);
        }
        let sink = LoadSink {
            tx: self.tx.clone(),
            scope,
            seq,
            reason,
        };
        match scope {
            SessionScope::Current => (self.current_loader)(sink),
            SessionScope::All => (self.all_loader)(sink),
        }
    }

    fn visible_sessions(&self) -> Vec<SessionInfo> {
        let sessions = match self.scope {
            SessionScope::All => &self.all_sessions,
            SessionScope::Current => &self.current_sessions,
        };
        sessions.clone().unwrap_or_default()
    }

    fn enter_rename_mode(&mut self, path: PathBuf, current_name: Option<String>) {
        self.mode = Mode::Rename;
        self.rename_target = Some(path);
        {
            let mut input = self.rename_input.borrow_mut();
            input.set_value(current_name.unwrap_or_default());
            input.set_focused(true);
        }
        let t = theme();
        let mut panel = Container::new();
        panel.add_child(Rc::new(RefCell::new(Text::new(
            t.bold("Rename Session"),
            1,
            0,
        ))));
        panel.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        panel.add_child(self.rename_input.clone());
        panel.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        panel.add_child(Rc::new(RefCell::new(Text::new(
            t.fg(
                "muted",
                &format!(
                    "{} to save · {} to cancel",
                    key_text("tui.select.confirm"),
                    key_text("tui.select.cancel")
                ),
            ),
            1,
            0,
        ))));
        self.build_base_layout(Rc::new(RefCell::new(panel)), false);
    }

    fn exit_rename_mode(&mut self) {
        self.mode = Mode::List;
        self.rename_target = None;
        let list: ComponentHandle = self.session_list.clone();
        self.build_base_layout(list, true);
    }

    fn confirm_rename(&mut self, value: &str) {
        let next = js_trim(value);
        if next.is_empty() {
            return;
        }
        let Some(target) = self.rename_target.clone() else {
            self.exit_rename_mode();
            return;
        };
        let Some(rename) = self.rename_session.as_mut() else {
            self.exit_rename_mode();
            return;
        };
        if rename(&target, next).is_ok() {
            self.load_scope(self.scope, LoadReason::Refresh);
        }
        self.exit_rename_mode();
    }

    fn toggle_sort_mode(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Threaded => SortMode::Recent,
            SortMode::Recent => SortMode::Relevance,
            SortMode::Relevance => SortMode::Threaded,
        };
        self.header.borrow_mut().sort_mode = self.sort_mode;
        self.session_list.borrow_mut().set_sort_mode(self.sort_mode);
    }

    fn toggle_name_filter(&mut self) {
        self.name_filter = match self.name_filter {
            NameFilter::All => NameFilter::Named,
            NameFilter::Named => NameFilter::All,
        };
        self.header.borrow_mut().name_filter = self.name_filter;
        self.session_list
            .borrow_mut()
            .set_name_filter(self.name_filter);
    }

    fn toggle_scope(&mut self) {
        if self.scope == SessionScope::Current {
            self.scope = SessionScope::All;
            self.header.borrow_mut().scope = self.scope;
            if let Some(all) = self.all_sessions.clone() {
                self.header.borrow_mut().set_loading(false);
                self.session_list.borrow_mut().set_sessions(all, true);
                return;
            }
            if !self.all_loading {
                self.load_scope(SessionScope::All, LoadReason::Toggle);
            }
            return;
        }
        self.scope = SessionScope::Current;
        {
            let mut header = self.header.borrow_mut();
            header.scope = self.scope;
            header.set_loading(self.current_loading);
        }
        let current = self.current_sessions.clone().unwrap_or_default();
        self.session_list.borrow_mut().set_sessions(current, false);
    }

    fn delete_session(&mut self, path: PathBuf) {
        match delete_session_file(&path) {
            Ok(method) => {
                if let Some(s) = &mut self.current_sessions {
                    s.retain(|x| x.path != path);
                }
                if let Some(s) = &mut self.all_sessions {
                    s.retain(|x| x.path != path);
                }
                let sessions = self.visible_sessions();
                let show_cwd = self.scope == SessionScope::All;
                self.session_list
                    .borrow_mut()
                    .set_sessions(sessions, show_cwd);
                let message = match method {
                    DeleteMethod::Trash => "Session moved to trash",
                    DeleteMethod::Unlink => "Session deleted",
                };
                self.header.borrow_mut().set_status(
                    Some((StatusKind::Info, message.to_string())),
                    Some(Duration::from_millis(2000)),
                );
                self.load_scope(self.scope, LoadReason::Refresh);
            }
            Err(error) => {
                self.header.borrow_mut().set_status(
                    Some((StatusKind::Error, format!("Failed to delete: {error}"))),
                    Some(Duration::from_millis(3000)),
                );
            }
        }
    }

    /// Act on what the list reported.
    pub fn apply_list_events(&mut self, events: Vec<ListEvent>) {
        for event in events {
            match event {
                ListEvent::Select(path) => {
                    self.header.borrow_mut().set_status(None, None);
                    (self.on_select)(path);
                }
                ListEvent::Cancel => {
                    self.header.borrow_mut().set_status(None, None);
                    (self.on_cancel)();
                }
                ListEvent::ToggleScope => self.toggle_scope(),
                ListEvent::ToggleSort => self.toggle_sort_mode(),
                ListEvent::ToggleNameFilter => self.toggle_name_filter(),
                ListEvent::TogglePath(show) => self.header.borrow_mut().show_path = show,
                ListEvent::DeleteConfirmationChange(path) => {
                    self.header.borrow_mut().confirming_delete = path.is_some();
                }
                ListEvent::DeleteSession(path) => self.delete_session(path),
                ListEvent::Rename(path) => {
                    if self.rename_session.is_none() {
                        continue;
                    }
                    let loading = match self.scope {
                        SessionScope::Current => self.current_loading,
                        SessionScope::All => self.all_loading,
                    };
                    if loading {
                        continue;
                    }
                    let name = self
                        .visible_sessions()
                        .into_iter()
                        .find(|s| s.path == path)
                        .and_then(|s| s.name);
                    self.enter_rename_mode(path, name);
                }
                ListEvent::Error(message) => {
                    self.header.borrow_mut().set_status(
                        Some((StatusKind::Error, message)),
                        Some(Duration::from_millis(3000)),
                    );
                }
            }
        }
    }
}

impl Component for SessionSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        if self.mode == Mode::Rename {
            if get_keybindings().matches(data, "tui.select.cancel") {
                self.exit_rename_mode();
                return;
            }
            self.rename_input.borrow_mut().handle_input(data);
            let submitted = self.rename_submitted.borrow_mut().take();
            if let Some(value) = submitted {
                self.confirm_rename(&value);
            }
            return;
        }
        let events = self.session_list.borrow_mut().handle_key(data);
        self.apply_list_events(events);
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.session_list.borrow_mut().set_focused(focused);
        self.rename_input.borrow_mut().set_focused(focused);
    }
}

//! Reading back through the transcript (`modes/interactive/scroll-view.ts`).
//!
//! The TUI owns the mechanism: a pinned window over the line buffer, painted
//! on the alternate screen. This owns what the TUI cannot know: which keys
//! mean what, and how the indicator on the bottom row looks in the theme.
//!
//! At the prompt only the pager keys are live (page up/down, the two ends,
//! jump by turn, search), none of which the prompt uses. Once the view is
//! pinned it captures keys like a picker: line steps on the arrows, escape
//! back to live. Anything that is not a scroll key un-pins the view and then
//! does what it always did, so typing never echoes into a prompt scrolled off
//! screen.
//!
//! hoocode installs editor actions for the prompt keys and an input listener
//! for the pinned view. Both run here in one TUI input interceptor: an
//! interceptor gets the TUI in hand, which a Rust listener cannot. The prompt
//! keys are gated on the editor holding focus, as editor actions are, and app
//! actions win over the editor's own keys in both.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_keybindings::key_text;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_keys::{get_keybindings, is_key_release};
use cortexcode_tui_render::{ComponentHandle, Container, ScrollStatus, Tui};
use cortexcode_tui_util::{truncate_to_width, visible_width};

/// Which way to jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnDirection {
    Previous,
    Next,
}

/// Whether a chat child stands for something the user said.
pub type IsUserMessage = Rc<dyn Fn(&ComponentHandle) -> bool>;

/// `jumpToUserMessage`: pin the view to the previous or next user message.
/// Rows come from the last frame's offsets: a message's offset inside the
/// chat plus the chat's offset at the root. False when there is nowhere to go
/// (nothing rendered yet, no user message, or already at the end).
pub fn jump_to_user_message(
    ui: &mut Tui,
    chat: &Rc<RefCell<Container>>,
    direction: TurnDirection,
    is_user_message: &dyn Fn(&ComponentHandle) -> bool,
) -> bool {
    let width = ui.terminal.columns();
    let (Some(root_offsets), Some(chat_offsets)) = (
        ui.child_row_offsets(width),
        chat.borrow().child_row_offsets(width),
    ) else {
        return false;
    };
    let chat_handle: ComponentHandle = chat.clone();
    let Some(chat_index) = ui
        .children()
        .iter()
        .position(|c| Rc::ptr_eq(c, &chat_handle))
    else {
        return false;
    };
    let base = root_offsets[chat_index] as i64;
    let rows: Vec<i64> = chat
        .borrow()
        .children
        .iter()
        .zip(&chat_offsets)
        .filter(|(child, _)| is_user_message(child))
        .map(|(_, offset)| base + *offset as i64)
        .collect();
    if rows.is_empty() {
        return false;
    }
    // Live counts as below the last message, so the first `Previous` lands
    // on the most recent one.
    let here = ui.get_scroll_position().map_or(i64::MAX, |(top, ..)| top);
    let target = match direction {
        TurnDirection::Previous => rows.iter().rev().find(|row| **row < here),
        TurnDirection::Next => rows.iter().find(|row| **row > here),
    };
    match target {
        Some(&row) => {
            ui.scroll_to_row(row, None);
            true
        }
        None => false,
    }
}

/// Plain typed text rather than a chord (control bytes and escapes are keys).
fn is_typed_text(data: &str) -> bool {
    !data.is_empty() && data.chars().all(|c| (c as u32) >= 32 && c as u32 != 127)
}

/// `formatStatus`: the bottom row of a pinned view, "where am I" first and
/// "how do I get out" second; while searching, the query line.
pub fn format_scroll_status(status: &ScrollStatus) -> String {
    let width = status.width.max(0) as usize;
    if let Some(search) = &status.search {
        let hits = if search.query.is_empty() {
            String::new()
        } else if search.count == 0 {
            "  no matches".to_string()
        } else {
            format!("  {}/{}", search.index, search.count)
        };
        let caret = if search.typing { "\u{2588}" } else { "" };
        let left = format!(" search: {}{caret}{hits}", search.query);
        let keys = if search.typing {
            format!(
                "{} keep · {} cancel",
                key_text("tui.select.confirm"),
                key_text("app.scroll.exit")
            )
        } else {
            format!(
                "{}/{} step · {} done",
                key_text("app.scroll.searchNext"),
                key_text("app.scroll.searchPrevious"),
                key_text("app.scroll.exit")
            )
        };
        let gap = status.width - visible_width(&left) as i64 - visible_width(&keys) as i64 - 2;
        let body = if gap >= 0 {
            format!("{left}{}{keys} ", " ".repeat(gap as usize + 1))
        } else {
            format!("{left} ")
        };
        return theme().inverse(&truncate_to_width(&body, width, "", true));
    }

    let position = format!(
        "{}\u{2013}{} of {}",
        status.top, status.bottom, status.total
    );
    let place = if status.at_top {
        "start of session".to_string()
    } else {
        // JS Math.round on a non-negative ratio.
        let percent = (status.top as f64 / status.total.max(1) as f64 * 100.0 + 0.5).floor();
        format!("{percent}%")
    };
    let left = format!("{position}  {place}");
    let keys = [
        format!(
            "{}/{} line",
            key_text("app.scroll.lineUp"),
            key_text("app.scroll.lineDown")
        ),
        format!(
            "{}/{} page",
            key_text("app.scroll.pageUp"),
            key_text("app.scroll.pageDown")
        ),
        format!("{} top", key_text("app.scroll.top")),
        format!("{} live", key_text("app.scroll.exit")),
    ]
    .join(" · ");
    // The keys only when they fit whole: a truncated chord reads as another.
    let gap = status.width - visible_width(&left) as i64 - visible_width(&keys) as i64 - 3;
    let body = if gap >= 0 {
        format!(" {left}{}{keys} ", " ".repeat(gap as usize + 1))
    } else {
        format!(" {left} ")
    };
    theme().inverse(&truncate_to_width(&body, width, "", true))
}

/// The key state of the scroll view: the query being typed, if any.
pub struct ScrollView {
    editor: ComponentHandle,
    chat: Rc<RefCell<Container>>,
    is_user_message: IsUserMessage,
    typing: Option<String>,
}

fn matches(data: &str, id: &str) -> bool {
    get_keybindings().matches(data, id)
}

impl ScrollView {
    fn editor_focused(&self, ui: &Tui) -> bool {
        ui.focused().is_some_and(|f| Rc::ptr_eq(&f, &self.editor))
    }

    fn open_search(&mut self, ui: &mut Tui) {
        self.typing = Some(String::new());
        ui.set_scroll_search("", true);
    }

    fn jump(&self, ui: &mut Tui, direction: TurnDirection) {
        jump_to_user_message(ui, &self.chat, direction, &*self.is_user_message);
    }

    /// The prompt's scroll keys (hoocode's editor actions), while live.
    fn handle_live(&mut self, ui: &mut Tui, data: &str) -> bool {
        if is_typed_text(data) || !self.editor_focused(ui) {
            return false;
        }
        if matches(data, "app.scroll.pageUp") {
            ui.scroll_by_pages(-1);
        } else if matches(data, "app.scroll.pageDown") {
            ui.scroll_by_pages(1);
        } else if matches(data, "app.scroll.top") {
            ui.scroll_to_top();
        } else if matches(data, "app.scroll.bottom") {
            ui.scroll_to_live();
        } else if matches(data, "app.scroll.previousMessage") {
            self.jump(ui, TurnDirection::Previous);
        } else if matches(data, "app.scroll.nextMessage") {
            self.jump(ui, TurnDirection::Next);
        } else if matches(data, "app.scroll.search") {
            // Pin first, at the bottom, so the search works back from the
            // newest thing.
            ui.scroll_to_row(i64::MAX, None);
            if ui.scroll_pinned() {
                self.open_search(ui);
            }
        } else {
            return false;
        }
        true
    }

    /// One input; `true` when it was consumed.
    pub fn handle_input(&mut self, ui: &mut Tui, data: &str) -> bool {
        if !ui.scroll_pinned() {
            // The view can un-pin without passing through here (a wheel notch
            // reaching the bottom): a stale query must not swallow keys.
            self.typing = None;
            return self.handle_live(ui, data);
        }
        // A key coming back up is not a decision to stop reading.
        if is_key_release(data) {
            return true;
        }
        if self.typing.is_some() && !ui.scroll_search_active() {
            self.typing = None;
        }
        // The query line owns every printable character while it is open.
        if let Some(mut query) = self.typing.take() {
            if matches(data, "tui.select.confirm") {
                // An empty query committed is a cancelled one.
                if ui.scroll_search_query().is_empty() {
                    ui.clear_scroll_search();
                } else {
                    ui.commit_scroll_search();
                }
                return true;
            }
            if matches(data, "app.scroll.exit") {
                ui.clear_scroll_search();
                return true;
            }
            if matches(data, "tui.editor.deleteCharBackward") {
                query.pop();
                ui.set_scroll_search(&query, true);
                self.typing = Some(query);
                return true;
            }
            if is_typed_text(data) {
                query.push_str(data);
                ui.set_scroll_search(&query, true);
                self.typing = Some(query);
                return true;
            }
            // A chord while composing: commit, then the chord as usual.
            ui.commit_scroll_search();
        }

        if matches(data, "app.scroll.search") || matches(data, "app.scroll.searchInView") {
            self.open_search(ui);
            return true;
        }
        if ui.scroll_search_active() {
            if matches(data, "app.scroll.searchNext") {
                ui.scroll_search_step(-1);
                return true;
            }
            if matches(data, "app.scroll.searchPrevious") {
                ui.scroll_search_step(1);
                return true;
            }
            // One escape drops the search, the next leaves the view.
            if matches(data, "app.scroll.exit") {
                ui.clear_scroll_search();
                return true;
            }
        }

        // Exit before the ends, so bindings sharing a key still get out.
        if matches(data, "app.scroll.exit") {
            ui.scroll_to_live();
        } else if matches(data, "app.scroll.top") {
            ui.scroll_to_top();
        } else if matches(data, "app.scroll.bottom") {
            ui.scroll_to_live();
        } else if matches(data, "app.scroll.pageUp") {
            ui.scroll_by_pages(-1);
        } else if matches(data, "app.scroll.pageDown") {
            ui.scroll_by_pages(1);
        } else if matches(data, "app.scroll.lineUp") {
            ui.scroll_by_lines(-1);
        } else if matches(data, "app.scroll.lineDown") {
            ui.scroll_by_lines(1);
        } else if matches(data, "app.scroll.previousMessage") {
            self.jump(ui, TurnDirection::Previous);
        } else if matches(data, "app.scroll.nextMessage") {
            self.jump(ui, TurnDirection::Next);
        } else {
            // Anything else: back to live, and the key does its usual job.
            ui.scroll_to_live();
            return false;
        }
        true
    }
}

/// `installScrollView`: the indicator, the focus gate on pinning, and the
/// keys. `editor` is the prompt; pinning is allowed only while it has focus
/// (a picker keeps its own arrow keys).
pub fn install_scroll_view(
    ui: &mut Tui,
    editor: ComponentHandle,
    chat: Rc<RefCell<Container>>,
    is_user_message: IsUserMessage,
) {
    ui.set_scroll_status_formatter(Box::new(format_scroll_status));
    let gate = editor.clone();
    ui.can_pin_scroll = Some(Box::new(move |ui: &Tui| {
        ui.focused().is_some_and(|f| Rc::ptr_eq(&f, &gate))
    }));
    let mut view = ScrollView {
        editor,
        chat,
        is_user_message,
        typing: None,
    };
    ui.set_input_interceptor(Some(Box::new(move |ui: &mut Tui, data: &str| {
        view.handle_input(ui, data)
    })));
}

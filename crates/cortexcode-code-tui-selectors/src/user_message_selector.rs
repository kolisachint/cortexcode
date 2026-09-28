//! `components/user-message-selector.ts`: pick a user message to fork from.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_theme::{paint_selected_row, select_gutter, theme, SELECT_CURSOR};
use cortexcode_code_tui_widgets::input_frame::{InputFrame, InputFrameOptions};
use cortexcode_tui_components::Text;
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::js_regex::is_js_space;
use cortexcode_tui_util::truncate_to_width;

/// `UserMessageItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessageItem {
    /// Entry id in the session.
    pub id: String,
    pub text: String,
    pub timestamp: Option<String>,
}

/// What a key did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserMessageEvent {
    Select(String),
    Cancel,
}

/// `UserMessageList`: two rows per message, oldest first.
pub struct UserMessageList {
    messages: Vec<UserMessageItem>,
    selected_index: usize,
    max_visible: usize,
    events: Vec<UserMessageEvent>,
}

impl UserMessageList {
    fn new(messages: Vec<UserMessageItem>, initial_selected_id: Option<&str>) -> Self {
        let initial = initial_selected_id.and_then(|id| messages.iter().position(|m| m.id == id));
        // The given message, else the most recent.
        let selected_index = initial.unwrap_or(messages.len().saturating_sub(1));
        Self {
            messages,
            selected_index,
            max_visible: 10,
            events: Vec::new(),
        }
    }

    /// What the keys since the last call did.
    pub fn take_events(&mut self) -> Vec<UserMessageEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Component for UserMessageList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let width = width as usize;
        let mut lines = Vec::new();
        let len = self.messages.len();
        if len == 0 {
            lines.push(t.fg("muted", "  No user messages found"));
            return lines;
        }

        let start = (self.selected_index as i64 - (self.max_visible / 2) as i64)
            .min(len as i64 - self.max_visible as i64)
            .max(0) as usize;
        let end = (start + self.max_visible).min(len);

        for i in start..end {
            let message = &self.messages[i];
            let is_selected = i == self.selected_index;
            // Blank between entries, none after the last.
            if i > start {
                lines.push(String::new());
            }
            let normalized = message.text.replace('\n', " ");
            let normalized = normalized.trim_matches(is_js_space);

            let cursor = if is_selected {
                t.fg("accent", SELECT_CURSOR)
            } else {
                select_gutter()
            };
            let truncated = truncate_to_width(normalized, width.saturating_sub(2), "...", false);
            let message_line = if is_selected {
                cursor + &t.bold(&t.fg("accent", &truncated))
            } else {
                cursor + &truncated
            };
            lines.push(if is_selected {
                paint_selected_row(&message_line, width)
            } else {
                message_line
            });
            lines.push(t.fg("muted", &format!("  Message {} of {len}", i + 1)));
        }

        if start > 0 || end < len {
            lines.push(t.fg("muted", &format!("  ({}/{len})", self.selected_index + 1)));
        }
        lines
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        let len = self.messages.len();
        if kb.matches(data, "tui.select.up") {
            // Older, wrapping to the newest.
            self.selected_index = if self.selected_index == 0 {
                len.saturating_sub(1)
            } else {
                self.selected_index - 1
            };
        } else if kb.matches(data, "tui.select.down") {
            self.selected_index = if self.selected_index + 1 >= len {
                0
            } else {
                self.selected_index + 1
            };
        } else if kb.matches(data, "tui.select.confirm") {
            if let Some(selected) = self.messages.get(self.selected_index) {
                self.events
                    .push(UserMessageEvent::Select(selected.id.clone()));
            }
        } else if kb.matches(data, "tui.select.cancel") {
            self.events.push(UserMessageEvent::Cancel);
        }
    }
}

/// `UserMessageSelectorComponent`: the list under a one-line explanation.
/// With no messages it cancels itself after 100ms ([`Self::poll`]).
pub struct UserMessageSelectorComponent {
    frame: InputFrame,
    list: Rc<RefCell<UserMessageList>>,
    auto_cancel_at: Option<Instant>,
}

impl UserMessageSelectorComponent {
    pub fn new(messages: Vec<UserMessageItem>, initial_selected_id: Option<&str>) -> Self {
        let mut frame = InputFrame::new(InputFrameOptions {
            title: Some("fork from message".to_string()),
            ..Default::default()
        });
        frame.add_child(Rc::new(RefCell::new(Text::new(
            theme().fg(
                "muted",
                "Select a user message to copy the active path up to that point into a new session",
            ),
            0,
            0,
        ))));
        let auto_cancel_at = messages
            .is_empty()
            .then(|| Instant::now() + Duration::from_millis(100));
        let list = Rc::new(RefCell::new(UserMessageList::new(
            messages,
            initial_selected_id,
        )));
        frame.add_child(list.clone() as ComponentHandle);
        Self {
            frame,
            list,
            auto_cancel_at,
        }
    }

    /// `getMessageList()`.
    pub fn message_list(&self) -> Rc<RefCell<UserMessageList>> {
        self.list.clone()
    }

    /// What happened since the last call: keys, or the empty list's
    /// auto-cancel once its time is up.
    pub fn poll(&mut self, now: Instant) -> Vec<UserMessageEvent> {
        let mut events = self.list.borrow_mut().take_events();
        if self.auto_cancel_at.is_some_and(|at| now >= at) {
            self.auto_cancel_at = None;
            events.push(UserMessageEvent::Cancel);
        }
        events
    }

    /// When the auto-cancel is due, if pending.
    pub fn deadline(&self) -> Option<Instant> {
        self.auto_cancel_at
    }
}

impl Component for UserMessageSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.frame.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.frame.invalidate();
    }
}

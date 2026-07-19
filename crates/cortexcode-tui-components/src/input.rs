//! Single-line text input with horizontal scrolling, ported from
//! `components/input.ts`.
//!
//! `cursor` is a **byte offset** into `value` (UTF-8), unlike the
//! TypeScript original where it's a UTF-16 code-unit offset — grapheme
//! cluster boundaries are still respected identically for all but
//! astral-plane characters (surrogate pairs), which is an accepted,
//! documented deviation.

use cortexcode_tui_editing::{KillPushOptions, KillRing, UndoStack};
use cortexcode_tui_keys::{decode_kitty_printable, KeybindingsManager};
use cortexcode_tui_render::{Component, CURSOR_MARKER};
use cortexcode_tui_util::{
    is_punctuation_char, is_whitespace_char, slice_by_column, visible_width,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone)]
struct InputState {
    value: String,
    cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

pub type OnSubmitFn = Box<dyn FnMut(&str)>;

pub struct Input {
    value: String,
    cursor: usize,
    pub on_submit: Option<OnSubmitFn>,
    pub on_escape: Option<Box<dyn FnMut()>>,

    focused: bool,

    paste_buffer: String,
    is_in_paste: bool,

    kill_ring: KillRing,
    last_action: Option<LastAction>,

    undo_stack: UndoStack<InputState>,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub fn new() -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            on_submit: None,
            on_escape: None,
            focused: false,
            paste_buffer: String::new(),
            is_in_paste: false,
            kill_ring: KillRing::new(),
            last_action: None,
            undo_stack: UndoStack::new(),
        }
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.cursor.min(self.value.len());
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn handle_input_with(&mut self, data: &str, kb: &KeybindingsManager) {
        // Bracketed paste mode.
        if let Some(start_idx) = data.find("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            let mut owned = String::with_capacity(data.len());
            owned.push_str(&data[..start_idx]);
            owned.push_str(&data[start_idx + "\x1b[200~".len()..]);
            return self.handle_input_owned(owned, kb);
        }

        if self.is_in_paste {
            self.paste_buffer.push_str(data);
            if let Some(end_idx) = self.paste_buffer.find("\x1b[201~") {
                let paste_content = self.paste_buffer[..end_idx].to_string();
                self.handle_paste(&paste_content);
                self.is_in_paste = false;
                let remaining = self.paste_buffer[end_idx + "\x1b[201~".len()..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input_with(&remaining, kb);
                }
            }
            return;
        }

        if kb.matches(data, "tui.select.cancel") {
            if let Some(cb) = &mut self.on_escape {
                cb();
            }
            return;
        }

        if kb.matches(data, "tui.editor.undo") {
            self.undo();
            return;
        }

        if kb.matches(data, "tui.input.submit") || data == "\n" {
            if let Some(cb) = &mut self.on_submit {
                let value = self.value.clone();
                cb(&value);
            }
            return;
        }

        if kb.matches(data, "tui.editor.deleteCharBackward") {
            self.handle_backspace();
            return;
        }
        if kb.matches(data, "tui.editor.deleteCharForward") {
            self.handle_forward_delete();
            return;
        }
        if kb.matches(data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }
        if kb.matches(data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }
        if kb.matches(data, "tui.editor.deleteToLineStart") {
            self.delete_to_line_start();
            return;
        }
        if kb.matches(data, "tui.editor.deleteToLineEnd") {
            self.delete_to_line_end();
            return;
        }
        if kb.matches(data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if kb.matches(data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }

        if kb.matches(data, "tui.editor.cursorLeft") {
            self.last_action = None;
            if self.cursor > 0 {
                let before_cursor = &self.value[..self.cursor];
                let len = before_cursor
                    .graphemes(true)
                    .next_back()
                    .map(|g| g.len())
                    .unwrap_or(1);
                self.cursor -= len;
            }
            return;
        }
        if kb.matches(data, "tui.editor.cursorRight") {
            self.last_action = None;
            if self.cursor < self.value.len() {
                let after_cursor = &self.value[self.cursor..];
                let len = after_cursor
                    .graphemes(true)
                    .next()
                    .map(|g| g.len())
                    .unwrap_or(1);
                self.cursor += len;
            }
            return;
        }
        if kb.matches(data, "tui.editor.cursorLineStart") {
            self.last_action = None;
            self.cursor = 0;
            return;
        }
        if kb.matches(data, "tui.editor.cursorLineEnd") {
            self.last_action = None;
            self.cursor = self.value.len();
            return;
        }
        if kb.matches(data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }
        if kb.matches(data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        // Kitty CSI-u printable character (e.g. \x1b[97u for 'a'). Decode
        // before the control-char check since CSI-u sequences contain
        // \x1b, which would otherwise be rejected below.
        if let Some(ch) = decode_kitty_printable(data) {
            self.insert_str(&ch.to_string());
            return;
        }

        let has_control_chars = data.chars().any(|ch| {
            let code = ch as u32;
            code < 32 || code == 0x7f || (0x80..=0x9f).contains(&code)
        });
        if !has_control_chars {
            self.insert_str(data);
        }
    }

    fn handle_input_owned(&mut self, data: String, kb: &KeybindingsManager) {
        self.handle_input_with(&data, kb);
    }

    fn insert_str(&mut self, s: &str) {
        if is_whitespace_char(s) || self.last_action != Some(LastAction::TypeWord) {
            self.push_undo();
        }
        self.last_action = Some(LastAction::TypeWord);
        self.value.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn handle_backspace(&mut self) {
        self.last_action = None;
        if self.cursor > 0 {
            self.push_undo();
            let before_cursor = &self.value[..self.cursor];
            let len = before_cursor
                .graphemes(true)
                .next_back()
                .map(|g| g.len())
                .unwrap_or(1);
            self.value.replace_range(self.cursor - len..self.cursor, "");
            self.cursor -= len;
        }
    }

    fn handle_forward_delete(&mut self) {
        self.last_action = None;
        if self.cursor < self.value.len() {
            self.push_undo();
            let after_cursor = &self.value[self.cursor..];
            let len = after_cursor
                .graphemes(true)
                .next()
                .map(|g| g.len())
                .unwrap_or(1);
            self.value.replace_range(self.cursor..self.cursor + len, "");
        }
    }

    fn delete_to_line_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let deleted = self.value[..self.cursor].to_string();
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.kill_ring.push(
            &deleted,
            KillPushOptions {
                prepend: true,
                accumulate,
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value.replace_range(..self.cursor, "");
        self.cursor = 0;
    }

    fn delete_to_line_end(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.push_undo();
        let deleted = self.value[self.cursor..].to_string();
        let accumulate = self.last_action == Some(LastAction::Kill);
        self.kill_ring.push(
            &deleted,
            KillPushOptions {
                prepend: false,
                accumulate,
            },
        );
        self.last_action = Some(LastAction::Kill);
        self.value.truncate(self.cursor);
    }

    fn delete_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let was_kill = self.last_action == Some(LastAction::Kill);
        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_backwards();
        let delete_from = self.cursor;
        self.cursor = old_cursor;

        let deleted = self.value[delete_from..self.cursor].to_string();
        self.kill_ring.push(
            &deleted,
            KillPushOptions {
                prepend: true,
                accumulate: was_kill,
            },
        );
        self.last_action = Some(LastAction::Kill);

        self.value.replace_range(delete_from..self.cursor, "");
        self.cursor = delete_from;
    }

    fn delete_word_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let was_kill = self.last_action == Some(LastAction::Kill);
        self.push_undo();

        let old_cursor = self.cursor;
        self.move_word_forwards();
        let delete_to = self.cursor;
        self.cursor = old_cursor;

        let deleted = self.value[self.cursor..delete_to].to_string();
        self.kill_ring.push(
            &deleted,
            KillPushOptions {
                prepend: false,
                accumulate: was_kill,
            },
        );
        self.last_action = Some(LastAction::Kill);

        self.value.replace_range(self.cursor..delete_to, "");
    }

    fn yank(&mut self) {
        let Some(text) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };
        self.push_undo();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo();

        let prev_text = self.kill_ring.peek().unwrap_or("").to_string();
        let start = self.cursor.saturating_sub(prev_text.len());
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;

        self.kill_ring.rotate();
        let text = self.kill_ring.peek().unwrap_or("").to_string();
        self.value.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.last_action = Some(LastAction::Yank);
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(&InputState {
            value: self.value.clone(),
            cursor: self.cursor,
        });
    }

    fn undo(&mut self) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.value = snapshot.value;
        self.cursor = snapshot.cursor;
        self.last_action = None;
    }

    fn move_word_backwards(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.last_action = None;
        let text_before_cursor = &self.value[..self.cursor];
        let mut graphemes: Vec<&str> = text_before_cursor.graphemes(true).collect();

        while matches!(graphemes.last(), Some(g) if is_whitespace_char(g)) {
            self.cursor -= graphemes.pop().unwrap().len();
        }

        if let Some(&last) = graphemes.last() {
            if is_punctuation_char(last) {
                while matches!(graphemes.last(), Some(g) if is_punctuation_char(g)) {
                    self.cursor -= graphemes.pop().unwrap().len();
                }
            } else {
                while matches!(graphemes.last(), Some(g) if !is_whitespace_char(g) && !is_punctuation_char(g))
                {
                    self.cursor -= graphemes.pop().unwrap().len();
                }
            }
        }
    }

    fn move_word_forwards(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.last_action = None;
        let text_after_cursor = &self.value[self.cursor..];
        let mut it = text_after_cursor.graphemes(true).peekable();

        while matches!(it.peek(), Some(g) if is_whitespace_char(g)) {
            self.cursor += it.next().unwrap().len();
        }

        if let Some(&first) = it.peek() {
            if is_punctuation_char(first) {
                while matches!(it.peek(), Some(g) if is_punctuation_char(g)) {
                    self.cursor += it.next().unwrap().len();
                }
            } else {
                while matches!(it.peek(), Some(g) if !is_whitespace_char(g) && !is_punctuation_char(g))
                {
                    self.cursor += it.next().unwrap().len();
                }
            }
        }
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.last_action = None;
        self.push_undo();

        let clean_text = pasted_text
            .replace("\r\n", "")
            .replace(['\r', '\n'], "")
            .replace('\t', "    ");

        self.value.insert_str(self.cursor, &clean_text);
        self.cursor += clean_text.len();
    }
}

impl Component for Input {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let prompt = "> ";
        let available_width = width as i64 - prompt.chars().count() as i64;

        if available_width <= 0 {
            return vec![prompt.to_string()];
        }
        let available_width = available_width as usize;

        let total_width = visible_width(&self.value);
        let visible_text;
        let mut cursor_display: usize;

        if total_width < available_width {
            visible_text = self.value.clone();
            cursor_display = self.cursor;
        } else {
            let scroll_width = if self.cursor == self.value.len() {
                available_width.saturating_sub(1)
            } else {
                available_width
            };
            let cursor_col = visible_width(&self.value[..self.cursor]);

            if scroll_width > 0 {
                let half_width = scroll_width / 2;
                let start_col = if cursor_col < half_width {
                    0
                } else if cursor_col > total_width.saturating_sub(half_width) {
                    total_width.saturating_sub(scroll_width)
                } else {
                    cursor_col.saturating_sub(half_width)
                };

                visible_text = slice_by_column(&self.value, start_col, scroll_width, true);
                let before_cursor = slice_by_column(
                    &self.value,
                    start_col,
                    cursor_col.saturating_sub(start_col),
                    true,
                );
                cursor_display = before_cursor.len();
            } else {
                visible_text = String::new();
                cursor_display = 0;
            }
        }
        cursor_display = cursor_display.min(visible_text.len());

        let cursor_grapheme = visible_text[cursor_display..].graphemes(true).next();
        let before_cursor = visible_text[..cursor_display].to_string();
        let at_cursor = cursor_grapheme.unwrap_or(" ").to_string();
        let after_cursor = if cursor_display + at_cursor.len() <= visible_text.len() {
            visible_text[cursor_display + at_cursor.len()..].to_string()
        } else {
            String::new()
        };

        let marker = if self.focused { CURSOR_MARKER } else { "" };
        let cursor_char = format!("\x1b[7m{at_cursor}\x1b[27m");
        let text_with_cursor = format!("{before_cursor}{marker}{cursor_char}{after_cursor}");

        let visual_length = visible_width(&text_with_cursor);
        let padding = " ".repeat(available_width.saturating_sub(visual_length));
        vec![format!("{prompt}{text_with_cursor}{padding}")]
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        Input::set_focused(self, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_tui_keys::default_tui_keybindings;
    use std::collections::HashMap;

    fn kb() -> KeybindingsManager {
        KeybindingsManager::new(default_tui_keybindings(), HashMap::new())
    }

    fn type_str(input: &mut Input, s: &str, kb: &KeybindingsManager) {
        for ch in s.chars() {
            input.handle_input_with(&ch.to_string(), kb);
        }
    }

    #[test]
    fn typing_appends_characters() {
        let mut input = Input::new();
        type_str(&mut input, "hello", &kb());
        assert_eq!(input.get_value(), "hello");
    }

    #[test]
    fn backspace_removes_last_character() {
        let mut input = Input::new();
        type_str(&mut input, "hello", &kb());
        input.handle_input_with("\x7f", &kb());
        assert_eq!(input.get_value(), "hell");
    }

    #[test]
    fn cursor_left_right_move_by_grapheme() {
        let mut input = Input::new();
        type_str(&mut input, "ab", &kb());
        input.handle_input_with("\x1b[D", &kb()); // left
        input.handle_input_with("x", &kb());
        assert_eq!(input.get_value(), "axb");
    }

    #[test]
    fn undo_restores_previous_value() {
        let mut input = Input::new();
        type_str(&mut input, "a", &kb());
        type_str(&mut input, " b", &kb());
        input.handle_input_with("\x1f", &kb()); // ctrl+-
        assert_eq!(input.get_value(), "a");
    }

    #[test]
    fn submit_calls_on_submit_with_value() {
        let mut input = Input::new();
        type_str(&mut input, "hi", &kb());
        let received = std::rc::Rc::new(std::cell::RefCell::new(None));
        let received_clone = received.clone();
        input.on_submit = Some(Box::new(move |v| {
            *received_clone.borrow_mut() = Some(v.to_string())
        }));
        input.handle_input_with("\r", &kb());
        assert_eq!(received.borrow().as_deref(), Some("hi"));
    }

    #[test]
    fn escape_calls_on_escape() {
        let mut input = Input::new();
        let called = std::rc::Rc::new(std::cell::Cell::new(false));
        let called_clone = called.clone();
        input.on_escape = Some(Box::new(move || called_clone.set(true)));
        input.handle_input_with("\x1b", &kb());
        assert!(called.get());
    }

    #[test]
    fn delete_to_line_start_and_end() {
        let mut input = Input::new();
        type_str(&mut input, "hello world", &kb());
        input.handle_input_with("\x01", &kb()); // ctrl+a -> line start
        input.handle_input_with("\x0b", &kb()); // ctrl+k -> delete to end
        assert_eq!(input.get_value(), "");
    }

    #[test]
    fn word_delete_backward() {
        let mut input = Input::new();
        type_str(&mut input, "hello world", &kb());
        input.handle_input_with("\x17", &kb()); // ctrl+w
        assert_eq!(input.get_value(), "hello ");
    }

    #[test]
    fn yank_reinserts_killed_text() {
        let mut input = Input::new();
        type_str(&mut input, "hello world", &kb());
        input.handle_input_with("\x17", &kb()); // ctrl+w kills "world"
        input.handle_input_with("\x19", &kb()); // ctrl+y yanks it back
        assert_eq!(input.get_value(), "hello world");
    }

    #[test]
    fn render_shows_prompt_and_value() {
        let mut input = Input::new();
        type_str(&mut input, "hi", &kb());
        let lines = input.render(20);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("> "));
        assert!(lines[0].contains("hi") || lines[0].contains('h'));
    }

    #[test]
    fn set_value_clamps_cursor() {
        let mut input = Input::new();
        type_str(&mut input, "hello", &kb());
        input.set_value("hi");
        assert_eq!(input.get_value(), "hi");
        // Further backspaces should not panic despite the shorter value.
        input.handle_input_with("\x7f", &kb());
        assert_eq!(input.get_value(), "h");
    }

    #[test]
    fn c1_control_characters_are_not_inserted() {
        let mut input = Input::new();
        // U+0090 is a C1 control character (0x80-0x9F), not a bound keybinding.
        input.handle_input_with("\u{0090}", &kb());
        assert_eq!(input.get_value(), "");
    }
}

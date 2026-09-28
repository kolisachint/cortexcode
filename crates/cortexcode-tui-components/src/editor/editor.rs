//! Multi-line prompt editor, ported from the pin's `components/editor.ts`.
//!
//! Faithful to the TypeScript, with these adaptations:
//! - Columns are UTF-16 code units (JS string indices); lines are Rust
//!   strings sliced through [`slice16`].
//! - The TUI reference becomes an [`EditorHost`]: the terminal's row count
//!   (for the visible-line cap and page moves) and a render request.
//! - Autocomplete providers are synchronous here, so a request resolves
//!   inside the keystroke that made it, except the 20ms debounce in `@`/`#`
//!   contexts, which is kept: such a request waits until
//!   [`Editor::poll_autocomplete`] runs it after its deadline. There is no
//!   in-flight request to abort.
//! - The select-list theme is a factory, since the list is rebuilt per popup
//!   and its colour closures are not `Clone`.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use cortexcode_tui_editing::{KillPushOptions, KillRing, UndoStack};
use cortexcode_tui_keys::{decode_printable_key, get_keybindings, matches_key, KeybindingsManager};
use cortexcode_tui_render::{Component, CURSOR_MARKER};
use cortexcode_tui_util::{is_punctuation_char, is_whitespace_char, visible_width};

use super::word_wrap::{
    find_paste_markers, is_paste_marker, len16, segment_with_markers, slice16, slice16_from,
    word_wrap_segments, Segment,
};
use crate::autocomplete::{AutocompleteItem, AutocompleteProvider, AutocompleteSuggestions};
use crate::color::ColorFn;
use crate::frame::{
    render_frame_edge, FrameBorderChars, FrameBorderCharsOverride, FrameBorderStyle, FrameEdge,
    FrameEdgeOptions, FrameLabel,
};
use crate::select_list::{SelectItem, SelectList, SelectListLayoutOptions, SelectListTheme};

/// `EditorBorderStyle` (an alias of the frame's).
pub type EditorBorderStyle = FrameBorderStyle;
/// `EditorTopBorderLabel`.
pub type EditorTopBorderLabel = FrameLabel;

pub type TextCallback = Box<dyn FnMut(&str)>;

pub fn identity_select_theme_color() -> ColorFn {
    Box::new(|s: &str| s.to_string())
}

/// `EditorTheme`.
pub struct EditorTheme {
    pub border_color: ColorFn,
    pub border_chars: Option<FrameBorderCharsOverride>,
    /// Builds the autocomplete list's theme each time a popup opens.
    pub select_list: Box<dyn Fn() -> SelectListTheme>,
}

/// `EditorOptions`.
#[derive(Default)]
pub struct EditorOptions {
    pub padding_x: Option<usize>,
    pub autocomplete_max_visible: Option<usize>,
    pub border: Option<EditorBorderStyle>,
}

/// What the editor needs from the TUI it lives in.
pub struct EditorHost {
    /// The terminal's current row count.
    pub rows: Box<dyn Fn() -> u16>,
    /// Ask for a frame (after work that completes outside a keystroke).
    pub request_render: Box<dyn Fn()>,
}

impl Default for EditorHost {
    fn default() -> Self {
        Self {
            rows: Box::new(|| 24),
            request_render: Box::new(|| {}),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EditorState {
    lines: Vec<String>,
    cursor_line: usize,
    /// UTF-16 offset into `lines[cursor_line]`.
    cursor_col: usize,
}

struct LayoutLine {
    text: String,
    has_cursor: bool,
    cursor_pos: Option<usize>,
}

#[derive(Clone, Copy)]
struct VisualLine {
    logical_line: usize,
    start_col: usize,
    length: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LastAction {
    Kill,
    Yank,
    TypeWord,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JumpDirection {
    Forward,
    Backward,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AutocompleteState {
    Regular,
    Force,
}

const ATTACHMENT_AUTOCOMPLETE_DEBOUNCE: Duration = Duration::from_millis(20);

/// `(?:^|[ \t])(?:@(?:"[^"]*|[^\s]*)|#[^\s]*)$`
fn is_symbol_autocomplete_context(text: &str) -> bool {
    text.char_indices().any(|(i, c)| {
        if c != '@' && c != '#' {
            return false;
        }
        if i > 0 && !text[..i].ends_with([' ', '\t']) {
            return false;
        }
        let rest = &text[i + 1..];
        if c == '@' {
            if let Some(quoted) = rest.strip_prefix('"') {
                if !quoted.contains('"') {
                    return true;
                }
            }
        }
        !rest.contains(is_js_space)
    })
}

fn slash_command_layout() -> SelectListLayoutOptions {
    SelectListLayoutOptions {
        min_primary_column_width: Some(12),
        max_primary_column_width: Some(32),
        ..Default::default()
    }
}

/// JS `\s` (whitespace, line terminators, BOM).
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// `(?:^|[\s])[@#][^\s]*$`: the token after the last whitespace starts with
/// `@` or `#`.
fn in_symbol_context(text_before_cursor: &str) -> bool {
    let token = match text_before_cursor.rfind(is_js_space) {
        Some(i) => {
            let ws_len = text_before_cursor[i..].chars().next().unwrap().len_utf8();
            &text_before_cursor[i + ws_len..]
        }
        None => text_before_cursor,
    };
    token.starts_with('@') || token.starts_with('#')
}

/// JS `String.prototype.trim` (whitespace and line terminators).
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// UTF-16 code unit before `col` in `line` equals `ch`.
fn unit_before_is(line: &str, col: usize, ch: char) -> bool {
    col > 0 && slice16(line, col - 1, col) == ch.to_string()
}

/// UTF-16 index of char index `i`.
fn chars_to_16(s: &str, i: usize) -> usize {
    s.chars().take(i).map(char::len_utf16).sum()
}

/// Char index of UTF-16 index `i`.
fn index16_to_chars(s: &str, i: usize) -> usize {
    let mut units = 0;
    let mut chars = 0;
    for ch in s.chars() {
        if units >= i {
            break;
        }
        units += ch.len_utf16();
        chars += 1;
    }
    chars
}

pub struct Editor {
    state: EditorState,
    focused: bool,

    host: EditorHost,
    theme: EditorTheme,
    padding_x: usize,
    border: EditorBorderStyle,
    border_chars: FrameBorderChars,

    /// Last layout width, for cursor navigation.
    last_width: usize,
    /// Vertical scroll over layout lines.
    scroll_offset: usize,

    /// Border color (can be changed dynamically).
    pub border_color: ColorFn,
    /// Label laid into the top border, flush right (the session chip).
    pub top_border_label: Option<EditorTopBorderLabel>,
    /// Prompt prefix shown on the first line (e.g. `❯`).
    pub prompt_prefix: String,
    pub prompt_color: ColorFn,

    autocomplete_provider: Option<Box<dyn AutocompleteProvider>>,
    autocomplete_list: Option<SelectList>,
    autocomplete_state: Option<AutocompleteState>,
    autocomplete_prefix: String,
    autocomplete_max_visible: usize,
    /// A debounced request: `(deadline, force, explicit_tab)`.
    pending_autocomplete: Option<(Instant, bool, bool)>,

    pastes: BTreeMap<u64, String>,
    paste_counter: u64,

    paste_buffer: String,
    is_in_paste: bool,

    history: Vec<String>,
    /// -1 = not browsing, 0 = most recent, 1 = older, ...
    history_index: i64,

    kill_ring: KillRing,
    last_action: Option<LastAction>,

    jump_mode: Option<JumpDirection>,

    /// Preferred visual column for vertical movement (sticky column).
    preferred_visual_col: Option<usize>,
    /// The pre-snap cursor column when snapped to an atomic segment's start.
    snapped_from_cursor_col: Option<usize>,

    undo_stack: UndoStack<EditorState>,

    pub on_submit: Option<TextCallback>,
    pub on_change: Option<TextCallback>,
    /// Called when the completion list appears or disappears.
    pub on_autocomplete_visibility_change: Option<Box<dyn FnMut(bool)>>,
    pub disable_submit: bool,
}

fn clamp_max_visible(v: usize) -> usize {
    v.clamp(3, 20)
}

impl Editor {
    pub fn new(host: EditorHost, theme: EditorTheme, options: EditorOptions) -> Self {
        let border_chars = FrameBorderChars::with_overrides(theme.border_chars.as_ref());
        let border_color: ColorFn = {
            // `this.borderColor = theme.borderColor`: the theme's function is
            // moved into the public field; the theme keeps an identity.
            Box::new(|s: &str| s.to_string())
        };
        let mut editor = Self {
            state: EditorState {
                lines: vec![String::new()],
                cursor_line: 0,
                cursor_col: 0,
            },
            focused: false,
            host,
            theme,
            padding_x: options.padding_x.unwrap_or(0),
            border: options.border.unwrap_or(FrameBorderStyle::Rule),
            border_chars,
            last_width: 80,
            scroll_offset: 0,
            border_color,
            top_border_label: None,
            prompt_prefix: String::new(),
            prompt_color: Box::new(|s: &str| s.to_string()),
            autocomplete_provider: None,
            autocomplete_list: None,
            autocomplete_state: None,
            autocomplete_prefix: String::new(),
            autocomplete_max_visible: clamp_max_visible(
                options.autocomplete_max_visible.unwrap_or(5),
            ),
            pending_autocomplete: None,
            pastes: BTreeMap::new(),
            paste_counter: 0,
            paste_buffer: String::new(),
            is_in_paste: false,
            history: Vec::new(),
            history_index: -1,
            kill_ring: KillRing::new(),
            last_action: None,
            jump_mode: None,
            preferred_visual_col: None,
            snapped_from_cursor_col: None,
            undo_stack: UndoStack::new(),
            on_submit: None,
            on_change: None,
            on_autocomplete_visibility_change: None,
            disable_submit: false,
        };
        std::mem::swap(&mut editor.border_color, &mut editor.theme.border_color);
        editor
    }

    fn valid_paste_ids(&self) -> HashSet<u64> {
        self.pastes.keys().copied().collect()
    }

    /// Segment text with paste-marker awareness (valid ids only).
    fn segment(&self, text: &str) -> Vec<Segment> {
        segment_with_markers(text, &self.valid_paste_ids())
    }

    fn current_line(&self) -> &str {
        self.state
            .lines
            .get(self.state.cursor_line)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn line_at(&self, i: usize) -> &str {
        self.state.lines.get(i).map(String::as_str).unwrap_or("")
    }

    fn request_render(&self) {
        (self.host.request_render)();
    }

    fn emit_change(&mut self) {
        let text = self.get_text();
        if let Some(cb) = &mut self.on_change {
            cb(&text);
        }
    }

    pub fn get_padding_x(&self) -> usize {
        self.padding_x
    }

    pub fn set_padding_x(&mut self, padding: usize) {
        if self.padding_x != padding {
            self.padding_x = padding;
            self.request_render();
        }
    }

    pub fn get_border(&self) -> EditorBorderStyle {
        self.border
    }

    pub fn set_border(&mut self, border: EditorBorderStyle) {
        if self.border != border {
            self.border = border;
            self.request_render();
        }
    }

    pub fn get_autocomplete_max_visible(&self) -> usize {
        self.autocomplete_max_visible
    }

    pub fn set_autocomplete_max_visible(&mut self, max_visible: usize) {
        let v = clamp_max_visible(max_visible);
        if self.autocomplete_max_visible != v {
            self.autocomplete_max_visible = v;
            self.request_render();
        }
    }

    pub fn set_autocomplete_provider(&mut self, provider: Box<dyn AutocompleteProvider>) {
        self.cancel_autocomplete();
        self.autocomplete_provider = Some(provider);
    }

    /// Add a prompt to history for up/down navigation (after submission).
    pub fn add_to_history(&mut self, text: &str) {
        let trimmed = js_trim(text);
        if trimmed.is_empty() {
            return;
        }
        if self.history.first().is_some_and(|h| h == trimmed) {
            return;
        }
        self.history.insert(0, trimmed.to_string());
        if self.history.len() > 100 {
            self.history.pop();
        }
    }

    fn is_editor_empty(&self) -> bool {
        self.state.lines.len() == 1 && self.state.lines[0].is_empty()
    }

    fn is_on_first_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) == 0
    }

    fn is_on_last_visual_line(&self) -> bool {
        let visual_lines = self.build_visual_line_map(self.last_width);
        self.find_current_visual_line(&visual_lines) + 1 == visual_lines.len()
    }

    fn navigate_history(&mut self, direction: i64) {
        self.last_action = None;
        if self.history.is_empty() {
            return;
        }
        let new_index = self.history_index - direction;
        if new_index < -1 || new_index >= self.history.len() as i64 {
            return;
        }
        if self.history_index == -1 && new_index >= 0 {
            self.push_undo_snapshot();
        }
        self.history_index = new_index;
        if self.history_index == -1 {
            self.set_text_internal("");
        } else {
            let text = self.history[self.history_index as usize].clone();
            self.set_text_internal(&text);
        }
    }

    /// `setTextInternal`: replaces the text without touching history state.
    fn set_text_internal(&mut self, text: &str) {
        self.state.lines = text.split('\n').map(String::from).collect();
        self.state.cursor_line = self.state.lines.len() - 1;
        let len = len16(self.current_line());
        self.set_cursor_col(len);
        self.scroll_offset = 0;
        self.emit_change();
    }

    fn render_border(
        &self,
        edge: FrameEdge,
        hidden: usize,
        bar_width: usize,
        is_box: bool,
    ) -> String {
        let color = |s: &str| (self.border_color)(s);
        render_frame_edge(&FrameEdgeOptions {
            edge,
            bar_width,
            is_box,
            chars: &self.border_chars,
            color: &color,
            label: self.top_border_label.as_ref(),
            hidden,
        })
    }

    fn layout_text(&self, content_width: usize) -> Vec<LayoutLine> {
        let mut layout_lines = Vec::new();
        if self.state.lines.is_empty() || self.is_editor_empty() {
            layout_lines.push(LayoutLine {
                text: String::new(),
                has_cursor: true,
                cursor_pos: Some(0),
            });
            return layout_lines;
        }
        for (i, line) in self.state.lines.iter().enumerate() {
            let is_current_line = i == self.state.cursor_line;
            if visible_width(line) <= content_width {
                layout_lines.push(LayoutLine {
                    text: line.clone(),
                    has_cursor: is_current_line,
                    cursor_pos: is_current_line.then_some(self.state.cursor_col),
                });
                continue;
            }
            let chunks = word_wrap_segments(line, content_width, Some(self.segment(line)));
            let count = chunks.len();
            for (chunk_index, chunk) in chunks.into_iter().enumerate() {
                let cursor_pos = self.state.cursor_col;
                let is_last_chunk = chunk_index + 1 == count;
                let mut has_cursor_in_chunk = false;
                let mut adjusted = 0;
                if is_current_line {
                    if is_last_chunk {
                        has_cursor_in_chunk = cursor_pos >= chunk.start_index;
                        adjusted = cursor_pos.saturating_sub(chunk.start_index);
                    } else {
                        has_cursor_in_chunk =
                            cursor_pos >= chunk.start_index && cursor_pos < chunk.end_index;
                        if has_cursor_in_chunk {
                            adjusted = (cursor_pos - chunk.start_index).min(len16(&chunk.text));
                        }
                    }
                }
                layout_lines.push(LayoutLine {
                    text: chunk.text,
                    has_cursor: has_cursor_in_chunk,
                    cursor_pos: has_cursor_in_chunk.then_some(adjusted),
                });
            }
        }
        layout_lines
    }

    pub fn get_text(&self) -> String {
        self.state.lines.join("\n")
    }

    fn expand_paste_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (paste_id, content) in &self.pastes {
            let markers: Vec<(usize, usize)> = find_paste_markers(&result)
                .into_iter()
                .filter(|(_, _, id)| id == paste_id)
                .map(|(s, e, _)| (s, e))
                .collect();
            for (s, e) in markers.into_iter().rev() {
                result.replace_range(s..e, content);
            }
        }
        result
    }

    /// Text with paste markers expanded to their content.
    pub fn get_expanded_text(&self) -> String {
        self.expand_paste_markers(&self.state.lines.join("\n"))
    }

    pub fn get_lines(&self) -> Vec<String> {
        self.state.lines.clone()
    }

    /// `(line, col)`, col in UTF-16 units.
    pub fn get_cursor(&self) -> (usize, usize) {
        (self.state.cursor_line, self.state.cursor_col)
    }

    pub fn set_text(&mut self, text: &str) {
        self.cancel_autocomplete();
        self.last_action = None;
        self.history_index = -1;
        let normalized = Self::normalize_text(text);
        if self.get_text() != normalized {
            self.push_undo_snapshot();
        }
        self.set_text_internal(&normalized);
    }

    /// Insert text at the cursor as one undo step (programmatic insertion).
    pub fn insert_text_at_cursor(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.cancel_autocomplete();
        self.push_undo_snapshot();
        self.last_action = None;
        self.history_index = -1;
        self.insert_text_at_cursor_internal(text);
    }

    /// Normalize line endings and expand tabs to 4 spaces.
    fn normalize_text(text: &str) -> String {
        text.replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ")
    }

    fn insert_text_at_cursor_internal(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = Self::normalize_text(text);
        let inserted: Vec<&str> = normalized.split('\n').collect();
        let current = self.current_line().to_string();
        let before = slice16(&current, 0, self.state.cursor_col).to_string();
        let after = slice16_from(&current, self.state.cursor_col).to_string();
        let cl = self.state.cursor_line;
        if inserted.len() == 1 {
            self.state.lines[cl] = format!("{before}{normalized}{after}");
            let col = self.state.cursor_col + len16(&normalized);
            self.set_cursor_col(col);
        } else {
            let mut lines: Vec<String> = self.state.lines[..cl].to_vec();
            lines.push(format!("{before}{}", inserted[0]));
            lines.extend(
                inserted[1..inserted.len() - 1]
                    .iter()
                    .map(|s| s.to_string()),
            );
            let last = inserted[inserted.len() - 1];
            lines.push(format!("{last}{after}"));
            lines.extend(self.state.lines[cl + 1..].iter().cloned());
            self.state.lines = lines;
            self.state.cursor_line += inserted.len() - 1;
            self.set_cursor_col(len16(last));
        }
        self.emit_change();
    }

    fn insert_character(&mut self, ch: &str, skip_undo_coalescing: bool) {
        self.history_index = -1;
        // Fish-style undo coalescing: word chars coalesce; a space captures
        // state before itself.
        if !skip_undo_coalescing {
            if is_whitespace_char(ch) || self.last_action != Some(LastAction::TypeWord) {
                self.push_undo_snapshot();
            }
            self.last_action = Some(LastAction::TypeWord);
        }

        let line = self.current_line().to_string();
        let before = slice16(&line, 0, self.state.cursor_col);
        let after = slice16_from(&line, self.state.cursor_col);
        let cl = self.state.cursor_line;
        self.state.lines[cl] = format!("{before}{ch}{after}");
        let col = self.state.cursor_col + len16(ch);
        self.set_cursor_col(col);
        self.emit_change();

        if self.autocomplete_state.is_none() {
            let text_before_cursor =
                slice16(self.current_line(), 0, self.state.cursor_col).to_string();
            if ch == "/" && self.is_at_start_of_message() {
                self.try_trigger_autocomplete(false);
            } else if ch == "@" || ch == "#" {
                let len = len16(&text_before_cursor);
                let char_before_symbol = if len >= 2 {
                    slice16(&text_before_cursor, len - 2, len - 1).to_string()
                } else {
                    String::new()
                };
                if len == 1 || char_before_symbol == " " || char_before_symbol == "\t" {
                    self.try_trigger_autocomplete(false);
                }
            } else if ch
                .chars()
                .any(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                && (self.is_in_slash_command_context(&text_before_cursor)
                    || in_symbol_context(&text_before_cursor))
            {
                self.try_trigger_autocomplete(false);
            }
        } else {
            self.update_autocomplete();
        }
    }

    fn handle_paste(&mut self, pasted_text: &str) {
        self.cancel_autocomplete();
        self.history_index = -1;
        self.last_action = None;
        self.push_undo_snapshot();

        // Control bytes re-encoded as CSI-u Ctrl+<letter> (`\x1b[<cp>;5u`) are
        // decoded back to their literal byte.
        let mut decoded = String::with_capacity(pasted_text.len());
        let mut rest = pasted_text;
        while let Some(pos) = rest.find("\x1b[") {
            decoded.push_str(&rest[..pos]);
            let after = &rest[pos + 2..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            let replacement = (digits > 0 && after[digits..].starts_with(";5u"))
                .then(|| after[..digits].parse::<u32>().ok())
                .flatten()
                .and_then(|cp| match cp {
                    97..=122 => char::from_u32(cp - 96),
                    65..=90 => char::from_u32(cp - 64),
                    _ => None,
                });
            match replacement {
                Some(c) => {
                    decoded.push(c);
                    rest = &after[digits + 3..];
                }
                None => {
                    decoded.push_str("\x1b[");
                    rest = after;
                }
            }
        }
        decoded.push_str(rest);

        let clean = Self::normalize_text(&decoded);
        let mut filtered: String = clean
            .chars()
            .filter(|&c| c == '\n' || c as u32 >= 32)
            .collect();

        // A pasted path right after a word character gets a separating space.
        if filtered.starts_with(['/', '~', '.']) {
            let line = self.current_line();
            let col = self.state.cursor_col;
            if col > 0 {
                let before = slice16(line, col - 1, col);
                if before
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    filtered = format!(" {filtered}");
                }
            }
        }

        let pasted_lines = filtered.split('\n').count();
        let total_chars = len16(&filtered);
        if pasted_lines > 10 || total_chars > 1000 {
            self.paste_counter += 1;
            let paste_id = self.paste_counter;
            self.pastes.insert(paste_id, filtered);
            let marker = if pasted_lines > 10 {
                format!("[paste #{paste_id} +{pasted_lines} lines]")
            } else {
                format!("[paste #{paste_id} {total_chars} chars]")
            };
            self.insert_text_at_cursor_internal(&marker);
            return;
        }
        self.insert_text_at_cursor_internal(&filtered);
    }

    fn add_new_line(&mut self) {
        self.cancel_autocomplete();
        self.history_index = -1;
        self.last_action = None;
        self.push_undo_snapshot();
        let line = self.current_line().to_string();
        let before = slice16(&line, 0, self.state.cursor_col).to_string();
        let after = slice16_from(&line, self.state.cursor_col).to_string();
        let cl = self.state.cursor_line;
        self.state.lines[cl] = before;
        self.state.lines.insert(cl + 1, after);
        self.state.cursor_line += 1;
        self.set_cursor_col(0);
        self.emit_change();
    }

    fn should_submit_on_backslash_enter(&self, data: &str, kb: &KeybindingsManager) -> bool {
        if self.disable_submit || !matches_key(data, "enter") {
            return false;
        }
        let submit_keys = kb.get_keys("tui.input.submit");
        let has_shift_enter = submit_keys
            .iter()
            .any(|k| k == "shift+enter" || k == "shift+return");
        if !has_shift_enter {
            return false;
        }
        unit_before_is(self.current_line(), self.state.cursor_col, '\\')
    }

    fn submit_value(&mut self) {
        self.cancel_autocomplete();
        let result = js_trim(&self.expand_paste_markers(&self.state.lines.join("\n"))).to_string();
        self.state = EditorState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
        };
        self.pastes.clear();
        self.paste_counter = 0;
        self.history_index = -1;
        self.scroll_offset = 0;
        self.undo_stack.clear();
        self.last_action = None;
        if let Some(cb) = &mut self.on_change {
            cb("");
        }
        if let Some(cb) = &mut self.on_submit {
            cb(&result);
        }
    }

    fn retrigger_autocomplete_after_delete(&mut self) {
        if self.autocomplete_state.is_some() {
            self.update_autocomplete();
            return;
        }
        let text_before_cursor = slice16(self.current_line(), 0, self.state.cursor_col).to_string();
        if self.is_in_slash_command_context(&text_before_cursor)
            || in_symbol_context(&text_before_cursor)
        {
            self.try_trigger_autocomplete(false);
        }
    }

    fn handle_backspace(&mut self) {
        self.history_index = -1;
        self.last_action = None;
        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let line = self.current_line().to_string();
            let before_cursor = slice16(&line, 0, self.state.cursor_col);
            let grapheme_len = self
                .segment(before_cursor)
                .last()
                .map_or(1, |g| len16(&g.segment));
            let new_col = self.state.cursor_col - grapheme_len;
            let cl = self.state.cursor_line;
            self.state.lines[cl] = format!(
                "{}{}",
                slice16(&line, 0, new_col),
                slice16_from(&line, self.state.cursor_col)
            );
            self.set_cursor_col(new_col);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            let cl = self.state.cursor_line;
            let current = self.state.lines.remove(cl);
            let previous_len = len16(&self.state.lines[cl - 1]);
            self.state.lines[cl - 1].push_str(&current);
            self.state.cursor_line -= 1;
            self.set_cursor_col(previous_len);
        }
        self.emit_change();
        self.retrigger_autocomplete_after_delete();
    }

    /// Set the cursor column and clear the sticky column.
    fn set_cursor_col(&mut self, col: usize) {
        self.state.cursor_col = col;
        self.preferred_visual_col = None;
        self.snapped_from_cursor_col = None;
    }

    fn move_to_visual_line(
        &mut self,
        visual_lines: &[VisualLine],
        current_visual_line: usize,
        target_visual_line: usize,
    ) {
        let (Some(&current_vl), Some(&target_vl)) = (
            visual_lines.get(current_visual_line),
            visual_lines.get(target_visual_line),
        ) else {
            return;
        };

        let current_visual_col = match self.snapped_from_cursor_col {
            Some(snapped) => {
                let vl_index =
                    self.find_visual_line_at(visual_lines, current_vl.logical_line, snapped);
                snapped.saturating_sub(visual_lines[vl_index].start_col)
            }
            None => self.state.cursor_col.saturating_sub(current_vl.start_col),
        };

        let is_last_source = current_visual_line + 1 == visual_lines.len()
            || visual_lines[current_visual_line + 1].logical_line != current_vl.logical_line;
        let source_max = if is_last_source {
            current_vl.length
        } else {
            current_vl.length.saturating_sub(1)
        };
        let is_last_target = target_visual_line + 1 == visual_lines.len()
            || visual_lines[target_visual_line + 1].logical_line != target_vl.logical_line;
        let target_max = if is_last_target {
            target_vl.length
        } else {
            target_vl.length.saturating_sub(1)
        };

        let move_to_visual_col =
            self.compute_vertical_move_column(current_visual_col, source_max, target_max);

        self.state.cursor_line = target_vl.logical_line;
        let target_col = target_vl.start_col + move_to_visual_col;
        let logical_line = self.line_at(target_vl.logical_line).to_string();
        self.state.cursor_col = target_col.min(len16(&logical_line));

        // Snap to an atomic segment's start so the cursor never lands inside.
        for seg in self.segment(&logical_line) {
            if seg.index > self.state.cursor_col {
                break;
            }
            let seg_len = len16(&seg.segment);
            if seg_len <= 1 {
                continue;
            }
            if self.state.cursor_col < seg.index + seg_len {
                let is_continuation = seg.index < target_vl.start_col;
                let is_moving_down = target_visual_line > current_visual_line;
                if is_continuation && is_moving_down {
                    // Already visited on the way down: land past it.
                    let seg_end = seg.index + seg_len;
                    let mut next = target_visual_line + 1;
                    while next < visual_lines.len()
                        && visual_lines[next].logical_line == target_vl.logical_line
                        && visual_lines[next].start_col < seg_end
                    {
                        next += 1;
                    }
                    if next < visual_lines.len() {
                        self.move_to_visual_line(visual_lines, current_visual_line, next);
                        return;
                    }
                }
                self.snapped_from_cursor_col = Some(self.state.cursor_col);
                self.state.cursor_col = seg.index;
                return;
            }
        }
        self.snapped_from_cursor_col = None;
    }

    /// The sticky-column decision table (`computeVerticalMoveColumn`).
    fn compute_vertical_move_column(
        &mut self,
        current_visual_col: usize,
        source_max: usize,
        target_max: usize,
    ) -> usize {
        let has_preferred = self.preferred_visual_col.is_some();
        let cursor_in_middle = current_visual_col < source_max;
        let target_too_short = target_max < current_visual_col;

        if !has_preferred || cursor_in_middle {
            if target_too_short {
                self.preferred_visual_col = Some(current_visual_col);
                return target_max;
            }
            self.preferred_visual_col = None;
            return current_visual_col;
        }
        let preferred = self.preferred_visual_col.unwrap();
        if target_too_short || target_max < preferred {
            return target_max;
        }
        self.preferred_visual_col = None;
        preferred
    }

    fn move_to_line_start(&mut self) {
        self.last_action = None;
        self.set_cursor_col(0);
    }

    fn move_to_line_end(&mut self) {
        self.last_action = None;
        let len = len16(self.current_line());
        self.set_cursor_col(len);
    }

    fn kill_push(&mut self, text: &str, prepend: bool, accumulate: bool) {
        self.kill_ring.push(
            text,
            KillPushOptions {
                prepend,
                accumulate,
            },
        );
        self.last_action = Some(LastAction::Kill);
    }

    fn merge_with_previous_line(&mut self) {
        let cl = self.state.cursor_line;
        let current = self.state.lines.remove(cl);
        let previous_len = len16(&self.state.lines[cl - 1]);
        self.state.lines[cl - 1].push_str(&current);
        self.state.cursor_line -= 1;
        self.set_cursor_col(previous_len);
    }

    fn merge_with_next_line(&mut self) {
        let cl = self.state.cursor_line;
        let next = self.state.lines.remove(cl + 1);
        self.state.lines[cl].push_str(&next);
    }

    fn delete_to_start_of_line(&mut self) {
        self.history_index = -1;
        let line = self.current_line().to_string();
        let was_kill = self.last_action == Some(LastAction::Kill);
        if self.state.cursor_col > 0 {
            self.push_undo_snapshot();
            let deleted = slice16(&line, 0, self.state.cursor_col).to_string();
            self.kill_push(&deleted, true, was_kill);
            let cl = self.state.cursor_line;
            self.state.lines[cl] = slice16_from(&line, self.state.cursor_col).to_string();
            self.set_cursor_col(0);
        } else if self.state.cursor_line > 0 {
            self.push_undo_snapshot();
            self.kill_push("\n", true, was_kill);
            self.merge_with_previous_line();
        }
        self.emit_change();
    }

    fn delete_to_end_of_line(&mut self) {
        self.history_index = -1;
        let line = self.current_line().to_string();
        let was_kill = self.last_action == Some(LastAction::Kill);
        if self.state.cursor_col < len16(&line) {
            self.push_undo_snapshot();
            let deleted = slice16_from(&line, self.state.cursor_col).to_string();
            self.kill_push(&deleted, false, was_kill);
            let cl = self.state.cursor_line;
            self.state.lines[cl] = slice16(&line, 0, self.state.cursor_col).to_string();
        } else if self.state.cursor_line + 1 < self.state.lines.len() {
            self.push_undo_snapshot();
            self.kill_push("\n", false, was_kill);
            self.merge_with_next_line();
        }
        self.emit_change();
    }

    fn delete_word_backwards(&mut self) {
        self.history_index = -1;
        let line = self.current_line().to_string();
        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.push_undo_snapshot();
                let was_kill = self.last_action == Some(LastAction::Kill);
                self.kill_push("\n", true, was_kill);
                self.merge_with_previous_line();
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);
            let old_col = self.state.cursor_col;
            self.move_word_backwards();
            let delete_from = self.state.cursor_col;
            self.set_cursor_col(old_col);
            let deleted = slice16(&line, delete_from, old_col).to_string();
            self.kill_push(&deleted, true, was_kill);
            let cl = self.state.cursor_line;
            self.state.lines[cl] = format!(
                "{}{}",
                slice16(&line, 0, delete_from),
                slice16_from(&line, old_col)
            );
            self.set_cursor_col(delete_from);
        }
        self.emit_change();
    }

    fn delete_word_forward(&mut self) {
        self.history_index = -1;
        let line = self.current_line().to_string();
        if self.state.cursor_col >= len16(&line) {
            if self.state.cursor_line + 1 < self.state.lines.len() {
                self.push_undo_snapshot();
                let was_kill = self.last_action == Some(LastAction::Kill);
                self.kill_push("\n", false, was_kill);
                self.merge_with_next_line();
            }
        } else {
            self.push_undo_snapshot();
            let was_kill = self.last_action == Some(LastAction::Kill);
            let old_col = self.state.cursor_col;
            self.move_word_forwards();
            let delete_to = self.state.cursor_col;
            self.set_cursor_col(old_col);
            let deleted = slice16(&line, old_col, delete_to).to_string();
            self.kill_push(&deleted, false, was_kill);
            let cl = self.state.cursor_line;
            self.state.lines[cl] = format!(
                "{}{}",
                slice16(&line, 0, old_col),
                slice16_from(&line, delete_to)
            );
        }
        self.emit_change();
    }

    fn handle_forward_delete(&mut self) {
        self.history_index = -1;
        self.last_action = None;
        let line = self.current_line().to_string();
        if self.state.cursor_col < len16(&line) {
            self.push_undo_snapshot();
            let after_cursor = slice16_from(&line, self.state.cursor_col);
            let grapheme_len = self
                .segment(after_cursor)
                .first()
                .map_or(1, |g| len16(&g.segment));
            let cl = self.state.cursor_line;
            self.state.lines[cl] = format!(
                "{}{}",
                slice16(&line, 0, self.state.cursor_col),
                slice16_from(&line, self.state.cursor_col + grapheme_len)
            );
        } else if self.state.cursor_line + 1 < self.state.lines.len() {
            self.push_undo_snapshot();
            self.merge_with_next_line();
        }
        self.emit_change();
        self.retrigger_autocomplete_after_delete();
    }

    /// Visual lines at `width`: `(logical line, start col, length)`.
    fn build_visual_line_map(&self, width: usize) -> Vec<VisualLine> {
        let mut visual_lines = Vec::new();
        for (i, line) in self.state.lines.iter().enumerate() {
            if line.is_empty() {
                visual_lines.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: 0,
                });
            } else if visible_width(line) <= width {
                visual_lines.push(VisualLine {
                    logical_line: i,
                    start_col: 0,
                    length: len16(line),
                });
            } else {
                for chunk in word_wrap_segments(line, width, Some(self.segment(line))) {
                    visual_lines.push(VisualLine {
                        logical_line: i,
                        start_col: chunk.start_index,
                        length: chunk.end_index - chunk.start_index,
                    });
                }
            }
        }
        visual_lines
    }

    fn find_visual_line_at(&self, visual_lines: &[VisualLine], line: usize, col: usize) -> usize {
        for (i, vl) in visual_lines.iter().enumerate() {
            if vl.logical_line != line {
                continue;
            }
            let is_last_segment =
                i + 1 == visual_lines.len() || visual_lines[i + 1].logical_line != vl.logical_line;
            if col >= vl.start_col {
                let offset = col - vl.start_col;
                if offset < vl.length || (is_last_segment && offset == vl.length) {
                    return i;
                }
            }
        }
        visual_lines.len().saturating_sub(1)
    }

    fn find_current_visual_line(&self, visual_lines: &[VisualLine]) -> usize {
        self.find_visual_line_at(visual_lines, self.state.cursor_line, self.state.cursor_col)
    }

    fn move_cursor(&mut self, delta_line: i64, delta_col: i64) {
        self.last_action = None;
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current_visual_line = self.find_current_visual_line(&visual_lines);

        if delta_line != 0 {
            let target = current_visual_line as i64 + delta_line;
            if target >= 0 && (target as usize) < visual_lines.len() {
                self.move_to_visual_line(&visual_lines, current_visual_line, target as usize);
            }
        }

        if delta_col != 0 {
            let line = self.current_line().to_string();
            if delta_col > 0 {
                if self.state.cursor_col < len16(&line) {
                    let after = slice16_from(&line, self.state.cursor_col);
                    let step = self.segment(after).first().map_or(1, |g| len16(&g.segment));
                    self.set_cursor_col(self.state.cursor_col + step);
                } else if self.state.cursor_line + 1 < self.state.lines.len() {
                    self.state.cursor_line += 1;
                    self.set_cursor_col(0);
                } else if let Some(vl) = visual_lines.get(current_visual_line) {
                    // At the very end: remember the column for up/down.
                    self.preferred_visual_col =
                        Some(self.state.cursor_col.saturating_sub(vl.start_col));
                }
            } else if self.state.cursor_col > 0 {
                let before = slice16(&line, 0, self.state.cursor_col);
                let step = self.segment(before).last().map_or(1, |g| len16(&g.segment));
                self.set_cursor_col(self.state.cursor_col - step);
            } else if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let len = len16(self.current_line());
                self.set_cursor_col(len);
            }
        }
    }

    fn page_scroll(&mut self, direction: i64) {
        self.last_action = None;
        let terminal_rows = (self.host.rows)() as f64;
        let page_size = ((terminal_rows * 0.3).floor() as i64).max(5);
        let visual_lines = self.build_visual_line_map(self.last_width);
        let current = self.find_current_visual_line(&visual_lines);
        let target =
            (current as i64 + direction * page_size).clamp(0, visual_lines.len() as i64 - 1);
        self.move_to_visual_line(&visual_lines, current, target as usize);
    }

    fn move_word_backwards(&mut self) {
        self.last_action = None;
        if self.state.cursor_col == 0 {
            if self.state.cursor_line > 0 {
                self.state.cursor_line -= 1;
                let len = len16(self.current_line());
                self.set_cursor_col(len);
            }
            return;
        }
        let text_before = slice16(self.current_line(), 0, self.state.cursor_col).to_string();
        let mut graphemes = self.segment(&text_before);
        let mut new_col = self.state.cursor_col;
        let last_is =
            |g: &Vec<Segment>, f: &dyn Fn(&str) -> bool| g.last().is_some_and(|s| f(&s.segment));

        // Skip trailing whitespace.
        while last_is(&graphemes, &|s| {
            !is_paste_marker(s) && is_whitespace_char(s)
        }) {
            new_col -= len16(&graphemes.pop().unwrap().segment);
        }
        if let Some(last) = graphemes.last().map(|s| s.segment.clone()) {
            if is_paste_marker(&last) {
                new_col -= len16(&graphemes.pop().unwrap().segment);
            } else if is_punctuation_char(&last) {
                while last_is(&graphemes, &|s| {
                    is_punctuation_char(s) && !is_paste_marker(s)
                }) {
                    new_col -= len16(&graphemes.pop().unwrap().segment);
                }
            } else {
                while last_is(&graphemes, &|s| {
                    !is_whitespace_char(s) && !is_punctuation_char(s) && !is_paste_marker(s)
                }) {
                    new_col -= len16(&graphemes.pop().unwrap().segment);
                }
            }
        }
        self.set_cursor_col(new_col);
    }

    fn move_word_forwards(&mut self) {
        self.last_action = None;
        let line = self.current_line().to_string();
        if self.state.cursor_col >= len16(&line) {
            if self.state.cursor_line + 1 < self.state.lines.len() {
                self.state.cursor_line += 1;
                self.set_cursor_col(0);
            }
            return;
        }
        let segments = self.segment(slice16_from(&line, self.state.cursor_col));
        let mut iter = segments.iter().peekable();
        let mut new_col = self.state.cursor_col;

        while let Some(s) =
            iter.next_if(|s| !is_paste_marker(&s.segment) && is_whitespace_char(&s.segment))
        {
            new_col += len16(&s.segment);
        }
        if let Some(first) = iter.peek().map(|s| s.segment.clone()) {
            if is_paste_marker(&first) {
                new_col += len16(&first);
            } else if is_punctuation_char(&first) {
                while let Some(s) = iter
                    .next_if(|s| is_punctuation_char(&s.segment) && !is_paste_marker(&s.segment))
                {
                    new_col += len16(&s.segment);
                }
            } else {
                while let Some(s) = iter.next_if(|s| {
                    !is_whitespace_char(&s.segment)
                        && !is_punctuation_char(&s.segment)
                        && !is_paste_marker(&s.segment)
                }) {
                    new_col += len16(&s.segment);
                }
            }
        }
        self.set_cursor_col(new_col);
    }

    fn yank(&mut self) {
        let Some(text) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };
        self.push_undo_snapshot();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn yank_pop(&mut self) {
        if self.last_action != Some(LastAction::Yank) || self.kill_ring.len() <= 1 {
            return;
        }
        self.push_undo_snapshot();
        self.delete_yanked_text();
        self.kill_ring.rotate();
        let text = self.kill_ring.peek().unwrap_or("").to_string();
        self.insert_yanked_text(&text);
        self.last_action = Some(LastAction::Yank);
    }

    fn insert_yanked_text(&mut self, text: &str) {
        self.history_index = -1;
        let lines: Vec<&str> = text.split('\n').collect();
        let current = self.current_line().to_string();
        let before = slice16(&current, 0, self.state.cursor_col).to_string();
        let after = slice16_from(&current, self.state.cursor_col).to_string();
        let cl = self.state.cursor_line;
        if lines.len() == 1 {
            self.state.lines[cl] = format!("{before}{text}{after}");
            let col = self.state.cursor_col + len16(text);
            self.set_cursor_col(col);
        } else {
            self.state.lines[cl] = format!("{before}{}", lines[0]);
            for (i, l) in lines[1..lines.len() - 1].iter().enumerate() {
                self.state.lines.insert(cl + 1 + i, l.to_string());
            }
            let last_index = cl + lines.len() - 1;
            let last = lines[lines.len() - 1];
            self.state
                .lines
                .insert(last_index, format!("{last}{after}"));
            self.state.cursor_line = last_index;
            self.set_cursor_col(len16(last));
        }
        self.emit_change();
    }

    fn delete_yanked_text(&mut self) {
        let Some(yanked) = self.kill_ring.peek().map(str::to_string) else {
            return;
        };
        if yanked.is_empty() {
            return;
        }
        let yank_lines: Vec<&str> = yanked.split('\n').collect();
        if yank_lines.len() == 1 {
            let line = self.current_line().to_string();
            let delete_len = len16(&yanked);
            let start = self.state.cursor_col.saturating_sub(delete_len);
            let cl = self.state.cursor_line;
            self.state.lines[cl] = format!(
                "{}{}",
                slice16(&line, 0, start),
                slice16_from(&line, self.state.cursor_col)
            );
            self.set_cursor_col(start);
        } else {
            let start_line = self.state.cursor_line + 1 - yank_lines.len();
            let start_col = len16(self.line_at(start_line)).saturating_sub(len16(yank_lines[0]));
            let after_cursor = slice16_from(self.current_line(), self.state.cursor_col).to_string();
            let before_yank = slice16(self.line_at(start_line), 0, start_col).to_string();
            self.state.lines.splice(
                start_line..start_line + yank_lines.len(),
                [format!("{before_yank}{after_cursor}")],
            );
            self.state.cursor_line = start_line;
            self.set_cursor_col(start_col);
        }
        self.emit_change();
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(&self.state);
    }

    fn undo(&mut self) {
        let snapshot = self.undo_stack.undo(&self.state);
        self.apply_snapshot(snapshot);
    }

    fn redo(&mut self) {
        let snapshot = self.undo_stack.redo(&self.state);
        self.apply_snapshot(snapshot);
    }

    fn apply_snapshot(&mut self, snapshot: Option<EditorState>) {
        let Some(snapshot) = snapshot else { return };
        self.history_index = -1;
        self.state = snapshot;
        self.last_action = None;
        self.preferred_visual_col = None;
        self.emit_change();
    }

    /// Jump to the next/previous occurrence of `ch` (multi-line,
    /// case-sensitive, skipping the cursor position).
    fn jump_to_char(&mut self, ch: &str, direction: JumpDirection) {
        self.last_action = None;
        let forward = direction == JumpDirection::Forward;
        let needle: Vec<u16> = ch.encode_utf16().collect();
        if needle.is_empty() {
            return;
        }
        let count = self.state.lines.len() as i64;
        let mut line_idx = self.state.cursor_line as i64;
        while line_idx >= 0 && line_idx < count {
            let line: Vec<u16> = self.state.lines[line_idx as usize].encode_utf16().collect();
            let is_current = line_idx as usize == self.state.cursor_line;
            let found = if forward {
                let from = if is_current {
                    self.state.cursor_col + 1
                } else {
                    0
                };
                (from..=line.len().saturating_sub(needle.len())).find(|&i| {
                    line.len() >= needle.len() && line[i..i + needle.len()] == needle[..]
                })
            } else {
                // lastIndexOf(char, cursorCol - 1): a negative start searches
                // index 0 only.
                let from = if is_current {
                    (self.state.cursor_col as i64 - 1).max(0) as usize
                } else {
                    line.len()
                };
                let from = from.min(line.len().saturating_sub(needle.len()));
                (0..=from).rev().find(|&i| {
                    line.len() >= needle.len() && line[i..i + needle.len()] == needle[..]
                })
            };
            if let Some(idx) = found {
                self.state.cursor_line = line_idx as usize;
                self.set_cursor_col(idx);
                return;
            }
            line_idx += if forward { 1 } else { -1 };
        }
    }

    /// Slash menu only on the first line.
    fn is_slash_menu_allowed(&self) -> bool {
        self.state.cursor_line == 0
    }

    fn is_at_start_of_message(&self) -> bool {
        if !self.is_slash_menu_allowed() {
            return false;
        }
        let before = js_trim(slice16(self.current_line(), 0, self.state.cursor_col));
        before.is_empty() || before == "/"
    }

    fn is_in_slash_command_context(&self, text_before_cursor: &str) -> bool {
        self.is_slash_menu_allowed()
            && text_before_cursor
                .trim_start_matches(is_js_space)
                .starts_with('/')
    }

    /// Exact match wins, then the first prefix match; case-sensitive, on
    /// `value` only.
    fn get_best_autocomplete_match_index(
        items: &[AutocompleteItem],
        prefix: &str,
    ) -> Option<usize> {
        if prefix.is_empty() {
            return None;
        }
        let mut first_prefix = None;
        for (i, item) in items.iter().enumerate() {
            if item.value == prefix {
                return Some(i);
            }
            if first_prefix.is_none() && item.value.starts_with(prefix) {
                first_prefix = Some(i);
            }
        }
        first_prefix
    }

    fn create_autocomplete_list(&self, prefix: &str, items: &[AutocompleteItem]) -> SelectList {
        let layout = if prefix.starts_with('/') {
            slash_command_layout()
        } else {
            SelectListLayoutOptions::default()
        };
        let items = items
            .iter()
            .map(|i| SelectItem {
                value: i.value.clone(),
                label: i.label.clone(),
                description: i.description.clone(),
            })
            .collect();
        SelectList::new(
            items,
            self.autocomplete_max_visible,
            (self.theme.select_list)(),
            layout,
        )
    }

    fn try_trigger_autocomplete(&mut self, explicit_tab: bool) {
        self.request_autocomplete(false, explicit_tab);
    }

    fn handle_tab_completion(&mut self) {
        if self.autocomplete_provider.is_none() {
            return;
        }
        let before = slice16(self.current_line(), 0, self.state.cursor_col).to_string();
        if self.is_in_slash_command_context(&before)
            && !before.trim_start_matches(is_js_space).contains(' ')
        {
            self.request_autocomplete(false, true);
        } else {
            self.request_autocomplete(true, true);
        }
    }

    fn provider_col(&self) -> usize {
        index16_to_chars(self.current_line(), self.state.cursor_col)
    }

    fn request_autocomplete(&mut self, force: bool, explicit_tab: bool) {
        let Some(provider) = &self.autocomplete_provider else {
            return;
        };
        let col = self.provider_col();
        if force
            && !provider.should_trigger_file_completion(
                &self.state.lines,
                self.state.cursor_line,
                col,
            )
        {
            return;
        }
        self.pending_autocomplete = None;
        let debounced = !explicit_tab
            && !force
            && is_symbol_autocomplete_context(slice16(
                self.current_line(),
                0,
                self.state.cursor_col,
            ));
        if debounced {
            self.pending_autocomplete = Some((
                Instant::now() + ATTACHMENT_AUTOCOMPLETE_DEBOUNCE,
                force,
                explicit_tab,
            ));
            return;
        }
        self.run_autocomplete_request(force, explicit_tab);
    }

    /// When a debounced autocomplete request is due, if one is waiting.
    pub fn autocomplete_deadline(&self) -> Option<Instant> {
        self.pending_autocomplete.map(|(deadline, ..)| deadline)
    }

    /// Run a debounced autocomplete request whose deadline has passed.
    /// Returns whether one ran (and a render is owed).
    pub fn poll_autocomplete(&mut self) -> bool {
        match self.pending_autocomplete {
            Some((deadline, force, explicit_tab)) if Instant::now() >= deadline => {
                self.pending_autocomplete = None;
                self.run_autocomplete_request(force, explicit_tab);
                true
            }
            _ => false,
        }
    }

    fn run_autocomplete_request(&mut self, force: bool, explicit_tab: bool) {
        let Some(provider) = &self.autocomplete_provider else {
            return;
        };
        let col = self.provider_col();
        let suggestions =
            provider.get_suggestions(&self.state.lines, self.state.cursor_line, col, force);
        let Some(suggestions) = suggestions.filter(|s| !s.items.is_empty()) else {
            self.cancel_autocomplete();
            self.request_render();
            return;
        };
        if force && explicit_tab && suggestions.items.len() == 1 {
            let item = suggestions.items[0].clone();
            self.push_undo_snapshot();
            self.last_action = None;
            self.apply_completion(&item, &suggestions.prefix);
            self.emit_change();
            self.request_render();
            return;
        }
        self.apply_autocomplete_suggestions(
            suggestions,
            if force {
                AutocompleteState::Force
            } else {
                AutocompleteState::Regular
            },
        );
        self.request_render();
    }

    /// Run the provider's `applyCompletion` and adopt its result.
    fn apply_completion(&mut self, item: &AutocompleteItem, prefix: &str) {
        let Some(provider) = &self.autocomplete_provider else {
            return;
        };
        let col = self.provider_col();
        let result =
            provider.apply_completion(&self.state.lines, self.state.cursor_line, col, item, prefix);
        self.state.lines = result.lines;
        self.state.cursor_line = result.cursor_line;
        let col16 = chars_to_16(self.current_line(), result.cursor_col);
        self.set_cursor_col(col16);
    }

    fn apply_autocomplete_suggestions(
        &mut self,
        suggestions: AutocompleteSuggestions,
        state: AutocompleteState,
    ) {
        self.autocomplete_prefix = suggestions.prefix.clone();
        let mut list = self.create_autocomplete_list(&suggestions.prefix, &suggestions.items);
        if let Some(best) =
            Self::get_best_autocomplete_match_index(&suggestions.items, &suggestions.prefix)
        {
            list.set_selected_index(best);
        }
        self.autocomplete_list = Some(list);
        self.set_autocomplete_state(Some(state));
    }

    fn clear_autocomplete_ui(&mut self) {
        self.set_autocomplete_state(None);
        self.autocomplete_list = None;
        self.autocomplete_prefix.clear();
    }

    /// The one place the list's presence changes, so it announces it.
    fn set_autocomplete_state(&mut self, state: Option<AutocompleteState>) {
        let was = self.autocomplete_state.is_some();
        self.autocomplete_state = state;
        let now = state.is_some();
        if was != now {
            if let Some(cb) = &mut self.on_autocomplete_visibility_change {
                cb(now);
            }
        }
    }

    fn cancel_autocomplete(&mut self) {
        self.pending_autocomplete = None;
        self.clear_autocomplete_ui();
    }

    pub fn is_showing_autocomplete(&self) -> bool {
        self.autocomplete_state.is_some()
    }

    fn update_autocomplete(&mut self) {
        let Some(state) = self.autocomplete_state else {
            return;
        };
        if self.autocomplete_provider.is_none() {
            return;
        }
        self.request_autocomplete(state == AutocompleteState::Force, false);
    }

    fn selected_autocomplete_item(&self) -> Option<AutocompleteItem> {
        let item = self.autocomplete_list.as_ref()?.get_selected_item()?;
        Some(AutocompleteItem {
            value: item.value.clone(),
            label: item.label.clone(),
            description: item.description.clone(),
        })
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// `handleInput`, with the keybindings passed explicitly.
    pub fn handle_input_with(&mut self, data: &str, kb: &KeybindingsManager) {
        let mut data = data.to_string();

        // Character jump mode (awaiting the character to jump to).
        if let Some(direction) = self.jump_mode {
            if kb.matches(&data, "tui.editor.jumpForward")
                || kb.matches(&data, "tui.editor.jumpBackward")
            {
                self.jump_mode = None;
                return;
            }
            let printable = decode_printable_key(&data)
                .map(|c| c.to_string())
                .or_else(|| {
                    (data.encode_utf16().next().is_some_and(|u| u >= 32)).then(|| data.clone())
                });
            if let Some(printable) = printable {
                self.jump_mode = None;
                self.jump_to_char(&printable, direction);
                return;
            }
            self.jump_mode = None;
        }

        // Bracketed paste.
        if data.contains("\x1b[200~") {
            self.is_in_paste = true;
            self.paste_buffer.clear();
            data = data.replacen("\x1b[200~", "", 1);
        }
        if self.is_in_paste {
            self.paste_buffer.push_str(&data);
            if let Some(end) = self.paste_buffer.find("\x1b[201~") {
                let content = self.paste_buffer[..end].to_string();
                if !content.is_empty() {
                    self.handle_paste(&content);
                }
                self.is_in_paste = false;
                let remaining = self.paste_buffer[end + 6..].to_string();
                self.paste_buffer.clear();
                if !remaining.is_empty() {
                    self.handle_input_with(&remaining, kb);
                }
            }
            return;
        }

        // Ctrl+C: the parent handles it.
        if kb.matches(&data, "tui.input.copy") {
            return;
        }
        if kb.matches(&data, "tui.editor.undo") {
            self.undo();
            return;
        }
        if kb.matches(&data, "tui.editor.redo") {
            self.redo();
            return;
        }

        // Autocomplete mode.
        if self.autocomplete_state.is_some() && self.autocomplete_list.is_some() {
            if kb.matches(&data, "tui.select.cancel") {
                self.cancel_autocomplete();
                return;
            }
            if kb.matches(&data, "tui.select.up") || kb.matches(&data, "tui.select.down") {
                if let Some(list) = &mut self.autocomplete_list {
                    list.handle_input_with(&data, kb);
                }
                return;
            }
            if kb.matches(&data, "tui.input.tab") {
                if let Some(selected) = self.selected_autocomplete_item() {
                    if self.autocomplete_provider.is_some() {
                        self.push_undo_snapshot();
                        self.last_action = None;
                        let prefix = self.autocomplete_prefix.clone();
                        self.apply_completion(&selected, &prefix);
                        self.cancel_autocomplete();
                        self.emit_change();
                    }
                }
                return;
            }
            if kb.matches(&data, "tui.select.confirm") {
                if let Some(selected) = self.selected_autocomplete_item() {
                    if self.autocomplete_provider.is_some() {
                        self.push_undo_snapshot();
                        self.last_action = None;
                        let prefix = self.autocomplete_prefix.clone();
                        self.apply_completion(&selected, &prefix);
                        if prefix.starts_with('/') {
                            self.cancel_autocomplete();
                            // Fall through to submit.
                        } else {
                            self.cancel_autocomplete();
                            self.emit_change();
                            return;
                        }
                    }
                }
            }
        }

        if kb.matches(&data, "tui.input.tab") && self.autocomplete_state.is_none() {
            self.handle_tab_completion();
            return;
        }

        if kb.matches(&data, "tui.editor.deleteToLineEnd") {
            self.delete_to_end_of_line();
            return;
        }
        if kb.matches(&data, "tui.editor.deleteToLineStart") {
            self.delete_to_start_of_line();
            return;
        }
        if kb.matches(&data, "tui.editor.deleteWordBackward") {
            self.delete_word_backwards();
            return;
        }
        if kb.matches(&data, "tui.editor.deleteWordForward") {
            self.delete_word_forward();
            return;
        }
        if kb.matches(&data, "tui.editor.deleteCharBackward")
            || matches_key(&data, "shift+backspace")
        {
            self.handle_backspace();
            return;
        }
        if kb.matches(&data, "tui.editor.deleteCharForward") || matches_key(&data, "shift+delete") {
            self.handle_forward_delete();
            return;
        }
        if kb.matches(&data, "tui.editor.yank") {
            self.yank();
            return;
        }
        if kb.matches(&data, "tui.editor.yankPop") {
            self.yank_pop();
            return;
        }
        if kb.matches(&data, "tui.editor.cursorLineStart") {
            self.move_to_line_start();
            return;
        }
        if kb.matches(&data, "tui.editor.cursorLineEnd") {
            self.move_to_line_end();
            return;
        }
        if kb.matches(&data, "tui.editor.cursorWordLeft") {
            self.move_word_backwards();
            return;
        }
        if kb.matches(&data, "tui.editor.cursorWordRight") {
            self.move_word_forwards();
            return;
        }

        // New line.
        let len = len16(&data);
        if kb.matches(&data, "tui.input.newLine")
            || (data.starts_with('\n') && len > 1)
            || data == "\x1b\r"
            || data == "\x1b[13;2~"
            || (len > 1 && data.contains('\x1b') && data.contains('\r'))
            || data == "\n"
        {
            if self.should_submit_on_backslash_enter(&data, kb) {
                self.handle_backspace();
                self.submit_value();
                return;
            }
            self.add_new_line();
            return;
        }

        // Submit.
        if kb.matches(&data, "tui.input.submit") {
            if self.disable_submit {
                return;
            }
            // Terminals without Shift+Enter: a trailing `\` makes a newline.
            if unit_before_is(self.current_line(), self.state.cursor_col, '\\') {
                self.handle_backspace();
                self.add_new_line();
                return;
            }
            self.submit_value();
            return;
        }

        // Arrow keys (with history).
        if kb.matches(&data, "tui.editor.cursorUp") {
            if self.is_editor_empty() || (self.history_index > -1 && self.is_on_first_visual_line())
            {
                self.navigate_history(-1);
            } else if self.is_on_first_visual_line() {
                self.move_to_line_start();
            } else {
                self.move_cursor(-1, 0);
            }
            return;
        }
        if kb.matches(&data, "tui.editor.cursorDown") {
            if self.history_index > -1 && self.is_on_last_visual_line() {
                self.navigate_history(1);
            } else if self.is_on_last_visual_line() {
                self.move_to_line_end();
            } else {
                self.move_cursor(1, 0);
            }
            return;
        }
        if kb.matches(&data, "tui.editor.cursorRight") {
            self.move_cursor(0, 1);
            return;
        }
        if kb.matches(&data, "tui.editor.cursorLeft") {
            self.move_cursor(0, -1);
            return;
        }
        if kb.matches(&data, "tui.editor.pageUp") {
            self.page_scroll(-1);
            return;
        }
        if kb.matches(&data, "tui.editor.pageDown") {
            self.page_scroll(1);
            return;
        }
        if kb.matches(&data, "tui.editor.jumpForward") {
            self.jump_mode = Some(JumpDirection::Forward);
            return;
        }
        if kb.matches(&data, "tui.editor.jumpBackward") {
            self.jump_mode = Some(JumpDirection::Backward);
            return;
        }
        if matches_key(&data, "shift+space") {
            self.insert_character(" ", false);
            return;
        }
        if let Some(printable) = decode_printable_key(&data) {
            self.insert_character(&printable.to_string(), false);
            return;
        }
        if data.encode_utf16().next().is_some_and(|u| u >= 32) {
            self.insert_character(&data, false);
        }
    }
}

impl Component for Editor {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        // Box mode needs two border columns plus one of content.
        let is_box = self.border == FrameBorderStyle::Box && width >= 4;
        // `none`: nested in a frame that already rules it.
        let bare = self.border == FrameBorderStyle::None;
        let border_width = usize::from(is_box);
        let inner_width = width.saturating_sub(border_width * 2).max(1);
        let max_padding = (inner_width - 1) / 2;
        let padding_x = self.padding_x.min(max_padding);
        let content_width = inner_width.saturating_sub(padding_x * 2).max(1);

        let prompt_prefix_width = if self.prompt_prefix.is_empty() {
            0
        } else {
            visible_width(&format!("{} ", self.prompt_prefix))
        };
        // Reserve a cursor column without padding, or always in box mode.
        let cursor_column = usize::from(is_box || padding_x == 0);
        let layout_width = content_width
            .saturating_sub(cursor_column)
            .saturating_sub(prompt_prefix_width)
            .max(1);
        self.last_width = layout_width;

        let bar_width = if is_box { width - 2 } else { width };
        let vertical = if is_box {
            (self.border_color)(&self.border_chars.vertical)
        } else {
            String::new()
        };

        let layout_lines = self.layout_text(layout_width);

        // At most 30% of the terminal, minimum 5 lines.
        let terminal_rows = (self.host.rows)() as f64;
        let max_visible_lines = ((terminal_rows * 0.3).floor() as usize).max(5);

        let cursor_line_index = layout_lines.iter().position(|l| l.has_cursor).unwrap_or(0);
        if cursor_line_index < self.scroll_offset {
            self.scroll_offset = cursor_line_index;
        } else if cursor_line_index >= self.scroll_offset + max_visible_lines {
            self.scroll_offset = cursor_line_index + 1 - max_visible_lines;
        }
        let max_scroll_offset = layout_lines.len().saturating_sub(max_visible_lines);
        self.scroll_offset = self.scroll_offset.min(max_scroll_offset);

        let visible_end = (self.scroll_offset + max_visible_lines).min(layout_lines.len());
        let visible_lines = &layout_lines[self.scroll_offset..visible_end];

        let mut result = Vec::new();
        let left_padding = " ".repeat(padding_x);
        let right_padding = left_padding.clone();

        if !bare {
            result.push(self.render_border(FrameEdge::Top, self.scroll_offset, bar_width, is_box));
        }

        // Hardware cursor marker only when focused and not autocompleting.
        let emit_cursor_marker = self.focused && self.autocomplete_state.is_none();

        for (visible_line_index, layout_line) in visible_lines.iter().enumerate() {
            let mut display_text = layout_line.text.clone();
            let mut line_visible_width = visible_width(&layout_line.text);
            let mut cursor_in_padding = false;

            if let (true, Some(cursor_pos)) = (layout_line.has_cursor, layout_line.cursor_pos) {
                let before = slice16(&display_text, 0, cursor_pos).to_string();
                let after = slice16_from(&display_text, cursor_pos).to_string();
                let marker = if emit_cursor_marker {
                    CURSOR_MARKER
                } else {
                    ""
                };
                if !after.is_empty() {
                    let first = self
                        .segment(&after)
                        .first()
                        .map(|s| s.segment.clone())
                        .unwrap_or_default();
                    let rest_after = &after[first.len()..];
                    display_text = format!("{before}{marker}\x1b[7m{first}\x1b[0m{rest_after}");
                } else {
                    display_text = format!("{before}{marker}\x1b[7m \x1b[0m");
                    line_visible_width += 1;
                    if !is_box && line_visible_width > content_width && padding_x > 0 {
                        cursor_in_padding = true;
                    }
                }
            }

            if visible_line_index == 0 && !self.prompt_prefix.is_empty() {
                let colored = (self.prompt_color)(&format!("{} ", self.prompt_prefix));
                display_text = format!("{colored}{display_text}");
                line_visible_width += prompt_prefix_width;
            }

            let padding = " ".repeat(content_width.saturating_sub(line_visible_width));
            let line_right_padding = if cursor_in_padding {
                &right_padding[1..]
            } else {
                right_padding.as_str()
            };
            result.push(format!(
                "{vertical}{left_padding}{display_text}{padding}{line_right_padding}{vertical}"
            ));
        }

        let lines_below = layout_lines
            .len()
            .saturating_sub(self.scroll_offset + visible_lines.len());
        if !bare {
            result.push(self.render_border(FrameEdge::Bottom, lines_below, bar_width, is_box));
        }

        if self.autocomplete_state.is_some() {
            if let Some(list) = &mut self.autocomplete_list {
                // Aligned to the inner text edge, not the outer box edge.
                let ac_padding = " ".repeat(border_width + padding_x);
                for line in list.render(content_width as u16) {
                    let line_padding =
                        " ".repeat(content_width.saturating_sub(visible_width(&line)));
                    result.push(format!("{ac_padding}{line}{line_padding}{ac_padding}"));
                }
            }
        }
        result
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        self.handle_input_with(data, &kb);
    }

    fn invalidate(&mut self) {}

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        Editor::set_focused(self, focused);
    }
}

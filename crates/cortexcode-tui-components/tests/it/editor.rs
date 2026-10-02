//! Port of the pin's `test/editor.test.ts`.
#![allow(non_snake_case, clippy::needless_range_loop)] // names and loops mirror the TS source

use cortexcode_tui_components::*;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};
use std::cell::RefCell;
use std::rc::Rc;

fn dim(t: &str) -> String {
    format!("\x1b[2m{t}\x1b[22m")
}

pub fn select_theme() -> SelectListTheme {
    SelectListTheme {
        selected_prefix: Box::new(|t: &str| format!("\x1b[34m{t}\x1b[39m")),
        selected_text: Box::new(|t: &str| format!("\x1b[1m{t}\x1b[22m")),
        description: Box::new(|t: &str| dim(t)),
        scroll_info: Box::new(|t: &str| dim(t)),
        no_match: Box::new(|t: &str| dim(t)),
        cursor: None,
        selected_row: None,
    }
}

pub fn theme() -> EditorTheme {
    EditorTheme {
        border_color: Box::new(|t: &str| dim(t)),
        border_chars: None,
        select_list: Box::new(select_theme),
    }
}

pub fn ed_with(
    _cols: usize,
    rows: u16,
    padding_x: usize,
    border: Option<EditorBorderStyle>,
) -> Editor {
    Editor::new(
        EditorHost {
            rows: Box::new(move || rows),
            request_render: Box::new(|| {}),
        },
        theme(),
        EditorOptions {
            padding_x: Some(padding_x),
            autocomplete_max_visible: None,
            border,
        },
    )
}

pub fn ed() -> Editor {
    ed_with(80, 24, 0, None)
}

pub fn render(editor: &mut Editor, width: usize) -> Vec<String> {
    editor.render(width as u16)
}

pub fn strip(s: &str) -> String {
    strip_vt_control_characters(s)
}

pub fn dbg(s: &str) -> String {
    format!("{s:?}")
}

/// Record every submitted value.
pub fn capture_submit(editor: &mut Editor) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    editor.on_submit = Some(Box::new(move |t: &str| {
        sink.borrow_mut().push(t.to_string())
    }));
    seen
}

/// Record every onChange value. Unused so far (no ported test drives onChange);
/// kept next to capture_submit for those cases.
#[allow(dead_code)]
pub fn capture_change(editor: &mut Editor) -> Rc<RefCell<Vec<String>>> {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    editor.on_change = Some(Box::new(move |t: &str| {
        sink.borrow_mut().push(t.to_string())
    }));
    seen
}

/// `applyCompletion` from the TS test: replaces the prefix with the value
/// (columns are chars here, as the Rust provider contract uses).
pub fn apply_completion(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    item: &AutocompleteItem,
    prefix: &str,
) -> ApplyCompletionResult {
    let line: Vec<char> = lines
        .get(cursor_line)
        .cloned()
        .unwrap_or_default()
        .chars()
        .collect();
    let p = prefix.chars().count();
    let before: String = line[..cursor_col - p].iter().collect();
    let after: String = line[cursor_col..].iter().collect();
    let mut new_lines = lines.to_vec();
    new_lines[cursor_line] = format!("{before}{}{after}", item.value);
    ApplyCompletionResult {
        lines: new_lines,
        cursor_line,
        cursor_col: cursor_col - p + item.value.chars().count(),
    }
}

pub type SuggestFn = Box<dyn Fn(&[String], usize, usize, bool) -> Option<AutocompleteSuggestions>>;

/// A mock provider built from a closure (the TS tests' object literals).
pub struct FnProvider {
    pub suggest: SuggestFn,
    pub calls: Rc<RefCell<usize>>,
}

impl AutocompleteProvider for FnProvider {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        *self.calls.borrow_mut() += 1;
        (self.suggest)(lines, cursor_line, cursor_col, force)
    }
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> ApplyCompletionResult {
        apply_completion(lines, cursor_line, cursor_col, item, prefix)
    }
}

pub fn item(value: &str, label: &str) -> AutocompleteItem {
    AutocompleteItem {
        value: value.into(),
        label: label.into(),
        description: None,
    }
}

pub fn suggestions(items: Vec<AutocompleteItem>, prefix: &str) -> Option<AutocompleteSuggestions> {
    Some(AutocompleteSuggestions {
        items,
        prefix: prefix.into(),
    })
}

/// Install a closure provider; returns its call counter.
pub fn provide(
    editor: &mut Editor,
    suggest: impl Fn(&[String], usize, usize, bool) -> Option<AutocompleteSuggestions> + 'static,
) -> Rc<RefCell<usize>> {
    let calls = Rc::new(RefCell::new(0));
    editor.set_autocomplete_provider(Box::new(FnProvider {
        suggest: Box::new(suggest),
        calls: calls.clone(),
    }));
    calls
}

/// `text.slice(0, cursorCol)` over chars.
pub fn before(lines: &[String], col: usize) -> String {
    lines
        .first()
        .map(|l| l.chars().take(col).collect())
        .unwrap_or_default()
}

pub fn type_str(editor: &mut Editor, text: &str) {
    for c in text.chars() {
        editor.handle_input(&c.to_string());
    }
}

/// Let a debounced autocomplete request run.
pub fn flush_debounce(editor: &mut Editor) {
    std::thread::sleep(std::time::Duration::from_millis(50));
    editor.poll_autocomplete();
}

/// `/argtest <prefix>` / `/model <prefix>` argument context.
pub fn arg_context(text: &str, command: &str) -> Option<String> {
    let rest = text.strip_prefix(command)?;
    let arg = rest.trim_start();
    (rest.len() > arg.len() && !arg.is_empty() && !arg.contains(char::is_whitespace))
        .then(|| arg.to_string())
}

mod editor_component {
    use super::*;
    mod prompt_history_navigation {
        use super::*;
        #[test]
        fn does_nothing_on_up_arrow_when_history_is_empty() {
            let mut editor = ed();

            editor.handle_input("\x1b[A"); // Up arrow

            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn shows_most_recent_history_entry_on_up_arrow_when_editor_is_empty() {
            let mut editor = ed();

            editor.add_to_history("first prompt");
            editor.add_to_history("second prompt");

            editor.handle_input("\x1b[A"); // Up arrow

            assert_eq!(editor.get_text(), "second prompt");
        }

        #[test]
        fn cycles_through_history_entries_on_repeated_up_arrow() {
            let mut editor = ed();

            editor.add_to_history("first");
            editor.add_to_history("second");
            editor.add_to_history("third");

            editor.handle_input("\x1b[A"); // Up - shows "third"
            assert_eq!(editor.get_text(), "third");

            editor.handle_input("\x1b[A"); // Up - shows "second"
            assert_eq!(editor.get_text(), "second");

            editor.handle_input("\x1b[A"); // Up - shows "first"
            assert_eq!(editor.get_text(), "first");

            editor.handle_input("\x1b[A"); // Up - stays at "first" (oldest)
            assert_eq!(editor.get_text(), "first");
        }

        #[test]
        fn returns_to_empty_editor_on_down_arrow_after_browsing_history() {
            let mut editor = ed();

            editor.add_to_history("prompt");

            editor.handle_input("\x1b[A"); // Up - shows "prompt"
            assert_eq!(editor.get_text(), "prompt");

            editor.handle_input("\x1b[B"); // Down - clears editor
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn navigates_forward_through_history_with_down_arrow() {
            let mut editor = ed();

            editor.add_to_history("first");
            editor.add_to_history("second");
            editor.add_to_history("third");

            // Go to oldest
            editor.handle_input("\x1b[A"); // third
            editor.handle_input("\x1b[A"); // second
            editor.handle_input("\x1b[A"); // first

            // Navigate back
            editor.handle_input("\x1b[B"); // second
            assert_eq!(editor.get_text(), "second");

            editor.handle_input("\x1b[B"); // third
            assert_eq!(editor.get_text(), "third");

            editor.handle_input("\x1b[B"); // empty
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn exits_history_mode_when_typing_a_character() {
            let mut editor = ed();

            editor.add_to_history("old prompt");

            editor.handle_input("\x1b[A"); // Up - shows "old prompt"
            editor.handle_input("x"); // Type a character - exits history mode

            assert_eq!(editor.get_text(), "old promptx");
        }

        #[test]
        fn exits_history_mode_on_settext() {
            let mut editor = ed();

            editor.add_to_history("first");
            editor.add_to_history("second");

            editor.handle_input("\x1b[A"); // Up - shows "second"
            editor.set_text(""); // External clear

            // Up should start fresh from most recent
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_text(), "second");
        }

        #[test]
        fn does_not_add_empty_strings_to_history() {
            let mut editor = ed();

            editor.add_to_history("");
            editor.add_to_history("   ");
            editor.add_to_history("valid");

            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_text(), "valid");

            // Should not have more entries
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_text(), "valid");
        }

        #[test]
        fn does_not_add_consecutive_duplicates_to_history() {
            let mut editor = ed();

            editor.add_to_history("same");
            editor.add_to_history("same");
            editor.add_to_history("same");

            editor.handle_input("\x1b[A"); // "same"
            assert_eq!(editor.get_text(), "same");

            editor.handle_input("\x1b[A"); // stays at "same" (only one entry)
            assert_eq!(editor.get_text(), "same");
        }

        #[test]
        fn allows_non_consecutive_duplicates_in_history() {
            let mut editor = ed();

            editor.add_to_history("first");
            editor.add_to_history("second");
            editor.add_to_history("first"); // Not consecutive, should be added

            editor.handle_input("\x1b[A"); // "first"
            assert_eq!(editor.get_text(), "first");

            editor.handle_input("\x1b[A"); // "second"
            assert_eq!(editor.get_text(), "second");

            editor.handle_input("\x1b[A"); // "first" (older one)
            assert_eq!(editor.get_text(), "first");
        }

        #[test]
        fn uses_cursor_movement_instead_of_history_when_editor_has_content() {
            let mut editor = ed();

            editor.add_to_history("history item");
            editor.set_text("line1\nline2");

            // Cursor is at end of line2, Up should move to line1
            editor.handle_input("\x1b[A"); // Up - cursor movement

            // Insert character to verify cursor position
            editor.handle_input("X");

            // X should be inserted in line1, not replace with history
            assert_eq!(editor.get_text(), "line1X\nline2");
        }

        #[test]
        fn limits_history_to_100_entries() {
            let mut editor = ed();

            // Add 105 entries
            for i in 0..105 {
                editor.add_to_history(&format!("prompt {i}"));
            }

            // Navigate to oldest
            for _i in 0..100 {
                editor.handle_input("\x1b[A");
            }

            // Should be at entry 5 (oldest kept), not entry 0
            assert_eq!(editor.get_text(), "prompt 5");

            // One more Up should not change anything
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_text(), "prompt 5");
        }

        #[test]
        fn allows_cursor_movement_within_multi_line_history_entry_with_down() {
            let mut editor = ed();

            editor.add_to_history("line1\nline2\nline3");

            // Browse to the multi-line entry
            editor.handle_input("\x1b[A"); // Up - shows entry, cursor at end of line3
            assert_eq!(editor.get_text(), "line1\nline2\nline3");

            // Down should exit history since cursor is on last line
            editor.handle_input("\x1b[B"); // Down
            assert_eq!(editor.get_text(), ""); // Exited to empty
        }

        #[test]
        fn allows_cursor_movement_within_multi_line_history_entry_with_up() {
            let mut editor = ed();

            editor.add_to_history("older entry");
            editor.add_to_history("line1\nline2\nline3");

            // Browse to the multi-line entry
            editor.handle_input("\x1b[A"); // Up - shows multi-line, cursor at end of line3

            // Up should move cursor within the entry (not on first line yet)
            editor.handle_input("\x1b[A"); // Up - cursor moves to line2
            assert_eq!(editor.get_text(), "line1\nline2\nline3"); // Still same entry

            editor.handle_input("\x1b[A"); // Up - cursor moves to line1 (now on first visual line)
            assert_eq!(editor.get_text(), "line1\nline2\nline3"); // Still same entry

            // Now Up should navigate to older history entry
            editor.handle_input("\x1b[A"); // Up - navigate to older
            assert_eq!(editor.get_text(), "older entry");
        }

        #[test]
        fn navigates_from_multi_line_entry_back_to_newer_via_down_after_cursor_movement() {
            let mut editor = ed();

            editor.add_to_history("line1\nline2\nline3");

            // Browse to entry and move cursor up
            editor.handle_input("\x1b[A"); // Up - shows entry, cursor at end
            editor.handle_input("\x1b[A"); // Up - cursor to line2
            editor.handle_input("\x1b[A"); // Up - cursor to line1

            // Now Down should move cursor down within the entry
            editor.handle_input("\x1b[B"); // Down - cursor to line2
            assert_eq!(editor.get_text(), "line1\nline2\nline3");

            editor.handle_input("\x1b[B"); // Down - cursor to line3
            assert_eq!(editor.get_text(), "line1\nline2\nline3");

            // Now on last line, Down should exit history
            editor.handle_input("\x1b[B"); // Down - exit to empty
            assert_eq!(editor.get_text(), "");
        }
    }

    mod public_state_accessors {
        use super::*;
        #[test]
        fn returns_cursor_position() {
            let mut editor = ed();

            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("a");
            editor.handle_input("b");
            editor.handle_input("c");

            assert_eq!(editor.get_cursor(), (0, 3));

            editor.handle_input("\x1b[D"); // Left
            assert_eq!(editor.get_cursor(), (0, 2));
        }

        #[test]
        fn returns_lines_as_a_defensive_copy() {
            let mut editor = ed();
            editor.set_text("a\nb");

            let mut lines = editor.get_lines();
            assert_eq!(lines, ["a", "b"]);

            lines[0] = "mutated".to_string();
            assert_eq!(editor.get_lines(), ["a", "b"]);
        }
    }

    mod backslash_enter_newline_workaround {
        use super::*;
        #[test]
        fn inserts_backslash_immediately_no_buffering() {
            let mut editor = ed();

            editor.handle_input("\\");

            // Backslash should be visible immediately, not buffered
            assert_eq!(editor.get_text(), "\\");
        }

        #[test]
        fn converts_standalone_backslash_to_newline_on_enter() {
            let mut editor = ed();

            editor.handle_input("\\");
            editor.handle_input("\r");

            assert_eq!(editor.get_text(), "\n");
        }

        #[test]
        fn inserts_backslash_normally_when_followed_by_other_characters() {
            let mut editor = ed();

            editor.handle_input("\\");
            editor.handle_input("x");

            assert_eq!(editor.get_text(), "\\x");
        }

        #[test]
        fn does_not_trigger_newline_when_backslash_is_not_immediately_before_cursor() {
            let mut editor = ed();
            let submitted = capture_submit(&mut editor);

            editor.handle_input("\\");
            editor.handle_input("x");
            editor.handle_input("\r");

            // Should submit, not insert newline (backslash not at cursor)
            assert!(!submitted.borrow().is_empty());
        }

        #[test]
        fn only_removes_one_backslash_when_multiple_are_present() {
            let mut editor = ed();

            editor.handle_input("\\");
            editor.handle_input("\\");
            editor.handle_input("\\");
            assert_eq!(editor.get_text(), "\\\\\\");

            editor.handle_input("\r");
            // Only the last backslash is removed, newline inserted
            assert_eq!(editor.get_text(), "\\\\\n");
        }
    }

    mod kitty_csi_u_handling {
        use super::*;
        #[test]
        fn ignores_printable_csi_u_sequences_with_unsupported_modifiers() {
            let mut editor = ed();

            editor.handle_input("\x1b[99;9u");

            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn inserts_shifted_csi_u_letters_as_text() {
            let mut editor = ed();

            editor.handle_input("\x1b[69;2u");

            assert_eq!(editor.get_text(), "E");
        }

        #[test]
        fn inserts_shifted_xterm_modifyotherkeys_letters_as_text() {
            let mut editor = ed();

            editor.handle_input("\x1b[27;2;69~");

            assert_eq!(editor.get_text(), "E");
        }
    }

    mod unicode_text_editing_behavior {
        use super::*;
        #[test]
        fn inserts_mixed_ascii_umlauts_and_emojis_as_literal_text() {
            let mut editor = ed();

            editor.handle_input("H");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("ä");
            editor.handle_input("ö");
            editor.handle_input("ü");
            editor.handle_input(" ");
            editor.handle_input("😀");

            let text = editor.get_text();
            assert_eq!(text, "Hello äöü 😀");
        }

        #[test]
        fn deletes_single_code_unit_unicode_characters_umlauts_with_backspace() {
            let mut editor = ed();

            editor.handle_input("ä");
            editor.handle_input("ö");
            editor.handle_input("ü");

            // Delete the last character (ü)
            editor.handle_input("\x7f"); // Backspace

            let text = editor.get_text();
            assert_eq!(text, "äö");
        }

        #[test]
        fn deletes_multi_code_unit_emojis_with_single_backspace() {
            let mut editor = ed();

            editor.handle_input("😀");
            editor.handle_input("👍");

            // Delete the last emoji (👍) - single backspace deletes whole grapheme cluster
            editor.handle_input("\x7f"); // Backspace

            let text = editor.get_text();
            assert_eq!(text, "😀");
        }

        #[test]
        fn inserts_characters_at_the_correct_position_after_cursor_movement_over_umlauts() {
            let mut editor = ed();

            editor.handle_input("ä");
            editor.handle_input("ö");
            editor.handle_input("ü");

            // Move cursor left twice
            editor.handle_input("\x1b[D"); // Left arrow
            editor.handle_input("\x1b[D"); // Left arrow

            // Insert 'x' in the middle
            editor.handle_input("x");

            let text = editor.get_text();
            assert_eq!(text, "äxöü");
        }

        #[test]
        fn moves_cursor_across_multi_code_unit_emojis_with_single_arrow_key() {
            let mut editor = ed();

            editor.handle_input("😀");
            editor.handle_input("👍");
            editor.handle_input("🎉");

            // Move cursor left over last emoji (🎉) - single arrow moves over whole grapheme
            editor.handle_input("\x1b[D"); // Left arrow

            // Move cursor left over second emoji (👍)
            editor.handle_input("\x1b[D");

            // Insert 'x' between first and second emoji
            editor.handle_input("x");

            let text = editor.get_text();
            assert_eq!(text, "😀x👍🎉");
        }

        #[test]
        fn preserves_umlauts_across_line_breaks() {
            let mut editor = ed();

            editor.handle_input("ä");
            editor.handle_input("ö");
            editor.handle_input("ü");
            editor.handle_input("\n"); // new line
            editor.handle_input("Ä");
            editor.handle_input("Ö");
            editor.handle_input("Ü");

            let text = editor.get_text();
            assert_eq!(text, "äöü\nÄÖÜ");
        }

        #[test]
        fn replaces_the_entire_document_with_unicode_text_via_settext_paste_simulation() {
            let mut editor = ed();

            // Simulate bracketed paste / programmatic replacement
            editor.set_text("Hällö Wörld! 😀 äöüÄÖÜß");

            let text = editor.get_text();
            assert_eq!(text, "Hällö Wörld! 😀 äöüÄÖÜß");
        }

        #[test]
        fn moves_cursor_to_document_start_on_ctrl_a_and_inserts_at_the_beginning() {
            let mut editor = ed();

            editor.handle_input("a");
            editor.handle_input("b");
            editor.handle_input("\x01"); // Ctrl+A (move to start)
            editor.handle_input("x"); // Insert at start

            let text = editor.get_text();
            assert_eq!(text, "xab");
        }

        #[test]
        fn deletes_words_correctly_with_ctrl_w_and_alt_backspace() {
            let mut editor = ed();

            // Basic word deletion
            editor.set_text("foo bar baz");
            editor.handle_input("\x17"); // Ctrl+W
            assert_eq!(editor.get_text(), "foo bar ");

            // Trailing whitespace
            editor.set_text("foo bar   ");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "foo ");

            // Punctuation run
            editor.set_text("foo bar...");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "foo bar");

            // Delete across multiple lines
            editor.set_text("line one\nline two");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "line one\nline ");

            // Delete empty line (merge)
            editor.set_text("line one\n");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "line one");

            // Grapheme safety (emoji as a word)
            editor.set_text("foo 😀😀 bar");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "foo 😀😀 ");
            editor.handle_input("\x17");
            assert_eq!(editor.get_text(), "foo ");

            // Alt+Backspace
            editor.set_text("foo bar");
            editor.handle_input("\x1b\x7f"); // Alt+Backspace (legacy)
            assert_eq!(editor.get_text(), "foo ");
        }

        #[test]
        fn navigates_words_correctly_with_ctrl_left_right() {
            let mut editor = ed();

            editor.set_text("foo bar... baz");
            // Cursor at end

            // Move left over baz
            editor.handle_input("\x1b[1;5D"); // Ctrl+Left
            assert_eq!(editor.get_cursor(), (0, 11)); // after '...'

            // Move left over punctuation
            editor.handle_input("\x1b[1;5D"); // Ctrl+Left
            assert_eq!(editor.get_cursor(), (0, 7)); // after 'bar'

            // Move left over bar
            editor.handle_input("\x1b[1;5D"); // Ctrl+Left
            assert_eq!(editor.get_cursor(), (0, 4)); // after 'foo '

            // Move right over bar
            editor.handle_input("\x1b[1;5C"); // Ctrl+Right
            assert_eq!(editor.get_cursor(), (0, 7)); // at end of 'bar'

            // Move right over punctuation run
            editor.handle_input("\x1b[1;5C"); // Ctrl+Right
            assert_eq!(editor.get_cursor(), (0, 10)); // after '...'

            // Move right skips space and lands after baz
            editor.handle_input("\x1b[1;5C"); // Ctrl+Right
            assert_eq!(editor.get_cursor(), (0, 14)); // end of line

            // Test forward from start with leading whitespace
            editor.set_text("   foo bar");
            editor.handle_input("\x01"); // Ctrl+A to go to start
            editor.handle_input("\x1b[1;5C"); // Ctrl+Right
            assert_eq!(editor.get_cursor(), (0, 6)); // after 'foo'
        }
    }

    mod grapheme_aware_text_wrapping {
        use super::*;
        #[test]
        fn wraps_lines_correctly_when_text_contains_wide_emojis() {
            let mut editor = ed();
            let width = 20;

            // ✅ is 2 columns wide, so "Hello ✅ World" is 14 columns
            editor.set_text("Hello ✅ World");
            let lines = render(&mut editor, width);

            // All content lines (between borders) should fit within width
            for i in 1..lines.len() - 1 {
                let lineWidth = visible_width(&lines[i]);
                assert_eq!(
                    lineWidth, width,
                    "Line {} has width {}, expected {}",
                    i, lineWidth, width
                );
            }
        }

        #[test]
        fn wraps_long_text_with_emojis_at_correct_positions() {
            let mut editor = ed();
            let width = 10;

            // Each ✅ is 2 columns. "✅✅✅✅✅" = 10 columns, fits exactly
            // "✅✅✅✅✅✅" = 12 columns, needs wrap
            editor.set_text("✅✅✅✅✅✅");
            let lines = render(&mut editor, width);

            // Should have 2 content lines (plus 2 border lines)
            // First line: 5 emojis (10 cols), second line: 1 emoji (2 cols) + padding
            for i in 1..lines.len() - 1 {
                let lineWidth = visible_width(&lines[i]);
                assert_eq!(
                    lineWidth, width,
                    "Line {} has width {}, expected {}",
                    i, lineWidth, width
                );
            }
        }

        #[test]
        fn renders_isolated_thai_and_lao_am_clusters_without_width_drift() {
            for text in ["ำabc", "ຳabc"] {
                let mut editor = ed();
                let width = 8;
                editor.set_text(text);

                for line in render(&mut editor, width) {
                    assert_eq!(
                        visible_width(&line),
                        width,
                        "line width drift for {}: {}",
                        dbg(text),
                        line
                    );
                }
            }
        }

        #[test]
        fn wraps_cjk_characters_correctly_each_is_2_columns_wide() {
            let mut editor = ed();
            let width = 10 + 1; // +1 col reserved for cursor

            // Each CJK char is 2 columns. "日本語テスト" = 6 chars = 12 columns
            editor.set_text("日本語テスト");
            let lines = render(&mut editor, width);

            for i in 1..lines.len() - 1 {
                let lineWidth = visible_width(&lines[i]);
                assert_eq!(
                    lineWidth, width,
                    "Line {} has width {}, expected {}",
                    i, lineWidth, width
                );
            }

            // Verify content split correctly
            let contentLines = lines[1..lines.len() - 1]
                .iter()
                .map(|l| strip(l).trim().to_string())
                .collect::<Vec<_>>();
            assert_eq!(contentLines.len(), 2);
            assert_eq!(contentLines[0], "日本語テス"); // 5 chars = 10 columns
            assert_eq!(contentLines[1], "ト"); // 1 char = 2 columns (+ padding)
        }

        #[test]
        fn handles_mixed_ascii_and_wide_characters_in_wrapping() {
            let mut editor = ed();
            let width = 15 + 1; // +1 col reserved for cursor

            // "Test ✅ OK 日本" = 4 + 1 + 2 + 1 + 2 + 1 + 4 = 15 columns (fits in width-1=15)
            editor.set_text("Test ✅ OK 日本");
            let lines = render(&mut editor, width);

            // Should fit in one content line
            let contentLines = lines[1..lines.len() - 1].to_vec();
            assert_eq!(contentLines.len(), 1);

            let lineWidth = visible_width(&contentLines[0]);
            assert_eq!(lineWidth, width);
        }

        #[test]
        fn renders_cursor_correctly_on_wide_characters() {
            let mut editor = ed();
            let width = 20;

            editor.set_text("A✅B");
            // Cursor should be at end (after B)
            let lines = render(&mut editor, width);

            // The cursor (reverse video space) should be visible
            let contentLine = lines[1].clone();
            assert!(
                contentLine.contains("\x1b[7m"),
                "Should have reverse video cursor"
            );

            // Line should still be correct width
            assert_eq!(visible_width(&contentLine), width);
        }

        #[test]
        fn does_not_exceed_terminal_width_with_emoji_at_wrap_boundary() {
            let mut editor = ed();
            let width = 11;

            // "0123456789✅" = 10 ASCII + 2-wide emoji = 12 columns
            // Should wrap before the emoji since it would exceed width
            editor.set_text("0123456789✅");
            let lines = render(&mut editor, width);

            for i in 1..lines.len() - 1 {
                let lineWidth = visible_width(&lines[i]);
                assert!(
                    lineWidth <= width,
                    "Line {} has width {}, exceeds max {}",
                    i,
                    lineWidth,
                    width
                );
            }
        }

        #[test]
        fn shows_cursor_at_end_of_line_before_wrap_wraps_on_next_char() {
            let width = 10;
            for paddingX in [0, 1] {
                let mut editor = ed_with(width + paddingX, 24, paddingX, None);

                // Type 9 chars → fills layoutWidth exactly, cursor at end on same line
                for ch in "aaaaaaaaa".chars().map(String::from) {
                    editor.handle_input(&ch);
                }
                let mut lines = render(&mut editor, width + paddingX);
                let mut contentLines = lines[1..lines.len() - 1].to_vec();
                assert_eq!(
                    contentLines.len(),
                    1,
                    "Should be 1 content line before wrap"
                );
                assert!(
                    contentLines[0].ends_with("\x1b[7m \x1b[0m"),
                    "Cursor should be at end of line"
                );

                // Type 1 more → text wraps to second line
                editor.handle_input("a");
                lines = render(&mut editor, width + paddingX);
                contentLines = lines[1..lines.len() - 1].to_vec();
                assert_eq!(contentLines.len(), 2, "Should wrap to 2 content lines");
            }
        }
    }

    mod prompt_prefix {
        use super::*;
        #[test]
        fn renders_prompt_prefix_on_the_first_line() {
            let mut editor = ed();
            let width = 20;

            editor.prompt_prefix = ">".into();
            editor.set_text("hello");
            let lines = render(&mut editor, width);

            let contentLines = lines[1..lines.len() - 1]
                .iter()
                .map(|l| strip(l).trim_end().to_string())
                .collect::<Vec<_>>();
            assert_eq!(contentLines.len(), 1);
            assert!(
                contentLines[0].starts_with("> "),
                "Should start with \"> \", got: \"{}\"",
                contentLines[0]
            );
            assert!(
                contentLines[0].contains("hello"),
                "Should contain text, got: \"{}\"",
                contentLines[0]
            );
        }

        #[test]
        fn applies_prompt_color_to_the_prefix() {
            let mut editor = ed();
            let width = 20;

            editor.prompt_prefix = "!".into();
            editor.prompt_color = Box::new(|s: &str| format!("\x1b[32m{s}\x1b[0m"));
            editor.set_text("bash");
            let lines = render(&mut editor, width);

            let contentLine = lines[1].clone();
            assert!(contentLine.contains("\x1b[32m"), "Prefix should be colored");
        }

        #[test]
        fn accounts_for_prefix_width_in_first_line_wrapping() {
            let mut editor = ed();
            let width = 12;

            editor.prompt_prefix = ">".into();
            editor.set_text("helloworld");
            let lines = render(&mut editor, width);

            let contentLines = lines[1..lines.len() - 1]
                .iter()
                .map(|l| strip(l).trim_end().to_string())
                .collect::<Vec<_>>();
            assert_eq!(
                contentLines.len(),
                2,
                "Should wrap to 2 lines accounting for prefix"
            );
            assert!(
                contentLines[0].starts_with("> "),
                "First line should have prefix"
            );
            assert!(
                !contentLines[1].starts_with(">"),
                "Second line should not have prefix"
            );
        }
    }

    mod word_wrapping {
        use super::*;
        #[test]
        fn wraps_at_word_boundaries_instead_of_mid_word() {
            let mut editor = ed();
            let width = 40;

            editor.set_text("Hello world this is a test of word wrapping functionality");
            let lines = render(&mut editor, width);

            // Get content lines (between borders)
            let contentLines = lines[1..lines.len() - 1]
                .iter()
                .map(|l| strip(l).trim().to_string())
                .collect::<Vec<_>>();

            // Should NOT break mid-word
            // Line 1 should end with a complete word
            assert!(
                !contentLines[0].ends_with("-"),
                "Line should not end with hyphen (mid-word break)"
            );

            // Each content line should be complete words
            for line in contentLines {
                // Words at end of line should be complete (no partial words)
                let lastChar = line.trim_end().chars().last();
                assert!(
                    lastChar
                        .is_none_or(|c| c.is_alphanumeric() || c == '_' || ".,!?;:".contains(c)),
                    "Line ends unexpectedly with: {:?}",
                    lastChar
                );
            }
        }

        #[test]
        fn does_not_start_lines_with_leading_whitespace_after_word_wrap() {
            let mut editor = ed();
            let width = 20;

            editor.set_text("Word1 Word2 Word3 Word4 Word5 Word6");
            let lines = render(&mut editor, width);

            // Get content lines (between borders)
            let contentLines = lines[1..lines.len() - 1].to_vec();

            // No line should start with whitespace (except for padding at the end)
            for i in 0..contentLines.len() {
                let line = strip(&contentLines[i]);
                let trimmedStart = line.trim_start();
                // The line should either be all padding or start with a word character
                if !trimmedStart.is_empty() {
                    let t = line.trim_end();
                    assert!(
                        !(t.starts_with(char::is_whitespace) && t.trim_start() != ""),
                        "Line {} starts with unexpected whitespace before content",
                        i
                    );
                }
            }
        }

        #[test]
        fn breaks_long_words_urls_at_character_level() {
            let mut editor = ed();
            let width = 30;

            editor.set_text("Check https://example.com/very/long/path/that/exceeds/width here");
            let lines = render(&mut editor, width);

            // All lines should fit within width
            for i in 1..lines.len() - 1 {
                let lineWidth = visible_width(&lines[i]);
                assert_eq!(
                    lineWidth, width,
                    "Line {} has width {}, expected {}",
                    i, lineWidth, width
                );
            }
        }

        #[test]
        fn preserves_multiple_spaces_within_words_on_same_line() {
            let mut editor = ed();
            let width = 50;

            editor.set_text("Word1   Word2    Word3");
            let lines = render(&mut editor, width);

            let contentLine = strip(&lines[1]).trim().to_string();
            // Multiple spaces should be preserved
            assert!(
                contentLine.contains("Word1   Word2"),
                "Multiple spaces should be preserved"
            );
        }

        #[test]
        fn handles_empty_string() {
            let mut editor = ed();
            let width = 40;

            editor.set_text("");
            let lines = render(&mut editor, width);

            // Should have border + empty content + border
            assert_eq!(lines.len(), 3);
        }

        #[test]
        fn handles_single_word_that_fits_exactly() {
            let mut editor = ed();
            let width = 10 + 1; // +1 col reserved for cursor

            editor.set_text("1234567890");
            let lines = render(&mut editor, width);

            // Should have exactly 3 lines (top border, content, bottom border)
            assert_eq!(lines.len(), 3);
            let contentLine = strip(&lines[1]);
            assert!(
                contentLine.contains("1234567890"),
                "Content should contain the word"
            );
        }

        #[test]
        fn wraps_word_to_next_line_when_it_ends_exactly_at_terminal_width() {
            // "hello " (6) + "world" (5) = 11, but "world" is non-whitespace ending at width.
            // Thus, wrap it to next line. The trailing space stays with "hello" on line 1
            let chunks = word_wrap_line("hello world test", 11);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "hello ");
            assert_eq!(chunks[1].text, "world test");
        }

        #[test]
        fn keeps_whitespace_at_terminal_width_boundary_on_same_line() {
            // "hello world " is exactly 12 chars (including trailing space)
            // The space at position 12 should stay on the first line
            let chunks = word_wrap_line("hello world test", 12);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "hello world ");
            assert_eq!(chunks[1].text, "test");
        }

        #[test]
        fn handles_unbreakable_word_filling_width_exactly_followed_by_space() {
            let chunks = word_wrap_line("aaaaaaaaaaaa aaaa", 12);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "aaaaaaaaaaaa");
            assert_eq!(chunks[1].text, " aaaa");
        }

        #[test]
        fn wraps_word_to_next_line_when_it_fits_width_but_not_remaining_space() {
            let chunks = word_wrap_line("      aaaaaaaaaaaa", 12);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "      ");
            assert_eq!(chunks[1].text, "aaaaaaaaaaaa");
        }

        #[test]
        fn keeps_word_with_multi_space_and_following_word_together_when_they_fit() {
            let chunks = word_wrap_line("Lorem ipsum dolor sit amet,    consectetur", 30);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,    consectetur");
        }

        #[test]
        fn keeps_word_with_multi_space_and_following_word_when_they_fill_width_exactly() {
            let chunks = word_wrap_line("Lorem ipsum dolor sit amet,              consectetur", 30);

            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,              consectetur");
        }

        #[test]
        fn splits_when_word_plus_multi_space_plus_word_exceeds_width() {
            let chunks =
                word_wrap_line("Lorem ipsum dolor sit amet,               consectetur", 30);

            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,               ");
            assert_eq!(chunks[2].text, "consectetur");
        }

        #[test]
        fn breaks_long_whitespace_at_line_boundary() {
            let chunks = word_wrap_line(
                "Lorem ipsum dolor sit amet,                         consectetur",
                30,
            );

            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,                         ");
            assert_eq!(chunks[2].text, "consectetur");
        }

        #[test]
        fn breaks_long_whitespace_at_line_boundary_2() {
            let chunks = word_wrap_line(
                "Lorem ipsum dolor sit amet,                          consectetur",
                30,
            );

            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,                         ");
            assert_eq!(chunks[2].text, " consectetur");
        }

        #[test]
        fn breaks_whitespace_spanning_full_lines() {
            let chunks = word_wrap_line(
                "Lorem ipsum dolor sit amet,                                     consectetur",
                30,
            );

            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
            assert_eq!(chunks[1].text, "amet,                         ");
            assert_eq!(chunks[2].text, "            consectetur");
        }

        fn segs(parts: &[&str]) -> Vec<Segment> {
            let mut index = 0;
            parts
                .iter()
                .map(|p| {
                    let s = Segment {
                        segment: p.to_string(),
                        index,
                    };
                    index += len16(p);
                    s
                })
                .collect()
        }

        fn check_chunks(line: &str, chunks: &[TextChunk], max: usize) {
            for chunk in chunks {
                assert!(
                    visible_width(&chunk.text) <= max,
                    "chunk {:?} has visible width {}, expected <= {max}",
                    chunk.text,
                    visible_width(&chunk.text)
                );
            }
            let reconstructed: String = chunks
                .iter()
                .map(|c| slice16(line, c.start_index, c.end_index))
                .collect();
            assert_eq!(reconstructed, line);
        }

        #[test]
        fn force_breaks_when_wide_char_after_word_boundary_wrap_still_overflows() {
            // " " (1) + "a"*186 (186) + "你" (2) = 189 visible width
            // maxWidth = 187: backtracking to the space would leave 186 + 2 = 188 > 187,
            // so the algorithm must force-break before the wide char instead.
            let line = format!(" {}你", "a".repeat(186));
            let chunks = word_wrap_line(&line, 187);
            check_chunks(&line, &chunks, 187);
        }

        #[test]
        fn splits_oversized_atomic_segment_across_multiple_chunks() {
            // Simulate a paste marker wider than maxWidth by passing pre-segmented data
            let marker = "[paste #1 +20 lines]";
            let line = format!("A{marker}B");
            let chunks = word_wrap_segments(&line, 10, Some(segs(&["A", marker, "B"])));
            check_chunks(&line, &chunks, 10);
        }

        #[test]
        fn splits_oversized_atomic_segment_at_start_of_line() {
            let marker = "[paste #1 +20 lines]";
            let line = format!("{marker}B");
            let chunks = word_wrap_segments(&line, 10, Some(segs(&[marker, "B"])));
            check_chunks(&line, &chunks, 10);
            // "B" ends up on the last line (either alone or with the marker tail)
            assert!(chunks[chunks.len() - 1].text.contains('B'));
        }

        #[test]
        fn splits_oversized_atomic_segment_at_end_of_line() {
            let marker = "[paste #1 +20 lines]";
            let line = format!("A{marker}");
            let chunks = word_wrap_segments(&line, 10, Some(segs(&["A", marker])));
            check_chunks(&line, &chunks, 10);
            assert_eq!(chunks[0].text, "A");
        }

        #[test]
        fn splits_consecutive_oversized_atomic_segments() {
            let m1 = "[paste #1 +20 lines]";
            let m2 = "[paste #2 +30 lines]";
            let line = format!("{m1}{m2}");
            let chunks = word_wrap_segments(&line, 10, Some(segs(&[m1, m2])));
            check_chunks(&line, &chunks, 10);
        }

        #[test]
        fn wraps_normally_after_oversized_atomic_segment() {
            let marker = "[paste #1 +20 lines]";
            let line = format!("{marker} hello world");
            let mut parts = vec![marker];
            let tail: Vec<String> = " hello world".chars().map(|c| c.to_string()).collect();
            parts.extend(tail.iter().map(|s| s.as_str()));
            let chunks = word_wrap_segments(&line, 10, Some(segs(&parts)));
            check_chunks(&line, &chunks, 10);
            // Last chunk should contain "world" (normal wrapping resumes)
            assert_eq!(chunks[chunks.len() - 1].text, "world");
        }
    }

    mod kill_ring {
        use super::*;
        #[test]
        fn ctrl_w_saves_deleted_text_to_kill_ring_and_ctrl_y_yanks_it() {
            let mut editor = ed();

            editor.set_text("foo bar baz");
            editor.handle_input("\x17"); // Ctrl+W - deletes "baz"
            assert_eq!(editor.get_text(), "foo bar ");

            // Move to beginning and yank
            editor.handle_input("\x01"); // Ctrl+A
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "bazfoo bar ");
        }

        #[test]
        fn ctrl_u_saves_deleted_text_to_kill_ring() {
            let mut editor = ed();

            editor.set_text("hello world");
            // Move cursor to middle
            editor.handle_input("\x01"); // Ctrl+A (start)
            editor.handle_input("\x1b[C"); // Right 5 times
            editor.handle_input("\x1b[C");
            editor.handle_input("\x1b[C");
            editor.handle_input("\x1b[C");
            editor.handle_input("\x1b[C");
            editor.handle_input("\x1b[C"); // After "hello "

            editor.handle_input("\x15"); // Ctrl+U - deletes "hello "
            assert_eq!(editor.get_text(), "world");

            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn ctrl_k_saves_deleted_text_to_kill_ring() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A (start)
            editor.handle_input("\x0b"); // Ctrl+K - deletes "hello world"

            assert_eq!(editor.get_text(), "");

            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn ctrl_y_does_nothing_when_kill_ring_is_empty() {
            let mut editor = ed();

            editor.set_text("test");
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "test");
        }

        #[test]
        fn alt_y_cycles_through_kill_ring_after_ctrl_y() {
            let mut editor = ed();

            // Create kill ring with multiple entries
            editor.set_text("first");
            editor.handle_input("\x17"); // Ctrl+W - deletes "first"
            editor.set_text("second");
            editor.handle_input("\x17"); // Ctrl+W - deletes "second"
            editor.set_text("third");
            editor.handle_input("\x17"); // Ctrl+W - deletes "third"

            // Kill ring now has: [first, second, third]
            assert_eq!(editor.get_text(), "");

            editor.handle_input("\x19"); // Ctrl+Y - yanks "third" (most recent)
            assert_eq!(editor.get_text(), "third");

            editor.handle_input("\x1by"); // Alt+Y - cycles to "second"
            assert_eq!(editor.get_text(), "second");

            editor.handle_input("\x1by"); // Alt+Y - cycles to "first"
            assert_eq!(editor.get_text(), "first");

            editor.handle_input("\x1by"); // Alt+Y - cycles back to "third"
            assert_eq!(editor.get_text(), "third");
        }

        #[test]
        fn alt_y_does_nothing_if_not_preceded_by_yank() {
            let mut editor = ed();

            editor.set_text("test");
            editor.handle_input("\x17"); // Ctrl+W - deletes "test"
            editor.set_text("other");

            // Type something to break the yank chain
            editor.handle_input("x");
            assert_eq!(editor.get_text(), "otherx");

            // Alt+Y should do nothing
            editor.handle_input("\x1by"); // Alt+Y
            assert_eq!(editor.get_text(), "otherx");
        }

        #[test]
        fn alt_y_does_nothing_if_kill_ring_has_1_entry() {
            let mut editor = ed();

            editor.set_text("only");
            editor.handle_input("\x17"); // Ctrl+W - deletes "only"

            editor.handle_input("\x19"); // Ctrl+Y - yanks "only"
            assert_eq!(editor.get_text(), "only");

            editor.handle_input("\x1by"); // Alt+Y - should do nothing (only 1 entry)
            assert_eq!(editor.get_text(), "only");
        }

        #[test]
        fn consecutive_ctrl_w_accumulates_into_one_kill_ring_entry() {
            let mut editor = ed();

            editor.set_text("one two three");
            editor.handle_input("\x17"); // Ctrl+W - deletes "three"
            editor.handle_input("\x17"); // Ctrl+W - deletes "two " (prepended)
            editor.handle_input("\x17"); // Ctrl+W - deletes "one " (prepended)

            assert_eq!(editor.get_text(), "");

            // Should be one combined entry
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "one two three");
        }

        #[test]
        fn ctrl_u_accumulates_multiline_deletes_including_newlines() {
            let mut editor = ed();

            // Start with multiline text, cursor at end
            editor.set_text("line1\nline2\nline3");
            // Cursor is at end of line3 (line 2, col 5)

            // Delete "line3"
            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "line1\nline2\n");

            // Delete newline (at start of empty line 2, merges with line1)
            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "line1\nline2");

            // Delete "line2"
            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "line1\n");

            // Delete newline
            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "line1");

            // Delete "line1"
            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "");

            // All deletions accumulated into one entry: "line1\nline2\nline3"
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "line1\nline2\nline3");
        }

        #[test]
        fn backward_deletions_prepend_forward_deletions_append_during_accumulation() {
            let mut editor = ed();

            editor.set_text("prefix|suffix");
            // Position cursor at |
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            } // Move right 6 times

            editor.handle_input("\x0b"); // Ctrl+K - deletes "suffix" (forward)
            editor.handle_input("\x0b"); // Ctrl+K - deletes "|" (forward, appended)
            assert_eq!(editor.get_text(), "prefix");

            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "prefix|suffix");
        }

        #[test]
        fn non_delete_actions_break_kill_accumulation() {
            let mut editor = ed();

            // Delete "baz", then type "x" to break accumulation, then delete "x"
            editor.set_text("foo bar baz");
            editor.handle_input("\x17"); // Ctrl+W - deletes "baz"
            assert_eq!(editor.get_text(), "foo bar ");

            editor.handle_input("x"); // Typing breaks accumulation
            assert_eq!(editor.get_text(), "foo bar x");

            editor.handle_input("\x17"); // Ctrl+W - deletes "x" (separate entry, not accumulated)
            assert_eq!(editor.get_text(), "foo bar ");

            // Yank most recent - should be "x", not "xbaz"
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "foo bar x");

            // Cycle to previous - should be "baz" (separate entry)
            editor.handle_input("\x1by"); // Alt+Y
            assert_eq!(editor.get_text(), "foo bar baz");
        }

        #[test]
        fn non_yank_actions_break_alt_y_chain() {
            let mut editor = ed();

            editor.set_text("first");
            editor.handle_input("\x17"); // Ctrl+W
            editor.set_text("second");
            editor.handle_input("\x17"); // Ctrl+W
            editor.set_text("");

            editor.handle_input("\x19"); // Ctrl+Y - yanks "second"
            assert_eq!(editor.get_text(), "second");

            editor.handle_input("x"); // Type breaks yank chain
            assert_eq!(editor.get_text(), "secondx");

            editor.handle_input("\x1by"); // Alt+Y - should do nothing
            assert_eq!(editor.get_text(), "secondx");
        }

        #[test]
        fn kill_ring_rotation_persists_after_cycling() {
            let mut editor = ed();

            editor.set_text("first");
            editor.handle_input("\x17"); // deletes "first"
            editor.set_text("second");
            editor.handle_input("\x17"); // deletes "second"
            editor.set_text("third");
            editor.handle_input("\x17"); // deletes "third"
            editor.set_text("");

            // Ring: [first, second, third]

            editor.handle_input("\x19"); // Ctrl+Y - yanks "third"
            editor.handle_input("\x1by"); // Alt+Y - cycles to "second", ring rotates

            // Now ring is: [third, first, second]
            assert_eq!(editor.get_text(), "second");

            // Do something else
            editor.handle_input("x");
            editor.set_text("");

            // New yank should get "second" (now at end after rotation)
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "second");
        }

        #[test]
        fn consecutive_deletions_across_lines_coalesce_into_one_entry() {
            let mut editor = ed();

            // "1\n2\n3" with cursor at end, delete everything with Ctrl+W
            editor.set_text("1\n2\n3");
            editor.handle_input("\x17"); // Ctrl+W - deletes "3"
            assert_eq!(editor.get_text(), "1\n2\n");

            editor.handle_input("\x17"); // Ctrl+W - deletes newline (merge with prev line)
            assert_eq!(editor.get_text(), "1\n2");

            editor.handle_input("\x17"); // Ctrl+W - deletes "2"
            assert_eq!(editor.get_text(), "1\n");

            editor.handle_input("\x17"); // Ctrl+W - deletes newline
            assert_eq!(editor.get_text(), "1");

            editor.handle_input("\x17"); // Ctrl+W - deletes "1"
            assert_eq!(editor.get_text(), "");

            // All deletions should have accumulated into one entry
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "1\n2\n3");
        }

        #[test]
        fn ctrl_k_at_line_end_deletes_newline_and_coalesces() {
            let mut editor = ed();

            // "ab" on line 1, "cd" on line 2, cursor at end of line 1
            editor.set_text("");
            editor.handle_input("a");
            editor.handle_input("b");
            editor.handle_input("\n");
            editor.handle_input("c");
            editor.handle_input("d");
            // Move to end of first line
            editor.handle_input("\x1b[A"); // Up arrow
            editor.handle_input("\x05"); // Ctrl+E - end of line

            // Now at end of "ab", Ctrl+K should delete newline (merge with "cd")
            editor.handle_input("\x0b"); // Ctrl+K - deletes newline
            assert_eq!(editor.get_text(), "abcd");

            // Continue deleting
            editor.handle_input("\x0b"); // Ctrl+K - deletes "cd"
            assert_eq!(editor.get_text(), "ab");

            // Both deletions should accumulate
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "ab\ncd");
        }

        #[test]
        fn handles_yank_in_middle_of_text() {
            let mut editor = ed();

            editor.set_text("word");
            editor.handle_input("\x17"); // Ctrl+W - deletes "word"
            editor.set_text("hello world");

            // Move to middle (after "hello ")
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            }

            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello wordworld");
        }

        #[test]
        fn handles_yank_pop_in_middle_of_text() {
            let mut editor = ed();

            // Create two kill ring entries
            editor.set_text("FIRST");
            editor.handle_input("\x17"); // Ctrl+W - deletes "FIRST"
            editor.set_text("SECOND");
            editor.handle_input("\x17"); // Ctrl+W - deletes "SECOND"

            // Ring: ["FIRST", "SECOND"]

            // Set up "hello world" and position cursor after "hello "
            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start of line
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            } // Move right 6

            // Yank "SECOND" in the middle
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello SECONDworld");

            // Yank-pop replaces "SECOND" with "FIRST"
            editor.handle_input("\x1by"); // Alt+Y
            assert_eq!(editor.get_text(), "hello FIRSTworld");
        }

        #[test]
        fn multiline_yank_and_yank_pop_in_middle_of_text() {
            let mut editor = ed();

            // Create single-line entry
            editor.set_text("SINGLE");
            editor.handle_input("\x17"); // Ctrl+W - deletes "SINGLE"

            // Create multiline entry via consecutive Ctrl+U
            editor.set_text("A\nB");
            editor.handle_input("\x15"); // Ctrl+U - deletes "B"
            editor.handle_input("\x15"); // Ctrl+U - deletes newline
            editor.handle_input("\x15"); // Ctrl+U - deletes "A"
                                         // Ring: ["SINGLE", "A\nB"]

            // Insert in middle of "hello world"
            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            }

            // Yank multiline "A\nB"
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello A\nBworld");

            // Yank-pop replaces with "SINGLE"
            editor.handle_input("\x1by"); // Alt+Y
            assert_eq!(editor.get_text(), "hello SINGLEworld");
        }

        #[test]
        fn alt_d_deletes_word_forward_and_saves_to_kill_ring() {
            let mut editor = ed();

            editor.set_text("hello world test");
            editor.handle_input("\x01"); // Ctrl+A - go to start

            editor.handle_input("\x1bd"); // Alt+D - deletes "hello"
            assert_eq!(editor.get_text(), " world test");

            editor.handle_input("\x1bd"); // Alt+D - deletes " world" (skips whitespace, then word)
            assert_eq!(editor.get_text(), " test");

            // Yank should get accumulated text
            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "hello world test");
        }

        #[test]
        fn alt_d_at_end_of_line_deletes_newline() {
            let mut editor = ed();

            editor.set_text("line1\nline2");
            // Move to start of document, then to end of first line
            editor.handle_input("\x1b[A"); // Up arrow - go to first line
            editor.handle_input("\x05"); // Ctrl+E - end of line

            editor.handle_input("\x1bd"); // Alt+D - deletes newline (merges lines)
            assert_eq!(editor.get_text(), "line1line2");

            editor.handle_input("\x19"); // Ctrl+Y
            assert_eq!(editor.get_text(), "line1\nline2");
        }
    }

    mod undo {
        use super::*;
        #[test]
        fn does_nothing_when_undo_stack_is_empty() {
            let mut editor = ed();

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn coalesces_consecutive_word_characters_into_one_undo_unit() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "hello world");

            // Undo removes " world" (space captured state before it, so we restore to "hello")
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello");

            // Undo removes "hello"
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn undoes_spaces_one_at_a_time() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input(" ");
            assert_eq!(editor.get_text(), "hello  ");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes second " "
            assert_eq!(editor.get_text(), "hello ");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes first " "
            assert_eq!(editor.get_text(), "hello");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo) - removes "hello"
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn undoes_newlines_and_signals_next_word_to_capture_state() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input("\n");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "hello\nworld");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello\n");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn undoes_backspace() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input("\x7f"); // Backspace
            assert_eq!(editor.get_text(), "hell");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello");
        }

        #[test]
        fn undoes_forward_delete() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            editor.handle_input("\x1b[C"); // Right arrow
            editor.handle_input("\x1b[3~"); // Delete key
            assert_eq!(editor.get_text(), "hllo");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello");
        }

        #[test]
        fn undoes_ctrl_w_delete_word_backward() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("\x17"); // Ctrl+W
            assert_eq!(editor.get_text(), "hello ");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn undoes_ctrl_k_delete_to_line_end() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            } // Move right 6 times

            editor.handle_input("\x0b"); // Ctrl+K
            assert_eq!(editor.get_text(), "hello ");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("|");
            assert_eq!(editor.get_text(), "hello |world");
        }

        #[test]
        fn undoes_ctrl_u_delete_to_line_start() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            } // Move right 6 times

            editor.handle_input("\x15"); // Ctrl+U
            assert_eq!(editor.get_text(), "world");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn undoes_yank() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("\x17"); // Ctrl+W - delete "hello "
            editor.handle_input("\x19"); // Ctrl+Y - yank
            assert_eq!(editor.get_text(), "hello ");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn undoes_single_line_paste_atomically() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            } // Move right 5 (after "hello", before space)

            // Simulate bracketed paste of "beep boop"
            editor.handle_input("\x1b[200~beep boop\x1b[201~");
            assert_eq!(editor.get_text(), "hellobeep boop world");

            // Single undo should restore entire pre-paste state
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("|");
            assert_eq!(editor.get_text(), "hello| world");
        }

        #[test]
        fn does_not_trigger_autocomplete_during_single_line_paste() {
            let mut editor = ed();
            let suggestionCalls = provide(&mut editor, |_, _, _, _| None);
            editor.handle_input("\x1b[200~look at @node_modules/react/index.js please\x1b[201~");

            assert_eq!(
                editor.get_text(),
                "look at @node_modules/react/index.js please"
            );
            assert_eq!(*suggestionCalls.borrow(), 0);
            assert!(!editor.is_showing_autocomplete());
        }

        #[test]
        fn decodes_csi_u_ctrl_letter_sequences_inside_bracketed_paste_tmux_popup() {
            let mut editor = ed();

            // tmux popups with extended-keys-format=csi-u re-encode \n in pastes as
            // \x1b[106;5u (Ctrl+J). Without decoding, the per-char filter strips ESC
            // and leaks "[106;5u" between lines. See issue #3599.
            editor.handle_input("\x1b[200~line1\x1b[106;5uline2\x1b[106;5uline3\x1b[201~");
            assert_eq!(editor.get_text(), "line1\nline2\nline3");
        }

        #[test]
        fn undoes_multi_line_paste_atomically() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            } // Move right 5 (after "hello", before space)

            // Simulate bracketed paste of multi-line text
            editor.handle_input("\x1b[200~line1\nline2\nline3\x1b[201~");
            assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

            // Single undo should restore entire pre-paste state
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("|");
            assert_eq!(editor.get_text(), "hello| world");
        }

        #[test]
        fn undoes_inserttextatcursor_atomically() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            } // Move right 5 (after "hello", before space)

            // Programmatic insertion (e.g., clipboard image path)
            editor.insert_text_at_cursor("/tmp/image.png");
            assert_eq!(editor.get_text(), "hello/tmp/image.png world");

            // Single undo should restore entire pre-insert state
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("|");
            assert_eq!(editor.get_text(), "hello| world");
        }

        #[test]
        fn inserttextatcursor_handles_multiline_text() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            } // Move right 5 (after "hello", before space)

            // Insert multiline text
            editor.insert_text_at_cursor("line1\nline2\nline3");
            assert_eq!(editor.get_text(), "helloline1\nline2\nline3 world");

            // Cursor should be at end of inserted text (after "line3", before " world")
            let cursor = editor.get_cursor();
            assert_eq!(cursor.0, 2);
            assert_eq!(cursor.1, 5); // "line3".len()

            // Single undo should restore entire pre-insert state
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn inserttextatcursor_normalizes_crlf_and_cr_line_endings() {
            let mut editor = ed();

            editor.set_text("");

            // Insert text with CRLF
            editor.insert_text_at_cursor("a\r\nb\r\nc");
            assert_eq!(editor.get_text(), "a\nb\nc");

            editor.handle_input("\x1b[45;5u"); // Undo
            assert_eq!(editor.get_text(), "");

            // Insert text with CR only
            editor.insert_text_at_cursor("x\ry\rz");
            assert_eq!(editor.get_text(), "x\ny\nz");
        }

        #[test]
        fn undoes_settext_to_empty_string() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "hello world");

            editor.set_text("");
            assert_eq!(editor.get_text(), "");

            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn clears_undo_stack_on_submit() {
            let mut editor = ed();
            let submitted = capture_submit(&mut editor);

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input("\r"); // Enter - submit

            assert_eq!(submitted.borrow().last().unwrap(), "hello");
            assert_eq!(editor.get_text(), "");

            // Undo should do nothing - stack was cleared
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");
        }

        #[test]
        fn exits_history_browsing_mode_on_undo() {
            let mut editor = ed();

            // Add "hello" to history
            editor.add_to_history("hello");
            assert_eq!(editor.get_text(), "");

            // Type "world"
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "world");

            // Ctrl+W - delete word
            editor.handle_input("\x17"); // Ctrl+W
            assert_eq!(editor.get_text(), "");

            // Press Up - enter history browsing, shows "hello"
            editor.handle_input("\x1b[A"); // Up arrow
            assert_eq!(editor.get_text(), "hello");

            // Undo should restore to "" (state before entering history browsing)
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");

            // Undo again should restore to "world" (state before Ctrl+W)
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "world");
        }

        #[test]
        fn undo_restores_to_pre_history_state_even_after_multiple_history_navigations() {
            let mut editor = ed();

            // Add history entries
            editor.add_to_history("first");
            editor.add_to_history("second");
            editor.add_to_history("third");

            // Type something
            editor.handle_input("c");
            editor.handle_input("u");
            editor.handle_input("r");
            editor.handle_input("r");
            editor.handle_input("e");
            editor.handle_input("n");
            editor.handle_input("t");
            assert_eq!(editor.get_text(), "current");

            // Clear editor
            editor.handle_input("\x17"); // Ctrl+W
            assert_eq!(editor.get_text(), "");

            // Navigate through history multiple times
            editor.handle_input("\x1b[A"); // Up - "third"
            assert_eq!(editor.get_text(), "third");
            editor.handle_input("\x1b[A"); // Up - "second"
            assert_eq!(editor.get_text(), "second");
            editor.handle_input("\x1b[A"); // Up - "first"
            assert_eq!(editor.get_text(), "first");

            // Undo should go back to "" (state before we started browsing), not intermediate states
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "");

            // Another undo goes back to "current"
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "current");
        }

        #[test]
        fn cursor_movement_starts_new_undo_unit() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input(" ");
            editor.handle_input("w");
            editor.handle_input("o");
            editor.handle_input("r");
            editor.handle_input("l");
            editor.handle_input("d");
            assert_eq!(editor.get_text(), "hello world");

            // Move cursor left 5 (to after "hello ")
            for _ in 0..5 {
                editor.handle_input("\x1b[D");
            }

            // Type "lol" in the middle
            editor.handle_input("l");
            editor.handle_input("o");
            editor.handle_input("l");
            assert_eq!(editor.get_text(), "hello lolworld");

            // Undo should restore to "hello world" (before inserting "lol")
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello world");

            editor.handle_input("|");
            assert_eq!(editor.get_text(), "hello |world");
        }

        #[test]
        fn no_op_delete_operations_do_not_push_undo_snapshots() {
            let mut editor = ed();

            editor.handle_input("h");
            editor.handle_input("e");
            editor.handle_input("l");
            editor.handle_input("l");
            editor.handle_input("o");
            assert_eq!(editor.get_text(), "hello");

            // Delete word on empty - multiple times (should be no-ops)
            editor.handle_input("\x17"); // Ctrl+W - deletes "hello"
            assert_eq!(editor.get_text(), "");
            editor.handle_input("\x17"); // Ctrl+W - no-op (nothing to delete)
            editor.handle_input("\x17"); // Ctrl+W - no-op

            // Single undo should restore "hello"
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "hello");
        }

        #[test]
        fn undoes_autocomplete() {
            let mut editor = ed();
            provide(&mut editor, |lines, _l, col, _force| {
                if before(lines, col) == "di" {
                    return suggestions(vec![item("dist/", "dist/")], "di");
                }
                None
            });
            type_str(&mut editor, "di");
            assert_eq!(editor.get_text(), "di");
            // Tab triggers autocomplete; a single forced suggestion applies.
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "dist/");
            assert!(!(editor.is_showing_autocomplete()));
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "di");
        }
    }

    mod autocomplete {
        use super::*;

        #[test]
        fn auto_applies_single_force_file_suggestion_without_showing_menu() {
            let mut editor = ed();
            provide(&mut editor, |lines, _l, col, force| {
                if !force {
                    return None;
                }
                if before(lines, col) == "Work" {
                    return suggestions(vec![item("Workspace/", "Workspace/")], "Work");
                }
                None
            });
            type_str(&mut editor, "Work");
            assert_eq!(editor.get_text(), "Work");
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "Workspace/");
            assert!(!(editor.is_showing_autocomplete()));
            editor.handle_input("\x1b[45;5u");
            assert_eq!(editor.get_text(), "Work");
        }

        #[test]
        fn shows_menu_when_force_file_has_multiple_suggestions() {
            let mut editor = ed();
            provide(&mut editor, |lines, _l, col, force| {
                if !force {
                    return None;
                }
                if before(lines, col) == "src" {
                    return suggestions(
                        vec![item("src/", "src/"), item("src.txt", "src.txt")],
                        "src",
                    );
                }
                None
            });
            type_str(&mut editor, "src");
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "src");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "src/");
            assert!(!(editor.is_showing_autocomplete()));
        }

        #[test]
        fn keeps_suggestions_open_when_typing_in_force_mode_tab_triggered() {
            let mut editor = ed();
            let all = ["readme.md", "package.json", "src/", "dist/"];
            provide(&mut editor, move |lines, _l, col, force| {
                let prefix = before(lines, col);
                if !(force || prefix.contains('/') || prefix.starts_with('.')) {
                    return None;
                }
                let filtered: Vec<_> = all
                    .iter()
                    .filter(|f| f.to_lowercase().starts_with(&prefix.to_lowercase()))
                    .map(|f| item(f, f))
                    .collect();
                (!filtered.is_empty())
                    .then(|| suggestions(filtered, &prefix))
                    .flatten()
            });
            editor.handle_input("\t");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("r");
            assert_eq!(editor.get_text(), "r");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("e");
            assert_eq!(editor.get_text(), "re");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "readme.md");
            assert!(!(editor.is_showing_autocomplete()));
        }

        #[test]
        fn debounces_autocomplete_while_typing() {
            let mut editor = ed();
            let calls = provide(&mut editor, |lines, _l, col, _f| {
                suggestions(vec![item("@main.ts", "main.ts")], &before(lines, col))
            });
            type_str(&mut editor, "@mai");
            assert_eq!(*calls.borrow(), 0);
            assert!(!(editor.is_showing_autocomplete()));
            flush_debounce(&mut editor);
            assert_eq!(*calls.borrow(), 1);
            assert!(editor.is_showing_autocomplete());
        }

        #[test]
        fn debounces_autocomplete_while_typing_2() {
            let mut editor = ed();
            let calls = provide(&mut editor, |lines, _l, col, _f| {
                suggestions(vec![item("#2983", "#2983")], &before(lines, col))
            });
            type_str(&mut editor, "#298");
            assert_eq!(*calls.borrow(), 0);
            assert!(!(editor.is_showing_autocomplete()));
            flush_debounce(&mut editor);
            assert_eq!(*calls.borrow(), 1);
            assert!(editor.is_showing_autocomplete());
        }

        // "aborts active autocomplete when typing continues" has no Rust
        // counterpart: providers are synchronous, so there is never an
        // in-flight request to abort (see the editor module docs).

        #[test]
        fn hides_autocomplete_when_backspacing_slash_command_to_empty() {
            let mut editor = ed();
            provide(&mut editor, |lines, _l, col, _f| {
                let prefix = before(lines, col);
                if let Some(query) = prefix.strip_prefix('/') {
                    let commands = [
                        ("/model", "model", "Change model"),
                        ("/help", "help", "Show help"),
                    ];
                    let filtered: Vec<_> = commands
                        .iter()
                        .filter(|c| c.0.starts_with(query))
                        .map(|c| AutocompleteItem {
                            value: c.0.into(),
                            label: c.1.into(),
                            description: Some(c.2.into()),
                        })
                        .collect();
                    if !filtered.is_empty() {
                        return suggestions(filtered, &prefix);
                    }
                }
                None
            });
            editor.handle_input("/");
            assert_eq!(editor.get_text(), "/");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\x7f");
            assert_eq!(editor.get_text(), "");
            assert!(!(editor.is_showing_autocomplete()));
        }

        fn argtest_provider(
            editor: &mut Editor,
            values: &'static [&'static str],
            filter: bool,
            command: &'static str,
        ) {
            provide(editor, move |lines, _l, col, _f| {
                let arg = arg_context(&before(lines, col), command)?;
                let items: Vec<_> = values
                    .iter()
                    .filter(|v| !filter || v.starts_with(&arg))
                    .map(|v| item(v, v))
                    .collect();
                (!items.is_empty())
                    .then(|| suggestions(items, &arg))
                    .flatten()
            });
        }

        #[test]
        fn applies_exact_typed_slash_argument_value_on_enter_even_when_first_item_is_highli() {
            let mut editor = ed();
            argtest_provider(&mut editor, &["one", "two", "three"], true, "/argtest");
            type_str(&mut editor, "/argtest two");
            assert_eq!(editor.get_text(), "/argtest two");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\r");
            assert_eq!(editor.get_text(), "/argtest two");
        }

        #[test]
        fn selects_first_prefix_match_on_enter_when_typed_arg_is_not_exact_match() {
            let mut editor = ed();
            argtest_provider(&mut editor, &["two", "three", "twelve"], true, "/argtest");
            type_str(&mut editor, "/argtest t");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\r");
            assert_eq!(editor.get_text(), "/argtest two");
        }

        #[test]
        fn highlights_unique_prefix_match_as_user_types_before_full_exact_match() {
            let mut editor = ed();
            argtest_provider(&mut editor, &["one", "two", "three"], false, "/argtest");
            type_str(&mut editor, "/argtest tw");
            assert_eq!(editor.get_text(), "/argtest tw");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\r");
            assert_eq!(editor.get_text(), "/argtest two");
        }

        #[test]
        fn selects_first_prefix_match_when_multiple_items_match() {
            let mut editor = ed();
            argtest_provider(&mut editor, &["one", "two", "three"], false, "/argtest");
            type_str(&mut editor, "/argtest t");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\r");
            assert_eq!(editor.get_text(), "/argtest two");
        }

        #[test]
        fn works_for_built_in_style_command_argument_completion_path_model_like() {
            let mut editor = ed();
            argtest_provider(
                &mut editor,
                &["gpt-4o", "gpt-4o-mini", "claude-sonnet"],
                true,
                "/model",
            );
            type_str(&mut editor, "/model gpt-4o-mini");
            assert_eq!(editor.get_text(), "/model gpt-4o-mini");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\r");
            assert_eq!(editor.get_text(), "/model gpt-4o-mini");
        }

        fn command(name: &str, completer: Option<ArgumentCompletionsFn>) -> CommandEntry {
            CommandEntry::Slash(SlashCommand {
                name: name.into(),
                description: Some(format!("{name} description")),
                argument_hint: None,
                get_argument_completions: completer,
            })
        }

        #[test]
        fn awaits_async_slash_command_argument_completions() {
            let mut editor = ed();
            let provider = CombinedAutocompleteProvider::new(
                vec![command(
                    "load-skills",
                    Some(Box::new(|prefix: &str| {
                        prefix
                            .starts_with('s')
                            .then(|| vec![item("skill-a", "skill-a")])
                    })),
                )],
                std::env::current_dir().unwrap(),
                None,
            );
            editor.set_autocomplete_provider(Box::new(provider));
            editor.set_text("/load-skills ");
            editor.handle_input("s");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "/load-skills skill-a");
            assert!(!(editor.is_showing_autocomplete()));
        }

        #[test]
        fn ignores_invalid_slash_command_argument_completion_results() {
            // A completer that yields nothing usable (the TS test returns a
            // non-array; Rust's types rule that out, so `None` stands in).
            let mut editor = ed();
            let provider = CombinedAutocompleteProvider::new(
                vec![command("load-skills", Some(Box::new(|_: &str| None)))],
                std::env::current_dir().unwrap(),
                None,
            );
            editor.set_autocomplete_provider(Box::new(provider));
            editor.set_text("/load-skills ");
            editor.handle_input("s");
            assert!(!(editor.is_showing_autocomplete()));
            assert_eq!(editor.get_text(), "/load-skills s");
        }

        #[test]
        fn does_not_show_argument_completions_when_command_has_no_argument_completer() {
            let mut editor = ed();
            let provider = CombinedAutocompleteProvider::new(
                vec![
                    command("help", None),
                    command(
                        "model",
                        Some(Box::new(|_: &str| {
                            Some(vec![item("claude-opus", "claude-opus")])
                        })),
                    ),
                ],
                std::env::current_dir().unwrap(),
                None,
            );
            editor.set_autocomplete_provider(Box::new(provider));
            type_str(&mut editor, "/he");
            assert!(editor.is_showing_autocomplete());
            editor.handle_input("\t");
            assert_eq!(editor.get_text(), "/help ");
            assert!(!(editor.is_showing_autocomplete()));
        }
    }

    mod character_jump_ctrl {
        use super::*;
        #[test]
        fn jumps_forward_to_first_occurrence_of_character_on_same_line() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+] (legacy sequence for ctrl+])
            editor.handle_input("o"); // Jump to first 'o'

            assert_eq!(editor.get_cursor(), (0, 4)); // 'o' in "hello"
        }

        #[test]
        fn jumps_forward_to_next_occurrence_after_cursor() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
                                         // Move cursor to the 'o' in "hello" (col 4)
            for _ in 0..4 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 4));

            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("o"); // Jump to next 'o' (in "world")

            assert_eq!(editor.get_cursor(), (0, 7)); // 'o' in "world"
        }

        #[test]
        fn jumps_forward_across_multiple_lines() {
            let mut editor = ed();

            editor.set_text("abc\ndef\nghi");
            // Cursor is at end (line 2, col 3). Move to line 0 via up arrows, then Ctrl+A
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x1b[A"); // Up - now on line 0
            editor.handle_input("\x01"); // Ctrl+A - go to start of line
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("g"); // Jump to 'g' on line 3

            assert_eq!(editor.get_cursor(), (2, 0));
        }

        #[test]
        fn jumps_backward_to_first_occurrence_before_cursor_on_same_line() {
            let mut editor = ed();

            editor.set_text("hello world");
            // Cursor at end (col 11)
            assert_eq!(editor.get_cursor(), (0, 11));

            editor.handle_input("\x1b\x1d"); // Ctrl+Alt+] (ESC followed by Ctrl+])
            editor.handle_input("o"); // Jump to last 'o' before cursor

            assert_eq!(editor.get_cursor(), (0, 7)); // 'o' in "world"
        }

        #[test]
        fn jumps_backward_across_multiple_lines() {
            let mut editor = ed();

            editor.set_text("abc\ndef\nghi");
            // Cursor at end of line 3
            assert_eq!(editor.get_cursor(), (2, 3));

            editor.handle_input("\x1b\x1d"); // Ctrl+Alt+]
            editor.handle_input("a"); // Jump to 'a' on line 1

            assert_eq!(editor.get_cursor(), (0, 0));
        }

        #[test]
        fn does_nothing_when_character_is_not_found_forward() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("z"); // 'z' doesn't exist

            assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged
        }

        #[test]
        fn does_nothing_when_character_is_not_found_backward() {
            let mut editor = ed();

            editor.set_text("hello world");
            // Cursor at end
            assert_eq!(editor.get_cursor(), (0, 11));

            editor.handle_input("\x1b\x1d"); // Ctrl+Alt+]
            editor.handle_input("z"); // 'z' doesn't exist

            assert_eq!(editor.get_cursor(), (0, 11)); // Cursor unchanged
        }

        #[test]
        fn is_case_sensitive() {
            let mut editor = ed();

            editor.set_text("Hello World");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            // Search for lowercase 'h' - should not find it (only 'H' exists)
            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("h");

            assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged

            // Search for uppercase 'W' - should find it
            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("W");

            assert_eq!(editor.get_cursor(), (0, 6)); // 'W' in "World"
        }

        #[test]
        fn cancels_jump_mode_when_ctrl_is_pressed_again() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+] - enter jump mode
            editor.handle_input("\x1d"); // Ctrl+] again - cancel

            // Type 'o' normally - should insert, not jump
            editor.handle_input("o");
            assert_eq!(editor.get_text(), "ohello world");
        }

        #[test]
        fn cancels_jump_mode_on_escape_and_processes_the_escape() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+] - enter jump mode
            editor.handle_input("\x1b"); // Escape - cancel jump mode

            // Cursor should be unchanged (Escape itself doesn't move cursor in editor)
            assert_eq!(editor.get_cursor(), (0, 0));

            // Type 'o' normally - should insert, not jump
            editor.handle_input("o");
            assert_eq!(editor.get_text(), "ohello world");
        }

        #[test]
        fn cancels_backward_jump_mode_when_ctrl_alt_is_pressed_again() {
            let mut editor = ed();

            editor.set_text("hello world");
            // Cursor at end
            assert_eq!(editor.get_cursor(), (0, 11));

            editor.handle_input("\x1b\x1d"); // Ctrl+Alt+] - enter backward jump mode
            editor.handle_input("\x1b\x1d"); // Ctrl+Alt+] again - cancel

            // Type 'o' normally - should insert, not jump
            editor.handle_input("o");
            assert_eq!(editor.get_text(), "hello worldo");
        }

        #[test]
        fn searches_for_special_characters() {
            let mut editor = ed();

            editor.set_text("foo(bar) = baz;");
            editor.handle_input("\x01"); // Ctrl+A - go to start
            assert_eq!(editor.get_cursor(), (0, 0));

            // Jump to '('
            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("(");

            assert_eq!(editor.get_cursor(), (0, 3));

            // Jump to '='
            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("=");

            assert_eq!(editor.get_cursor(), (0, 9));
        }

        #[test]
        fn handles_empty_text_gracefully() {
            let mut editor = ed();

            editor.set_text("");
            assert_eq!(editor.get_cursor(), (0, 0));

            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("x");

            assert_eq!(editor.get_cursor(), (0, 0)); // Cursor unchanged
        }

        #[test]
        fn resets_lastaction_when_jumping() {
            let mut editor = ed();

            editor.set_text("hello world");
            editor.handle_input("\x01"); // Ctrl+A - go to start

            // Type to set lastAction to "type-word"
            editor.handle_input("x");
            assert_eq!(editor.get_text(), "xhello world");

            // Jump forward
            editor.handle_input("\x1d"); // Ctrl+]
            editor.handle_input("o");

            // Type more - should start a new undo unit (lastAction was reset)
            editor.handle_input("Y");
            assert_eq!(editor.get_text(), "xhellYo world");

            // Undo should only undo "Y", not "x" as well
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "xhello world");
        }
    }

    mod sticky_column {
        use super::*;
        // Helper: position cursor at a specific line and column
        fn position_cursor(editor: &mut Editor, line: usize, col: usize) {
            // Go to line 0 first
            for _ in 0..20 {
                editor.handle_input("\x1b[A");
            }
            // Go to target line
            for _ in 0..line {
                editor.handle_input("\x1b[B");
            }
            // Go to target col
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..col {
                editor.handle_input("\x1b[C");
            }
        }

        #[test]
        fn preserves_target_column_when_moving_up_through_a_shorter_line() {
            let mut editor = ed();

            // Line 0: "2222222222x222" (x at col 10)
            // Line 1: "" (empty)
            // Line 2: "1111111111_111111111111" (_ at col 10)
            editor.set_text("2222222222x222\n\n1111111111_111111111111");

            // Position cursor on _ (line 2, col 10)
            assert_eq!(editor.get_cursor(), (2, 23)); // At end
            editor.handle_input("\x01"); // Ctrl+A - go to start of line
            for _ in 0..10 {
                editor.handle_input("\x1b[C");
            } // Move right to col 10
            assert_eq!(editor.get_cursor(), (2, 10));

            // Press Up - should move to empty line (col clamped to 0)
            editor.handle_input("\x1b[A"); // Up arrow
            assert_eq!(editor.get_cursor(), (1, 0));

            // Press Up again - should move to line 0 at col 10 (on 'x')
            editor.handle_input("\x1b[A"); // Up arrow
            assert_eq!(editor.get_cursor(), (0, 10));
        }

        #[test]
        fn preserves_target_column_when_moving_down_through_a_shorter_line() {
            let mut editor = ed();

            editor.set_text("1111111111_111\n\n2222222222x222222222222");

            // Position cursor on _ (line 0, col 10)
            editor.handle_input("\x1b[A"); // Up to line 1
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..10 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 10));

            // Press Down - should move to empty line (col clamped to 0)
            editor.handle_input("\x1b[B"); // Down arrow
            assert_eq!(editor.get_cursor(), (1, 0));

            // Press Down again - should move to line 2 at col 10 (on 'x')
            editor.handle_input("\x1b[B"); // Down arrow
            assert_eq!(editor.get_cursor(), (2, 10));
        }

        #[test]
        fn resets_sticky_column_on_horizontal_movement_left_arrow() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Start at line 2, col 5
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (2, 5));

            // Move up through empty line
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 5 (sticky)
            assert_eq!(editor.get_cursor(), (0, 5));

            // Move left - resets sticky column
            editor.handle_input("\x1b[D"); // Left
            assert_eq!(editor.get_cursor(), (0, 4));

            // Move down twice
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 4 (new sticky from col 4)
            assert_eq!(editor.get_cursor(), (2, 4));
        }

        #[test]
        fn resets_sticky_column_on_horizontal_movement_right_arrow() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Start at line 0, col 5
            editor.handle_input("\x1b[A"); // Up to line 1
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..5 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 5));

            // Move down through empty line
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 5 (sticky)
            assert_eq!(editor.get_cursor(), (2, 5));

            // Move right - resets sticky column
            editor.handle_input("\x1b[C"); // Right
            assert_eq!(editor.get_cursor(), (2, 6));

            // Move up twice
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 6 (new sticky from col 6)
            assert_eq!(editor.get_cursor(), (0, 6));
        }

        #[test]
        fn resets_sticky_column_on_typing() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Start at line 2, col 8
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..8 {
                editor.handle_input("\x1b[C");
            }

            // Move up through empty line
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x1b[A"); // Up - line 0, col 8
            assert_eq!(editor.get_cursor(), (0, 8));

            // Type a character - resets sticky column
            editor.handle_input("X");
            assert_eq!(editor.get_cursor(), (0, 9));

            // Move down twice
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 9 (new sticky from col 9)
            assert_eq!(editor.get_cursor(), (2, 9));
        }

        #[test]
        fn resets_sticky_column_on_backspace() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Start at line 2, col 8
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..8 {
                editor.handle_input("\x1b[C");
            }

            // Move up through empty line
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x1b[A"); // Up - line 0, col 8
            assert_eq!(editor.get_cursor(), (0, 8));

            // Backspace - resets sticky column
            editor.handle_input("\x7f"); // Backspace
            assert_eq!(editor.get_cursor(), (0, 7));

            // Move down twice
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 7 (new sticky from col 7)
            assert_eq!(editor.get_cursor(), (2, 7));
        }

        #[test]
        fn resets_sticky_column_on_ctrl_a_move_to_line_start() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Start at line 2, col 8
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..8 {
                editor.handle_input("\x1b[C");
            }

            // Move up - establishes sticky col 8
            editor.handle_input("\x1b[A"); // Up - line 1, col 0

            // Ctrl+A - resets sticky column to 0
            editor.handle_input("\x01"); // Ctrl+A
            assert_eq!(editor.get_cursor(), (1, 0));

            // Move up
            editor.handle_input("\x1b[A"); // Up - line 0, col 0 (new sticky from col 0)
            assert_eq!(editor.get_cursor(), (0, 0));
        }

        #[test]
        fn resets_sticky_column_on_ctrl_e_move_to_line_end() {
            let mut editor = ed();

            editor.set_text("12345\n\n1234567890");

            // Start at line 2, col 3
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..3 {
                editor.handle_input("\x1b[C");
            }

            // Move up through empty line - establishes sticky col 3
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 3
            assert_eq!(editor.get_cursor(), (0, 3));

            // Ctrl+E - resets sticky column to end
            editor.handle_input("\x05"); // Ctrl+E
            assert_eq!(editor.get_cursor(), (0, 5));

            // Move down twice
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 5 (new sticky from col 5)
            assert_eq!(editor.get_cursor(), (2, 5));
        }

        #[test]
        fn resets_sticky_column_on_word_movement_ctrl_left() {
            let mut editor = ed();

            editor.set_text("hello world\n\nhello world");

            // Start at end of line 2 (col 11)
            assert_eq!(editor.get_cursor(), (2, 11));

            // Move up through empty line - establishes sticky col 11
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 11
            assert_eq!(editor.get_cursor(), (0, 11));

            // Ctrl+Left - word movement resets sticky column
            editor.handle_input("\x1b[1;5D"); // Ctrl+Left
            assert_eq!(editor.get_cursor(), (0, 6)); // Before "world"

            // Move down twice
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 6 (new sticky from col 6)
            assert_eq!(editor.get_cursor(), (2, 6));
        }

        #[test]
        fn resets_sticky_column_on_word_movement_ctrl_right() {
            let mut editor = ed();

            editor.set_text("hello world\n\nhello world");

            // Start at line 0, col 0
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x01"); // Ctrl+A
            assert_eq!(editor.get_cursor(), (0, 0));

            // Move down through empty line - establishes sticky col 0
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 0
            assert_eq!(editor.get_cursor(), (2, 0));

            // Ctrl+Right - word movement resets sticky column
            editor.handle_input("\x1b[1;5C"); // Ctrl+Right
            assert_eq!(editor.get_cursor(), (2, 5)); // After "hello"

            // Move up twice
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 5 (new sticky from col 5)
            assert_eq!(editor.get_cursor(), (0, 5));
        }

        #[test]
        fn resets_sticky_column_on_undo() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Go to line 0, col 8
            editor.handle_input("\x1b[A"); // Up to line 1
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..8 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 8));

            // Move down through empty line - establishes sticky col 8
            editor.handle_input("\x1b[B"); // Down - line 1, col 0
            editor.handle_input("\x1b[B"); // Down - line 2, col 8 (sticky)
            assert_eq!(editor.get_cursor(), (2, 8));

            // Type something to create undo state - this clears sticky and sets col to 9
            editor.handle_input("X");
            assert_eq!(editor.get_text(), "1234567890\n\n12345678X90");
            assert_eq!(editor.get_cursor(), (2, 9));

            // Move up - establishes new sticky col 9
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 9
            assert_eq!(editor.get_cursor(), (0, 9));

            // Undo - resets sticky column and restores cursor to line 2, col 8
            editor.handle_input("\x1b[45;5u"); // Ctrl+- (undo)
            assert_eq!(editor.get_text(), "1234567890\n\n1234567890");
            assert_eq!(editor.get_cursor(), (2, 8));

            // Move up - should capture new sticky from restored col 8, not old col 9
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 8 (new sticky from restored position)
            assert_eq!(editor.get_cursor(), (0, 8));
        }

        #[test]
        fn handles_multiple_consecutive_up_down_movements() {
            let mut editor = ed();

            editor.set_text("1234567890\nab\ncd\nef\n1234567890");

            // Start at line 4, col 7
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..7 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (4, 7));

            // Move up multiple times through short lines
            editor.handle_input("\x1b[A"); // Up - line 3, col 2 (clamped)
            editor.handle_input("\x1b[A"); // Up - line 2, col 2 (clamped)
            editor.handle_input("\x1b[A"); // Up - line 1, col 2 (clamped)
            editor.handle_input("\x1b[A"); // Up - line 0, col 7 (restored)
            assert_eq!(editor.get_cursor(), (0, 7));

            // Move down multiple times - sticky should still be 7
            editor.handle_input("\x1b[B"); // Down - line 1, col 2
            editor.handle_input("\x1b[B"); // Down - line 2, col 2
            editor.handle_input("\x1b[B"); // Down - line 3, col 2
            editor.handle_input("\x1b[B"); // Down - line 4, col 7 (restored)
            assert_eq!(editor.get_cursor(), (4, 7));
        }

        #[test]
        fn moves_correctly_through_wrapped_visual_lines_without_getting_stuck() {
            let mut editor = ed_with(15, 24, 0, None); // Narrow terminal

            // Line 0: short
            // Line 1: 30 chars = wraps to 3 visual lines at width 10 (after padding)
            editor.set_text("short\n123456789012345678901234567890");
            render(&mut editor, 15); // This gives 14 layout width

            // Position at end of line 1 (col 30)
            assert_eq!(editor.get_cursor(), (1, 30));

            // Move up repeatedly - should traverse all visual lines of the wrapped text
            // and eventually reach line 0
            editor.handle_input("\x1b[A"); // Up - to previous visual line within line 1
            assert_eq!(editor.get_cursor().0, 1);

            editor.handle_input("\x1b[A"); // Up - another visual line
            assert_eq!(editor.get_cursor().0, 1);

            editor.handle_input("\x1b[A"); // Up - should reach line 0
            assert_eq!(editor.get_cursor().0, 0);
        }

        #[test]
        fn handles_settext_resetting_sticky_column() {
            let mut editor = ed();

            editor.set_text("1234567890\n\n1234567890");

            // Establish sticky column
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..8 {
                editor.handle_input("\x1b[C");
            }
            editor.handle_input("\x1b[A"); // Up

            // setText should reset sticky column
            editor.set_text("abcdefghij\n\nabcdefghij");
            assert_eq!(editor.get_cursor(), (2, 10)); // At end

            // Move up - should capture new sticky from current position (10)
            editor.handle_input("\x1b[A"); // Up - line 1, col 0
            editor.handle_input("\x1b[A"); // Up - line 0, col 10
            assert_eq!(editor.get_cursor(), (0, 10));
        }

        #[test]
        fn sets_preferredvisualcol_when_pressing_right_at_end_of_prompt_last_line() {
            let mut editor = ed();

            // Line 0: 20 chars with 'x' at col 10
            // Line 1: empty
            // Line 2: 10 chars ending with '_'
            editor.set_text("111111111x1111111111\n\n333333333_");

            // Go to line 0, press Ctrl+E (end of line) - col 20
            editor.handle_input("\x1b[A"); // Up to line 1
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x05"); // Ctrl+E - move to end of line
            assert_eq!(editor.get_cursor(), (0, 20));

            // Move down to line 2 - cursor clamped to col 10 (end of line)
            editor.handle_input("\x1b[B"); // Down to line 1, col 0
            editor.handle_input("\x1b[B"); // Down to line 2, col 10 (clamped)
            assert_eq!(editor.get_cursor(), (2, 10));

            // Press Right at end of prompt - nothing visible happens, but sets preferredVisualCol to 10
            editor.handle_input("\x1b[C"); // Right - can't move, but sets preferredVisualCol
            assert_eq!(editor.get_cursor(), (2, 10)); // Still at same position

            // Move up twice to line 0 - should use preferredVisualCol (10) to land on 'x'
            editor.handle_input("\x1b[A"); // Up to line 1, col 0
            editor.handle_input("\x1b[A"); // Up to line 0, col 10 (on 'x')
            assert_eq!(editor.get_cursor(), (0, 10));
        }

        #[test]
        fn handles_editor_resizes_when_preferredvisualcol_is_on_the_same_line() {
            // Create editor with wider terminal
            let mut editor = ed_with(80, 24, 0, None);

            editor.set_text("12345678901234567890\n\n12345678901234567890");

            // Start at line 2, col 15
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..15 {
                editor.handle_input("\x1b[C");
            }

            // Move up through empty line - establishes sticky col 15
            editor.handle_input("\x1b[A"); // Up
            editor.handle_input("\x1b[A"); // Up - line 0, col 15
            assert_eq!(editor.get_cursor(), (0, 15));

            // Render with narrower width to simulate resize
            render(&mut editor, 12); // Width 12

            // Move down - sticky should be clamped to new width
            editor.handle_input("\x1b[B"); // Down - line 1
            editor.handle_input("\x1b[B"); // Down - line 2, col should be clamped
            assert_eq!(editor.get_cursor().1, 4);
        }

        #[test]
        fn handles_editor_resizes_when_preferredvisualcol_is_on_a_different_line() {
            let mut editor = ed_with(80, 24, 0, None);

            // Create a line that wraps into multiple visual lines at width 10
            // "12345678901234567890" = 20 chars, wraps to 2 visual lines at width 10
            editor.set_text("short\n12345678901234567890");

            // Go to line 1, col 15
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..15 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (1, 15));

            // Move up to establish sticky col 15
            editor.handle_input("\x1b[A"); // Up to line 0
                                           // Line 0 has only 5 chars, so cursor at col 5
            assert_eq!(editor.get_cursor(), (0, 5));

            // Narrow the editor
            render(&mut editor, 10);

            // Move down - preferredVisualCol was 15, but width is 10
            // Should land on line 1, clamped to width (visual col 9, which is logical col 9)
            editor.handle_input("\x1b[B"); // Down to line 1
            assert_eq!(editor.get_cursor(), (1, 8));

            // Move up
            editor.handle_input("\x1b[A"); // Up - should go to line 0
            assert_eq!(editor.get_cursor(), (0, 5)); // Line 0 only has 5 chars

            // Restore the original width
            render(&mut editor, 80);

            // Move down - preferredVisualCol was kept at 15
            editor.handle_input("\x1b[B"); // Down to line 1
            assert_eq!(editor.get_cursor(), (1, 15));
        }

        #[test]
        fn rewrapped_lines_target_fits_current_visual_column() {
            let mut editor = ed_with(80, 24, 0, None);
            editor.set_text("abcdefghijklmnopqr\n123456789012345678");

            position_cursor(&mut editor, 0, 18);
            assert_eq!(editor.get_cursor(), (0, 18));

            // Narrow to width 10 (layoutWidth = 9).
            // Line 0 last segment has visual col max 9, line 1 first segment max 8
            render(&mut editor, 10);

            // Move down: cursor clamps to 8
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 8));

            // Widen back. Move up, the current visual col wins
            render(&mut editor, 80);
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor(), (0, 8));

            // Preferred was cleared by the rewrapped branch
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 8));
        }

        #[test]
        fn rewrapped_lines_target_shorter_than_current_visual_column() {
            let mut editor = ed_with(80, 24, 0, None);
            editor.set_text("abcdefghijklmnopqr\n123456789012345678\nab");

            position_cursor(&mut editor, 0, 18);
            assert_eq!(editor.get_cursor(), (0, 18));

            // Narrow to width 10 (layoutWidth = 9). Moving down clamps to col 8
            render(&mut editor, 10);
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 8));

            // Widen the editor
            render(&mut editor, 80);

            // Move down to short line "ab".
            // preferredVisualCol is replaced with current visual col (8), cursor clamps to 2
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (2, 2));

            // Moving up restores to preferred col 8
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor(), (1, 8));
        }
    }

    mod paste_marker_atomic_behavior {
        use super::*;
        /// Simulate a large paste that creates a marker.
        fn paste_with_marker(editor: &mut Editor) -> String {
            let big_content = "line\n".repeat(20);
            let big_content = big_content.trim_end(); // 20 lines
            editor.handle_input(&format!("\x1b[200~{big_content}\x1b[201~"));
            // The editor replaces large pastes with a marker like "[paste #1 +20 lines]"
            editor.get_text()
        }

        /// The first `[paste #N +L lines]` marker in `text`.
        fn marker_in(text: &str) -> String {
            let (s, e, _) = find_paste_markers(text)[0];
            text[s..e].to_string()
        }

        #[test]
        fn creates_a_paste_marker_for_large_pastes() {
            let mut editor = ed();
            let text = paste_with_marker(&mut editor);
            assert!(!find_paste_markers(&text).is_empty(), "{text}");
        }

        #[test]
        fn treats_paste_marker_as_single_unit_for_right_arrow() {
            let mut editor = ed();
            editor.handle_input("A");
            paste_with_marker(&mut editor);
            editor.handle_input("B");
            // Text: "A[paste #1 +20 lines]B", cursor at end

            // Go to start
            editor.handle_input("\x01"); // Ctrl+A
            assert_eq!(editor.get_cursor(), (0, 0));

            // Right arrow: should move past "A"
            editor.handle_input("\x1b[C");
            assert_eq!(editor.get_cursor(), (0, 1));

            // Right arrow: should skip the entire marker
            editor.handle_input("\x1b[C");
            let marker = marker_in(&editor.get_text());
            assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

            // Right arrow: should move past "B"
            editor.handle_input("\x1b[C");
            assert_eq!(editor.get_cursor(), (0, 1 + marker.len() + 1));
        }

        #[test]
        fn treats_paste_marker_as_single_unit_for_left_arrow() {
            let mut editor = ed();
            editor.handle_input("A");
            paste_with_marker(&mut editor);
            editor.handle_input("B");
            // Cursor at end

            // Left arrow: past "B"
            editor.handle_input("\x1b[D");
            let text = editor.get_text();
            let marker = marker_in(&text);
            assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

            // Left arrow: skip the entire marker
            editor.handle_input("\x1b[D");
            assert_eq!(editor.get_cursor(), (0, 1));

            // Left arrow: past "A"
            editor.handle_input("\x1b[D");
            assert_eq!(editor.get_cursor(), (0, 0));
        }

        #[test]
        fn treats_paste_marker_as_single_unit_for_backspace() {
            let mut editor = ed();
            editor.handle_input("A");
            paste_with_marker(&mut editor);
            editor.handle_input("B");

            let text = editor.get_text();
            let marker = marker_in(&text);

            // Position cursor right after the marker (before "B")
            editor.handle_input("\x01"); // Ctrl+A
                                         // Move past "A" and the marker
            editor.handle_input("\x1b[C"); // past "A"
            editor.handle_input("\x1b[C"); // past marker
            assert_eq!(editor.get_cursor(), (0, 1 + marker.len()));

            // Backspace: should delete the entire marker at once
            editor.handle_input("\x7f");
            assert_eq!(editor.get_text(), "AB");
            assert_eq!(editor.get_cursor(), (0, 1));
        }

        #[test]
        fn treats_paste_marker_as_single_unit_for_forward_delete() {
            let mut editor = ed();
            editor.handle_input("A");
            paste_with_marker(&mut editor);
            editor.handle_input("B");

            // Position cursor on "A" (col 0) then move right once to be just before marker
            editor.handle_input("\x01"); // Ctrl+A
            editor.handle_input("\x1b[C"); // past "A", now at col 1 (start of marker)

            // Forward delete: should delete the entire marker at once
            editor.handle_input("\x1b[3~"); // Delete key
            assert_eq!(editor.get_text(), "AB");
            assert_eq!(editor.get_cursor(), (0, 1));
        }

        #[test]
        fn treats_paste_marker_as_single_unit_for_word_movement() {
            let mut editor = ed();
            editor.handle_input("X");
            editor.handle_input(" ");
            paste_with_marker(&mut editor);
            editor.handle_input(" ");
            editor.handle_input("Y");
            // Text: "X [paste #1 +20 lines] Y"

            let text = editor.get_text();
            let marker = marker_in(&text);

            // Go to start
            editor.handle_input("\x01"); // Ctrl+A

            // Ctrl+Right: skip "X"
            editor.handle_input("\x1b[1;5C");
            assert_eq!(editor.get_cursor(), (0, 1));

            // Ctrl+Right: skip whitespace + marker (marker treated as single non-ws, non-punct unit)
            editor.handle_input("\x1b[1;5C");
            assert_eq!(editor.get_cursor(), (0, 2 + marker.len()));
        }

        #[test]
        fn undo_restores_marker_after_backspace_deletion() {
            let mut editor = ed();
            editor.handle_input("A");
            paste_with_marker(&mut editor);
            editor.handle_input("B");

            let textBefore = editor.get_text();

            // Position after marker
            editor.handle_input("\x01");
            editor.handle_input("\x1b[C"); // past A
            editor.handle_input("\x1b[C"); // past marker

            // Delete marker
            editor.handle_input("\x7f");
            assert_eq!(editor.get_text(), "AB");

            // Undo
            editor.handle_input("\x1b[45;5u");
            assert_eq!(editor.get_text(), textBefore);
        }

        #[test]
        fn handles_multiple_paste_markers_in_same_line() {
            let mut editor = ed();
            paste_with_marker(&mut editor);
            editor.handle_input(" ");
            paste_with_marker(&mut editor);

            let text = editor.get_text();
            let markers = find_paste_markers(&text);
            assert_eq!(markers.len(), 2);

            // Go to start
            editor.handle_input("\x01");

            // Right arrow: should skip first marker atomically
            editor.handle_input("\x1b[C");
            assert_eq!(editor.get_cursor(), (0, (markers[0].1 - markers[0].0)));

            // Right arrow: past space
            editor.handle_input("\x1b[C");
            assert_eq!(editor.get_cursor(), (0, (markers[0].1 - markers[0].0) + 1));

            // Right arrow: should skip second marker atomically
            editor.handle_input("\x1b[C");
            assert_eq!(
                editor.get_cursor(),
                (
                    0,
                    (markers[0].1 - markers[0].0) + 1 + (markers[1].1 - markers[1].0)
                )
            );
        }

        #[test]
        fn does_not_treat_manually_typed_marker_like_text_as_atomic_no_valid_paste_id() {
            let mut editor = ed();
            // Type text that matches the pattern but was typed manually (no paste entry)
            let fakeMarker = "[paste #99 +5 lines]";
            for ch in fakeMarker.chars() {
                editor.handle_input(&ch.to_string());
            }

            assert_eq!(editor.get_text(), fakeMarker);

            // No paste with ID 99 exists, so the marker is NOT treated atomically.
            // Right arrow should move one grapheme at a time.
            editor.handle_input("\x01"); // Ctrl+A
            editor.handle_input("\x1b[C"); // Right
            assert_eq!(editor.get_cursor(), (0, 1)); // Just past "["
        }

        #[test]
        fn does_not_crash_when_paste_marker_is_wider_than_terminal_width() {
            // Reproduce: terminal width 8, paste marker "[paste #1 +47 lines]" (21 chars)
            let mut editor = ed();
            let bigContent = "line\n".repeat(47).trim_end().to_string();
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));

            let text = editor.get_text();
            let marker = find_paste_markers(&text);
            assert!(!marker.is_empty(), "paste marker should be created");
            let marker = &text[marker[0].0..marker[0].1];
            assert!(
                visible_width(marker) > 8,
                "marker should be wider than render width"
            );

            // Render at very narrow width - should not throw
            let lines = render(&mut editor, 8);
            // Every rendered line must fit within the width (marker is split)
            for line in lines {
                assert!(
                    visible_width(&line) <= 8,
                    "line exceeds width 8: visible={} text={}",
                    visible_width(&line),
                    dbg(&line),
                );
            }
        }

        #[test]
        fn does_not_crash_when_text_paste_marker_exceeds_terminal_width_with_cursor_on_mark() {
            // Reproduce: terminal width 54, text "b".repeat(35) + "[paste #1 +27 lines]" + "bbbb"
            // Cursor lands on the paste marker after word-wrap, causing the rendered line
            // to be 55 visible chars (1 over the width).
            let mut editor = ed();

            // Type 35 'b' characters
            for _ in 0..35 {
                editor.handle_input("b");
            }

            // Paste 27 lines
            let bigContent = "line\n".repeat(27).trim_end().to_string();
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));

            // Type a few more characters
            for _ in 0..4 {
                editor.handle_input("b");
            }

            // Move cursor left to land on the paste marker
            editor.handle_input("\x1b[D"); // past last 'b'
            editor.handle_input("\x1b[D"); // past last 'b'
            editor.handle_input("\x1b[D"); // past last 'b'
            editor.handle_input("\x1b[D"); // past last 'b'
            editor.handle_input("\x1b[D"); // now on the paste marker

            // Render at width 54 - should not throw
            let renderWidth = 54;
            let lines = render(&mut editor, renderWidth);
            for line in lines {
                assert!(
                    visible_width(&line) <= renderWidth,
                    "line exceeds width {}: visible={} text={}",
                    renderWidth,
                    visible_width(&line),
                    dbg(&line),
                );
            }
        }

        #[test]
        fn wordwrapline_re_checks_overflow_after_backtracking_to_wrap_opportunity() {
            // Reproduce crash #2: " " + "b".repeat(35) + atomic_marker(20 chars) + "bbbb"
            // layoutWidth=53. After wrapping at the space, the remaining 35 b's + marker = 55
            // must trigger a second force-break instead of silently overflowing.
            let mut editor = ed();

            // Type a space, then 35 b's
            editor.handle_input(" ");
            for _ in 0..35 {
                editor.handle_input("b");
            }

            // Paste 27 lines to create marker
            let bigContent = "line\n".repeat(27).trim_end().to_string();
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));

            // Type trailing chars
            for _ in 0..4 {
                editor.handle_input("b");
            }

            // Render at width 54 (contentWidth=54, layoutWidth=53 with paddingX=0)
            let renderWidth = 54;
            let lines = render(&mut editor, renderWidth);
            for line in lines {
                assert!(
                    visible_width(&line) <= renderWidth,
                    "line exceeds width {}: visible={} text={}",
                    renderWidth,
                    visible_width(&line),
                    dbg(&line),
                );
            }
        }

        #[test]
        fn expands_large_pasted_content_literally_in_getexpandedtext() {
            let mut editor = ed();
            let pastedText = [
                "line 1",
                "line 2",
                "line 3",
                "line 4",
                "line 5",
                "line 6",
                "line 7",
                "line 8",
                "line 9",
                "line 10",
                "tokens $1 $2 $& $$ $` $' end",
            ]
            .join("\n");

            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", pastedText));

            assert!(!find_paste_markers(&editor.get_text()).is_empty());
            assert_eq!(editor.get_expanded_text(), pastedText);
        }

        #[test]
        fn snaps_to_the_paste_marker_start_when_navigating_down_into_it() {
            let mut editor = ed();

            // Line 0: long enough text to establish a sticky column
            editor.set_text("12345678901234567890\n\nhello ");

            // Create a large paste to get a marker
            let bigContent = "x".repeat(2000);
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));
            render(&mut editor, 80);

            let text = editor.get_text();
            let _marker = marker_in(&text);
            // Line 0: "12345678901234567890"
            // Line 1: "" (empty)
            // Line 2: "hello [paste #1 2000 chars]"
            //         marker starts at col 6

            // Navigate to line 0, col 10
            editor.handle_input("\x1b[A"); // Up to line 1
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A (start of line)
            for _ in 0..10 {
                editor.handle_input("\x1b[C");
            } // Right 10
            assert_eq!(editor.get_cursor(), (0, 10));

            // Down to empty line
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 0));

            // Down to paste marker line - sticky col 10 falls inside marker (starts at col 6).
            // Cursor should snap to start of marker (col 6), not end (col 6 + marker.len()).
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (2, 6));
        }

        #[test]
        fn preserves_sticky_column_when_navigating_through_paste_marker_line() {
            let mut editor = ed_with(30, 24, 0, None);

            // Build:
            // Line 0: "1234567890123456" (16 chars)
            // Line 1: "" (empty)
            // Line 2: "[paste #1 2000 chars]" (22 chars, paste marker)
            // Line 3: "" (empty)
            // Line 4: "abcdefghijklmnop" (16 chars)
            for ch in "1234567890123456".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            editor.handle_input("\n");
            editor.handle_input("\n");
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", "x".repeat(2000)));
            editor.handle_input("\n");
            editor.handle_input("\n");
            for ch in "abcdefghijklmnop".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            render(&mut editor, 30);

            // Navigate to line 0, col 10
            for _ in 0..4 {
                editor.handle_input("\x1b[A");
            } // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..10 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 10));

            // Down to empty line - sticky col 10 established
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 0));

            // Down to paste marker - cursor snapped to col 0 (start of marker)
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (2, 0));

            // Down to empty line
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (3, 0));

            // Down to last line - should restore sticky col 10
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (4, 10));
        }

        #[test]
        fn does_not_get_stuck_moving_down_from_a_multi_visual_line_paste_marker() {
            let mut editor = ed_with(20, 24, 0, None);

            // Build:
            // Logical line 0: "abcdefgh" + marker(21 chars) + "ijklmnopqr"
            // Logical line 1: "123456789012345678"
            //
            // Marker "[paste #1 +100 lines]" (21 chars) is wider than the
            // terminal (20). Word-wrap splits at the space before "lines",
            // producing:
            //   VL1: abcdefgh              (startCol 0,  len 8)
            //   VL2: [paste #1 +100        (startCol 8,  len 15) <- marker head
            //   VL3: lines]ijklmnopqr      (startCol 23, len 16) <- marker tail + content
            //   VL4: 123456789012345678    (line 1)
            //
            // On VL3 the marker tail "lines]" occupies visual cols 0-5.
            // Content ("i") starts at visual col 6 = logical col 29.
            for ch in "abcdefgh".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            let bigContent = "line\n".repeat(100).trim_end().to_string();
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));
            for ch in "ijklmnopqr".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            editor.handle_input("\n");
            for ch in "123456789012345678".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            render(&mut editor, 20);

            let text = editor.get_text();
            let markerMatch = find_paste_markers(&text);
            assert!(!markerMatch.is_empty(), "paste marker should be created");
            let markerLen = markerMatch[0].1 - markerMatch[0].0; // 21
            assert!(markerLen > 20, "marker should be wider than terminal");
            let markerStart = 8;
            let markerEnd = markerStart + markerLen; // 29

            // Navigate to line 0, col 6 (on "g"). Preferred col 6 is past the
            // marker tail on VL3, so the cursor should land on content ("i" at
            // col 29) without snapping back.
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A (start of line)
            for _ in 0..6 {
                editor.handle_input("\x1b[C");
            } // Right to col 6
            assert_eq!(editor.get_cursor(), (0, 6));

            // Down: cursor lands on paste marker start
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (0, markerStart));

            // Down again: preferred col 6 lands at VL3 col 29 ("i"), which is
            // past the marker. Cursor stays on line 0.
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor().0, 0);
            assert_eq!(editor.get_cursor().1, markerEnd); // col 29 = "i"

            // Up: back to paste marker
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor(), (0, markerStart));

            // Up again: back to col 6 ("g")
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor(), (0, 6));
        }

        #[test]
        fn skips_marker_continuation_vls_when_preferred_col_falls_in_marker_tail() {
            let mut editor = ed_with(20, 24, 0, None);

            // Same layout. Start at col 3 ("d"). Preferred col 3 maps to VL3
            // visual col 3 which is inside the "lines]" marker tail.
            // moveToVisualLine detects the continuation VL and skips to VL4
            // (line 1).
            //   VL1: abcdefgh              (startCol 0,  len 8)
            //   VL2: [paste #1 +100        (startCol 8,  len 15) <- marker head
            //   VL3: lines]ijklmnopqr      (startCol 23, len 16) <- marker tail + content
            //   VL4: 123456789012345678    (line 1)
            for ch in "abcdefgh".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            let bigContent = "line\n".repeat(100).trim_end().to_string();
            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", bigContent));
            for ch in "ijklmnopqr".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            editor.handle_input("\n");
            for ch in "123456789012345678".chars().map(String::from) {
                editor.handle_input(&ch);
            }
            render(&mut editor, 20);

            // Navigate to line 0, col 3 (on "d")
            editor.handle_input("\x1b[A"); // Up to line 0
            editor.handle_input("\x01"); // Ctrl+A
            for _ in 0..3 {
                editor.handle_input("\x1b[C");
            }
            assert_eq!(editor.get_cursor(), (0, 3));

            // Down: marker
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor().1, 8);

            // Down: skips VL3 (col 3 in marker tail) and lands on line 1
            editor.handle_input("\x1b[B");
            assert_eq!(editor.get_cursor(), (1, 3));

            // Round-trip back
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor().1, 8); // marker
            editor.handle_input("\x1b[A");
            assert_eq!(editor.get_cursor(), (0, 3));
        }

        #[test]
        fn submits_large_pasted_content_literally() {
            let mut editor = ed();
            let pastedText = [
                "line 1",
                "line 2",
                "line 3",
                "line 4",
                "line 5",
                "line 6",
                "line 7",
                "line 8",
                "line 9",
                "line 10",
                "tokens $1 $2 $& $$ $` $' end",
            ]
            .join("\n");
            let submitted = capture_submit(&mut editor);

            editor.handle_input(&format!("\x1b[200~{}\x1b[201~", pastedText));
            editor.handle_input("\r");

            assert_eq!(submitted.borrow().last().unwrap(), &pastedText);
        }
    }

    mod oversized_graphemes {
        use super::*;
        #[test]
        fn does_not_recurse_forever_on_a_grapheme_wider_than_the_wrap_width() {
            // A double-width grapheme cannot be split any further, so re-wrapping it
            // at grapheme granularity used to re-enter with identical arguments.
            for text in ["好", "🎉", "ab好cd"] {
                let chunks = word_wrap_line(text, 1);
                let joined: String = chunks.iter().map(|c| c.text.as_str()).collect();
                assert_eq!(joined, text, "lossy wrap of {text}");
                assert!(
                    chunks.iter().all(|c| !c.text.is_empty()),
                    "empty chunk in {chunks:?}"
                );
            }
        }

        #[test]
        fn renders_a_double_width_character_in_a_very_narrow_editor_without_throwing() {
            for border in [EditorBorderStyle::Rule, EditorBorderStyle::Box] {
                let mut editor = ed_with(80, 24, 1, Some(border));
                editor.prompt_prefix = "❯".into();
                editor.set_text("好");
                for width in 1..=20 {
                    render(&mut editor, width);
                }
            }
        }
    }

    mod box_border {
        use super::*;

        fn boxed(padding_x: usize) -> Editor {
            ed_with(80, 24, padding_x, Some(EditorBorderStyle::Box))
        }

        fn label() -> EditorTopBorderLabel {
            EditorTopBorderLabel {
                plain: " refactor-auth ".into(),
                styled: "\x1b[7m refactor-auth \x1b[27m".into(),
            }
        }

        fn numbered(n: usize) -> String {
            (0..n)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        #[test]
        fn draws_corners_and_side_borders_at_the_exact_render_width() {
            let mut editor = boxed(1);
            let width = 24;

            editor.set_text("hello");
            let lines = render(&mut editor, width);

            for line in &lines {
                assert_eq!(
                    visible_width(line),
                    width,
                    "line width drift: {}",
                    dbg(line)
                );
            }
            let plain = lines.iter().map(|l| strip(l)).collect::<Vec<_>>();
            assert_eq!(plain[0], format!("┌{}┐", "─".repeat(width - 2)));
            assert_eq!(
                plain[plain.len() - 1],
                format!("└{}┘", "─".repeat(width - 2))
            );
            for line in &plain[1..lines.len() - 1] {
                assert!(
                    line.starts_with("│") && line.ends_with("│"),
                    "missing side border: {}",
                    dbg(line)
                );
            }
        }

        #[test]
        fn reserves_two_columns_of_content_for_the_side_borders() {
            let width = 20;
            let text = "a".repeat(18); // fits rule layout (19), overflows box layout (17)

            let mut rule_editor = ed();
            rule_editor.set_text(&text);
            assert_eq!(render(&mut rule_editor, width).len() - 2, 1);

            let mut box_editor = boxed(0);
            box_editor.set_text(&text);
            assert_eq!(render(&mut box_editor, width).len() - 2, 2);
        }

        #[test]
        fn keeps_the_cursor_inside_the_right_border_when_the_line_is_full() {
            let width = 20;
            for padding_x in [0, 1] {
                let mut editor = boxed(padding_x);
                let layout_width = width - 2 - padding_x * 2 - 1;

                type_str(&mut editor, &"a".repeat(layout_width));
                let lines = render(&mut editor, width);

                let content_lines = lines[1..lines.len() - 1].to_vec();
                assert_eq!(
                    content_lines.len(),
                    1,
                    "paddingX {padding_x}: should not wrap"
                );
                for line in &lines {
                    assert_eq!(
                        visible_width(line),
                        width,
                        "paddingX {padding_x}: {}",
                        dbg(line)
                    );
                }
                let plain = strip(&content_lines[0]);
                assert!(
                    plain.ends_with("│"),
                    "cursor overwrote the border: {}",
                    dbg(&plain)
                );
            }
        }

        #[test]
        fn keeps_corners_on_scroll_indicator_borders() {
            let width = 40;
            let mut editor = ed_with(80, 20, 0, Some(EditorBorderStyle::Box));
            editor.set_text(&numbered(20));

            // Cursor sits on the last line, so content is hidden above.
            let mut plain = render(&mut editor, width)
                .iter()
                .map(|l| strip(l))
                .collect::<Vec<_>>();
            assert!(
                plain[0].starts_with("┌─── ↑ "),
                "top indicator lost its corner: {}",
                dbg(&plain[0])
            );
            assert!(plain[0].ends_with("┐"));
            assert_eq!(visible_width(&plain[0]), width);

            // Move to the first line, so content is hidden below instead.
            for _ in 0..19 {
                editor.handle_input("\x1b[A");
            }
            plain = render(&mut editor, width)
                .iter()
                .map(|l| strip(l))
                .collect::<Vec<_>>();
            let bottom = &plain[plain.len() - 1];
            assert!(
                bottom.starts_with("└─── ↓ "),
                "bottom indicator lost its corner: {}",
                dbg(bottom)
            );
            assert!(bottom.ends_with("┘"));
            assert_eq!(visible_width(bottom), width);
        }

        #[test]
        fn lays_a_top_border_label_in_flush_right_inset_from_the_corner() {
            let width = 60;
            let mut editor = boxed(0);
            editor.top_border_label = Some(label());

            let lines = render(&mut editor, width);
            let top = strip(&lines[0]);

            assert_eq!(
                visible_width(&lines[0]),
                width,
                "label pushed the border off width: {top}"
            );
            assert!(
                top.starts_with("┌───"),
                "label ate the left run of border: {top}"
            );
            assert!(
                top.ends_with(" refactor-auth ──┐"),
                "label not inset from the corner: {top}"
            );
            // Only the top border carries it.
            assert_eq!(
                strip(&lines[lines.len() - 1]),
                format!("└{}┘", "─".repeat(width - 2))
            );
        }

        #[test]
        fn keeps_the_label_out_of_the_way_of_the_scroll_indicator() {
            let width = 60;
            let mut editor = ed_with(80, 20, 0, Some(EditorBorderStyle::Box));
            editor.top_border_label = Some(label());
            editor.set_text(&numbered(20));

            let top = strip(&render(&mut editor, width)[0]);

            assert!(
                top.starts_with("┌─── ↑ "),
                "indicator lost to the label: {top}"
            );
            assert!(
                top.ends_with(" refactor-auth ──┐"),
                "label lost to the indicator: {top}"
            );
            assert_eq!(visible_width(&top), width);
        }

        #[test]
        fn drops_the_label_rather_than_crowding_a_narrow_border() {
            let mut editor = boxed(0);
            editor.top_border_label = Some(label());

            // The label needs its own 15 cells, 2 of inset, and 4 of lead-in, plus
            // the two corners: 23 columns. Below that the border is drawn plain.
            for width in 1..=40 {
                let lines = render(&mut editor, width);
                let top = strip(&lines[0]);
                assert_eq!(
                    top.contains("refactor-auth"),
                    width >= 23,
                    "wrong label decision at width {width}: {top}"
                );
                for line in &lines {
                    assert!(
                        visible_width(line) <= width,
                        "width drift at {width}: {}",
                        dbg(line)
                    );
                }
            }
        }

        #[test]
        fn carries_the_label_in_rule_mode_too_with_no_corners_to_inset_from() {
            let width = 60;
            let mut editor = ed_with(80, 24, 0, Some(EditorBorderStyle::Rule));
            editor.top_border_label = Some(label());

            let top = strip(&render(&mut editor, width)[0]);

            assert_eq!(visible_width(&top), width);
            assert!(
                top.starts_with("───"),
                "label ate the left run of border: {top}"
            );
            assert!(
                top.ends_with(" refactor-auth ──"),
                "label not inset from the edge: {top}"
            );
        }

        #[test]
        fn aligns_the_autocomplete_dropdown_to_the_inner_text_edge_outside_the_box() {
            let width = 40;
            let mut editor = boxed(1);
            provide(&mut editor, |_, _, _, _| {
                suggestions(
                    vec![item("/model", "model"), item("/models", "models")],
                    "/m",
                )
            });

            editor.handle_input("/");
            editor.handle_input("m");
            flush_debounce(&mut editor);
            assert!(editor.is_showing_autocomplete());

            let lines = render(&mut editor, width);
            let bottom_index = lines
                .iter()
                .position(|l| strip(l).starts_with("└"))
                .unwrap_or(0);
            assert!(bottom_index > 0, "box should be closed before the dropdown");

            let dropdown = &lines[bottom_index + 1..];
            assert!(!dropdown.is_empty(), "dropdown should render below the box");
            for line in dropdown {
                let plain = strip(line);
                assert!(
                    !plain.starts_with("│"),
                    "dropdown should sit outside the box: {}",
                    dbg(&plain)
                );
                // borderWidth (1) + paddingX (1) => aligned with the inner text edge
                assert!(
                    plain.starts_with("  "),
                    "dropdown misaligned: {}",
                    dbg(&plain)
                );
                assert_eq!(visible_width(line), width);
            }
        }

        #[test]
        fn degrades_to_rule_mode_when_the_width_cannot_fit_the_borders() {
            let mut editor = boxed(0);
            editor.set_text("x");

            for width in [1, 2, 3] {
                for line in render(&mut editor, width) {
                    let plain = strip(&line);
                    assert!(
                        !plain.contains("┌"),
                        "width {width} should not draw corners: {}",
                        dbg(&plain)
                    );
                    assert!(
                        !plain.contains("│"),
                        "width {width} should not draw side borders: {}",
                        dbg(&plain)
                    );
                }
            }
        }

        #[test]
        fn uses_themed_border_characters() {
            let mut t = theme();
            t.border_chars = Some(FrameBorderCharsOverride {
                top_left: Some("╭".into()),
                top_right: Some("╮".into()),
                bottom_left: Some("╰".into()),
                bottom_right: Some("╯".into()),
                ..Default::default()
            });
            let mut editor = Editor::new(
                EditorHost {
                    rows: Box::new(|| 24),
                    request_render: Box::new(|| {}),
                },
                t,
                EditorOptions {
                    padding_x: None,
                    autocomplete_max_visible: None,
                    border: Some(EditorBorderStyle::Box),
                },
            );

            let plain = render(&mut editor, 12)
                .iter()
                .map(|l| strip(l))
                .collect::<Vec<_>>();
            assert_eq!(plain[0], format!("╭{}╮", "─".repeat(10)));
            assert_eq!(plain[plain.len() - 1], format!("╰{}╯", "─".repeat(10)));
        }

        #[test]
        fn switches_border_style_at_runtime() {
            let mut editor = ed();
            assert_eq!(editor.get_border(), EditorBorderStyle::Rule);
            assert_eq!(strip(&render(&mut editor, 12)[0]), "─".repeat(12));

            editor.set_border(EditorBorderStyle::Box);
            assert_eq!(editor.get_border(), EditorBorderStyle::Box);
            assert_eq!(
                strip(&render(&mut editor, 12)[0]),
                format!("┌{}┐", "─".repeat(10))
            );
        }
    }

    mod autocomplete_visibility_notifications {
        use super::*;
        // Anything that lends the completion list screen space needs to hear when
        // the list arrives and when it goes. Both edges are easy to get wrong in
        // the same way — by looking at the wrong moment — so both are pinned here.
        fn list_editor() -> (Editor, Rc<RefCell<Vec<bool>>>) {
            let mut editor = ed();
            let seen = Rc::new(RefCell::new(Vec::new()));
            let sink = seen.clone();
            editor.on_autocomplete_visibility_change =
                Some(Box::new(move |v| sink.borrow_mut().push(v)));
            provide(&mut editor, |lines, _l, col, _f| {
                let prefix = before(lines, col);
                if !prefix.starts_with('/') {
                    return None;
                }
                suggestions(vec![item("/model", "/model")], &prefix)
            });
            (editor, seen)
        }

        #[test]
        fn announces_the_list_only_once_it_actually_exists() {
            // In hoocode the provider is async, so nothing shows on the keystroke
            // that asks; here the provider answers synchronously, so the list and
            // its announcement arrive together — exactly once.
            let (mut editor, seen) = list_editor();
            editor.handle_input("/");
            flush_debounce(&mut editor);
            assert_eq!(*seen.borrow(), vec![true]);
            assert!(editor.is_showing_autocomplete());
        }

        #[test]
        fn announces_the_list_going_away_on_escape_which_changes_no_text() {
            let (mut editor, seen) = list_editor();
            editor.handle_input("/");
            flush_debounce(&mut editor);
            seen.borrow_mut().clear();

            editor.handle_input("\x1b");
            assert_eq!(*seen.borrow(), vec![false]);
            assert!(!editor.is_showing_autocomplete());
        }

        #[test]
        fn says_nothing_while_the_list_merely_stays_open() {
            // The hot path: this fires on keystrokes, so a notification per
            // keystroke would be a render per keystroke.
            let (mut editor, seen) = list_editor();
            editor.handle_input("/");
            flush_debounce(&mut editor);
            seen.borrow_mut().clear();

            for ch in ["m", "o", "d"] {
                editor.handle_input(ch);
                flush_debounce(&mut editor);
            }
            assert!(seen.borrow().is_empty(), "no edge, no notification");
        }
    }

    mod redo {
        use super::*;
        const UNDO: &str = "\x1b[45;5u"; // ctrl+-
        const REDO: &str = "\x1bu"; // alt+u

        fn typed(text: &str) -> Editor {
            let mut editor = ed();
            type_str(&mut editor, text);
            editor
        }

        #[test]
        fn does_nothing_with_nothing_undone() {
            let mut editor = typed("hello");
            editor.handle_input(REDO);
            assert_eq!(editor.get_text(), "hello");
        }

        #[test]
        fn puts_back_exactly_what_undo_took() {
            let mut editor = typed("hello world");
            editor.handle_input(UNDO);
            assert_eq!(editor.get_text(), "hello");
            editor.handle_input(REDO);
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn walks_back_and_forward_over_several_steps() {
            let mut editor = typed("hello world");
            editor.handle_input(UNDO);
            editor.handle_input(UNDO);
            assert_eq!(editor.get_text(), "");
            editor.handle_input(REDO);
            assert_eq!(editor.get_text(), "hello");
            editor.handle_input(REDO);
            assert_eq!(editor.get_text(), "hello world");
        }

        #[test]
        fn abandons_the_undone_future_as_soon_as_you_type() {
            // The invariant that makes redo correct: once you undo and then edit,
            // the future you undid never happened. Offering it back would drop the
            // character just typed.
            let mut editor = typed("hello world");
            editor.handle_input(UNDO);
            assert_eq!(editor.get_text(), "hello");

            editor.handle_input("!");
            editor.handle_input(REDO);
            assert_eq!(
                editor.get_text(),
                "hello!",
                "redo did not resurrect \" world\""
            );
        }

        #[test]
        fn restores_the_cursor_not_only_the_text() {
            let mut editor = typed("hello world");
            editor.handle_input(UNDO);
            editor.handle_input(REDO);
            // Typing lands where the cursor was restored to, which is the end.
            editor.handle_input("!");
            assert_eq!(editor.get_text(), "hello world!");
        }
    }
}

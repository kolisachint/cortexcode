//! Port of the pin's `test/markdown.test.ts`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

use crate::common::{sgr, theme};
use cortexcode_tui_components::{DefaultTextStyle, Markdown};
use cortexcode_tui_images::{set_capabilities, TerminalCapabilities};
use cortexcode_tui_render::{Component, Tui};

/// Capabilities are process-wide; every test pins them (no hyperlinks unless
/// it says otherwise) and runs alone.
fn lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    caps(false);
    guard
}

fn caps(hyperlinks: bool) {
    set_capabilities(TerminalCapabilities {
        images: None,
        true_color: false,
        hyperlinks,
    });
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find("\x1b[") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let end = tail
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(tail.len());
        if tail[end..].starts_with('m') {
            rest = &tail[end + 1..];
        } else {
            out.push_str("\x1b[");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

fn md(text: &str) -> Markdown {
    Markdown::new(text, 0, 0, theme(), None)
}

fn styled(color: &str, close: &str, italic: bool) -> Option<DefaultTextStyle> {
    Some(DefaultTextStyle {
        color: Some(sgr(color, close)),
        italic,
        ..Default::default()
    })
}

fn gray_italic() -> Option<DefaultTextStyle> {
    styled("\x1b[90m", "\x1b[39m", true)
}

fn plain(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| strip_ansi(l)).collect()
}

fn plain_trimmed(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|l| strip_ansi(l).trim_end().to_string())
        .collect()
}

fn render_plain(text: &str, width: u16) -> Vec<String> {
    plain(&md(text).render(width))
}

fn render_trimmed(text: &str, width: u16) -> Vec<String> {
    plain_trimmed(&md(text).render(width))
}

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

/// The 40 bytes before `needle` in `hay`.
fn preceding(hay: &str, needle: &str) -> String {
    let idx = hay.find(needle).expect("needle");
    assert!(idx > 0);
    let mut start = idx.saturating_sub(40);
    while !hay.is_char_boundary(start) {
        start += 1;
    }
    hay[start..idx].to_string()
}

fn empty_lines_after(lines: &[String], idx: usize) -> Option<usize> {
    lines[idx + 1..].iter().position(|l| !l.is_empty())
}

mod lists {
    use super::*;

    #[test]
    fn should_render_simple_nested_list() {
        let _g = lock();
        let lines = render_plain("- Item 1\n  - Nested 1.1\n  - Nested 1.2\n- Item 2", 80);
        assert!(!lines.is_empty());
        assert!(has(&lines, "- Item 1"));
        assert!(has(&lines, "    - Nested 1.1"));
        assert!(has(&lines, "    - Nested 1.2"));
        assert!(has(&lines, "- Item 2"));
    }

    #[test]
    fn should_render_deeply_nested_list() {
        let _g = lock();
        let lines = render_plain("- Level 1\n  - Level 2\n    - Level 3\n      - Level 4", 80);
        assert!(has(&lines, "- Level 1"));
        assert!(has(&lines, "    - Level 2"));
        assert!(has(&lines, "        - Level 3"));
        assert!(has(&lines, "            - Level 4"));
    }

    #[test]
    fn should_render_ordered_nested_list() {
        let _g = lock();
        let lines = render_plain(
            "1. First\n   1. Nested first\n   2. Nested second\n2. Second",
            80,
        );
        assert!(has(&lines, "1. First"));
        assert!(has(&lines, "    1. Nested first"));
        assert!(has(&lines, "    2. Nested second"));
        assert!(has(&lines, "2. Second"));
    }

    #[test]
    fn should_render_mixed_ordered_and_unordered_nested_lists() {
        let _g = lock();
        let lines = render_plain(
            "1. Ordered item\n   - Unordered nested\n   - Another nested\n2. Second ordered\n   - More nested",
            80,
        );
        assert!(has(&lines, "1. Ordered item"));
        assert!(has(&lines, "    - Unordered nested"));
        assert!(has(&lines, "2. Second ordered"));
    }

    #[test]
    fn should_maintain_numbering_when_code_blocks_are_not_indented() {
        let _g = lock();
        let text = "1. First item\n\n```typescript\n// code block\n```\n\n2. Second item\n\n```typescript\n// another code block\n```\n\n3. Third item";
        let lines: Vec<String> = render_plain(text, 80)
            .iter()
            .map(|l| l.trim().to_string())
            .collect();
        let numbered: Vec<&String> = lines
            .iter()
            .filter(|l| {
                let digits = l.chars().take_while(char::is_ascii_digit).count();
                digits > 0 && l[digits..].starts_with('.')
            })
            .collect();
        assert_eq!(numbered.len(), 3, "{numbered:?}");
        assert!(numbered[0].starts_with("1."));
        assert!(numbered[1].starts_with("2."));
        assert!(numbered[2].starts_with("3."));
    }

    #[test]
    fn should_indent_wrapped_unordered_list_lines() {
        let _g = lock();
        assert_eq!(
            render_trimmed("- alpha beta gamma delta epsilon", 20),
            ["- alpha beta gamma", "  delta epsilon"]
        );
    }

    #[test]
    fn should_indent_wrapped_ordered_list_lines() {
        let _g = lock();
        assert_eq!(
            render_trimmed("1. alpha beta gamma delta epsilon", 20),
            ["1. alpha beta gamma", "   delta epsilon"]
        );
    }

    #[test]
    fn should_indent_wrapped_ordered_list_lines_with_multi_digit_markers() {
        let _g = lock();
        assert_eq!(
            render_trimmed("10. alpha beta gamma delta epsilon", 21),
            ["10. alpha beta gamma", "    delta epsilon"]
        );
    }

    #[test]
    fn should_indent_wrapped_nested_list_lines() {
        let _g = lock();
        assert_eq!(
            render_trimmed("- parent\n  - alpha beta gamma delta epsilon", 24),
            ["- parent", "    - alpha beta gamma", "      delta epsilon"]
        );
    }

    #[test]
    fn should_indent_wrapped_nested_list_lines_under_ordered_parents() {
        let _g = lock();
        assert_eq!(
            render_trimmed("1. parent\n   - alpha beta gamma delta epsilon", 24),
            ["1. parent", "    - alpha beta gamma", "      delta epsilon"]
        );
    }

    #[test]
    fn should_render_and_wrap_blockquotes_inside_list_items() {
        let _g = lock();
        assert_eq!(
            render_trimmed("- > alpha beta gamma delta epsilon zeta", 24),
            ["- │ alpha beta gamma", "  │ delta epsilon zeta"]
        );
    }

    #[test]
    fn should_render_and_wrap_code_blocks_inside_list_items() {
        let _g = lock();
        assert_eq!(
            render_trimmed("- ```ts\n  alpha beta gamma delta epsilon zeta\n  ```", 24),
            [
                "- ```ts",
                "    alpha beta gamma",
                "  delta epsilon zeta",
                "  ```"
            ]
        );
    }
}

mod tables {
    use super::*;

    const PEOPLE: &str = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |";

    fn assert_fits(lines: &[String], width: usize) {
        for line in lines {
            let n = line.chars().count();
            assert!(
                n <= width,
                "Line exceeds width {width}: {line:?} (length: {n})"
            );
        }
    }

    fn assert_two_borders(lines: &[String]) {
        let table: Vec<&String> = lines.iter().filter(|l| l.starts_with('│')).collect();
        assert!(!table.is_empty());
        for line in table {
            assert_eq!(line.matches('│').count(), 2, "{line:?}");
        }
    }

    #[test]
    fn should_render_simple_table() {
        let _g = lock();
        let lines = render_plain(PEOPLE, 80);
        for s in ["Name", "Age", "Alice", "Bob", "│", "─"] {
            assert!(has(&lines, s), "{s}");
        }
    }

    #[test]
    fn should_render_row_dividers_between_data_rows() {
        let _g = lock();
        let lines = render_plain(PEOPLE, 80);
        assert_eq!(lines.iter().filter(|l| l.contains('┼')).count(), 2);
    }

    #[test]
    fn should_keep_column_width_at_least_the_longest_word() {
        let _g = lock();
        let longest = "superlongword";
        let lines = render_plain(
            &format!("| Column One | Column Two |\n| --- | --- |\n| {longest} short | otherword |\n| small | tiny |"),
            32,
        );
        let data = lines
            .iter()
            .find(|l| l.contains(longest))
            .expect("data row");
        let segments: Vec<&str> = data.split('│').collect();
        let first = segments[1];
        let width = first.chars().count() - 2;
        assert!(width >= longest.len(), "{width}");
    }

    #[test]
    fn should_render_table_with_alignment() {
        let _g = lock();
        let lines = render_plain(
            "| Left | Center | Right |\n| :--- | :---: | ---: |\n| A | B | C |\n| Long text | Middle | End |",
            80,
        );
        for s in ["Left", "Center", "Right", "Long text"] {
            assert!(has(&lines, s));
        }
    }

    #[test]
    fn should_handle_tables_with_varying_column_widths() {
        let _g = lock();
        let lines = render_plain(
            "| Short | Very long column header |\n| --- | --- |\n| A | This is a much longer cell content |\n| B | Short |",
            80,
        );
        assert!(!lines.is_empty());
        assert!(has(&lines, "Very long column header"));
        assert!(has(&lines, "This is a much longer cell content"));
    }

    #[test]
    fn should_wrap_table_cells_when_table_exceeds_available_width() {
        let _g = lock();
        let lines = render_trimmed(
            "| Command | Description | Example |\n| --- | --- | --- |\n| npm install | Install all dependencies | npm install |\n| npm run build | Build the project | npm run build |",
            50,
        );
        assert_fits(&lines, 50);
        let all = lines.join(" ");
        for s in ["Command", "Description", "npm install", "Install"] {
            assert!(all.contains(s), "{s}");
        }
    }

    #[test]
    fn should_wrap_long_cell_content_to_multiple_lines() {
        let _g = lock();
        let lines = render_trimmed(
            "| Header |\n| --- |\n| This is a very long cell content that should wrap |",
            25,
        );
        let data = lines
            .iter()
            .filter(|l| l.starts_with('│') && !l.contains('─'))
            .count();
        assert!(data > 2, "{data}");
        let all = lines.join(" ");
        for s in ["very long", "cell content", "should wrap"] {
            assert!(all.contains(s));
        }
    }

    #[test]
    fn should_wrap_long_unbroken_tokens_inside_table_cells() {
        let _g = lock();
        let url = "https://example.com/this/is/a/very/long/url/that/should/wrap";
        let lines = render_trimmed(&format!("| Value |\n| --- |\n| prefix {url} |"), 30);
        assert_fits(&lines, 30);
        assert_two_borders(&lines);
        let extracted: String = lines
            .join("")
            .chars()
            .filter(|c| !matches!(c, '│' | '├' | '┤' | '─') && !c.is_whitespace())
            .collect();
        assert!(extracted.contains("prefix"));
        assert!(extracted.contains(url));
    }

    #[test]
    fn should_wrap_styled_inline_code_inside_table_cells_without_breaking_borders() {
        let _g = lock();
        let raw = md("| Code |\n| --- |\n| `averyveryveryverylongidentifier` |").render(20);
        assert!(raw.join("\n").contains("\x1b[33m"));
        let lines = plain_trimmed(&raw);
        assert_fits(&lines, 20);
        for line in lines.iter().filter(|l| l.starts_with('│')) {
            assert_eq!(line.matches('│').count(), 2, "{line:?}");
        }
    }

    #[test]
    fn should_handle_extremely_narrow_width_gracefully() {
        let _g = lock();
        let raw = md("| A | B | C |\n| --- | --- | --- |\n| 1 | 2 | 3 |").render(15);
        assert!(!raw.is_empty());
        assert_fits(&plain_trimmed(&raw), 15);
    }

    #[test]
    fn should_render_table_correctly_when_it_fits_naturally() {
        let _g = lock();
        let lines = render_trimmed("| A | B |\n| --- | --- |\n| 1 | 2 |", 80);
        let header = lines
            .iter()
            .find(|l| l.contains('A') && l.contains('B'))
            .expect("header");
        assert!(header.contains('│'));
        assert!(lines.iter().any(|l| l.contains('├') && l.contains('┼')));
        assert!(lines.iter().any(|l| l.contains('1') && l.contains('2')));
    }

    #[test]
    fn should_respect_padding_x_when_calculating_table_width() {
        let _g = lock();
        let lines = plain_trimmed(
            &Markdown::new(
                "| Column One | Column Two |\n| --- | --- |\n| Data 1 | Data 2 |",
                2,
                0,
                theme(),
                None,
            )
            .render(40),
        );
        assert_fits(&lines, 40);
        let row = lines.iter().find(|l| l.contains('│')).unwrap();
        assert!(row.starts_with("  "));
    }

    #[test]
    fn should_not_add_a_trailing_blank_line_when_table_is_the_last_rendered_block() {
        let _g = lock();
        let lines = render_trimmed("| Name |\n| --- |\n| Alice |", 80);
        assert_ne!(lines.last().unwrap(), "");
    }
}

mod combined_features {
    use super::*;

    #[test]
    fn should_render_lists_and_tables_together() {
        let _g = lock();
        let lines = render_plain(
            "# Test Document\n\n- Item 1\n  - Nested item\n- Item 2\n\n| Col1 | Col2 |\n| --- | --- |\n| A | B |",
            80,
        );
        assert!(has(&lines, "Test Document"));
        assert!(has(&lines, "- Item 1"));
        assert!(has(&lines, "    - Nested item"));
        assert!(has(&lines, "Col1"));
        assert!(has(&lines, "│"));
    }
}

/// `MarkdownWithInput`: the markdown lines, then an `INPUT` line.
struct MarkdownWithInput {
    markdown: Markdown,
    markdown_line_count: Rc<RefCell<usize>>,
}

impl Component for MarkdownWithInput {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines = self.markdown.render(width);
        *self.markdown_line_count.borrow_mut() = lines.len();
        lines.push("INPUT".into());
        lines
    }

    fn invalidate(&mut self) {
        self.markdown.invalidate();
    }
}

mod pre_styled_text {
    use super::*;

    #[test]
    fn should_preserve_gray_italic_styling_after_inline_code() {
        let _g = lock();
        let out = Markdown::new(
            "This is thinking with `inline code` and more text after",
            1,
            0,
            theme(),
            gray_italic(),
        )
        .render(80)
        .join("\n");
        assert!(out.contains("inline code"));
        assert!(out.contains("\x1b[90m"));
        assert!(out.contains("\x1b[3m"));
        assert!(out.contains("\x1b[33m"));
    }

    #[test]
    fn should_preserve_gray_italic_styling_after_bold_text() {
        let _g = lock();
        let out = Markdown::new(
            "This is thinking with **bold text** and more after",
            1,
            0,
            theme(),
            gray_italic(),
        )
        .render(80)
        .join("\n");
        assert!(out.contains("bold text"));
        assert!(out.contains("\x1b[90m"));
        assert!(out.contains("\x1b[3m"));
        assert!(out.contains("\x1b[1m"));
    }

    #[test]
    fn should_not_leak_styles_into_following_lines_when_rendered_in_tui() {
        let _g = lock();
        let count = Rc::new(RefCell::new(0));
        let component = Rc::new(RefCell::new(MarkdownWithInput {
            markdown: Markdown::new(
                "This is thinking with `inline code`",
                1,
                0,
                theme(),
                gray_italic(),
            ),
            markdown_line_count: count.clone(),
        }));
        let (terminal, term) = crate::render_support::virtual_terminal(80, 6);
        let mut tui = Tui::new(terminal, None);
        tui.add_child(component);
        let _events = tui.start();
        let input_row = *count.borrow();
        assert!(input_row > 0);
        assert!(!term.cell_italic(input_row as u16, 0));
        tui.stop();
    }
}

mod spacing {
    use super::*;

    #[test]
    fn after_code_blocks_only_one_blank_line_before_following_paragraph() {
        let _g = lock();
        let lines = render_trimmed(
            "hello world\n\n```js\nconst hello = \"world\";\n```\n\nagain, hello world",
            80,
        );
        let idx = lines
            .iter()
            .position(|l| l == "```")
            .expect("closing backticks");
        assert_eq!(empty_lines_after(&lines, idx), Some(1));
    }

    #[test]
    fn should_normalize_paragraph_and_code_block_spacing_to_one_blank_line() {
        let _g = lock();
        let expected = [
            "hello this is text",
            "",
            "```",
            "  code block",
            "```",
            "",
            "more text",
        ];
        for text in [
            "hello this is text\n```\ncode block\n```\nmore text",
            "hello this is text\n\n```\ncode block\n```\n\nmore text",
        ] {
            assert_eq!(render_trimmed(text, 80), expected, "{text:?}");
        }
    }

    #[test]
    fn no_trailing_blank_line_when_code_block_is_last() {
        let _g = lock();
        for text in [
            "```js\nconst hello = 'world';\n```",
            "hello world\n\n```js\nconst hello = 'world';\n```",
        ] {
            assert_ne!(render_trimmed(text, 80).last().unwrap(), "");
        }
    }

    #[test]
    fn after_dividers_only_one_blank_line_before_following_paragraph() {
        let _g = lock();
        let lines = render_trimmed("hello world\n\n---\n\nagain, hello world", 80);
        let idx = lines.iter().position(|l| l.contains('─')).expect("divider");
        assert_eq!(empty_lines_after(&lines, idx), Some(1));
    }

    #[test]
    fn no_trailing_blank_line_when_divider_is_last() {
        let _g = lock();
        assert_ne!(render_trimmed("---", 80).last().unwrap(), "");
    }

    #[test]
    fn after_headings_only_one_blank_line_before_following_paragraph() {
        let _g = lock();
        let lines = render_trimmed("# Hello\n\nThis is a paragraph", 80);
        let idx = lines
            .iter()
            .position(|l| l.contains("Hello"))
            .expect("heading");
        assert_eq!(empty_lines_after(&lines, idx), Some(1));
    }

    #[test]
    fn no_trailing_blank_line_when_heading_is_last() {
        let _g = lock();
        assert_ne!(render_trimmed("# Hello", 80).last().unwrap(), "");
    }

    #[test]
    fn after_blockquotes_only_one_blank_line_before_following_paragraph() {
        let _g = lock();
        let lines = render_trimmed("hello world\n\n> This is a quote\n\nagain, hello world", 80);
        let idx = lines
            .iter()
            .position(|l| l.contains("This is a quote"))
            .expect("quote");
        assert_eq!(empty_lines_after(&lines, idx), Some(1));
    }

    #[test]
    fn no_trailing_blank_line_when_blockquote_is_last() {
        let _g = lock();
        assert_ne!(render_trimmed("> This is a quote", 80).last().unwrap(), "");
    }
}

mod blockquotes_with_multiline_content {
    use super::*;

    fn check_quote_lines(text: &str, color: &str, close: &str) {
        let lines = Markdown::new(text, 0, 0, theme(), styled(color, close, false)).render(80);
        let quoted = plain(&lines)
            .into_iter()
            .filter(|l| l.starts_with("│ "))
            .count();
        assert_eq!(quoted, 2, "{:?}", plain(&lines));
        let foo_line = lines.iter().find(|l| l.contains("Foo")).expect("Foo line");
        let bar = lines.iter().find(|l| l.contains("bar")).expect("bar line");
        assert!(foo_line.contains("\x1b[3m"), "{foo_line:?}");
        assert!(bar.contains("\x1b[3m"), "{bar:?}");
        assert!(!foo_line.contains(color), "{foo_line:?}");
        assert!(!bar.contains(color), "{bar:?}");
    }

    #[test]
    fn should_apply_consistent_styling_to_all_lines_in_lazy_continuation_blockquote() {
        let _g = lock();
        check_quote_lines(">Foo\nbar", "\x1b[35m", "\x1b[39m");
    }

    #[test]
    fn should_apply_consistent_styling_to_explicit_multiline_blockquote() {
        let _g = lock();
        check_quote_lines(">Foo\n>bar", "\x1b[36m", "\x1b[39m");
    }

    #[test]
    fn should_render_list_content_inside_blockquotes() {
        let _g = lock();
        let lines = render_plain("> 1. bla bla\n> - nested bullet", 80);
        let quoted: Vec<String> = lines.into_iter().filter(|l| l.starts_with("│ ")).collect();
        assert!(has(&quoted, "1. bla bla"), "{quoted:?}");
        assert!(has(&quoted, "- nested bullet"), "{quoted:?}");
    }

    #[test]
    fn should_wrap_long_blockquote_lines_and_add_border_to_each_wrapped_line() {
        let _g = lock();
        let long =
            "This is a very long blockquote line that should wrap to multiple lines when rendered";
        let content: Vec<String> = render_trimmed(&format!("> {long}"), 30)
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect();
        assert!(content.len() > 1);
        for line in &content {
            assert!(line.starts_with("│ "), "{line:?}");
        }
        let all = content.join(" ");
        for s in ["very long", "blockquote", "multiple"] {
            assert!(all.contains(s));
        }
    }

    #[test]
    fn should_properly_indent_wrapped_blockquote_lines_with_styling() {
        let _g = lock();
        let lines = Markdown::new(
            "> This is styled text that is long enough to wrap",
            0,
            0,
            theme(),
            styled("\x1b[33m", "\x1b[39m", true),
        )
        .render(25);
        for line in plain_trimmed(&lines).iter().filter(|l| !l.is_empty()) {
            assert!(line.starts_with("│ "), "{line:?}");
        }
        let all = lines.join("\n");
        assert!(all.contains("\x1b[3m"));
        assert!(!all.contains("\x1b[33m"));
    }

    #[test]
    fn should_render_inline_formatting_inside_blockquotes_and_reapply_quote_styling_after() {
        let _g = lock();
        let lines = md("> Quote with **bold** and `code`").render(80);
        let plain_lines = plain(&lines);
        assert!(plain_lines.iter().any(|l| l.starts_with("│ ")));
        let all_plain = plain_lines.join(" ");
        for s in ["Quote with", "bold", "code"] {
            assert!(all_plain.contains(s));
        }
        let all = lines.join("\n");
        assert!(all.contains("\x1b[1m"));
        assert!(all.contains("\x1b[33m"));
        assert!(all.contains("\x1b[3m"));
    }
}

mod heading_with_inline_code {
    use super::*;

    #[test]
    fn should_preserve_heading_styling_after_inline_code() {
        let _g = lock();
        let out = md("### Why `sourceInfo` should not be optional")
            .render(80)
            .join("\n");
        assert!(out.contains("\x1b[33m"));
        let chunk = preceding(&out, "should not be optional");
        assert!(chunk.contains("\x1b[1m"), "{chunk:?}");
        assert!(chunk.contains("\x1b[36m"), "{chunk:?}");
    }

    #[test]
    fn should_preserve_heading_styling_after_inline_code_for_h1() {
        let _g = lock();
        let out = md("# Title with `code` inside").render(80).join("\n");
        let chunk = preceding(&out, "inside");
        assert!(chunk.contains("\x1b[1m"), "{chunk:?}");
        assert!(chunk.contains("\x1b[36m"), "{chunk:?}");
        assert!(chunk.contains("\x1b[4m"), "{chunk:?}");
    }

    #[test]
    fn should_not_leak_h1_underline_into_padding_when_inline_code_is_the_last_token() {
        let _g = lock();
        let markdown = Rc::new(RefCell::new(md("# Important distinction from `open()`")));
        let (terminal, term) = crate::render_support::virtual_terminal(80, 4);
        let mut tui = Tui::new(terminal, None);
        tui.add_child(markdown.clone());
        let _events = tui.start();
        let line = markdown.borrow_mut().render(80)[0].clone();
        let content_width = strip_ansi(&line).trim_end().chars().count();
        assert!(content_width > 0);
        for col in content_width..80 {
            assert!(
                !term.cell_underline(0, col as u16),
                "underline in padding at col {col}"
            );
        }
        tui.stop();
    }

    #[test]
    fn should_preserve_heading_styling_after_bold_text() {
        let _g = lock();
        let out = md("## Heading with **bold** and more")
            .render(80)
            .join("\n");
        let chunk = preceding(&out, "and more");
        assert!(chunk.contains("\x1b[1m"), "{chunk:?}");
        assert!(chunk.contains("\x1b[36m"), "{chunk:?}");
    }
}

mod strikethrough_syntax {
    use super::*;

    #[test]
    fn should_render_double_tilde_text_as_strikethrough() {
        let _g = lock();
        let lines = md("Use ~~strikethrough~~ here").render(80);
        let plain_text = plain(&lines).join(" ");
        assert!(lines.join("\n").contains("\x1b[9m"));
        assert!(plain_text.contains("strikethrough"));
        assert!(!plain_text.contains("~~strikethrough~~"));
    }

    #[test]
    fn should_keep_single_tilde_text_as_plain_text() {
        let _g = lock();
        let lines = md("Use ~strikethrough~ literally").render(80);
        assert!(plain(&lines).join(" ").contains("~strikethrough~"));
        assert!(!lines.join("\n").contains("\x1b[9m"));
    }
}

mod links {
    use super::*;

    fn strip_osc8(line: &str) -> String {
        let mut out = String::new();
        let mut rest = line;
        while let Some(i) = rest.find("\x1b]8;;") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 5..];
            match tail.find("\x1b\\") {
                Some(end) => rest = &tail[end + 2..],
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        strip_ansi(&out)
    }

    #[test]
    fn should_not_duplicate_url_for_autolinked_emails() {
        let _g = lock();
        let joined = render_plain("Contact user@example.com for help", 80).join(" ");
        assert!(joined.contains("user@example.com"));
        assert!(!joined.contains("mailto:"));
    }

    #[test]
    fn should_not_duplicate_url_for_bare_urls() {
        let _g = lock();
        let joined = render_plain("Visit https://example.com for more", 80).join(" ");
        assert_eq!(joined.matches("https://example.com").count(), 1);
    }

    #[test]
    fn should_show_url_in_parentheses_when_hyperlinks_are_not_supported() {
        let _g = lock();
        let joined = render_plain("[click here](https://example.com)", 80).join(" ");
        assert!(joined.contains("click here"));
        assert!(joined.contains("(https://example.com)"));
    }

    #[test]
    fn should_show_mailto_url_in_parentheses_when_hyperlinks_are_not_supported() {
        let _g = lock();
        let joined = render_plain("[Email me](mailto:test@example.com)", 80).join(" ");
        assert!(joined.contains("Email me"));
        assert!(joined.contains("(mailto:test@example.com)"));
    }

    #[test]
    fn should_emit_osc8_hyperlink_sequence_when_terminal_supports_hyperlinks() {
        let _g = lock();
        caps(true);
        let lines = md("[click here](https://example.com)").render(80);
        let joined = lines.join("");
        assert!(joined.contains("\x1b]8;;https://example.com\x1b\\"));
        assert!(joined.contains("\x1b]8;;\x1b\\"));
        let raw_plain: String = lines.iter().map(|l| strip_osc8(l)).collect();
        assert!(raw_plain.contains("click here"));
        assert!(!raw_plain.contains("(https://example.com)"));
    }

    #[test]
    fn should_use_osc8_for_mailto_links_when_terminal_supports_hyperlinks() {
        let _g = lock();
        caps(true);
        let joined = md("[Email me](mailto:test@example.com)")
            .render(80)
            .join("");
        assert!(joined.contains("\x1b]8;;mailto:test@example.com\x1b\\"));
        assert!(joined.contains("\x1b]8;;\x1b\\"));
    }

    #[test]
    fn should_use_osc8_for_bare_urls_when_terminal_supports_hyperlinks() {
        let _g = lock();
        caps(true);
        let lines = md("Visit https://example.com for more").render(80);
        assert!(lines.join("").contains("\x1b]8;;https://example.com\x1b\\"));
        let raw_plain: String = lines.iter().map(|l| strip_osc8(l)).collect();
        assert!(!raw_plain.contains("(https://example.com)"));
    }
}

mod html_like_tags_in_text {
    use super::*;

    #[test]
    fn should_render_content_with_html_like_tags_as_text() {
        let _g = lock();
        let joined = render_plain(
            "This is text with <thinking>hidden content</thinking> that should be visible",
            80,
        )
        .join(" ");
        assert!(joined.contains("hidden content") || joined.contains("<thinking>"));
    }

    #[test]
    fn should_render_html_tags_in_code_blocks_correctly() {
        let _g = lock();
        let joined = render_plain("```html\n<div>Some HTML</div>\n```", 80).join("\n");
        assert!(joined.contains("<div>") && joined.contains("</div>"));
    }
}

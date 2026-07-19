//! Markdown-to-terminal rendering component, ported from `components/markdown.ts`.
//!
//! **Reduced fidelity** relative to the original (a deliberate scope
//! trade-off — see the crate root docs): the original parses with
//! `marked` (a token-tree parser) and a custom strict-strikethrough
//! tokenizer variant; this port parses with `pulldown-cmark` (a flat
//! event-stream parser) via a small intermediate AST (`ast.rs`) built to
//! resemble `marked`'s tree closely enough to reuse the same rendering
//! structure. Consequences:
//! - The "reapply outer style prefix after an inner ANSI reset" trick
//!   (used so bold/italic text inside a colored heading or blockquote
//!   doesn't lose its outer color) is not ported; nested emphasis can
//!   locally reset to the terminal's default color.
//! - Table column sizing uses natural-width-then-proportional-shrink
//!   instead of the original's two-phase min-word-width-preserving
//!   algorithm; tables still wrap and fit the given width, but column
//!   proportions can differ in edge cases.
//! - Footnotes, definition lists, and inline images are not represented.
//! - No render-output caching (matches the simplification already applied
//!   throughout this crate and `cortexcode-tui-render`: re-render is
//!   always correct, just not memoized).

mod ast;
mod render;

pub use render::{DefaultTextStyle, HighlightCodeFn, MarkdownTheme};

use cortexcode_tui_images::is_image_line;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{apply_background_to_line, visible_width, wrap_text_with_ansi};

use render::MarkdownRenderer;

pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: MarkdownTheme,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
        default_text_style: Option<DefaultTextStyle>,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            default_text_style,
            theme,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for Markdown {
    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        let content_width = width.saturating_sub(self.padding_x * 2).max(1);

        if self.text.trim().is_empty() {
            return Vec::new();
        }

        let normalized = self.text.replace('\t', "   ");
        let blocks = ast::parse_markdown(&normalized);

        let renderer = MarkdownRenderer {
            theme: &self.theme,
            default_style: self.default_text_style.as_ref(),
        };
        let rendered_lines = renderer.render_document(&blocks, content_width);

        let mut wrapped_lines = Vec::new();
        for line in &rendered_lines {
            if is_image_line(line) {
                wrapped_lines.push(line.clone());
            } else {
                wrapped_lines.extend(wrap_text_with_ansi(line, content_width));
            }
        }

        let left_margin = " ".repeat(self.padding_x);
        let right_margin = " ".repeat(self.padding_x);
        let bg_fn = self
            .default_text_style
            .as_ref()
            .and_then(|s| s.bg_color.as_ref());
        let mut content_lines = Vec::new();

        for line in &wrapped_lines {
            if is_image_line(line) {
                content_lines.push(line.clone());
                continue;
            }
            let line_with_margins = format!("{left_margin}{line}{right_margin}");
            if let Some(bg_fn) = bg_fn {
                content_lines.push(apply_background_to_line(&line_with_margins, width, |s| {
                    bg_fn(s)
                }));
            } else {
                let visible_len = visible_width(&line_with_margins);
                content_lines.push(format!(
                    "{line_with_margins}{}",
                    " ".repeat(width.saturating_sub(visible_len))
                ));
            }
        }

        let empty_line = " ".repeat(width);
        let mut empty_lines = Vec::with_capacity(self.padding_y);
        for _ in 0..self.padding_y {
            let line = match bg_fn {
                Some(bg_fn) => apply_background_to_line(&empty_line, width, |s| bg_fn(s)),
                None => empty_line.clone(),
            };
            empty_lines.push(line);
        }

        let mut result = Vec::with_capacity(empty_lines.len() * 2 + content_lines.len());
        result.extend(empty_lines.iter().cloned());
        result.extend(content_lines);
        result.extend(empty_lines);

        if result.is_empty() {
            vec![String::new()]
        } else {
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> crate::ColorFn {
        Box::new(|s: &str| s.to_string())
    }

    fn theme() -> MarkdownTheme {
        MarkdownTheme {
            heading: identity(),
            link: identity(),
            link_url: identity(),
            code: identity(),
            code_block: identity(),
            code_block_border: identity(),
            quote: identity(),
            quote_border: identity(),
            hr: identity(),
            list_bullet: identity(),
            bold: identity(),
            italic: identity(),
            strikethrough: identity(),
            underline: identity(),
            highlight_code: None,
            code_block_indent: None,
        }
    }

    #[test]
    fn empty_text_renders_nothing() {
        let mut md = Markdown::new("", 0, 0, theme(), None);
        assert_eq!(md.render(40), Vec::<String>::new());
    }

    #[test]
    fn renders_heading_text() {
        let mut md = Markdown::new("# Title", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines[0].contains("Title"));
    }

    #[test]
    fn h3_heading_keeps_hash_prefix() {
        let mut md = Markdown::new("### Sub", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines[0].starts_with("### "));
    }

    #[test]
    fn renders_paragraph_with_bold_and_italic() {
        let mut md = Markdown::new("hello **bold** and *italic*", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines.join(" ").contains("bold"));
        assert!(lines.join(" ").contains("italic"));
    }

    #[test]
    fn renders_code_block_with_fences() {
        let mut md = Markdown::new("```rust\nfn x() {}\n```", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines.iter().any(|l| l.contains("```rust")));
        assert!(lines.iter().any(|l| l.contains("fn x()")));
    }

    #[test]
    fn renders_unordered_list() {
        let mut md = Markdown::new("- one\n- two\n", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines.iter().any(|l| l.contains("one")));
        assert!(lines.iter().any(|l| l.contains("two")));
    }

    #[test]
    fn renders_ordered_list_with_numbers() {
        let mut md = Markdown::new("1. first\n2. second\n", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines.iter().any(|l| l.contains("1.")));
        assert!(lines.iter().any(|l| l.contains("2.")));
    }

    #[test]
    fn renders_blockquote_with_border() {
        let mut md = Markdown::new("> quoted\n", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines
            .iter()
            .any(|l| l.starts_with("│ ") && l.contains("quoted")));
    }

    #[test]
    fn renders_table_with_borders() {
        let mut md = Markdown::new("| a | b |\n|---|---|\n| 1 | 2 |\n", 0, 0, theme(), None);
        let lines = md.render(40);
        assert!(lines.iter().any(|l| l.starts_with('┌')));
        assert!(lines.iter().any(|l| l.contains('a') && l.contains('b')));
        assert!(lines.iter().any(|l| l.starts_with('└')));
    }

    #[test]
    fn renders_horizontal_rule() {
        let mut md = Markdown::new("---\n", 0, 0, theme(), None);
        let lines = md.render(10);
        assert!(lines.iter().any(|l| l.contains('─')));
    }

    #[test]
    fn wraps_long_paragraphs_to_width() {
        let long_text = "word ".repeat(30);
        let mut md = Markdown::new(&long_text, 0, 0, theme(), None);
        let lines = md.render(20);
        for line in &lines {
            assert!(visible_width(line) <= 20);
        }
    }

    #[test]
    fn padding_adds_margin_lines() {
        let mut md = Markdown::new("hi", 2, 1, theme(), None);
        let lines = md.render(20);
        assert_eq!(lines[0], " ".repeat(20));
        assert!(lines[1].starts_with("  hi"));
    }
}

//! Markdown-to-terminal rendering component, ported from `components/markdown.ts`.
//!
//! Parsing is a literal port of the pinned `marked` lexer (`lexer.rs`, over
//! `marked`'s own regex sources); rendering follows markdown.ts token by
//! token (`render.rs`).

mod js_regex;
pub mod lexer;
mod render;
mod rules;
mod rules_gen;

pub use render::{DefaultTextStyle, HeadingFn, HighlightCodeFn, MarkdownTheme};

use std::collections::VecDeque;

use cortexcode_tui_images::is_image_line;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{apply_background_to_line, visible_width, wrap_text_with_ansi};

use lexer::Token;
use render::Renderer;

/// Rendered widths kept per text (markdown.ts `LINE_CACHE_WIDTHS`).
const LINE_CACHE_WIDTHS: usize = 3;

pub struct Markdown {
    text: String,
    padding_x: usize,
    padding_y: usize,
    default_text_style: Option<DefaultTextStyle>,
    theme: MarkdownTheme,
    line_cache: VecDeque<(usize, Vec<String>)>,
    line_cache_text: Option<String>,
    cached_tokens: Option<(String, Vec<Token>)>,
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
            line_cache: VecDeque::new(),
            line_cache_text: None,
            cached_tokens: None,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.invalidate_lines();
    }

    /// Drop rendered lines (they bake in theme and width) but keep the lexed
    /// tokens, which depend on the text alone.
    fn invalidate_lines(&mut self) {
        self.line_cache.clear();
        self.line_cache_text = None;
    }

    fn store_lines(&mut self, width: usize, lines: &[String]) {
        if self.line_cache.len() >= LINE_CACHE_WIDTHS {
            self.line_cache.pop_front();
        }
        self.line_cache.push_back((width, lines.to_vec()));
    }
}

impl Component for Markdown {
    fn invalidate(&mut self) {
        self.invalidate_lines();
    }

    fn render(&mut self, width: u16) -> Vec<String> {
        let width = width as usize;
        if self.line_cache_text.as_deref() == Some(self.text.as_str()) {
            if let Some((_, lines)) = self.line_cache.iter().find(|(w, _)| *w == width) {
                return lines.clone();
            }
        } else {
            self.line_cache.clear();
            self.line_cache_text = Some(self.text.clone());
        }

        let content_width = width.saturating_sub(self.padding_x * 2).max(1);

        if js_regex::js_trim(&self.text).is_empty() {
            self.store_lines(width, &[]);
            return Vec::new();
        }

        if self.cached_tokens.as_ref().map(|(t, _)| t) != Some(&self.text) {
            let tokens = lexer::lex(&self.text.replace('\t', "   ")).tokens;
            self.cached_tokens = Some((self.text.clone(), tokens));
        }
        let tokens = &self.cached_tokens.as_ref().unwrap().1;

        let renderer = Renderer {
            theme: &self.theme,
            default_style: self.default_text_style.as_ref(),
        };
        let rendered_lines = renderer.render_tokens(tokens, content_width);

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

        let result = if result.is_empty() {
            vec![String::new()]
        } else {
            result
        };
        self.store_lines(width, &result);
        result
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
            heading: Box::new(|s: &str, _| s.to_string()),
            heading_block: None,
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

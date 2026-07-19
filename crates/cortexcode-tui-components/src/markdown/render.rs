//! Renders the [`super::ast::Block`]/[`super::ast::Inline`] AST to styled
//! terminal lines, ported (with reduced fidelity — see module docs) from
//! `markdown.ts`'s `renderToken`/`renderInlineTokens`/`renderList`/`renderTable`.

use cortexcode_tui_images::{get_capabilities, hyperlink};
use cortexcode_tui_util::{visible_width, wrap_text_with_ansi};

use crate::color::ColorFn;

use super::ast::{Block, Inline};

#[derive(Default)]
pub struct DefaultTextStyle {
    pub color: Option<ColorFn>,
    pub bg_color: Option<ColorFn>,
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
}

pub type HighlightCodeFn = Box<dyn Fn(&str, Option<&str>) -> Vec<String>>;

pub struct MarkdownTheme {
    pub heading: ColorFn,
    pub link: ColorFn,
    pub link_url: ColorFn,
    pub code: ColorFn,
    pub code_block: ColorFn,
    pub code_block_border: ColorFn,
    pub quote: ColorFn,
    pub quote_border: ColorFn,
    pub hr: ColorFn,
    pub list_bullet: ColorFn,
    pub bold: ColorFn,
    pub italic: ColorFn,
    pub strikethrough: ColorFn,
    pub underline: ColorFn,
    pub highlight_code: Option<HighlightCodeFn>,
    pub code_block_indent: Option<String>,
}

#[derive(Clone, Copy)]
enum StyleMode {
    Default,
    Heading(u8),
    /// No default styling applied to bare text (used inside blockquotes).
    Plain,
}

pub struct MarkdownRenderer<'a> {
    pub theme: &'a MarkdownTheme,
    pub default_style: Option<&'a DefaultTextStyle>,
}

impl<'a> MarkdownRenderer<'a> {
    fn apply_default_style(&self, text: &str) -> String {
        let Some(style) = self.default_style else {
            return text.to_string();
        };
        let mut styled = text.to_string();
        if let Some(color) = &style.color {
            styled = color(&styled);
        }
        if style.bold {
            styled = (self.theme.bold)(&styled);
        }
        if style.italic {
            styled = (self.theme.italic)(&styled);
        }
        if style.strikethrough {
            styled = (self.theme.strikethrough)(&styled);
        }
        if style.underline {
            styled = (self.theme.underline)(&styled);
        }
        styled
    }

    fn apply_heading_style(&self, level: u8, text: &str) -> String {
        if level == 1 {
            (self.theme.heading)(&(self.theme.bold)(&(self.theme.underline)(text)))
        } else {
            (self.theme.heading)(&(self.theme.bold)(text))
        }
    }

    fn apply_style_mode(&self, mode: StyleMode, text: &str) -> String {
        match mode {
            StyleMode::Default => self.apply_default_style(text),
            StyleMode::Heading(level) => self.apply_heading_style(level, text),
            StyleMode::Plain => text.to_string(),
        }
    }

    /// Render the whole document to lines wrapped to `width` (already the
    /// content width, i.e. padding has been subtracted by the caller).
    pub fn render_document(&self, blocks: &[Block], width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            lines.extend(self.render_block(block, width, StyleMode::Default));
            if i + 1 < blocks.len() && !matches!(block, Block::List { .. }) {
                lines.push(String::new());
            }
        }
        lines
    }

    fn render_block(&self, block: &Block, width: usize, mode: StyleMode) -> Vec<String> {
        match block {
            Block::Heading { level, inlines } => {
                let heading_mode = StyleMode::Heading(*level);
                let text = self.render_inlines(inlines, heading_mode);
                if *level >= 3 {
                    let prefix = format!("{} ", "#".repeat(*level as usize));
                    vec![format!(
                        "{}{text}",
                        self.apply_heading_style(*level, &prefix)
                    )]
                } else {
                    vec![text]
                }
            }
            Block::Paragraph(inlines) => {
                vec![self.render_inlines(inlines, mode)]
            }
            Block::CodeBlock { lang, text } => {
                let mut lines = Vec::new();
                let indent = self
                    .theme
                    .code_block_indent
                    .clone()
                    .unwrap_or_else(|| "  ".to_string());
                lines.push((self.theme.code_block_border)(&format!(
                    "```{}",
                    lang.as_deref().unwrap_or("")
                )));
                if let Some(hl) = &self.theme.highlight_code {
                    for line in hl(text, lang.as_deref()) {
                        lines.push(format!("{indent}{line}"));
                    }
                } else {
                    for code_line in text.split('\n') {
                        lines.push(format!("{indent}{}", (self.theme.code_block)(code_line)));
                    }
                }
                lines.push((self.theme.code_block_border)("```"));
                lines
            }
            Block::List {
                ordered,
                start,
                items,
            } => self.render_list(*ordered, *start, items, 0, width, mode),
            Block::Table { header, rows } => self.render_table(header, rows, width, mode),
            Block::BlockQuote(inner) => self.render_blockquote(inner, width),
            Block::Rule => {
                vec![(self.theme.hr)(&"─".repeat(width.min(80)))]
            }
            Block::Html(raw) => {
                vec![self.apply_style_mode(mode, raw.trim())]
            }
        }
    }

    fn render_blockquote(&self, inner: &[Block], width: usize) -> Vec<String> {
        let quote_content_width = width.saturating_sub(2).max(1);
        let mut rendered = Vec::new();
        for (i, block) in inner.iter().enumerate() {
            rendered.extend(self.render_block(block, quote_content_width, StyleMode::Plain));
            if i + 1 < inner.len() && !matches!(block, Block::List { .. }) {
                rendered.push(String::new());
            }
        }
        while rendered.last().is_some_and(String::is_empty) {
            rendered.pop();
        }

        let mut lines = Vec::new();
        for line in rendered {
            let styled = (self.theme.quote)(&(self.theme.italic)(&line));
            for wrapped in wrap_text_with_ansi(&styled, quote_content_width) {
                lines.push(format!("{}{wrapped}", (self.theme.quote_border)("│ ")));
            }
        }
        lines
    }

    fn render_list(
        &self,
        ordered: bool,
        start: u64,
        items: &[Vec<Block>],
        depth: usize,
        width: usize,
        mode: StyleMode,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let indent = "    ".repeat(depth);

        for (i, item) in items.iter().enumerate() {
            let bullet = if ordered {
                format!("{}. ", start + i as u64)
            } else {
                "- ".to_string()
            };
            let first_prefix = format!("{indent}{}", (self.theme.list_bullet)(&bullet));
            let continuation_prefix = format!("{indent}{}", " ".repeat(visible_width(&bullet)));
            let item_width = width.saturating_sub(visible_width(&first_prefix)).max(1);
            let mut rendered_any_line = false;

            for block in item {
                if let Block::List {
                    ordered: nested_ordered,
                    start: nested_start,
                    items: nested_items,
                } = block
                {
                    lines.extend(self.render_list(
                        *nested_ordered,
                        *nested_start,
                        nested_items,
                        depth + 1,
                        width,
                        mode,
                    ));
                    rendered_any_line = true;
                    continue;
                }

                let item_lines = self.render_block(block, item_width, mode);
                for line in item_lines {
                    for wrapped in wrap_text_with_ansi(&line, item_width) {
                        let prefix = if rendered_any_line {
                            &continuation_prefix
                        } else {
                            &first_prefix
                        };
                        lines.push(format!("{prefix}{wrapped}"));
                        rendered_any_line = true;
                    }
                }
            }

            if !rendered_any_line {
                lines.push(first_prefix);
            }
        }

        lines
    }

    fn render_inlines(&self, inlines: &[Inline], mode: StyleMode) -> String {
        let mut result = String::new();
        for inline in inlines {
            result.push_str(&self.render_inline(inline, mode));
        }
        result
    }

    fn render_inline(&self, inline: &Inline, mode: StyleMode) -> String {
        match inline {
            Inline::Text(t) => self.apply_style_mode(mode, t),
            Inline::Html(t) => self.apply_style_mode(mode, t),
            Inline::Code(t) => (self.theme.code)(t),
            Inline::Strong(inner) => (self.theme.bold)(&self.render_inlines(inner, mode)),
            Inline::Emphasis(inner) => (self.theme.italic)(&self.render_inlines(inner, mode)),
            Inline::Strikethrough(inner) => {
                (self.theme.strikethrough)(&self.render_inlines(inner, mode))
            }
            Inline::Link { text, href } => {
                let link_text = self.render_inlines(text, mode);
                let styled_link = (self.theme.link)(&(self.theme.underline)(&link_text));
                if get_capabilities().hyperlinks {
                    hyperlink(&styled_link, href)
                } else {
                    let href_for_comparison = href.strip_prefix("mailto:").unwrap_or(href);
                    let raw_text: String = text
                        .iter()
                        .map(|i| match i {
                            Inline::Text(t) => t.clone(),
                            _ => String::new(),
                        })
                        .collect();
                    if raw_text == *href || raw_text == href_for_comparison {
                        styled_link
                    } else {
                        format!(
                            "{styled_link}{}",
                            (self.theme.link_url)(&format!(" ({href})"))
                        )
                    }
                }
            }
            Inline::SoftBreak => " ".to_string(),
            Inline::HardBreak => "\n".to_string(),
        }
    }

    fn wrap_cell_text(&self, text: &str, max_width: usize) -> Vec<String> {
        wrap_text_with_ansi(text, max_width.max(1))
    }

    fn render_table(
        &self,
        header: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        available_width: usize,
        mode: StyleMode,
    ) -> Vec<String> {
        let num_cols = header.len();
        if num_cols == 0 {
            return Vec::new();
        }

        let header_text: Vec<String> = header
            .iter()
            .map(|cell| self.render_inlines(cell, mode))
            .collect();
        let row_texts: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| self.render_inlines(cell, mode))
                    .collect()
            })
            .collect();

        let mut natural_widths: Vec<usize> = header_text.iter().map(|t| visible_width(t)).collect();
        for row in &row_texts {
            for (i, cell) in row.iter().enumerate() {
                if i < natural_widths.len() {
                    natural_widths[i] = natural_widths[i].max(visible_width(cell));
                }
            }
        }

        let border_overhead = 3 * num_cols + 1;
        let available_for_cells = available_width
            .saturating_sub(border_overhead)
            .max(num_cols);

        let total_natural: usize = natural_widths.iter().sum();
        let column_widths: Vec<usize> = if total_natural + border_overhead <= available_width {
            natural_widths.iter().map(|w| (*w).max(1)).collect()
        } else if total_natural == 0 {
            vec![(available_for_cells / num_cols).max(1); num_cols]
        } else {
            let mut widths: Vec<usize> = natural_widths
                .iter()
                .map(|w| ((*w * available_for_cells) / total_natural).max(1))
                .collect();
            let allocated: usize = widths.iter().sum();
            let mut remaining = available_for_cells.saturating_sub(allocated);
            let mut i = 0;
            while remaining > 0 && num_cols > 0 {
                widths[i % num_cols] += 1;
                remaining -= 1;
                i += 1;
            }
            widths
        };

        let mut lines = Vec::new();
        let border_row = |left: &str, mid: &str, right: &str| -> String {
            let cells: Vec<String> = column_widths.iter().map(|w| "─".repeat(*w)).collect();
            format!("{left}{}{right}", cells.join(mid))
        };

        lines.push(border_row("┌─", "─┬─", "─┐"));

        let header_cell_lines: Vec<Vec<String>> = header_text
            .iter()
            .zip(&column_widths)
            .map(|(text, w)| self.wrap_cell_text(text, *w))
            .collect();
        let header_line_count = header_cell_lines.iter().map(|c| c.len()).max().unwrap_or(1);
        for line_idx in 0..header_line_count {
            let parts: Vec<String> = header_cell_lines
                .iter()
                .zip(&column_widths)
                .map(|(cell_lines, w)| {
                    let text = cell_lines.get(line_idx).cloned().unwrap_or_default();
                    let padded = format!(
                        "{text}{}",
                        " ".repeat(w.saturating_sub(visible_width(&text)))
                    );
                    (self.theme.bold)(&padded)
                })
                .collect();
            lines.push(format!("│ {} │", parts.join(" │ ")));
        }

        let separator_line = border_row("├─", "─┼─", "─┤");
        lines.push(separator_line.clone());

        for (row_index, row) in row_texts.iter().enumerate() {
            let row_cell_lines: Vec<Vec<String>> = row
                .iter()
                .zip(&column_widths)
                .map(|(text, w)| self.wrap_cell_text(text, *w))
                .collect();
            let row_line_count = row_cell_lines.iter().map(|c| c.len()).max().unwrap_or(1);
            for line_idx in 0..row_line_count {
                let parts: Vec<String> = row_cell_lines
                    .iter()
                    .zip(&column_widths)
                    .map(|(cell_lines, w)| {
                        let text = cell_lines.get(line_idx).cloned().unwrap_or_default();
                        format!(
                            "{text}{}",
                            " ".repeat(w.saturating_sub(visible_width(&text)))
                        )
                    })
                    .collect();
                lines.push(format!("│ {} │", parts.join(" │ ")));
            }
            if row_index + 1 < row_texts.len() {
                lines.push(separator_line.clone());
            }
        }

        lines.push(border_row("└─", "─┴─", "─┘"));
        lines
    }
}

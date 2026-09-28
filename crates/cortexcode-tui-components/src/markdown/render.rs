//! Token rendering, ported from markdown.ts's `renderToken`,
//! `renderInlineTokens`, `renderList` and `renderTable`.

use cortexcode_tui_images::{get_capabilities, hyperlink};
use cortexcode_tui_util::{visible_width, wrap_text_with_ansi};

use crate::color::ColorFn;

use super::lexer::{Token, TokenType};

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

/// A heading style, given the heading's level.
pub type HeadingFn = Box<dyn Fn(&str, u8) -> String>;

pub struct MarkdownTheme {
    pub heading: HeadingFn,
    /// Wrapper applied to a *finished* heading line (`headingBlock`), the hook
    /// a theme uses to render headings as a filled chip. Separate from
    /// `heading`, whose output is also spliced around inline tokens.
    pub heading_block: Option<HeadingFn>,
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
    /// Prefix applied to each rendered code block line (default: two spaces).
    pub code_block_indent: Option<String>,
}

/// `InlineStyleContext.applyText`.
#[derive(Clone, Copy)]
enum ApplyText {
    Default,
    Heading(u8),
    Identity,
}

/// `InlineStyleContext`.
#[derive(Clone)]
struct StyleContext {
    apply: ApplyText,
    prefix: String,
}

const SENTINEL: &str = "\u{0}";

fn prefix_of(styled: &str) -> String {
    styled
        .find(SENTINEL)
        .map(|i| styled[..i].to_string())
        .unwrap_or_default()
}

pub(super) struct Renderer<'a> {
    pub theme: &'a MarkdownTheme,
    pub default_style: Option<&'a DefaultTextStyle>,
}

impl Renderer<'_> {
    /// `applyDefaultStyle`: foreground and decorations (the background is
    /// applied at the padding stage so it spans the full line).
    pub fn apply_default_style(&self, text: &str) -> String {
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

    fn default_style_prefix(&self) -> String {
        if self.default_style.is_none() {
            return String::new();
        }
        prefix_of(&self.apply_default_style(SENTINEL))
    }

    fn heading_style(&self, level: u8, text: &str) -> String {
        if level == 1 {
            (self.theme.heading)(&(self.theme.bold)(&(self.theme.underline)(text)), level)
        } else {
            (self.theme.heading)(&(self.theme.bold)(text), level)
        }
    }

    fn apply(&self, apply: ApplyText, text: &str) -> String {
        match apply {
            ApplyText::Default => self.apply_default_style(text),
            ApplyText::Heading(level) => self.heading_style(level, text),
            ApplyText::Identity => text.to_string(),
        }
    }

    fn default_context(&self) -> StyleContext {
        StyleContext {
            apply: ApplyText::Default,
            prefix: self.default_style_prefix(),
        }
    }

    /// Render top-level tokens to unwrapped lines.
    pub fn render_tokens(&self, tokens: &[Token], width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for (i, token) in tokens.iter().enumerate() {
            let next = tokens.get(i + 1).map(|t| t.kind);
            lines.extend(self.render_token(token, width, next, None));
        }
        lines
    }

    fn render_token(
        &self,
        token: &Token,
        width: usize,
        next: Option<TokenType>,
        ctx: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let spaced = next.is_some_and(|n| n != TokenType::Space);
        match token.kind {
            TokenType::Heading => {
                let level = token.depth.unwrap_or(1);
                let heading_ctx = StyleContext {
                    apply: ApplyText::Heading(level),
                    prefix: prefix_of(&self.heading_style(level, SENTINEL)),
                };
                let text = self.render_inline_tokens(token.children(), Some(&heading_ctx));
                let styled = if level >= 3 {
                    let prefix = format!("{} ", "#".repeat(level as usize));
                    format!("{}{text}", self.heading_style(level, &prefix))
                } else {
                    text
                };
                lines.push(match &self.theme.heading_block {
                    Some(block) => block(&styled, level),
                    None => styled,
                });
                if spaced {
                    lines.push(String::new());
                }
            }
            TokenType::Paragraph => {
                lines.push(self.render_inline_tokens(token.children(), ctx));
                if next.is_some_and(|n| n != TokenType::List && n != TokenType::Space) {
                    lines.push(String::new());
                }
            }
            TokenType::Text => {
                lines.push(self.render_inline_tokens(std::slice::from_ref(token), ctx));
            }
            TokenType::Code => {
                let indent = self.theme.code_block_indent.as_deref().unwrap_or("  ");
                let lang = token.lang.as_deref().unwrap_or("");
                lines.push((self.theme.code_block_border)(&format!("```{lang}")));
                if let Some(highlight) = &self.theme.highlight_code {
                    for line in highlight(token.text(), token.lang.as_deref()) {
                        lines.push(format!("{indent}{line}"));
                    }
                } else {
                    for line in token.text().split('\n') {
                        lines.push(format!("{indent}{}", (self.theme.code_block)(line)));
                    }
                }
                lines.push((self.theme.code_block_border)("```"));
                if spaced {
                    lines.push(String::new());
                }
            }
            TokenType::List => {
                lines.extend(self.render_list(token, 0, width, ctx));
            }
            TokenType::Table => {
                lines.extend(self.render_table(token, width, next, ctx));
            }
            TokenType::Blockquote => {
                let quote_style = |text: &str| (self.theme.quote)(&(self.theme.italic)(text));
                let quote_prefix = prefix_of(&quote_style(SENTINEL));
                let apply_quote_style = |line: &str| {
                    if quote_prefix.is_empty() {
                        quote_style(line)
                    } else {
                        quote_style(&line.replace("\x1b[0m", &format!("\x1b[0m{quote_prefix}")))
                    }
                };
                let quote_width = width.saturating_sub(2).max(1);
                let quote_ctx = StyleContext {
                    apply: ApplyText::Identity,
                    prefix: quote_prefix.clone(),
                };
                let children = token.children();
                let mut rendered = Vec::new();
                for (i, child) in children.iter().enumerate() {
                    let next_child = children.get(i + 1).map(|t| t.kind);
                    rendered.extend(self.render_token(
                        child,
                        quote_width,
                        next_child,
                        Some(&quote_ctx),
                    ));
                }
                while rendered.last().is_some_and(|l: &String| l.is_empty()) {
                    rendered.pop();
                }
                for line in &rendered {
                    let styled = apply_quote_style(line);
                    for wrapped in wrap_text_with_ansi(&styled, quote_width) {
                        lines.push(format!("{}{wrapped}", (self.theme.quote_border)("│ ")));
                    }
                }
                if spaced {
                    lines.push(String::new());
                }
            }
            TokenType::Hr => {
                lines.push((self.theme.hr)(&"─".repeat(width.min(80))));
                if spaced {
                    lines.push(String::new());
                }
            }
            TokenType::Html => {
                lines.push(
                    self.apply_default_style(token.raw.trim_matches(super::js_regex::is_js_space)),
                );
            }
            TokenType::Space => lines.push(String::new()),
            _ => {
                if let Some(text) = &token.text {
                    lines.push(text.clone());
                }
            }
        }
        lines
    }

    fn render_inline_tokens(&self, tokens: &[Token], ctx: Option<&StyleContext>) -> String {
        let owned;
        let ctx = match ctx {
            Some(c) => c,
            None => {
                owned = self.default_context();
                &owned
            }
        };
        let prefix = ctx.prefix.as_str();
        let apply_lines = |text: &str| {
            text.split('\n')
                .map(|seg| self.apply(ctx.apply, seg))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut result = String::new();
        for token in tokens {
            match token.kind {
                TokenType::Text => match &token.tokens {
                    Some(children) if !children.is_empty() => {
                        result.push_str(&self.render_inline_tokens(children, Some(ctx)));
                    }
                    _ => result.push_str(&apply_lines(token.text())),
                },
                TokenType::Paragraph => {
                    result.push_str(&self.render_inline_tokens(token.children(), Some(ctx)));
                }
                TokenType::Strong => {
                    let content = self.render_inline_tokens(token.children(), Some(ctx));
                    result.push_str(&(self.theme.bold)(&content));
                    result.push_str(prefix);
                }
                TokenType::Em => {
                    let content = self.render_inline_tokens(token.children(), Some(ctx));
                    result.push_str(&(self.theme.italic)(&content));
                    result.push_str(prefix);
                }
                TokenType::Codespan => {
                    result.push_str(&(self.theme.code)(token.text()));
                    result.push_str(prefix);
                }
                TokenType::Link => {
                    let link_text = self.render_inline_tokens(token.children(), Some(ctx));
                    let styled = (self.theme.link)(&(self.theme.underline)(&link_text));
                    let href = token.href.as_deref().unwrap_or("");
                    if get_capabilities().hyperlinks {
                        result.push_str(&hyperlink(&styled, href));
                    } else {
                        let cmp = href.strip_prefix("mailto:").unwrap_or(href);
                        result.push_str(&styled);
                        if token.text() != href && token.text() != cmp {
                            result.push_str(&(self.theme.link_url)(&format!(" ({href})")));
                        }
                    }
                    result.push_str(prefix);
                }
                TokenType::Br => result.push('\n'),
                TokenType::Del => {
                    let content = self.render_inline_tokens(token.children(), Some(ctx));
                    result.push_str(&(self.theme.strikethrough)(&content));
                    result.push_str(prefix);
                }
                TokenType::Html => result.push_str(&apply_lines(&token.raw)),
                _ => {
                    if let Some(text) = &token.text {
                        result.push_str(&apply_lines(text));
                    }
                }
            }
        }
        if !prefix.is_empty() {
            while result.ends_with(prefix) {
                result.truncate(result.len() - prefix.len());
            }
        }
        result
    }

    fn render_list(
        &self,
        token: &Token,
        depth: usize,
        width: usize,
        ctx: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let Some(list) = token.list.as_deref() else {
            return lines;
        };
        let indent = "    ".repeat(depth);
        let start = list.start.unwrap_or(1) as u64;
        for (i, item) in list.items.iter().enumerate() {
            let bullet = if list.ordered {
                format!("{}. ", start + i as u64)
            } else {
                "- ".to_string()
            };
            let first_prefix = format!("{indent}{}", (self.theme.list_bullet)(&bullet));
            let continuation_prefix = format!("{indent}{}", " ".repeat(visible_width(&bullet)));
            let item_width = width.saturating_sub(visible_width(&first_prefix)).max(1);
            let mut rendered_any = false;
            for item_token in &item.tokens {
                if item_token.kind == TokenType::List {
                    lines.extend(self.render_list(item_token, depth + 1, width, ctx));
                    rendered_any = true;
                    continue;
                }
                for line in self.render_token(item_token, item_width, None, ctx) {
                    for wrapped in wrap_text_with_ansi(&line, item_width) {
                        let prefix = if rendered_any {
                            &continuation_prefix
                        } else {
                            &first_prefix
                        };
                        lines.push(format!("{prefix}{wrapped}"));
                        rendered_any = true;
                    }
                }
            }
            if !rendered_any {
                lines.push(first_prefix);
            }
        }
        lines
    }

    fn longest_word_width(text: &str, max: usize) -> usize {
        text.split(super::js_regex::is_js_space)
            .filter(|w| !w.is_empty())
            .map(visible_width)
            .max()
            .unwrap_or(0)
            .min(max)
    }

    fn render_table(
        &self,
        token: &Token,
        available: usize,
        next: Option<TokenType>,
        ctx: Option<&StyleContext>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let Some(table) = token.table.as_deref() else {
            return lines;
        };
        let num_cols = table.header.len();
        if num_cols == 0 {
            return lines;
        }
        let border_overhead = 3 * num_cols + 1;
        let available_for_cells = available as isize - border_overhead as isize;
        if available_for_cells < num_cols as isize {
            let mut fallback = if token.raw.is_empty() {
                Vec::new()
            } else {
                wrap_text_with_ansi(&token.raw, available)
            };
            if next.is_some_and(|n| n != TokenType::Space) {
                fallback.push(String::new());
            }
            return fallback;
        }
        let available_for_cells = available_for_cells as usize;
        const MAX_UNBROKEN_WORD_WIDTH: usize = 30;

        let mut natural = vec![0usize; num_cols];
        let mut min_word = vec![1usize; num_cols];
        for (i, cell) in table.header.iter().enumerate() {
            let text = self.render_inline_tokens(&cell.tokens, ctx);
            natural[i] = visible_width(&text);
            min_word[i] = Self::longest_word_width(&text, MAX_UNBROKEN_WORD_WIDTH).max(1);
        }
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                let text = self.render_inline_tokens(&cell.tokens, ctx);
                natural[i] = natural[i].max(visible_width(&text));
                min_word[i] =
                    min_word[i].max(Self::longest_word_width(&text, MAX_UNBROKEN_WORD_WIDTH));
            }
        }

        let mut min_cols = min_word.clone();
        let mut min_cells: usize = min_cols.iter().sum();
        if min_cells > available_for_cells {
            min_cols = vec![1; num_cols];
            let remaining = available_for_cells - num_cols;
            if remaining > 0 {
                let total_weight: usize = min_word.iter().map(|w| w.saturating_sub(1)).sum();
                let growth: Vec<usize> = min_word
                    .iter()
                    .map(|w| {
                        if total_weight > 0 {
                            ((w.saturating_sub(1) as f64 / total_weight as f64) * remaining as f64)
                                .floor() as usize
                        } else {
                            0
                        }
                    })
                    .collect();
                for i in 0..num_cols {
                    min_cols[i] += growth[i];
                }
                let allocated: usize = growth.iter().sum();
                let mut leftover = remaining.saturating_sub(allocated);
                let mut i = 0;
                while leftover > 0 && i < num_cols {
                    min_cols[i] += 1;
                    leftover -= 1;
                    i += 1;
                }
            }
            min_cells = min_cols.iter().sum();
        }

        let total_natural = natural.iter().sum::<usize>() + border_overhead;
        let widths: Vec<usize> = if total_natural <= available {
            natural
                .iter()
                .zip(&min_cols)
                .map(|(n, m)| *n.max(m))
                .collect()
        } else {
            let total_grow: usize = natural
                .iter()
                .zip(&min_cols)
                .map(|(n, m)| n.saturating_sub(*m))
                .sum();
            let extra = available_for_cells.saturating_sub(min_cells);
            let mut widths: Vec<usize> = min_cols
                .iter()
                .zip(&natural)
                .map(|(m, n)| {
                    let delta = n.saturating_sub(*m);
                    let grow = if total_grow > 0 {
                        ((delta as f64 / total_grow as f64) * extra as f64).floor() as usize
                    } else {
                        0
                    };
                    m + grow
                })
                .collect();
            let allocated: usize = widths.iter().sum();
            let mut remaining = available_for_cells as isize - allocated as isize;
            while remaining > 0 {
                let mut grew = false;
                for i in 0..num_cols {
                    if remaining <= 0 {
                        break;
                    }
                    if widths[i] < natural[i] {
                        widths[i] += 1;
                        remaining -= 1;
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            widths
        };

        let rule = |l: &str, m: &str, r: &str| {
            let cells: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            format!("{l}─{}─{r}", cells.join(&format!("─{m}─")))
        };
        let wrap_cell = |text: &str, w: usize| wrap_text_with_ansi(text, w.max(1));
        let pad = |text: &str, w: usize| {
            format!(
                "{text}{}",
                " ".repeat(w.saturating_sub(visible_width(text)))
            )
        };

        lines.push(rule("┌", "┬", "┐"));
        let header_lines: Vec<Vec<String>> = table
            .header
            .iter()
            .enumerate()
            .map(|(i, c)| wrap_cell(&self.render_inline_tokens(&c.tokens, ctx), widths[i]))
            .collect();
        let header_count = header_lines.iter().map(Vec::len).max().unwrap_or(0);
        for li in 0..header_count {
            let parts: Vec<String> = header_lines
                .iter()
                .enumerate()
                .map(|(ci, cl)| {
                    (self.theme.bold)(&pad(cl.get(li).map_or("", String::as_str), widths[ci]))
                })
                .collect();
            lines.push(format!("│ {} │", parts.join(" │ ")));
        }
        let separator = rule("├", "┼", "┤");
        lines.push(separator.clone());
        for (ri, row) in table.rows.iter().enumerate() {
            let cell_lines: Vec<Vec<String>> = row
                .iter()
                .enumerate()
                .map(|(i, c)| wrap_cell(&self.render_inline_tokens(&c.tokens, ctx), widths[i]))
                .collect();
            let count = cell_lines.iter().map(Vec::len).max().unwrap_or(0);
            for li in 0..count {
                let parts: Vec<String> = cell_lines
                    .iter()
                    .enumerate()
                    .map(|(ci, cl)| pad(cl.get(li).map_or("", String::as_str), widths[ci]))
                    .collect();
                lines.push(format!("│ {} │", parts.join(" │ ")));
            }
            if ri + 1 < table.rows.len() {
                lines.push(separator.clone());
            }
        }
        lines.push(rule("└", "┴", "┘"));
        if next.is_some_and(|n| n != TokenType::Space) {
            lines.push(String::new());
        }
        lines
    }
}

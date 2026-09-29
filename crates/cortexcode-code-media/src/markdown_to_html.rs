//! Markdown to the HTML a word processor will take, hoocode
//! `utils/markdown-to-html.ts`.
//!
//! `/copy` offers the transcript twice: the markdown as plain text, and this
//! as HTML for anything that takes a rich paste. It is the subset an agent
//! transcript contains (headings, emphasis, code, links, lists, quotes, rules,
//! GFM tables) rendered as the plainest HTML that survives a paste. There is
//! no HTML passthrough: every `<` in the source is escaped. Styling is inline
//! because a clipboard fragment carries no stylesheet.
//!
//! The patterns follow the pin's JS regexes; `\w` and `\d` are spelled as
//! ASCII classes because JS reads them that way.

use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};

/// Escape the five characters that are markup, so text stays text.
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLE_CODE: &str = r#"style="font-family:Consolas,Menlo,monospace;background:#f4f4f4;padding:1px 4px;border-radius:3px""#;
const STYLE_PRE: &str = r#"style="font-family:Consolas,Menlo,monospace;background:#f4f4f4;padding:10px;border-radius:4px;white-space:pre-wrap""#;
const STYLE_TABLE: &str = r#"style="border-collapse:collapse""#;
const STYLE_CELL: &str = r#"style="border:1px solid #bbb;padding:4px 8px;text-align:left""#;
const STYLE_QUOTE: &str =
    r#"style="border-left:3px solid #bbb;margin:0 0 0 8px;padding-left:10px;color:#555""#;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("static pattern")
}

static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| re(r"(`+)([^`]|[^`][\s\S]*?[^`])\1(?!`)"));
static IMAGE: LazyLock<Regex> =
    LazyLock::new(|| re(r"!\[([^\]]*)\]\(([^)\s]+)(?:\s+&quot;[^&]*&quot;)?\)"));
static LINK: LazyLock<Regex> =
    LazyLock::new(|| re(r"\[([^\]]+)\]\(([^)\s]+)(?:\s+&quot;[^&]*&quot;)?\)"));
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| re(r"(^|[\s(])(https?://[^\s<>()]+)"));
static URL_TRAILING: LazyLock<Regex> = LazyLock::new(|| re(r"[.,;:!?]+$"));
static BOLD_ITALIC: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*\*([^*]+)\*\*\*"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"\*\*([^*]+)\*\*"));
static ITALIC_STAR: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|[^*A-Za-z0-9_])\*([^*\n]+)\*(?![*A-Za-z0-9_])"));
static ITALIC_UNDERSCORE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(^|[^_A-Za-z0-9_])_([^_\n]+)_(?![_A-Za-z0-9_])"));
static STRIKE: LazyLock<Regex> = LazyLock::new(|| re(r"~~([^~]+)~~"));
static STASHED: LazyLock<Regex> = LazyLock::new(|| re(r"\x00([0-9]+)\x00"));
static TABLE_RULE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\|?[\s:|-]+\|[\s:|-]*$"));
static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| re(r"^(\s*)([-*+]|[0-9]+[.)])\s+(.*)$"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*(```+|~~~+)(.*)$"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^(#{1,6})\s+(.*)$"));
static HEADING_CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+#+\s*$"));
static RULE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*([-*_])\s*\1\s*\1[\s\-*_]*$"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*>\s?(.*)$"));
static QUOTE_CONT: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*>"));
static QUOTE_STRIP: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*>\s?"));

fn is_match(regex: &Regex, text: &str) -> bool {
    regex.is_match(text).unwrap_or(false)
}

fn captures<'t>(regex: &Regex, text: &'t str) -> Option<Captures<'t>> {
    regex.captures(text).ok().flatten()
}

fn group<'t>(caps: &Captures<'t>, i: usize) -> &'t str {
    caps.get(i).map_or("", |m| m.as_str())
}

/// Inline markup, innermost first. Code spans are stashed behind NUL-wrapped
/// indices before anything is escaped and put back at the end, so nothing
/// inside one is read as markup.
fn render_inline(markdown: &str) -> String {
    let mut code_spans: Vec<String> = Vec::new();
    let text = CODE_SPAN.replace_all(markdown, |caps: &Captures| {
        code_spans.push(format!(
            "<code {STYLE_CODE}>{}</code>",
            escape_html(group(caps, 2).trim())
        ));
        format!("\0{}\0", code_spans.len() - 1)
    });

    let text = escape_html(&text);
    // Images before links: the link pattern would eat the `[...]`.
    let text = IMAGE.replace_all(&text, |caps: &Captures| {
        format!(r#"<img src="{}" alt="{}">"#, group(caps, 2), group(caps, 1))
    });
    let text = LINK.replace_all(&text, |caps: &Captures| {
        format!(r#"<a href="{}">{}</a>"#, group(caps, 2), group(caps, 1))
    });
    let text = BARE_URL.replace_all(&text, |caps: &Captures| {
        let url = group(caps, 2);
        let trailing = captures(&URL_TRAILING, url)
            .map(|c| group(&c, 0).to_string())
            .unwrap_or_default();
        let bare = &url[..url.len() - trailing.len()];
        format!(r#"{}<a href="{bare}">{bare}</a>{trailing}"#, group(caps, 1))
    });
    let text = BOLD_ITALIC.replace_all(&text, "<strong><em>$1</em></strong>");
    let text = BOLD.replace_all(&text, "<strong>$1</strong>");
    let text = ITALIC_STAR.replace_all(&text, "$1<em>$2</em>");
    let text = ITALIC_UNDERSCORE.replace_all(&text, "$1<em>$2</em>");
    let text = STRIKE.replace_all(&text, "<del>$1</del>");

    STASHED
        .replace_all(&text, |caps: &Captures| {
            group(caps, 1)
                .parse::<usize>()
                .ok()
                .and_then(|i| code_spans.get(i).cloned())
                .unwrap_or_default()
        })
        .into_owned()
}

/// One row of a GFM table, split on the pipes that are not escaped.
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim_start();
    let line = line.strip_prefix('|').unwrap_or(line);
    let trimmed = line.trim_end();
    let line = trimmed.strip_suffix('|').unwrap_or(trimmed);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut previous = None;
    for c in line.chars() {
        if c == '|' && previous != Some('\\') {
            cells.push(std::mem::take(&mut cell));
        } else {
            cell.push(c);
        }
        previous = Some(c);
    }
    cells.push(cell);
    cells
        .into_iter()
        .map(|cell| cell.replace("\\|", "|").trim().to_string())
        .collect()
}

/// Whether `line` is the `|---|:--:|` rule that makes the row above a header.
fn is_table_rule(line: Option<&str>) -> bool {
    line.is_some_and(|line| is_match(&TABLE_RULE, line) && line.contains('-'))
}

struct ListMarker {
    indent: usize,
    ordered: bool,
    content: String,
}

/// A list item's marker, if the line opens one.
fn list_marker(line: &str) -> Option<ListMarker> {
    let caps = captures(&LIST_MARKER, line)?;
    Some(ListMarker {
        indent: group(&caps, 1).encode_utf16().count(),
        ordered: group(&caps, 2).chars().any(|c| c.is_ascii_digit()),
        content: group(&caps, 3).to_string(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListTag {
    Ul,
    Ol,
}

impl ListTag {
    fn name(self) -> &'static str {
        match self {
            Self::Ul => "ul",
            Self::Ol => "ol",
        }
    }
}

struct Builder {
    html: Vec<String>,
    /// Open list elements, outermost first, so nesting closes in order.
    lists: Vec<(usize, ListTag)>,
    paragraph: Vec<String>,
}

impl Builder {
    fn close_lists(&mut self, to_indent: Option<usize>) {
        while let Some(&(indent, tag)) = self.lists.last() {
            if to_indent.is_some_and(|to| indent <= to) {
                break;
            }
            self.lists.pop();
            self.html.push(format!("</{}>", tag.name()));
        }
    }

    fn flush_paragraph(&mut self) {
        if self.paragraph.is_empty() {
            return;
        }
        // A single newline inside a paragraph is kept as a line break.
        let lines: Vec<String> = self.paragraph.iter().map(|l| render_inline(l)).collect();
        self.html.push(format!("<p>{}</p>", lines.join("<br>")));
        self.paragraph.clear();
    }

    fn flush(&mut self) {
        self.flush_paragraph();
        self.close_lists(None);
    }
}

/// `markdownToHtml`: a markdown document as an HTML fragment (no `<html>`,
/// no `<body>`), which is what a clipboard carries.
pub fn markdown_to_html(markdown: &str) -> String {
    let normalized = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut b = Builder {
        html: Vec::new(),
        lists: Vec::new(),
        paragraph: Vec::new(),
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Fenced code: taken whole, contents never interpreted.
        if let Some(fence) = captures(&FENCE, line) {
            b.flush();
            let marker = group(&fence, 1).chars().next().unwrap_or('`');
            let closing: String = std::iter::repeat_n(marker, 3).collect();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(&closing) {
                body.push(lines[i]);
                i += 1;
            }
            b.html.push(format!(
                "<pre {STYLE_PRE}><code>{}</code></pre>",
                escape_html(&body.join("\n"))
            ));
            i += 1;
            continue;
        }

        if line.trim().is_empty() {
            b.flush_paragraph();
            i += 1;
            continue;
        }

        if let Some(heading) = captures(&HEADING, line) {
            b.flush();
            let level = group(&heading, 1).len();
            let text = HEADING_CLOSE.replace(group(&heading, 2), "");
            b.html
                .push(format!("<h{level}>{}</h{level}>", render_inline(&text)));
            i += 1;
            continue;
        }

        if is_match(&RULE, line) {
            b.flush();
            b.html.push("<hr>".into());
            i += 1;
            continue;
        }

        // A table is only a table with its rule.
        if line.contains('|') && is_table_rule(lines.get(i + 1).copied()) {
            b.flush();
            let header = split_row(line);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(split_row(lines[i]));
                i += 1;
            }
            let cells = |row: &[String], tag: &str| -> String {
                row.iter()
                    .map(|cell| format!("<{tag} {STYLE_CELL}>{}</{tag}>", render_inline(cell)))
                    .collect()
            };
            let body: String = rows
                .iter()
                .map(|row| format!("<tr>{}</tr>", cells(row, "td")))
                .collect();
            b.html.push(format!(
                "<table {STYLE_TABLE}><thead><tr>{}</tr></thead><tbody>{body}</tbody></table>",
                cells(&header, "th")
            ));
            continue;
        }

        if let Some(quote) = captures(&QUOTE, line) {
            b.flush();
            let mut body = vec![group(&quote, 1).to_string()];
            while i + 1 < lines.len() && is_match(&QUOTE_CONT, lines[i + 1]) {
                body.push(QUOTE_STRIP.replace(lines[i + 1], "").into_owned());
                i += 1;
            }
            b.html.push(format!(
                "<blockquote {STYLE_QUOTE}>{}</blockquote>",
                markdown_to_html(&body.join("\n"))
            ));
            i += 1;
            continue;
        }

        if let Some(item) = list_marker(line) {
            b.flush_paragraph();
            let tag = if item.ordered {
                ListTag::Ol
            } else {
                ListTag::Ul
            };
            match b.lists.last().copied() {
                Some((indent, _)) if item.indent <= indent => {
                    b.close_lists(Some(item.indent));
                    match b.lists.last().copied() {
                        None => {
                            b.lists.push((item.indent, tag));
                            b.html.push(format!("<{}>", tag.name()));
                        }
                        Some((_, current)) if current != tag => {
                            // The marker changed at the same depth: one list
                            // ended and another began.
                            b.html.push(format!("</{}>", current.name()));
                            if let Some(last) = b.lists.last_mut() {
                                *last = (item.indent, tag);
                            }
                            b.html.push(format!("<{}>", tag.name()));
                        }
                        Some(_) => {}
                    }
                }
                _ => {
                    b.lists.push((item.indent, tag));
                    b.html.push(format!("<{}>", tag.name()));
                }
            }
            b.html
                .push(format!("<li>{}</li>", render_inline(&item.content)));
            i += 1;
            continue;
        }

        b.close_lists(None);
        b.paragraph.push(line.trim().to_string());
        i += 1;
    }

    b.flush();
    b.html.join("\n")
}

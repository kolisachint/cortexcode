//! Compact, terminal-friendly formatting shared by every surface that reports
//! numbers, durations and capability listings: ports of hoocode
//! `core/format-tokens.ts`, `core/format-duration.ts` and `core/format-list.ts`.

use cortexcode_tui_util::visible_width;

/// The shared segment separator (`SEGMENT_SEP` in hoocode `core/brand.ts`).
pub const SEGMENT_SEP: &str = "\u{b7}";

/// JS `Math.round`: ties round toward +infinity.
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// JS `Number.prototype.toFixed(digits)` for finite values.
///
/// `toFixed` rounds the exact binary value, picking the larger candidate on an
/// exact tie; Rust's `{:.N}` also rounds the exact value but breaks ties to
/// even, so only exact ties (`1.25`, `0.75`, ...) need handling here.
pub fn js_to_fixed(x: f64, digits: usize) -> String {
    let wide = format!("{:.*}", digits + 30, x.abs());
    let dot = wide.find('.').unwrap_or(wide.len());
    let tail = &wide[dot + 1 + digits..];
    let is_tie = tail.starts_with('5') && tail[1..].bytes().all(|b| b == b'0');
    let magnitude = if is_tie {
        let step = 10f64.powi(-(digits as i32));
        // The tie value plus half a step lands strictly inside the upper
        // candidate's rounding interval, so `{:.N}` picks it.
        format!("{:.*}", digits, x.abs() + step / 2.0)
    } else {
        format!("{:.*}", digits, x.abs())
    };
    // `x < 0` (so not `-0`) keeps its sign even when it rounds to zero.
    if x < 0.0 {
        format!("-{magnitude}")
    } else {
        magnitude
    }
}

/// `formatTokens`: numbers stay exact where they fit and degrade to one
/// decimal of `k`/`M`, so a count never grows the fixed-width chrome it sits in.
pub fn format_tokens(count: u64) -> String {
    let c = count as f64;
    if count < 1_000 {
        count.to_string()
    } else if count < 10_000 {
        format!("{}k", js_to_fixed(c / 1_000.0, 1))
    } else if count < 1_000_000 {
        format!("{}k", js_round(c / 1_000.0))
    } else if count < 10_000_000 {
        format!("{}M", js_to_fixed(c / 1_000_000.0, 1))
    } else {
        format!("{}M", js_round(c / 1_000_000.0))
    }
}

/// `formatDurationSecs`: the same run never reads "94s" in one place and
/// "1m34s" in another.
pub fn format_duration_secs(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 10.0 {
        return format!("{}s", js_to_fixed(s, 1));
    }
    if s < 60.0 {
        return format!("{}s", js_round(s));
    }
    if s < 3600.0 {
        let mins = (s / 60.0).floor();
        let rem = js_round(s % 60.0);
        return format!("{mins}m{rem:02}s");
    }
    let hrs = (s / 3600.0).floor();
    let rem_mins = js_round((s % 3600.0) / 60.0);
    format!("{hrs}h{rem_mins:02}m")
}

/// `plural`: `1 plugin` / `2 plugins`, so listings stop printing `plugin(s)`.
pub fn plural(count: usize, word: &str, plural_form: Option<&str>) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        match plural_form {
            Some(p) => format!("{count} {p}"),
            None => format!("{count} {word}s"),
        }
    }
}

/// `padCell`: pad to `width` display columns, measuring ANSI-aware.
pub fn pad_cell(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(visible_width(text)))
    )
}

/// `truncateVisible`: truncate to `max_width` display columns.
///
/// Deliberately not the TUI's `truncate_to_width`, which appends a full reset
/// even for plain input and would put escape codes into text the model and RPC
/// clients read.
pub fn truncate_visible(text: &str, max_width: i64, ellipsis: &str) -> String {
    if max_width <= 0 {
        return String::new();
    }
    let max_width = max_width as usize;
    if visible_width(text) <= max_width {
        return text.to_string();
    }
    let ellipsis_width = visible_width(ellipsis);
    if max_width <= ellipsis_width {
        return ellipsis.to_string();
    }
    let budget = max_width - ellipsis_width;
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let mut buf = [0u8; 4];
        let char_width = visible_width(ch.encode_utf8(&mut buf));
        if width + char_width > budget {
            break;
        }
        out.push(ch);
        width += char_width;
    }
    format!("{out}{ellipsis}")
}

/// `wrapIndented`: wrap `text` to `width`, indenting every line by `indent`
/// spaces, continuations included.
pub fn wrap_indented(text: &str, indent: usize, width: Option<usize>) -> Vec<String> {
    let prefix = " ".repeat(indent);
    let Some(width) = width else {
        return vec![format!("{prefix}{text}")];
    };
    let budget = width.saturating_sub(indent).max(8);
    let mut lines = Vec::new();
    let mut current = String::new();
    // JS `split(/\s+/)`: `\s` includes the Unicode space separators.
    for word in text
        .split(|c: char| c.is_whitespace() || c == '\u{feff}')
        .filter(|w| !w.is_empty())
    {
        if current.is_empty() {
            current = word.to_string();
        } else if visible_width(&current) + 1 + visible_width(word) <= budget {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(format!("{prefix}{current}"));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(format!("{prefix}{current}"));
    }
    lines
}

/// A single listed capability (`ListRow`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListRow {
    /// Primary identifier. Every row in the listing aligns on this column.
    pub name: String,
    /// Marker before the name, e.g. an installed tick. Counted in the column width.
    pub marker: Option<String>,
    /// Short facts beside the name (platforms, capabilities, source kind).
    pub facts: Vec<String>,
    /// Free text on its own wrapped, indented line(s) below the row.
    pub detail: Option<String>,
    /// Extra indented line below the detail: provenance, a path, a URL.
    pub trailer: Option<String>,
}

/// Rows under an optional heading (`ListGroup`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListGroup {
    pub title: Option<String>,
    pub rows: Vec<ListRow>,
}

/// A styling hook; `None` is identity.
pub type StyleFn<'a> = Option<&'a dyn Fn(&str) -> String>;

/// Styling hooks (`ListStyle`). Every one defaults to identity so the plain
/// path emits no escape codes at all, as tool results and RPC payloads need.
#[derive(Default, Clone, Copy)]
pub struct ListStyle<'a> {
    pub name: StyleFn<'a>,
    pub marker: StyleFn<'a>,
    pub facts: StyleFn<'a>,
    pub detail: StyleFn<'a>,
    pub trailer: StyleFn<'a>,
    pub group_title: StyleFn<'a>,
}

fn apply(style: StyleFn<'_>, text: &str) -> String {
    match style {
        Some(f) => f(text),
        None => text.to_string(),
    }
}

/// `RenderListOptions`.
#[derive(Default, Clone, Copy)]
pub struct RenderListOptions<'a> {
    /// Terminal width, when the surface knows it. Enables wrapping and truncation.
    pub columns: Option<usize>,
    /// Columns of indent applied to group titles. Rows sit two further in.
    pub indent: usize,
    pub style: ListStyle<'a>,
    /// Separator between facts. Defaults to the shared segment dot.
    pub fact_separator: Option<&'a str>,
}

/// JS `String.prototype.trimEnd` (whitespace and line terminators).
fn trim_end_js(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `renderList`: grouped rows as aligned text. The name column is measured
/// across every group, so the listing reads as one table.
pub fn render_list(groups: &[ListGroup], options: &RenderListOptions<'_>) -> String {
    let style = options.style;
    let default_sep = format!(" {SEGMENT_SEP} ");
    let fact_separator = options.fact_separator.unwrap_or(&default_sep);

    let base_indent = options.indent;
    let has_titles = groups.iter().any(|g| g.title.is_some());
    let row_indent = base_indent + if has_titles { 2 } else { 0 };
    let detail_indent = row_indent + 4;

    let all_rows: Vec<&ListRow> = groups.iter().flat_map(|g| g.rows.iter()).collect();
    if all_rows.is_empty() {
        return String::new();
    }

    // `r.marker` is truthy only when non-empty.
    let any_marker = all_rows
        .iter()
        .any(|r| r.marker.as_deref().is_some_and(|m| !m.is_empty()));
    let marker_width = if any_marker {
        all_rows
            .iter()
            .map(|r| visible_width(r.marker.as_deref().unwrap_or("")))
            .max()
            .unwrap_or(0)
            + 1
    } else {
        0
    };
    let name_width = all_rows
        .iter()
        .map(|r| visible_width(&r.name))
        .max()
        .unwrap_or(0)
        + marker_width;

    let mut lines = Vec::new();
    for group in groups {
        if let Some(title) = &group.title {
            lines.push(format!(
                "{}{}",
                " ".repeat(base_indent),
                apply(style.group_title, title)
            ));
        }
        for row in &group.rows {
            // Widths are measured on the plain text and the padding is emitted
            // separately, so styling can never be counted as visible cells.
            let marker_plain = row.marker.as_deref().unwrap_or("");
            let marker_cell = if any_marker {
                let styled = if marker_plain.is_empty() {
                    String::new()
                } else {
                    apply(style.marker, marker_plain)
                };
                format!(
                    "{styled}{}",
                    " ".repeat(marker_width.saturating_sub(visible_width(marker_plain)))
                )
            } else {
                String::new()
            };
            let name_pad = " ".repeat(
                name_width
                    .saturating_sub(marker_width)
                    .saturating_sub(visible_width(&row.name)),
            );
            let name_cell = format!("{marker_cell}{}{name_pad}", apply(style.name, &row.name));
            let facts: Vec<&str> = row
                .facts
                .iter()
                .map(String::as_str)
                .filter(|f| !f.is_empty())
                .collect();
            let facts_text = if facts.is_empty() {
                String::new()
            } else {
                format!("  {}", apply(style.facts, &facts.join(fact_separator)))
            };
            let line = format!("{}{name_cell}{facts_text}", " ".repeat(row_indent));
            lines.push(trim_end_js(&line).to_string());

            if let Some(detail) = row.detail.as_deref().filter(|d| !d.is_empty()) {
                for line in wrap_indented(detail, detail_indent, options.columns) {
                    lines.push(apply(style.detail, &line));
                }
            }
            if let Some(trailer) = row.trailer.as_deref().filter(|t| !t.is_empty()) {
                let trailer = match options.columns {
                    None => trailer.to_string(),
                    Some(columns) => truncate_visible(
                        trailer,
                        (columns as i64 - detail_indent as i64).max(8),
                        "\u{2026}",
                    ),
                };
                lines.push(apply(
                    style.trailer,
                    &format!("{}{trailer}", " ".repeat(detail_indent)),
                ));
            }
        }
    }
    lines.join("\n")
}

/// Options for [`render_compact_rows`].
#[derive(Clone, Copy)]
pub struct CompactRowsOptions<'a> {
    pub columns: Option<usize>,
    pub indent: usize,
    pub name_style: StyleFn<'a>,
    pub detail_style: StyleFn<'a>,
}

impl Default for CompactRowsOptions<'_> {
    fn default() -> Self {
        Self {
            columns: None,
            indent: 2,
            name_style: None,
            detail_style: None,
        }
    }
}

/// `renderCompactRows`: name column plus one description, truncated to width
/// rather than wrapped, so a listing stays one row per item.
pub fn render_compact_rows(rows: &[(&str, &str)], options: &CompactRowsOptions<'_>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let indent = options.indent;
    let name_width = rows
        .iter()
        .map(|(n, _)| visible_width(n))
        .max()
        .unwrap_or(0);
    rows.iter()
        .map(|(name, detail)| {
            let gap = " ".repeat(name_width.saturating_sub(visible_width(name)) + 2);
            let detail = match options.columns {
                None => detail.to_string(),
                Some(columns) => truncate_visible(
                    detail,
                    (columns as i64 - indent as i64 - name_width as i64 - 2).max(8),
                    "\u{2026}",
                ),
            };
            let line = format!(
                "{}{}{gap}{}",
                " ".repeat(indent),
                apply(options.name_style, name),
                apply(options.detail_style, &detail)
            );
            trim_end_js(&line).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

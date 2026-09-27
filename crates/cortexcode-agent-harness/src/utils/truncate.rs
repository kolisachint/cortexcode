//! Truncation of tool output: hoocode `harness/utils/truncate.ts`
//! (v0.5.89). Two independent limits, whichever is hit first: lines
//! (default 2000) and bytes (default 50KB). Never returns partial lines,
//! except the tail edge case of a last line longer than the byte limit.
//!
//! The coding agent keeps its own copy with tighter defaults
//! (`cortexcode-code-tool-api`), as hoocode does.

pub const DEFAULT_MAX_LINES: usize = 2000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;
/// Max characters per grep match line.
pub const GREP_MAX_LINE_LENGTH: usize = 500;

/// Which limit truncated the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// `TruncationResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationResult {
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    /// Lines in the original content.
    pub total_lines: usize,
    /// Bytes in the original content.
    pub total_bytes: usize,
    /// Complete lines in the output.
    pub output_lines: usize,
    pub output_bytes: usize,
    /// The last line was cut (tail truncation edge case).
    pub last_line_partial: bool,
    /// The first line alone exceeds the byte limit (head truncation).
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// `TruncationOptions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}

/// `(bytes / unit).toFixed(1)`, rounding half up as `toFixed` does for
/// these exactly representable quotients.
fn to_fixed_1(bytes: usize, unit: usize) -> String {
    let tenths = (bytes as u128 * 10 + unit as u128 / 2) / unit as u128;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// `formatSize`: `512B`, `1.5KB`, `2.0MB`.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", to_fixed_1(bytes, 1024))
    } else {
        format!("{}MB", to_fixed_1(bytes, 1024 * 1024))
    }
}

fn result(
    content: String,
    truncated_by: Option<TruncatedBy>,
    totals: (usize, usize),
    output_lines: usize,
    flags: (bool, bool),
    limits: (usize, usize),
) -> TruncationResult {
    TruncationResult {
        output_bytes: content.len(),
        content,
        truncated: truncated_by.is_some(),
        truncated_by,
        total_lines: totals.0,
        total_bytes: totals.1,
        output_lines,
        last_line_partial: flags.0,
        first_line_exceeds_limit: flags.1,
        max_lines: limits.0,
        max_bytes: limits.1,
    }
}

fn limits(options: TruncationOptions) -> (usize, usize) {
    (
        options.max_lines.unwrap_or(DEFAULT_MAX_LINES),
        options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
    )
}

/// `truncateHead`: keep the first lines (file reads). A first line over the
/// byte limit gives empty content with `first_line_exceeds_limit`.
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = limits(options);
    let total_bytes = content.len();
    let lines: Vec<&str> = content.split('\n').collect();
    let total_lines = lines.len();
    let totals = (total_lines, total_bytes);
    let limits = (max_lines, max_bytes);

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return result(
            content.to_string(),
            None,
            totals,
            total_lines,
            (false, false),
            limits,
        );
    }
    if lines[0].len() > max_bytes {
        return result(
            String::new(),
            Some(TruncatedBy::Bytes),
            totals,
            0,
            (false, true),
            limits,
        );
    }

    let mut output: Vec<&str> = Vec::new();
    let mut output_bytes = 0;
    let mut truncated_by = TruncatedBy::Lines;
    for (i, line) in lines.iter().enumerate().take(max_lines) {
        let line_bytes = line.len() + usize::from(i > 0);
        if output_bytes + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        output.push(line);
        output_bytes += line_bytes;
    }
    if output.len() >= max_lines && output_bytes <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }
    let output_lines = output.len();
    result(
        output.join("\n"),
        Some(truncated_by),
        totals,
        output_lines,
        (false, false),
        limits,
    )
}

/// `truncateTail`: keep the last lines (bash output). A last line over the
/// byte limit is cut to its end (`last_line_partial`).
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = limits(options);
    let total_bytes = content.len();
    let lines: Vec<&str> = content.split('\n').collect();
    let total_lines = lines.len();
    let totals = (total_lines, total_bytes);
    let limits = (max_lines, max_bytes);

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return result(
            content.to_string(),
            None,
            totals,
            total_lines,
            (false, false),
            limits,
        );
    }

    let mut output: Vec<String> = Vec::new();
    let mut output_bytes = 0;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;
    for line in lines.iter().rev() {
        if output.len() >= max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(!output.is_empty());
        if output_bytes + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            if output.is_empty() {
                let partial = truncate_string_to_bytes_from_end(line, max_bytes);
                output_bytes = partial.len();
                output.push(partial);
                last_line_partial = true;
            }
            break;
        }
        output.push(line.to_string());
        output_bytes += line_bytes;
    }
    if output.len() >= max_lines && output_bytes <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }
    output.reverse();
    let output_lines = output.len();
    result(
        output.join("\n"),
        Some(truncated_by),
        totals,
        output_lines,
        (last_line_partial, false),
        limits,
    )
}

/// The end of `text` within `max_bytes`, starting on a character boundary.
fn truncate_string_to_bytes_from_end(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

/// `truncateLine`: cap a line at `max_chars` (UTF-16 units) with a
/// `... [truncated]` suffix; returns `(text, was_truncated)`.
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    let units: Vec<u16> = line.encode_utf16().collect();
    if units.len() <= max_chars {
        return (line.to_string(), false);
    }
    (
        format!(
            "{}... [truncated]",
            String::from_utf16_lossy(&units[..max_chars])
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(max_lines: usize, max_bytes: usize) -> TruncationOptions {
        TruncationOptions {
            max_lines: Some(max_lines),
            max_bytes: Some(max_bytes),
        }
    }

    #[test]
    fn format_size_rounds_like_to_fixed() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1280), "1.3KB");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0MB");
    }

    #[test]
    fn head_keeps_whole_lines_within_both_limits() {
        let r = truncate_head("a\nb\nc\nd", opts(2, 100));
        assert_eq!(r.content, "a\nb");
        assert_eq!(r.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!((r.total_lines, r.output_lines), (4, 2));

        let r = truncate_head("aaaa\nbbbb\ncccc", opts(10, 9));
        assert_eq!(r.content, "aaaa\nbbbb");
        assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(r.output_bytes, 9);

        let r = truncate_head("toolongline\nx", opts(10, 5));
        assert!(r.first_line_exceeds_limit);
        assert_eq!(r.content, "");

        let r = truncate_head("fits", TruncationOptions::default());
        assert!(!r.truncated);
        assert_eq!(r.truncated_by, None);
    }

    #[test]
    fn tail_keeps_the_end_and_cuts_a_long_last_line_on_a_char_boundary() {
        let r = truncate_tail("a\nb\nc\nd", opts(2, 100));
        assert_eq!(r.content, "c\nd");
        assert_eq!(r.truncated_by, Some(TruncatedBy::Lines));

        let r = truncate_tail("x\nééééé", opts(10, 5));
        // 5 bytes back lands inside an `é`; the cut moves forward.
        assert_eq!(r.content, "éé");
        assert!(r.last_line_partial);
        assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
    }

    #[test]
    fn truncate_line_counts_utf16_units() {
        assert_eq!(truncate_line("abc", 3), ("abc".into(), false));
        assert_eq!(
            truncate_line("abcdef", 3),
            ("abc... [truncated]".into(), true)
        );
    }
}

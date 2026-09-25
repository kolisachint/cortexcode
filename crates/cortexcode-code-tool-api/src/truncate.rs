//! Shared truncation utilities for tool outputs (`core/tools/truncate.ts`).
//!
//! Truncation is based on two independent limits; whichever is hit first wins:
//! - Line limit (default: 800 lines)
//! - Byte limit (default: 32KB)
//!
//! These caps bound how much a single tool result can inject into the transcript.
//! read/bash page past them on demand, and both are overridable through the
//! `toolOutput` setting.
//!
//! Never returns partial lines (except the bash tail-truncation edge case).

use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_LINES: usize = 800;
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024;
/// Max chars per grep match line.
pub const GREP_MAX_LINE_LENGTH: usize = 500;

/// Defaults of the agent harness copy (`packages/agent/src/harness/utils/truncate.ts`),
/// which is otherwise the same algorithm.
pub const HARNESS_DEFAULT_MAX_LINES: usize = 2000;
pub const HARNESS_DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Which limit was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// `TruncationResult`, serialized with hoocode's field names (it is stored in
/// tool result `details`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TruncationResult {
    /// The truncated content.
    pub content: String,
    /// Whether truncation occurred.
    pub truncated: bool,
    /// Which limit was hit, `None` (`null`) if not truncated.
    pub truncated_by: Option<TruncatedBy>,
    /// Total number of lines in the original content.
    pub total_lines: usize,
    /// Total number of bytes in the original content.
    pub total_bytes: usize,
    /// Number of complete lines in the truncated output.
    pub output_lines: usize,
    /// Number of bytes in the truncated output.
    pub output_bytes: usize,
    /// Whether the last line was partially truncated (tail truncation edge case).
    pub last_line_partial: bool,
    /// Whether the first line exceeded the byte limit (head truncation).
    pub first_line_exceeds_limit: bool,
    /// The max lines limit that was applied.
    pub max_lines: usize,
    /// The max bytes limit that was applied.
    pub max_bytes: usize,
}

/// `TruncationOptions`; `None` means the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TruncationOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
}

impl TruncationOptions {
    fn resolve(self) -> (usize, usize) {
        (
            self.max_lines.unwrap_or(DEFAULT_MAX_LINES),
            self.max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
        )
    }
}

/// `n / d` formatted like JS `(n / d).toFixed(1)`. The exact quotient is
/// rounded half up (toFixed picks the larger candidate on a tie), which Rust's
/// `{:.1}` does not do (it rounds ties to even).
fn to_fixed_1(n: usize, d: usize) -> String {
    let n = n as u128;
    let d = d as u128;
    let tenths = (n * 20 + d) / (2 * d);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Format bytes as a human-readable size (`formatSize`).
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", to_fixed_1(bytes, 1024))
    } else {
        format!("{}MB", to_fixed_1(bytes, 1024 * 1024))
    }
}

fn untruncated(
    content: &str,
    total_lines: usize,
    max_lines: usize,
    max_bytes: usize,
) -> TruncationResult {
    TruncationResult {
        content: content.to_string(),
        truncated: false,
        truncated_by: None,
        total_lines,
        total_bytes: content.len(),
        output_lines: total_lines,
        output_bytes: content.len(),
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the head (keep the first N lines/bytes). Suitable for
/// file reads where you want to see the beginning.
///
/// Never returns partial lines. If the first line exceeds the byte limit,
/// returns empty content with `first_line_exceeds_limit`.
pub fn truncate_head(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = options.resolve();
    let total_bytes = content.len();
    let lines: Vec<&str> = content.split('\n').collect();
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return untruncated(content, total_lines, max_lines, max_bytes);
    }

    if lines[0].len() > max_bytes {
        return TruncationResult {
            content: String::new(),
            truncated: true,
            truncated_by: Some(TruncatedBy::Bytes),
            total_lines,
            total_bytes,
            output_lines: 0,
            output_bytes: 0,
            last_line_partial: false,
            first_line_exceeds_limit: true,
            max_lines,
            max_bytes,
        };
    }

    let mut output: Vec<&str> = Vec::new();
    let mut output_bytes = 0usize;
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

    let content = output.join("\n");
    TruncationResult {
        output_bytes: content.len(),
        content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output.len(),
        last_line_partial: false,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// Truncate content from the tail (keep the last N lines/bytes). Suitable for
/// bash output where you want to see the end (errors, final results).
///
/// May return a partial first line if the last line of the original content
/// exceeds the byte limit.
pub fn truncate_tail(content: &str, options: TruncationOptions) -> TruncationResult {
    let (max_lines, max_bytes) = options.resolve();
    let total_bytes = content.len();
    let lines: Vec<&str> = content.split('\n').collect();
    let total_lines = lines.len();

    if total_lines <= max_lines && total_bytes <= max_bytes {
        return untruncated(content, total_lines, max_lines, max_bytes);
    }

    // Collected back to front, reversed at the end.
    let mut output: Vec<&str> = Vec::new();
    let mut output_bytes = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;
    for line in lines.iter().rev() {
        if output.len() >= max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(!output.is_empty());
        if output_bytes + line_bytes > max_bytes {
            truncated_by = TruncatedBy::Bytes;
            // Nothing added yet and this line alone is too big: keep its end.
            if output.is_empty() {
                let partial = truncate_str_to_bytes_from_end(line, max_bytes);
                output_bytes = partial.len();
                output.push(partial);
                last_line_partial = true;
            }
            break;
        }
        output.push(line);
        output_bytes += line_bytes;
    }
    if output.len() >= max_lines && output_bytes <= max_bytes {
        truncated_by = TruncatedBy::Lines;
    }
    output.reverse();

    let content = output.join("\n");
    TruncationResult {
        output_bytes: content.len(),
        content,
        truncated: true,
        truncated_by: Some(truncated_by),
        total_lines,
        total_bytes,
        output_lines: output.len(),
        last_line_partial,
        first_line_exceeds_limit: false,
        max_lines,
        max_bytes,
    }
}

/// The last `max_bytes` bytes of `s`, advanced to a UTF-8 character boundary.
fn truncate_str_to_bytes_from_end(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len() - max_bytes;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Truncate a single line to `max_chars` UTF-16 code units, adding a
/// `... [truncated]` suffix (`truncateLine`, used for grep match lines).
pub fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    let units = line.encode_utf16().count();
    if units <= max_chars {
        return (line.to_string(), false);
    }
    let head: Vec<u16> = line.encode_utf16().take(max_chars).collect();
    (
        format!("{}... [truncated]", String::from_utf16_lossy(&head)),
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
    fn format_size_matches_js_to_fixed() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(1023), "1023B");
        assert_eq!(format_size(1024), "1.0KB");
        // 1.25 exactly: toFixed rounds the tie up.
        assert_eq!(format_size(1280), "1.3KB");
        assert_eq!(format_size(32 * 1024), "32.0KB");
        assert_eq!(format_size(16 * 1024), "16.0KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
        assert_eq!(format_size(1024 * 1024 + 1024 * 1024 / 4), "1.3MB");
    }

    #[test]
    fn head_untruncated_returns_input() {
        let r = truncate_head("a\nb\nc", TruncationOptions::default());
        assert!(!r.truncated);
        assert_eq!(r.truncated_by, None);
        assert_eq!(r.content, "a\nb\nc");
        assert_eq!(r.total_lines, 3);
        assert_eq!(r.output_bytes, 5);
        assert_eq!(r.max_lines, DEFAULT_MAX_LINES);
        assert_eq!(r.max_bytes, DEFAULT_MAX_BYTES);
    }

    #[test]
    fn head_truncates_by_lines() {
        let text: Vec<String> = (1..=10).map(|i| format!("Line {i}")).collect();
        let r = truncate_head(&text.join("\n"), opts(3, 1000));
        assert!(r.truncated);
        assert_eq!(r.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(r.content, "Line 1\nLine 2\nLine 3");
        assert_eq!(r.output_lines, 3);
        assert_eq!(r.total_lines, 10);
    }

    #[test]
    fn head_truncates_by_bytes_without_partial_lines() {
        let r = truncate_head("aaaa\nbbbb\ncccc", opts(100, 9));
        assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
        assert_eq!(r.content, "aaaa\nbbbb");
        assert_eq!(r.output_bytes, 9);
    }

    #[test]
    fn head_first_line_too_long() {
        let r = truncate_head("aaaaaaaaaa\nb", opts(100, 5));
        assert!(r.first_line_exceeds_limit);
        assert_eq!(r.content, "");
        assert_eq!(r.output_lines, 0);
        assert_eq!(r.truncated_by, Some(TruncatedBy::Bytes));
    }

    #[test]
    fn head_counts_bytes_not_chars() {
        // "é" is 2 bytes.
        let r = truncate_head("éé\néé", opts(100, 4));
        assert_eq!(r.content, "éé");
        assert_eq!(r.total_bytes, 9);
    }

    #[test]
    fn tail_keeps_the_end() {
        let r = truncate_tail("1\n2\n3\n4", opts(2, 1000));
        assert_eq!(r.content, "3\n4");
        assert_eq!(r.truncated_by, Some(TruncatedBy::Lines));
    }

    #[test]
    fn tail_partial_last_line_on_char_boundary() {
        let r = truncate_tail("x\nabcé", opts(100, 3));
        // Last 3 bytes are "cé" minus... "c" + 2-byte "é" = 3 bytes.
        assert_eq!(r.content, "cé");
        assert!(r.last_line_partial);
        let r = truncate_tail("x\nabé", opts(100, 1));
        // One byte from the end lands mid-"é": advance to the boundary.
        assert_eq!(r.content, "");
        assert!(r.last_line_partial);
    }

    #[test]
    fn truncate_line_counts_utf16_units() {
        assert_eq!(truncate_line("abc", 3), ("abc".to_string(), false));
        assert_eq!(
            truncate_line("abcdef", 3),
            ("abc... [truncated]".to_string(), true)
        );
    }

    #[test]
    fn serializes_with_hoocode_field_names() {
        let r = truncate_head("a\nb", opts(1, 100));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["truncatedBy"], "lines");
        assert_eq!(v["firstLineExceedsLimit"], false);
        assert_eq!(v["outputLines"], 1);
        let r = truncate_head("a", TruncationOptions::default());
        assert!(serde_json::to_value(&r).unwrap()["truncatedBy"].is_null());
    }
}

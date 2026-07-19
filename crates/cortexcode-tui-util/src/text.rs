//! ANSI-aware text wrapping, truncation, and column slicing.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`
//! (`wrapTextWithAnsi`, `truncateToWidth`, `sliceByColumn`, `sliceWithWidth`,
//! `extractSegments`, `normalizeTerminalOutput`, `isWhitespaceChar`,
//! `isPunctuationChar`, `applyBackgroundToLine`).

use unicode_segmentation::UnicodeSegmentation;

use crate::ansi::{extract_ansi_code, update_tracker_from_text, AnsiCodeTracker};
use crate::width::{grapheme_width, visible_width};

// ---------------------------------------------------------------------------
// Character classification
// ---------------------------------------------------------------------------

/// Whether a (single-character) string is whitespace.
pub fn is_whitespace_char(c: &str) -> bool {
    c.chars().all(char::is_whitespace) && !c.is_empty()
}

const PUNCTUATION_CHARS: &str = "(){}[]<>.,;:'\"!?+-=*/\\|&%^$#@~`";

/// Whether a (single-character) string is one of the common punctuation marks.
pub fn is_punctuation_char(c: &str) -> bool {
    c.chars().count() == 1
        && c.chars()
            .next()
            .is_some_and(|ch| PUNCTUATION_CHARS.contains(ch))
}

/// Normalize precomposed Thai/Lao AM vowels to their compatibility
/// decomposition, avoiding stale-cell artifacts in some terminals'
/// differential repaint during streaming.
pub fn normalize_terminal_output(s: &str) -> String {
    if !s.contains('\u{0e33}') && !s.contains('\u{0eb3}') {
        return s.to_string();
    }
    s.chars()
        .flat_map(|c| match c {
            '\u{0e33}' => vec!['\u{0e4d}', '\u{0e32}'],
            '\u{0eb3}' => vec!['\u{0ecd}', '\u{0eb2}'],
            other => vec![other],
        })
        .collect()
}

/// Apply a background-color function to `line`, padding it to `width` first.
pub fn apply_background_to_line(
    line: &str,
    width: usize,
    bg_fn: impl Fn(&str) -> String,
) -> String {
    let visible_len = visible_width(line);
    let padding = " ".repeat(width.saturating_sub(visible_len));
    bg_fn(&format!("{line}{padding}"))
}

// ---------------------------------------------------------------------------
// Wrapping
// ---------------------------------------------------------------------------

fn split_into_tokens_with_ansi(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut in_whitespace = false;
    let mut i = 0;

    while i < text.len() {
        if let Some((code, len)) = extract_ansi_code(text, i) {
            pending_ansi.push_str(code);
            i += len;
            continue;
        }

        let ch_len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        let ch = &text[i..i + ch_len];
        let char_is_space = ch == " ";

        if char_is_space != in_whitespace && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }

        if !pending_ansi.is_empty() {
            current.push_str(&pending_ansi);
            pending_ansi.clear();
        }

        in_whitespace = char_is_space;
        current.push_str(ch);
        i += ch_len;
    }

    if !pending_ansi.is_empty() {
        current.push_str(&pending_ansi);
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn break_long_word(word: &str, width: usize, tracker: &mut AnsiCodeTracker) -> Vec<String> {
    enum Seg<'a> {
        Ansi(&'a str),
        Grapheme(&'a str),
    }

    let mut segments = Vec::new();
    let mut i = 0;
    while i < word.len() {
        if let Some((code, len)) = extract_ansi_code(word, i) {
            segments.push(Seg::Ansi(code));
            i += len;
        } else {
            let mut end = i;
            while end < word.len() && extract_ansi_code(word, end).is_none() {
                end += 1;
            }
            for g in word[i..end].graphemes(true) {
                segments.push(Seg::Grapheme(g));
            }
            i = end;
        }
    }

    let mut lines = Vec::new();
    let mut current_line = tracker.active_codes();
    let mut current_width = 0usize;

    for seg in segments {
        match seg {
            Seg::Ansi(code) => {
                current_line.push_str(code);
                tracker.process(code);
            }
            Seg::Grapheme(g) => {
                if g.is_empty() {
                    continue;
                }
                let w = grapheme_width(g);
                if current_width + w > width {
                    let reset = tracker.line_end_reset();
                    current_line.push_str(&reset);
                    lines.push(std::mem::take(&mut current_line));
                    current_line = tracker.active_codes();
                    current_width = 0;
                }
                current_line.push_str(g);
                current_width += w;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }

    let mut wrapped = Vec::new();
    let mut tracker = AnsiCodeTracker::new();
    let tokens = split_into_tokens_with_ansi(line);

    let mut current_line = String::new();
    let mut current_visible_len = 0usize;

    for token in &tokens {
        let token_visible_len = visible_width(token);
        let is_whitespace = token.trim().is_empty();

        if token_visible_len > width && !is_whitespace {
            if !current_line.is_empty() {
                let reset = tracker.line_end_reset();
                if !reset.is_empty() {
                    current_line.push_str(&reset);
                }
                wrapped.push(std::mem::take(&mut current_line));
            }
            let mut broken = break_long_word(token, width, &mut tracker);
            let last = broken.pop().unwrap_or_default();
            wrapped.extend(broken);
            current_visible_len = visible_width(&last);
            current_line = last;
            continue;
        }

        let total_needed = current_visible_len + token_visible_len;
        if total_needed > width && current_visible_len > 0 {
            let mut line_to_wrap = current_line.trim_end().to_string();
            let reset = tracker.line_end_reset();
            if !reset.is_empty() {
                line_to_wrap.push_str(&reset);
            }
            wrapped.push(line_to_wrap);
            if is_whitespace {
                current_line = tracker.active_codes();
                current_visible_len = 0;
            } else {
                current_line = tracker.active_codes() + token;
                current_visible_len = token_visible_len;
            }
        } else {
            current_line.push_str(token);
            current_visible_len += token_visible_len;
        }

        update_tracker_from_text(token, &mut tracker);
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }

    if wrapped.is_empty() {
        vec![String::new()]
    } else {
        wrapped
            .into_iter()
            .map(|l| l.trim_end().to_string())
            .collect()
    }
}

/// Word-wrap `text` (which may contain ANSI codes and newlines) to `width`
/// visible columns per line. Only wraps — no padding, no background colors.
/// Active ANSI codes are carried across wrapped and literal newline breaks.
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let input_lines: Vec<&str> = text.split('\n').collect();
    let mut result = Vec::new();
    let mut tracker = AnsiCodeTracker::new();

    for input_line in input_lines {
        let prefix = if !result.is_empty() {
            tracker.active_codes()
        } else {
            String::new()
        };
        result.extend(wrap_single_line(&format!("{prefix}{input_line}"), width));
        update_tracker_from_text(input_line, &mut tracker);
    }

    if result.is_empty() {
        vec![String::new()]
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

fn truncate_fragment_to_width(text: &str, max_width: usize) -> (String, usize) {
    if max_width == 0 || text.is_empty() {
        return (String::new(), 0);
    }

    let mut result = String::new();
    let mut width = 0;
    for g in text.graphemes(true) {
        let w = grapheme_width(g);
        if width + w > max_width {
            break;
        }
        result.push_str(g);
        width += w;
    }
    (result, width)
}

fn finalize_truncated(
    prefix: &str,
    prefix_width: usize,
    ellipsis: &str,
    ellipsis_width: usize,
    max_width: usize,
    pad: bool,
) -> String {
    const RESET: &str = "\x1b[0m";
    let visible_width = prefix_width + ellipsis_width;
    let mut result = if ellipsis.is_empty() {
        format!("{prefix}{RESET}")
    } else {
        format!("{prefix}{RESET}{ellipsis}{RESET}")
    };
    if pad {
        result.push_str(&" ".repeat(max_width.saturating_sub(visible_width)));
    }
    result
}

/// Truncate `text` (which may contain ANSI codes) to at most `max_width`
/// visible columns, appending `ellipsis` when truncated. If `pad` is set,
/// the result is padded with spaces to exactly `max_width`.
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let text_width = visible_width(text);
        if text_width <= max_width {
            return if pad {
                format!("{text}{}", " ".repeat(max_width - text_width))
            } else {
                text.to_string()
            };
        }
        let (clipped_text, clipped_width) = truncate_fragment_to_width(ellipsis, max_width);
        if clipped_width == 0 {
            return if pad {
                " ".repeat(max_width)
            } else {
                String::new()
            };
        }
        return finalize_truncated("", 0, &clipped_text, clipped_width, max_width, pad);
    }

    let target_width = max_width - ellipsis_width;
    let mut result = String::new();
    let mut visible_so_far = 0usize;
    let mut kept_width = 0usize;
    let mut keep_contiguous_prefix = true;
    let mut overflowed = false;

    for g in text.graphemes(true) {
        let w = grapheme_width(g);
        if keep_contiguous_prefix && kept_width + w <= target_width {
            result.push_str(g);
            kept_width += w;
        } else {
            keep_contiguous_prefix = false;
        }
        visible_so_far += w;
        if visible_so_far > max_width {
            overflowed = true;
            break;
        }
    }
    let exhausted = !overflowed;

    if !overflowed && exhausted && visible_so_far <= max_width && kept_width == visible_so_far {
        return if pad {
            format!(
                "{text}{}",
                " ".repeat(max_width.saturating_sub(visible_so_far))
            )
        } else {
            text.to_string()
        };
    }

    finalize_truncated(
        &result,
        kept_width,
        ellipsis,
        ellipsis_width,
        max_width,
        pad,
    )
}

// ---------------------------------------------------------------------------
// Column slicing
// ---------------------------------------------------------------------------

/// Extract a range of visible columns `[start_col, start_col+length)` from
/// `line`, along with the actual visible width extracted. If `strict`, a
/// wide grapheme that would straddle the range boundary is excluded rather
/// than clipped.
pub fn slice_with_width(
    line: &str,
    start_col: usize,
    length: usize,
    strict: bool,
) -> (String, usize) {
    if length == 0 {
        return (String::new(), 0);
    }
    let end_col = start_col + length;
    let mut result = String::new();
    let mut result_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0;
    let mut pending_ansi = String::new();

    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                result.push_str(code);
            } else if current_col < start_col {
                pending_ansi.push_str(code);
            }
            i += len;
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += 1;
        }

        for g in line[i..text_end].graphemes(true) {
            let w = grapheme_width(g);
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                if !pending_ansi.is_empty() {
                    result.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                result.push_str(g);
                result_width += w;
            }
            current_col += w;
            if current_col >= end_col {
                break;
            }
        }
        i = text_end;
        if current_col >= end_col {
            break;
        }
    }

    (result, result_width)
}

/// Like [`slice_with_width`] but returns only the extracted text.
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).0
}

/// Extract "before" (`[0, before_end)`) and "after" (`[after_start,
/// after_start+after_len)`) column ranges from `line` in a single pass. The
/// "after" segment inherits SGR styling active at `after_start` so it reads
/// correctly when composited around an overlay.
pub fn extract_segments(
    line: &str,
    before_end: usize,
    after_start: usize,
    after_len: usize,
    strict_after: bool,
) -> (String, usize, String, usize) {
    let mut before = String::new();
    let mut before_width = 0usize;
    let mut after = String::new();
    let mut after_width = 0usize;
    let mut current_col = 0usize;
    let mut i = 0;
    let mut pending_ansi_before = String::new();
    let mut after_started = false;
    let after_end = after_start + after_len;

    let mut tracker = AnsiCodeTracker::new();

    while i < line.len() {
        if let Some((code, len)) = extract_ansi_code(line, i) {
            tracker.process(code);
            if current_col < before_end {
                pending_ansi_before.push_str(code);
            } else if current_col >= after_start && current_col < after_end && after_started {
                after.push_str(code);
            }
            i += len;
            continue;
        }

        let mut text_end = i;
        while text_end < line.len() && extract_ansi_code(line, text_end).is_none() {
            text_end += 1;
        }

        for g in line[i..text_end].graphemes(true) {
            let w = grapheme_width(g);

            if current_col < before_end {
                if !pending_ansi_before.is_empty() {
                    before.push_str(&pending_ansi_before);
                    pending_ansi_before.clear();
                }
                before.push_str(g);
                before_width += w;
            } else if current_col >= after_start && current_col < after_end {
                let fits = !strict_after || current_col + w <= after_end;
                if fits {
                    if !after_started {
                        after.push_str(&tracker.active_codes());
                        after_started = true;
                    }
                    after.push_str(g);
                    after_width += w;
                }
            }

            current_col += w;
            let done = if after_len == 0 {
                current_col >= before_end
            } else {
                current_col >= after_end
            };
            if done {
                break;
            }
        }
        i = text_end;
        let done = if after_len == 0 {
            current_col >= before_end
        } else {
            current_col >= after_end
        };
        if done {
            break;
        }
    }

    (before, before_width, after, after_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_whitespace_char() {
        assert!(is_whitespace_char(" "));
        assert!(!is_whitespace_char("a"));
    }

    #[test]
    fn test_is_punctuation_char() {
        assert!(is_punctuation_char("."));
        assert!(is_punctuation_char("("));
        assert!(!is_punctuation_char("a"));
    }

    #[test]
    fn test_normalize_terminal_output_noop() {
        assert_eq!(normalize_terminal_output("hello"), "hello");
    }

    #[test]
    fn test_normalize_terminal_output_thai_am() {
        let out = normalize_terminal_output("\u{0e33}");
        assert_eq!(out, "\u{0e4d}\u{0e32}");
    }

    #[test]
    fn test_apply_background_to_line_pads() {
        let out = apply_background_to_line("hi", 5, |s| format!("[{s}]"));
        assert_eq!(out, "[hi   ]");
    }

    #[test]
    fn test_wrap_text_with_ansi_short_line_unchanged() {
        assert_eq!(wrap_text_with_ansi("hello", 20), vec!["hello".to_string()]);
    }

    #[test]
    fn test_wrap_text_with_ansi_wraps_on_word_boundary() {
        let wrapped = wrap_text_with_ansi("hello world foo", 5);
        assert_eq!(wrapped, vec!["hello", "world", "foo"]);
    }

    #[test]
    fn test_wrap_text_with_ansi_preserves_newlines() {
        let wrapped = wrap_text_with_ansi("a\nb", 20);
        assert_eq!(wrapped, vec!["a", "b"]);
    }

    #[test]
    fn test_wrap_text_with_ansi_breaks_long_word() {
        let wrapped = wrap_text_with_ansi("abcdefgh", 3);
        assert_eq!(wrapped, vec!["abc", "def", "gh"]);
    }

    #[test]
    fn test_wrap_text_with_ansi_carries_style_across_lines() {
        let wrapped = wrap_text_with_ansi("\x1b[1mhello world\x1b[0m", 5);
        assert!(
            wrapped[1].starts_with("\x1b[1m"),
            "continuation line should re-open bold: {:?}",
            wrapped[1]
        );
    }

    #[test]
    fn test_truncate_to_width_no_truncation_needed() {
        assert_eq!(truncate_to_width("hi", 10, "...", false), "hi");
    }

    #[test]
    fn test_truncate_to_width_basic() {
        assert_eq!(
            truncate_to_width("hello world", 8, "...", false),
            "hello\x1b[0m...\x1b[0m"
        );
    }

    #[test]
    fn test_truncate_to_width_pad() {
        let out = truncate_to_width("hi", 5, "", true);
        assert_eq!(out, "hi   ");
    }

    #[test]
    fn test_truncate_to_width_pad_when_truncated() {
        let out = truncate_to_width("hello world", 8, "", true);
        assert_eq!(out, "hello wo\x1b[0m");
    }

    #[test]
    fn test_truncate_to_width_zero_width() {
        assert_eq!(truncate_to_width("hello", 0, "...", false), "");
    }

    #[test]
    fn test_slice_by_column_basic() {
        assert_eq!(slice_by_column("hello world", 6, 5, false), "world");
    }

    #[test]
    fn test_slice_by_column_with_ansi() {
        let sliced = slice_by_column("\x1b[1mhello\x1b[0m world", 0, 5, false);
        assert!(sliced.starts_with("\x1b[1m"));
        assert!(sliced.contains("hello"));
    }

    #[test]
    fn test_slice_with_width_reports_width() {
        let (text, width) = slice_with_width("hello", 0, 3, false);
        assert_eq!(text, "hel");
        assert_eq!(width, 3);
    }

    #[test]
    fn test_extract_segments_before_and_after() {
        let (before, before_w, after, after_w) = extract_segments("0123456789", 3, 6, 4, false);
        assert_eq!(before, "012");
        assert_eq!(before_w, 3);
        assert_eq!(after, "6789");
        assert_eq!(after_w, 4);
    }

    #[test]
    fn test_extract_segments_after_inherits_style() {
        let (_, _, after, _) = extract_segments("\x1b[1m0123456789", 3, 6, 4, false);
        assert!(after.starts_with("\x1b[1m"));
    }
}

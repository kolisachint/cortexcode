//! Terminal cell-width calculation, grapheme-cluster aware.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `utils.ts`
//! (`visibleWidth`, `graphemeWidth`, `couldBeEmoji`).
//!
//! Simplification vs. the TS source: the TS code does a fast heuristic
//! pre-filter (`couldBeEmoji`) before running the exact `\p{RGI_Emoji}`
//! regex (an ECMAScript Unicode-set alias with no equivalent Rust crate).
//! Here the heuristic pre-filter *is* the classifier — codepoints/sequences
//! it flags as "could be emoji" are given width 2 directly. This is close
//! to the TS behavior for real-world emoji text and only diverges for
//! contrived non-emoji sequences that happen to fall in emoji code blocks.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use crate::ansi::extract_ansi_code;

fn could_be_emoji(segment: &str) -> bool {
    let Some(cp) = segment.chars().next().map(|c| c as u32) else {
        return false;
    };
    (0x1f000..=0x1fbff).contains(&cp)
        || (0x2300..=0x23ff).contains(&cp)
        || (0x2600..=0x27bf).contains(&cp)
        || (0x2b50..=0x2b55).contains(&cp)
        || segment.contains('\u{FE0F}')
        || segment.chars().count() > 2
}

/// Stand-in for the TS `^\p{RGI_Emoji}$` test, applied after the
/// `could_be_emoji` pre-filter. A lone codepoint is an RGI emoji exactly when
/// it has Emoji_Presentation, and those are all East Asian Wide, so a
/// text-presentation symbol like `✓` (U+2713) stays 1 column as in hoocode.
/// Sequences count when they carry an emoji-forming joiner: VS16, ZWJ, a skin
/// tone modifier, the keycap mark or tag characters.
fn looks_like_rgi_emoji(segment: &str, base: char) -> bool {
    if UnicodeWidthChar::width(base) == Some(2) {
        return true;
    }
    segment.chars().any(|c| {
        matches!(c as u32,
            0xFE0F | 0x200D | 0x1F3FB..=0x1F3FF | 0x20E3 | 0xE0020..=0xE007F)
    })
}

/// Codepoints treated as zero-width: combining marks, formatting/control
/// characters, and other default-ignorable code points.
fn is_zero_width_char(c: char) -> bool {
    matches!(c.width(), Some(0))
        || c.width().is_none()
        || is_combining_mark(c)
        || is_format_or_control(c)
}

fn is_combining_mark(c: char) -> bool {
    // Common combining-mark ranges (Mn/Me general categories, abbreviated —
    // covers the overwhelming majority of real-world combining diacritics).
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x05BF | 0x05C1..=0x05C2
        | 0x0610..=0x061A | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06DC | 0x06DF..=0x06E4
        | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F)
}

fn is_format_or_control(c: char) -> bool {
    let cp = c as u32;
    cp < 0x20
        || cp == 0x7f
        || (0x80..=0x9f).contains(&cp)
        || matches!(cp, 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0xFEFF)
}

pub(crate) fn grapheme_width(segment: &str) -> usize {
    if segment.chars().all(is_zero_width_char) {
        return 0;
    }

    let Some(base) = segment.chars().find(|c| !is_zero_width_char(*c)) else {
        return 0;
    };

    if could_be_emoji(segment) && looks_like_rgi_emoji(segment, base) {
        return 2;
    }

    // Regional indicators (flag halves) render full-width even in isolation.
    if (0x1F1E6..=0x1F1FF).contains(&(base as u32)) {
        return 2;
    }

    let mut width = UnicodeWidthChar::width(base).unwrap_or(0);

    // Trailing halfwidth/fullwidth forms and AM vowels that segment with a base.
    for c in segment.chars().skip(1) {
        match c as u32 {
            0xFF00..=0xFFEF => width += UnicodeWidthChar::width(c).unwrap_or(0),
            0x0E33 | 0x0EB3 => width += 1,
            _ => {}
        }
    }

    width
}

fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// Calculate the visible width of a string in terminal columns: ANSI escape
/// codes and tabs (expanded to 3 columns) don't count, wide/emoji graphemes
/// count as 2.
pub fn visible_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    if is_printable_ascii(s) {
        return s.len();
    }

    let mut clean = s.replace('\t', "   ");
    if clean.contains('\x1b') {
        let mut stripped = String::with_capacity(clean.len());
        let mut i = 0;
        while i < clean.len() {
            if let Some((_, len)) = extract_ansi_code(&clean, i) {
                i += len;
            } else {
                let ch_len = clean[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                stripped.push_str(&clean[i..i + ch_len]);
                i += ch_len;
            }
        }
        clean = stripped;
    }

    clean.graphemes(true).map(grapheme_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_visible_width_ascii() {
        assert_eq!(visible_width("hello"), 5);
    }

    #[test]
    fn test_visible_width_empty() {
        assert_eq!(visible_width(""), 0);
    }

    #[test]
    fn test_visible_width_strips_ansi() {
        assert_eq!(visible_width("\x1b[31mhello\x1b[0m"), 5);
    }

    #[test]
    fn test_visible_width_tab_expands_to_three() {
        assert_eq!(visible_width("\t"), 3);
    }

    #[test]
    fn test_visible_width_cjk_is_double() {
        assert_eq!(visible_width("你好"), 4);
    }

    #[test]
    fn test_visible_width_emoji_is_double() {
        assert_eq!(visible_width("😀"), 2);
    }

    #[test]
    fn test_visible_width_text_presentation_symbols_are_single() {
        // Not RGI emoji on their own: hoocode measures these as 1 column.
        assert_eq!(visible_width("\u{2713}"), 1);
        assert_eq!(visible_width("\u{2714}"), 1);
        assert_eq!(visible_width("\u{2022}"), 1);
        assert_eq!(visible_width("\u{23f5}"), 1);
        assert_eq!(visible_width("\u{2600}"), 1);
        assert_eq!(visible_width("\u{1f170}"), 1);
        assert_eq!(visible_width("\u{2713}\u{301}"), 1);
        // Emoji_Presentation singletons and VS16 sequences stay wide.
        assert_eq!(visible_width("\u{2705}"), 2);
        assert_eq!(visible_width("\u{231b}"), 2);
        assert_eq!(visible_width("\u{26a1}"), 2);
        assert_eq!(visible_width("\u{2b50}"), 2);
        assert_eq!(visible_width("\u{2714}\u{fe0f}"), 2);
        assert_eq!(visible_width("\u{1f44d}\u{1f3fd}"), 2);
    }

    #[test]
    fn test_visible_width_combining_mark_is_zero_extra() {
        // "e" + combining acute accent (U+0301) should still measure as 1 column.
        assert_eq!(visible_width("e\u{0301}"), 1);
    }

    #[test]
    fn test_visible_width_mixed_ansi_and_cjk() {
        assert_eq!(visible_width("\x1b[1m你好\x1b[0m"), 4);
    }
}

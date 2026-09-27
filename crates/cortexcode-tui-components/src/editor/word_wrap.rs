//! Word wrapping and paste-marker-aware segmentation for the editor, ported
//! from `components/editor.ts` (`wordWrapLine`, `segmentWithMarkers`).
//!
//! Indices are UTF-16 code units, like the JavaScript string indices the
//! editor's cursor arithmetic was written against; [`slice16`] and friends
//! convert at the edges.

use std::collections::HashSet;

use cortexcode_tui_util::{is_whitespace_char, visible_width};
use unicode_segmentation::UnicodeSegmentation;

/// UTF-16 length of `s` (JS `s.length`).
pub fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Byte offset of UTF-16 index `i` in `s`, rounded down to a char boundary
/// (an index inside a surrogate pair lands on the pair's start). Clamped to
/// `s.len()`.
pub fn byte_at16(s: &str, i: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in s.char_indices() {
        let next = units + ch.len_utf16();
        if next > i {
            return byte;
        }
        units = next;
    }
    s.len()
}

/// JS `s.slice(a, b)` on UTF-16 indices (clamped, empty when `b <= a`).
pub fn slice16(s: &str, a: usize, b: usize) -> &str {
    let start = byte_at16(s, a);
    let end = byte_at16(s, b);
    if end <= start {
        ""
    } else {
        &s[start..end]
    }
}

/// JS `s.slice(a)`.
pub fn slice16_from(s: &str, a: usize) -> &str {
    &s[byte_at16(s, a)..]
}

/// UTF-16 index of byte offset `b`.
pub fn index16(s: &str, b: usize) -> usize {
    len16(&s[..b])
}

/// One segment of text: a grapheme, or a whole paste marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub segment: String,
    /// UTF-16 index of the segment in the segmented text.
    pub index: usize,
}

/// A paste marker `[paste #N]`, `[paste #N +L lines]` or `[paste #N C chars]`
/// starting at byte `at`: its byte length and id.
fn match_paste_marker(text: &str, at: usize) -> Option<(usize, u64)> {
    let rest = text[at..].strip_prefix("[paste #")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let id: u64 = rest[..digits].parse().ok()?;
    let mut pos = digits;
    let tail = &rest[pos..];
    if let Some(after_space) = tail.strip_prefix(' ') {
        // ( (\+\d+ lines|\d+ chars))?
        let optional = if let Some(plus) = after_space.strip_prefix('+') {
            let n = plus.bytes().take_while(u8::is_ascii_digit).count();
            (n > 0 && plus[n..].starts_with(" lines")).then(|| 1 + 1 + n + " lines".len())
        } else {
            let n = after_space.bytes().take_while(u8::is_ascii_digit).count();
            (n > 0 && after_space[n..].starts_with(" chars")).then(|| 1 + n + " chars".len())
        };
        if let Some(len) = optional {
            if rest[pos + len..].starts_with(']') {
                pos += len;
            }
        }
    }
    if !rest[pos..].starts_with(']') {
        return None;
    }
    Some(("[paste #".len() + pos + 1, id))
}

/// Every paste marker in `text` (`PASTE_MARKER_REGEX`, global, non-overlapping):
/// `(byte start, byte end, id)`.
pub fn find_paste_markers(text: &str) -> Vec<(usize, usize, u64)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = text[i..].find("[paste #") {
        let at = i + rel;
        match match_paste_marker(text, at) {
            Some((len, id)) => {
                out.push((at, at + len, id));
                i = at + len;
            }
            None => i = at + 1,
        }
    }
    out
}

/// Whether a segment is a paste marker (merged by [`segment_with_markers`]).
pub fn is_paste_marker(segment: &str) -> bool {
    len16(segment) >= 10
        && match_paste_marker(segment, 0).is_some_and(|(len, _)| len == segment.len())
}

fn base_segments(text: &str) -> Vec<Segment> {
    let mut index = 0;
    text.graphemes(true)
        .map(|g| {
            let seg = Segment {
                segment: g.to_string(),
                index,
            };
            index += len16(g);
            seg
        })
        .collect()
}

/// Graphemes of `text`, with every paste marker whose id is in `valid_ids`
/// merged into one atomic segment (`segmentWithMarkers`).
pub fn segment_with_markers(text: &str, valid_ids: &HashSet<u64>) -> Vec<Segment> {
    if valid_ids.is_empty() || !text.contains("[paste #") {
        return base_segments(text);
    }
    let markers: Vec<(usize, usize)> = find_paste_markers(text)
        .into_iter()
        .filter(|(_, _, id)| valid_ids.contains(id))
        .map(|(s, e, _)| (index16(text, s), index16(text, e)))
        .collect();
    if markers.is_empty() {
        return base_segments(text);
    }
    let mut result = Vec::new();
    let mut marker_idx = 0;
    for seg in base_segments(text) {
        while marker_idx < markers.len() && markers[marker_idx].1 <= seg.index {
            marker_idx += 1;
        }
        match markers.get(marker_idx) {
            Some(&(start, end)) if seg.index >= start && seg.index < end => {
                if seg.index == start {
                    result.push(Segment {
                        segment: slice16(text, start, end).to_string(),
                        index: start,
                    });
                }
            }
            _ => result.push(seg),
        }
    }
    result
}

/// A chunk of a word-wrapped line (UTF-16 indices into the line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}

/// Split a line into word-wrapped chunks (`wordWrapLine`): at word boundaries
/// when possible, character-level for words wider than `max_width`.
pub fn word_wrap_line(line: &str, max_width: usize) -> Vec<TextChunk> {
    word_wrap_segments(line, max_width, None)
}

/// [`word_wrap_line`] with pre-segmented graphemes (e.g. marker-aware).
pub fn word_wrap_segments(
    line: &str,
    max_width: usize,
    pre_segmented: Option<Vec<Segment>>,
) -> Vec<TextChunk> {
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }
    let line_len = len16(line);
    if visible_width(line) <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: line_len,
        }];
    }

    let chunk = |a: usize, b: usize| TextChunk {
        text: slice16(line, a, b).to_string(),
        start_index: a,
        end_index: b,
    };
    let mut chunks = Vec::new();
    let segments = pre_segmented.unwrap_or_else(|| base_segments(line));

    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    // Wrap opportunity: the position after the last whitespace before a
    // non-whitespace grapheme.
    let mut wrap_opp_index: Option<usize> = None;
    let mut wrap_opp_width = 0usize;

    for (i, seg) in segments.iter().enumerate() {
        let grapheme = seg.segment.as_str();
        let g_width = visible_width(grapheme);
        let char_index = seg.index;
        let is_ws = !is_paste_marker(grapheme) && is_whitespace_char(grapheme);

        // Overflow check before advancing.
        if current_width + g_width > max_width {
            match wrap_opp_index {
                Some(opp) if current_width - wrap_opp_width + g_width <= max_width => {
                    // Backtrack to the last wrap opportunity.
                    chunks.push(chunk(chunk_start, opp));
                    chunk_start = opp;
                    current_width -= wrap_opp_width;
                }
                _ if chunk_start < char_index => {
                    // No viable wrap opportunity: force-break here.
                    chunks.push(chunk(chunk_start, char_index));
                    chunk_start = char_index;
                    current_width = 0;
                }
                _ => {}
            }
            wrap_opp_index = None;
        }

        if g_width > max_width {
            // A single atomic segment wider than max_width: re-wrap it at
            // grapheme granularity (still atomic for editing).
            let sub_segments = base_segments(grapheme);
            if sub_segments.len() <= 1 {
                // A single grapheme wider than the editor: emit it alone and
                // let it overflow; recursing would never terminate.
                let end = char_index + len16(grapheme);
                if chunk_start < char_index {
                    chunks.push(chunk(chunk_start, char_index));
                }
                chunks.push(TextChunk {
                    text: grapheme.to_string(),
                    start_index: char_index,
                    end_index: end,
                });
                chunk_start = end;
                current_width = 0;
                wrap_opp_index = None;
                continue;
            }
            let sub_chunks = word_wrap_segments(grapheme, max_width, Some(sub_segments));
            for sc in &sub_chunks[..sub_chunks.len() - 1] {
                chunks.push(TextChunk {
                    text: sc.text.clone(),
                    start_index: char_index + sc.start_index,
                    end_index: char_index + sc.end_index,
                });
            }
            let last = sub_chunks.last().unwrap();
            chunk_start = char_index + last.start_index;
            current_width = visible_width(&last.text);
            wrap_opp_index = None;
            continue;
        }

        current_width += g_width;

        // Whitespace followed by non-whitespace is where a break is allowed.
        if let Some(next) = segments.get(i + 1) {
            if is_ws && (is_paste_marker(&next.segment) || !is_whitespace_char(&next.segment)) {
                wrap_opp_index = Some(next.index);
                wrap_opp_width = current_width;
            }
        }
    }

    // Final chunk, unless an atomic oversized grapheme ended exactly at the end.
    if chunk_start < line_len || chunks.is_empty() {
        chunks.push(chunk(chunk_start, line_len));
    }
    chunks
}

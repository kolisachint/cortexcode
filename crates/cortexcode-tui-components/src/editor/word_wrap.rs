//! Word-aware line wrapping for layout purposes, ported from
//! `components/editor.ts`'s `wordWrapLine`.
//!
//! Unlike the original, paste-marker-aware atomic-segment merging
//! (`segmentWithMarkers`) is not ported — see the `editor` module docs —
//! so this always segments by plain grapheme cluster.

use cortexcode_tui_util::{is_whitespace_char, visible_width};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub text: String,
    pub start_index: usize,
    pub end_index: usize,
}

struct Seg<'a> {
    segment: &'a str,
    index: usize,
}

fn segments(line: &str) -> Vec<Seg<'_>> {
    line.grapheme_indices(true)
        .map(|(index, segment)| Seg { segment, index })
        .collect()
}

/// Split `line` into word-wrapped chunks of at most `max_width` visible
/// columns each, wrapping at word boundaries when possible and falling
/// back to grapheme-level wrapping for words longer than `max_width`.
pub fn word_wrap_line(line: &str, max_width: usize) -> Vec<TextChunk> {
    if line.is_empty() || max_width == 0 {
        return vec![TextChunk {
            text: String::new(),
            start_index: 0,
            end_index: 0,
        }];
    }

    let line_width = visible_width(line);
    if line_width <= max_width {
        return vec![TextChunk {
            text: line.to_string(),
            start_index: 0,
            end_index: line.len(),
        }];
    }

    let segs = segments(line);
    let mut chunks = Vec::new();

    let mut current_width = 0usize;
    let mut chunk_start = 0usize;
    let mut wrap_opp_index: Option<usize> = None;
    let mut wrap_opp_width = 0usize;

    let mut i = 0usize;
    while i < segs.len() {
        let seg = &segs[i];
        let grapheme = seg.segment;
        let g_width = visible_width(grapheme);
        let char_index = seg.index;
        let is_ws = is_whitespace_char(grapheme);

        if current_width + g_width > max_width {
            if let Some(opp_idx) = wrap_opp_index {
                if current_width.saturating_sub(wrap_opp_width) + g_width <= max_width {
                    chunks.push(TextChunk {
                        text: line[chunk_start..opp_idx].to_string(),
                        start_index: chunk_start,
                        end_index: opp_idx,
                    });
                    chunk_start = opp_idx;
                    current_width -= wrap_opp_width;
                } else if chunk_start < char_index {
                    chunks.push(TextChunk {
                        text: line[chunk_start..char_index].to_string(),
                        start_index: chunk_start,
                        end_index: char_index,
                    });
                    chunk_start = char_index;
                    current_width = 0;
                }
            } else if chunk_start < char_index {
                chunks.push(TextChunk {
                    text: line[chunk_start..char_index].to_string(),
                    start_index: chunk_start,
                    end_index: char_index,
                });
                chunk_start = char_index;
                current_width = 0;
            }
            wrap_opp_index = None;
        }

        if g_width > max_width {
            // Single grapheme wider than max_width: re-wrap at grapheme
            // granularity (logically the grapheme stays atomic for cursor
            // movement, but visually it must split).
            let sub_chunks = word_wrap_line(grapheme, max_width);
            for sc in &sub_chunks[..sub_chunks.len().saturating_sub(1)] {
                chunks.push(TextChunk {
                    text: sc.text.clone(),
                    start_index: char_index + sc.start_index,
                    end_index: char_index + sc.end_index,
                });
            }
            let last = &sub_chunks[sub_chunks.len() - 1];
            chunk_start = char_index + last.start_index;
            current_width = visible_width(&last.text);
            wrap_opp_index = None;
            i += 1;
            continue;
        }

        current_width += g_width;

        if let Some(next) = segs.get(i + 1) {
            if is_ws && !is_whitespace_char(next.segment) {
                wrap_opp_index = Some(next.index);
                wrap_opp_width = current_width;
            }
        }

        i += 1;
    }

    chunks.push(TextChunk {
        text: line[chunk_start..].to_string(),
        start_index: chunk_start,
        end_index: line.len(),
    });

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_line_is_one_chunk() {
        let chunks = word_wrap_line("hello", 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "hello");
    }

    #[test]
    fn empty_line_returns_single_empty_chunk() {
        let chunks = word_wrap_line("", 20);
        assert_eq!(
            chunks,
            vec![TextChunk {
                text: String::new(),
                start_index: 0,
                end_index: 0
            }]
        );
    }

    #[test]
    fn force_breaks_long_word() {
        let chunks = word_wrap_line("abcdefghij", 4);
        assert_eq!(chunks[0].text, "abcd");
        assert_eq!(chunks[1].text, "efgh");
        assert_eq!(chunks[2].text, "ij");
    }

    #[test]
    fn chunk_indices_reconstruct_the_original_line() {
        let line = "aaaa bbbb";
        let chunks = word_wrap_line(line, 4);
        let reconstructed: String = chunks
            .iter()
            .map(|c| &line[c.start_index..c.end_index])
            .collect();
        assert_eq!(reconstructed, line);
        assert_eq!(chunks[0].start_index, 0);
        assert_eq!(chunks[0].end_index, 4);
        assert_eq!(chunks.last().unwrap().end_index, line.len());
    }

    // The following cases are ported directly from hoocode's
    // `editor.test.ts` `wordWrapLine` suite (ground truth for this
    // intentionally subtle backtracking algorithm).

    #[test]
    fn wraps_word_that_would_overflow_with_trailing_space() {
        let chunks = word_wrap_line("hello world test", 11);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "hello ");
        assert_eq!(chunks[1].text, "world test");
    }

    #[test]
    fn keeps_whitespace_at_terminal_width_boundary_on_same_line() {
        let chunks = word_wrap_line("hello world test", 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "hello world ");
        assert_eq!(chunks[1].text, "test");
    }

    #[test]
    fn handles_unbreakable_word_filling_width_exactly_followed_by_space() {
        let chunks = word_wrap_line("aaaaaaaaaaaa aaaa", 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "aaaaaaaaaaaa");
        assert_eq!(chunks[1].text, " aaaa");
    }

    #[test]
    fn wraps_word_to_next_line_when_it_fits_width_but_not_remaining_space() {
        let chunks = word_wrap_line("      aaaaaaaaaaaa", 12);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "      ");
        assert_eq!(chunks[1].text, "aaaaaaaaaaaa");
    }

    #[test]
    fn keeps_word_with_multi_space_and_following_word_together_when_they_fit() {
        let chunks = word_wrap_line("Lorem ipsum dolor sit amet,    consectetur", 30);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
        assert_eq!(chunks[1].text, "amet,    consectetur");
    }

    #[test]
    fn splits_when_word_plus_multi_space_plus_word_exceeds_width() {
        let chunks = word_wrap_line("Lorem ipsum dolor sit amet,               consectetur", 30);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].text, "Lorem ipsum dolor sit ");
        assert_eq!(chunks[1].text, "amet,               ");
        assert_eq!(chunks[2].text, "consectetur");
    }

    #[test]
    fn reconstructs_original_line_from_chunk_indices() {
        let line = " ".to_string() + &"a".repeat(186) + "\u{4f60}";
        let chunks = word_wrap_line(&line, 187);
        for chunk in &chunks {
            assert!(visible_width(&chunk.text) <= 187);
        }
        let reconstructed: String = chunks
            .iter()
            .map(|c| &line[c.start_index..c.end_index])
            .collect();
        assert_eq!(reconstructed, line);
    }

    #[test]
    fn zero_width_returns_empty_chunk() {
        let chunks = word_wrap_line("hello", 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "");
    }
}

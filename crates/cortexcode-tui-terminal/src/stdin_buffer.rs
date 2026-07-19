//! Buffers stdin input and emits complete escape sequences.
//!
//! This is necessary because stdin data can arrive in partial chunks,
//! especially for escape sequences like mouse events. Without buffering,
//! partial sequences can be misinterpreted as regular keypresses.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` -> `stdin-buffer.ts`,
//! itself based on code from OpenTUI (https://github.com/anomalyco/opentui),
//! MIT License, Copyright (c) 2025 opentui.

use std::time::{Duration, Instant};

const ESC: char = '\x1b';
const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// An event produced by [`StdinBuffer::process`] / [`StdinBuffer::poll_timeout`] / [`StdinBuffer::flush`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdinEvent {
    /// A complete input sequence (single character or escape sequence).
    Data(String),
    /// Content captured between bracketed-paste markers.
    Paste(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeqStatus {
    Complete,
    Incomplete,
    NotEscape,
}

fn is_complete_sequence(data: &[char]) -> SeqStatus {
    if data.first() != Some(&ESC) {
        return SeqStatus::NotEscape;
    }
    if data.len() == 1 {
        return SeqStatus::Incomplete;
    }

    let after_esc = &data[1..];
    match after_esc[0] {
        '[' => {
            // Old-style mouse sequence: ESC[M + 3 bytes = 6 total
            if after_esc.len() >= 2 && after_esc[1] == 'M' {
                return if data.len() >= 6 {
                    SeqStatus::Complete
                } else {
                    SeqStatus::Incomplete
                };
            }
            is_complete_csi_sequence(data)
        }
        ']' => is_complete_osc_sequence(data),
        'P' => is_complete_dcs_sequence(data),
        '_' => is_complete_apc_sequence(data),
        'O' => {
            if after_esc.len() >= 2 {
                SeqStatus::Complete
            } else {
                SeqStatus::Incomplete
            }
        }
        _ if after_esc.len() == 1 => SeqStatus::Complete,
        _ => SeqStatus::Complete,
    }
}

fn is_complete_csi_sequence(data: &[char]) -> SeqStatus {
    if data.len() < 2 || data[0] != ESC || data[1] != '[' {
        return SeqStatus::Complete;
    }
    if data.len() < 3 {
        return SeqStatus::Incomplete;
    }

    let payload = &data[2..];
    let last_char = *payload.last().unwrap();
    let last_char_code = last_char as u32;

    if (0x40..=0x7e).contains(&last_char_code) {
        if payload[0] == '<' {
            // SGR mouse sequence: <digits;digits;digits[Mm]
            let inner = &payload[1..payload.len() - 1];
            let inner_str: String = inner.iter().collect();
            let parts: Vec<&str> = inner_str.split(';').collect();
            let is_mouse = parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
            return if is_mouse {
                SeqStatus::Complete
            } else {
                SeqStatus::Incomplete
            };
        }
        return SeqStatus::Complete;
    }

    SeqStatus::Incomplete
}

fn is_complete_osc_sequence(data: &[char]) -> SeqStatus {
    if data.len() < 2 || data[0] != ESC || data[1] != ']' {
        return SeqStatus::Complete;
    }
    if ends_with(data, &[ESC, '\\']) || ends_with(data, &['\x07']) {
        SeqStatus::Complete
    } else {
        SeqStatus::Incomplete
    }
}

fn is_complete_dcs_sequence(data: &[char]) -> SeqStatus {
    if data.len() < 2 || data[0] != ESC || data[1] != 'P' {
        return SeqStatus::Complete;
    }
    if ends_with(data, &[ESC, '\\']) {
        SeqStatus::Complete
    } else {
        SeqStatus::Incomplete
    }
}

fn is_complete_apc_sequence(data: &[char]) -> SeqStatus {
    if data.len() < 2 || data[0] != ESC || data[1] != '_' {
        return SeqStatus::Complete;
    }
    if ends_with(data, &[ESC, '\\']) {
        SeqStatus::Complete
    } else {
        SeqStatus::Incomplete
    }
}

fn ends_with(data: &[char], suffix: &[char]) -> bool {
    data.len() >= suffix.len() && &data[data.len() - suffix.len()..] == suffix
}

/// Parses the codepoint of an unmodified Kitty "printable" CSI-u sequence,
/// e.g. `\x1b[97u` (press) but not `\x1b[97;3u` (with modifiers).
fn parse_unmodified_kitty_printable_codepoint(sequence: &[char]) -> Option<u32> {
    let s: String = sequence.iter().collect();
    let rest = s.strip_prefix("\x1b[")?;
    let rest = rest.strip_suffix('u')?;
    // Optional `:digits` and `:digits` suffixes are not allowed here (that's a modified event).
    let first_segment = rest.split(':').next().unwrap_or(rest);
    if first_segment.len() != rest.len() {
        return None;
    }
    if first_segment.is_empty() || !first_segment.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let codepoint: u32 = first_segment.parse().ok()?;
    if codepoint >= 32 {
        Some(codepoint)
    } else {
        None
    }
}

fn extract_complete_sequences(buffer: &[char]) -> (Vec<String>, Vec<char>) {
    let mut sequences = Vec::new();
    let mut pos = 0usize;

    while pos < buffer.len() {
        let remaining = &buffer[pos..];

        if remaining[0] == ESC {
            let mut seq_end = 1usize;
            loop {
                if seq_end > remaining.len() {
                    return (sequences, remaining.to_vec());
                }
                let candidate = &remaining[..seq_end];
                match is_complete_sequence(candidate) {
                    SeqStatus::Complete => {
                        sequences.push(candidate.iter().collect());
                        pos += seq_end;
                        break;
                    }
                    SeqStatus::Incomplete => {
                        seq_end += 1;
                    }
                    SeqStatus::NotEscape => {
                        // Should not happen when starting with ESC.
                        sequences.push(candidate.iter().collect());
                        pos += seq_end;
                        break;
                    }
                }
            }
        } else {
            sequences.push(remaining[0].to_string());
            pos += 1;
        }
    }

    (sequences, Vec::new())
}

/// Options for [`StdinBuffer`].
#[derive(Debug, Clone, Copy)]
pub struct StdinBufferOptions {
    /// Maximum time to wait for sequence completion before flushing anyway.
    pub timeout: Duration,
}

impl Default for StdinBufferOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(10),
        }
    }
}

/// Buffers stdin input and yields complete sequences.
///
/// Handles partial escape sequences that arrive across multiple chunks, and
/// bracketed-paste content re-assembly. Unlike the TypeScript original this
/// is a pure state machine: instead of firing a JS timer internally, callers
/// must periodically invoke [`StdinBuffer::poll_timeout`] (e.g. from the
/// thread reading stdin) so idle partial sequences get flushed.
pub struct StdinBuffer {
    buffer: Vec<char>,
    timeout: Duration,
    pending_since: Option<Instant>,
    paste_mode: bool,
    paste_buffer: String,
    pending_kitty_printable_codepoint: Option<u32>,
}

impl StdinBuffer {
    pub fn new(options: StdinBufferOptions) -> Self {
        Self {
            buffer: Vec::new(),
            timeout: options.timeout,
            pending_since: None,
            paste_mode: false,
            paste_buffer: String::new(),
            pending_kitty_printable_codepoint: None,
        }
    }

    /// Feed raw stdin data through the buffer, returning any events it produced.
    pub fn process(&mut self, data: &str) -> Vec<StdinEvent> {
        self.pending_since = None;
        let mut out = Vec::new();
        self.process_inner(data, &mut out);
        out
    }

    fn process_inner(&mut self, data: &str, out: &mut Vec<StdinEvent>) {
        if data.is_empty() && self.buffer.is_empty() {
            self.emit_data_sequence(String::new(), out);
            return;
        }

        self.buffer.extend(data.chars());

        if self.paste_mode {
            let appended: String = self.buffer.iter().collect();
            self.paste_buffer.push_str(&appended);
            self.buffer.clear();

            if let Some(end_idx) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted = self.paste_buffer[..end_idx].to_string();
                let remaining =
                    self.paste_buffer[end_idx + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                out.push(StdinEvent::Paste(pasted));

                if !remaining.is_empty() {
                    self.process_inner(&remaining, out);
                }
            }
            return;
        }

        let buffer_str: String = self.buffer.iter().collect();
        if let Some(start_idx) = buffer_str.find(BRACKETED_PASTE_START) {
            if start_idx > 0 {
                let before_paste: Vec<char> = buffer_str[..start_idx].chars().collect();
                let (sequences, _) = extract_complete_sequences(&before_paste);
                for sequence in sequences {
                    self.emit_data_sequence(sequence, out);
                }
            }

            self.pending_kitty_printable_codepoint = None;
            let after_start = buffer_str[start_idx + BRACKETED_PASTE_START.len()..].to_string();
            self.buffer.clear();
            self.paste_mode = true;
            self.paste_buffer = after_start;

            if let Some(end_idx) = self.paste_buffer.find(BRACKETED_PASTE_END) {
                let pasted = self.paste_buffer[..end_idx].to_string();
                let remaining =
                    self.paste_buffer[end_idx + BRACKETED_PASTE_END.len()..].to_string();

                self.paste_mode = false;
                self.paste_buffer.clear();
                self.pending_kitty_printable_codepoint = None;

                out.push(StdinEvent::Paste(pasted));

                if !remaining.is_empty() {
                    self.process_inner(&remaining, out);
                }
            }
            return;
        }

        let (sequences, remainder) = extract_complete_sequences(&self.buffer);
        self.buffer = remainder;

        for sequence in sequences {
            self.emit_data_sequence(sequence, out);
        }

        if !self.buffer.is_empty() {
            self.pending_since = Some(Instant::now());
        }
    }

    fn emit_data_sequence(&mut self, sequence: String, out: &mut Vec<StdinEvent>) {
        let raw_codepoint = {
            let mut chars = sequence.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c as u32),
                _ => None,
            }
        };
        if let (Some(raw), Some(pending)) = (raw_codepoint, self.pending_kitty_printable_codepoint)
        {
            if raw == pending {
                self.pending_kitty_printable_codepoint = None;
                return;
            }
        }

        let seq_chars: Vec<char> = sequence.chars().collect();
        self.pending_kitty_printable_codepoint =
            parse_unmodified_kitty_printable_codepoint(&seq_chars);
        out.push(StdinEvent::Data(sequence));
    }

    /// Call periodically (e.g. from the reader loop) so an idle partial
    /// sequence gets flushed after the configured timeout.
    pub fn poll_timeout(&mut self, now: Instant) -> Vec<StdinEvent> {
        match self.pending_since {
            Some(since) if now.duration_since(since) >= self.timeout => self.flush(),
            _ => Vec::new(),
        }
    }

    /// Immediately flush any buffered (incomplete) sequence.
    pub fn flush(&mut self) -> Vec<StdinEvent> {
        self.pending_since = None;
        if self.buffer.is_empty() {
            return Vec::new();
        }
        let sequence: String = self.buffer.iter().collect();
        self.buffer.clear();
        self.pending_kitty_printable_codepoint = None;
        vec![StdinEvent::Data(sequence)]
    }

    /// Discard buffered content without emitting anything.
    pub fn clear(&mut self) {
        self.pending_since = None;
        self.buffer.clear();
        self.paste_mode = false;
        self.paste_buffer.clear();
        self.pending_kitty_printable_codepoint = None;
    }

    pub fn buffer_contents(&self) -> String {
        self.buffer.iter().collect()
    }

    pub fn destroy(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_strings(events: &[StdinEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                StdinEvent::Data(s) => Some(s.clone()),
                StdinEvent::Paste(_) => None,
            })
            .collect()
    }

    fn paste_strings(events: &[StdinEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                StdinEvent::Paste(s) => Some(s.clone()),
                StdinEvent::Data(_) => None,
            })
            .collect()
    }

    #[test]
    fn passes_through_regular_characters_immediately() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("a");
        assert_eq!(data_strings(&ev), vec!["a"]);
    }

    #[test]
    fn passes_through_multiple_regular_characters() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("abc");
        assert_eq!(data_strings(&ev), vec!["a", "b", "c"]);
    }

    #[test]
    fn handles_unicode_characters() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("hello 世界");
        assert_eq!(
            data_strings(&ev),
            vec!["h", "e", "l", "l", "o", " ", "世", "界"]
        );
    }

    #[test]
    fn passes_through_complete_mouse_sgr_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[<35;20;5m");
        assert_eq!(data_strings(&ev), vec!["\x1b[<35;20;5m"]);
    }

    #[test]
    fn passes_through_complete_arrow_key_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[A");
        assert_eq!(data_strings(&ev), vec!["\x1b[A"]);
    }

    #[test]
    fn passes_through_complete_function_key_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[11~");
        assert_eq!(data_strings(&ev), vec!["\x1b[11~"]);
    }

    #[test]
    fn passes_through_meta_key_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1ba");
        assert_eq!(data_strings(&ev), vec!["\x1ba"]);
    }

    #[test]
    fn passes_through_ss3_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1bOA");
        assert_eq!(data_strings(&ev), vec!["\x1bOA"]);
    }

    #[test]
    fn buffers_incomplete_mouse_sgr_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b").is_empty());
        assert_eq!(b.buffer_contents(), "\x1b");

        assert!(b.process("[<35").is_empty());
        assert_eq!(b.buffer_contents(), "\x1b[<35");

        let ev = b.process(";20;5m");
        assert_eq!(data_strings(&ev), vec!["\x1b[<35;20;5m"]);
        assert_eq!(b.buffer_contents(), "");
    }

    #[test]
    fn buffers_incomplete_csi_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[").is_empty());
        assert!(b.process("1;").is_empty());
        let ev = b.process("5H");
        assert_eq!(data_strings(&ev), vec!["\x1b[1;5H"]);
    }

    #[test]
    fn buffers_split_across_many_chunks() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let mut all = Vec::new();
        for ch in ["\x1b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"] {
            all.extend(b.process(ch));
        }
        assert_eq!(data_strings(&all), vec!["\x1b[<35;20;5m"]);
    }

    #[test]
    fn flushes_incomplete_sequence_after_timeout() {
        let mut b = StdinBuffer::new(StdinBufferOptions {
            timeout: Duration::from_millis(10),
        });
        assert!(b.process("\x1b[<35").is_empty());
        let ev = b.poll_timeout(Instant::now() + Duration::from_millis(15));
        assert_eq!(data_strings(&ev), vec!["\x1b[<35"]);
    }

    #[test]
    fn handles_characters_followed_by_escape_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("abc\x1b[A");
        assert_eq!(data_strings(&ev), vec!["a", "b", "c", "\x1b[A"]);
    }

    #[test]
    fn handles_escape_sequence_followed_by_characters() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[Aabc");
        assert_eq!(data_strings(&ev), vec!["\x1b[A", "a", "b", "c"]);
    }

    #[test]
    fn handles_multiple_complete_sequences() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[A\x1b[B\x1b[C");
        assert_eq!(data_strings(&ev), vec!["\x1b[A", "\x1b[B", "\x1b[C"]);
    }

    #[test]
    fn handles_partial_sequence_with_preceding_characters() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("abc\x1b[<35");
        assert_eq!(data_strings(&ev), vec!["a", "b", "c"]);
        assert_eq!(b.buffer_contents(), "\x1b[<35");

        let ev2 = b.process(";20;5m");
        assert_eq!(data_strings(&ev2), vec!["\x1b[<35;20;5m"]);
    }

    #[test]
    fn kitty_csi_u_press_and_release() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert_eq!(data_strings(&b.process("\x1b[97u")), vec!["\x1b[97u"]);
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert_eq!(
            data_strings(&b.process("\x1b[97;1:3u")),
            vec!["\x1b[97;1:3u"]
        );
    }

    #[test]
    fn kitty_batched_press_and_release() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[97u\x1b[97;1:3u");
        assert_eq!(data_strings(&ev), vec!["\x1b[97u", "\x1b[97;1:3u"]);
    }

    #[test]
    fn kitty_multiple_batched_events() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[97u\x1b[97;1:3u\x1b[98u\x1b[98;1:3u");
        assert_eq!(
            data_strings(&ev),
            vec!["\x1b[97u", "\x1b[97;1:3u", "\x1b[98u", "\x1b[98;1:3u"]
        );
    }

    #[test]
    fn kitty_arrow_keys_with_event_type() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[1;1:1A");
        assert_eq!(data_strings(&ev), vec!["\x1b[1;1:1A"]);
    }

    #[test]
    fn kitty_functional_keys_with_event_type() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[3;1:3~");
        assert_eq!(data_strings(&ev), vec!["\x1b[3;1:3~"]);
    }

    #[test]
    fn kitty_plain_characters_mixed_with_kitty_sequences() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("a\x1b[97;1:3u");
        assert_eq!(data_strings(&ev), vec!["a", "\x1b[97;1:3u"]);
    }

    #[test]
    fn drops_raw_duplicate_character_after_matching_kitty_printable_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[224u\u{e0}");
        assert_eq!(data_strings(&ev), vec!["\x1b[224u"]);
    }

    #[test]
    fn drops_raw_duplicate_character_across_chunks() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let mut ev = b.process("\x1b[64u");
        ev.extend(b.process("@"));
        assert_eq!(data_strings(&ev), vec!["\x1b[64u"]);
    }

    #[test]
    fn keeps_non_matching_plain_character_after_kitty_printable_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[97ub");
        assert_eq!(data_strings(&ev), vec!["\x1b[97u", "b"]);
    }

    #[test]
    fn keeps_raw_character_after_modified_kitty_printable_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[64;3u@");
        assert_eq!(data_strings(&ev), vec!["\x1b[64;3u", "@"]);
    }

    #[test]
    fn mouse_press_release_move_events() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert_eq!(
            data_strings(&b.process("\x1b[<0;10;5M")),
            vec!["\x1b[<0;10;5M"]
        );
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert_eq!(
            data_strings(&b.process("\x1b[<0;10;5m")),
            vec!["\x1b[<0;10;5m"]
        );
    }

    #[test]
    fn split_mouse_events() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let mut all = Vec::new();
        for chunk in ["\x1b[<3", "5;1", "5;", "10m"] {
            all.extend(b.process(chunk));
        }
        assert_eq!(data_strings(&all), vec!["\x1b[<35;15;10m"]);
    }

    #[test]
    fn multiple_mouse_events() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[<35;1;1m\x1b[<35;2;2m\x1b[<35;3;3m");
        assert_eq!(
            data_strings(&ev),
            vec!["\x1b[<35;1;1m", "\x1b[<35;2;2m", "\x1b[<35;3;3m"]
        );
    }

    #[test]
    fn old_style_mouse_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[M abc");
        assert_eq!(data_strings(&ev), vec!["\x1b[M ab", "c"]);
    }

    #[test]
    fn buffers_incomplete_old_style_mouse_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[M").is_empty());
        assert_eq!(b.buffer_contents(), "\x1b[M");
        assert!(b.process(" a").is_empty());
        assert_eq!(b.buffer_contents(), "\x1b[M a");
        let ev = b.process("b");
        assert_eq!(data_strings(&ev), vec!["\x1b[M ab"]);
    }

    #[test]
    fn handles_empty_input() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("");
        assert_eq!(data_strings(&ev), vec![""]);
    }

    #[test]
    fn handles_lone_escape_character_with_timeout() {
        let mut b = StdinBuffer::new(StdinBufferOptions {
            timeout: Duration::from_millis(10),
        });
        assert!(b.process("\x1b").is_empty());
        let ev = b.poll_timeout(Instant::now() + Duration::from_millis(15));
        assert_eq!(data_strings(&ev), vec!["\x1b"]);
    }

    #[test]
    fn handles_lone_escape_character_with_explicit_flush() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b").is_empty());
        let flushed = b.flush();
        assert_eq!(data_strings(&flushed), vec!["\x1b"]);
    }

    #[test]
    fn handles_very_long_sequences() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let long_seq = format!("\x1b[{}H", "1;".repeat(50));
        let ev = b.process(&long_seq);
        assert_eq!(data_strings(&ev), vec![long_seq]);
    }

    #[test]
    fn flush_returns_incomplete_sequence() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[<35").is_empty());
        let flushed = b.flush();
        assert_eq!(data_strings(&flushed), vec!["\x1b[<35"]);
        assert_eq!(b.buffer_contents(), "");
    }

    #[test]
    fn flush_returns_empty_when_nothing_buffered() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.flush().is_empty());
    }

    #[test]
    fn clear_discards_buffered_content_without_emitting() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[<35").is_empty());
        assert_eq!(b.buffer_contents(), "\x1b[<35");
        b.clear();
        assert_eq!(b.buffer_contents(), "");
    }

    #[test]
    fn emits_paste_event_for_complete_bracketed_paste() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[200~hello world\x1b[201~");
        assert_eq!(paste_strings(&ev), vec!["hello world"]);
        assert!(data_strings(&ev).is_empty());
    }

    #[test]
    fn handles_paste_arriving_in_chunks() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[200~").is_empty());
        assert!(b.process("hello ").is_empty());
        let ev = b.process("world\x1b[201~");
        assert_eq!(paste_strings(&ev), vec!["hello world"]);
    }

    #[test]
    fn handles_paste_with_input_before_and_after() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let mut all = Vec::new();
        all.extend(b.process("a"));
        all.extend(b.process("\x1b[200~pasted\x1b[201~"));
        all.extend(b.process("b"));
        assert_eq!(data_strings(&all), vec!["a", "b"]);
        assert_eq!(paste_strings(&all), vec!["pasted"]);
    }

    #[test]
    fn handles_paste_with_newlines() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[200~line1\nline2\nline3\x1b[201~");
        assert_eq!(paste_strings(&ev), vec!["line1\nline2\nline3"]);
    }

    #[test]
    fn handles_paste_with_unicode() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        let ev = b.process("\x1b[200~Hello 世界 🎉\x1b[201~");
        assert_eq!(paste_strings(&ev), vec!["Hello 世界 🎉"]);
    }

    #[test]
    fn destroy_clears_buffer() {
        let mut b = StdinBuffer::new(StdinBufferOptions::default());
        assert!(b.process("\x1b[<35").is_empty());
        b.destroy();
        assert_eq!(b.buffer_contents(), "");
    }
}

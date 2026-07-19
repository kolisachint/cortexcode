//! Minimal Server-Sent-Events line reader.
//!
//! Anthropic's streaming Messages API sends standard SSE frames (`event:` +
//! `data:` lines separated by a blank line). We only need the `data:`
//! payload — its JSON body already carries a `"type"` field identifying the
//! event, so the `event:` line is redundant and can be ignored.

use std::io::BufRead;

/// Iterator over SSE `data:` payloads read from a buffered byte stream.
pub struct SseEvents<R: BufRead> {
    reader: R,
}

impl<R: BufRead> SseEvents<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

impl<R: BufRead> Iterator for SseEvents<R> {
    /// The concatenated `data:` payload for one SSE frame.
    type Item = std::io::Result<String>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut data_lines: Vec<String> = Vec::new();

        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    return if data_lines.is_empty() {
                        None
                    } else {
                        Some(Ok(data_lines.join("\n")))
                    };
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        if !data_lines.is_empty() {
                            return Some(Ok(data_lines.join("\n")));
                        }
                        continue;
                    }
                    if let Some(rest) = trimmed.strip_prefix("data:") {
                        data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                    }
                    // `event:`, `id:`, `retry:`, and `:comment` lines are ignored —
                    // the JSON payload's own `type` field is authoritative.
                }
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_single_event() {
        let raw = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        let mut events = SseEvents::new(Cursor::new(raw));
        assert_eq!(
            events.next().unwrap().unwrap(),
            "{\"type\":\"message_stop\"}"
        );
        assert!(events.next().is_none());
    }

    #[test]
    fn test_multiple_events() {
        let raw = "event: a\ndata: {\"x\":1}\n\nevent: b\ndata: {\"x\":2}\n\n";
        let mut events = SseEvents::new(Cursor::new(raw));
        assert_eq!(events.next().unwrap().unwrap(), "{\"x\":1}");
        assert_eq!(events.next().unwrap().unwrap(), "{\"x\":2}");
        assert!(events.next().is_none());
    }

    #[test]
    fn test_multiline_data() {
        let raw = "data: line1\ndata: line2\n\n";
        let mut events = SseEvents::new(Cursor::new(raw));
        assert_eq!(events.next().unwrap().unwrap(), "line1\nline2");
    }

    #[test]
    fn test_trailing_event_without_blank_line() {
        let raw = "data: {\"x\":1}\n";
        let mut events = SseEvents::new(Cursor::new(raw));
        assert_eq!(events.next().unwrap().unwrap(), "{\"x\":1}");
        assert!(events.next().is_none());
    }

    #[test]
    fn test_ignores_comments_and_ids() {
        let raw = ": keepalive\nid: 1\ndata: {\"x\":1}\n\n";
        let mut events = SseEvents::new(Cursor::new(raw));
        assert_eq!(events.next().unwrap().unwrap(), "{\"x\":1}");
    }

    #[test]
    fn test_empty_input() {
        let mut events = SseEvents::new(Cursor::new(""));
        assert!(events.next().is_none());
    }
}

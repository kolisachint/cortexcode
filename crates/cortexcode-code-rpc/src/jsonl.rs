//! `modes/rpc/jsonl.ts`: strict JSONL framing for RPC mode.
//!
//! Records are split on LF only. U+2028 / U+2029 are valid inside JSON strings
//! and must not end a record (why hoocode avoids Node's `readline`).

/// `serializeJsonLine`: `JSON.stringify(value) + "\n"`. serde_json, like
/// `JSON.stringify`, leaves U+2028 / U+2029 unescaped.
pub fn serialize_json_line(value: &serde_json::Value) -> String {
    format!("{value}\n")
}

/// `attachJsonlLineReader` without the stream: feed bytes with [`push`], call
/// [`finish`] at end of input.
///
/// [`push`]: JsonlLineReader::push
/// [`finish`]: JsonlLineReader::finish
#[derive(Debug, Default)]
pub struct JsonlLineReader {
    buffer: Vec<u8>,
    /// Cap (in UTF-16 code units, as hoocode counts) on an unterminated line;
    /// a longer line is dropped whole. `None`: unbounded (trusted peers).
    max_buffer: Option<usize>,
    /// Skipping the rest of a line that overflowed `max_buffer`.
    discarding: bool,
}

impl JsonlLineReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// `JsonlLineReaderOptions.maxBuffer`.
    pub fn with_max_buffer(max_buffer: usize) -> Self {
        Self {
            max_buffer: Some(max_buffer),
            ..Self::default()
        }
    }

    /// Add a chunk; returns the lines it completed (a trailing `\r` stripped).
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            if self.discarding {
                // The tail of an oversized line that was already dropped.
                self.discarding = false;
                continue;
            }
            lines.push(Self::decode(&line[..line.len() - 1]));
        }
        if let Some(max) = self.max_buffer {
            if String::from_utf8_lossy(&self.buffer).encode_utf16().count() > max {
                self.buffer.clear();
                self.discarding = true;
            }
        }
        lines
    }

    /// End of input: the last line if it had no trailing LF.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buffer);
        let discarding = std::mem::take(&mut self.discarding);
        (!rest.is_empty() && !discarding).then(|| Self::decode(&rest))
    }

    fn decode(bytes: &[u8]) -> String {
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // rpc-jsonl.test.ts

    #[test]
    fn serializes_strict_jsonl_records_without_escaping_unicode_separators() {
        let line = serialize_json_line(&json!({"text": "a\u{2028}b\u{2029}c"}));
        assert!(line.contains("a\u{2028}b\u{2029}c"));
        assert!(line.ends_with('\n'));
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed, json!({"text": "a\u{2028}b\u{2029}c"}));
    }

    #[test]
    fn splits_on_lf_only_and_preserves_u2028_u2029_inside_payloads() {
        let mut reader = JsonlLineReader::new();
        let mut lines =
            reader.push(serialize_json_line(&json!({"text": "a\u{2028}b\u{2029}c"})).as_bytes());
        lines.extend(reader.finish());
        assert_eq!(lines.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(parsed, json!({"text": "a\u{2028}b\u{2029}c"}));
    }

    #[test]
    fn handles_crlf_delimited_input() {
        let mut reader = JsonlLineReader::new();
        let mut lines = reader.push(b"{\"a\":1}\r\n{\"b\":2}\r\n");
        lines.extend(reader.finish());
        assert_eq!(lines, ["{\"a\":1}", "{\"b\":2}"]);
    }

    #[test]
    fn emits_a_final_line_without_trailing_lf() {
        let mut reader = JsonlLineReader::new();
        let mut lines = reader.push(b"{\"a\":1}");
        lines.extend(reader.finish());
        assert_eq!(lines, ["{\"a\":1}"]);
    }

    #[test]
    fn joins_lines_and_utf8_split_across_chunks() {
        let mut reader = JsonlLineReader::new();
        let bytes = "{\"t\":\"é\"}\n".as_bytes();
        assert!(reader.push(&bytes[..7]).is_empty());
        assert_eq!(reader.push(&bytes[7..]), ["{\"t\":\"é\"}"]);
        assert_eq!(reader.finish(), None);
    }

    #[test]
    fn drops_a_line_that_overflows_max_buffer() {
        let mut reader = JsonlLineReader::with_max_buffer(4);
        assert!(reader.push(b"123456").is_empty());
        assert_eq!(reader.push(b"789\nok\n"), ["ok"]);
        assert!(reader.push(b"toolong").is_empty());
        assert_eq!(reader.finish(), None);
    }
}

//! `core/tools/output-accumulator.ts`: streaming output with bounded memory.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use cortexcode_agent_harness::utils::output_compression::compress_bash_output;
use cortexcode_code_tool_api::{
    truncate_tail, TruncatedBy, TruncationOptions, TruncationResult, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_LINES,
};

/// A streaming UTF-8 decoder (`TextDecoder` with `stream: true`): invalid
/// sequences become U+FFFD, and a character split across chunks is held
/// until its remaining bytes arrive.
#[derive(Debug, Default)]
pub struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode `data`; with `stream`, an incomplete trailing character waits
    /// for the next call.
    pub fn decode(&mut self, data: &[u8], stream: bool) -> String {
        self.pending.extend_from_slice(data);
        let bytes = std::mem::take(&mut self.pending);
        let mut out = String::with_capacity(bytes.len());
        let mut rest = &bytes[..];
        loop {
            match std::str::from_utf8(rest) {
                Ok(text) => {
                    out.push_str(text);
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&rest[..valid]).expect("valid prefix"));
                    match error.error_len() {
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid + len..];
                        }
                        None if stream => {
                            self.pending = rest[valid..].to_vec();
                            break;
                        }
                        None => {
                            out.push('\u{FFFD}');
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// End of input (`decoder.decode()`).
    pub fn finish(&mut self) -> String {
        self.decode(&[], false)
    }
}

/// `OutputAccumulatorOptions`.
#[derive(Debug, Clone, Default)]
pub struct OutputAccumulatorOptions {
    pub max_lines: Option<usize>,
    pub max_bytes: Option<usize>,
    pub temp_file_prefix: Option<String>,
    /// The command that produced the output (for command-specific
    /// compression of the final snapshot).
    pub command: Option<String>,
}

/// `OutputSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSnapshot {
    pub content: String,
    pub truncation: TruncationResult,
    pub full_output_path: Option<String>,
}

/// A new temp file `<tmpdir>/<prefix>-<random>.log` that is kept on disk.
pub(crate) fn create_temp_log(prefix: &str) -> std::io::Result<(PathBuf, File)> {
    let (file, path) = tempfile::Builder::new()
        .prefix(&format!("{prefix}-"))
        .suffix(".log")
        .rand_bytes(16)
        .tempfile()?
        .keep()
        .map_err(|e| e.error)?;
    Ok((path, file))
}

/// `OutputAccumulator`: keeps a decoded tail for display and, once the
/// output passes the caps, the full raw bytes in a temp file.
#[derive(Debug)]
pub struct OutputAccumulator {
    max_lines: usize,
    max_bytes: usize,
    max_rolling_bytes: usize,
    temp_file_prefix: String,
    command: Option<String>,
    decoder: Utf8StreamDecoder,

    raw_chunks: Vec<Vec<u8>>,
    tail_text: String,
    tail_starts_at_line_boundary: bool,
    total_raw_bytes: usize,
    total_decoded_bytes: usize,
    total_lines: usize,
    current_line_bytes: usize,
    finished: bool,

    temp_file_path: Option<String>,
    temp_file: Option<File>,
    snapshot_cache: Option<(String, TruncationResult)>,
}

impl Default for OutputAccumulator {
    fn default() -> Self {
        Self::new(OutputAccumulatorOptions::default())
    }
}

impl OutputAccumulator {
    pub fn new(options: OutputAccumulatorOptions) -> Self {
        let max_bytes = options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        Self {
            max_lines: options.max_lines.unwrap_or(DEFAULT_MAX_LINES),
            max_bytes,
            max_rolling_bytes: (max_bytes * 2).max(1),
            temp_file_prefix: options
                .temp_file_prefix
                .unwrap_or_else(|| "cortex-output".into()),
            command: options.command,
            decoder: Utf8StreamDecoder::new(),
            raw_chunks: Vec::new(),
            tail_text: String::new(),
            tail_starts_at_line_boundary: true,
            total_raw_bytes: 0,
            total_decoded_bytes: 0,
            total_lines: 1,
            current_line_bytes: 0,
            finished: false,
            temp_file_path: None,
            temp_file: None,
            snapshot_cache: None,
        }
    }

    /// `append`.
    ///
    /// # Panics
    /// After [`finish`](Self::finish).
    pub fn append(&mut self, data: &[u8]) {
        assert!(
            !self.finished,
            "Cannot append to a finished output accumulator"
        );
        self.total_raw_bytes += data.len();
        self.snapshot_cache = None;
        let text = self.decoder.decode(data, true);
        self.append_decoded_text(&text);

        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_file();
            if let Some(file) = &mut self.temp_file {
                let _ = file.write_all(data);
            }
        } else if !data.is_empty() {
            self.raw_chunks.push(data.to_vec());
        }
    }

    /// `finish`: flush the decoder; no more appends.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.snapshot_cache = None;
        let text = self.decoder.finish();
        self.append_decoded_text(&text);
        if self.should_use_temp_file() {
            self.ensure_temp_file();
        }
    }

    /// `snapshot`: the tail as the model will see it. Compression applies
    /// only once finished.
    pub fn snapshot(&mut self, persist_if_truncated: bool) -> OutputSnapshot {
        if self.snapshot_cache.is_none() {
            let raw = self.snapshot_text();
            let text = match (&self.command, self.finished) {
                (Some(command), true) => compress_bash_output(command, &raw),
                _ => raw,
            };
            let tail = truncate_tail(
                &text,
                TruncationOptions {
                    max_lines: Some(self.max_lines),
                    max_bytes: Some(self.max_bytes),
                },
            );
            let truncated =
                self.total_lines > self.max_lines || self.total_decoded_bytes > self.max_bytes;
            let truncated_by = truncated.then(|| {
                tail.truncated_by
                    .unwrap_or(if self.total_decoded_bytes > self.max_bytes {
                        TruncatedBy::Bytes
                    } else {
                        TruncatedBy::Lines
                    })
            });
            let truncation = TruncationResult {
                truncated,
                truncated_by,
                total_lines: self.total_lines,
                total_bytes: self.total_decoded_bytes,
                max_lines: self.max_lines,
                max_bytes: self.max_bytes,
                ..tail
            };
            self.snapshot_cache = Some((truncation.content.clone(), truncation));
        }
        let (content, truncation) = self.snapshot_cache.clone().expect("cached");
        if persist_if_truncated && truncation.truncated {
            self.ensure_temp_file();
        }
        OutputSnapshot {
            content,
            truncation,
            full_output_path: self.temp_file_path.clone(),
        }
    }

    /// `closeTempFile`: flush and close the full-output file.
    pub fn close_temp_file(&mut self) -> std::io::Result<()> {
        match self.temp_file.take() {
            Some(mut file) => file.flush(),
            None => Ok(()),
        }
    }

    /// `getLastLineBytes`: bytes of the line still being written.
    pub fn last_line_bytes(&self) -> usize {
        self.current_line_bytes
    }

    fn append_decoded_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bytes = text.len();
        self.total_decoded_bytes += bytes;
        self.tail_text.push_str(text);
        if self.tail_text.len() > self.max_rolling_bytes * 2 {
            self.trim_tail();
        }
        match text.rfind('\n') {
            None => self.current_line_bytes += bytes,
            Some(last_newline) => {
                self.total_lines += text.matches('\n').count();
                self.current_line_bytes = bytes - last_newline - 1;
            }
        }
    }

    fn trim_tail(&mut self) {
        let len = self.tail_text.len();
        if len <= self.max_rolling_bytes {
            return;
        }
        let mut start = len - self.max_rolling_bytes;
        while !self.tail_text.is_char_boundary(start) {
            start += 1;
        }
        if start > 0 {
            self.tail_starts_at_line_boundary = self.tail_text.as_bytes()[start - 1] == b'\n';
        }
        self.tail_text = self.tail_text[start..].to_owned();
    }

    fn snapshot_text(&self) -> String {
        if self.tail_starts_at_line_boundary {
            return self.tail_text.clone();
        }
        match self.tail_text.find('\n') {
            None => self.tail_text.clone(),
            Some(i) => self.tail_text[i + 1..].to_owned(),
        }
    }

    fn should_use_temp_file(&self) -> bool {
        self.total_raw_bytes > self.max_bytes
            || self.total_decoded_bytes > self.max_bytes
            || self.total_lines > self.max_lines
    }

    fn ensure_temp_file(&mut self) {
        if self.temp_file_path.is_some() {
            return;
        }
        // hoocode's createWriteStream reports failures asynchronously and
        // the path is still shown; here a failed create leaves no path.
        let Ok((path, mut file)) = create_temp_log(&self.temp_file_prefix) else {
            return;
        };
        for chunk in self.raw_chunks.drain(..) {
            let _ = file.write_all(&chunk);
        }
        self.temp_file_path = Some(path.to_string_lossy().into_owned());
        self.temp_file = Some(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_holds_split_characters_and_replaces_invalid_bytes() {
        let mut decoder = Utf8StreamDecoder::new();
        let euro = "€".as_bytes();
        assert_eq!(decoder.decode(&euro[..1], true), "");
        assert_eq!(decoder.decode(&euro[1..], true), "€");
        assert_eq!(decoder.decode(b"a\xffb", true), "a\u{FFFD}b");
        assert_eq!(decoder.decode(&euro[..2], true), "");
        assert_eq!(decoder.finish(), "\u{FFFD}");
    }

    #[test]
    fn small_output_stays_in_memory() {
        let mut acc = OutputAccumulator::default();
        acc.append(b"hello\nworld");
        acc.finish();
        let snapshot = acc.snapshot(true);
        assert_eq!(snapshot.content, "hello\nworld");
        assert!(!snapshot.truncation.truncated);
        assert_eq!(snapshot.truncation.total_lines, 2);
        assert_eq!(snapshot.full_output_path, None);
        assert_eq!(acc.last_line_bytes(), 5);
    }

    #[test]
    fn large_output_keeps_the_tail_and_spills_to_a_temp_file() {
        let mut acc = OutputAccumulator::new(OutputAccumulatorOptions {
            max_lines: Some(10),
            max_bytes: Some(1000),
            ..Default::default()
        });
        for i in 1..=5000 {
            acc.append(format!("{i}\n").as_bytes());
        }
        acc.finish();
        let snapshot = acc.snapshot(true);
        assert!(snapshot.truncation.truncated);
        assert_eq!(snapshot.truncation.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(snapshot.truncation.total_lines, 5001);
        assert!(snapshot.content.ends_with("4999\n5000\n"));
        acc.close_temp_file().unwrap();
        let path = snapshot.full_output_path.unwrap();
        let full = std::fs::read_to_string(&path).unwrap();
        assert!(full.starts_with("1\n2\n3\n"));
        assert!(full.ends_with("4999\n5000\n"));
        let _ = std::fs::remove_file(path);
    }
}

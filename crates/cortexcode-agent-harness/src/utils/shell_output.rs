//! Shell output capture: hoocode `harness/utils/shell-output.ts` (v0.5.89).
//! [`ShellCapture`] is the accumulator `executeShellWithCapture` feeds
//! stdout/stderr chunks into (sanitized, `\r` dropped, a rolling window of
//! ~100KB, and a temp file once the output passes 50KB); running the command
//! through an `ExecutionEnv` is ledger 9.3b.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;

use super::truncate::{truncate_tail, TruncationOptions, DEFAULT_MAX_BYTES};

/// `ShellCaptureResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCaptureResult {
    pub output: String,
    /// `None` when cancelled.
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    /// The temp file holding the full output, once there is one.
    pub full_output_path: Option<String>,
}

/// `sanitizeBinaryOutput`: drop control characters (except tab, newline and
/// carriage return) and the U+FFF9..U+FFFB annotation characters.
pub fn sanitize_binary_output(text: &str) -> String {
    text.chars()
        .filter(|c| {
            let code = *c as u32;
            matches!(code, 0x09 | 0x0a | 0x0d)
                || (code > 0x1f && !(0xfff9..=0xfffb).contains(&code))
        })
        .collect()
}

/// The chunk accumulator of `executeShellWithCapture`.
#[derive(Debug, Default)]
pub struct ShellCapture {
    chunks: VecDeque<String>,
    /// Kept output size in UTF-16 units (`text.length`), as TS counts it.
    output_units: usize,
    /// Raw bytes seen (before sanitizing).
    total_bytes: usize,
    temp_file_path: Option<PathBuf>,
    temp_file: Option<std::fs::File>,
}

impl ShellCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ensureTempFile`: `bash-<16 hex>.log` in the temp dir, seeded with the
    /// chunks kept so far.
    fn ensure_temp_file(&mut self) {
        if self.temp_file_path.is_some() {
            return;
        }
        let id = &uuid::Uuid::new_v4().simple().to_string()[..16];
        let path = std::env::temp_dir().join(format!("bash-{id}.log"));
        // A failed create leaves only the in-memory window, as a failed
        // write stream would.
        let mut file = std::fs::File::create(&path).ok();
        if let Some(file) = file.as_mut() {
            for chunk in &self.chunks {
                let _ = file.write_all(chunk.as_bytes());
            }
        }
        self.temp_file = file;
        self.temp_file_path = Some(path);
    }

    /// `onChunk`: the sanitized text (what `options.onChunk` receives).
    pub fn push(&mut self, chunk: &str) -> String {
        self.total_bytes += chunk.len();
        let text = sanitize_binary_output(chunk).replace('\r', "");
        if self.total_bytes > DEFAULT_MAX_BYTES {
            self.ensure_temp_file();
        }
        if let Some(file) = self.temp_file.as_mut() {
            let _ = file.write_all(text.as_bytes());
        }
        self.output_units += text.encode_utf16().count();
        self.chunks.push_back(text.clone());
        let max_output_units = DEFAULT_MAX_BYTES * 2;
        while self.output_units > max_output_units && self.chunks.len() > 1 {
            if let Some(removed) = self.chunks.pop_front() {
                self.output_units -= removed.encode_utf16().count();
            }
        }
        text
    }

    /// The result once the command ended (`exit_code` is dropped when
    /// `cancelled`): the tail-truncated output, spilling to the temp file
    /// when truncated.
    pub fn finish(mut self, exit_code: Option<i32>, cancelled: bool) -> ShellCaptureResult {
        let full_output: String = self.chunks.iter().map(String::as_str).collect();
        let truncation = truncate_tail(&full_output, TruncationOptions::default());
        if truncation.truncated {
            self.ensure_temp_file();
        }
        if let Some(mut file) = self.temp_file.take() {
            let _ = file.flush();
        }
        ShellCaptureResult {
            output: if truncation.truncated {
                truncation.content
            } else {
                full_output
            },
            exit_code: if cancelled { None } else { exit_code },
            cancelled,
            truncated: truncation.truncated,
            full_output_path: self
                .temp_file_path
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizing_keeps_tabs_and_newlines_only() {
        assert_eq!(
            sanitize_binary_output("a\tb\u{0}\u{1b}[0m\r\n\u{FFFA}c"),
            "a\tb[0m\r\nc"
        );
    }

    #[test]
    fn small_output_stays_in_memory() {
        let mut capture = ShellCapture::new();
        assert_eq!(capture.push("hi\r\n"), "hi\n");
        capture.push("there");
        let result = capture.finish(Some(0), false);
        assert_eq!(result.output, "hi\nthere");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.truncated);
        assert_eq!(result.full_output_path, None);
    }

    #[test]
    fn large_output_spills_to_a_temp_file_and_keeps_the_tail() {
        let mut capture = ShellCapture::new();
        let line = format!("{}\n", "y".repeat(99));
        for _ in 0..1200 {
            capture.push(&line);
        }
        let result = capture.finish(Some(1), true);
        assert!(result.truncated);
        assert!(result.cancelled);
        assert_eq!(result.exit_code, None);
        assert!(result.output.len() <= DEFAULT_MAX_BYTES);
        let path = result.full_output_path.unwrap();
        let file = std::fs::read_to_string(&path).unwrap();
        // Everything, including the chunks from before the spill.
        assert_eq!(file.len(), line.len() * 1200);
        std::fs::remove_file(path).unwrap();
    }
}

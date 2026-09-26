//! `core/bash-executor.ts`: run a command for the user (`!` commands in
//! interactive and RPC modes) with sanitized, truncated output.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use cortexcode_agent_harness::utils::output_compression::compress_bash_output;
use cortexcode_ai_types::AbortSignal;
use cortexcode_code_tool_api::{truncate_tail, ToolError, TruncationOptions, DEFAULT_MAX_BYTES};

use crate::accumulator::{create_temp_log, Utf8StreamDecoder};
use crate::operations::{BashExecOptions, BashOperations};
use crate::shell::{sanitize_binary_output, strip_ansi};

/// `BashExecutorOptions`.
#[derive(Default)]
pub struct BashExecutorOptions<'a> {
    /// Receives each sanitized chunk.
    pub on_chunk: Option<&'a mut dyn FnMut(&str)>,
    pub signal: Option<AbortSignal>,
}

/// `BashResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashResult {
    /// Combined stdout + stderr (sanitized, possibly truncated).
    pub output: String,
    /// `None` when killed or cancelled.
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub truncated: bool,
    /// The full output, once it passed the truncation threshold.
    pub full_output_path: Option<String>,
}

struct Collector<'a> {
    chunks: std::collections::VecDeque<String>,
    /// UTF-16 length of `chunks` (hoocode counts `text.length`).
    output_units: usize,
    total_bytes: usize,
    temp_path: Option<String>,
    temp_file: Option<File>,
    decoder: Utf8StreamDecoder,
    on_chunk: Option<&'a mut dyn FnMut(&str)>,
}

impl Collector<'_> {
    fn ensure_temp_file(&mut self) {
        if self.temp_path.is_some() {
            return;
        }
        let Ok((path, mut file)) = create_temp_log("cortex-bash") else {
            return;
        };
        for chunk in &self.chunks {
            let _ = file.write_all(chunk.as_bytes());
        }
        self.temp_path = Some(path.to_string_lossy().into_owned());
        self.temp_file = Some(file);
    }

    fn on_data(&mut self, data: &[u8]) {
        self.total_bytes += data.len();
        let decoded = self.decoder.decode(data, true);
        let text = sanitize_binary_output(&strip_ansi(&decoded)).replace('\r', "");
        if self.total_bytes > DEFAULT_MAX_BYTES {
            self.ensure_temp_file();
        }
        if let Some(file) = &mut self.temp_file {
            let _ = file.write_all(text.as_bytes());
        }
        let units = text.encode_utf16().count();
        self.chunks.push_back(text.clone());
        self.output_units += units;
        let max_output_units = DEFAULT_MAX_BYTES * 2;
        while self.output_units > max_output_units && self.chunks.len() > 1 {
            if let Some(removed) = self.chunks.pop_front() {
                self.output_units -= removed.encode_utf16().count();
            }
        }
        if let Some(on_chunk) = &mut self.on_chunk {
            on_chunk(&text);
        }
    }

    fn finish(&mut self, command: &str) -> (String, bool) {
        let full: String = self.chunks.iter().map(String::as_str).collect();
        let compressed = compress_bash_output(command, &full);
        let truncation = truncate_tail(&compressed, TruncationOptions::default());
        if truncation.truncated {
            self.ensure_temp_file();
        }
        if let Some(mut file) = self.temp_file.take() {
            let _ = file.flush();
        }
        let output = if truncation.truncated {
            truncation.content
        } else {
            compressed
        };
        (output, truncation.truncated)
    }
}

/// `executeBashWithOperations`.
pub fn execute_bash_with_operations(
    command: &str,
    cwd: &Path,
    operations: &dyn BashOperations,
    options: BashExecutorOptions<'_>,
) -> Result<BashResult, ToolError> {
    let signal = options.signal.clone();
    let mut collector = Collector {
        chunks: Default::default(),
        output_units: 0,
        total_bytes: 0,
        temp_path: None,
        temp_file: None,
        decoder: Utf8StreamDecoder::new(),
        on_chunk: options.on_chunk,
    };
    let result = operations.exec(
        command,
        cwd,
        BashExecOptions {
            on_data: &mut |data| collector.on_data(data),
            signal: signal.clone(),
            timeout: None,
            env: None,
        },
    );
    let aborted = signal.as_ref().is_some_and(AbortSignal::aborted);
    match result {
        Ok(exit_code) => {
            let (output, truncated) = collector.finish(command);
            Ok(BashResult {
                output,
                exit_code: if aborted { None } else { exit_code },
                cancelled: aborted,
                truncated,
                full_output_path: collector.temp_path,
            })
        }
        Err(_) if aborted => {
            let (output, truncated) = collector.finish(command);
            Ok(BashResult {
                output,
                exit_code: None,
                cancelled: true,
                truncated,
                full_output_path: collector.temp_path,
            })
        }
        Err(error) => {
            if let Some(mut file) = collector.temp_file.take() {
                let _ = file.flush();
            }
            Err(error)
        }
    }
}

//! The `read` tool (`core/tools/read.ts`).
//!
//! Text files come back byte-for-byte (the model copies them into edits), paged
//! by `offset`/`limit` and capped by the output limits with an actionable
//! continuation notice. Images are sent as attachments (resized to fit),
//! structured documents (docx/xlsx/pptx/pdf) get a note instead of bytes, and
//! a re-read that an earlier live read already covers returns a pointer.
//! Interactive rendering (`renderCall`/`renderResult`) arrives with phase 11.

use crate::read_dedup::{
    build_dedup_pointer_text, find_covering_read, read_range_from_args, read_stamp_matches,
    record_read_stamp, CoveringReadDisplay, FindCoveringReadOptions,
};
use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{Content, ImageContent, Model};
use cortexcode_code_media::{
    detect_supported_image_mime_type, format_dimension_note, resize_image, ImageResizeOptions,
    FILE_TYPE_SNIFF_BYTES,
};
use cortexcode_code_tool_api::{
    format_size, js_number, node_fs_error, resolve_read_path, truncate_head, wrap_tool_definition,
    ToolContext, ToolContextFactory, ToolDefinition, ToolError, TruncatedBy, TruncationOptions,
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use serde_json::{json, Value};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Pluggable operations for the read tool. Override these to delegate file
/// reading to remote systems (for example SSH).
pub trait ReadOperations: Send + Sync {
    /// Read file contents.
    fn read_file(&self, absolute_path: &Path) -> Result<Vec<u8>, ToolError>;
    /// Check that the file is readable (error if not).
    fn access(&self, absolute_path: &Path) -> Result<(), ToolError>;
    /// Detect an image MIME type; `None` for non-images. The default never
    /// detects images (an operations object without `detectImageMimeType`).
    fn detect_image_mime_type(&self, _absolute_path: &Path) -> Result<Option<String>, ToolError> {
        Ok(None)
    }
}

/// The local filesystem, with Node's error messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalReadOperations;

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open(path: &Path) -> Result<std::fs::File, ToolError> {
    std::fs::File::open(path).map_err(|e| node_fs_error(e, "open", Some(&display(path))).into())
}

impl ReadOperations for LocalReadOperations {
    fn read_file(&self, absolute_path: &Path) -> Result<Vec<u8>, ToolError> {
        let mut file = open(absolute_path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .map_err(|e| node_fs_error(e, "read", None))?;
        Ok(buf)
    }

    fn access(&self, absolute_path: &Path) -> Result<(), ToolError> {
        access_readable(absolute_path)
            .map_err(|e| node_fs_error(e, "access", Some(&display(absolute_path))).into())
    }

    fn detect_image_mime_type(&self, absolute_path: &Path) -> Result<Option<String>, ToolError> {
        let file = open(absolute_path)?;
        let mut head = Vec::with_capacity(FILE_TYPE_SNIFF_BYTES);
        file.take(FILE_TYPE_SNIFF_BYTES as u64)
            .read_to_end(&mut head)
            .map_err(|e| node_fs_error(e, "read", None))?;
        Ok(detect_supported_image_mime_type(&head).map(str::to_string))
    }
}

/// `fs.access(path, R_OK)`.
#[cfg(unix)]
fn access_readable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `c_path` is a valid NUL-terminated string for the duration of the call.
    if unsafe { libc::access(c_path.as_ptr(), libc::R_OK) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `fs.access(path, R_OK)`.
#[cfg(not(unix))]
fn access_readable(path: &Path) -> std::io::Result<()> {
    std::fs::metadata(path).map(|_| ())
}

/// `ReadToolOptions`.
#[derive(Clone)]
pub struct ReadToolOptions {
    /// Auto-resize images to 2000x2000 max. Default: true.
    pub auto_resize_images: bool,
    /// Custom operations for file reading. Default: local filesystem.
    pub operations: Option<Arc<dyn ReadOperations>>,
    /// Byte cap on the returned text before truncation.
    pub max_output_bytes: usize,
    /// Line cap on the returned text before truncation.
    pub max_output_lines: usize,
    /// Short-circuit a text read whose range an earlier, still-live read in the
    /// session already covers (gated on the context GC setting). Default: false.
    pub dedup_reads: bool,
}

impl Default for ReadToolOptions {
    fn default() -> Self {
        Self {
            auto_resize_images: true,
            operations: None,
            max_output_bytes: DEFAULT_MAX_BYTES,
            max_output_lines: DEFAULT_MAX_LINES,
            dedup_reads: false,
        }
    }
}

/// Structured/binary document formats: a utf-8 read yields garbage, so read
/// short-circuits with a note.
fn doc_format_hint(absolute_path: &str) -> Option<&'static str> {
    let dot = absolute_path.rfind('.')?;
    match absolute_path[dot..].to_lowercase().as_str() {
        ".docx" => Some("Word (OOXML)"),
        ".xlsx" => Some("Excel (OOXML)"),
        ".pptx" => Some("PowerPoint (OOXML)"),
        ".pdf" => Some("PDF"),
        _ => None,
    }
}

fn non_vision_image_note(model: Option<&Model>) -> Option<&'static str> {
    match model {
        Some(model) if !model.input.iter().any(|i| i == "image") => Some(
            "[Current model does not support images. The image will be omitted from this request.]",
        ),
        _ => None,
    }
}

/// JS `Array.prototype.slice(start, end)` on a line list.
fn js_slice<'a, 'b>(lines: &'b [&'a str], start: f64, end: f64) -> &'b [&'a str] {
    let len = lines.len() as f64;
    let relative = |x: f64| {
        let x = if x.is_nan() { 0.0 } else { x.trunc() };
        if x < 0.0 {
            (len + x).max(0.0)
        } else {
            x.min(len)
        }
    };
    let (s, e) = (relative(start) as usize, relative(end) as usize);
    if e <= s {
        &[]
    } else {
        &lines[s..e]
    }
}

/// A numeric argument: a JSON number, or a numeric string (what schema
/// coercion would have produced).
fn number_arg(args: &Value, key: &str) -> Option<f64> {
    match args.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text_block(text: impl Into<String>) -> Content {
    Content::text(text)
}

fn aborted(signal: &Option<cortexcode_ai_types::AbortSignal>) -> bool {
    signal.as_ref().is_some_and(|s| s.aborted())
}

struct ReadCall<'a> {
    cwd: &'a Path,
    ops: &'a dyn ReadOperations,
    options: &'a ReadToolOptions,
    tool_call_id: &'a str,
    path: &'a str,
    offset: Option<f64>,
    limit: Option<f64>,
    ctx: Option<&'a ToolContext>,
}

impl ReadCall<'_> {
    fn run(&self) -> Result<(Vec<Content>, Value), ToolError> {
        let absolute = resolve_read_path(self.path, self.cwd);
        let absolute_str = display(&absolute);
        self.ops.access(&absolute)?;
        let mime_type = self.ops.detect_image_mime_type(&absolute)?;
        let non_vision_note = non_vision_image_note(self.ctx.and_then(|c| c.model.as_ref()));
        let doc_hint = doc_format_hint(&absolute_str);

        // At-call dedup: for plain-text reads, point at an earlier still-live
        // read that already covers the requested range.
        let session = self.ctx.and_then(|c| c.session_manager.as_ref());
        let dedup_covering = match session {
            Some(session)
                if self.options.dedup_reads && mime_type.is_none() && doc_hint.is_none() =>
            {
                let cwd = self.cwd.to_path_buf();
                let resolve = move |raw: &str| display(&resolve_read_path(raw, &cwd));
                let mut args = json!({});
                if let Some(offset) = self.offset {
                    args["offset"] = json!(offset);
                }
                if let Some(limit) = self.limit {
                    args["limit"] = json!(limit);
                }
                find_covering_read(
                    &session.get_branch(),
                    &FindCoveringReadOptions {
                        resolved_path: &absolute_str,
                        requested_range: read_range_from_args(Some(&args)),
                        current_call_id: self.tool_call_id,
                        resolve_path: &resolve,
                    },
                )
            }
            _ => None,
        };

        if let Some(mime_type) = mime_type {
            return Ok((
                self.read_image(&absolute, &mime_type, non_vision_note)?,
                Value::Null,
            ));
        }

        if let Some(label) = doc_hint {
            return Ok((
                vec![text_block(format!(
                    "[{label} document — not plain text; read cannot show it. Extract it with a converter via bash if you need its contents.]"
                ))],
                Value::Null,
            ));
        }

        let buffer = self.ops.read_file(&absolute)?;
        // Stamp the bytes on disk, so the pointer's "has not changed since" is checked.
        let stamp = sha1_smol::Sha1::from(&buffer).digest().to_string();
        if let Some(covering) = dedup_covering {
            if read_stamp_matches(&covering.call_id, &stamp) {
                let text = build_dedup_pointer_text(&CoveringReadDisplay::from(&covering));
                return Ok((vec![text_block(text)], Value::Null));
            }
        }

        let (text, details) = self.read_text(&buffer)?;
        record_read_stamp(self.tool_call_id, &stamp);
        Ok((vec![text_block(text)], details))
    }

    fn read_image(
        &self,
        absolute: &Path,
        mime_type: &str,
        non_vision_note: Option<&str>,
    ) -> Result<Vec<Content>, ToolError> {
        use base64::Engine as _;
        let buffer = self.ops.read_file(absolute)?;
        let base64 = base64::engine::general_purpose::STANDARD.encode(&buffer);
        let image = |data: String, mime: String| {
            Content::Image(ImageContent {
                data,
                media_type: mime,
                cache_control: None,
            })
        };
        if !self.options.auto_resize_images {
            let mut note = format!("Read image file [{mime_type}]");
            if let Some(n) = non_vision_note {
                note.push_str(&format!("\n{n}"));
            }
            return Ok(vec![text_block(note), image(base64, mime_type.to_string())]);
        }
        match resize_image(&base64, Some(mime_type), ImageResizeOptions::default()) {
            None => {
                let mut note = format!(
                    "Read image file [{mime_type}]\n[Image omitted: could not be resized below the inline image size limit.]"
                );
                if let Some(n) = non_vision_note {
                    note.push_str(&format!("\n{n}"));
                }
                Ok(vec![text_block(note)])
            }
            Some(resized) => {
                let mut note = format!("Read image file [{}]", resized.mime_type);
                if let Some(d) = format_dimension_note(&resized) {
                    note.push_str(&format!("\n{d}"));
                }
                if let Some(n) = non_vision_note {
                    note.push_str(&format!("\n{n}"));
                }
                Ok(vec![
                    text_block(note),
                    image(resized.data, resized.mime_type),
                ])
            }
        }
    }

    fn read_text(&self, buffer: &[u8]) -> Result<(String, Value), ToolError> {
        let max_bytes = self.options.max_output_bytes;
        let max_lines = self.options.max_output_lines;
        let text_content = String::from_utf8_lossy(buffer);
        let all_lines: Vec<&str> = text_content.split('\n').collect();
        let total_file_lines = all_lines.len();
        let len = total_file_lines as f64;

        // 1-indexed input to 0-indexed access; offset 0 means "from the start".
        let start_line = match self.offset {
            Some(o) if o != 0.0 && !o.is_nan() => (o - 1.0).max(0.0),
            _ => 0.0,
        };
        let start_line_display = start_line + 1.0;
        if start_line >= len {
            return Err(format!(
                "Offset {} is beyond end of file ({} lines total)",
                js_number(self.offset.unwrap_or(0.0)),
                total_file_lines
            )
            .into());
        }

        let mut user_limited_lines: Option<f64> = None;
        let selected: std::borrow::Cow<'_, str> = if let Some(limit) = self.limit {
            let end_line = (start_line + limit).min(len);
            user_limited_lines = Some(end_line - start_line);
            js_slice(&all_lines, start_line, end_line).join("\n").into()
        } else if start_line == 0.0 {
            // Whole-file read: reuse the original string.
            std::borrow::Cow::Borrowed(text_content.as_ref())
        } else {
            js_slice(&all_lines, start_line, len).join("\n").into()
        };

        // The text is handed over byte-for-byte: whatever the model reads here
        // is what it sends back as an edit's `oldText`.
        let truncation = truncate_head(
            &selected,
            TruncationOptions {
                max_lines: Some(max_lines),
                max_bytes: Some(max_bytes),
            },
        );

        if truncation.first_line_exceeds_limit {
            let first_line = all_lines.get(start_line as usize).copied().unwrap_or("");
            let text = format!(
                "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                js_number(start_line_display),
                format_size(first_line.len()),
                format_size(max_bytes),
                js_number(start_line_display),
                self.path,
                max_bytes
            );
            return Ok((text, json!({ "truncation": truncation })));
        }

        if truncation.truncated {
            let end_line_display = start_line_display + truncation.output_lines as f64 - 1.0;
            let next_offset = end_line_display + 1.0;
            let mut text = truncation.content.clone();
            if truncation.truncated_by == Some(TruncatedBy::Lines) {
                text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    js_number(start_line_display),
                    js_number(end_line_display),
                    total_file_lines,
                    js_number(next_offset)
                ));
            } else {
                text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    js_number(start_line_display),
                    js_number(end_line_display),
                    total_file_lines,
                    format_size(max_bytes),
                    js_number(next_offset)
                ));
            }
            return Ok((text, json!({ "truncation": truncation })));
        }

        if let Some(user_limited) = user_limited_lines {
            if start_line + user_limited < len {
                // The user's limit stopped early, but the file has more.
                let remaining = len - (start_line + user_limited);
                let next_offset = start_line + user_limited + 1.0;
                let text = format!(
                    "{}\n\n[{} more lines in file. Use offset={} to continue.]",
                    truncation.content,
                    js_number(remaining),
                    js_number(next_offset)
                );
                return Ok((text, Value::Null));
            }
        }
        Ok((truncation.content, Value::Null))
    }
}

/// `createReadToolDefinition`.
pub fn create_read_tool_definition(
    cwd: impl Into<PathBuf>,
    options: ReadToolOptions,
) -> ToolDefinition {
    let cwd: PathBuf = cwd.into();
    let ops: Arc<dyn ReadOperations> = options
        .operations
        .clone()
        .unwrap_or_else(|| Arc::new(LocalReadOperations));
    // Math.round(maxBytes / 1024)
    let kb = (options.max_output_bytes * 2 + 1024) / 2048;
    let description = format!(
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp). Images are sent as attachments. For text files, output is truncated to {} lines or {}KB (whichever is hit first). Use offset/limit for large files. When you need the full file, continue with offset until complete.",
        options.max_output_lines, kb
    );
    ToolDefinition {
        name: "read".into(),
        label: "read".into(),
        description,
        prompt_snippet: Some("Read file contents".into()),
        // No promptGuidelines: "use read instead of cat/sed" is already covered
        // by the system prompt's file-exploration guideline and bash's snippet.
        prompt_guidelines: Vec::new(),
        parameters: read_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |tool_call_id, args, signal, _on_update, ctx| {
            if aborted(&signal) {
                return Err("Operation aborted".into());
            }
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or("The \"path\" argument must be of type string")?;
            let call = ReadCall {
                cwd: &cwd,
                ops: ops.as_ref(),
                options: &options,
                tool_call_id: &tool_call_id,
                path,
                offset: number_arg(&args, "offset"),
                limit: number_arg(&args, "limit"),
                ctx,
            };
            let (content, details) = call.run()?;
            if aborted(&signal) {
                return Err("Operation aborted".into());
            }
            Ok(AgentToolResult {
                content,
                details,
                terminate: false,
            })
        }),
    }
}

/// The TypeBox schema hoocode sends for `read`.
pub fn read_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {"type": "string", "description": "Path to the file to read (relative or absolute)"},
            "offset": {"type": "number", "description": "Line number to start reading from (1-indexed)"},
            "limit": {"type": "number", "description": "Maximum number of lines to read"}
        }
    })
}

/// `createReadTool`: the definition wrapped for the agent loop.
pub fn create_read_tool(
    cwd: impl Into<PathBuf>,
    options: ReadToolOptions,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    wrap_tool_definition(create_read_tool_definition(cwd, options), ctx_factory)
}

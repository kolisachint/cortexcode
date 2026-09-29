//! The `edit` tool (`core/tools/edit.ts`). Interactive rendering (the diff
//! preview) arrives with phase 11; [`compute_edits_diff`] is its data.
//!
//! [`compute_edits_diff`]: crate::edit_diff::compute_edits_diff

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortexcode_agent_types::{AgentTool, AgentToolResult, PrepareArgumentsFn};
use cortexcode_ai_types::{AbortSignal, Content, TextContent};
use cortexcode_code_tool_api::{
    node_fs_error, resolve_to_cwd, wrap_tool_definition, ToolContextFactory, ToolDefinition,
    ToolError,
};
use serde_json::{json, Map, Value};

use crate::edit_diff::{
    apply_edits_to_normalized_content, detect_line_ending, generate_diff_string, normalize_to_lf,
    restore_line_endings, strip_bom, Edit,
};
use crate::mutation_queue::with_file_mutation_queue;

/// Maximum characters of diff kept in the result details.
const MAX_DIFF_CHARS: usize = 4000;

/// `EditOperations`: override to edit files elsewhere (for example SSH).
pub trait EditOperations: Send + Sync {
    fn read_file(&self, absolute_path: &Path) -> Result<Vec<u8>, ToolError>;
    fn write_file(&self, absolute_path: &Path, content: &str) -> Result<(), ToolError>;
    /// Fail unless the file is readable and writable. The error's Node code
    /// (`ENOENT`, `EACCES`, ...) is reported to the model.
    fn access(&self, absolute_path: &Path) -> Result<(), ToolError>;
}

/// The local filesystem, with Node's error messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalEditOperations;

impl EditOperations for LocalEditOperations {
    fn read_file(&self, absolute_path: &Path) -> Result<Vec<u8>, ToolError> {
        std::fs::read(absolute_path)
            .map_err(|e| node_fs_error(e, "open", Some(&absolute_path.to_string_lossy())).into())
    }

    fn write_file(&self, absolute_path: &Path, content: &str) -> Result<(), ToolError> {
        std::fs::write(absolute_path, content)
            .map_err(|e| node_fs_error(e, "open", Some(&absolute_path.to_string_lossy())).into())
    }

    fn access(&self, absolute_path: &Path) -> Result<(), ToolError> {
        crate::access::access(absolute_path, true)
            .map_err(|e| node_fs_error(e, "access", Some(&absolute_path.to_string_lossy())).into())
    }
}

/// `EditToolOptions`.
#[derive(Clone, Default)]
pub struct EditToolOptions {
    pub operations: Option<Arc<dyn EditOperations>>,
}

/// The TypeBox schema hoocode sends for `edit`.
pub fn edit_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "edits"],
        "properties": {
            "path": {"type": "string", "description": "Path to the file to edit (relative or absolute)"},
            "edits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["oldText", "newText"],
                    "properties": {
                        "oldText": {"type": "string", "description": "Exact text for one targeted replacement. It must be unique in the original file (unless replaceAll is true) and must not overlap with any other edits[].oldText in the same call."},
                        "newText": {"type": "string", "description": "Replacement text for this targeted edit."},
                        "replaceAll": {"type": "boolean", "description": "When true, replace every occurrence of oldText instead of requiring it to be unique. Use for renaming a symbol/string throughout the file. Default false."}
                    },
                    "additionalProperties": false
                },
                "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead."
            }
        },
        "additionalProperties": false
    })
}

/// `prepareEditArguments`: parse a stringified `edits` array and fold the
/// legacy top-level `oldText`/`newText` into `edits`.
pub fn prepare_edit_arguments(input: Value) -> Value {
    let Value::Object(mut args) = input else {
        return input;
    };
    if let Some(Value::String(edits)) = args.get("edits") {
        if let Ok(parsed @ Value::Array(_)) = serde_json::from_str::<Value>(edits) {
            args.insert("edits".into(), parsed);
        }
    }
    let (Some(Value::String(old_text)), Some(Value::String(new_text))) =
        (args.get("oldText"), args.get("newText"))
    else {
        return Value::Object(args);
    };
    let legacy = json!({"oldText": old_text, "newText": new_text});
    let mut edits = match args.get("edits") {
        Some(Value::Array(edits)) => edits.clone(),
        _ => Vec::new(),
    };
    edits.push(legacy);
    let mut rest: Map<String, Value> = args
        .into_iter()
        .filter(|(k, _)| k != "oldText" && k != "newText")
        .collect();
    rest.insert("edits".into(), Value::Array(edits));
    Value::Object(rest)
}

/// `validateEditInput`.
fn validate_edit_input(args: &Value) -> Result<(String, Vec<Edit>), ToolError> {
    let invalid = || -> ToolError {
        "Edit tool input is invalid. edits must contain at least one replacement.".into()
    };
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .filter(|e| !e.is_empty())
        .ok_or_else(invalid)?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("The \"path\" argument must be of type string")?
        .to_owned();
    let edits = edits
        .iter()
        .map(|e| Edit {
            old_text: e
                .get("oldText")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            new_text: e
                .get("newText")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            replace_all: e.get("replaceAll").and_then(Value::as_bool) == Some(true),
        })
        .collect();
    Ok((path, edits))
}

fn aborted(signal: &Option<AbortSignal>) -> bool {
    signal.as_ref().is_some_and(AbortSignal::aborted)
}

/// The Node error code carried by an operations error, if any.
fn error_code(error: &ToolError) -> Option<String> {
    error
        .downcast_ref::<cortexcode_code_tool_api::NodeFsError>()
        .map(|e| e.code.clone())
}

fn run_edit(
    ops: &dyn EditOperations,
    absolute: &Path,
    path: &str,
    edits: &[Edit],
    signal: &Option<AbortSignal>,
) -> Result<AgentToolResult, ToolError> {
    if aborted(signal) {
        return Err("Operation aborted".into());
    }
    if let Err(error) = ops.access(absolute) {
        let message = match error_code(&error) {
            Some(code) => format!("Error code: {code}"),
            // String(error) on a plain Error.
            None => format!("Error: {error}"),
        };
        return Err(format!("Could not edit file: {path}. {message}.").into());
    }
    if aborted(signal) {
        return Err("Operation aborted".into());
    }
    let buffer = ops.read_file(absolute)?;
    let raw = String::from_utf8_lossy(&buffer);
    if aborted(signal) {
        return Err("Operation aborted".into());
    }
    // The model never includes the invisible BOM in oldText.
    let (bom, content) = strip_bom(&raw);
    let original_ending = detect_line_ending(content);
    let normalized = normalize_to_lf(content);
    let applied = apply_edits_to_normalized_content(&normalized, edits, path)?;
    if aborted(signal) {
        return Err("Operation aborted".into());
    }
    let final_content = format!(
        "{bom}{}",
        restore_line_endings(&applied.new_content, original_ending)
    );
    ops.write_file(absolute, &final_content)?;
    if aborted(signal) {
        return Err("Operation aborted".into());
    }
    let diff = generate_diff_string(&applied.base_content, &applied.new_content, 4);
    // diff.slice(0, MAX_DIFF_CHARS) counts UTF-16 units.
    let units: Vec<u16> = diff.diff.encode_utf16().collect();
    let shown = if units.len() > MAX_DIFF_CHARS {
        format!(
            "{}\n\n[diff truncated for brevity]",
            String::from_utf16_lossy(&units[..MAX_DIFF_CHARS])
        )
    } else {
        diff.diff
    };
    let mut details = Map::new();
    details.insert("diff".into(), shown.into());
    if let Some(line) = diff.first_changed_line {
        details.insert("firstChangedLine".into(), line.into());
    }
    Ok(AgentToolResult {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: format!("Successfully replaced {} block(s) in {path}.", edits.len()),
        })],
        details: Value::Object(details),
        terminate: false,
    })
}

/// `createEditToolDefinition`.
pub fn create_edit_tool_definition(
    cwd: impl Into<PathBuf>,
    options: EditToolOptions,
) -> ToolDefinition {
    let cwd: PathBuf = cwd.into();
    let ops: Arc<dyn EditOperations> = options
        .operations
        .unwrap_or_else(|| Arc::new(LocalEditOperations));
    let prepare: PrepareArgumentsFn = Arc::new(prepare_edit_arguments);
    ToolDefinition { background_when: None, ordered_start: true,
        name: "edit".into(),
        label: "edit".into(),
        description: "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file, unless that edit sets replaceAll: true to replace all of its occurrences. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.".into(),
        prompt_snippet: Some(
            "Make precise file edits with exact text replacement, including multiple disjoint edits in one call".into(),
        ),
        prompt_guidelines: vec![
            "When changing several separate locations in one file, send one edit call with multiple entries in edits[] rather than several edit calls. Keep each oldText as small as it can be while staying unique.".into(),
        ],
        parameters: edit_parameters_schema(),
        prepare_arguments: Some(prepare),
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, args, signal, _on_update, _ctx| {
            let (path, edits) = validate_edit_input(&args)?;
            let absolute = resolve_to_cwd(&path, &cwd);
            with_file_mutation_queue(&absolute, || {
                run_edit(ops.as_ref(), &absolute, &path, &edits, &signal)
            })
        }),
    }
}

/// `createEditTool`.
pub fn create_edit_tool(
    cwd: impl Into<PathBuf>,
    options: EditToolOptions,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    wrap_tool_definition(create_edit_tool_definition(cwd, options), ctx_factory)
}

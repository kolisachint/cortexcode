//! The `write` tool (`core/tools/write.ts`). Interactive rendering arrives
//! with phase 11.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{AbortSignal, Content, TextContent};
use cortexcode_code_tool_api::{
    node_fs_error, resolve_to_cwd, wrap_tool_definition, ToolContextFactory, ToolDefinition,
    ToolError,
};
use serde_json::{json, Value};

use crate::mutation_queue::with_file_mutation_queue;

/// `WriteOperations`: override to write files elsewhere (for example SSH).
pub trait WriteOperations: Send + Sync {
    fn write_file(&self, absolute_path: &Path, content: &str) -> Result<(), ToolError>;
    /// Create a directory and its parents.
    fn mkdir(&self, dir: &Path) -> Result<(), ToolError>;
}

/// The local filesystem, with Node's error messages.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalWriteOperations;

impl WriteOperations for LocalWriteOperations {
    fn write_file(&self, absolute_path: &Path, content: &str) -> Result<(), ToolError> {
        std::fs::write(absolute_path, content)
            .map_err(|e| node_fs_error(e, "open", Some(&absolute_path.to_string_lossy())).into())
    }

    fn mkdir(&self, dir: &Path) -> Result<(), ToolError> {
        std::fs::create_dir_all(dir)
            .map_err(|e| node_fs_error(e, "mkdir", Some(&dir.to_string_lossy())).into())
    }
}

/// `WriteToolOptions`.
#[derive(Clone, Default)]
pub struct WriteToolOptions {
    pub operations: Option<Arc<dyn WriteOperations>>,
}

/// The TypeBox schema hoocode sends for `write`.
pub fn write_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["path", "content"],
        "properties": {
            "path": {"type": "string", "description": "Path to the file to write (relative or absolute)"},
            "content": {"type": "string", "description": "Content to write to the file"}
        }
    })
}

fn aborted(signal: &Option<AbortSignal>) -> bool {
    signal.as_ref().is_some_and(AbortSignal::aborted)
}

/// `createWriteToolDefinition`.
pub fn create_write_tool_definition(
    cwd: impl Into<PathBuf>,
    options: WriteToolOptions,
) -> ToolDefinition {
    let cwd: PathBuf = cwd.into();
    let ops: Arc<dyn WriteOperations> = options
        .operations
        .unwrap_or_else(|| Arc::new(LocalWriteOperations));
    ToolDefinition { background_when: None,
        name: "write".into(),
        label: "write".into(),
        description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.".into(),
        prompt_snippet: Some("Create or overwrite files".into()),
        prompt_guidelines: vec!["Use write only for new files or complete rewrites.".into()],
        parameters: write_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, args, signal, _on_update, _ctx| {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or("The \"path\" argument must be of type string")?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or("The \"content\" argument must be of type string")?;
            let absolute = resolve_to_cwd(path, &cwd);
            let dir = absolute.parent().map(Path::to_path_buf).unwrap_or_default();
            with_file_mutation_queue(&absolute, || {
                if aborted(&signal) {
                    return Err::<AgentToolResult, ToolError>("Operation aborted".into());
                }
                ops.mkdir(&dir)?;
                if aborted(&signal) {
                    return Err("Operation aborted".into());
                }
                ops.write_file(&absolute, content)?;
                if aborted(&signal) {
                    return Err("Operation aborted".into());
                }
                Ok(AgentToolResult {
                    content: vec![Content::Text(TextContent {
                        text_signature: None,
                        text: format!("Successfully wrote {} bytes to {path}", content.len()),
                    })],
                    details: Value::Null,
                    terminate: false,
                })
            })
        }),
    }
}

/// `createWriteTool`.
pub fn create_write_tool(
    cwd: impl Into<PathBuf>,
    options: WriteToolOptions,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    wrap_tool_definition(create_write_tool_definition(cwd, options), ctx_factory)
}

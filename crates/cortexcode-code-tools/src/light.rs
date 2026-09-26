//! Light mode: a minimal, low-token preset for small/local models (`core/light.ts`).
//!
//! The preset restricts the session to the four core tools (read, write, edit,
//! bash) with shortened descriptions and undocumented parameter schemas. The
//! terse system prompt is `cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT`.

use crate::placeholder_tools;
use cortexcode_agent_types::AgentTool;
use cortexcode_code_tool_api::{tool_definition_from_agent_tool, ToolDefinition};
use cortexcode_code_tool_bash::{create_bash_tool, BashToolOptions};
use cortexcode_code_tools_fs::{create_read_tool, ReadToolOptions};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// The only tools a light session exposes.
pub const LIGHT_TOOL_NAMES: [&str; 4] = ["read", "write", "edit", "bash"];

fn take(tools: &mut Vec<AgentTool>, name: &str) -> AgentTool {
    let i = tools
        .iter()
        .position(|t| t.name == name)
        .unwrap_or_else(|| panic!("placeholder tool {name} is missing"));
    tools.swap_remove(i)
}

/// `createLightTools`: the real tools wearing short descriptions and stripped
/// parameter schemas (same shapes, no per-property descriptions). read and
/// bash get default options and no context, as hoocode's `createReadTool(cwd)`
/// and `createBashTool(cwd)` behind `baseToolsOverride` do.
pub fn create_light_tools(cwd: PathBuf) -> Vec<AgentTool> {
    let mut read = create_read_tool(cwd.clone(), ReadToolOptions::default(), None);
    read.description = "Read a file. args: path, offset?, limit?".into();
    read.parameters = json!({
        "type": "object",
        "required": ["path"],
        "properties": {
            "path": {"type": "string"},
            "offset": {"type": "number"},
            "limit": {"type": "number"}
        }
    });

    let cwd_bash = cwd.clone();
    let mut placeholders = placeholder_tools(cwd);
    let mut write = take(&mut placeholders, "write");
    write.description = "Write file (overwrites). args: path, content".into();
    write.parameters = json!({
        "type": "object",
        "required": ["path", "content"],
        "properties": {
            "path": {"type": "string"},
            "content": {"type": "string"}
        }
    });

    // Flat single-replacement shape. hoocode converts it to the real edit
    // tool's edits[] batch; until that port (10.2c) it maps onto the
    // placeholder edit's argument names.
    let mut edit = take(&mut placeholders, "edit");
    let inner = edit.execute.clone();
    edit.description = "Replace exact text. args: path, oldText, newText".into();
    edit.parameters = json!({
        "type": "object",
        "required": ["path", "oldText", "newText"],
        "properties": {
            "path": {"type": "string"},
            "oldText": {"type": "string"},
            "newText": {"type": "string"}
        }
    });
    edit.prepare_arguments = None;
    edit.execute = Arc::new(move |id, params: Value, signal, on_update| {
        let args = json!({
            "path": params.get("path").cloned().unwrap_or(Value::Null),
            "old_text": params.get("oldText").cloned().unwrap_or(Value::Null),
            "new_text": params.get("newText").cloned().unwrap_or(Value::Null),
        });
        inner(id, args, signal, on_update)
    });

    let mut bash = create_bash_tool(cwd_bash, BashToolOptions::default(), None);
    bash.description = "Run a shell command. args: command, timeout?".into();
    bash.parameters = json!({
        "type": "object",
        "required": ["command"],
        "properties": {
            "command": {"type": "string"},
            "timeout": {"type": "number"}
        }
    });

    vec![read, write, edit, bash]
}

/// The light tools as definitions. They go in as `baseToolsOverride` in
/// hoocode, so they carry no prompt snippet or guidelines.
pub fn light_tool_definitions(cwd: PathBuf) -> Vec<ToolDefinition> {
    create_light_tools(cwd)
        .into_iter()
        .map(tool_definition_from_agent_tool)
        .collect()
}

/// Same conservative chars/4 heuristic the agent harness uses (UTF-16 units).
fn estimate_string_tokens(text: &str) -> usize {
    text.encode_utf16().count().div_ceil(4)
}

/// What one tool costs on every request: its serialized
/// `{name, description, parameters}` (`measureToolSchemaTokens`).
pub fn measure_tool_schema_tokens(name: &str, description: &str, parameters: &Value) -> usize {
    let serialized = json!({
        "name": name,
        "description": description,
        "parameters": parameters,
    })
    .to_string();
    estimate_string_tokens(&serialized)
}

/// Token breakdown of a session's fixed per-turn surface (`PromptSurface`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptSurface {
    pub system_prompt_tokens: usize,
    pub tool_schema_tokens: usize,
    pub total_tokens: usize,
    pub tools: Vec<(String, usize)>,
}

/// `measurePromptSurface`: the system prompt plus the active tools' schemas.
pub fn measure_prompt_surface(system_prompt: &str, tools: &[AgentTool]) -> PromptSurface {
    let tools: Vec<(String, usize)> = tools
        .iter()
        .map(|t| {
            (
                t.name.clone(),
                measure_tool_schema_tokens(&t.name, &t.description, &t.parameters),
            )
        })
        .collect();
    let system_prompt_tokens = estimate_string_tokens(system_prompt);
    let tool_schema_tokens = tools.iter().map(|(_, n)| n).sum();
    PromptSurface {
        system_prompt_tokens,
        tool_schema_tokens,
        total_tokens: system_prompt_tokens + tool_schema_tokens,
        tools,
    }
}

#[cfg(test)]
mod tests {
    //! Port of the tool-level cases in `test/light-mode.test.ts`.
    use super::*;

    const SHORT_DESCRIPTIONS: [(&str, &str); 4] = [
        ("read", "Read a file. args: path, offset?, limit?"),
        ("write", "Write file (overwrites). args: path, content"),
        ("edit", "Replace exact text. args: path, oldText, newText"),
        ("bash", "Run a shell command. args: command, timeout?"),
    ];

    #[test]
    fn exposes_exactly_the_four_light_tools_with_short_descriptions_and_stripped_schemas() {
        let tools = create_light_tools(std::env::temp_dir());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, LIGHT_TOOL_NAMES);
        for (name, description) in SHORT_DESCRIPTIONS {
            let tool = tools.iter().find(|t| t.name == name).unwrap();
            assert_eq!(tool.description, description);
            assert!(!tool.parameters.to_string().contains("description"));
        }
        // Same key order as TypeBox emits.
        assert_eq!(
            tools[0].parameters.to_string(),
            r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"},"offset":{"type":"number"},"limit":{"type":"number"}}}"#
        );
        for def in light_tool_definitions(std::env::temp_dir()) {
            assert_eq!(def.prompt_snippet, None);
        }
    }

    #[test]
    fn executes_a_flat_light_edit_through_the_edit_tool() {
        let dir = std::env::temp_dir().join(format!("cortex-light-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, "hello old world\n").unwrap();
        let tools = create_light_tools(dir.clone());
        let edit = tools.iter().find(|t| t.name == "edit").unwrap();
        (edit.execute)(
            "e1".into(),
            json!({"path": "hello.txt", "oldText": "old", "newText": "new"}),
            None,
            None,
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello new world\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn light_read_reads() {
        let dir = std::env::temp_dir().join(format!("cortex-light-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo").unwrap();
        let tools = create_light_tools(dir.clone());
        let result = (tools[0].execute)(
            "r1".into(),
            json!({"path": "a.txt", "limit": 1}),
            None,
            None,
        )
        .unwrap();
        let text = match &result.content[0] {
            cortexcode_ai_types::Content::Text(t) => t.text.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            text,
            "one\n\n[1 more lines in file. Use offset=2 to continue.]"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_the_fixed_per_turn_surface_small() {
        let tools = create_light_tools(std::env::temp_dir());
        let prompt = format!(
            "{}\n\nCurrent date: 2026-01-01\nCurrent working directory: /tmp/x",
            cortexcode_code_prompts::LIGHT_SYSTEM_PROMPT
        );
        let surface = measure_prompt_surface(&prompt, &tools);
        assert_eq!(surface.tools.len(), 4);
        assert!(surface.total_tokens > 0);
        assert!(surface.total_tokens < 400, "{surface:?}");
    }
}

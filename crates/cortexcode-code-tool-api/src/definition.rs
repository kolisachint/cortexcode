//! Tool definitions and their wrapping into agent tools
//! (`core/tools/tool-definition-wrapper.ts`, `ToolDefinition` in `extensions/types.ts`).
//!
//! A [`ToolDefinition`] is what the coding agent registers: the agent-facing
//! tool plus prompt metadata, with an `execute` that also receives the
//! [`ToolContext`] (current model, session branch). [`wrap_tool_definition`]
//! turns it into the [`AgentTool`] the agent loop runs. Interactive rendering
//! (`renderCall`/`renderResult`) arrives with the TUI (phase 11).

use cortexcode_agent_types::{
    AgentTool, AgentToolResult, AgentToolUpdateCallback, PrepareArgumentsFn, ToolExecutionMode,
};
use cortexcode_ai_types::{AbortSignal, Model};
use std::sync::Arc;

/// The error a tool's `execute` throws; its `Display` is what the model sees.
pub type ToolError = Box<dyn std::error::Error + Send + Sync>;

/// Read access to the current session branch (`ctx.sessionManager.getBranch()`),
/// as JSON entries/messages in hoocode's wire shape.
pub trait SessionBranch: Send + Sync {
    fn get_branch(&self) -> Vec<serde_json::Value>;
}

/// The subset of hoocode's `ExtensionContext` that built-in tools use.
#[derive(Clone, Default)]
pub struct ToolContext {
    /// `ctx.model`: the model the current request goes to.
    pub model: Option<Model>,
    /// `ctx.sessionManager`: `None` when the tool runs outside a session.
    pub session_manager: Option<Arc<dyn SessionBranch>>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("model", &self.model.as_ref().map(|m| &m.id))
            .field("session_manager", &self.session_manager.is_some())
            .finish()
    }
}

/// Builds a fresh [`ToolContext`] for each call (`ctxFactory`).
pub type ToolContextFactory = Arc<dyn Fn() -> ToolContext + Send + Sync>;

/// A definition's `execute(toolCallId, params, signal, onUpdate, ctx)`.
pub type DefinitionExecuteFn = Arc<
    dyn Fn(
            String,
            serde_json::Value,
            Option<AbortSignal>,
            Option<AgentToolUpdateCallback>,
            Option<&ToolContext>,
        ) -> Result<AgentToolResult, ToolError>
        + Send
        + Sync,
>;

/// `ToolDefinition` (without the TUI renderers).
#[derive(Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub label: String,
    pub description: String,
    /// One-line summary for the system prompt's tool list (`promptSnippet`).
    pub prompt_snippet: Option<String>,
    /// Extra system prompt guidelines (`promptGuidelines`).
    pub prompt_guidelines: Vec<String>,
    /// JSON schema of the arguments, in hoocode's key order.
    pub parameters: serde_json::Value,
    pub prepare_arguments: Option<PrepareArgumentsFn>,
    pub execution_mode: Option<ToolExecutionMode>,
    pub background: bool,
    pub execute: DefinitionExecuteFn,
}

impl std::fmt::Debug for ToolDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDefinition")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("prompt_snippet", &self.prompt_snippet)
            .field("parameters", &self.parameters)
            .finish()
    }
}

/// Wrap a [`ToolDefinition`] into an [`AgentTool`] for the core runtime.
/// `ctx_factory` supplies the context for each call; without it `execute`
/// gets `None`, as in hoocode.
pub fn wrap_tool_definition(
    definition: ToolDefinition,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    let execute = definition.execute.clone();
    AgentTool {
        plain_json_schema: false,
        name: definition.name,
        label: definition.label,
        description: definition.description,
        parameters: definition.parameters,
        prepare_arguments: definition.prepare_arguments,
        execution_mode: definition.execution_mode,
        background: definition.background,
        execute: Arc::new(move |id, params, signal, on_update| {
            let ctx = ctx_factory.as_ref().map(|f| f());
            execute(id, params, signal, on_update, ctx.as_ref())
        }),
    }
}

/// Wrap several definitions with the same context factory.
pub fn wrap_tool_definitions(
    definitions: Vec<ToolDefinition>,
    ctx_factory: Option<ToolContextFactory>,
) -> Vec<AgentTool> {
    definitions
        .into_iter()
        .map(|d| wrap_tool_definition(d, ctx_factory.clone()))
        .collect()
}

/// Synthesize a minimal [`ToolDefinition`] from an [`AgentTool`]
/// (`createToolDefinitionFromAgentTool`): no prompt metadata, context ignored.
pub fn tool_definition_from_agent_tool(tool: AgentTool) -> ToolDefinition {
    let execute = tool.execute.clone();
    ToolDefinition {
        name: tool.name,
        label: tool.label,
        description: tool.description,
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
        parameters: tool.parameters,
        prepare_arguments: tool.prepare_arguments,
        execution_mode: tool.execution_mode,
        background: tool.background,
        execute: Arc::new(move |id, params, signal, on_update, _ctx| {
            execute(id, params, signal, on_update)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::Content;

    fn echo_definition() -> ToolDefinition {
        ToolDefinition {
            name: "echo".into(),
            label: "Echo".into(),
            description: "Echo the model id".into(),
            prompt_snippet: Some("Echo".into()),
            prompt_guidelines: vec![],
            parameters: serde_json::json!({"type": "object"}),
            prepare_arguments: None,
            execution_mode: Some(ToolExecutionMode::Sequential),
            background: false,
            execute: Arc::new(|id, _params, _signal, _update, ctx| {
                let model = ctx
                    .and_then(|c| c.model.as_ref())
                    .map(|m| m.id.clone())
                    .unwrap_or_else(|| "none".into());
                Ok(AgentToolResult {
                    content: vec![Content::text(format!("{id}:{model}"))],
                    details: serde_json::Value::Null,
                    terminate: false,
                })
            }),
        }
    }

    fn model(id: &str) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: "faux".into(),
            provider: "faux".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: Default::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    fn text(result: &AgentToolResult) -> String {
        match &result.content[0] {
            Content::Text(t) => t.text.clone(),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn wrap_passes_the_context_from_the_factory() {
        let factory: ToolContextFactory = Arc::new(|| ToolContext {
            model: Some(model("m1")),
            session_manager: None,
        });
        let tool = wrap_tool_definition(echo_definition(), Some(factory));
        assert_eq!(tool.name, "echo");
        assert_eq!(tool.label, "Echo");
        assert_eq!(tool.execution_mode, Some(ToolExecutionMode::Sequential));
        let result = (tool.execute)("c1".into(), serde_json::json!({}), None, None).unwrap();
        assert_eq!(text(&result), "c1:m1");
    }

    #[test]
    fn wrap_without_factory_passes_no_context() {
        let tool = wrap_tool_definition(echo_definition(), None);
        let result = (tool.execute)("c2".into(), serde_json::json!({}), None, None).unwrap();
        assert_eq!(text(&result), "c2:none");
    }

    #[test]
    fn definition_from_agent_tool_round_trips() {
        let tool = wrap_tool_definition(echo_definition(), None);
        let def = tool_definition_from_agent_tool(tool);
        assert_eq!(def.prompt_snippet, None);
        let result = (def.execute)("c3".into(), serde_json::json!({}), None, None, None).unwrap();
        assert_eq!(text(&result), "c3:none");
    }
}

//! Tool registry and factory for agent tools.
//!
//! Mirrors `tools/default-tools.ts` from the TypeScript
//! `@kolisachint/hoocode-agent-core` package.

use cortexcode_agent_types::{AgentTool, AgentToolCall, AgentToolResult, AgentTools};
use cortexcode_ai_types::{Content, Message, TextContent, ToolResultMessage};
use std::collections::HashMap;

/// A registry of tools that can be queried by name.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, AgentTool>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Replaces any existing tool with the same name.
    pub fn register(&mut self, tool: AgentTool) -> &mut Self {
        self.tools.insert(tool.name.clone(), tool);
        self
    }

    /// Register many tools at once.
    pub fn register_many(&mut self, tools: impl IntoIterator<Item = AgentTool>) -> &mut Self {
        for tool in tools {
            self.register(tool);
        }
        self
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&AgentTool> {
        self.tools.get(name)
    }

    /// Check whether a tool is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Remove a tool by name.
    pub fn remove(&mut self, name: &str) -> Option<AgentTool> {
        self.tools.remove(name)
    }

    /// Return all registered tools as a vector (sorted by name).
    pub fn list(&self) -> Vec<&AgentTool> {
        let mut tools: Vec<_> = self.tools.values().collect();
        tools.sort_by_key(|t| &t.name);
        tools
    }

    /// Return the number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Return true if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Merge another registry into this one. Later registrations win on name
    /// collisions.
    pub fn merge(&mut self, other: ToolRegistry) -> &mut Self {
        for (_, tool) in other.tools {
            self.register(tool);
        }
        self
    }

    /// Convert into an `AgentTools` collection.
    pub fn into_agent_tools(self) -> AgentTools {
        AgentTools::new(self.into_tools())
    }

    /// Consume the registry and return the tools as a vector.
    pub fn into_tools(self) -> Vec<AgentTool> {
        let mut tools: Vec<_> = self.tools.into_values().collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }
}

impl FromIterator<AgentTool> for ToolRegistry {
    fn from_iter<T: IntoIterator<Item = AgentTool>>(iter: T) -> Self {
        let mut registry = Self::new();
        registry.register_many(iter);
        registry
    }
}

/// Create a simple tool result with a text message.
pub fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text: text.into(),
            cache_control: None,
        })],
        details: serde_json::Value::Null,
        terminate: false,
    }
}

/// Create a tool result from an arbitrary serializable value.
pub fn json_result<T: serde::Serialize>(value: T) -> Result<AgentToolResult, serde_json::Error> {
    let text = serde_json::to_string_pretty(&value)?;
    Ok(text_result(text))
}

/// Create a tool result that terminates the agent loop.
pub fn terminate_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text: text.into(),
            cache_control: None,
        })],
        details: serde_json::Value::Null,
        terminate: true,
    }
}

/// Build a tool result message suitable for appending to a conversation.
pub fn tool_result_message(tool_call: &AgentToolCall, result: &AgentToolResult) -> Message {
    Message::ToolResult(ToolResultMessage {
        content: result.content.clone(),
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        is_error: false,
        timestamp: None,
    })
}

/// Factory helpers for constructing common tool shapes.
pub mod factory {
    use super::*;

    /// Build a tool that takes no parameters and returns a static string.
    pub fn simple_tool(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: impl Fn(String) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> AgentTool {
        AgentTool::new(
            name,
            description,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
            }),
            Box::new(move |id, _args, _ctx, _signal| handler(id)),
        )
    }

    /// Build a tool with a single string parameter named `input`.
    pub fn input_tool(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: impl Fn(String, String) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> AgentTool {
        AgentTool::new(
            name,
            description,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"],
            }),
            Box::new(move |id, args, _ctx, _signal| {
                let input = args
                    .get("input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                handler(id, input)
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dummy_tool(name: &str) -> AgentTool {
        AgentTool::new(
            name,
            "a dummy tool",
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
            }),
            Box::new(|_id, _args, _ctx, _signal| Ok(text_result("ok"))),
        )
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(dummy_tool("read"));
        assert!(registry.has("read"));
        assert_eq!(registry.get("read").unwrap().name, "read");
    }

    #[test]
    fn test_list_sorted() {
        let mut registry = ToolRegistry::new();
        registry.register(dummy_tool("zeta"));
        registry.register(dummy_tool("alpha"));
        let names: Vec<_> = registry.list().iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn test_into_agent_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(dummy_tool("read"));
        let tools = registry.into_agent_tools();
        assert_eq!(tools.len(), 1);
    }

    #[test]
    fn test_factory_input_tool() {
        let tool = factory::input_tool("echo", "echo input", |_id, input| Ok(text_result(input)));
        let result = (tool.execute)(
            "call-1".into(),
            serde_json::json!({"input": "hello"}),
            None,
            None,
        )
        .unwrap();
        assert!(result
            .content
            .iter()
            .any(|c| matches!(c, Content::Text(t) if t.text == "hello")));
    }

    #[test]
    fn test_text_result() {
        let result = text_result("done");
        assert!(result
            .content
            .iter()
            .any(|c| matches!(c, Content::Text(t) if t.text == "done")));
        assert!(!result.terminate);
    }

    #[test]
    fn test_terminate_result() {
        let result = terminate_result("stop");
        assert!(result.terminate);
    }
}

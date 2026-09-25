//! Shared types for cortex agents.
//!
//! These types mirror the TypeScript types in `@kolisachint/hoocode-agent-core` and
//! are used by the agent runtime, harness, and tool crates.

use cortexcode_ai_types::{
    AssistantMessage, Content, Message, Model, SimpleStreamOptions, ThinkingLevel,
    ToolResultMessage, UserMessage,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// Tool execution mode
// ---------------------------------------------------------------------------

/// How tool calls from a single assistant message are executed.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

// ---------------------------------------------------------------------------
// Agent tool call
// ---------------------------------------------------------------------------

/// A single tool call content block emitted by an assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Permission gate
// ---------------------------------------------------------------------------

/// Decision returned by a permission gate for a requested tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Approve this invocation.
    Grant,
    /// Approve this invocation and all future invocations of this tool.
    GrantAlways,
    /// Reject this invocation.
    Deny { reason: String },
}

/// A gate that approves or denies tool calls before they are executed.
///
/// Implementations may prompt the user, consult a configuration policy, or
/// auto-approve based on the tool name and arguments.
pub trait PermissionGate: Send + Sync {
    /// Return the permission decision for `tool_call`.
    fn request(&self, tool_call: &AgentToolCall) -> PermissionDecision;
}

/// A finished background tool result.
#[derive(Debug, Clone)]
pub struct BackgroundToolResult {
    pub tool_call: AgentToolCall,
    pub result: AgentToolResult,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Tool lifecycle hooks
// ---------------------------------------------------------------------------

/// Context passed to `before_tool_call`.
#[derive(Debug, Clone)]
pub struct BeforeToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: AgentToolCall,
    pub args: serde_json::Value,
    pub context: AgentContext,
}

/// Result from `before_tool_call`.
#[derive(Debug, Clone)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
}

/// Context passed to `after_tool_call`.
#[derive(Debug, Clone)]
pub struct AfterToolCallContext {
    pub assistant_message: AssistantMessage,
    pub tool_call: AgentToolCall,
    pub args: serde_json::Value,
    pub result: AgentToolResult,
    pub is_error: bool,
    pub context: AgentContext,
}

/// Partial override returned from `after_tool_call`.
#[derive(Debug, Clone)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<Content>>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

// ---------------------------------------------------------------------------
// Agent events
// ---------------------------------------------------------------------------

/// Lifecycle events emitted by the agent loop.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        assistant_message_event: AssistantMessagePartialEvent,
        message: AgentMessage,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        result: AgentToolResult,
        is_error: bool,
    },
    TurnEnd {
        message: AssistantMessage,
        tool_results: Vec<Message>,
    },
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

/// Assistant message partial events (mapped from streaming events).
#[derive(Debug, Clone)]
pub enum AssistantMessagePartialEvent {
    TextStart { index: usize },
    TextDelta { index: usize, delta: String },
    TextEnd { index: usize },
    ThinkingStart { index: usize },
    ThinkingDelta { index: usize, delta: String },
    ThinkingEnd { index: usize },
    ToolCallStart { index: usize },
    ToolCallDelta { index: usize, delta: String },
    ToolCallEnd { index: usize },
}

// ---------------------------------------------------------------------------
// Agent message (hoocode `AgentMessage` union, tagged by `role`)
// ---------------------------------------------------------------------------
//
// Mirrors `AgentMessage` in hoocode `packages/agent/src/types.ts` plus the harness
// roles declared in `packages/agent/src/harness/messages.ts`. Serialized exactly as
// in hoocode session files: `{"role":"user",...}`, `{"role":"bashExecution",...}`.

/// `!` / `!!` bash execution recorded in the conversation (`role: "bashExecution"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashExecutionMessage {
    pub command: String,
    pub output: String,
    /// `undefined` in TS when the process did not exit normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub cancelled: bool,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    pub timestamp: i64,
    /// `!!` prefix: excluded from LLM context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_from_context: Option<bool>,
}

/// Extension-injected message (`role: "custom"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    #[serde(deserialize_with = "cortexcode_ai_types::deserialize_string_or_blocks")]
    pub content: Vec<Content>,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// Summary of an abandoned branch (`role: "branchSummary"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryMessage {
    pub summary: String,
    pub from_id: String,
    pub timestamp: i64,
}

/// Compaction summary (`role: "compactionSummary"`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummaryMessage {
    pub summary: String,
    pub tokens_before: u64,
    /// Absent on entries written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    pub timestamp: i64,
}

/// A conversation message as seen by the agent: an LLM message or a harness message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
    #[serde(rename = "bashExecution")]
    BashExecution(BashExecutionMessage),
    #[serde(rename = "custom")]
    Custom(CustomMessage),
    #[serde(rename = "branchSummary")]
    BranchSummary(BranchSummaryMessage),
    #[serde(rename = "compactionSummary")]
    CompactionSummary(CompactionSummaryMessage),
}

impl AgentMessage {
    /// Wrap an LLM message.
    pub fn from_message(message: Message) -> Self {
        match message {
            Message::User(m) => AgentMessage::User(m),
            Message::Assistant(m) => AgentMessage::Assistant(m),
            Message::ToolResult(m) => AgentMessage::ToolResult(m),
        }
    }

    /// A user text message stamped with the current time.
    pub fn user_text(text: impl Into<String>) -> Self {
        AgentMessage::User(UserMessage {
            content: vec![Content::text(text)],
            timestamp: cortexcode_ai_types::now_ms(),
        })
    }

    /// The LLM message, if this is a user/assistant/toolResult message.
    pub fn extract_message(&self) -> Option<Message> {
        match self {
            AgentMessage::User(m) => Some(Message::User(m.clone())),
            AgentMessage::Assistant(m) => Some(Message::Assistant(m.clone())),
            AgentMessage::ToolResult(m) => Some(Message::ToolResult(m.clone())),
            _ => None,
        }
    }

    /// The `role` discriminator as written on the wire.
    pub fn role(&self) -> &'static str {
        match self {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            AgentMessage::ToolResult(_) => "toolResult",
            AgentMessage::BashExecution(_) => "bashExecution",
            AgentMessage::Custom(_) => "custom",
            AgentMessage::BranchSummary(_) => "branchSummary",
            AgentMessage::CompactionSummary(_) => "compactionSummary",
        }
    }

    /// Unix-ms timestamp of the message.
    pub fn timestamp(&self) -> i64 {
        match self {
            AgentMessage::User(m) => m.timestamp,
            AgentMessage::Assistant(m) => m.timestamp,
            AgentMessage::ToolResult(m) => m.timestamp,
            AgentMessage::BashExecution(m) => m.timestamp,
            AgentMessage::Custom(m) => m.timestamp,
            AgentMessage::BranchSummary(m) => m.timestamp,
            AgentMessage::CompactionSummary(m) => m.timestamp,
        }
    }
}

impl From<Message> for AgentMessage {
    fn from(m: Message) -> Self {
        AgentMessage::from_message(m)
    }
}

// ---------------------------------------------------------------------------
// Agent tool result
// ---------------------------------------------------------------------------

/// Final or partial result produced by a tool.
#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub content: Vec<Content>,
    pub details: serde_json::Value,
    pub terminate: bool,
}

// ---------------------------------------------------------------------------
// Agent tool definition
// ---------------------------------------------------------------------------

/// Tool definition used by the agent runtime.
///
/// `AgentTool` stores function pointers and boxed closures, so it does not
/// implement `Clone` or `Debug`. Use the tool-building helpers to create one.
#[allow(clippy::type_complexity)]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    pub label: String,
    pub parameters: serde_json::Value,
    pub prepare_arguments: Option<PrepareArgumentsFn>,
    pub execute: ToolExecuteFn,
    pub background: bool,
    pub execution_mode: Option<ToolExecutionMode>,
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("label", &self.label)
            .field("parameters", &self.parameters)
            .field("background", &self.background)
            .field("execution_mode", &self.execution_mode)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Context inner (wraps AgentTool list so outer types can be Clone)
// ---------------------------------------------------------------------------

/// Wrapper around `Vec<AgentTool>` that provides Clone (via Arc).
#[derive(Clone)]
pub struct AgentTools(pub std::sync::Arc<Vec<AgentTool>>);

impl std::fmt::Debug for AgentTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter()).finish()
    }
}

impl AgentTools {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(tools: Vec<AgentTool>) -> Self {
        Self(std::sync::Arc::new(tools))
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentTool> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<&AgentTool> {
        self.0.iter().find(|t| t.name == name)
    }
}

impl AgentTool {
    /// Create a new agent tool.
    #[allow(clippy::type_complexity)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        execute: Box<
            dyn Fn(
                    String,
                    serde_json::Value,
                    Option<cortexcode_ai_types::AbortSignal>,
                    Option<AgentToolUpdateCallback>,
                )
                    -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
                + Send
                + Sync,
        >,
    ) -> Self {
        let name = name.into();
        Self {
            name: name.clone(),
            description: description.into(),
            label: name,
            parameters,
            prepare_arguments: None,
            execute: std::sync::Arc::from(execute),
            background: false,
            execution_mode: None,
        }
    }
}

impl Clone for AgentTool {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            description: self.description.clone(),
            label: self.label.clone(),
            parameters: self.parameters.clone(),
            prepare_arguments: self.prepare_arguments.clone(),
            execute: std::sync::Arc::clone(&self.execute),
            background: self.background,
            execution_mode: self.execution_mode.clone(),
        }
    }
}

/// A tool's `execute`. Shared so that cloning a tool keeps a working closure.
pub type ToolExecuteFn = std::sync::Arc<
    dyn Fn(
            String,
            serde_json::Value,
            Option<cortexcode_ai_types::AbortSignal>,
            Option<AgentToolUpdateCallback>,
        ) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
        + Send
        + Sync,
>;

/// A tool's `prepareArguments` compatibility shim.
pub type PrepareArgumentsFn =
    std::sync::Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>;

/// Callback used by tools to stream partial execution updates.
pub type AgentToolUpdateCallback = Box<dyn Fn(AgentToolResult) + Send>;

// ---------------------------------------------------------------------------
// Agent context
// ---------------------------------------------------------------------------

/// The context passed to the agent loop.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: AgentTools,
}

impl AgentContext {
    pub fn new(system_prompt: String, messages: Vec<AgentMessage>, tools: Vec<AgentTool>) -> Self {
        Self {
            system_prompt,
            messages,
            tools: AgentTools::new(tools),
        }
    }

    /// Create an AgentContext directly from an `AgentTools` value (avoids cloning).
    pub fn new_with_tools(
        system_prompt: String,
        messages: Vec<AgentMessage>,
        tools: AgentTools,
    ) -> Self {
        Self {
            system_prompt,
            messages,
            tools,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent state
// ---------------------------------------------------------------------------

/// Public agent state.
#[derive(Debug, Clone)]
pub struct AgentState {
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub tools: AgentTools,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: HashSet<String>,
    pub error_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Agent loop turn update
// ---------------------------------------------------------------------------

/// Replacement runtime state used by the agent loop before starting another provider request.
#[derive(Debug, Clone)]
pub struct AgentLoopTurnUpdate {
    pub context: Option<AgentContext>,
    pub model: Option<Model>,
    pub thinking_level: Option<ThinkingLevel>,
}

// ---------------------------------------------------------------------------
// Agent loop config
// ---------------------------------------------------------------------------

/// Configuration for the agent loop.
///
/// Contains optional callback closures and is not `Clone` nor fully `Debug`.
#[allow(clippy::type_complexity)]
pub struct AgentLoopConfig {
    pub model: Model,
    pub reasoning: Option<ThinkingLevel>,
    pub convert_to_llm: Option<
        Box<
            dyn Fn(
                    Vec<AgentMessage>,
                )
                    -> Result<Vec<Message>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub transform_context: Option<
        Box<
            dyn Fn(
                    Vec<AgentMessage>,
                    Option<cortexcode_ai_types::AbortSignal>,
                )
                    -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub get_api_key: Option<
        Box<
            dyn Fn(String) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub should_stop_after_turn: Option<
        Box<
            dyn Fn(
                    ShouldStopAfterTurnContext,
                ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub prepare_next_turn: Option<
        Box<
            dyn Fn(
                    PrepareNextTurnContext,
                )
                    -> Result<Option<AgentLoopTurnUpdate>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub get_steering_messages: Option<
        Box<dyn Fn() -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> + Send>,
    >,
    pub get_follow_up_messages: Option<
        Box<dyn Fn() -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> + Send>,
    >,
    pub create_background_result_message:
        Option<Box<dyn Fn(BackgroundToolResult) -> AgentMessage + Send>>,
    pub create_background_placeholder: Option<Box<dyn Fn(AgentToolCall) -> Option<String> + Send>>,
    pub on_background_task_count_change: Option<Box<dyn Fn(usize) + Send>>,
    pub before_tool_call: Option<
        Box<
            dyn Fn(
                    BeforeToolCallContext,
                    Option<cortexcode_ai_types::AbortSignal>,
                )
                    -> Result<Option<BeforeToolCallResult>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    pub after_tool_call: Option<
        Box<
            dyn Fn(
                    AfterToolCallContext,
                    Option<cortexcode_ai_types::AbortSignal>,
                )
                    -> Result<Option<AfterToolCallResult>, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
    >,
    /// Gate that approves or denies tool calls before execution.
    pub permission_gate: Option<std::sync::Arc<dyn PermissionGate>>,
    pub tool_execution: ToolExecutionMode,
    /// Stream function used to call the LLM.
    pub stream_fn: Option<
        Box<
            dyn Fn(
                    Model,
                    cortexcode_ai_types::Context,
                    SimpleStreamOptions,
                ) -> Result<
                    cortexcode_ai_stream::AssistantMessageEventStream,
                    Box<dyn std::error::Error + Send + Sync>,
                > + Send
                + Sync,
        >,
    >,
    pub signal: Option<cortexcode_ai_types::AbortSignal>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub max_retry_delay_ms: Option<u64>,
    pub thinking_budgets: Option<cortexcode_ai_types::ThinkingBudgets>,
    pub thinking_display: Option<cortexcode_ai_types::ThinkingDisplay>,
    pub transport: Option<cortexcode_ai_types::Transport>,
    pub on_payload: Option<Box<dyn Fn(String) + Send>>,
    pub on_response: Option<Box<dyn Fn(String) + Send>>,
    pub cache_control_format: Option<cortexcode_ai_types::CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub prompt_suffix: Option<String>,
}

impl std::fmt::Debug for AgentLoopConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentLoopConfig")
            .field("model", &self.model)
            .field("reasoning", &self.reasoning)
            .field("tool_execution", &self.tool_execution)
            .field("session_id", &self.session_id)
            .field("max_retry_delay_ms", &self.max_retry_delay_ms)
            .field("thinking_budgets", &self.thinking_budgets)
            .field("thinking_display", &self.thinking_display)
            .field("transport", &self.transport)
            .field("permission_gate", &self.permission_gate.is_some())
            .field(
                "send_session_affinity_headers",
                &self.send_session_affinity_headers,
            )
            .field(
                "supports_long_cache_retention",
                &self.supports_long_cache_retention,
            )
            .field("prompt_suffix", &self.prompt_suffix)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Should-stop context
// ---------------------------------------------------------------------------

/// Context passed to `should_stop_after_turn`.
#[derive(Debug, Clone)]
pub struct ShouldStopAfterTurnContext {
    pub message: AssistantMessage,
    pub tool_results: Vec<Message>,
    pub context: AgentContext,
    pub new_messages: Vec<AgentMessage>,
}

/// Context passed to `prepare_next_turn`.
pub type PrepareNextTurnContext = ShouldStopAfterTurnContext;

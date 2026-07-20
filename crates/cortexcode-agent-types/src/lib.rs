//! Shared types for cortex agents.
//!
//! These types mirror the TypeScript types in `@kolisachint/hoocode-agent-core` and
//! are used by the agent runtime, harness, and tool crates.

use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEventStream, Content, Message, Model, SimpleStreamOptions,
    ThinkingLevel,
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
// Agent message (extensible)
// ---------------------------------------------------------------------------

/// Agent message: a `Message` or a custom app message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub inner: AgentMessageInner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessageInner {
    Standard(Message),
    Custom {
        role: String,
        content: Vec<Content>,
        timestamp: Option<i64>,
    },
}

impl AgentMessage {
    pub fn new(inner: AgentMessageInner) -> Self {
        Self { inner }
    }

    pub fn from_message(message: Message) -> Self {
        Self {
            inner: AgentMessageInner::Standard(message),
        }
    }

    /// Extract the inner `Message` if this is a standard message.
    pub fn extract_message(&self) -> Option<Message> {
        match &self.inner {
            AgentMessageInner::Standard(m) => Some(m.clone()),
            AgentMessageInner::Custom { .. } => None,
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
    pub prepare_arguments: Option<Box<dyn Fn(serde_json::Value) -> serde_json::Value + Send>>,
    pub execute: Box<
        dyn Fn(
                String,
                serde_json::Value,
                Option<cortexcode_ai_types::AbortSignal>,
                Option<AgentToolUpdateCallback>,
            ) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
            + Send,
    >,
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
    /// Clone the tool's identifying fields (not the closures).
    /// Used when we need an owned copy for background dispatch.
    pub fn clone_via_fields(&self) -> Self {
        Self {
            name: self.name.clone(),
            description: self.description.clone(),
            label: self.label.clone(),
            parameters: self.parameters.clone(),
            prepare_arguments: None,
            // The execute closure is moved, so we need to signal this is a copy
            // In practice, tools should be constructed fresh for each use
            execute: Box::new(|_id, _params, _signal, _update| {
                Err("Cloned tool: execute not available".into())
            }),
            background: self.background,
            execution_mode: self.execution_mode.clone(),
        }
    }
}

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
    pub tool_execution: ToolExecutionMode,
    /// Stream function used to call the LLM.
    pub stream_fn: Option<
        Box<
            dyn Fn(
                    Model,
                    cortexcode_ai_types::Context,
                    SimpleStreamOptions,
                ) -> Result<
                    Box<dyn AssistantMessageEventStream>,
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

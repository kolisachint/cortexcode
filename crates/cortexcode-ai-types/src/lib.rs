//! Core types for the cortex AI namespace.
//!
//! These types mirror the TypeScript types in `@kolisachint/hoocode-ai` and are used
//! by all AI providers, the agent runtime, and tooling crates.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

/// Text content block.
#[derive(Debug, Clone, PartialEq)]
pub struct TextContent {
    pub text: String,
    /// Optional cache control for prompt caching.
    pub cache_control: Option<CacheControl>,
}

/// Image content block (base64-encoded data or URI).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageContent {
    pub data: String,
    pub media_type: String,
    pub cache_control: Option<CacheControl>,
}

/// Thinking/reasoning content block.
#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingContent {
    pub thinking: String,
    pub signature: Option<String>,
}

/// A tool-call content block inside an assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallContent {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Union of all content-block types.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(TextContent),
    Image(ImageContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallContent),
}

// ---------------------------------------------------------------------------
// Cache control
// ---------------------------------------------------------------------------

/// Cache-control marker for prompt caching support.
#[derive(Debug, Clone, PartialEq)]
pub enum CacheControl {
    /// Anthropic-style `cache_control` with optional TTL.
    Ephemeral,
    /// Long cache retention (e.g. "24h").
    Ttl(String),
}

// ---------------------------------------------------------------------------
// Stop reason
// ---------------------------------------------------------------------------

/// Reason why an assistant message stopped generating.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    StopSequence,
    MaxTokens,
    ToolUse,
    Error,
    Aborted,
    Other(String),
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Token usage statistics for a model request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: Cost,
}

/// Cost breakdown (in USD).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// A user message.
#[derive(Debug, Clone, PartialEq)]
pub struct UserMessage {
    pub content: Vec<Content>,
    pub timestamp: Option<i64>,
}

/// An assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    pub stop_reason: Option<StopReason>,
    pub stop_sequence: Option<String>,
    pub usage: Option<Usage>,
    pub timestamp: Option<i64>,
    /// Human-readable error message, set when `stop_reason` is `Error` or `Aborted`.
    pub error_message: Option<String>,
}

/// A tool-result message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultMessage {
    pub content: Vec<Content>,
    pub tool_call_id: String,
    pub is_error: bool,
    pub timestamp: Option<i64>,
}

/// Union of all message types.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// JSON Schema definition for tool parameters.
pub type JsonSchema = serde_json::Value;

/// Shape of a tool that can be called by the model.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: JsonSchema,
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// API identifier string.
pub type Api = String;

/// Provider identifier string.
pub type Provider = String;

/// Thinking/reasoning level for models that support it.
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

/// Per-level thinking token budgets.
#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingBudgets {
    pub minimal: Option<u64>,
    pub low: Option<u64>,
    pub medium: Option<u64>,
    pub high: Option<u64>,
    pub xhigh: Option<u64>,
}

/// Mapping from `ThinkingLevel` to provider-specific values.
pub type ThinkingLevelMap = HashMap<String, serde_json::Value>;

/// Transport preference for streaming.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Auto,
    Sse,
    Stdio,
    StreamableHttp,
}

impl Default for Transport {
    fn default() -> Self {
        Transport::Auto
    }
}

/// Model definition.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: Provider,
    pub base_url: String,
    pub reasoning: bool,
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub input: Vec<String>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    pub headers: Option<HashMap<String, String>>,
}

/// Model pricing (per million tokens).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// The context passed to the model for each request.
#[derive(Debug, Clone)]
pub struct Context {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
}

impl Context {
    pub fn new(system_prompt: String, messages: Vec<Message>, tools: Vec<Tool>) -> Self {
        Self {
            system_prompt,
            messages,
            tools,
        }
    }
}

// ---------------------------------------------------------------------------
// Stream options
// ---------------------------------------------------------------------------

/// Stream options used by the low-level provider `stream` method.
#[derive(Debug, Clone)]
pub struct StreamOptions {
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub max_retries: Option<u64>,
    pub max_retry_delay_ms: Option<u64>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub thinking_display: Option<ThinkingDisplay>,
    pub transport: Option<Transport>,
    pub on_payload: Option<String>,
    pub on_response: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub prompt_suffix: Option<String>,
}

/// Simple stream options used by the `streamSimple` function.
#[derive(Debug, Clone, Default)]
pub struct SimpleStreamOptions {
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub max_retries: Option<u64>,
    pub max_retry_delay_ms: Option<u64>,
    pub reasoning: Option<ThinkingLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub thinking_display: Option<ThinkingDisplay>,
    pub transport: Option<Transport>,
    pub on_payload: Option<String>,
    pub on_response: Option<String>,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub prompt_suffix: Option<String>,
}

/// Cache control format for prompt caching.
#[derive(Debug, Clone, PartialEq)]
pub enum CacheControlFormat {
    Anthropic,
}

/// Thinking display mode for adaptive-thinking models.
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingDisplay {
    Summarized,
    Omitted,
}

/// Provider stream options (used internally by providers).
#[derive(Debug, Clone)]
pub struct ProviderStreamOptions {
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub max_retries: Option<u64>,
    pub max_retry_delay_ms: Option<u64>,
    pub reasoning: Option<ThinkingLevel>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub thinking_display: Option<ThinkingDisplay>,
    pub transport: Option<Transport>,
    pub headers: Option<HashMap<String, String>>,
    pub cache_control_format: Option<CacheControlFormat>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub prompt_suffix: Option<String>,
    pub on_payload: Option<String>,
    pub on_response: Option<String>,
}

// ---------------------------------------------------------------------------
// Stream events
// ---------------------------------------------------------------------------

/// Events emitted during streaming of an assistant message.
#[derive(Debug, Clone)]
pub enum AssistantMessageEvent {
    /// Streaming has started; `partial` contains the initial message.
    Start {
        partial: AssistantMessage,
    },
    /// A text content block has started.
    TextStart {
        partial: AssistantMessage,
        index: usize,
    },
    /// Delta for a text content block.
    TextDelta {
        partial: AssistantMessage,
        index: usize,
        delta: String,
    },
    /// A text content block has ended.
    TextEnd {
        partial: AssistantMessage,
        index: usize,
    },
    /// A thinking content block has started.
    ThinkingStart {
        partial: AssistantMessage,
        index: usize,
    },
    /// Delta for a thinking content block.
    ThinkingDelta {
        partial: AssistantMessage,
        index: usize,
        delta: String,
    },
    /// A thinking content block has ended.
    ThinkingEnd {
        partial: AssistantMessage,
        index: usize,
    },
    /// A tool-call content block has started.
    ToolCallStart {
        partial: AssistantMessage,
        index: usize,
    },
    /// Delta for a tool-call block.
    ToolCallDelta {
        partial: AssistantMessage,
        index: usize,
        delta: String,
    },
    /// A tool-call content block has ended.
    ToolCallEnd {
        partial: AssistantMessage,
        index: usize,
    },
    /// Streaming completed successfully.
    Done {
        message: AssistantMessage,
    },
    /// An error occurred during streaming.
    Error {
        error: AssistantMessage,
    },
}

// ---------------------------------------------------------------------------
// Abort signal (simplified)
// ---------------------------------------------------------------------------

/// A simple cancellation token.
#[derive(Debug, Clone)]
pub struct AbortSignal {
    aborted: bool,
}

impl AbortSignal {
    pub fn new() -> Self {
        Self { aborted: false }
    }

    pub fn aborted(&self) -> bool {
        self.aborted
    }

    pub fn abort(&mut self) {
        self.aborted = true;
    }
}

impl Default for AbortSignal {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// A stream of assistant message events.
///
/// Provides both event-by-event iteration and a final result promise.
pub trait AssistantMessageEventStream: Send {
    /// Return the next event from the stream, blocking until one is available.
    /// Returns `None` when the stream is exhausted.
    fn next_event(&mut self) -> Option<AssistantMessageEvent>;
    /// Wait for the stream to finish and return the final result.
    fn result(&mut self) -> AssistantMessage;
}

/// Result of a provider stream call.
pub type ProviderStreamResult = Box<dyn AssistantMessageEventStream>;

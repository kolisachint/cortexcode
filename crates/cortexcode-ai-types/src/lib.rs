//! Core types for the cortex AI namespace.
//!
//! These types mirror the TypeScript types in `@kolisachint/hoocode-ai` and are used
//! by all AI providers, the agent runtime, and tooling crates.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Wire format
// ---------------------------------------------------------------------------
//
// Every type in this section serializes exactly like its TypeScript counterpart in
// hoocode `packages/ai/src/types.ts` (pinned v0.5.89): internally tagged on `type`
// (content) / `role` (messages), camelCase field names, optional fields omitted when
// absent. Session files, the RPC protocol and `--mode json` output depend on this.
// Fields marked "not on the wire" are Rust-side request-building hints.

/// Current Unix time in milliseconds (TS `Date.now()`).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

/// Text content block (`{"type":"text"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextContent {
    pub text: String,
    /// Provider message metadata, e.g. OpenAI Responses item id or `TextSignatureV1` JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

/// Image content block (`{"type":"image"}`), base64 data.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
    pub data: String,
    /// MIME type, e.g. `image/png` (`mimeType` on the wire).
    #[serde(rename = "mimeType")]
    pub media_type: String,
}

/// Thinking/reasoning content block (`{"type":"thinking"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingContent {
    pub thinking: String,
    /// Provider signature / reasoning item id / redacted payload (`thinkingSignature`).
    #[serde(
        rename = "thinkingSignature",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub signature: Option<String>,
    /// The thinking was redacted by safety filters; the payload is in `signature`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub redacted: bool,
}

/// A tool-call content block inside an assistant message (`{"type":"toolCall"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallContent {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Google-specific opaque signature for reusing thought context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Union of all content-block types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    #[serde(rename = "toolCall")]
    ToolCall(ToolCallContent),
}

impl Content {
    /// Plain text block.
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text(TextContent::new(text))
    }
}

// ---------------------------------------------------------------------------
// Cache retention
// ---------------------------------------------------------------------------

/// Prompt cache retention preference (TS `CacheRetention`). Providers map it
/// to their own markers; see `cortexcode_ai_util::resolve_cache_retention`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

// ---------------------------------------------------------------------------
// Stop reason
// ---------------------------------------------------------------------------

/// Reason why an assistant message stopped generating (TS `StopReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// `"stop"`: natural end of turn.
    #[default]
    Stop,
    /// `"length"`: output token limit reached.
    Length,
    /// `"toolUse"`: the model wants tool results.
    ToolUse,
    /// `"error"`.
    Error,
    /// `"aborted"`.
    Aborted,
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Token usage statistics for a model request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
    pub cost: Cost,
}

/// Cost breakdown (in USD).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
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

/// Deserialize TS `string | (TextContent | ImageContent)[]` into blocks.
/// A plain string becomes one text block; serialization always emits the array form.
pub fn deserialize_string_or_blocks<'de, D>(de: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrBlocks {
        Text(String),
        Blocks(Vec<Content>),
    }
    Ok(match StringOrBlocks::deserialize(de)? {
        StringOrBlocks::Text(t) => vec![Content::text(t)],
        StringOrBlocks::Blocks(b) => b,
    })
}

/// A user message (`{"role":"user"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    #[serde(deserialize_with = "deserialize_string_or_blocks")]
    pub content: Vec<Content>,
    /// Unix time in milliseconds.
    #[serde(default)]
    pub timestamp: i64,
}

/// An assistant message (`{"role":"assistant"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    pub content: Vec<Content>,
    /// API that produced the message, e.g. `anthropic-messages`.
    #[serde(default)]
    pub api: String,
    /// Provider id, e.g. `anthropic`.
    #[serde(default)]
    pub provider: String,
    /// Requested model id.
    #[serde(default)]
    pub model: String,
    /// Concrete model reported by the provider when it differs from `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    /// Provider response/message id when the API exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Redacted provider/runtime diagnostics (TS `AssistantMessageDiagnostic[]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub stop_reason: StopReason,
    /// Human-readable error, set when `stop_reason` is `Error` or `Aborted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Unix time in milliseconds.
    #[serde(default)]
    pub timestamp: i64,
}

impl AssistantMessage {
    /// Empty message attributed to `model` (TS: `{ api: model.api, provider: model.provider,
    /// model: model.id, usage: empty, stopReason: "stop", timestamp: Date.now() }`).
    pub fn for_model(model: &Model) -> Self {
        Self {
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            timestamp: now_ms(),
            ..Default::default()
        }
    }
}

/// A tool-result message (`{"role":"toolResult"}`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    /// Name of the tool that produced this result.
    pub tool_name: String,
    pub content: Vec<Content>,
    /// Tool-specific metadata (TS `details?: TDetails`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub is_error: bool,
    /// Unix time in milliseconds.
    #[serde(default)]
    pub timestamp: i64,
}

/// Union of all message types, tagged by `role`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
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
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Transport {
    #[default]
    Auto,
    Sse,
    Stdio,
    StreamableHttp,
}

/// Model definition (hoocode `Model`; same JSON shape as the catalog and
/// `models.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: Api,
    pub provider: Provider,
    pub base_url: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<ThinkingLevelMap>,
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// Provider compatibility overrides, kept as JSON so `models.json` overrides
    /// can deep-merge them. Read typed views with [`Model::compat_as`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<serde_json::Value>,
}

impl Model {
    /// The `compat` object read as one of the compat structs
    /// ([`OpenAICompletionsCompat`], [`OpenAIResponsesCompat`],
    /// [`AnthropicMessagesCompat`]); every field unset when absent or malformed.
    pub fn compat_as<T: serde::de::DeserializeOwned + Default>(&self) -> T {
        self.compat
            .as_ref()
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default()
    }
}

/// Model pricing (per million tokens).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// `OpenAICompletionsCompat`: overrides for OpenAI-compatible chat
/// completions endpoints. Unset fields are auto-detected from the base URL.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenAICompletionsCompat {
    pub supports_store: Option<bool>,
    pub supports_developer_role: Option<bool>,
    pub supports_reasoning_effort: Option<bool>,
    pub supports_usage_in_streaming: Option<bool>,
    /// `"max_completion_tokens"` or `"max_tokens"`.
    pub max_tokens_field: Option<String>,
    pub requires_tool_result_name: Option<bool>,
    pub requires_assistant_after_tool_result: Option<bool>,
    pub requires_thinking_as_text: Option<bool>,
    pub requires_reasoning_content_on_assistant_messages: Option<bool>,
    /// `openai`, `openrouter`, `deepseek`, `together`, `zai`, `qwen`, `qwen-chat-template`.
    pub thinking_format: Option<String>,
    /// `OpenRouterRouting`, sent as the request's `provider` field.
    pub open_router_routing: Option<serde_json::Value>,
    /// `VercelGatewayRouting`.
    pub vercel_gateway_routing: Option<serde_json::Value>,
    pub zai_tool_stream: Option<bool>,
    pub supports_strict_mode: Option<bool>,
    /// `"strict"` or `"none"`.
    pub tool_call_constraint: Option<String>,
    /// `"anthropic"`.
    pub cache_control_format: Option<String>,
    pub send_session_affinity_headers: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub prompt_suffix: Option<String>,
}

/// `OpenAIResponsesCompat`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OpenAIResponsesCompat {
    pub send_session_id_header: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
}

/// `AnthropicMessagesCompat`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnthropicMessagesCompat {
    pub supports_eager_tool_input_streaming: Option<bool>,
    pub supports_long_cache_retention: Option<bool>,
    pub supports_tool_search: Option<bool>,
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
    /// `cacheRetention`: prompt cache retention preference (default: long).
    pub cache_retention: Option<CacheRetention>,
    pub send_session_affinity_headers: Option<bool>,
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
    /// `cacheRetention`: prompt cache retention preference (default: long).
    pub cache_retention: Option<CacheRetention>,
    pub send_session_affinity_headers: Option<bool>,
    pub prompt_suffix: Option<String>,
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
    /// `cacheRetention`: prompt cache retention preference (default: long).
    pub cache_retention: Option<CacheRetention>,
    pub send_session_affinity_headers: Option<bool>,
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
    Start { partial: AssistantMessage },
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
    Done { message: AssistantMessage },
    /// An error occurred during streaming.
    Error { error: AssistantMessage },
}

// ---------------------------------------------------------------------------
// Abort signal
// ---------------------------------------------------------------------------

/// `AbortSignal`: a cancellation token shared by every clone.
///
/// Wraps `tokio_util::sync::CancellationToken`. Aborting any clone aborts all
/// of them, and async code can wait for it with [`AbortSignal::cancelled`].
#[derive(Debug, Clone, Default)]
pub struct AbortSignal(tokio_util::sync::CancellationToken);

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// `signal.aborted`.
    pub fn aborted(&self) -> bool {
        self.0.is_cancelled()
    }

    /// `controller.abort()`.
    pub fn abort(&self) {
        self.0.cancel();
    }

    /// Resolves once the signal is aborted.
    pub fn cancelled(&self) -> tokio_util::sync::WaitForCancellationFuture<'_> {
        self.0.cancelled()
    }

    /// The underlying token, e.g. for `tokio::select!` or child tokens.
    pub fn token(&self) -> &tokio_util::sync::CancellationToken {
        &self.0
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_blocks_use_hoocode_tags_and_names() {
        let blocks = vec![
            Content::text("hi"),
            Content::Image(ImageContent {
                data: "AA==".into(),
                media_type: "image/png".into(),
            }),
            Content::Thinking(ThinkingContent {
                thinking: "t".into(),
                signature: Some("s".into()),
                redacted: false,
            }),
            Content::ToolCall(ToolCallContent {
                id: "c1".into(),
                name: "read".into(),
                arguments: json!({"path": "a"}),
                thought_signature: None,
            }),
        ];
        assert_eq!(
            serde_json::to_value(&blocks).unwrap(),
            json!([
                {"type": "text", "text": "hi"},
                {"type": "image", "data": "AA==", "mimeType": "image/png"},
                {"type": "thinking", "thinking": "t", "thinkingSignature": "s"},
                {"type": "toolCall", "id": "c1", "name": "read", "arguments": {"path": "a"}}
            ])
        );
    }

    #[test]
    fn messages_are_tagged_by_role_in_camel_case() {
        let m = Message::ToolResult(ToolResultMessage {
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: vec![Content::text("x")],
            details: None,
            is_error: false,
            timestamp: 5,
        });
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"role": "toolResult", "toolCallId": "c1", "toolName": "read",
                   "content": [{"type": "text", "text": "x"}], "isError": false, "timestamp": 5})
        );
        let user: Message =
            serde_json::from_value(json!({"role": "user", "content": "hello", "timestamp": 1}))
                .unwrap();
        assert_eq!(
            user,
            Message::User(UserMessage {
                content: vec![Content::text("hello")],
                timestamp: 1
            })
        );
    }

    #[test]
    fn abort_reaches_every_clone() {
        let signal = AbortSignal::new();
        let seen_by_tool = signal.clone();
        assert!(!seen_by_tool.aborted());
        signal.abort();
        assert!(seen_by_tool.aborted());
        assert!(!AbortSignal::default().token().is_cancelled());
    }

    #[test]
    fn stop_reasons_match_ts_strings() {
        for (r, s) in [
            (StopReason::Stop, "stop"),
            (StopReason::Length, "length"),
            (StopReason::ToolUse, "toolUse"),
            (StopReason::Error, "error"),
            (StopReason::Aborted, "aborted"),
        ] {
            assert_eq!(serde_json::to_value(r).unwrap(), json!(s));
        }
    }

    #[test]
    fn for_model_stamps_identity() {
        let model = Model {
            compat: None,
            id: "m".into(),
            name: "M".into(),
            api: "openai-completions".into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 1,
            max_tokens: 1,
            headers: None,
        };
        let a = AssistantMessage::for_model(&model);
        assert_eq!(
            (a.api.as_str(), a.provider.as_str(), a.model.as_str()),
            ("openai-completions", "p", "m")
        );
        assert_eq!(a.stop_reason, StopReason::Stop);
        assert!(a.timestamp > 0);
    }
}

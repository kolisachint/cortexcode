//! Request construction for the Anthropic Messages API: the option mapping of
//! `streamSimpleAnthropic`, `createClient`'s headers, `buildParams`,
//! `convertMessages` and `convertTools` of hoocode `providers/anthropic.ts`
//! (v0.5.89).

use std::collections::HashMap;

use cortexcode_ai_types::{
    AbortSignal, AnthropicMessagesCompat, AssistantMessage, CacheRetention, Content, Context,
    Message, Model, OnPayload, OnResponse, SimpleStreamOptions, ThinkingBudgets, ThinkingDisplay,
    ThinkingLevel, Tool, ToolResultMessage, UserContent,
};
use cortexcode_ai_util::{
    build_copilot_dynamic_headers, has_copilot_vision_input, resolve_cache_retention,
    transform_messages,
};
use serde_json::{json, Value};

/// Claude Code version mimicked on OAuth requests (`claudeCodeVersion`).
const CLAUDE_CODE_VERSION: &str = "2.1.280";

/// Claude Code 2.x tool names in canonical casing (`claudeCodeTools`).
const CLAUDE_CODE_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "KillShell",
    "NotebookEdit",
    "Skill",
    "Task",
    "TaskOutput",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
];

const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// `TOOL_SEARCH_TOOL`: the BM25 server-side tool-search tool.
const TOOL_SEARCH_TOOL_TYPE: &str = "tool_search_tool_bm25_20251119";
const TOOL_SEARCH_TOOL_NAME: &str = "tool_search_tool_bm25";

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// `AnthropicOptions`: `StreamOptions` plus the thinking/tool-choice knobs.
#[derive(Debug, Clone, Default)]
pub struct AnthropicOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub cache_retention: Option<CacheRetention>,
    pub headers: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    /// SDK client retries (default 2).
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub metadata: Option<Value>,
    pub thinking_enabled: Option<bool>,
    pub thinking_budget_tokens: Option<u64>,
    /// `low` | `medium` | `high` | `xhigh` | `max`.
    pub effort: Option<String>,
    pub thinking_display: Option<ThinkingDisplay>,
    /// Defaults to true.
    pub interleaved_thinking: Option<bool>,
    /// `"auto" | "any" | "none"` or `{type: "tool", name}`.
    pub tool_choice: Option<Value>,
    pub on_payload: Option<OnPayload>,
    pub on_response: Option<OnResponse>,
}

/// `streamSimpleAnthropic`'s option mapping (`buildBaseOptions` plus the
/// thinking mode for the model).
pub fn simple_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: String,
) -> AnthropicOptions {
    let base = AnthropicOptions {
        temperature: options.temperature,
        max_tokens: options
            .max_tokens
            .or((model.max_tokens > 0).then(|| model.max_tokens.min(32_000))),
        signal: options.signal.clone(),
        api_key: Some(api_key),
        cache_retention: options.cache_retention,
        headers: options.headers.clone(),
        timeout_ms: options.timeout_ms,
        max_retries: options.max_retries.map(|n| n as u32),
        max_retry_delay_ms: options.max_retry_delay_ms,
        metadata: options.metadata.clone(),
        on_payload: options.on_payload.clone(),
        on_response: options.on_response.clone(),
        ..Default::default()
    };
    let Some(level) = options
        .reasoning
        .clone()
        .filter(|l| *l != ThinkingLevel::Off)
    else {
        return AnthropicOptions {
            thinking_enabled: Some(false),
            ..base
        };
    };

    if supports_adaptive_thinking(&model.id) {
        return AnthropicOptions {
            thinking_enabled: Some(true),
            effort: Some(map_thinking_level_to_effort(model, &level)),
            thinking_display: options.thinking_display.clone(),
            ..base
        };
    }

    let (max_tokens, thinking_budget) = adjust_max_tokens_for_thinking(
        base.max_tokens.unwrap_or(0),
        model.max_tokens,
        &level,
        options.thinking_budgets.as_ref(),
    );
    AnthropicOptions {
        max_tokens: Some(max_tokens),
        thinking_enabled: Some(true),
        thinking_budget_tokens: Some(thinking_budget),
        thinking_display: options.thinking_display.clone(),
        ..base
    }
}

/// `adjustMaxTokensForThinking` (simple-options.ts): `xhigh` uses the `high`
/// budget.
fn adjust_max_tokens_for_thinking(
    base_max_tokens: u64,
    model_max_tokens: u64,
    level: &ThinkingLevel,
    budgets: Option<&ThinkingBudgets>,
) -> (u64, u64) {
    const MIN_OUTPUT_TOKENS: u64 = 1024;
    let custom = |pick: fn(&ThinkingBudgets) -> Option<u64>| budgets.and_then(pick);
    let mut thinking_budget = match level {
        ThinkingLevel::Minimal => custom(|b| b.minimal).unwrap_or(1024),
        ThinkingLevel::Low => custom(|b| b.low).unwrap_or(2048),
        ThinkingLevel::Medium => custom(|b| b.medium).unwrap_or(8192),
        ThinkingLevel::High | ThinkingLevel::XHigh | ThinkingLevel::Off => {
            custom(|b| b.high).unwrap_or(16384)
        }
    };
    let max_tokens = (base_max_tokens + thinking_budget).min(model_max_tokens);
    if max_tokens <= thinking_budget {
        thinking_budget = max_tokens.saturating_sub(MIN_OUTPUT_TOKENS);
    }
    (max_tokens, thinking_budget)
}

/// `supportsAdaptiveThinking`: Opus 4.6+, Opus 5, Sonnet 4.6+, Fable 5.
pub fn supports_adaptive_thinking(model_id: &str) -> bool {
    [
        "fable-5",
        "opus-4-6",
        "opus-4.6",
        "opus-4-7",
        "opus-4.7",
        "opus-4-8",
        "opus-4.8",
        "opus-5",
        "sonnet-4-6",
        "sonnet-4.6",
        "sonnet-5",
    ]
    .iter()
    .any(|family| model_id.contains(family))
}

/// `isThinkingAlwaysOn`: models that reject `thinking: {type: "disabled"}`.
fn is_thinking_always_on(model_id: &str) -> bool {
    ["fable-5", "mythos-5", "opus-5-5", "opus-5.5"]
        .iter()
        .any(|family| model_id.contains(family))
}

/// `defaultThinkingDisplay`: Opus 4.8 and Opus 5 default to omitted.
fn default_thinking_display(model_id: &str) -> &'static str {
    if ["opus-4-8", "opus-4.8", "opus-5"]
        .iter()
        .any(|family| model_id.contains(family))
    {
        "omitted"
    } else {
        "summarized"
    }
}

fn level_key(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

/// `mapThinkingLevelToEffort`: the model's `thinkingLevelMap` entry when it
/// is a string, else minimal/low -> low, medium, and high for the rest.
fn map_thinking_level_to_effort(model: &Model, level: &ThinkingLevel) -> String {
    if let Some(Value::String(mapped)) = model
        .thinking_level_map
        .as_ref()
        .and_then(|m| m.get(level_key(level)))
    {
        return mapped.clone();
    }
    match level {
        ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        _ => "high",
    }
    .to_string()
}

fn compat(model: &Model) -> AnthropicMessagesCompat {
    model.compat_as()
}

// ---------------------------------------------------------------------------
// Claude Code tool names (OAuth)
// ---------------------------------------------------------------------------

/// `toClaudeCodeName`: CC's canonical casing for a case-insensitive match.
pub fn to_claude_code_name(name: &str) -> String {
    CLAUDE_CODE_TOOLS
        .iter()
        .find(|t| t.to_lowercase() == name.to_lowercase())
        .map_or_else(|| name.to_string(), |t| t.to_string())
}

/// `fromClaudeCodeName`: back to the caller's tool name (case-insensitive).
pub fn from_claude_code_name(name: &str, tools: &[Tool]) -> String {
    let lower = name.to_lowercase();
    tools
        .iter()
        .find(|t| t.name.to_lowercase() == lower)
        .map_or_else(|| name.to_string(), |t| t.name.clone())
}

// ---------------------------------------------------------------------------
// Client headers
// ---------------------------------------------------------------------------

/// `isOAuthToken`.
pub fn is_oauth_token(api_key: &str) -> bool {
    api_key.contains("sk-ant-oat")
}

/// Set `name`, replacing a case-insensitive match (`mergeHeaders` over records).
fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some(slot) = headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
    {
        *slot = (name.to_string(), value.to_string());
    } else {
        headers.push((name.to_string(), value.to_string()));
    }
}

/// `createClient`: the request headers (the SDK's auth and version headers
/// plus `defaultHeaders`) and whether the key is an OAuth token. Copilot uses
/// Bearer auth and is never treated as OAuth.
pub fn build_headers(
    model: &Model,
    context: &Context,
    api_key: &str,
    options: &AnthropicOptions,
) -> (Vec<(String, String)>, bool) {
    // Adaptive thinking models have interleaved thinking built in.
    let needs_interleaved_beta =
        options.interleaved_thinking.unwrap_or(true) && !supports_adaptive_thinking(&model.id);
    let mut betas: Vec<&str> = Vec::new();
    // `shouldUseFineGrainedToolStreamingBeta`.
    if !context.tools.is_empty()
        && !compat(model)
            .supports_eager_tool_input_streaming
            .unwrap_or(true)
    {
        betas.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if needs_interleaved_beta {
        betas.push(INTERLEAVED_THINKING_BETA);
    }

    let copilot = model.provider == "github-copilot";
    let oauth = !copilot && is_oauth_token(api_key);
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
    ];
    if copilot || oauth {
        headers.push(("authorization".into(), format!("Bearer {api_key}")));
    } else {
        headers.push(("x-api-key".into(), api_key.to_string()));
    }
    set_header(&mut headers, "accept", "application/json");
    set_header(
        &mut headers,
        "anthropic-dangerous-direct-browser-access",
        "true",
    );
    if oauth {
        let mut all = vec!["claude-code-20250219", "oauth-2025-04-20"];
        all.extend(&betas);
        set_header(&mut headers, "anthropic-beta", &all.join(","));
        set_header(
            &mut headers,
            "user-agent",
            &format!("claude-cli/{CLAUDE_CODE_VERSION}"),
        );
        set_header(&mut headers, "x-app", "cli");
    } else if !betas.is_empty() {
        set_header(&mut headers, "anthropic-beta", &betas.join(","));
    }
    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    if copilot {
        let has_images = has_copilot_vision_input(&context.messages);
        for (k, v) in build_copilot_dynamic_headers(&context.messages, has_images) {
            set_header(&mut headers, &k, &v);
        }
    }
    if let Some(extra) = &options.headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    (headers, oauth)
}

// ---------------------------------------------------------------------------
// buildParams
// ---------------------------------------------------------------------------

/// `getCacheControl`: the marker for the resolved retention; long retention
/// asks for a 1h TTL unless `compat.supportsLongCacheRetention` is false.
fn get_cache_control(model: &Model, cache_retention: Option<CacheRetention>) -> Option<Value> {
    match resolve_cache_retention(cache_retention) {
        CacheRetention::None => None,
        CacheRetention::Short => Some(json!({"type": "ephemeral"})),
        CacheRetention::Long => Some(
            if compat(model).supports_long_cache_retention.unwrap_or(true) {
                json!({"type": "ephemeral", "ttl": "1h"})
            } else {
                json!({"type": "ephemeral"})
            },
        ),
    }
}

fn text_block(text: &str, cache_control: Option<&Value>) -> Value {
    let mut block = json!({"type": "text", "text": text});
    if let Some(cc) = cache_control {
        block["cache_control"] = cc.clone();
    }
    block
}

/// `buildParams`.
pub fn build_params(
    model: &Model,
    context: &Context,
    is_oauth: bool,
    options: &AnthropicOptions,
) -> Value {
    let cache_control = get_cache_control(model, options.cache_retention);
    let cc = cache_control.as_ref();
    let max_tokens = options
        .max_tokens
        .filter(|n| *n > 0)
        .unwrap_or(model.max_tokens / 3);
    let mut params = json!({
        "model": model.id,
        "messages": convert_messages(&context.messages, model, is_oauth, cc),
        "max_tokens": max_tokens,
        "stream": true,
    });

    if is_oauth {
        let mut system = vec![text_block(
            "You are Claude Code, Anthropic's official CLI for Claude.",
            cc,
        )];
        if !context.system_prompt.is_empty() {
            system.push(text_block(&context.system_prompt, cc));
        }
        params["system"] = Value::Array(system);
    } else if !context.system_prompt.is_empty() {
        params["system"] = json!([text_block(&context.system_prompt, cc)]);
    }

    // Temperature is incompatible with extended thinking.
    if let Some(temperature) = options.temperature {
        if options.thinking_enabled != Some(true) && !is_thinking_always_on(&model.id) {
            params["temperature"] = json!(temperature);
        }
    }

    if !context.tools.is_empty() {
        let compat = compat(model);
        params["tools"] = Value::Array(convert_tools(
            &context.tools,
            is_oauth,
            compat.supports_eager_tool_input_streaming.unwrap_or(true),
            compat.supports_tool_search.unwrap_or(true),
            cc,
        ));
    }

    if model.reasoning {
        match options.thinking_enabled {
            Some(true) => {
                let display = match &options.thinking_display {
                    Some(ThinkingDisplay::Summarized) => "summarized",
                    Some(ThinkingDisplay::Omitted) => "omitted",
                    None => default_thinking_display(&model.id),
                };
                if supports_adaptive_thinking(&model.id) {
                    params["thinking"] = json!({"type": "adaptive", "display": display});
                    if let Some(effort) = &options.effort {
                        params["output_config"] = json!({"effort": effort});
                    }
                } else {
                    let budget = options
                        .thinking_budget_tokens
                        .filter(|n| *n > 0)
                        .unwrap_or(1024);
                    params["thinking"] =
                        json!({"type": "enabled", "budget_tokens": budget, "display": display});
                }
            }
            Some(false) => {
                if is_thinking_always_on(&model.id) {
                    // "disabled" is a 400 here: omit `thinking`, think as little as allowed.
                    params["output_config"] = json!({"effort": "low"});
                } else {
                    params["thinking"] = json!({"type": "disabled"});
                }
            }
            None => {}
        }
    }

    if let Some(Value::String(user_id)) = options.metadata.as_ref().map(|m| &m["user_id"]) {
        params["metadata"] = json!({"user_id": user_id});
    }

    match &options.tool_choice {
        Some(Value::String(choice)) => params["tool_choice"] = json!({"type": choice}),
        Some(choice @ Value::Object(_)) => params["tool_choice"] = choice.clone(),
        _ => {}
    }

    params
}

/// `normalizeToolCallId`: Anthropic's id pattern, at most 64 characters.
fn normalize_tool_call_id(id: &str) -> String {
    id.encode_utf16()
        .map(|unit| match char::from_u32(unit as u32) {
            Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
            _ => '_',
        })
        .take(64)
        .collect()
}

fn image_block(media_type: &str, data: &str) -> Value {
    json!({
        "type": "image",
        "source": {"type": "base64", "media_type": media_type, "data": data},
    })
}

/// `convertContentBlocks` (tool results): text only -> the texts joined by
/// newlines; with images -> blocks, led by a placeholder when there is no text.
fn convert_content_blocks(content: &[Content]) -> Value {
    let has_images = content.iter().any(|c| matches!(c, Content::Image(_)));
    if !has_images {
        return Value::String(
            content
                .iter()
                .map(|c| match c {
                    Content::Text(t) => t.text.as_str(),
                    _ => "",
                })
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let mut blocks: Vec<Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(json!({"type": "text", "text": t.text})),
            Content::Image(img) => Some(image_block(&img.media_type, &img.data)),
            _ => None,
        })
        .collect();
    if !blocks.iter().any(|b| b["type"] == "text") {
        blocks.insert(0, json!({"type": "text", "text": "(see attached image)"}));
    }
    Value::Array(blocks)
}

fn tool_result_block(tool: &ToolResultMessage) -> Value {
    json!({
        "type": "tool_result",
        "tool_use_id": tool.tool_call_id,
        "content": convert_content_blocks(&tool.content),
        "is_error": tool.is_error,
    })
}

fn assistant_blocks(message: &AssistantMessage, is_oauth: bool) -> Vec<Value> {
    let mut blocks = Vec::new();
    for block in &message.content {
        match block {
            Content::Text(t) => {
                if !t.text.trim().is_empty() {
                    blocks.push(json!({"type": "text", "text": t.text}));
                }
            }
            Content::Thinking(t) => {
                // Redacted thinking goes back as its opaque payload.
                if t.redacted {
                    blocks.push(json!({
                        "type": "redacted_thinking",
                        "data": t.signature.clone().unwrap_or_default(),
                    }));
                    continue;
                }
                if t.thinking.trim().is_empty() {
                    continue;
                }
                // No signature (e.g. an aborted stream): plain text, no tags.
                match t.signature.as_deref().filter(|s| !s.trim().is_empty()) {
                    None => blocks.push(json!({"type": "text", "text": t.thinking})),
                    Some(signature) => blocks.push(json!({
                        "type": "thinking",
                        "thinking": t.thinking,
                        "signature": signature,
                    })),
                }
            }
            Content::ToolCall(call) => {
                let name = if is_oauth {
                    to_claude_code_name(&call.name)
                } else {
                    call.name.clone()
                };
                let input = if call.arguments.is_null() {
                    json!({})
                } else {
                    call.arguments.clone()
                };
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": name,
                    "input": input,
                }));
            }
            Content::Image(_) => {}
        }
    }
    blocks
}

/// `convertMessages`.
pub fn convert_messages(
    messages: &[Message],
    model: &Model,
    is_oauth: bool,
    cache_control: Option<&Value>,
) -> Vec<Value> {
    let normalize = |id: &str, _: &Model, _: &AssistantMessage| normalize_tool_call_id(id);
    let transformed = transform_messages(messages, model, Some(&normalize));
    let mut params: Vec<Value> = Vec::new();

    let mut i = 0;
    while i < transformed.len() {
        match &transformed[i] {
            Message::User(m) => match &m.content {
                UserContent::Text(text) => {
                    if !text.trim().is_empty() {
                        params.push(json!({"role": "user", "content": text}));
                    }
                }
                UserContent::Blocks(content) => {
                    let blocks: Vec<Value> = content
                        .iter()
                        .filter_map(|item| match item {
                            Content::Text(t) if !t.text.trim().is_empty() => {
                                Some(json!({"type": "text", "text": t.text}))
                            }
                            Content::Image(img) => Some(image_block(&img.media_type, &img.data)),
                            _ => None,
                        })
                        .collect();
                    if !blocks.is_empty() {
                        params.push(json!({"role": "user", "content": blocks}));
                    }
                }
            },
            Message::Assistant(m) => {
                let blocks = assistant_blocks(m, is_oauth);
                if !blocks.is_empty() {
                    params.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Message::ToolResult(_) => {
                // Consecutive tool results become one user message (z.ai needs it).
                let mut results = Vec::new();
                while let Some(Message::ToolResult(tool)) = transformed.get(i) {
                    results.push(tool_result_block(tool));
                    i += 1;
                }
                params.push(json!({"role": "user", "content": results}));
                continue;
            }
        }
        i += 1;
    }

    // Cache the conversation history on the last user message.
    if let (Some(cc), Some(last)) = (cache_control, params.last_mut()) {
        if last["role"] == "user" {
            match &mut last["content"] {
                Value::Array(blocks) => {
                    if let Some(block) = blocks.last_mut() {
                        if matches!(
                            block["type"].as_str(),
                            Some("text" | "image" | "tool_result")
                        ) {
                            block["cache_control"] = cc.clone();
                        }
                    }
                }
                Value::String(text) => {
                    let text = std::mem::take(text);
                    last["content"] = json!([{"type": "text", "text": text, "cache_control": cc}]);
                }
                _ => {}
            }
        }
    }

    params
}

/// `convertTools`, honouring per-tool `deferLoading`.
pub fn convert_tools(
    tools: &[Tool],
    is_oauth: bool,
    supports_eager_tool_input_streaming: bool,
    supports_tool_search: bool,
    cache_control: Option<&Value>,
) -> Vec<Value> {
    // An endpoint that cannot defer gets eager schemas rather than a 400.
    let deferring = supports_tool_search && tools.iter().any(|t| t.defer_loading == Some(true));
    let mut converted: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let name = if is_oauth {
                to_claude_code_name(&tool.name)
            } else {
                tool.name.clone()
            };
            let mut value = json!({"name": name, "description": tool.description});
            if supports_eager_tool_input_streaming {
                value["eager_input_streaming"] = json!(true);
            }
            if deferring && tool.defer_loading == Some(true) {
                value["defer_loading"] = json!(true);
            }
            let or = |key: &str, default: Value| match tool.parameters.get(key) {
                Some(v) if !v.is_null() => v.clone(),
                _ => default,
            };
            value["input_schema"] = json!({
                "type": "object",
                "properties": or("properties", json!({})),
                "required": or("required", json!([])),
            });
            value
        })
        .collect();

    if deferring {
        converted.push(json!({"type": TOOL_SEARCH_TOOL_TYPE, "name": TOOL_SEARCH_TOOL_NAME}));
    }
    // The breakpoint goes on the last entry of the final array: the search
    // tool once one is appended.
    if let (Some(cc), Some(last)) = (cache_control, converted.last_mut()) {
        last["cache_control"] = cc.clone();
    }
    converted
}

#[cfg(test)]
#[path = "request_tests.rs"]
pub(crate) mod tests;

//! Request construction for the OpenAI Chat Completions API.
//!
//! Port of the request-building half of hoocode
//! `providers/openai-completions.ts` (v0.5.89): `getCompat`/`detectCompat`,
//! `buildParams`, `convertMessages`, `convertTools`, the Anthropic-style cache
//! markers, `promptSuffix`, and the client headers of `createClient`.

use std::collections::HashMap;

use cortexcode_ai_types::{
    AbortSignal, CacheRetention, Content, Context, Message, Model, OnPayload, OnResponse,
    OpenAICompletionsCompat, SimpleStreamOptions, ThinkingLevel, Tool,
};
use cortexcode_ai_util::{
    build_copilot_dynamic_headers, has_copilot_vision_input, rejected_params_for,
    to_strict_json_schema, transform_messages,
};
use serde_json::{json, Map, Value};

/// `ResolvedOpenAICompletionsCompat`: every compat field resolved, from the
/// model's explicit `compat` over what `detectCompat` infers from the
/// provider and base URL.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    /// `max_completion_tokens` or `max_tokens`.
    pub max_tokens_field: String,
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub requires_thinking_as_text: bool,
    pub requires_reasoning_content_on_assistant_messages: bool,
    /// `openai`, `openrouter`, `deepseek`, `together`, `zai`, `qwen`, `qwen-chat-template`.
    pub thinking_format: String,
    pub open_router_routing: Value,
    pub vercel_gateway_routing: Value,
    pub zai_tool_stream: bool,
    pub supports_strict_mode: bool,
    /// `strict` or `none`.
    pub tool_call_constraint: String,
    pub cache_control_format: Option<String>,
    pub send_session_affinity_headers: bool,
    pub supports_long_cache_retention: bool,
    pub prompt_suffix: Option<String>,
}

/// `detectCompat`: compat settings inferred for known providers. The
/// provider id wins over URL-based detection.
pub fn detect_compat(model: &Model) -> ResolvedCompat {
    let provider = model.provider.as_str();
    let base_url = model.base_url.as_str();

    let is_zai = provider == "zai" || base_url.contains("api.z.ai");
    let is_together = provider == "together"
        || base_url.contains("api.together.ai")
        || base_url.contains("api.together.xyz");
    let is_moonshot = provider == "moonshotai"
        || provider == "moonshotai-cn"
        || base_url.contains("api.moonshot.");
    let is_opencode_go = provider == "opencode-go" || base_url.contains("opencode.ai/zen/go");

    let is_non_standard = provider == "cerebras"
        || base_url.contains("cerebras.ai")
        || provider == "xai"
        || base_url.contains("api.x.ai")
        || is_together
        || base_url.contains("chutes.ai")
        || base_url.contains("deepseek.com")
        || is_zai
        || is_moonshot
        || provider == "opencode"
        || base_url.contains("opencode.ai");

    let use_max_tokens = base_url.contains("chutes.ai") || is_moonshot || is_together;
    let is_grok = provider == "xai" || base_url.contains("api.x.ai");
    let is_deepseek = provider == "deepseek" || base_url.contains("deepseek.com");
    let cache_control_format = (provider == "openrouter" && model.id.starts_with("anthropic/"))
        .then(|| "anthropic".to_string());

    let thinking_format = if is_deepseek {
        "deepseek"
    } else if is_zai {
        "zai"
    } else if is_together {
        "together"
    } else if provider == "openrouter" || base_url.contains("openrouter.ai") {
        "openrouter"
    } else {
        "openai"
    };

    ResolvedCompat {
        supports_store: !is_non_standard,
        supports_developer_role: !is_non_standard,
        supports_reasoning_effort: !is_grok && !is_zai && !is_moonshot && !is_together,
        supports_usage_in_streaming: true,
        max_tokens_field: if use_max_tokens {
            "max_tokens"
        } else {
            "max_completion_tokens"
        }
        .to_string(),
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: false,
        requires_reasoning_content_on_assistant_messages: is_deepseek,
        thinking_format: thinking_format.to_string(),
        open_router_routing: json!({}),
        vercel_gateway_routing: json!({}),
        zai_tool_stream: false,
        supports_strict_mode: !is_moonshot && !is_together,
        tool_call_constraint: if provider == "openai" || base_url.contains("api.openai.com") {
            "strict"
        } else {
            "none"
        }
        .to_string(),
        cache_control_format,
        send_session_affinity_headers: false,
        supports_long_cache_retention: !(is_together || is_opencode_go),
        prompt_suffix: None,
    }
}

/// `getCompat`: explicit `model.compat` fields over the detected ones.
pub fn get_compat(model: &Model) -> ResolvedCompat {
    let detected = detect_compat(model);
    if model.compat.is_none() {
        return detected;
    }
    let c: OpenAICompletionsCompat = model.compat_as();
    ResolvedCompat {
        supports_store: c.supports_store.unwrap_or(detected.supports_store),
        supports_developer_role: c
            .supports_developer_role
            .unwrap_or(detected.supports_developer_role),
        supports_reasoning_effort: c
            .supports_reasoning_effort
            .unwrap_or(detected.supports_reasoning_effort),
        supports_usage_in_streaming: c
            .supports_usage_in_streaming
            .unwrap_or(detected.supports_usage_in_streaming),
        max_tokens_field: c.max_tokens_field.unwrap_or(detected.max_tokens_field),
        requires_tool_result_name: c
            .requires_tool_result_name
            .unwrap_or(detected.requires_tool_result_name),
        requires_assistant_after_tool_result: c
            .requires_assistant_after_tool_result
            .unwrap_or(detected.requires_assistant_after_tool_result),
        requires_thinking_as_text: c
            .requires_thinking_as_text
            .unwrap_or(detected.requires_thinking_as_text),
        requires_reasoning_content_on_assistant_messages: c
            .requires_reasoning_content_on_assistant_messages
            .unwrap_or(detected.requires_reasoning_content_on_assistant_messages),
        thinking_format: c.thinking_format.unwrap_or(detected.thinking_format),
        open_router_routing: c.open_router_routing.unwrap_or_else(|| json!({})),
        vercel_gateway_routing: c
            .vercel_gateway_routing
            .unwrap_or(detected.vercel_gateway_routing),
        zai_tool_stream: c.zai_tool_stream.unwrap_or(detected.zai_tool_stream),
        supports_strict_mode: c
            .supports_strict_mode
            .unwrap_or(detected.supports_strict_mode),
        tool_call_constraint: c
            .tool_call_constraint
            .unwrap_or(detected.tool_call_constraint),
        cache_control_format: c.cache_control_format.or(detected.cache_control_format),
        send_session_affinity_headers: c
            .send_session_affinity_headers
            .unwrap_or(detected.send_session_affinity_headers),
        supports_long_cache_retention: c
            .supports_long_cache_retention
            .unwrap_or(detected.supports_long_cache_retention),
        prompt_suffix: c.prompt_suffix.or(detected.prompt_suffix),
    }
}

/// `OpenAICompletionsOptions`: `StreamOptions` plus `toolChoice` and
/// `reasoningEffort`, as `streamSimpleOpenAICompletions` builds them.
#[derive(Debug, Clone, Default)]
pub struct CompletionsOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub timeout_ms: Option<u64>,
    /// SDK client retries (default 2).
    pub max_retries: Option<u32>,
    pub max_retry_delay_ms: Option<u64>,
    pub constrain_tool_calls: bool,
    pub tool_choice: Option<Value>,
    /// `minimal` .. `xhigh`, already clamped to what the model supports.
    pub reasoning_effort: Option<String>,
    pub on_payload: Option<OnPayload>,
    pub on_response: Option<OnResponse>,
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

/// `streamSimpleOpenAICompletions` option mapping: `buildBaseOptions`
/// (`maxTokens` defaults to `min(model.maxTokens, 32000)`) plus the clamped
/// reasoning level and `toolChoice`.
pub fn simple_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: String,
) -> CompletionsOptions {
    let reasoning_effort = options
        .reasoning
        .as_ref()
        .map(|level| cortexcode_ai_models::clamp_thinking_level(model, level))
        .filter(|level| *level != ThinkingLevel::Off)
        .map(|level| level_key(&level).to_string());
    CompletionsOptions {
        temperature: options.temperature,
        max_tokens: options
            .max_tokens
            .or((model.max_tokens > 0).then(|| model.max_tokens.min(32_000))),
        signal: options.signal.clone(),
        api_key: Some(api_key),
        cache_retention: options.cache_retention,
        session_id: options.session_id.clone(),
        headers: options.headers.clone(),
        timeout_ms: options.timeout_ms,
        max_retries: options.max_retries.map(|n| n as u32),
        max_retry_delay_ms: options.max_retry_delay_ms,
        constrain_tool_calls: options.constrain_tool_calls == Some(true),
        tool_choice: options.tool_choice.clone(),
        reasoning_effort,
        on_payload: options.on_payload.clone(),
        on_response: options.on_response.clone(),
    }
}

/// Set `name` in an ordered header list, replacing any case-insensitive match
/// (the later source wins, as with `Object.assign` over a record).
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

/// The client headers of `createClient`: auth, the model's headers, the
/// Copilot dynamic headers, session-affinity headers when compat asks for
/// them (and a cache session id is in use), then the caller's headers.
pub fn build_headers(
    model: &Model,
    context: &Context,
    api_key: &str,
    options_headers: Option<&HashMap<String, String>>,
    cache_session_id: Option<&str>,
    compat: &ResolvedCompat,
) -> Vec<(String, String)> {
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {api_key}")),
    ];
    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    if model.provider == "github-copilot" {
        let has_images = has_copilot_vision_input(&context.messages);
        for (k, v) in build_copilot_dynamic_headers(&context.messages, has_images) {
            set_header(&mut headers, &k, &v);
        }
    }
    if let Some(session_id) = cache_session_id {
        if compat.send_session_affinity_headers {
            set_header(&mut headers, "session_id", session_id);
            set_header(&mut headers, "x-client-request-id", session_id);
            set_header(&mut headers, "x-session-affinity", session_id);
        }
    }
    if let Some(extra) = options_headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    headers
}

/// `hasToolHistory`: the conversation already carries tool calls or results.
fn has_tool_history(messages: &[Message]) -> bool {
    messages.iter().any(|msg| match msg {
        Message::ToolResult(_) => true,
        Message::Assistant(a) => a.content.iter().any(|b| matches!(b, Content::ToolCall(_))),
        Message::User(_) => false,
    })
}

fn mapped_effort(model: &Model, effort: &str) -> Value {
    match model
        .thinking_level_map
        .as_ref()
        .and_then(|m| m.get(effort))
    {
        Some(Value::Null) | None => json!(effort),
        Some(v) => v.clone(),
    }
}

/// `buildParams`: the streaming Chat Completions request body.
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &CompletionsOptions,
    compat: &ResolvedCompat,
    cache_retention: &CacheRetention,
) -> Value {
    let mut messages = convert_messages(model, context, compat);
    if let Some(suffix) = &compat.prompt_suffix {
        append_prompt_suffix(&mut messages, suffix);
    }
    let cache_control = get_compat_cache_control(compat, cache_retention);
    // An endpoint that already refused one of these keeps refusing it.
    let rejected = rejected_params_for(&model.base_url);
    let long = *cache_retention == CacheRetention::Long;

    let mut params = Map::new();
    params.insert("model".into(), json!(model.id));
    params.insert("messages".into(), Value::Null);
    params.insert("stream".into(), json!(true));
    let send_cache_key = !rejected.contains("prompt_cache_key")
        && ((model.base_url.contains("api.openai.com")
            && *cache_retention != CacheRetention::None)
            || (long && compat.supports_long_cache_retention));
    if send_cache_key {
        if let Some(session_id) = &options.session_id {
            params.insert("prompt_cache_key".into(), json!(session_id));
        }
    }
    if long && compat.supports_long_cache_retention && !rejected.contains("prompt_cache_retention")
    {
        params.insert("prompt_cache_retention".into(), json!("24h"));
    }
    if compat.supports_usage_in_streaming && !rejected.contains("stream_options") {
        params.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if compat.supports_store && !rejected.contains("store") {
        params.insert("store".into(), json!(false));
    }
    if let Some(max_tokens) = options.max_tokens.filter(|&n| n > 0) {
        if compat.max_tokens_field == "max_tokens" {
            params.insert("max_tokens".into(), json!(max_tokens));
        } else {
            params.insert("max_completion_tokens".into(), json!(max_tokens));
        }
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".into(), json!(temperature));
    }

    let mut tools: Option<Vec<Value>> = None;
    if !context.tools.is_empty() {
        tools = Some(convert_tools(
            &context.tools,
            compat,
            options.constrain_tool_calls,
        ));
    } else if has_tool_history(&context.messages) {
        // Anthropic behind LiteLLM-style proxies requires `tools` when the
        // conversation has tool calls or results.
        tools = Some(Vec::new());
    }

    if let Some(cache_control) = &cache_control {
        apply_anthropic_cache_control(&mut messages, tools.as_mut(), cache_control);
    }
    params.insert("messages".into(), Value::Array(messages));
    if let Some(tools) = tools {
        let has_tools = !tools.is_empty();
        params.insert("tools".into(), Value::Array(tools));
        if has_tools && compat.zai_tool_stream {
            params.insert("tool_stream".into(), json!(true));
        }
    }

    if let Some(tool_choice) = &options.tool_choice {
        params.insert("tool_choice".into(), tool_choice.clone());
    }

    let effort = options.reasoning_effort.as_deref();
    let format = compat.thinking_format.as_str();
    if model.reasoning && (format == "zai" || format == "qwen") {
        params.insert("enable_thinking".into(), json!(effort.is_some()));
    } else if model.reasoning && format == "qwen-chat-template" {
        params.insert(
            "chat_template_kwargs".into(),
            json!({"enable_thinking": effort.is_some(), "preserve_thinking": true}),
        );
    } else if model.reasoning && format == "deepseek" {
        params.insert(
            "thinking".into(),
            json!({"type": if effort.is_some() { "enabled" } else { "disabled" }}),
        );
        if let Some(effort) = effort {
            params.insert("reasoning_effort".into(), mapped_effort(model, effort));
        }
    } else if model.reasoning && format == "openrouter" {
        // OpenRouter normalizes reasoning across providers via a nested object.
        if let Some(effort) = effort {
            params.insert(
                "reasoning".into(),
                json!({"effort": mapped_effort(model, effort)}),
            );
        } else {
            let off = model.thinking_level_map.as_ref().and_then(|m| m.get("off"));
            match off {
                Some(Value::Null) => {}
                Some(v) => {
                    params.insert("reasoning".into(), json!({"effort": v}));
                }
                None => {
                    params.insert("reasoning".into(), json!({"effort": "none"}));
                }
            }
        }
    } else if model.reasoning && format == "together" {
        params.insert("reasoning".into(), json!({"enabled": effort.is_some()}));
        if let Some(effort) = effort.filter(|_| compat.supports_reasoning_effort) {
            params.insert("reasoning_effort".into(), mapped_effort(model, effort));
        }
    } else if model.reasoning && compat.supports_reasoning_effort {
        match effort {
            Some(effort) => {
                params.insert("reasoning_effort".into(), mapped_effort(model, effort));
            }
            None => {
                if let Some(Value::String(off)) =
                    model.thinking_level_map.as_ref().and_then(|m| m.get("off"))
                {
                    params.insert("reasoning_effort".into(), json!(off));
                }
            }
        }
    }

    // Provider routing preferences (read from the raw compat, as in TS).
    let raw: OpenAICompletionsCompat = model.compat_as();
    if model.base_url.contains("openrouter.ai") {
        if let Some(routing) = raw.open_router_routing.filter(|v| !v.is_null()) {
            params.insert("provider".into(), routing);
        }
    }
    if model.base_url.contains("ai-gateway.vercel.sh") {
        if let Some(routing) = raw.vercel_gateway_routing.filter(|v| !v.is_null()) {
            let only = routing.get("only").filter(|v| !v.is_null());
            let order = routing.get("order").filter(|v| !v.is_null());
            if only.is_some() || order.is_some() {
                let mut gateway = Map::new();
                if let Some(only) = only {
                    gateway.insert("only".into(), only.clone());
                }
                if let Some(order) = order {
                    gateway.insert("order".into(), order.clone());
                }
                params.insert("providerOptions".into(), json!({"gateway": gateway}));
            }
        }
    }

    Value::Object(params)
}

/// `getCompatCacheControl`.
fn get_compat_cache_control(
    compat: &ResolvedCompat,
    cache_retention: &CacheRetention,
) -> Option<Value> {
    if compat.cache_control_format.as_deref() != Some("anthropic")
        || *cache_retention == CacheRetention::None
    {
        return None;
    }
    if *cache_retention == CacheRetention::Long && compat.supports_long_cache_retention {
        Some(json!({"type": "ephemeral", "ttl": "1h"}))
    } else {
        Some(json!({"type": "ephemeral"}))
    }
}

/// `applyAnthropicCacheControl`: mark the system prompt, the last tool and
/// the last user/assistant message with text.
fn apply_anthropic_cache_control(
    messages: &mut [Value],
    tools: Option<&mut Vec<Value>>,
    cache_control: &Value,
) {
    if let Some(msg) = messages
        .iter_mut()
        .find(|m| matches!(m["role"].as_str(), Some("system" | "developer")))
    {
        add_cache_control_to_text_content(msg, cache_control);
    }
    if let Some(last) = tools.and_then(|t| t.last_mut()) {
        last["cache_control"] = cache_control.clone();
    }
    for msg in messages.iter_mut().rev() {
        if matches!(msg["role"].as_str(), Some("user" | "assistant"))
            && add_cache_control_to_text_content(msg, cache_control)
        {
            return;
        }
    }
}

fn add_cache_control_to_text_content(message: &mut Value, cache_control: &Value) -> bool {
    match &mut message["content"] {
        Value::String(text) => {
            if text.is_empty() {
                return false;
            }
            let text = std::mem::take(text);
            message["content"] =
                json!([{"type": "text", "text": text, "cache_control": cache_control}]);
            true
        }
        Value::Array(parts) => {
            for part in parts.iter_mut().rev() {
                if part["type"] == "text" {
                    part["cache_control"] = cache_control.clone();
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// `appendPromptSuffix`: append to the last user message's (last) text.
fn append_prompt_suffix(messages: &mut [Value], suffix: &str) {
    let Some(msg) = messages.iter_mut().rev().find(|m| m["role"] == "user") else {
        return;
    };
    match &mut msg["content"] {
        Value::String(text) => *text = format!("{text} {suffix}"),
        Value::Array(parts) => {
            if let Some(part) = parts.iter_mut().rev().find(|p| p["type"] == "text") {
                let text = part["text"].as_str().unwrap_or_default();
                part["text"] = json!(format!("{text} {suffix}"));
            } else {
                parts.push(json!({"type": "text", "text": suffix}));
            }
        }
        _ => {}
    }
}

fn image_url_block(media_type: &str, data: &str) -> Value {
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{media_type};base64,{data}")},
    })
}

/// `normalizeToolCallId` of `convertMessages`.
fn normalize_tool_call_id(model: &Model, id: &str) -> String {
    // Pipe-separated ids come from the OpenAI Responses API
    // (`{call_id}|{id}`); keep the sanitized call id, max 40 chars.
    if id.contains('|') {
        let call_id = id.split('|').next().unwrap_or_default();
        return call_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
    }
    if model.provider == "openai" && id.chars().count() > 40 {
        return id.chars().take(40).collect();
    }
    id.to_string()
}

/// `convertMessages`: context messages to Chat Completions messages.
pub fn convert_messages(model: &Model, context: &Context, compat: &ResolvedCompat) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    let normalize = |id: &str, model: &Model, _: &cortexcode_ai_types::AssistantMessage| {
        normalize_tool_call_id(model, id)
    };
    let transformed = transform_messages(&context.messages, model, Some(&normalize));

    if !context.system_prompt.is_empty() {
        let role = if model.reasoning && compat.supports_developer_role {
            "developer"
        } else {
            "system"
        };
        params.push(json!({"role": role, "content": context.system_prompt}));
    }

    let mut last_role: Option<&str> = None;
    let mut i = 0;
    while i < transformed.len() {
        let msg = &transformed[i];
        if compat.requires_assistant_after_tool_result
            && last_role == Some("toolResult")
            && matches!(msg, Message::User(_))
        {
            params.push(
                json!({"role": "assistant", "content": "I have processed the tool results."}),
            );
        }

        match msg {
            Message::User(m) => {
                if let Some(text) = m.content.as_str() {
                    i += 1;
                    params.push(json!({"role": "user", "content": text}));
                    last_role = Some("user");
                    continue;
                }
                let content: Vec<Value> = m
                    .content
                    .blocks()
                    .iter()
                    .filter_map(|item| match item {
                        Content::Text(t) => Some(json!({"type": "text", "text": t.text})),
                        Content::Image(img) => Some(image_url_block(&img.media_type, &img.data)),
                        Content::Thinking(_) | Content::ToolCall(_) => None,
                    })
                    .collect();
                i += 1;
                if content.is_empty() {
                    continue;
                }
                params.push(json!({"role": "user", "content": content}));
                last_role = Some("user");
            }
            Message::Assistant(m) => {
                i += 1;
                // A skipped (empty) assistant message leaves `lastRole` as is.
                if let Some(v) = assistant_message(model, compat, &m.content) {
                    params.push(v);
                    last_role = Some("assistant");
                }
            }
            Message::ToolResult(_) => {
                let mut image_blocks = Vec::new();
                while let Some(Message::ToolResult(tool)) = transformed.get(i) {
                    let text = tool
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = tool.content.iter().any(|c| matches!(c, Content::Image(_)));
                    let mut tool_msg = json!({
                        "role": "tool",
                        "content": if text.is_empty() { "(see attached image)".to_string() } else { text },
                        "tool_call_id": tool.tool_call_id,
                    });
                    if compat.requires_tool_result_name && !tool.tool_name.is_empty() {
                        tool_msg["name"] = json!(tool.tool_name);
                    }
                    params.push(tool_msg);
                    if has_images && model.input.iter().any(|i| i == "image") {
                        for c in &tool.content {
                            if let Content::Image(img) = c {
                                image_blocks.push(image_url_block(&img.media_type, &img.data));
                            }
                        }
                    }
                    i += 1;
                }
                if image_blocks.is_empty() {
                    last_role = Some("toolResult");
                } else {
                    if compat.requires_assistant_after_tool_result {
                        params.push(json!({"role": "assistant", "content": "I have processed the tool results."}));
                    }
                    let mut parts = vec![
                        json!({"type": "text", "text": "Attached image(s) from tool result:"}),
                    ];
                    parts.extend(image_blocks);
                    params.push(json!({"role": "user", "content": parts}));
                    last_role = Some("user");
                }
            }
        }
    }
    params
}

/// The assistant half of `convertMessages`; `None` when the message has
/// neither content nor tool calls (e.g. an aborted empty turn).
fn assistant_message(model: &Model, compat: &ResolvedCompat, content: &[Content]) -> Option<Value> {
    let mut msg = Map::new();
    msg.insert("role".into(), json!("assistant"));
    msg.insert(
        "content".into(),
        if compat.requires_assistant_after_tool_result {
            json!("")
        } else {
            Value::Null
        },
    );

    let text_parts: Vec<&str> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) if !t.text.trim().is_empty() => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    let assistant_text: String = text_parts.concat();

    let thinking: Vec<&cortexcode_ai_types::ThinkingContent> = content
        .iter()
        .filter_map(|c| match c {
            Content::Thinking(t) if !t.thinking.trim().is_empty() => Some(t),
            _ => None,
        })
        .collect();
    if !thinking.is_empty() {
        if compat.requires_thinking_as_text {
            // Plain text, no tags, so the model does not mimic them.
            let thinking_text = thinking
                .iter()
                .map(|t| t.thinking.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let mut parts = vec![json!({"type": "text", "text": thinking_text})];
            parts.extend(
                text_parts
                    .iter()
                    .map(|t| json!({"type": "text", "text": t})),
            );
            msg.insert("content".into(), Value::Array(parts));
        } else {
            if !assistant_text.is_empty() {
                msg.insert("content".into(), json!(assistant_text));
            }
            // The first block's signature names the field to replay it in
            // (llama.cpp server + gpt-oss).
            if let Some(signature) = thinking[0].signature.as_deref().filter(|s| !s.is_empty()) {
                let joined = thinking
                    .iter()
                    .map(|t| t.thinking.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                msg.insert(signature.to_string(), json!(joined));
            }
        }
    } else if !assistant_text.is_empty() {
        msg.insert("content".into(), json!(assistant_text));
    }

    let tool_calls: Vec<&cortexcode_ai_types::ToolCallContent> = content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(tc),
            _ => None,
        })
        .collect();
    if !tool_calls.is_empty() {
        msg.insert(
            "tool_calls".into(),
            Value::Array(
                tool_calls
                    .iter()
                    .map(|tc| {
                        json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {"name": tc.name, "arguments": tc.arguments.to_string()},
                        })
                    })
                    .collect(),
            ),
        );
        let reasoning_details: Vec<Value> = tool_calls
            .iter()
            .filter_map(|tc| tc.thought_signature.as_deref())
            .filter(|s| !s.is_empty())
            .filter_map(|s| serde_json::from_str::<Value>(s).ok())
            .filter(truthy)
            .collect();
        if !reasoning_details.is_empty() {
            msg.insert("reasoning_details".into(), Value::Array(reasoning_details));
        }
    }
    if compat.requires_reasoning_content_on_assistant_messages
        && model.reasoning
        && !msg.contains_key("reasoning_content")
    {
        msg.insert("reasoning_content".into(), json!(""));
    }

    let has_content = match &msg["content"] {
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    };
    if !has_content && !msg.contains_key("tool_calls") {
        return None;
    }
    Some(Value::Object(msg))
}

/// JavaScript truthiness of a parsed JSON value (`.filter(Boolean)`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `convertTools`. Strict function calling needs `constrainToolCalls`, a
/// `strict` tool-call constraint and strict-mode support; `strict` is only
/// sent where the provider accepts the field.
fn convert_tools(
    tools: &[Tool],
    compat: &ResolvedCompat,
    constrain_tool_calls: bool,
) -> Vec<Value> {
    let strict = constrain_tool_calls
        && compat.tool_call_constraint == "strict"
        && compat.supports_strict_mode;
    tools
        .iter()
        .map(|t| {
            let mut function = json!({
                "name": t.name,
                "description": t.description,
                "parameters": if strict { to_strict_json_schema(&t.parameters) } else { t.parameters.clone() },
            });
            if compat.supports_strict_mode {
                function["strict"] = json!(strict);
            }
            json!({"type": "function", "function": function})
        })
        .collect()
}

#[cfg(test)]
#[path = "request_tests.rs"]
pub(crate) mod tests;

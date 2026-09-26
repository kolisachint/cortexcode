//! OpenAI Responses API provider (`openai-responses`) for cortex AI, plus the
//! Responses plumbing shared with `azure-openai-responses` ([`shared`]).
//!
//! Port of hoocode `providers/openai-responses.ts` and
//! `providers/openai-responses-shared.ts` (v0.5.89).
//!
//! Client retries follow the `openai` SDK (see
//! `cortexcode_ai_util::send_with_sdk_retries`). Known deviation: the
//! `onPayload` / `onResponse` hooks are not supported.

pub mod shared;

use std::collections::{HashMap, HashSet};

use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{
    AbortSignal, CacheRetention, Context, Model, OpenAIResponsesCompat, SimpleStreamOptions,
    ThinkingLevel, Usage,
};
use cortexcode_ai_util::{
    build_copilot_dynamic_headers, has_copilot_vision_input, resolve_cache_retention,
};
use serde_json::{json, Map, Value};
use shared::{
    convert_responses_messages, convert_responses_tools, run_responses_stream, set_header,
    ResponsesRequest, ResponsesStreamOptions,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Providers whose `call_id|item_id` tool call ids stay paired on replay.
const OPENAI_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];

/// `OpenAIResponsesOptions`: `StreamOptions` plus reasoning and service tier.
#[derive(Debug, Clone, Default)]
pub struct ResponsesOptions {
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
    /// `minimal` .. `xhigh`, already clamped to what the model supports.
    pub reasoning_effort: Option<String>,
    /// `auto`, `detailed` or `concise`.
    pub reasoning_summary: Option<String>,
    /// `service_tier` (`auto`, `default`, `flex`, `scale`, `priority`).
    pub service_tier: Option<String>,
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

/// The `streamSimple*` option mapping shared by the Responses providers:
/// `buildBaseOptions` plus the clamped reasoning level.
pub fn simple_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: String,
) -> ResponsesOptions {
    let reasoning_effort = options
        .reasoning
        .as_ref()
        .map(|level| cortexcode_ai_models::clamp_thinking_level(model, level))
        .filter(|level| *level != ThinkingLevel::Off)
        .map(|level| level_key(&level).to_string());
    ResponsesOptions {
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
        reasoning_effort,
        reasoning_summary: None,
        service_tier: None,
    }
}

/// Resolve the API key as `streamSimple*` does: explicit, else the provider's
/// environment variable; missing is an immediate error.
pub fn require_api_key(model: &Model, options: &SimpleStreamOptions) -> Result<String, BoxError> {
    options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
        .ok_or_else(|| format!("No API key for provider: {}", model.provider).into())
}

/// `streamSimpleOpenAIResponses`.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = require_api_key(&model, &options)?;
    let options = simple_options(&model, &options, api_key);
    Ok(stream_responses(model, context, options))
}

/// `streamOpenAIResponses`.
pub fn stream_responses(
    model: Model,
    context: Context,
    options: ResponsesOptions,
) -> AssistantMessageEventStream {
    let cache_retention = resolve_cache_retention(options.cache_retention);
    let cache_session_id = (cache_retention != CacheRetention::None)
        .then_some(options.session_id.as_deref())
        .flatten();
    let api_key = options.api_key.clone().unwrap_or_default();
    let headers = build_headers(
        &model,
        &context,
        &api_key,
        options.headers.as_ref(),
        cache_session_id,
    );
    let body = build_params(&model, &context, &options);
    let request = ResponsesRequest {
        url: format!("{}/responses", model.base_url.trim_end_matches('/')),
        headers,
        body,
        timeout_ms: options.timeout_ms,
        max_retries: options.max_retries,
        max_retry_delay_ms: options.max_retry_delay_ms,
    };
    let stream_options = ResponsesStreamOptions {
        service_tier: options.service_tier.clone(),
        resolve_service_tier: None,
        apply_service_tier_pricing: Some(apply_service_tier_pricing),
    };
    run_responses_stream(model, Ok(request), options.signal, stream_options)
}

/// `getCompat` for openai-responses.
fn get_compat(model: &Model) -> (bool, bool) {
    let c: OpenAIResponsesCompat = model.compat_as();
    (
        c.send_session_id_header.unwrap_or(true),
        c.supports_long_cache_retention.unwrap_or(true),
    )
}

/// The client headers of `createClient`: auth, the model's headers, the
/// Copilot dynamic headers, cache-affinity headers, then the caller's.
pub fn build_headers(
    model: &Model,
    context: &Context,
    api_key: &str,
    options_headers: Option<&HashMap<String, String>>,
    cache_session_id: Option<&str>,
) -> Vec<(String, String)> {
    let (send_session_id_header, _) = get_compat(model);
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
        if send_session_id_header {
            set_header(&mut headers, "session_id", session_id);
        }
        set_header(&mut headers, "x-client-request-id", session_id);
    }
    if let Some(extra) = options_headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    headers
}

/// `model.thinkingLevelMap?.[effort] ?? effort`.
pub fn mapped_effort(model: &Model, effort: &str) -> Value {
    match model
        .thinking_level_map
        .as_ref()
        .and_then(|m| m.get(effort))
    {
        Some(Value::Null) | None => json!(effort),
        Some(v) => v.clone(),
    }
}

/// The `reasoning` / `include` fields shared by the Responses providers.
/// `skip_off_default` leaves out the "off" effort (GitHub Copilot).
pub fn apply_reasoning(
    params: &mut Map<String, Value>,
    model: &Model,
    reasoning_effort: Option<&str>,
    reasoning_summary: Option<&str>,
    skip_off_default: bool,
) {
    if !model.reasoning {
        return;
    }
    if reasoning_effort.is_some() || reasoning_summary.is_some() {
        let effort = reasoning_effort
            .map(|e| mapped_effort(model, e))
            .unwrap_or_else(|| json!("medium"));
        params.insert(
            "reasoning".into(),
            json!({"effort": effort, "summary": reasoning_summary.unwrap_or("auto")}),
        );
        params.insert("include".into(), json!(["reasoning.encrypted_content"]));
    } else if !skip_off_default {
        match model.thinking_level_map.as_ref().and_then(|m| m.get("off")) {
            Some(Value::Null) => {}
            Some(off) => {
                params.insert("reasoning".into(), json!({"effort": off}));
            }
            None => {
                params.insert("reasoning".into(), json!({"effort": "none"}));
            }
        }
    }
}

/// `buildParams` for openai-responses.
pub fn build_params(model: &Model, context: &Context, options: &ResponsesOptions) -> Value {
    let allowed: HashSet<&str> = OPENAI_TOOL_CALL_PROVIDERS.into_iter().collect();
    let input = convert_responses_messages(model, context, &allowed, true);
    let cache_retention = resolve_cache_retention(options.cache_retention);
    let (_, supports_long_cache_retention) = get_compat(model);

    let mut params = Map::new();
    params.insert("model".into(), json!(model.id));
    params.insert("input".into(), Value::Array(input));
    params.insert("stream".into(), json!(true));
    if cache_retention != CacheRetention::None {
        if let Some(session_id) = &options.session_id {
            params.insert("prompt_cache_key".into(), json!(session_id));
        }
    }
    if cache_retention == CacheRetention::Long && supports_long_cache_retention {
        params.insert("prompt_cache_retention".into(), json!("24h"));
    }
    params.insert("store".into(), json!(false));
    if let Some(max_tokens) = options.max_tokens.filter(|&n| n > 0) {
        params.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".into(), json!(temperature));
    }
    if let Some(tier) = &options.service_tier {
        params.insert("service_tier".into(), json!(tier));
    }
    if !context.tools.is_empty() {
        params.insert(
            "tools".into(),
            Value::Array(convert_responses_tools(
                &context.tools,
                None,
                options.constrain_tool_calls,
            )),
        );
    }
    apply_reasoning(
        &mut params,
        model,
        options.reasoning_effort.as_deref(),
        options.reasoning_summary.as_deref(),
        model.provider == "github-copilot",
    );
    Value::Object(params)
}

/// `getServiceTierCostMultiplier`.
fn service_tier_cost_multiplier(model: &Model, service_tier: Option<&str>) -> f64 {
    match service_tier {
        Some("flex") => 0.5,
        Some("priority") if model.id == "gpt-5.5" => 2.5,
        Some("priority") => 2.0,
        _ => 1.0,
    }
}

/// `applyServiceTierPricing`.
pub fn apply_service_tier_pricing(usage: &mut Usage, service_tier: Option<&str>, model: &Model) {
    let multiplier = service_tier_cost_multiplier(model, service_tier);
    if multiplier == 1.0 {
        return;
    }
    let cost = &mut usage.cost;
    cost.input *= multiplier;
    cost.output *= multiplier;
    cost.cache_read *= multiplier;
    cost.cache_write *= multiplier;
    cost.total = cost.input + cost.output + cost.cache_read + cost.cache_write;
}

#[cfg(test)]
mod tests;

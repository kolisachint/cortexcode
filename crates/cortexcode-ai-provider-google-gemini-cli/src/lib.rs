//! Google Cloud Code Assist provider (`google-gemini-cli` API), shared by the
//! `google-gemini-cli` and `google-antigravity` providers: port of hoocode
//! `providers/google-gemini-cli.ts` (v0.5.89).
//!
//! [`stream`] is `streamSimpleGoogleGeminiCli`; [`stream_google_gemini_cli`]
//! takes [`GeminiCliOptions`]. The API key is the JSON `{token, projectId}`
//! the Google OAuth providers hand out.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use cortexcode_ai_provider_google::{
    convert_messages, convert_tools, is_thinking_part, map_stop_reason_string, map_tool_choice,
    retain_thought_signature, GoogleThinking,
};
use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Model, OnPayload,
    SimpleStreamOptions, StopReason, TextContent, ThinkingBudgets, ThinkingContent, ThinkingLevel,
    ToolCallContent, Usage,
};
use futures_util::StreamExt;
use regex::Regex;
use reqwest::header::HeaderMap;
use serde_json::{json, Map, Value};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const DEFAULT_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const ANTIGRAVITY_DAILY_ENDPOINT: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com";
const ANTIGRAVITY_AUTOPUSH_ENDPOINT: &str = "https://autopush-cloudcode-pa.sandbox.googleapis.com";
const ANTIGRAVITY_ENDPOINT_FALLBACKS: [&str; 3] = [
    ANTIGRAVITY_DAILY_ENDPOINT,
    ANTIGRAVITY_AUTOPUSH_ENDPOINT,
    DEFAULT_ENDPOINT,
];
const DEFAULT_ANTIGRAVITY_VERSION: &str = "1.18.4";

/// Antigravity system instruction (compact version from CLIProxyAPI).
const ANTIGRAVITY_SYSTEM_INSTRUCTION: &str = concat!(
    "You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.",
    "You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.",
    "**Absolute paths only**",
    "**Proactiveness**",
);

const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;
const MAX_EMPTY_STREAM_RETRIES: u32 = 2;
const EMPTY_STREAM_BASE_DELAY_MS: u64 = 500;
const CLAUDE_THINKING_BETA_HEADER: &str = "interleaved-thinking-2025-05-14";
const ABORTED: &str = "Request was aborted";
const NO_AUTH: &str =
    "Google Cloud Code Assist requires OAuth authentication. Use /login to authenticate.";

/// `toolCallCounter`: module-wide, as in TS.
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `GoogleGeminiCliOptions`.
#[derive(Debug, Clone, Default)]
pub struct GeminiCliOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u64>,
    pub signal: Option<AbortSignal>,
    /// JSON `{token, projectId}`.
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    /// Longest server-requested retry delay to wait for (default 60 s; 0
    /// waits for any).
    pub max_retry_delay_ms: Option<u64>,
    /// `auto` | `none` | `any`.
    pub tool_choice: Option<String>,
    /// Gemini 2.x: `budget_tokens`; Gemini 3: `level`.
    pub thinking: Option<GoogleThinking>,
    pub on_payload: Option<OnPayload>,
}

/// `streamSimpleGoogleGeminiCli`: fails before streaming without an API key.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or(NO_AUTH)?;
    let options = simple_options(&model, &options, api_key);
    Ok(stream_google_gemini_cli(model, context, options))
}

/// `streamSimpleGoogleGeminiCli`'s option mapping.
pub fn simple_options(
    model: &Model,
    options: &SimpleStreamOptions,
    api_key: String,
) -> GeminiCliOptions {
    let base = GeminiCliOptions {
        temperature: options.temperature,
        max_tokens: options
            .max_tokens
            .or((model.max_tokens > 0).then(|| model.max_tokens.min(32_000))),
        signal: options.signal.clone(),
        api_key: Some(api_key),
        session_id: options.session_id.clone(),
        headers: options.headers.clone(),
        max_retry_delay_ms: options.max_retry_delay_ms,
        on_payload: options.on_payload.clone(),
        ..Default::default()
    };
    let effort = match options.reasoning.clone() {
        None | Some(ThinkingLevel::Off) => {
            return GeminiCliOptions {
                thinking: Some(GoogleThinking::default()),
                ..base
            }
        }
        // `clampReasoning`.
        Some(ThinkingLevel::XHigh) => ThinkingLevel::High,
        Some(level) => level,
    };
    if is_gemini3_model(&model.id) {
        return GeminiCliOptions {
            thinking: Some(GoogleThinking {
                enabled: true,
                level: Some(thinking_level(effort, &model.id).to_string()),
                budget_tokens: None,
            }),
            ..base
        };
    }

    let custom = options.thinking_budgets.clone().unwrap_or(ThinkingBudgets {
        minimal: None,
        low: None,
        medium: None,
        high: None,
        xhigh: None,
    });
    let mut thinking_budget = match effort {
        ThinkingLevel::Minimal => custom.minimal.unwrap_or(1024),
        ThinkingLevel::Low => custom.low.unwrap_or(2048),
        ThinkingLevel::Medium => custom.medium.unwrap_or(8192),
        _ => custom.high.unwrap_or(16384),
    };
    let max_tokens = (base.max_tokens.unwrap_or(0) + thinking_budget).min(model.max_tokens);
    if max_tokens <= thinking_budget {
        thinking_budget = max_tokens.saturating_sub(1024);
    }
    GeminiCliOptions {
        max_tokens: Some(max_tokens),
        thinking: Some(GoogleThinking {
            enabled: true,
            budget_tokens: Some(thinking_budget as i64),
            level: None,
        }),
        ..base
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// `isGemini3ProModel` (`gemini-pro-agent` is Antigravity's Gemini 3.1 Pro).
fn is_gemini3_pro_model(model_id: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let id = model_id.to_lowercase();
    regex(&RE, r"gemini-3(?:\.\d+)?-pro").is_match(&id) || id == "gemini-pro-agent"
}

fn is_gemini3_flash_model(model_id: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(&RE, r"gemini-3(?:\.\d+)?-flash").is_match(&model_id.to_lowercase())
}

/// `rejectsMinimalThinkingLevel`: Gemini 3.5 Flash and newer reject MINIMAL.
fn rejects_minimal_thinking_level(model_id: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(&RE, r"gemini-3(?:\.(\d+))?-flash")
        .captures(&model_id.to_lowercase())
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .is_some_and(|minor| minor >= 5)
}

fn is_gemini3_model(model_id: &str) -> bool {
    is_gemini3_pro_model(model_id) || is_gemini3_flash_model(model_id)
}

/// `getGeminiCliThinkingLevel` (effort already clamped).
fn thinking_level(effort: ThinkingLevel, model_id: &str) -> &'static str {
    if is_gemini3_pro_model(model_id) {
        return match effort {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "LOW",
            _ => "HIGH",
        };
    }
    match effort {
        ThinkingLevel::Minimal if rejects_minimal_thinking_level(model_id) => "LOW",
        ThinkingLevel::Minimal => "MINIMAL",
        ThinkingLevel::Low => "LOW",
        ThinkingLevel::Medium => "MEDIUM",
        _ => "HIGH",
    }
}

fn needs_claude_thinking_beta_header(model: &Model) -> bool {
    model.provider == "google-antigravity" && model.id.starts_with("claude-") && model.reasoning
}

/// `ms` as the TS `normalizeDelay`: `ceil(ms + 1000)` when positive.
fn normalize_delay(ms: f64) -> Option<u64> {
    (ms > 0.0).then(|| (ms + 1000.0).ceil() as u64)
}

/// `Number(value)` for the header forms used here.
fn js_number(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        return Some(0.0);
    }
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `parseInt(value, 10)`: the leading integer.
fn js_parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, digits) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|n| sign * n)
}

/// `parseFloat(value)`: the longest leading decimal number.
fn js_parse_float(value: &str) -> Option<f64> {
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in value.char_indices() {
        match c {
            '0'..='9' => end = i + 1,
            '.' if !seen_dot => seen_dot = true,
            _ => break,
        }
    }
    value[..end].parse::<f64>().ok()
}

fn now_ms() -> f64 {
    cortexcode_ai_types::now_ms() as f64
}

/// `extractRetryDelay`: the server-advertised retry delay in milliseconds
/// (plus a one-second margin), from `Retry-After` / `x-ratelimit-reset` /
/// `x-ratelimit-reset-after`, then the error body ("reset after 1h2m3s",
/// "Please retry in 5s", `"retryDelay": "34.07s"`).
pub fn extract_retry_delay(error_text: &str, headers: Option<&HeaderMap>) -> Option<u64> {
    let header = |name: &str| {
        headers
            .and_then(|h| h.get(name))
            .and_then(|v| v.to_str().ok())
            .filter(|v| !v.is_empty())
    };
    if let Some(retry_after) = header("retry-after") {
        if let Some(delay) = js_number(retry_after).and_then(|s| normalize_delay(s * 1000.0)) {
            return Some(delay);
        }
        let date = chrono::DateTime::parse_from_rfc2822(retry_after)
            .or_else(|_| chrono::DateTime::parse_from_rfc3339(retry_after));
        if let Ok(date) = date {
            if let Some(delay) = normalize_delay(date.timestamp_millis() as f64 - now_ms()) {
                return Some(delay);
            }
        }
    }
    if let Some(reset) = header("x-ratelimit-reset").and_then(js_parse_int) {
        if let Some(delay) = normalize_delay(reset as f64 * 1000.0 - now_ms()) {
            return Some(delay);
        }
    }
    if let Some(after) = header("x-ratelimit-reset-after").and_then(js_number) {
        if let Some(delay) = normalize_delay(after * 1000.0) {
            return Some(delay);
        }
    }

    static DURATION: OnceLock<Regex> = OnceLock::new();
    if let Some(c) = regex(
        &DURATION,
        r"(?i)reset after (?:(\d+)h)?(?:(\d+)m)?(\d+(?:\.\d+)?)s",
    )
    .captures(error_text)
    {
        let int = |i: usize| {
            c.get(i)
                .and_then(|m| m.as_str().parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let seconds = c.get(3).and_then(|m| m.as_str().parse::<f64>().ok());
        if let Some(seconds) = seconds {
            let total_ms = ((int(1) * 60.0 + int(2)) * 60.0 + seconds) * 1000.0;
            if let Some(delay) = normalize_delay(total_ms) {
                return Some(delay);
            }
        }
    }

    let value_with_unit = |c: regex::Captures<'_>| {
        let value = js_parse_float(c.get(1)?.as_str()).filter(|v| *v > 0.0)?;
        let ms = if c.get(2)?.as_str().eq_ignore_ascii_case("ms") {
            value
        } else {
            value * 1000.0
        };
        normalize_delay(ms)
    };
    static RETRY_IN: OnceLock<Regex> = OnceLock::new();
    if let Some(delay) = regex(&RETRY_IN, r"(?i)Please retry in ([0-9.]+)(ms|s)")
        .captures(error_text)
        .and_then(value_with_unit)
    {
        return Some(delay);
    }
    static RETRY_DELAY: OnceLock<Regex> = OnceLock::new();
    regex(&RETRY_DELAY, r#"(?i)"retryDelay":\s*"([0-9.]+)(ms|s)""#)
        .captures(error_text)
        .and_then(value_with_unit)
}

/// `isRetryableError`: rate limits, server errors, and their messages.
fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    regex(
        &RE,
        r"(?i)resource.?exhausted|rate.?limit|overloaded|service.?unavailable|other.?side.?closed",
    )
    .is_match(error_text)
}

/// `extractErrorMessage`: `error.message` of a JSON body, else the body.
fn extract_error_message(error_text: &str) -> String {
    serde_json::from_str::<Value>(error_text)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .filter(|m| !m.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| error_text.to_string())
}

/// `Math.random().toString(36).slice(2, 11)`: nine base-36 characters.
fn random_suffix() -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut n = uuid::Uuid::new_v4().as_u128();
    (0..9)
        .map(|_| {
            let c = DIGITS[(n % 36) as usize] as char;
            n /= 36;
            c
        })
        .collect()
}

/// `buildRequest`: the Cloud Code Assist envelope around a Gemini request.
pub fn build_request(
    model: &Model,
    context: &Context,
    project_id: &str,
    options: &GeminiCliOptions,
    is_antigravity: bool,
) -> Value {
    let mut generation_config = Map::new();
    if let Some(temperature) = options.temperature {
        generation_config.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = options.max_tokens {
        generation_config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    if let Some(thinking) = options.thinking.as_ref().filter(|t| t.enabled) {
        if model.reasoning {
            let mut config = Map::new();
            config.insert("includeThoughts".into(), json!(true));
            if let Some(level) = &thinking.level {
                config.insert("thinkingLevel".into(), json!(level));
            } else if let Some(budget) = thinking.budget_tokens {
                config.insert("thinkingBudget".into(), json!(budget));
            }
            generation_config.insert("thinkingConfig".into(), Value::Object(config));
        }
    }

    let mut request = Map::new();
    request.insert(
        "contents".into(),
        Value::Array(convert_messages(model, context)),
    );
    if let Some(session_id) = &options.session_id {
        request.insert("sessionId".into(), json!(session_id));
    }
    if !context.system_prompt.is_empty() {
        request.insert(
            "systemInstruction".into(),
            json!({"parts": [{"text": context.system_prompt}]}),
        );
    }
    if !generation_config.is_empty() {
        request.insert("generationConfig".into(), Value::Object(generation_config));
    }
    // Claude models on Cloud Code Assist need the legacy `parameters` field.
    if let Some(tools) = convert_tools(&context.tools, model.id.starts_with("claude-")) {
        request.insert("tools".into(), tools);
        if let Some(choice) = &options.tool_choice {
            request.insert(
                "toolConfig".into(),
                json!({"functionCallingConfig": {"mode": map_tool_choice(choice)}}),
            );
        }
    }
    if is_antigravity {
        let mut parts = vec![
            json!({"text": ANTIGRAVITY_SYSTEM_INSTRUCTION}),
            json!({"text": format!("Please ignore following [ignore]{ANTIGRAVITY_SYSTEM_INSTRUCTION}[/ignore]")}),
        ];
        if let Some(existing) = request
            .get("systemInstruction")
            .and_then(|s| s["parts"].as_array())
        {
            parts.extend(existing.iter().cloned());
        }
        // Keeps its key position when a system prompt set it already.
        request.insert(
            "systemInstruction".into(),
            json!({"role": "user", "parts": parts}),
        );
    }

    let mut envelope = Map::new();
    envelope.insert("project".into(), json!(project_id));
    envelope.insert("model".into(), json!(model.id));
    envelope.insert("request".into(), Value::Object(request));
    if is_antigravity {
        envelope.insert("requestType".into(), json!("agent"));
    }
    envelope.insert(
        "userAgent".into(),
        json!(if is_antigravity {
            "antigravity"
        } else {
            "hoocode"
        }),
    );
    envelope.insert(
        "requestId".into(),
        json!(format!(
            "{}-{}-{}",
            if is_antigravity { "agent" } else { "hoo" },
            cortexcode_ai_types::now_ms(),
            random_suffix()
        )),
    );
    Value::Object(envelope)
}

/// The provider's fixed headers: Gemini CLI's, or Antigravity's User-Agent.
fn provider_headers(is_antigravity: bool) -> Vec<(String, String)> {
    if is_antigravity {
        let version = [
            "CORTEXCODE_ANTIGRAVITY_VERSION",
            "HOOCODE_ANTIGRAVITY_VERSION",
        ]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_ANTIGRAVITY_VERSION.to_string());
        return vec![(
            "User-Agent".into(),
            format!("antigravity/{version} darwin/arm64"),
        )];
    }
    vec![
        (
            "User-Agent".into(),
            "google-cloud-sdk vscode_cloudshelleditor/0.1".into(),
        ),
        ("X-Goog-Api-Client".into(), "gl-node/22.17.0".into()),
        (
            "Client-Metadata".into(),
            json!({
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI",
            })
            .to_string(),
        ),
    ]
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
    {
        Some(slot) => *slot = (name.to_string(), value.to_string()),
        None => headers.push((name.to_string(), value.to_string())),
    }
}

/// The request headers of `streamGoogleGeminiCli`.
pub fn build_headers(
    model: &Model,
    access_token: &str,
    options: &GeminiCliOptions,
) -> Vec<(String, String)> {
    let mut headers = vec![
        (
            "Authorization".to_string(),
            format!("Bearer {access_token}"),
        ),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "text/event-stream".to_string()),
    ];
    for (k, v) in provider_headers(model.provider == "google-antigravity") {
        set_header(&mut headers, &k, &v);
    }
    if needs_claude_thinking_beta_header(model) {
        set_header(&mut headers, "anthropic-beta", CLAUDE_THINKING_BETA_HEADER);
    }
    if let Some(extra) = &options.headers {
        for (k, v) in extra {
            set_header(&mut headers, k, v);
        }
    }
    headers
}

/// The endpoints to try in order: Antigravity keeps its fallbacks even when
/// the model names one (which host serves which account moves).
fn endpoints(model: &Model) -> Vec<String> {
    let base_url = model.base_url.trim();
    if model.provider != "google-antigravity" {
        let base = if base_url.is_empty() {
            DEFAULT_ENDPOINT
        } else {
            base_url
        };
        return vec![base.to_string()];
    }
    let mut list: Vec<String> = Vec::new();
    let candidates = (!base_url.is_empty())
        .then_some(base_url)
        .into_iter()
        .chain(ANTIGRAVITY_ENDPOINT_FALLBACKS);
    for endpoint in candidates {
        if !list.iter().any(|e| e == endpoint) {
            list.push(endpoint.to_string());
        }
    }
    list
}

/// `streamGoogleGeminiCli`.
pub fn stream_google_gemini_cli(
    model: Model,
    context: Context,
    options: GeminiCliOptions,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, run(model, context, options, sender));
    stream
}

async fn run(
    model: Model,
    context: Context,
    options: GeminiCliOptions,
    sender: AssistantMessageEventStream,
) {
    let mut state = StreamState::new(&model);
    let signal = options.signal.clone();
    let outcome = match &signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled(), if !signal.aborted() => Err(ABORTED.to_string()),
            r = drive(&model, &context, &options, &mut state, &sender) => r,
        },
        None => drive(&model, &context, &options, &mut state, &sender).await,
    };
    match outcome {
        Ok(()) => {
            let message = state.output;
            sender.push(AssistantMessageEvent::Done {
                message: message.clone(),
            });
            sender.end(Some(message));
        }
        Err(message) => {
            let mut output = state.output;
            output.stop_reason = if signal.as_ref().is_some_and(AbortSignal::aborted) {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            output.error_message = Some(message);
            sender.push(AssistantMessageEvent::Error {
                error: output.clone(),
            });
            sender.end(Some(output));
        }
    }
}

/// `{token, projectId}` from the API key.
fn parse_credentials(api_key: Option<&str>) -> Result<(String, String), String> {
    let raw = api_key.filter(|k| !k.is_empty()).ok_or(NO_AUTH)?;
    let parsed: Value = serde_json::from_str(raw).map_err(|_| {
        "Invalid Google Cloud Code Assist credentials. Use /login to re-authenticate.".to_string()
    })?;
    let field = |name: &str| {
        parsed[name]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match (field("token"), field("projectId")) {
        (Some(token), Some(project_id)) => Ok((token, project_id)),
        _ => Err(
            "Missing token or projectId in Google Cloud credentials. Use /login to re-authenticate."
                .to_string(),
        ),
    }
}

fn post(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &str,
) -> reqwest::RequestBuilder {
    let mut request = client.post(url).body(body.to_string());
    for (k, v) in headers {
        request = request.header(k, v);
    }
    request
}

async fn sleep_ms(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// One pass of the TS retry loop's `try` block.
enum Attempt {
    Done(reqwest::Response),
    Next,
}

/// The try block of `streamGoogleGeminiCli`.
async fn drive(
    model: &Model,
    context: &Context,
    options: &GeminiCliOptions,
    state: &mut StreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), String> {
    let (access_token, project_id) = parse_credentials(options.api_key.as_deref())?;
    let is_antigravity = model.provider == "google-antigravity";
    let endpoints = endpoints(model);
    let body = build_request(model, context, &project_id, options, is_antigravity);
    let body = OnPayload::apply(options.on_payload.as_ref(), body, model)
        .await
        .to_string();
    let headers = build_headers(model, &access_token, options);
    let client = reqwest::Client::new();

    // 403/404 cascade to the next endpoint at once; 429/5xx back off. Errors
    // raised inside the loop (including a non-retryable status) are caught
    // and retried with backoff like network errors, as in TS.
    let mut response = None;
    let mut last_error: Option<String> = None;
    let mut request_url = String::new();
    let mut endpoint_index = 0;
    for attempt in 0..=MAX_RETRIES {
        request_url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            endpoints[endpoint_index]
        );
        let result = try_request(
            &client,
            &request_url,
            &headers,
            &body,
            attempt,
            &mut endpoint_index,
            endpoints.len(),
            options,
        )
        .await;
        match result {
            Ok(Attempt::Done(r)) => {
                response = Some(r);
                break;
            }
            Ok(Attempt::Next) => continue,
            Err(error) => {
                last_error = Some(error);
                if attempt < MAX_RETRIES {
                    sleep_ms(BASE_DELAY_MS * 2u64.pow(attempt)).await;
                    continue;
                }
                return Err(last_error.unwrap_or_default());
            }
        }
    }
    let response = response.ok_or_else(|| {
        last_error.unwrap_or_else(|| "Failed to get response after retries".to_string())
    })?;

    // An empty stream is fetched again (0.5 s, 1 s) from the same URL.
    let mut first = Some(response);
    let mut received_content = false;
    for empty_attempt in 0..=MAX_EMPTY_STREAM_RETRIES {
        let current = match first.take() {
            Some(response) => response,
            None => {
                sleep_ms(EMPTY_STREAM_BASE_DELAY_MS * 2u64.pow(empty_attempt - 1)).await;
                let retry = post(&client, &request_url, &headers, &body)
                    .send()
                    .await
                    .map_err(|e| network_error(&e))?;
                if !retry.status().is_success() {
                    let status = retry.status().as_u16();
                    let text = retry.text().await.unwrap_or_default();
                    return Err(format!("Cloud Code Assist API error ({status}): {text}"));
                }
                retry
            }
        };
        if state.stream_response(model, current, sender).await? {
            received_content = true;
            break;
        }
        if empty_attempt < MAX_EMPTY_STREAM_RETRIES {
            state.reset();
        }
    }
    if !received_content {
        return Err("Cloud Code Assist API returned an empty response".to_string());
    }
    if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err(ABORTED.to_string());
    }
    if matches!(
        state.output.stop_reason,
        StopReason::Aborted | StopReason::Error
    ) {
        return Err("An unknown error occurred".to_string());
    }
    Ok(())
}

/// `fetch failed` with its cause: `Network error: <cause>`.
fn network_error(error: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    format!("Network error: {cause}")
}

#[allow(clippy::too_many_arguments)]
async fn try_request(
    client: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    attempt: u32,
    endpoint_index: &mut usize,
    endpoint_count: usize,
    options: &GeminiCliOptions,
) -> Result<Attempt, String> {
    let response = post(client, url, headers, body)
        .send()
        .await
        .map_err(|e| network_error(&e))?;
    if response.status().is_success() {
        return Ok(Attempt::Done(response));
    }
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let error_text = response.text().await.unwrap_or_default();

    if matches!(status, 403 | 404) && *endpoint_index < endpoint_count - 1 {
        *endpoint_index += 1;
        return Ok(Attempt::Next);
    }
    if attempt < MAX_RETRIES && is_retryable_error(status, &error_text) {
        if *endpoint_index < endpoint_count - 1 {
            *endpoint_index += 1;
        }
        let server_delay = extract_retry_delay(&error_text, Some(&response_headers));
        let delay_ms = server_delay.unwrap_or(BASE_DELAY_MS * 2u64.pow(attempt));
        let max_delay_ms = options.max_retry_delay_ms.unwrap_or(60_000);
        if let Some(server_delay) = server_delay.filter(|d| max_delay_ms > 0 && *d > max_delay_ms) {
            return Err(format!(
                "Server requested {}s retry delay (max: {}s). {}",
                server_delay.div_ceil(1000),
                max_delay_ms.div_ceil(1000),
                extract_error_message(&error_text)
            ));
        }
        sleep_ms(delay_ms).await;
        return Ok(Attempt::Next);
    }
    Err(format!(
        "Cloud Code Assist API error ({status}): {}",
        extract_error_message(&error_text)
    ))
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

struct StreamState {
    output: AssistantMessage,
    /// Whether `start` was pushed (`ensureStarted`).
    started: bool,
}

impl StreamState {
    fn new(model: &Model) -> Self {
        let mut output = AssistantMessage::for_model(model);
        output.api = "google-gemini-cli".to_string();
        Self {
            output,
            started: false,
        }
    }

    /// `resetOutput` before retrying an empty stream.
    fn reset(&mut self) {
        self.output.content.clear();
        self.output.usage = Usage::default();
        self.output.stop_reason = StopReason::Stop;
        self.output.error_message = None;
        self.output.timestamp = cortexcode_ai_types::now_ms();
        self.started = false;
    }

    fn ensure_started(&mut self, sender: &AssistantMessageEventStream) {
        if !self.started {
            sender.push(AssistantMessageEvent::Start {
                partial: self.output.clone(),
            });
            self.started = true;
        }
    }

    /// `streamResponse`: whether any content arrived. Lines are split on
    /// `\n`; `data:` lines that are not JSON are skipped, and an unterminated
    /// last line is dropped.
    async fn stream_response(
        &mut self,
        model: &Model,
        response: reqwest::Response,
        sender: &AssistantMessageEventStream,
    ) -> Result<bool, String> {
        let mut has_content = false;
        let mut current: Option<usize> = None;
        let mut buffer: Vec<u8> = Vec::new();
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| network_error(&e))?;
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(chunk) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                if self.handle_chunk(model, &chunk, &mut current, sender) {
                    has_content = true;
                }
            }
        }
        self.close_block(&mut current, sender);
        Ok(has_content)
    }

    fn close_block(&mut self, current: &mut Option<usize>, sender: &AssistantMessageEventStream) {
        let Some(index) = current.take() else {
            return;
        };
        let partial = self.output.clone();
        match &self.output.content[index] {
            Content::Thinking(_) => {
                sender.push(AssistantMessageEvent::ThinkingEnd { index, partial })
            }
            _ => sender.push(AssistantMessageEvent::TextEnd { index, partial }),
        }
    }

    /// One `data:` chunk; whether it carried content.
    fn handle_chunk(
        &mut self,
        model: &Model,
        chunk: &Value,
        current: &mut Option<usize>,
        sender: &AssistantMessageEventStream,
    ) -> bool {
        let Some(response) = chunk.get("response").filter(|r| r.is_object()) else {
            return false;
        };
        if self.output.response_id.as_deref().unwrap_or("").is_empty() {
            if let Some(id) = response["responseId"].as_str().filter(|id| !id.is_empty()) {
                self.output.response_id = Some(id.to_string());
            }
        }
        let mut has_content = false;
        let candidate = &response["candidates"][0];
        if let Some(parts) = candidate["content"]["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    has_content = true;
                    self.handle_text(part, text, current, sender);
                }
                if let Some(call) = part.get("functionCall").filter(|c| c.is_object()) {
                    has_content = true;
                    self.handle_function_call(part, call, current, sender);
                }
            }
        }

        if let Some(reason) = candidate["finishReason"].as_str().filter(|r| !r.is_empty()) {
            self.output.stop_reason = map_stop_reason_string(reason);
            if self
                .output
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)))
            {
                self.output.stop_reason = StopReason::ToolUse;
            }
        }

        if let Some(usage) = response.get("usageMetadata").filter(|u| u.is_object()) {
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let u = &mut self.output.usage;
            u.input = count("promptTokenCount").saturating_sub(count("cachedContentTokenCount"));
            u.output = count("candidatesTokenCount") + count("thoughtsTokenCount");
            u.cache_read = count("cachedContentTokenCount");
            u.cache_write = 0;
            u.total_tokens = count("totalTokenCount");
            u.cost = cortexcode_ai_models::calculate_cost(model, u);
        }
        has_content
    }

    fn handle_text(
        &mut self,
        part: &Value,
        text: &str,
        current: &mut Option<usize>,
        sender: &AssistantMessageEventStream,
    ) {
        let thinking = is_thinking_part(part);
        let open_kind_matches = current
            .is_some_and(|i| matches!(self.output.content[i], Content::Thinking(_)) == thinking);
        if !open_kind_matches {
            self.close_block(current, sender);
            let block = if thinking {
                Content::Thinking(ThinkingContent::default())
            } else {
                Content::Text(TextContent::new(""))
            };
            self.output.content.push(block);
            let index = self.output.content.len() - 1;
            *current = Some(index);
            self.ensure_started(sender);
            let partial = self.output.clone();
            sender.push(if thinking {
                AssistantMessageEvent::ThinkingStart { index, partial }
            } else {
                AssistantMessageEvent::TextStart { index, partial }
            });
        }
        let index = current.unwrap_or_default();
        let signature = part["thoughtSignature"].as_str();
        match &mut self.output.content[index] {
            Content::Thinking(block) => {
                block.thinking.push_str(text);
                block.signature = retain_thought_signature(block.signature.take(), signature);
            }
            Content::Text(block) => {
                block.text.push_str(text);
                block.text_signature =
                    retain_thought_signature(block.text_signature.take(), signature);
            }
            _ => {}
        }
        let partial = self.output.clone();
        let delta = text.to_string();
        sender.push(if thinking {
            AssistantMessageEvent::ThinkingDelta {
                index,
                delta,
                partial,
            }
        } else {
            AssistantMessageEvent::TextDelta {
                index,
                delta,
                partial,
            }
        });
    }

    fn handle_function_call(
        &mut self,
        part: &Value,
        call: &Value,
        current: &mut Option<usize>,
        sender: &AssistantMessageEventStream,
    ) {
        self.close_block(current, sender);
        let name = call["name"].as_str().unwrap_or_default().to_string();
        let provided = call["id"].as_str().filter(|id| !id.is_empty());
        let duplicate = provided.is_some_and(|id| {
            self.output
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(t) if t.id == id))
        });
        let id = match provided {
            Some(id) if !duplicate => id.to_string(),
            _ => format!(
                "{name}_{}_{}",
                cortexcode_ai_types::now_ms(),
                TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
            ),
        };
        let arguments = match call.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(args) => args.clone(),
        };
        let thought_signature = part["thoughtSignature"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self.output.content.push(Content::ToolCall(ToolCallContent {
            id,
            name,
            arguments: arguments.clone(),
            thought_signature,
        }));
        let index = self.output.content.len() - 1;
        self.ensure_started(sender);
        sender.push(AssistantMessageEvent::ToolCallStart {
            index,
            partial: self.output.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallDelta {
            index,
            delta: arguments.to_string(),
            partial: self.output.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallEnd {
            index,
            partial: self.output.clone(),
        });
    }
}

#[cfg(test)]
mod tests;

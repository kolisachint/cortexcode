//! OpenAI Codex Responses provider (`openai-codex-responses`, the ChatGPT
//! subscription backend) for cortex AI.
//!
//! Port of hoocode `providers/openai-codex-responses.ts` (v0.5.89): the
//! request body and headers, the WebSocket transport with its per-session
//! connection cache and `previous_response_id` continuation ([`websocket`]),
//! and the SSE fallback with its own retry loop. Events go through the shared
//! Responses stream processor of `cortexcode-ai-provider-openai-responses`.
//!
//! Known deviations: the `onPayload` / `onResponse` hooks are not supported
//! (ledger task 8.8), errors carry no JS stack in their diagnostics, and a JSON
//! parse failure reads with serde's message instead of V8's.

mod websocket;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use base64::Engine as _;
use cortexcode_ai_provider_openai_responses::shared::{
    convert_responses_messages, convert_responses_tools, set_header, ResponsesStreamOptions,
    ResponsesStreamState,
};
use cortexcode_ai_provider_openai_responses::{apply_service_tier_pricing, mapped_effort};
use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessageEvent, Context, Model, SimpleStreamOptions, StopReason, Transport,
};
use cortexcode_ai_util::{
    append_assistant_message_diagnostic, create_assistant_message_diagnostic, DiagnosticError,
};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};

pub use websocket::{
    close_openai_codex_websocket_sessions, get_openai_codex_websocket_debug_stats,
    reset_openai_codex_websocket_debug_stats, OpenAICodexWebSocketDebugStats,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 1000;
/// Providers whose `call_id|item_id` tool call ids stay paired on replay.
const CODEX_TOOL_CALL_PROVIDERS: [&str; 3] = ["openai", "openai-codex", "opencode"];
const CODEX_RESPONSE_STATUSES: [&str; 6] = [
    "completed",
    "incomplete",
    "failed",
    "cancelled",
    "queued",
    "in_progress",
];

/// The originator the Codex OAuth client is registered under. A protocol
/// value the ChatGPT backend validates together with the client id (and
/// cross-checks with the User-Agent), not branding.
const CODEX_ORIGINATOR: &str = "pi";

/// `OpenAICodexResponsesOptions`.
#[derive(Debug, Clone, Default)]
pub struct CodexOptions {
    pub temperature: Option<f64>,
    pub signal: Option<AbortSignal>,
    pub api_key: Option<String>,
    pub session_id: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    /// Default `auto`: WebSocket with cached context, SSE on failure.
    pub transport: Option<Transport>,
    /// `none`, `minimal`, `low`, `medium`, `high` or `xhigh`.
    pub reasoning_effort: Option<String>,
    /// `auto` (default), `concise`, `detailed`, `off` or `on`.
    pub reasoning_summary: Option<String>,
    pub service_tier: Option<String>,
    /// `low` (default), `medium` or `high`.
    pub text_verbosity: Option<String>,
}

// =============================================================================
// Errors
// =============================================================================

/// What a Codex request can fail with. `Api` and `Protocol` are
/// `CodexApiError` / `CodexProtocolError`: they never fall back to SSE.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CodexError {
    Api(String),
    Protocol(String),
    /// Any other error (transport failures, HTTP errors).
    Other(DiagnosticError),
    Aborted,
}

impl CodexError {
    fn other(message: impl Into<String>) -> Self {
        Self::Other(DiagnosticError::error(message))
    }

    fn message(&self) -> String {
        match self {
            Self::Api(m) | Self::Protocol(m) => m.clone(),
            Self::Other(e) => e.message.clone(),
            Self::Aborted => "Request was aborted".to_string(),
        }
    }

    /// `isCodexNonTransportError`.
    fn is_non_transport(&self) -> bool {
        matches!(self, Self::Api(_) | Self::Protocol(_))
    }

    fn diagnostic_error(&self) -> DiagnosticError {
        match self {
            Self::Other(e) => e.clone(),
            other => DiagnosticError::error(other.message()),
        }
    }
}

fn aborted(signal: Option<&AbortSignal>) -> bool {
    signal.is_some_and(AbortSignal::aborted)
}

/// Await `future`, or fail with `Request was aborted` once `signal` fires.
pub(crate) async fn abortable<F: std::future::Future>(
    signal: Option<&AbortSignal>,
    future: F,
) -> Result<F::Output, CodexError> {
    match signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled() => Err(CodexError::Aborted),
            output = future => Ok(output),
        },
        None => Ok(future.await),
    }
}

// =============================================================================
// Stream functions
// =============================================================================

/// `streamSimpleOpenAICodexResponses`: missing credentials are an immediate
/// error, as the TS function throws.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = cortexcode_ai_provider_openai_responses::require_api_key(&model, &options)?;
    let base =
        cortexcode_ai_provider_openai_responses::simple_options(&model, &options, api_key.clone());
    let options = CodexOptions {
        temperature: options.temperature,
        signal: options.signal.clone(),
        api_key: Some(api_key),
        session_id: options.session_id.clone(),
        headers: options.headers.clone(),
        transport: options.transport,
        reasoning_effort: base.reasoning_effort,
        ..CodexOptions::default()
    };
    Ok(stream_codex_responses(model, context, options))
}

/// `streamOpenAICodexResponses`.
pub fn stream_codex_responses(
    model: Model,
    context: Context,
    options: CodexOptions,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, async move {
        let mut state = ResponsesStreamState::new(&model);
        let outcome = run(&model, &context, &options, &mut state, &sender).await;
        let mut output = state.output;
        match outcome {
            Ok(()) => {
                sender.push(AssistantMessageEvent::Done {
                    message: output.clone(),
                });
                sender.end(Some(output));
            }
            Err(error) => {
                output.stop_reason = if aborted(options.signal.as_ref()) {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                };
                output.error_message = Some(error.message());
                sender.push(AssistantMessageEvent::Error {
                    error: output.clone(),
                });
                sender.end(Some(output));
            }
        }
    });
    stream
}

fn stream_options(options: &CodexOptions) -> ResponsesStreamOptions {
    ResponsesStreamOptions {
        service_tier: options.service_tier.clone(),
        resolve_service_tier: Some(resolve_codex_service_tier),
        apply_service_tier_pricing: Some(apply_service_tier_pricing),
    }
}

async fn run(
    model: &Model,
    context: &Context,
    options: &CodexOptions,
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), CodexError> {
    let signal = options.signal.as_ref();
    let api_key = options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
        .ok_or_else(|| CodexError::other(format!("No API key for provider: {}", model.provider)))?;
    let account_id = extract_account_id(&api_key)?;
    let body = build_request_body(model, context, options);
    let websocket_request_id = options
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let sse_headers = build_sse_headers(
        model.headers.as_ref(),
        options.headers.as_ref(),
        &account_id,
        &api_key,
        options.session_id.as_deref(),
    );
    let websocket_headers = build_websocket_headers(
        model.headers.as_ref(),
        options.headers.as_ref(),
        &account_id,
        &api_key,
        &websocket_request_id,
    );
    let body_json = body.to_string();
    let transport = options.transport.unwrap_or_default();
    let session_id = options.session_id.as_deref();
    let websocket_disabled =
        transport != Transport::Sse && websocket::is_sse_fallback_active(session_id);
    if websocket_disabled {
        websocket::record_sse_fallback(session_id);
    }

    if transport != Transport::Sse && !websocket_disabled {
        let mut started = false;
        let result = websocket::process_websocket_stream(
            &resolve_codex_websocket_url(&model.base_url),
            &body,
            &websocket_headers,
            state,
            sender,
            model,
            &mut started,
            options,
        )
        .await;
        match result {
            Ok(()) => {
                if aborted(signal) {
                    return Err(CodexError::Aborted);
                }
                return Ok(());
            }
            Err(error) => {
                if aborted(signal) || error.is_non_transport() {
                    return Err(error);
                }
                let mut details = Map::new();
                details.insert("configuredTransport".into(), transport_json(transport));
                if !started {
                    details.insert("fallbackTransport".into(), json!("sse"));
                }
                details.insert("eventsEmitted".into(), json!(started));
                details.insert(
                    "phase".into(),
                    json!(if started {
                        "after_message_stream_start"
                    } else {
                        "before_message_stream_start"
                    }),
                );
                details.insert("requestBytes".into(), json!(body_json.len()));
                append_assistant_message_diagnostic(
                    &mut state.output,
                    create_assistant_message_diagnostic(
                        "provider_transport_failure",
                        &error.diagnostic_error(),
                        Some(details),
                    ),
                );
                websocket::record_websocket_failure(session_id, &error);
                if started {
                    return Err(error);
                }
                websocket::record_sse_fallback(session_id);
            }
        }
    }

    let response = fetch_with_retries(model, &sse_headers, body_json, signal).await?;
    sender.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });
    process_sse(response, state, sender, model, options).await?;
    if aborted(signal) {
        return Err(CodexError::Aborted);
    }
    Ok(())
}

fn transport_json(transport: Transport) -> Value {
    serde_json::to_value(transport).unwrap_or(Value::Null)
}

// =============================================================================
// SSE transport
// =============================================================================

/// `isRetryableError`.
fn is_retryable_error(status: u16, error_text: &str) -> bool {
    if matches!(status, 429 | 500 | 502 | 503 | 504) {
        return true;
    }
    let text = error_text.to_lowercase();
    // `a.?b`: `.` is any character but a line terminator.
    let spaced = |a: &str, b: &str| {
        text.match_indices(a).any(|(i, _)| {
            let rest = &text[i + a.len()..];
            rest.starts_with(b)
                || rest.chars().next().is_some_and(|c| {
                    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
                        && rest[c.len_utf8()..].starts_with(b)
                })
        })
    };
    spaced("rate", "limit")
        || text.contains("overloaded")
        || spaced("service", "unavailable")
        || spaced("upstream", "connect")
        || spaced("connection", "refused")
}

async fn sleep(ms: u64, signal: Option<&AbortSignal>) -> Result<(), CodexError> {
    if aborted(signal) {
        return Err(CodexError::Aborted);
    }
    abortable(signal, tokio::time::sleep(Duration::from_millis(ms))).await
}

/// The POST with retries for rate limits and transient errors. Any failure
/// other than a usage limit is retried, as in the TS loop.
async fn fetch_with_retries(
    model: &Model,
    headers: &[(String, String)],
    body_json: String,
    signal: Option<&AbortSignal>,
) -> Result<reqwest::Response, CodexError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| CodexError::other(format!("failed to build HTTP client: {e}")))?;
    let url = resolve_codex_url(&model.base_url);
    for attempt in 0..=MAX_RETRIES {
        if aborted(signal) {
            return Err(CodexError::Aborted);
        }
        let mut request = client.post(&url).body(body_json.clone());
        for (k, v) in headers {
            request = request.header(k.as_str(), v.as_str());
        }
        let error = match abortable(signal, request.send()).await? {
            Err(e) => CodexError::other(format!("fetch failed: {e}")),
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) => {
                let status = response.status();
                let error_text = abortable(signal, response.text())
                    .await?
                    .unwrap_or_default();
                if attempt < MAX_RETRIES && is_retryable_error(status.as_u16(), &error_text) {
                    sleep(BASE_DELAY_MS * 2u64.pow(attempt), signal).await?;
                    continue;
                }
                let (message, friendly) = parse_error_response(
                    status.as_u16(),
                    status.canonical_reason().unwrap_or_default(),
                    &error_text,
                    cortexcode_ai_types::now_ms(),
                );
                CodexError::other(friendly.unwrap_or(message))
            }
        };
        if attempt < MAX_RETRIES && !error.message().contains("usage limit") {
            sleep(BASE_DELAY_MS * 2u64.pow(attempt), signal).await?;
            continue;
        }
        return Err(error);
    }
    Err(CodexError::other("Failed after retries"))
}

/// `parseSSE` + `mapCodexEvents` + `processResponsesStream` over the body.
async fn process_sse(
    response: reqwest::Response,
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
    model: &Model,
    options: &CodexOptions,
) -> Result<(), CodexError> {
    let signal = options.signal.as_ref();
    let stream_options = stream_options(options);
    let mut body = response.bytes_stream();
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = abortable(signal, body.next()).await? {
        let chunk = chunk.map_err(|e| CodexError::other(format!("terminated: {e}")))?;
        buffer.extend_from_slice(&chunk);
        while let Some(idx) = buffer.windows(2).position(|w| w == b"\n\n") {
            let raw: Vec<u8> = buffer.drain(..idx + 2).collect();
            let chunk = String::from_utf8_lossy(&raw[..idx]);
            let Some(event) = parse_sse_chunk(&chunk)? else {
                continue;
            };
            if handle_codex_event(event, state, sender, model, &stream_options)? {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// One `\n\n`-separated SSE chunk: its `data:` lines joined, `[DONE]` and
/// empty data skipped.
fn parse_sse_chunk(chunk: &str) -> Result<Option<Value>, CodexError> {
    let data_lines: Vec<&str> = chunk
        .split('\n')
        .filter_map(|l| l.strip_prefix("data:"))
        .map(str::trim)
        .collect();
    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    serde_json::from_str(data)
        .map(Some)
        .map_err(|e| CodexError::Protocol(format!("Invalid Codex SSE JSON: {e}")))
}

/// `mapCodexEvents` for one event, then `processResponsesStream`. Returns
/// `true` once the terminal event was handled (the generator returns).
pub(crate) fn handle_codex_event(
    event: Value,
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
    model: &Model,
    options: &ResponsesStreamOptions,
) -> Result<bool, CodexError> {
    let Some(mapped) = map_codex_event(event)? else {
        return Ok(false);
    };
    let terminal = mapped["type"] == "response.completed";
    state
        .handle_event(&mapped, model, options, sender)
        .map_err(CodexError::Api)?;
    Ok(terminal)
}

/// `mapCodexEvents`: `error` / `response.failed` throw, the terminal events
/// become `response.completed` with a normalized status, events without a
/// type are dropped.
pub(crate) fn map_codex_event(mut event: Value) -> Result<Option<Value>, CodexError> {
    let Some(kind) = event
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(None);
    };
    match kind.as_str() {
        "error" => {
            let code = truthy_str(&event["code"]);
            let message = truthy_str(&event["message"]);
            let text = message.or(code).unwrap_or_else(|| event.to_string());
            Err(CodexError::Api(format!("Codex error: {text}")))
        }
        "response.failed" => Err(CodexError::Api(
            truthy_str(&event["response"]["error"]["message"])
                .unwrap_or_else(|| "Codex response failed".to_string()),
        )),
        "response.done" | "response.completed" | "response.incomplete" => {
            if let Some(response) = event.get_mut("response").filter(|r| r.is_object()) {
                let status = response["status"]
                    .as_str()
                    .filter(|s| CODEX_RESPONSE_STATUSES.contains(s))
                    .map(|s| json!(s));
                match status {
                    Some(status) => response["status"] = status,
                    None => {
                        if let Some(obj) = response.as_object_mut() {
                            obj.remove("status");
                        }
                    }
                }
            }
            event["type"] = json!("response.completed");
            Ok(Some(event))
        }
        _ => Ok(Some(event)),
    }
}

fn truthy_str(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// `parseErrorResponse`: `(message, friendlyMessage)`.
fn parse_error_response(
    status: u16,
    status_text: &str,
    raw: &str,
    now_ms: i64,
) -> (String, Option<String>) {
    let mut message = [raw, status_text]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("Request failed")
        .to_string();
    let mut friendly = None;
    if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
        let err = &parsed["error"];
        if is_truthy(err) {
            let code = truthy_str(&err["code"])
                .or_else(|| truthy_str(&err["type"]))
                .unwrap_or_default()
                .to_ascii_lowercase();
            let limited = [
                "usage_limit_reached",
                "usage_not_included",
                "rate_limit_exceeded",
            ]
            .iter()
            .any(|c| code.contains(c));
            if limited || status == 429 {
                let plan = truthy_str(&err["plan_type"])
                    .map(|p| format!(" ({} plan)", p.to_lowercase()))
                    .unwrap_or_default();
                let when = err["resets_at"]
                    .as_f64()
                    .filter(|r| *r != 0.0)
                    .map(|resets_at| {
                        let mins = ((resets_at * 1000.0 - now_ms as f64) / 60000.0 + 0.5).floor();
                        format!(" Try again in ~{} min.", mins.max(0.0) as i64)
                    })
                    .unwrap_or_default();
                friendly = Some(
                    format!("You have hit your ChatGPT usage limit{plan}.{when}")
                        .trim()
                        .to_string(),
                );
            }
            message = truthy_str(&err["message"])
                .or_else(|| friendly.clone())
                .unwrap_or(message);
        }
    }
    (message, friendly)
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

// =============================================================================
// Request building
// =============================================================================

/// `buildRequestBody`.
pub fn build_request_body(model: &Model, context: &Context, options: &CodexOptions) -> Value {
    let allowed: HashSet<&str> = CODEX_TOOL_CALL_PROVIDERS.into_iter().collect();
    let input = convert_responses_messages(model, context, &allowed, false);
    let mut body = Map::new();
    body.insert("model".into(), json!(model.id));
    body.insert("store".into(), json!(false));
    body.insert("stream".into(), json!(true));
    let instructions = if context.system_prompt.is_empty() {
        "You are a helpful assistant."
    } else {
        context.system_prompt.as_str()
    };
    body.insert("instructions".into(), json!(instructions));
    body.insert("input".into(), Value::Array(input));
    body.insert(
        "text".into(),
        json!({"verbosity": options.text_verbosity.as_deref().filter(|v| !v.is_empty()).unwrap_or("low")}),
    );
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    if let Some(session_id) = &options.session_id {
        body.insert("prompt_cache_key".into(), json!(session_id));
    }
    body.insert("tool_choice".into(), json!("auto"));
    body.insert("parallel_tool_calls".into(), json!(true));
    if let Some(temperature) = options.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(tier) = &options.service_tier {
        body.insert("service_tier".into(), json!(tier));
    }
    if !context.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(convert_responses_tools(
                &context.tools,
                Some(Value::Null),
                false,
            )),
        );
    }
    if let Some(effort) = &options.reasoning_effort {
        let effort = if effort == "none" {
            match model.thinking_level_map.as_ref().and_then(|m| m.get("off")) {
                Some(Value::Null) | None => json!("none"),
                Some(off) => off.clone(),
            }
        } else {
            mapped_effort(model, effort)
        };
        body.insert(
            "reasoning".into(),
            json!({
                "effort": effort,
                "summary": options.reasoning_summary.as_deref().unwrap_or("auto"),
            }),
        );
    }
    Value::Object(body)
}

/// `resolveCodexServiceTier`: Codex echoes `default` for flex/priority.
pub fn resolve_codex_service_tier(
    response_tier: Option<&str>,
    request_tier: Option<&str>,
) -> Option<String> {
    if response_tier == Some("default") && matches!(request_tier, Some("flex" | "priority")) {
        return request_tier.map(str::to_string);
    }
    response_tier.or(request_tier).map(str::to_string)
}

/// `resolveCodexUrl`.
pub fn resolve_codex_url(base_url: &str) -> String {
    let raw = if base_url.trim().is_empty() {
        DEFAULT_CODEX_BASE_URL
    } else {
        base_url
    };
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_string()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

/// `resolveCodexWebSocketUrl`: `https:` → `wss:`, `http:` → `ws:`.
pub fn resolve_codex_websocket_url(base_url: &str) -> String {
    let url = resolve_codex_url(base_url);
    if let Some(rest) = url.strip_prefix("https:") {
        format!("wss:{rest}")
    } else if let Some(rest) = url.strip_prefix("http:") {
        format!("ws:{rest}")
    } else {
        url
    }
}

// =============================================================================
// Auth & headers
// =============================================================================

/// `extractAccountId`: the `chatgpt_account_id` claim of the access token.
pub(crate) fn extract_account_id(token: &str) -> Result<String, CodexError> {
    let failed = || CodexError::other("Failed to extract accountId from token");
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(failed());
    }
    let payload = atob(parts[1]).ok_or_else(failed)?;
    let payload: Value = serde_json::from_slice(&payload).map_err(|_| failed())?;
    payload[JWT_CLAIM_PATH]["chatgpt_account_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(failed)
}

/// `atob`: standard base64, padding optional.
fn atob(input: &str) -> Option<Vec<u8>> {
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    engine.decode(cleaned).ok()
}

/// `os.platform()`.
fn os_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// `os.arch()`.
fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        "powerpc64" => "ppc64",
        other => other,
    }
}

/// `os.release()`: the kernel release (`uname -r`).
fn os_release() -> String {
    #[cfg(unix)]
    {
        // SAFETY: `uname` fills a zeroed `utsname`; `release` is a
        // NUL-terminated C string inside it.
        unsafe {
            let mut name: libc::utsname = std::mem::zeroed();
            if libc::uname(&mut name) == 0 {
                return std::ffi::CStr::from_ptr(name.release.as_ptr())
                    .to_string_lossy()
                    .into_owned();
            }
        }
        String::new()
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}

/// `buildBaseCodexHeaders`.
fn build_base_codex_headers(
    init_headers: Option<&HashMap<String, String>>,
    additional_headers: Option<&HashMap<String, String>>,
    account_id: &str,
    token: &str,
) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for source in [init_headers, additional_headers].into_iter().flatten() {
        for (k, v) in source {
            set_header(&mut headers, &k.to_ascii_lowercase(), v);
        }
    }
    set_header(&mut headers, "authorization", &format!("Bearer {token}"));
    set_header(&mut headers, "chatgpt-account-id", account_id);
    set_header(&mut headers, "originator", CODEX_ORIGINATOR);
    let user_agent = format!(
        "{CODEX_ORIGINATOR} ({} {}; {})",
        os_platform(),
        os_release(),
        os_arch()
    );
    set_header(&mut headers, "user-agent", &user_agent);
    headers
}

/// `buildSSEHeaders`.
pub fn build_sse_headers(
    init_headers: Option<&HashMap<String, String>>,
    additional_headers: Option<&HashMap<String, String>>,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = build_base_codex_headers(init_headers, additional_headers, account_id, token);
    set_header(&mut headers, "openai-beta", "responses=experimental");
    set_header(&mut headers, "accept", "text/event-stream");
    set_header(&mut headers, "content-type", "application/json");
    if let Some(session_id) = session_id {
        set_header(&mut headers, "session_id", session_id);
        set_header(&mut headers, "x-client-request-id", session_id);
    }
    headers
}

/// `buildWebSocketHeaders`.
pub fn build_websocket_headers(
    init_headers: Option<&HashMap<String, String>>,
    additional_headers: Option<&HashMap<String, String>>,
    account_id: &str,
    token: &str,
    request_id: &str,
) -> Vec<(String, String)> {
    let mut headers = build_base_codex_headers(init_headers, additional_headers, account_id, token);
    headers.retain(|(k, _)| !matches!(k.as_str(), "accept" | "content-type" | "openai-beta"));
    set_header(
        &mut headers,
        "openai-beta",
        websocket::OPENAI_BETA_RESPONSES_WEBSOCKETS,
    );
    set_header(&mut headers, "x-client-request-id", request_id);
    set_header(&mut headers, "session_id", request_id);
    headers
}

#[cfg(test)]
mod tests;

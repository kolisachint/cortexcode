//! The WebSocket transport of `openai-codex-responses.ts`: one connection per
//! session kept for 5 minutes between requests, `previous_response_id`
//! continuation that sends only the input delta, the per-session SSE fallback
//! after a WebSocket failure, and the debug stats.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use cortexcode_ai_provider_openai_responses::shared::{
    convert_responses_messages, ResponsesStreamState,
};
use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{AbortSignal, AssistantMessageEvent, Context, Message, Model, Transport};
use cortexcode_ai_util::DiagnosticError;
use futures_util::{FutureExt, SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::{abortable, map_codex_event, stream_options, CodexError, CodexOptions};

pub(crate) const OPENAI_BETA_RESPONSES_WEBSOCKETS: &str = "responses_websockets=2026-02-06";
const SESSION_WEBSOCKET_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE: u16 = 1009;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// `OpenAICodexWebSocketDebugStats`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenAICodexWebSocketDebugStats {
    pub requests: u64,
    pub connections_created: u64,
    pub connections_reused: u64,
    pub cached_context_requests: u64,
    pub store_true_requests: u64,
    pub full_context_requests: u64,
    pub delta_requests: u64,
    pub last_input_items: u64,
    pub last_delta_input_items: Option<u64>,
    pub last_previous_response_id: Option<String>,
    pub websocket_failures: u64,
    pub sse_fallbacks: u64,
    pub websocket_fallback_active: Option<bool>,
    pub last_websocket_error: Option<String>,
}

/// `CachedWebSocketContinuationState`.
struct Continuation {
    last_request_body: Value,
    last_response_id: String,
    last_response_items: Vec<Value>,
}

/// `CachedWebSocketConnection`. The socket is taken out while a request
/// uses it (`busy`); `id` tells a replaced entry from the current one.
struct CachedConnection {
    id: u64,
    socket: Option<Socket>,
    busy: bool,
    idle_timer: Option<tokio::task::JoinHandle<()>>,
    continuation: Option<Continuation>,
}

fn lock<T>(m: &'static OnceLock<Mutex<T>>, init: fn() -> T) -> MutexGuard<'static, T> {
    m.get_or_init(|| Mutex::new(init()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn cache() -> MutexGuard<'static, HashMap<String, CachedConnection>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedConnection>>> = OnceLock::new();
    lock(&CACHE, HashMap::new)
}

fn debug_stats() -> MutexGuard<'static, HashMap<String, OpenAICodexWebSocketDebugStats>> {
    static STATS: OnceLock<Mutex<HashMap<String, OpenAICodexWebSocketDebugStats>>> =
        OnceLock::new();
    lock(&STATS, HashMap::new)
}

fn sse_fallback_sessions() -> MutexGuard<'static, HashSet<String>> {
    static FALLBACK: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    lock(&FALLBACK, HashSet::new)
}

fn next_connection_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn with_stats<R>(session_id: &str, f: impl FnOnce(&mut OpenAICodexWebSocketDebugStats) -> R) -> R {
    let mut stats = debug_stats();
    f(stats.entry(session_id.to_string()).or_default())
}

/// `getOpenAICodexWebSocketDebugStats`.
pub fn get_openai_codex_websocket_debug_stats(
    session_id: &str,
) -> Option<OpenAICodexWebSocketDebugStats> {
    debug_stats().get(session_id).cloned()
}

/// `resetOpenAICodexWebSocketDebugStats`: one session, or all.
pub fn reset_openai_codex_websocket_debug_stats(session_id: Option<&str>) {
    match session_id {
        Some(id) => {
            debug_stats().remove(id);
            sse_fallback_sessions().remove(id);
        }
        None => {
            debug_stats().clear();
            sse_fallback_sessions().clear();
        }
    }
}

/// `closeOpenAICodexWebSocketSessions`: one session's cached connection, or
/// all of them (the session-resource cleanup hook).
pub fn close_openai_codex_websocket_sessions(session_id: Option<&str>) {
    let entries: Vec<CachedConnection> = {
        let mut cache = cache();
        match session_id {
            Some(id) => cache.remove(id).into_iter().collect(),
            None => cache.drain().map(|(_, e)| e).collect(),
        }
    };
    for mut entry in entries {
        if let Some(timer) = entry.idle_timer.take() {
            timer.abort();
        }
        if let Some(socket) = entry.socket.take() {
            close_silently(socket, 1000, "debug_close");
        }
    }
}

pub(crate) fn is_sse_fallback_active(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|id| sse_fallback_sessions().contains(id))
}

/// `recordWebSocketSseFallback`.
pub(crate) fn record_sse_fallback(session_id: Option<&str>) {
    let Some(id) = session_id else { return };
    let active = is_sse_fallback_active(Some(id));
    with_stats(id, |stats| {
        stats.sse_fallbacks += 1;
        stats.websocket_fallback_active = Some(active);
    });
}

/// `recordWebSocketFailure`.
pub(crate) fn record_websocket_failure(session_id: Option<&str>, error: &CodexError) {
    let Some(id) = session_id else { return };
    sse_fallback_sessions().insert(id.to_string());
    with_stats(id, |stats| {
        stats.websocket_failures += 1;
        stats.last_websocket_error = Some(error.message());
        stats.websocket_fallback_active = Some(true);
    });
}

/// `closeWebSocketSilently`: a close frame when a runtime is at hand, else
/// just drop the connection.
fn close_silently(mut socket: Socket, code: u16, reason: &'static str) {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            let frame = CloseFrame {
                code: CloseCode::from(code),
                reason: reason.into(),
            };
            let _ = socket.close(Some(frame)).await;
        });
    }
}

/// `isWebSocketReusable` (`readyState === OPEN`): nothing but pings has
/// arrived on the idle connection and it has not been closed.
fn is_reusable(socket: &mut Socket) -> bool {
    loop {
        match socket.next().now_or_never() {
            None => return true,
            Some(Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))) => continue,
            Some(_) => return false,
        }
    }
}

/// `scheduleSessionWebSocketExpiry`.
fn schedule_expiry(session_id: &str, entry: &mut CachedConnection) {
    if let Some(timer) = entry.idle_timer.take() {
        timer.abort();
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let session_id = session_id.to_string();
    let id = entry.id;
    entry.idle_timer = Some(handle.spawn(async move {
        tokio::time::sleep(SESSION_WEBSOCKET_CACHE_TTL).await;
        let socket = {
            let mut cache = cache();
            match cache.get(&session_id) {
                Some(e) if e.id == id && !e.busy => {
                    cache.remove(&session_id).and_then(|e| e.socket)
                }
                _ => None,
            }
        };
        if let Some(socket) = socket {
            close_silently(socket, 1000, "idle_timeout");
        }
    }));
}

/// `connectWebSocket`.
async fn connect(
    url: &str,
    headers: &[(String, String)],
    signal: Option<&AbortSignal>,
) -> Result<Socket, CodexError> {
    let mut request = url
        .into_client_request()
        .map_err(|e| CodexError::other(e.to_string()))?;
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| CodexError::other(e.to_string()))?;
        let value = HeaderValue::from_str(value).map_err(|e| CodexError::other(e.to_string()))?;
        request.headers_mut().insert(name, value);
    }
    let (socket, _) = abortable(signal, tokio_tungstenite::connect_async(request))
        .await?
        .map_err(websocket_error)?;
    Ok(socket)
}

/// `extractWebSocketError` / `extractWebSocketCloseError` for a failed read.
fn websocket_error(error: WsError) -> CodexError {
    match error {
        WsError::ConnectionClosed
        | WsError::AlreadyClosed
        | WsError::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => close_error(Some(1006), ""),
        other => {
            let message = other.to_string();
            CodexError::other(if message.is_empty() {
                "WebSocket error".to_string()
            } else {
                message
            })
        }
    }
}

/// `extractWebSocketCloseError`: `WebSocket closed <code> <reason>`.
fn close_error(code: Option<u16>, reason: &str) -> CodexError {
    let code_text = code.map(|c| format!(" {c}")).unwrap_or_default();
    let mut reason_text = if reason.is_empty() {
        String::new()
    } else {
        format!(" {reason}")
    };
    if reason_text.is_empty() && code == Some(WEBSOCKET_MESSAGE_TOO_BIG_CLOSE_CODE) {
        reason_text = " message too big".to_string();
    }
    CodexError::Other(DiagnosticError {
        name: "WebSocketCloseError".to_string(),
        message: format!("WebSocket closed{code_text}{reason_text}")
            .trim()
            .to_string(),
        code: code.map(|c| json!(c)),
    })
}

/// Whose connection a request holds.
enum Lease {
    /// A one-off connection, closed after the request.
    Unshared,
    /// The session's cached connection (entry `id`).
    Cached { session_id: String, id: u64 },
}

/// `acquireWebSocket`.
async fn acquire(
    url: &str,
    headers: &[(String, String)],
    session_id: Option<&str>,
    signal: Option<&AbortSignal>,
) -> Result<(Socket, Lease, bool), CodexError> {
    let Some(session_id) = session_id else {
        return Ok((connect(url, headers, signal).await?, Lease::Unshared, false));
    };
    enum Plan {
        Reuse(Box<Socket>, u64),
        Separate,
        New,
    }
    let plan = {
        let mut cache = cache();
        match cache.get_mut(session_id) {
            Some(entry) => {
                if let Some(timer) = entry.idle_timer.take() {
                    timer.abort();
                }
                if entry.busy {
                    Plan::Separate
                } else {
                    let mut socket = entry.socket.take();
                    if socket.as_mut().is_some_and(is_reusable) {
                        entry.busy = true;
                        Plan::Reuse(Box::new(socket.expect("checked above")), entry.id)
                    } else {
                        if let Some(socket) = socket {
                            close_silently(socket, 1000, "done");
                        }
                        cache.remove(session_id);
                        Plan::New
                    }
                }
            }
            None => Plan::New,
        }
    };
    match plan {
        Plan::Reuse(socket, id) => Ok((
            *socket,
            Lease::Cached {
                session_id: session_id.to_string(),
                id,
            },
            true,
        )),
        Plan::Separate => Ok((connect(url, headers, signal).await?, Lease::Unshared, false)),
        Plan::New => {
            let socket = connect(url, headers, signal).await?;
            let id = next_connection_id();
            cache().insert(
                session_id.to_string(),
                CachedConnection {
                    id,
                    socket: None,
                    busy: true,
                    idle_timer: None,
                    continuation: None,
                },
            );
            Ok((
                socket,
                Lease::Cached {
                    session_id: session_id.to_string(),
                    id,
                },
                false,
            ))
        }
    }
}

/// `release({ keep })`: a kept session connection goes back to the cache
/// with a fresh idle timer; anything else is closed.
fn release(socket: Socket, lease: &Lease, keep: bool) {
    let Lease::Cached { session_id, id } = lease else {
        close_silently(socket, 1000, "done");
        return;
    };
    let mut cache = cache();
    match cache.get_mut(session_id).filter(|e| e.id == *id) {
        Some(entry) if keep => {
            entry.socket = Some(socket);
            entry.busy = false;
            schedule_expiry(session_id, entry);
        }
        Some(_) => {
            if let Some(mut entry) = cache.remove(session_id) {
                if let Some(timer) = entry.idle_timer.take() {
                    timer.abort();
                }
            }
            close_silently(socket, 1000, "done");
        }
        None => close_silently(socket, 1000, "done"),
    }
}

/// `requestBodyWithoutInput`.
fn without_input(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(obj) = body.as_object_mut() {
        obj.remove("input");
        obj.remove("previous_response_id");
    }
    body
}

fn input_of(body: &Value) -> &[Value] {
    body["input"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// `getCachedWebSocketInputDelta`: the input after the previous request's
/// input plus its response items, when the rest of the body is unchanged.
fn input_delta(body: &Value, continuation: &Continuation) -> Option<Vec<Value>> {
    if without_input(body) != without_input(&continuation.last_request_body) {
        return None;
    }
    let current = input_of(body);
    let baseline: Vec<&Value> = input_of(&continuation.last_request_body)
        .iter()
        .chain(continuation.last_response_items.iter())
        .collect();
    if current.len() < baseline.len() {
        return None;
    }
    if !current.iter().zip(baseline.iter()).all(|(a, b)| a == *b) {
        return None;
    }
    Some(current[baseline.len()..].to_vec())
}

/// `buildCachedWebSocketRequestBody`.
fn build_cached_request_body(session_id: &str, id: u64, body: &Value) -> Value {
    let mut cache = cache();
    let Some(entry) = cache.get_mut(session_id).filter(|e| e.id == id) else {
        return body.clone();
    };
    let Some(continuation) = entry.continuation.as_ref() else {
        return body.clone();
    };
    match input_delta(body, continuation) {
        Some(delta) if !continuation.last_response_id.is_empty() => {
            let mut next = body.clone();
            next["previous_response_id"] = json!(continuation.last_response_id);
            next["input"] = Value::Array(delta);
            next
        }
        _ => {
            entry.continuation = None;
            body.clone()
        }
    }
}

fn set_continuation(session_id: &str, id: u64, continuation: Option<Continuation>) {
    if let Some(entry) = cache().get_mut(session_id).filter(|e| e.id == id) {
        entry.continuation = continuation;
    }
}

/// `processWebSocketStream`. `started` turns true with the first event
/// (when the `start` event is pushed).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_websocket_stream(
    url: &str,
    body: &Value,
    headers: &[(String, String)],
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
    model: &Model,
    started: &mut bool,
    options: &CodexOptions,
) -> Result<(), CodexError> {
    let signal = options.signal.as_ref();
    let (mut socket, lease, reused) =
        acquire(url, headers, options.session_id.as_deref(), signal).await?;
    let use_cached_context = matches!(
        options.transport.unwrap_or_default(),
        Transport::WebSocketCached | Transport::Auto
    );
    // ChatGPT Codex Responses rejects `store: true`; continuation works via
    // the connection-scoped previous_response_id state instead.
    let request_body = match (&lease, use_cached_context) {
        (Lease::Cached { session_id, id }, true) => {
            build_cached_request_body(session_id, *id, body)
        }
        _ => body.clone(),
    };
    if let Some(session_id) = options.session_id.as_deref() {
        with_stats(session_id, |stats| {
            stats.requests += 1;
            if reused {
                stats.connections_reused += 1;
            } else {
                stats.connections_created += 1;
            }
            if use_cached_context {
                stats.cached_context_requests += 1;
            }
            if request_body["store"] == json!(true) {
                stats.store_true_requests += 1;
            }
            let items = input_of(&request_body).len() as u64;
            stats.last_input_items = items;
            match request_body["previous_response_id"].as_str() {
                Some(previous) if !previous.is_empty() => {
                    stats.delta_requests += 1;
                    stats.last_delta_input_items = Some(items);
                    stats.last_previous_response_id = Some(previous.to_string());
                }
                _ => {
                    stats.full_context_requests += 1;
                    stats.last_delta_input_items = None;
                    stats.last_previous_response_id = None;
                }
            }
        });
    }

    let result = exchange(
        &mut socket,
        &request_body,
        state,
        sender,
        model,
        started,
        options,
    )
    .await;
    let cached = match &lease {
        Lease::Cached { session_id, id } => Some((session_id.as_str(), *id)),
        Lease::Unshared => None,
    };
    let keep = match &result {
        Ok(()) if signal.is_some_and(AbortSignal::aborted) => false,
        Ok(()) => {
            if let (true, Some((session_id, id)), Some(response_id)) =
                (use_cached_context, cached, state.output.response_id.clone())
            {
                let allowed: HashSet<&str> = crate::CODEX_TOOL_CALL_PROVIDERS.into_iter().collect();
                let context = Context::new(
                    String::new(),
                    vec![Message::Assistant(state.output.clone())],
                    vec![],
                );
                let items = convert_responses_messages(model, &context, &allowed, false)
                    .into_iter()
                    .filter(|item| item["type"] != "function_call_output")
                    .collect();
                set_continuation(
                    session_id,
                    id,
                    Some(Continuation {
                        last_request_body: body.clone(),
                        last_response_id: response_id,
                        last_response_items: items,
                    }),
                );
            }
            true
        }
        Err(_) => {
            if let Some((session_id, id)) = cached {
                set_continuation(session_id, id, None);
            }
            false
        }
    };
    release(socket, &lease, keep);
    result
}

/// Send `response.create` and process events until the terminal one
/// (`parseWebSocket` + `mapCodexEvents` + `startWebSocketOutputOnFirstEvent`
/// + `processResponsesStream`).
async fn exchange(
    socket: &mut Socket,
    request_body: &Value,
    state: &mut ResponsesStreamState,
    sender: &AssistantMessageEventStream,
    model: &Model,
    started: &mut bool,
    options: &CodexOptions,
) -> Result<(), CodexError> {
    let signal = options.signal.as_ref();
    let mut message = serde_json::Map::new();
    message.insert("type".into(), json!("response.create"));
    if let Some(body) = request_body.as_object() {
        for (k, v) in body {
            message.insert(k.clone(), v.clone());
        }
    }
    let text = Value::Object(message).to_string();
    abortable(signal, socket.send(WsMessage::Text(text.into())))
        .await?
        .map_err(websocket_error)?;

    let stream_options = stream_options(options);
    loop {
        if signal.is_some_and(AbortSignal::aborted) {
            return Err(CodexError::Aborted);
        }
        let text = match abortable(signal, socket.next()).await? {
            None => return Err(close_error(Some(1006), "")),
            Some(Err(e)) => return Err(websocket_error(e)),
            Some(Ok(WsMessage::Text(t))) => t.to_string(),
            Some(Ok(WsMessage::Binary(b))) => String::from_utf8_lossy(&b).into_owned(),
            Some(Ok(WsMessage::Close(frame))) => {
                return Err(match frame {
                    Some(f) => close_error(Some(u16::from(f.code)), f.reason.as_str()),
                    None => close_error(Some(1005), ""),
                })
            }
            Some(Ok(_)) => continue,
        };
        if text.is_empty() {
            continue;
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|e| CodexError::Protocol(format!("Invalid Codex WebSocket JSON: {e}")))?;
        let Some(event) = map_codex_event(parsed)? else {
            continue;
        };
        if !*started {
            *started = true;
            sender.push(AssistantMessageEvent::Start {
                partial: state.output.clone(),
            });
        }
        let terminal = event["type"] == "response.completed";
        state
            .handle_event(&event, model, &stream_options, sender)
            .map_err(CodexError::Api)?;
        if terminal {
            return Ok(());
        }
    }
}

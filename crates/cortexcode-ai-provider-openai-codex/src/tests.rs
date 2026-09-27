//! Port of hoocode `packages/ai/test/openai-codex-stream.test.ts` (v0.5.89)
//! against a local HTTP / WebSocket server instead of stubbed `fetch` and
//! `WebSocket` globals, plus unit tests of the request/error helpers and the
//! live `openai-codex-cache-affinity-e2e.test.ts` (`#[ignore]`d).

use super::*;
use cortexcode_ai_types::Tool;
use cortexcode_ai_types::{
    AssistantMessage, Content, Message, TextContent, ThinkingLevel, UserMessage,
};
use futures_util::SinkExt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn mock_token() -> String {
    let payload = base64::engine::general_purpose::STANDARD.encode(
        json!({"https://api.openai.com/auth": {"chatgpt_account_id": "acc_test"}}).to_string(),
    );
    format!("aaa.{payload}.bbb")
}

fn model(id: &str, base_url: &str) -> Model {
    serde_json::from_value(json!({
        "id": id,
        "name": id,
        "api": "openai-codex-responses",
        "provider": "openai-codex",
        "baseUrl": base_url,
        "reasoning": true,
        "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 400000,
        "maxTokens": 128000,
    }))
    .unwrap()
}

fn user(text: &str, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: vec![Content::Text(TextContent::new(text))].into(),
        timestamp,
    })
}

fn say_hello() -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![user("Say hello", 1)],
        vec![],
    )
}

fn events(status: &str, service_tier: Option<&str>, usage_tokens: u64) -> Vec<Value> {
    let terminal = if status == "incomplete" {
        "response.incomplete"
    } else {
        "response.completed"
    };
    let mut response = json!({
        "status": status,
        "incomplete_details": if status == "incomplete" { json!({"reason": "max_output_tokens"}) } else { Value::Null },
        "usage": {
            "input_tokens": usage_tokens,
            "output_tokens": usage_tokens,
            "total_tokens": usage_tokens * 2,
            "input_tokens_details": {"cached_tokens": 0},
        },
    });
    if let Some(tier) = service_tier {
        response["service_tier"] = json!(tier);
    }
    vec![
        json!({"type": "response.output_item.added", "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "delta": "Hello"}),
        json!({"type": "response.output_item.done", "item": {"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "Hello"}]}}),
        json!({"type": terminal, "response": response}),
    ]
}

fn sse(events: &[Value], include_done: bool) -> String {
    let mut chunks: Vec<String> = events.iter().map(|e| format!("data: {e}")).collect();
    if include_done {
        chunks.push("data: [DONE]".into());
    }
    format!("{}\n\n", chunks.join("\n\n"))
}

// -----------------------------------------------------------------------------
// Mock HTTP server
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Recorded {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

struct Reply {
    status: &'static str,
    content_type: &'static str,
    body: String,
    /// Leave the connection open after the body (a stream that never ends).
    hold_open: bool,
}

fn sse_reply(body: String, hold_open: bool) -> Reply {
    Reply {
        status: "200 OK",
        content_type: "text/event-stream",
        body,
        hold_open,
    }
}

type Requests = Arc<Mutex<Vec<Recorded>>>;
type HandshakeHeaders = Arc<Mutex<Vec<Vec<(String, String)>>>>;

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Recorded> {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while data.len() < header_end + length {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    Some(Recorded {
        path,
        headers,
        body: String::from_utf8_lossy(&data[header_end..]).to_string(),
    })
}

/// Serve `replies` in order, one per connection; later requests get a 500.
async fn http_server(replies: Vec<Reply>) -> (String, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests: Requests = Arc::default();
    let recorded = requests.clone();
    tokio::spawn(async move {
        let mut replies = replies.into_iter();
        let mut held = Vec::new();
        while let Ok((mut stream, _)) = listener.accept().await {
            let Some(request) = read_request(&mut stream).await else {
                continue;
            };
            recorded.lock().unwrap().push(request);
            let reply = replies.next().unwrap_or(Reply {
                status: "500 Internal Server Error",
                content_type: "text/plain",
                body: "unexpected fetch".into(),
                hold_open: false,
            });
            let head = if reply.hold_open {
                format!(
                    "HTTP/1.1 {}\r\ncontent-type: {}\r\n\r\n",
                    reply.status, reply.content_type
                )
            } else {
                format!(
                    "HTTP/1.1 {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.status,
                    reply.content_type,
                    reply.body.len()
                )
            };
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(reply.body.as_bytes()).await;
            let _ = stream.flush().await;
            if reply.hold_open {
                held.push(stream);
            }
        }
    });
    (base, requests)
}

// -----------------------------------------------------------------------------
// Mock WebSocket server
// -----------------------------------------------------------------------------

/// A WebSocket server answering each `response.create` with the events
/// `respond(n)` returns for the n-th request (0-based, across connections).
/// Records the sent bodies and counts accepted connections.
struct WsServer {
    base_url: String,
    bodies: Arc<Mutex<Vec<Value>>>,
    connections: Arc<Mutex<usize>>,
    handshake_headers: HandshakeHeaders,
}

#[allow(clippy::result_large_err)] // the handshake callback's signature is tungstenite's
async fn ws_server(respond: fn(usize) -> Vec<Value>) -> WsServer {
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let connections: Arc<Mutex<usize>> = Arc::default();
    let handshake_headers: HandshakeHeaders = Arc::default();
    let (b, c, h) = (
        bodies.clone(),
        connections.clone(),
        handshake_headers.clone(),
    );
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (bodies, h) = (b.clone(), h.clone());
            *c.lock().unwrap() += 1;
            tokio::spawn(async move {
                let callback = |req: &Request, resp: Response| {
                    h.lock().unwrap().push(
                        req.headers()
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                            .collect(),
                    );
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    let WsMessage::Text(text) = msg else { continue };
                    let n = {
                        let mut bodies = bodies.lock().unwrap();
                        bodies.push(serde_json::from_str(text.as_str()).unwrap());
                        bodies.len() - 1
                    };
                    for event in respond(n) {
                        if ws
                            .send(WsMessage::Text(event.to_string().into()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    });
    WsServer {
        base_url,
        bodies,
        connections,
        handshake_headers,
    }
}

use tokio_tungstenite::tungstenite::Message as WsMessage;

fn text_of(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

fn sse_options(extra: CodexOptions) -> CodexOptions {
    CodexOptions {
        api_key: Some(mock_token()),
        transport: Some(Transport::Sse),
        ..extra
    }
}

// -----------------------------------------------------------------------------
// openai-codex streaming
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn streams_sse_responses_into_the_event_stream() {
    let (base, requests) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), false),
        false,
    )])
    .await;
    let token = mock_token();
    let mut stream = stream_codex_responses(
        model("gpt-5.1-codex", &base),
        say_hello(),
        sse_options(CodexOptions::default()),
    );
    let mut saw_text_delta = false;
    let mut saw_done = false;
    while let Some(event) = stream.next().await {
        match event {
            AssistantMessageEvent::TextDelta { .. } => saw_text_delta = true,
            AssistantMessageEvent::Done { message } => {
                saw_done = true;
                assert_eq!(text_of(&message), "Hello");
            }
            AssistantMessageEvent::Error { error } => panic!("{:?}", error.error_message),
            _ => {}
        }
    }
    assert!(saw_text_delta && saw_done);

    let request = requests.lock().unwrap().remove(0);
    assert_eq!(request.path, "/codex/responses");
    assert_eq!(
        request.header("authorization"),
        Some(format!("Bearer {token}").as_str())
    );
    assert_eq!(request.header("chatgpt-account-id"), Some("acc_test"));
    assert_eq!(
        request.header("openai-beta"),
        Some("responses=experimental")
    );
    assert_eq!(request.header("originator"), Some("pi"));
    assert_eq!(request.header("accept"), Some("text/event-stream"));
    assert!(request.header("x-api-key").is_none());
    assert!(request.header("user-agent").unwrap().starts_with("pi ("));
}

#[tokio::test(flavor = "multi_thread")]
async fn completes_after_response_completed_even_when_the_sse_body_stays_open() {
    let (base, _) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), true),
        true,
    )])
    .await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_codex_responses(
            model("gpt-5.1-codex", &base),
            say_hello(),
            sse_options(CodexOptions::default()),
        )
        .result(),
    )
    .await
    .expect("Timed out waiting for completed SSE stream");
    assert_eq!(text_of(&result), "Hello");
    assert_eq!(result.stop_reason, StopReason::Stop);
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_response_incomplete_to_length_even_when_the_sse_body_stays_open() {
    let (base, _) = http_server(vec![sse_reply(
        sse(&events("incomplete", None, 5), false),
        true,
    )])
    .await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_codex_responses(
            model("gpt-5.1-codex", &base),
            say_hello(),
            sse_options(CodexOptions::default()),
        )
        .result(),
    )
    .await
    .expect("Timed out waiting for incomplete SSE stream");
    assert_eq!(text_of(&result), "Hello");
    assert_eq!(result.stop_reason, StopReason::Length);
}

#[tokio::test(flavor = "multi_thread")]
async fn sets_session_headers_and_prompt_cache_key_when_session_id_is_provided() {
    let (base, requests) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), false),
        false,
    )])
    .await;
    let session_id = "test-session-123";
    stream_codex_responses(
        model("gpt-5.1-codex", &base),
        say_hello(),
        sse_options(CodexOptions {
            session_id: Some(session_id.into()),
            ..Default::default()
        }),
    )
    .result()
    .await;
    let request = requests.lock().unwrap().remove(0);
    assert_eq!(request.header("session_id"), Some(session_id));
    assert_eq!(request.header("x-client-request-id"), Some(session_id));
    assert_eq!(request.json()["prompt_cache_key"], session_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_set_session_headers_when_session_id_is_not_provided() {
    let (base, requests) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), false),
        false,
    )])
    .await;
    stream_codex_responses(
        model("gpt-5.1-codex", &base),
        say_hello(),
        sse_options(CodexOptions::default()),
    )
    .result()
    .await;
    let request = requests.lock().unwrap().remove(0);
    assert!(request.header("session_id").is_none());
    assert!(request.header("x-client-request-id").is_none());
    assert!(request.json().get("prompt_cache_key").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn preserves_gpt_5_5_xhigh_reasoning_effort_from_simple_options() {
    let (base, requests) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), false),
        false,
    )])
    .await;
    let mut m = model("gpt-5.5", &base);
    m.thinking_level_map = Some(
        [("xhigh".to_string(), json!("xhigh"))]
            .into_iter()
            .collect(),
    );
    stream(
        m,
        say_hello(),
        SimpleStreamOptions {
            api_key: Some(mock_token()),
            reasoning: Some(ThinkingLevel::XHigh),
            transport: Some(Transport::Sse),
            ..Default::default()
        },
    )
    .unwrap()
    .result()
    .await;
    let body = requests.lock().unwrap().remove(0).json();
    assert_eq!(
        body["reasoning"],
        json!({"effort": "xhigh", "summary": "auto"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn clamps_minimal_reasoning_effort_to_low() {
    for id in ["gpt-5.3-codex", "gpt-5.4", "gpt-5.5"] {
        let (base, requests) = http_server(vec![sse_reply(
            sse(&events("completed", None, 5), false),
            false,
        )])
        .await;
        let mut m = model(id, &base);
        m.thinking_level_map = Some(
            [
                ("xhigh".to_string(), json!("xhigh")),
                ("minimal".to_string(), json!("low")),
            ]
            .into_iter()
            .collect(),
        );
        stream_codex_responses(
            m,
            say_hello(),
            sse_options(CodexOptions {
                reasoning_effort: Some("minimal".into()),
                ..Default::default()
            }),
        )
        .result()
        .await;
        let body = requests.lock().unwrap().remove(0).json();
        assert_eq!(
            body["reasoning"],
            json!({"effort": "low", "summary": "auto"}),
            "{id}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uses_the_client_sent_service_tier_when_codex_echoes_default() {
    for (id, tier, multiplier) in [
        ("gpt-5.1-codex", "flex", 0.5),
        ("gpt-5.1-codex", "priority", 2.0),
        ("gpt-5.5", "flex", 0.5),
        ("gpt-5.5", "priority", 2.5),
    ] {
        let (base, _) = http_server(vec![sse_reply(
            sse(&events("completed", Some("default"), 1_000_000), false),
            false,
        )])
        .await;
        let mut m = model(id, &base);
        m.cost.input = 1.0;
        m.cost.output = 2.0;
        let result = stream_codex_responses(
            m,
            say_hello(),
            sse_options(CodexOptions {
                service_tier: Some(tier.into()),
                ..Default::default()
            }),
        )
        .result()
        .await;
        assert_eq!(result.usage.cost.input, multiplier, "{id} {tier}");
        assert_eq!(result.usage.cost.output, 2.0 * multiplier, "{id} {tier}");
        assert_eq!(result.usage.cost.total, 3.0 * multiplier, "{id} {tier}");
    }
}

fn ws_hello(_: usize) -> Vec<Value> {
    events("completed", None, 5)
}

#[tokio::test(flavor = "multi_thread")]
async fn forwards_auto_transport_from_simple_options_and_uses_cached_websocket_context() {
    let server = ws_server(ws_hello).await;
    let session = "session-auto-rs";
    reset_openai_codex_websocket_debug_stats(Some(session));
    let result = stream(
        model("gpt-5.1-codex", &server.base_url),
        say_hello(),
        SimpleStreamOptions {
            api_key: Some(mock_token()),
            session_id: Some(session.into()),
            transport: Some(Transport::Auto),
            ..Default::default()
        },
    )
    .unwrap()
    .result()
    .await;
    assert_eq!(result.error_message, None);
    assert_eq!(text_of(&result), "Hello");
    // One WebSocket request; the HTTP fallback never ran (the server only
    // speaks WebSocket).
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    let stats = get_openai_codex_websocket_debug_stats(session).unwrap();
    assert_eq!(stats.cached_context_requests, 1);
    assert_eq!(stats.full_context_requests, 1);
    assert_eq!(stats.sse_fallbacks, 0);

    let headers = server.handshake_headers.lock().unwrap()[0].clone();
    let header = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        header("openai-beta").as_deref(),
        Some(websocket::OPENAI_BETA_RESPONSES_WEBSOCKETS)
    );
    assert_eq!(header("session_id").as_deref(), Some(session));
    assert_eq!(header("x-client-request-id").as_deref(), Some(session));
    assert_eq!(header("chatgpt-account-id").as_deref(), Some("acc_test"));
    assert!(header("accept").is_none_or(|v| v != "text/event-stream"));
    close_openai_codex_websocket_sessions(Some(session));
}

fn ws_two_turns(n: usize) -> Vec<Value> {
    let (response_id, message_id, text) =
        [("resp_1", "msg_1", "Hello"), ("resp_2", "msg_2", "Done")][n];
    vec![
        json!({"type": "response.created", "response": {"id": response_id}}),
        json!({"type": "response.output_item.added", "item": {"type": "message", "id": message_id, "role": "assistant", "status": "in_progress", "content": []}}),
        json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "delta": text}),
        json!({"type": "response.output_item.done", "item": {"type": "message", "id": message_id, "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": text}]}}),
        json!({"type": "response.completed", "response": {"id": response_id, "status": "completed", "usage": {"input_tokens": 5, "output_tokens": 3, "total_tokens": 8, "input_tokens_details": {"cached_tokens": 0}}}}),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn sends_only_response_input_deltas_in_websocket_cached_mode() {
    let server = ws_server(ws_two_turns).await;
    let session = "session-1-rs";
    reset_openai_codex_websocket_debug_stats(Some(session));
    let options = || CodexOptions {
        api_key: Some(mock_token()),
        session_id: Some(session.into()),
        transport: Some(Transport::WebSocketCached),
        ..Default::default()
    };
    let m = model("gpt-5.1-codex", &server.base_url);
    let first = stream_codex_responses(m.clone(), say_hello(), options())
        .result()
        .await;
    assert_eq!(first.error_message, None);
    let second_context = Context::new(
        "You are a helpful assistant.".into(),
        vec![
            user("Say hello", 1),
            Message::Assistant(first),
            user("Now finish", 2),
        ],
        vec![],
    );
    let second = stream_codex_responses(m, second_context, options())
        .result()
        .await;
    assert_eq!(second.error_message, None);
    assert_eq!(text_of(&second), "Done");

    let bodies = server.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["type"], "response.create");
    assert_eq!(bodies[0]["store"], false);
    assert!(bodies[0].get("previous_response_id").is_none());
    assert_eq!(
        bodies[0]["input"],
        json!([{"role": "user", "content": [{"type": "input_text", "text": "Say hello"}]}])
    );
    assert_eq!(bodies[1]["store"], false);
    assert_eq!(bodies[1]["previous_response_id"], "resp_1");
    assert_eq!(
        bodies[1]["input"],
        json!([{"role": "user", "content": [{"type": "input_text", "text": "Now finish"}]}])
    );
    assert_eq!(*server.connections.lock().unwrap(), 1);
    let stats = get_openai_codex_websocket_debug_stats(session).unwrap();
    assert_eq!(
        (
            stats.requests,
            stats.connections_created,
            stats.connections_reused,
            stats.cached_context_requests,
            stats.store_true_requests,
            stats.full_context_requests,
            stats.delta_requests,
        ),
        (2, 1, 1, 2, 0, 1, 1)
    );
    assert_eq!(stats.last_delta_input_items, Some(1));
    assert_eq!(stats.last_previous_response_id.as_deref(), Some("resp_1"));
    close_openai_codex_websocket_sessions(Some(session));
}

// -----------------------------------------------------------------------------
// Transport fallback and errors (behavior of the TS source, not in its tests)
// -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn websocket_failure_before_start_falls_back_to_sse_and_records_a_diagnostic() {
    // A plain HTTP server: the WebSocket handshake fails, SSE succeeds.
    let (base, requests) = http_server(vec![
        Reply {
            status: "404 Not Found",
            content_type: "text/plain",
            body: "no websockets".into(),
            hold_open: false,
        },
        sse_reply(sse(&events("completed", None, 5), false), false),
    ])
    .await;
    let session = "fallback-rs";
    reset_openai_codex_websocket_debug_stats(Some(session));
    let options = || CodexOptions {
        api_key: Some(mock_token()),
        session_id: Some(session.into()),
        ..Default::default()
    };
    let result = stream_codex_responses(model("gpt-5.1-codex", &base), say_hello(), options())
        .result()
        .await;
    assert_eq!(result.error_message, None);
    assert_eq!(text_of(&result), "Hello");
    let diagnostics = result.diagnostics.expect("transport failure diagnostic");
    assert_eq!(diagnostics.len(), 1);
    let d = &diagnostics[0];
    assert_eq!(d["type"], "provider_transport_failure");
    assert_eq!(d["details"]["configuredTransport"], "auto");
    assert_eq!(d["details"]["fallbackTransport"], "sse");
    assert_eq!(d["details"]["eventsEmitted"], false);
    assert_eq!(d["details"]["phase"], "before_message_stream_start");
    assert!(d["details"]["requestBytes"].as_u64().unwrap() > 0);
    let stats = get_openai_codex_websocket_debug_stats(session).unwrap();
    assert_eq!((stats.websocket_failures, stats.sse_fallbacks), (1, 1));
    assert_eq!(stats.websocket_fallback_active, Some(true));
    assert_eq!(requests.lock().unwrap()[1].path, "/codex/responses");

    // The session now goes straight to SSE.
    let (base, requests) = http_server(vec![sse_reply(
        sse(&events("completed", None, 5), false),
        false,
    )])
    .await;
    let result = stream_codex_responses(model("gpt-5.1-codex", &base), say_hello(), options())
        .result()
        .await;
    assert_eq!(result.error_message, None);
    assert!(result.diagnostics.is_none());
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        get_openai_codex_websocket_debug_stats(session)
            .unwrap()
            .sse_fallbacks,
        2
    );
    reset_openai_codex_websocket_debug_stats(Some(session));
}

fn ws_codex_error(_: usize) -> Vec<Value> {
    vec![
        json!({"type": "error", "code": "context_length_exceeded", "message": "Your input exceeds the context window"}),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_error_events_fail_without_falling_back() {
    let server = ws_server(ws_codex_error).await;
    let result = stream_codex_responses(
        model("gpt-5.1-codex", &server.base_url),
        say_hello(),
        CodexOptions {
            api_key: Some(mock_token()),
            transport: Some(Transport::WebSocket),
            ..Default::default()
        },
    )
    .result()
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Codex error: Your input exceeds the context window")
    );
    assert!(result.diagnostics.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn usage_limit_errors_are_not_retried_and_read_friendly() {
    let body =
        json!({"error": {"code": "usage_limit_reached", "plan_type": "PLUS", "message": "limit"}})
            .to_string();
    let (base, requests) = http_server(vec![Reply {
        status: "400 Bad Request",
        content_type: "application/json",
        body,
        hold_open: false,
    }])
    .await;
    let result = stream_codex_responses(
        model("gpt-5.1-codex", &base),
        say_hello(),
        sse_options(CodexOptions::default()),
    )
    .result()
    .await;
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("You have hit your ChatGPT usage limit (plus plan).")
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn retries_transient_http_errors() {
    let (base, requests) = http_server(vec![
        Reply {
            status: "503 Service Unavailable",
            content_type: "text/plain",
            body: "upstream connect error".into(),
            hold_open: false,
        },
        sse_reply(sse(&events("completed", None, 5), false), false),
    ])
    .await;
    let result = stream_codex_responses(
        model("gpt-5.1-codex", &base),
        say_hello(),
        sse_options(CodexOptions::default()),
    )
    .result()
    .await;
    assert_eq!(result.error_message, None);
    assert_eq!(text_of(&result), "Hello");
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_tokens_fail_with_the_ts_message() {
    let result = stream_codex_responses(
        model("gpt-5.1-codex", "http://127.0.0.1:9"),
        say_hello(),
        CodexOptions {
            api_key: Some("not-a-jwt".into()),
            ..Default::default()
        },
    )
    .result()
    .await;
    assert_eq!(
        result.error_message.as_deref(),
        Some("Failed to extract accountId from token")
    );
}

#[test]
fn stream_simple_requires_an_api_key() {
    let error = stream(
        model("gpt-5.1-codex", ""),
        say_hello(),
        SimpleStreamOptions::default(),
    )
    .err()
    .unwrap();
    assert_eq!(error.to_string(), "No API key for provider: openai-codex");
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

#[test]
fn resolves_codex_urls() {
    assert_eq!(
        resolve_codex_url(""),
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x/api/"),
        "https://x/api/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x/codex"),
        "https://x/codex/responses"
    );
    assert_eq!(
        resolve_codex_url("https://x/codex/responses//"),
        "https://x/codex/responses"
    );
    assert_eq!(
        resolve_codex_websocket_url("https://chatgpt.com/backend-api"),
        "wss://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(
        resolve_codex_websocket_url("http://127.0.0.1:1"),
        "ws://127.0.0.1:1/codex/responses"
    );
}

#[test]
fn request_body_matches_build_request_body() {
    let mut context = say_hello();
    context.system_prompt = String::new();
    let body = build_request_body(
        &model("gpt-5.1-codex", ""),
        &context,
        &CodexOptions {
            temperature: Some(0.5),
            reasoning_effort: Some("none".into()),
            ..Default::default()
        },
    );
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "model",
            "store",
            "stream",
            "instructions",
            "input",
            "text",
            "include",
            "tool_choice",
            "parallel_tool_calls",
            "temperature",
            "reasoning"
        ]
    );
    assert_eq!(body["instructions"], "You are a helpful assistant.");
    assert_eq!(body["text"], json!({"verbosity": "low"}));
    assert_eq!(
        body["reasoning"],
        json!({"effort": "none", "summary": "auto"})
    );
}

#[test]
fn tools_are_sent_with_strict_null() {
    let mut context = say_hello();
    context.tools = vec![Tool {
        name: "read".into(),
        description: "Read a file".into(),
        parameters: serde_json::from_value(json!({"type": "object", "properties": {}})).unwrap(),
        defer_loading: None,
    }];
    let body = build_request_body(
        &model("gpt-5.1-codex", ""),
        &context,
        &CodexOptions::default(),
    );
    assert_eq!(body["tools"][0]["strict"], Value::Null);
    assert_eq!(body["tools"][0]["type"], "function");
}

#[test]
fn parses_codex_error_responses() {
    let now = 1_000_000_000;
    let (message, friendly) = parse_error_response(
        429,
        "Too Many Requests",
        &json!({"error": {"type": "x", "resets_at": (now + 5 * 60_000) / 1000}}).to_string(),
        now,
    );
    assert_eq!(
        friendly.as_deref(),
        Some("You have hit your ChatGPT usage limit. Try again in ~5 min.")
    );
    assert_eq!(message, friendly.unwrap());
    assert_eq!(
        parse_error_response(500, "Internal Server Error", "", now),
        ("Internal Server Error".to_string(), None)
    );
    assert_eq!(
        parse_error_response(400, "Bad Request", r#"{"error":{"message":"bad"}}"#, now),
        ("bad".to_string(), None)
    );
}

#[test]
fn retryable_errors_follow_the_ts_regex() {
    assert!(is_retryable_error(429, ""));
    assert!(is_retryable_error(400, "Rate limit reached"));
    assert!(is_retryable_error(400, "rate_limit"));
    assert!(is_retryable_error(400, "ratelimit"));
    assert!(is_retryable_error(400, "Service Unavailable"));
    assert!(!is_retryable_error(400, "rate  limit"));
    assert!(!is_retryable_error(400, "bad request"));
}

#[test]
fn maps_codex_events() {
    assert_eq!(map_codex_event(json!({"delta": "x"})).unwrap(), None);
    assert_eq!(
        map_codex_event(json!({"type": "error", "code": "c"})),
        Err(CodexError::Api("Codex error: c".into()))
    );
    assert_eq!(
        map_codex_event(json!({"type": "error"})),
        Err(CodexError::Api(r#"Codex error: {"type":"error"}"#.into()))
    );
    assert_eq!(
        map_codex_event(json!({"type": "response.failed", "response": {}})),
        Err(CodexError::Api("Codex response failed".into()))
    );
    assert_eq!(
        map_codex_event(
            json!({"type": "response.done", "response": {"status": "weird", "id": "r"}})
        )
        .unwrap(),
        Some(json!({"type": "response.completed", "response": {"id": "r"}}))
    );
    assert!(matches!(
        parse_sse_chunk("data: {nope"),
        Err(CodexError::Protocol(m)) if m.starts_with("Invalid Codex SSE JSON: ")
    ));
    assert_eq!(parse_sse_chunk("event: x\ndata: [DONE]").unwrap(), None);
    assert_eq!(
        parse_sse_chunk("data: {\"type\":\"a\"}").unwrap(),
        Some(json!({"type": "a"}))
    );
}

#[test]
fn websocket_headers_drop_the_sse_ones() {
    let headers = build_websocket_headers(None, None, "acc", "tok", "req-1");
    let get = |n: &str| {
        headers
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("openai-beta"), Some("responses_websockets=2026-02-06"));
    assert_eq!(get("session_id"), Some("req-1"));
    assert_eq!(get("x-client-request-id"), Some("req-1"));
    assert_eq!(get("accept"), None);
    assert_eq!(get("content-type"), None);
    assert_eq!(get("authorization"), Some("Bearer tok"));
}

/// `openai-codex-cache-affinity-e2e.test.ts`: needs a ChatGPT OAuth access
/// token in `OPENAI_CODEX_OAUTH_TOKEN`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: needs OPENAI_CODEX_OAUTH_TOKEN"]
async fn live_handles_sse_requests_with_aligned_cache_affinity_identifiers() {
    let Ok(token) = std::env::var("OPENAI_CODEX_OAUTH_TOKEN") else {
        return;
    };
    let model = cortexcode_ai_models::get_model("openai-codex", "gpt-5.3-codex")
        .unwrap()
        .clone();
    let context = Context::new(
        "You are a helpful assistant. Reply exactly as requested.".into(),
        vec![user("Reply with exactly: cache affinity e2e success", 0)],
        vec![],
    );
    let response = stream_codex_responses(
        model,
        context,
        CodexOptions {
            api_key: Some(token),
            session_id: Some("0195d6e4-4cf9-7f44-a2d8-f8f7f49ee9d3".into()),
            transport: Some(Transport::Sse),
            ..Default::default()
        },
    )
    .result()
    .await;
    assert_ne!(
        response.stop_reason,
        StopReason::Error,
        "{:?}",
        response.error_message
    );
    assert!(text_of(&response).contains("cache affinity e2e success"));
}

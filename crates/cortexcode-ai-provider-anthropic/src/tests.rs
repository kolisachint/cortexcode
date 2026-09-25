//! Stream tests against mock servers: ported from `anthropic-sse-parsing.test.ts`,
//! `anthropic-eager-tool-input-compat.test.ts` and `abort.test.ts` (hoocode
//! v0.5.89), plus the event mapping of `streamAnthropic`.

use super::*;
use crate::request::tests::{catalog, test_model as ts_model, user};
use cortexcode_ai_stream::testing::serve_script;
use cortexcode_ai_stream::testing::{serve_error, serve_sse, serve_sse_then_hang};
use cortexcode_ai_types::Tool;
use serde_json::json;
use std::time::Duration;

fn spawn_mock_server(sse_body: &'static str) -> String {
    serve_sse(sse_body)
}

fn spawn_mock_error_server(status_line: &'static str, body: &'static str) -> String {
    serve_error(status_line, body)
}

fn test_model(base_url: String) -> Model {
    Model {
        compat: None,
        id: "claude-test".into(),
        name: "Claude Test".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url,
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: cortexcode_ai_types::ModelCost::default(),
        context_window: 200_000,
        max_tokens: 4096,
        headers: None,
    }
}

fn collect(mut s: AssistantMessageEventStream) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::new();
    while let Some(e) = s.next_blocking() {
        events.push(e);
    }
    events
}

#[test]
fn test_stream_missing_credentials_errors_immediately() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("ANTHROPIC_OAUTH_TOKEN");
    let model = test_model("http://127.0.0.1:0".into());
    let context = Context::new("".into(), vec![], vec![]);
    let result = stream(model, context, SimpleStreamOptions::default());
    assert_eq!(
        result.err().unwrap().to_string(),
        "No API key for provider: anthropic"
    );
}

#[test]
fn test_stream_text_response() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\", world\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);

    assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
    let deltas: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, "Hello, world");

    match events.last().unwrap() {
        AssistantMessageEvent::Done { message } => {
            assert_eq!(message.stop_reason, StopReason::Stop);
            let usage = &message.usage;
            assert_eq!(usage.input, 10);
            assert_eq!(usage.output, 5);
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn test_stream_tool_call_response() {
    let sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);

    match events.last().unwrap() {
        AssistantMessageEvent::Done { message } => {
            assert_eq!(message.stop_reason, StopReason::ToolUse);
            assert_eq!(message.content.len(), 1);
            match &message.content[0] {
                Content::ToolCall(tc) => {
                    assert_eq!(tc.name, "read_file");
                    assert_eq!(tc.id, "call_1");
                    assert_eq!(tc.arguments["path"], "a.rs");
                }
                other => panic!("expected tool call content, got {other:?}"),
            }
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn test_stream_http_error_response() {
    let base_url = spawn_mock_error_server(
        "HTTP/1.1 429 Too Many Requests",
        "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"rate limited\"}}",
    );
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        max_retries: Some(0),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);
    match events.last().unwrap() {
        AssistantMessageEvent::Error { error } => {
            assert_eq!(error.stop_reason, StopReason::Error);
            assert_eq!(
                error.error_message.as_deref(),
                Some(
                    r#"429 {"type":"error","error":{"type":"rate_limit_error","message":"rate limited"}}"#
                )
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn api_error_message_matches_the_anthropic_sdk() {
    assert_eq!(
        api_error_message(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#
        ),
        r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#
    );
    assert_eq!(api_error_message(401, r#"{"message":"nope"}"#), "401 nope");
    assert_eq!(api_error_message(502, "upstream down"), "502 upstream down");
    assert_eq!(api_error_message(500, ""), "500 status code (no body)");
}

#[test]
fn test_stream_api_level_error_event() {
    let sse = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"overloaded\"}}\n\n",
    );
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);
    match events.last().unwrap() {
        AssistantMessageEvent::Error { error } => {
            // `throw new Error(sse.data)`: the raw event data.
            assert_eq!(
                error.error_message.as_deref(),
                Some(
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"overloaded"}}"#
                )
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

/// `testAbortSignal` in `abort.test.ts`, against a server that stalls
/// mid-message: aborting ends the stream with `aborted` and keeps what
/// was streamed so far.
#[test]
fn test_abort_mid_stream_keeps_partial_content() {
    let head = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"15 + 27 = 42. \"}}\n\n",
    );
    let base_url = serve_sse_then_hang(head, Duration::from_secs(30));
    let signal = AbortSignal::new();
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        signal: Some(signal.clone()),
        ..Default::default()
    };
    let mut s = stream(
        test_model(base_url),
        Context::new("".into(), vec![], vec![]),
        options,
    )
    .unwrap();

    let started = std::time::Instant::now();
    let mut text = String::new();
    while let Some(event) = s.next_blocking() {
        if let AssistantMessageEvent::TextDelta { delta, .. } = &event {
            text.push_str(delta);
            if text.len() >= 10 {
                signal.abort();
            }
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "abort must not wait for the server"
    );

    let msg = s.result_blocking();
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(msg.error_message.as_deref(), Some("Request was aborted"));
    assert_eq!(msg.usage.input, 10);
    match msg.content.as_slice() {
        [Content::Text(t)] => assert_eq!(t.text, "15 + 27 = 42. "),
        other => panic!("expected the partial text block, got {other:?}"),
    }
}

/// `testImmediateAbort` in `abort.test.ts`.
#[test]
fn test_immediate_abort() {
    let signal = AbortSignal::new();
    signal.abort();
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        signal: Some(signal),
        ..Default::default()
    };
    let s = stream(
        test_model("http://127.0.0.1:9".into()),
        Context::new("".into(), vec![], vec![]),
        options,
    )
    .unwrap();
    let msg = s.result_blocking();
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert!(msg.content.is_empty());
}

#[test]
fn test_map_stop_reason() {
    assert_eq!(map_stop_reason("end_turn"), Ok(StopReason::Stop));
    assert_eq!(map_stop_reason("tool_use"), Ok(StopReason::ToolUse));
    assert_eq!(map_stop_reason("max_tokens"), Ok(StopReason::Length));
    assert_eq!(map_stop_reason("refusal"), Ok(StopReason::Error));
    assert_eq!(map_stop_reason("pause_turn"), Ok(StopReason::Stop));
    assert_eq!(map_stop_reason("sensitive"), Ok(StopReason::Error));
    assert_eq!(
        map_stop_reason("weird"),
        Err("Unhandled stop reason: weird".to_string())
    );
}

// --- helpers for the TS ports ---

const OK: &str = "HTTP/1.1 200 OK";
const SSE: &str = "text/event-stream";

/// `createSseResponse`: `event: …\ndata: …\n` joined by blank lines.
fn sse_body(events: &[(&str, String)]) -> &'static str {
    let body = events
        .iter()
        .map(|(event, data)| format!("event: {event}\ndata: {data}\n"))
        .collect::<Vec<_>>()
        .join("\n");
    Box::leak(body.into_boxed_str())
}

fn usage_json(output: u64) -> Value {
    json!({"input_tokens": 12, "output_tokens": output, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0})
}

fn minimal_events() -> Vec<(&'static str, String)> {
    vec![
        ("message_start", json!({"type": "message_start", "message": {"id": "msg_test", "usage": usage_json(0)}}).to_string()),
        ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}).to_string()),
        ("content_block_delta", json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}).to_string()),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0}).to_string()),
        ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": usage_json(5)}).to_string()),
        ("message_stop", json!({"type": "message_stop"}).to_string()),
    ]
}

/// `streamAnthropic` against a scripted server with one SSE response.
fn run_script(
    model: Model,
    context: Context,
    events: &[(&str, String)],
) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let server = serve_script(vec![(OK, SSE, sse_body(events))]);
    let mut model = model;
    model.base_url = server.base_url.clone();
    let s = stream_anthropic(
        model,
        context,
        AnthropicOptions {
            api_key: Some("test-key".into()),
            ..Default::default()
        },
    );
    let result = s.result_blocking();
    (collect(s), result)
}

// --- anthropic-sse-parsing.test.ts ---

#[test]
fn repairs_malformed_sse_json_and_malformed_streamed_tool_json() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let context = Context::new(
        String::new(),
        vec![user("Use the edit tool.")],
        vec![Tool {
            name: "edit".into(),
            description: "Edit a file.".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}, "text": {"type": "string"}}, "required": ["path", "text"]}),
            defer_loading: None,
        }],
    );
    // An invalid `\H` escape and a raw tab inside the SSE data's JSON.
    let malformed = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"A\H\",\"text\":\"col1	col2\"}"}}"#;
    let events = vec![
        ("message_start", json!({"type": "message_start", "message": {"id": "msg_test", "usage": usage_json(0)}}).to_string()),
        ("content_block_start", json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_test", "name": "edit", "input": {}}}).to_string()),
        ("content_block_delta", malformed.to_string()),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 0}).to_string()),
        ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": usage_json(5)}).to_string()),
        ("message_stop", json!({"type": "message_stop"}).to_string()),
    ];
    let (_, result) = run_script(model, context, &events);
    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert_eq!(result.error_message, None);
    let Some(Content::ToolCall(call)) = result.content.first() else {
        panic!("expected a tool call, got {:?}", result.content);
    };
    assert_eq!(
        call.arguments,
        json!({"path": "A\\H", "text": "col1\tcol2"})
    );
}

#[test]
fn ignores_unknown_sse_events_after_message_stop() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let mut events = minimal_events();
    events.push(("done", "[DONE]".into()));
    events.push(("proxy.stats", "not json".into()));
    let (_, result) = run_script(
        model,
        Context::new(String::new(), vec![user("Say hello.")], vec![]),
        &events,
    );
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(result.error_message, None);
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{"type": "text", "text": "Hello"}])
    );
}

// --- anthropic-eager-tool-input-compat.test.ts ---

/// `captureAnthropicRequest`: headers and body of the one request sent.
fn capture_request(
    compat: Option<Value>,
    tools: Vec<Tool>,
) -> cortexcode_ai_stream::testing::Recorded {
    let server = serve_script(vec![(OK, SSE, "")]);
    let model = ts_model(&server.base_url, compat);
    let context = Context::new(String::new(), vec![user("Use the tool")], tools);
    let s = stream_anthropic(
        model,
        context,
        AnthropicOptions {
            api_key: Some("test-key".into()),
            cache_retention: Some(cortexcode_ai_types::CacheRetention::None),
            ..Default::default()
        },
    );
    s.result_blocking();
    server.requests().remove(0)
}

fn lookup_tool() -> Tool {
    Tool {
        name: "lookup".into(),
        description: "Look up a value".into(),
        parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]}),
        defer_loading: None,
    }
}

#[test]
fn sends_per_tool_eager_input_streaming_by_default() {
    let request = capture_request(None, vec![lookup_tool()]);
    assert_eq!(request.json()["tools"][0]["eager_input_streaming"], true);
    assert_eq!(request.header("anthropic-beta"), None);
}

#[test]
fn uses_the_fine_grained_beta_when_eager_tool_input_streaming_is_disabled() {
    let request = capture_request(
        Some(json!({"supportsEagerToolInputStreaming": false})),
        vec![lookup_tool()],
    );
    assert!(request.json()["tools"][0]
        .get("eager_input_streaming")
        .is_none());
    assert_eq!(
        request.header("anthropic-beta"),
        Some("fine-grained-tool-streaming-2025-05-14")
    );
}

#[test]
fn no_fine_grained_beta_without_tools() {
    let request = capture_request(
        Some(json!({"supportsEagerToolInputStreaming": false})),
        vec![],
    );
    assert!(request.json().get("tools").is_none());
    assert_eq!(request.header("anthropic-beta"), None);
}

#[test]
fn api_key_requests_carry_the_sdk_headers() {
    let request = capture_request(None, vec![]);
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header("x-api-key"), Some("test-key"));
    assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        request.header("anthropic-dangerous-direct-browser-access"),
        Some("true")
    );
}

// --- streamAnthropic event mapping ---

#[test]
fn maps_blocks_by_position_with_signatures_redaction_usage_and_cost() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let events = vec![
        ("message_start", json!({"type": "message_start", "message": {"id": "msg_1", "usage": {"input_tokens": 1000, "output_tokens": 1, "cache_read_input_tokens": 200}}}).to_string()),
        ("content_block_start", json!({"type": "content_block_start", "index": 3, "content_block": {"type": "thinking", "thinking": ""}}).to_string()),
        ("content_block_delta", json!({"type": "content_block_delta", "index": 3, "delta": {"type": "thinking_delta", "thinking": "hmm"}}).to_string()),
        ("content_block_delta", json!({"type": "content_block_delta", "index": 3, "delta": {"type": "signature_delta", "signature": "sig"}}).to_string()),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 3}).to_string()),
        ("content_block_start", json!({"type": "content_block_start", "index": 4, "content_block": {"type": "redacted_thinking", "data": "opaque"}}).to_string()),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 4}).to_string()),
        ("content_block_start", json!({"type": "content_block_start", "index": 5, "content_block": {"type": "text", "text": ""}}).to_string()),
        ("content_block_delta", json!({"type": "content_block_delta", "index": 5, "delta": {"type": "text_delta", "text": "hi"}}).to_string()),
        ("content_block_stop", json!({"type": "content_block_stop", "index": 5}).to_string()),
        ("message_delta", json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 50, "input_tokens": null}}).to_string()),
        ("message_stop", json!({"type": "message_stop"}).to_string()),
    ];
    let (events, result) = run_script(
        model.clone(),
        Context::new(String::new(), vec![user("hi")], vec![]),
        &events,
    );
    let indexes: Vec<(&str, usize)> = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ThinkingStart { index, .. } => Some(("thinking_start", *index)),
            AssistantMessageEvent::TextStart { index, .. } => Some(("text_start", *index)),
            AssistantMessageEvent::TextEnd { index, .. } => Some(("text_end", *index)),
            _ => None,
        })
        .collect();
    // contentIndex is the position in `content`, not Anthropic's block index.
    assert_eq!(
        indexes,
        [
            ("thinking_start", 0),
            ("thinking_start", 1),
            ("text_start", 2),
            ("text_end", 2)
        ]
    );
    assert_eq!(result.response_id.as_deref(), Some("msg_1"));
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([
            {"type": "thinking", "thinking": "hmm", "thinkingSignature": "sig"},
            {"type": "thinking", "thinking": "[Reasoning redacted]", "thinkingSignature": "opaque", "redacted": true},
            {"type": "text", "text": "hi"},
        ])
    );
    // input kept from message_start (null in message_delta), output replaced.
    assert_eq!(
        (
            result.usage.input,
            result.usage.output,
            result.usage.cache_read,
            result.usage.total_tokens
        ),
        (1000, 50, 200, 1250)
    );
    assert_eq!(
        result.usage.cost,
        cortexcode_ai_models::calculate_cost(&model, &result.usage)
    );
    assert!(result.usage.cost.total > 0.0);
}

#[test]
fn a_started_message_without_message_stop_is_an_error() {
    let mut events = minimal_events();
    events.pop();
    let (_, result) = run_script(
        test_model(String::new()),
        Context::new(String::new(), vec![user("hi")], vec![]),
        &events,
    );
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Anthropic stream ended before message_stop")
    );
    // What streamed so far is kept.
    assert_eq!(result.content.len(), 1);
}

#[test]
fn unknown_and_error_stop_reasons_end_in_error() {
    let with_reason = |reason: &str| {
        let mut events = minimal_events();
        events[4].1 = json!({"type": "message_delta", "delta": {"stop_reason": reason}, "usage": usage_json(5)}).to_string();
        run_script(
            test_model(String::new()),
            Context::new(String::new(), vec![user("hi")], vec![]),
            &events,
        )
        .1
    };
    let result = with_reason("weird");
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("Unhandled stop reason: weird")
    );
    let result = with_reason("refusal");
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(
        result.error_message.as_deref(),
        Some("An unknown error occurred")
    );
}

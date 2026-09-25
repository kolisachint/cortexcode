//! Stream tests against mock servers (the `streamGoogle` event loop, the
//! SDK's chunk decoding and errors), plus `abort.test.ts` cases.

use super::*;
use crate::request::tests::{clear_vertex_env, env_lock, restore_env};
use cortexcode_ai_stream::testing::serve_script;
use cortexcode_ai_stream::testing::{serve_sse, serve_sse_then_hang};
use cortexcode_ai_types::AbortSignal;
use serde_json::json;
use std::time::Duration;

fn spawn_mock_server(sse_body: &'static str) -> String {
    serve_sse(sse_body)
}

fn test_model(base_url: String) -> Model {
    Model {
        compat: None,
        id: "gemini-2.0-flash".into(),
        name: "Gemini Test".into(),
        api: "google-generative-ai".into(),
        provider: "google".into(),
        base_url,
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: cortexcode_ai_types::ModelCost::default(),
        context_window: 1_000_000,
        max_tokens: 8192,
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
    let _env = env_lock();
    let saved = std::env::var("GEMINI_API_KEY").ok();
    std::env::remove_var("GEMINI_API_KEY");
    let model = test_model("http://127.0.0.1:0".into());
    let context = Context::new("".into(), vec![], vec![]);
    let err = stream(model, context, SimpleStreamOptions::default()).err();
    if let Some(v) = saved {
        std::env::set_var("GEMINI_API_KEY", v);
    }
    assert_eq!(err.unwrap().to_string(), "No API key for provider: google");
}

#[test]
fn test_stream_text_response() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\", world\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"totalTokenCount\":15}}\n\n",
    );
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("key123".into()),
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
fn test_stream_thinking_then_text() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"let me think\",\"thought\":true}],\"role\":\"model\"}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n",
    );
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("key123".into()),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);
    match events.last().unwrap() {
        AssistantMessageEvent::Done { message } => {
            assert_eq!(message.content.len(), 2);
            match &message.content[0] {
                Content::Thinking(t) => assert_eq!(t.thinking, "let me think"),
                other => panic!("expected thinking first, got {other:?}"),
            }
            match &message.content[1] {
                Content::Text(t) => assert_eq!(t.text, "answer"),
                other => panic!("expected text second, got {other:?}"),
            }
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[test]
fn test_stream_tool_call_response() {
    let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a.rs\"}}}]},\"finishReason\":\"STOP\"}]}\n\n";
    let base_url = spawn_mock_server(sse);
    let model = test_model(base_url);
    let context = Context::new("".into(), vec![], vec![]);
    let options = SimpleStreamOptions {
        api_key: Some("key123".into()),
        ..Default::default()
    };

    let s = stream(model, context, options).expect("stream should start");
    let events = collect(s);
    match events.last().unwrap() {
        AssistantMessageEvent::Done { message } => {
            assert_eq!(message.stop_reason, StopReason::ToolUse);
            match &message.content[0] {
                Content::ToolCall(tc) => {
                    assert_eq!(tc.name, "read_file");
                    assert_eq!(tc.arguments["path"], "a.rs");
                }
                other => panic!("expected tool call, got {other:?}"),
            }
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

/// `testAbortSignal` in `abort.test.ts`, against a server that stalls
/// mid-message.
#[test]
fn test_abort_mid_stream_keeps_partial_content() {
    let head =
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"15 + 27 = 42. \"}]}}]}\n\n";
    let base_url = serve_sse_then_hang(head, Duration::from_secs(30));
    let signal = AbortSignal::new();
    let options = SimpleStreamOptions {
        api_key: Some("gkey".into()),
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
    while let Some(event) = s.next_blocking() {
        if let AssistantMessageEvent::TextDelta { .. } = &event {
            signal.abort();
        }
    }
    assert!(started.elapsed() < Duration::from_secs(10));

    let msg = s.result_blocking();
    assert_eq!(msg.stop_reason, StopReason::Aborted);
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
        api_key: Some("gkey".into()),
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
    // buildParams throws before any request.
    assert_eq!(msg.error_message.as_deref(), Some("Request aborted"));
}

#[test]
fn test_stream_vertex_without_project_is_an_error_event() {
    let _env = env_lock();
    let saved = clear_vertex_env();
    let model = test_model("http://127.0.0.1:0".into());
    let context = Context::new("".into(), vec![], vec![]);
    let s = stream_vertex(model, context, SimpleStreamOptions::default()).unwrap();
    let msg = s.result_blocking();
    restore_env(saved);
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(
        msg.error_message.as_deref(),
        Some("Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.")
    );
}

// --- request, chunk decoding and errors ---

const OK: &str = "HTTP/1.1 200 OK";
const SSE: &str = "text/event-stream";

fn options_with_key() -> SimpleStreamOptions {
    SimpleStreamOptions {
        api_key: Some("key123".into()),
        ..Default::default()
    }
}

#[test]
fn gemini_requests_hit_the_model_base_url_with_the_api_key_header() {
    let server = serve_script(vec![(OK, SSE, "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}]}\n\n")]);
    let mut model = test_model(format!("{}/v1beta", server.base_url));
    model.headers = Some([("X-Extra".to_string(), "1".to_string())].into());
    let context = Context::new("sys".into(), vec![], vec![]);
    let s = stream(model, context, options_with_key()).unwrap();
    assert_eq!(s.result_blocking().stop_reason, StopReason::Stop);

    let request = server.requests().remove(0);
    assert_eq!(
        request.path,
        "/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse"
    );
    assert_eq!(request.header("x-goog-api-key"), Some("key123"));
    assert_eq!(request.header("x-extra"), Some("1"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.json(),
        json!({
            "contents": [],
            "systemInstruction": {"parts": [{"text": "sys"}], "role": "user"},
            "generationConfig": {"maxOutputTokens": 8192},
        })
    );
}

#[test]
fn http_errors_carry_the_sdk_api_error_message() {
    let json_error = r#"{"error":{"code":400,"message":"bad","status":"INVALID_ARGUMENT"}}"#;
    let server = serve_script(vec![(
        "HTTP/1.1 400 Bad Request",
        "application/json",
        json_error,
    )]);
    let msg = stream(
        test_model(server.base_url.clone()),
        Context::new(String::new(), vec![], vec![]),
        options_with_key(),
    )
    .unwrap()
    .result_blocking();
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some(json_error));

    let server = serve_script(vec![(
        "HTTP/1.1 503 Service Unavailable",
        "text/plain",
        "down",
    )]);
    let msg = stream(
        test_model(server.base_url.clone()),
        Context::new(String::new(), vec![], vec![]),
        options_with_key(),
    )
    .unwrap()
    .result_blocking();
    assert_eq!(
        msg.error_message.as_deref(),
        Some(r#"{"error":{"message":"down","code":503,"status":"Service Unavailable"}}"#)
    );
}

#[test]
fn chunk_decoder_splits_on_every_sdk_delimiter_and_rejects_a_torn_tail() {
    let mut decoder = ChunkDecoder::default();
    let events = decoder
        .push(b"data: {\"a\":1}\r\n\r\ndata:{\"b\":2}\r\r: note\n\ndata: {\"c\"")
        .unwrap();
    assert_eq!(events, vec!["{\"a\":1}", "{\"b\":2}"]);
    assert_eq!(decoder.push(b":3}\n\n").unwrap(), vec!["{\"c\":3}"]);
    assert!(decoder.finish().is_ok());
    decoder.push(b"data: {\"d\"").unwrap();
    assert_eq!(
        decoder.finish(),
        Err("Incomplete JSON segment at the end".to_string())
    );
    // A chunk that is an error object is an ApiError.
    let mut decoder = ChunkDecoder::default();
    let err = decoder
        .push(br#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"slow down"}}"#)
        .unwrap_err();
    assert_eq!(
        err,
        r#"got status: RESOURCE_EXHAUSTED. {"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"slow down"}}"#
    );
}

#[test]
fn stream_keeps_signatures_ids_response_id_usage_and_cost() {
    let chunks = [
        json!({"responseId": "r1", "candidates": [{"content": {"parts": [
            {"text": "think", "thought": true, "thoughtSignature": "sig1"},
            {"text": " more", "thought": true},
            {"text": "answer", "thoughtSignature": "tsig"},
        ]}}]}),
        json!({"responseId": "r2", "candidates": [{"content": {"parts": [
            {"functionCall": {"id": "c1", "name": "read", "args": {"path": "a"}}, "thoughtSignature": "fsig"},
            {"functionCall": {"id": "c1", "name": "read"}},
        ]}, "finishReason": "STOP"}],
         "usageMetadata": {"promptTokenCount": 100, "cachedContentTokenCount": 40, "candidatesTokenCount": 7, "thoughtsTokenCount": 3, "totalTokenCount": 110}}),
    ];
    let body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    let server = serve_script(vec![(OK, SSE, Box::leak(body.into_boxed_str()))]);
    let mut model = test_model(server.base_url.clone());
    model.cost = cortexcode_ai_types::ModelCost {
        input: 1.0,
        output: 2.0,
        cache_read: 0.5,
        cache_write: 0.0,
    };
    let s = stream(
        model.clone(),
        Context::new(String::new(), vec![], vec![]),
        options_with_key(),
    )
    .unwrap();
    let msg = s.result_blocking();
    let events = collect(s);

    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.response_id.as_deref(), Some("r1"));
    let content = serde_json::to_value(&msg.content).unwrap();
    assert_eq!(
        content[0],
        json!({"type": "thinking", "thinking": "think more", "thinkingSignature": "sig1"})
    );
    assert_eq!(
        content[1],
        json!({"type": "text", "text": "answer", "textSignature": "tsig"})
    );
    assert_eq!(content[2]["id"], "c1");
    assert_eq!(content[2]["thoughtSignature"], "fsig");
    // A repeated id gets a generated `<name>_<ms>_<n>` one; missing args are {}.
    let second_id = content[3]["id"].as_str().unwrap();
    assert!(
        second_id.starts_with("read_") && second_id != "c1",
        "{second_id}"
    );
    assert_eq!(content[3]["arguments"], json!({}));
    assert_eq!(
        (
            msg.usage.input,
            msg.usage.output,
            msg.usage.cache_read,
            msg.usage.total_tokens
        ),
        (60, 10, 40, 110)
    );
    assert_eq!(
        msg.usage.cost,
        cortexcode_ai_models::calculate_cost(&model, &msg.usage)
    );
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            AssistantMessageEvent::Start { .. } => "start",
            AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
            AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
            AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
            AssistantMessageEvent::TextStart { .. } => "text_start",
            AssistantMessageEvent::TextDelta { .. } => "text_delta",
            AssistantMessageEvent::TextEnd { .. } => "text_end",
            AssistantMessageEvent::ToolCallStart { .. } => "toolcall_start",
            AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
            AssistantMessageEvent::ToolCallEnd { .. } => "toolcall_end",
            AssistantMessageEvent::Done { .. } => "done",
            AssistantMessageEvent::Error { .. } => "error",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "done",
        ]
    );
}

#[test]
fn error_finish_reasons_and_unknown_ones_end_in_error() {
    let run = |reason: &str| {
        let body = format!(
            "data: {}\n\n",
            json!({"candidates": [{"content": {"parts": [{"text": "x"}]}, "finishReason": reason}]})
        );
        let server = serve_script(vec![(OK, SSE, Box::leak(body.into_boxed_str()))]);
        stream(
            test_model(server.base_url.clone()),
            Context::new(String::new(), vec![], vec![]),
            options_with_key(),
        )
        .unwrap()
        .result_blocking()
    };
    let msg = run("SAFETY");
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(
        msg.error_message.as_deref(),
        Some("An unknown error occurred")
    );
    let msg = run("NEW_REASON");
    assert_eq!(
        msg.error_message.as_deref(),
        Some("Unhandled stop reason: NEW_REASON")
    );
    assert_eq!(run("MAX_TOKENS").stop_reason, StopReason::Length);
}

//! Stream tests against a recording mock server.
//!
//! Ported from the streaming cases of `openai-completions-tool-choice.test.ts`,
//! `-param-fallback.test.ts`, `-response-model.test.ts`,
//! `-thinking-as-text.test.ts` and `-prompt-suffix.test.ts`, plus
//! `abort.test.ts` (hoocode v0.5.89).

use super::*;
use crate::request::tests::{completions, env_lock, repro_context, repro_model, thinking, user};
use cortexcode_ai_stream::testing::{serve_script, serve_sse_then_hang, ScriptedServer};
use cortexcode_ai_types::Tool;
use serde_json::{json, Value};
use std::time::Duration;

const OK: &str = "HTTP/1.1 200 OK";
const SSE: &str = "text/event-stream";

fn sse(chunks: &[Value]) -> String {
    let mut body: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
    body.push_str("data: [DONE]\n\n");
    body
}

fn serve_chunks(chunks: &[Value]) -> ScriptedServer {
    let body: &'static str = Box::leak(sse(chunks).into_boxed_str());
    serve_script(vec![(OK, SSE, body)])
}

fn at(mut model: Model, server: &ScriptedServer) -> Model {
    model.base_url = server.base_url.clone();
    model
}

fn key() -> SimpleStreamOptions {
    SimpleStreamOptions {
        api_key: Some("test".into()),
        ..Default::default()
    }
}

fn collect(mut s: AssistantMessageEventStream) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut events = Vec::new();
    while let Some(e) = s.next_blocking() {
        events.push(e);
    }
    let message = s.result_blocking();
    (events, message)
}

fn run(model: Model, context: Context) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    collect(stream(model, context, key()).expect("stream starts"))
}

fn event_type(e: &AssistantMessageEvent) -> &'static str {
    match e {
        AssistantMessageEvent::Start { .. } => "start",
        AssistantMessageEvent::TextStart { .. } => "text_start",
        AssistantMessageEvent::TextDelta { .. } => "text_delta",
        AssistantMessageEvent::TextEnd { .. } => "text_end",
        AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        AssistantMessageEvent::ToolCallStart { .. } => "toolcall_start",
        AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
        AssistantMessageEvent::ToolCallEnd { .. } => "toolcall_end",
        AssistantMessageEvent::Done { .. } => "done",
        AssistantMessageEvent::Error { .. } => "error",
    }
}

fn tool_index(e: &AssistantMessageEvent) -> Option<usize> {
    match e {
        AssistantMessageEvent::ToolCallStart { index, .. }
        | AssistantMessageEvent::ToolCallDelta { index, .. }
        | AssistantMessageEvent::ToolCallEnd { index, .. } => Some(*index),
        _ => None,
    }
}

fn usage(prompt: u64, completion: u64) -> Value {
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "prompt_tokens_details": {"cached_tokens": 0},
        "completion_tokens_details": {"reasoning_tokens": 0}
    })
}

fn read_tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("{name} tool"),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    }
}

fn tool_call(content: &Content) -> &ToolCallContent {
    match content {
        Content::ToolCall(tc) => tc,
        other => panic!("expected toolCall, got {other:?}"),
    }
}

// --- openai-completions-tool-choice.test.ts (stream cases) ---

#[test]
fn maps_non_standard_finish_reason_to_error() {
    let server = serve_chunks(&[
        json!({"choices": [{"delta": {"content": "partial"}, "finish_reason": null}]}),
        json!({"choices": [{"delta": {}, "finish_reason": "network_error"}], "usage": usage(1, 1)}),
    ]);
    let model = at(
        cortexcode_ai_models::get_model("zai", "glm-5.2")
            .unwrap()
            .clone(),
        &server,
    );
    let (events, message) = run(model, Context::new(String::new(), vec![user("Hi")], vec![]));
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some("Provider finish_reason: network_error")
    );
    assert_eq!(event_type(events.last().unwrap()), "error");
}

#[test]
fn ignores_null_stream_chunks() {
    let server = serve_chunks(&[
        Value::Null,
        json!({"id": "chatcmpl-test", "choices": [{"delta": {"content": "OK"}, "finish_reason": null}]}),
        json!({"id": "chatcmpl-test", "choices": [{"delta": {}, "finish_reason": "stop"}], "usage": usage(3, 1)}),
    ]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let (_, message) = run(
        model,
        Context::new(String::new(), vec![user("Reply with exactly OK")], vec![]),
    );
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.error_message, None);
    assert_eq!(message.response_id.as_deref(), Some("chatcmpl-test"));
    assert_eq!(message.usage.total_tokens, 4);
    assert_eq!(message.content, vec![Content::Text(TextContent::new("OK"))]);
}

#[test]
fn coalesces_tool_call_deltas_by_stable_index_when_ids_change() {
    let call = |id: &str, name: Value, args: &str| json!({"index": 0, "id": id, "type": "function", "function": {"name": name, "arguments": args}});
    let server = serve_chunks(&[
        json!({"id": "c", "choices": [{"delta": {"tool_calls": [call("functions.read:0", json!("read"), "")]}, "finish_reason": null}]}),
        json!({"id": "c", "choices": [{"delta": {"tool_calls": [call("chatcmpl-tool-a", Value::Null, "{\"path\":\"README")]}, "finish_reason": null}]}),
        json!({"id": "c", "choices": [{"delta": {"tool_calls": [call("chatcmpl-tool-b", Value::Null, ".md\"}")]}, "finish_reason": "tool_calls"}], "usage": usage(10, 5)}),
    ]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let (events, message) = run(
        model,
        Context::new(
            String::new(),
            vec![user("Read README.md")],
            vec![read_tool("read")],
        ),
    );
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    let indexes: Vec<usize> = events.iter().filter_map(tool_index).collect();
    assert_eq!(indexes, [0, 0, 0, 0, 0]);
    assert_eq!(message.content.len(), 1);
    let tc = tool_call(&message.content[0]);
    assert_eq!(tc.id, "functions.read:0");
    assert_eq!(tc.name, "read");
    assert_eq!(tc.arguments, json!({"path": "README.md"}));
    let serialized = serde_json::to_value(&message.content[0]).unwrap();
    assert!(serialized.get("streamIndex").is_none() && serialized.get("partialArgs").is_none());
}

#[test]
fn accumulates_mixed_content_reasoning_and_parallel_tool_calls() {
    let tc = |index: Option<u64>, id: Option<&str>, name: Option<&str>, args: &str| {
        let mut v = json!({"type": "function", "function": {"arguments": args}});
        if let Some(i) = index {
            v["index"] = json!(i);
        }
        if let Some(id) = id {
            v["id"] = json!(id);
        }
        if let Some(name) = name {
            v["function"]["name"] = json!(name);
        }
        v
    };
    let server = serve_chunks(&[
        json!({"id": "m", "choices": [{"delta": {
            "content": "answer 1",
            "reasoning_content": "think 1",
            "tool_calls": [
                tc(Some(0), Some("tc_read_initial"), Some("read"), "{\"path\":\"README"),
                tc(Some(1), Some("tc_grep_initial"), Some("grep"), "{\"pattern\":\"TODO"),
                tc(None, Some("tc_list_no_index"), Some("list"), "{\"path\":\"packages"),
                tc(None, Some("tc_write_no_index"), Some("write"), "{\"path\":\"out"),
            ]}, "finish_reason": null}]}),
        json!({"id": "m", "choices": [{"delta": {
            "content": " answer 2",
            "tool_calls": [
                tc(Some(1), Some("tc_grep_changed"), None, "\",\"path\":\"src"),
                tc(None, Some("tc_write_no_index"), None, ".txt\",\"content\":\"ok\"}"),
                tc(None, Some("tc_list_no_index"), None, "/ai\"}"),
            ]}, "finish_reason": null}]}),
        json!({"id": "m", "choices": [{"delta": {
            "content": "\n",
            "reasoning_content": " think 2",
            "tool_calls": [
                tc(Some(0), Some("tc_read_changed"), None, ".md\"}"),
                tc(Some(1), None, None, "\"}"),
            ]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 8, "prompt_tokens_details": {"cached_tokens": 0}, "completion_tokens_details": {"reasoning_tokens": 2}}}),
    ]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let tools = ["read", "grep", "list", "write"].map(read_tool).to_vec();
    let (events, message) = run(
        model,
        Context::new(
            String::new(),
            vec![user("Think, answer, and use tools.")],
            tools,
        ),
    );
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    let count = |t: &str| events.iter().filter(|e| event_type(e) == t).count();
    assert_eq!(count("text_start"), 1);
    assert_eq!(count("text_delta"), 3);
    assert_eq!(count("text_end"), 1);
    assert_eq!(count("thinking_start"), 1);
    assert_eq!(count("thinking_delta"), 2);
    assert_eq!(count("thinking_end"), 1);
    assert_eq!(count("toolcall_start"), 4);
    assert_eq!(count("toolcall_delta"), 9);
    assert_eq!(count("toolcall_end"), 4);
    let for_index = |i: usize| -> Vec<&str> {
        events
            .iter()
            .filter(|e| tool_index(e) == Some(i))
            .map(event_type)
            .collect()
    };
    let (s, d, e) = ("toolcall_start", "toolcall_delta", "toolcall_end");
    assert_eq!(for_index(2), [s, d, d, e]);
    assert_eq!(for_index(3), [s, d, d, d, e]);
    assert_eq!(for_index(4), [s, d, d, e]);
    assert_eq!(for_index(5), [s, d, d, e]);

    assert_eq!(message.content.len(), 6);
    assert_eq!(
        serde_json::to_value(&message.content[0]).unwrap(),
        json!({"type": "text", "text": "answer 1 answer 2\n"})
    );
    assert_eq!(
        serde_json::to_value(&message.content[1]).unwrap(),
        json!({"type": "thinking", "thinking": "think 1 think 2", "thinkingSignature": "reasoning_content"})
    );
    let expected = [
        ("tc_read_initial", "read", json!({"path": "README.md"})),
        (
            "tc_grep_initial",
            "grep",
            json!({"pattern": "TODO", "path": "src"}),
        ),
        ("tc_list_no_index", "list", json!({"path": "packages/ai"})),
        (
            "tc_write_no_index",
            "write",
            json!({"path": "out.txt", "content": "ok"}),
        ),
    ];
    for (i, (id, name, args)) in expected.into_iter().enumerate() {
        let tc = tool_call(&message.content[2 + i]);
        assert_eq!((tc.id.as_str(), tc.name.as_str()), (id, name));
        assert_eq!(tc.arguments, args);
    }
}

#[test]
fn does_not_double_count_reasoning_tokens() {
    let server = serve_chunks(&[json!({
        "id": "r", "choices": [{"delta": {}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 33, "prompt_tokens_details": {"cached_tokens": 0}, "completion_tokens_details": {"reasoning_tokens": 21}}
    })]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let (_, message) = run(
        model,
        Context::new(String::new(), vec![user("Use reasoning.")], vec![]),
    );
    assert_eq!(
        (
            message.usage.input,
            message.usage.output,
            message.usage.total_tokens
        ),
        (10, 33, 43)
    );
}

fn cache_write_usage() -> Value {
    json!({
        "prompt_tokens": 100,
        "completion_tokens": 5,
        "prompt_tokens_details": {"cached_tokens": 50, "cache_write_tokens": 30},
        "completion_tokens_details": {"reasoning_tokens": 0}
    })
}

#[test]
fn preserves_cache_write_tokens_from_chunk_usage() {
    let server = serve_chunks(&[
        json!({"id": "w", "choices": [{"delta": {"content": "OK"}, "finish_reason": null}]}),
        json!({"id": "w", "choices": [{"delta": {}, "finish_reason": "stop"}], "usage": cache_write_usage()}),
    ]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let (_, m) = run(model, Context::new(String::new(), vec![user("x")], vec![]));
    assert_eq!(
        (
            m.usage.input,
            m.usage.cache_read,
            m.usage.cache_write,
            m.usage.total_tokens
        ),
        (50, 20, 30, 105)
    );
    // Priced with the catalog model's rates.
    assert!(m.usage.cost.total > 0.0);
}

#[test]
fn preserves_cache_write_tokens_from_choice_usage_fallback() {
    let server = serve_chunks(&[
        json!({"id": "w", "choices": [{"delta": {"content": "OK"}, "finish_reason": null}]}),
        json!({"id": "w", "choices": [{"delta": {}, "finish_reason": "stop", "usage": cache_write_usage()}]}),
    ]);
    let model = at(completions("openai", "gpt-4o-mini"), &server);
    let (_, m) = run(model, Context::new(String::new(), vec![user("x")], vec![]));
    assert_eq!(
        (
            m.usage.input,
            m.usage.cache_read,
            m.usage.cache_write,
            m.usage.total_tokens
        ),
        (50, 20, 30, 105)
    );
}

// --- openai-completions-param-fallback.test.ts ---

fn refusal(param: &str) -> (&'static str, &'static str, &'static str) {
    let body: &'static str = Box::leak(
        json!({"error": {"message": format!("{param}: Extra inputs are not permitted.")}})
            .to_string()
            .into_boxed_str(),
    );
    (
        "HTTP/1.1 422 Unprocessable Entity",
        "application/json",
        body,
    )
}

fn ok_body() -> &'static str {
    Box::leak(
        sse(&[json!({"choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}})])
            .into_boxed_str(),
    )
}

/// A model on a gateway of our own, so base-URL detection stays out of it.
fn gateway_run(server: &ScriptedServer) -> AssistantMessage {
    let model = at(completions("openai", "gpt-4o-mini"), server);
    let options = SimpleStreamOptions {
        api_key: Some("test-key".into()),
        session_id: Some("session-1".into()),
        cache_retention: Some(cortexcode_ai_types::CacheRetention::Long),
        ..Default::default()
    };
    let context = Context::new("sys".into(), vec![user("hi")], vec![]);
    collect(stream(model, context, options).unwrap()).1
}

#[test]
fn retries_without_the_param_a_strict_gateway_refuses() {
    let server = serve_script(vec![
        refusal("prompt_cache_retention"),
        (OK, SSE, ok_body()),
    ]);
    let result = gateway_run(&server);
    assert_eq!(result.stop_reason, StopReason::Stop);
    let payloads: Vec<Value> = server.requests().iter().map(|r| r.json()).collect();
    assert_eq!(payloads.len(), 2);
    assert_eq!(payloads[0]["prompt_cache_retention"], "24h");
    assert!(payloads[1].get("prompt_cache_retention").is_none());
    // Only the named param goes.
    assert_eq!(payloads[1]["prompt_cache_key"], "session-1");
    assert_eq!(payloads[1]["messages"], payloads[0]["messages"]);
}

#[test]
fn drops_one_param_per_pass_when_several_are_refused() {
    let server = serve_script(vec![
        refusal("prompt_cache_retention"),
        refusal("store"),
        (OK, SSE, ok_body()),
    ]);
    let result = gateway_run(&server);
    assert_eq!(result.stop_reason, StopReason::Stop);
    let payloads: Vec<Value> = server.requests().iter().map(|r| r.json()).collect();
    assert_eq!(payloads.len(), 3);
    assert!(payloads[2].get("prompt_cache_retention").is_none());
    assert!(payloads[2].get("store").is_none());
}

#[test]
fn remembers_the_refusal_for_later_requests() {
    let server = serve_script(vec![
        refusal("prompt_cache_retention"),
        (OK, SSE, ok_body()),
        (OK, SSE, ok_body()),
    ]);
    gateway_run(&server);
    gateway_run(&server);
    let payloads: Vec<Value> = server.requests().iter().map(|r| r.json()).collect();
    assert_eq!(payloads.len(), 3);
    assert!(payloads[2].get("prompt_cache_retention").is_none());
}

#[test]
fn keeps_what_it_learned_to_the_endpoint_that_taught_it() {
    let refusing = serve_script(vec![
        refusal("prompt_cache_retention"),
        (OK, SSE, ok_body()),
    ]);
    gateway_run(&refusing);
    let other = serve_script(vec![(OK, SSE, ok_body())]);
    gateway_run(&other);
    assert_eq!(other.requests()[0].json()["prompt_cache_retention"], "24h");
}

#[test]
fn does_not_drop_params_on_an_error_naming_none_of_them() {
    let server = serve_script(vec![refusal("messages")]);
    let result = gateway_run(&server);
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(server.requests().len(), 1);
    assert_eq!(
        result.error_message.as_deref(),
        Some("422 messages: Extra inputs are not permitted.")
    );
}

// --- openai-completions-response-model.test.ts ---

fn open_router_auto(server: &ScriptedServer) -> Model {
    Model {
        id: "openrouter/auto".into(),
        name: "OpenRouter Auto".into(),
        api: "openai-completions".into(),
        provider: "openrouter".into(),
        base_url: server.base_url.clone(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 200_000,
        max_tokens: 8192,
        headers: None,
        compat: None,
    }
}

fn response_model_run(chunks: &[Value]) -> AssistantMessage {
    let server = serve_chunks(chunks);
    run(
        open_router_auto(&server),
        Context::new(String::new(), vec![user("hi")], vec![]),
    )
    .1
}

#[test]
fn surfaces_routed_chunk_model_on_response_model() {
    let m = response_model_run(&[
        json!({"id": "chatcmpl-1", "model": "anthropic/claude-opus-4.7", "choices": [{"index": 0, "delta": {"content": "hi"}}]}),
        json!({"id": "chatcmpl-1", "model": "anthropic/claude-opus-4.7", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage(10, 5)}),
    ]);
    assert_eq!(m.model, "openrouter/auto");
    assert_eq!(
        m.response_model.as_deref(),
        Some("anthropic/claude-opus-4.7")
    );
    assert_eq!(m.provider, "openrouter");
    assert_eq!(m.stop_reason, StopReason::Stop);
}

#[test]
fn leaves_response_model_unset_when_chunks_echo_the_requested_id() {
    let m = response_model_run(&[
        json!({"id": "chatcmpl-2", "model": "openrouter/auto", "choices": [{"index": 0, "delta": {"content": "hi"}}]}),
        json!({"id": "chatcmpl-2", "model": "openrouter/auto", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage(1, 1)}),
    ]);
    assert_eq!(m.model, "openrouter/auto");
    assert_eq!(m.response_model, None);
}

#[test]
fn ignores_empty_or_missing_chunk_model() {
    let m = response_model_run(&[
        json!({"id": "chatcmpl-3", "choices": [{"index": 0, "delta": {"content": "hi"}}]}),
        json!({"id": "chatcmpl-3", "model": "", "choices": [{"index": 0, "delta": {"content": "!"}}]}),
        json!({"id": "chatcmpl-3", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage(1, 2)}),
    ]);
    assert_eq!(m.response_model, None);
}

// --- openai-completions-thinking-as-text.test.ts / -prompt-suffix.test.ts (over HTTP) ---

fn ok_chunks() -> Vec<Value> {
    vec![
        json!({"id": "chatcmpl-repro", "object": "chat.completion.chunk", "created": 0, "model": "repro-model",
               "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": null}]}),
        json!({"id": "chatcmpl-repro", "object": "chat.completion.chunk", "created": 0, "model": "repro-model",
               "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1}}),
    ]
}

#[test]
fn reaches_the_endpoint_when_replay_contains_thinking_and_text() {
    let server = serve_chunks(&ok_chunks());
    let context = repro_context(vec![
        thinking("internal reasoning"),
        Content::Text(TextContent::new("visible answer")),
    ]);
    let (events, _) = run(repro_model(&server.base_url), context);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/chat/completions");
    assert_eq!(
        requests[0].json()["messages"][1],
        json!({"role": "assistant", "content": [
            {"type": "text", "text": "internal reasoning"},
            {"type": "text", "text": "visible answer"}
        ]})
    );
    assert_eq!(event_type(events.last().unwrap()), "done");
}

#[test]
fn prompt_suffix_reaches_the_endpoint() {
    let server = serve_chunks(&ok_chunks());
    let mut model = repro_model(&server.base_url);
    model.reasoning = false;
    model.compat = Some(json!({"promptSuffix": "/no_think"}));
    run(
        model,
        Context::new(String::new(), vec![user("summarize this")], vec![]),
    );
    let body = server.requests()[0].json();
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type": "text", "text": "summarize this /no_think"}])
    );
}

// --- request plumbing ---

#[test]
fn sends_auth_model_and_copilot_headers() {
    let server = serve_chunks(&ok_chunks());
    let mut model = repro_model(&server.base_url);
    model.provider = "github-copilot".into();
    model.headers = Some([("X-Custom".to_string(), "1".to_string())].into());
    run(model, Context::new(String::new(), vec![user("hi")], vec![]));
    let req = &server.requests()[0];
    assert_eq!(req.header("authorization"), Some("Bearer test"));
    assert_eq!(req.header("x-custom"), Some("1"));
    assert_eq!(req.header("x-initiator"), Some("user"));
    assert_eq!(req.header("openai-intent"), Some("conversation-edits"));
    assert_eq!(req.header("copilot-vision-request"), None);
}

#[test]
fn stream_populates_partial_content_while_streaming() {
    let server = serve_chunks(&ok_chunks());
    let (events, _) = run(
        repro_model(&server.base_url),
        Context::new(String::new(), vec![user("hi")], vec![]),
    );
    let delta = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::TextDelta { partial, .. } => Some(partial.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(delta.content, vec![Content::Text(TextContent::new("ok"))]);
    assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
}

#[test]
fn missing_api_key_errors_immediately() {
    let _lock = env_lock();
    let saved = std::env::var("OPENAI_API_KEY").ok();
    std::env::remove_var("OPENAI_API_KEY");
    let mut model = completions("openai", "gpt-4o-mini");
    model.base_url = "http://127.0.0.1:9".into();
    let err = stream(
        model,
        Context::new(String::new(), vec![], vec![]),
        SimpleStreamOptions::default(),
    )
    .err()
    .expect("missing key is an error");
    assert_eq!(err.to_string(), "No API key for provider: openai");
    if let Some(v) = saved {
        std::env::set_var("OPENAI_API_KEY", v);
    }
}

#[test]
fn api_error_message_matches_the_openai_sdk() {
    assert_eq!(
        api_error_message(400, r#"{"error": {"message": "bad input"}}"#),
        "400 bad input"
    );
    assert_eq!(
        api_error_message(500, r#"{"error": {"code": 1}}"#),
        r#"500 {"code":1}"#
    );
    assert_eq!(api_error_message(502, "upstream down"), "502 upstream down");
    assert_eq!(
        api_error_message(503, r#"{"detail": "x"}"#),
        "503 status code (no body)"
    );
    assert_eq!(api_error_message(429, ""), "429 status code (no body)");
}

#[test]
fn http_error_appends_openrouter_raw_metadata() {
    let server = serve_script(vec![(
        "HTTP/1.1 400 Bad Request",
        "application/json",
        r#"{"error":{"message":"Provider returned error","metadata":{"raw":"upstream said no"}}}"#,
    )]);
    let (events, m) = run(
        repro_model(&server.base_url),
        Context::new(String::new(), vec![user("hi")], vec![]),
    );
    assert_eq!(events.len(), 1, "no start event before a failed request");
    assert_eq!(m.stop_reason, StopReason::Error);
    assert_eq!(
        m.error_message.as_deref(),
        Some("400 Provider returned error\nupstream said no")
    );
}

/// `testAbortSignal` in `abort.test.ts`, against a server that stalls
/// mid-message.
#[test]
fn abort_mid_stream_keeps_partial_content() {
    let head = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"15 + 27 = 42. \"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Names: Ann\"},\"finish_reason\":null}]}\n\n",
    );
    let base_url = serve_sse_then_hang(head, Duration::from_secs(30));
    let signal = AbortSignal::new();
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        signal: Some(signal.clone()),
        ..Default::default()
    };
    let mut model = repro_model(&base_url);
    model.reasoning = false;
    let mut s = stream(model, Context::new(String::new(), vec![], vec![]), options).unwrap();

    let started = std::time::Instant::now();
    let mut text = String::new();
    while let Some(event) = s.next_blocking() {
        if let AssistantMessageEvent::TextDelta { delta, .. } = &event {
            text.push_str(delta);
            if text.len() >= 20 {
                signal.abort();
            }
        }
    }
    assert!(started.elapsed() < Duration::from_secs(10));

    let msg = s.result_blocking();
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(msg.error_message.as_deref(), Some("Request was aborted"));
    match msg.content.as_slice() {
        [Content::Text(t)] => assert_eq!(t.text, "15 + 27 = 42. Names: Ann"),
        other => panic!("expected the partial text block, got {other:?}"),
    }
}

/// `testImmediateAbort` in `abort.test.ts`.
#[test]
fn immediate_abort() {
    let signal = AbortSignal::new();
    signal.abort();
    let options = SimpleStreamOptions {
        api_key: Some("sk-test".into()),
        signal: Some(signal),
        ..Default::default()
    };
    let s = stream(
        repro_model("http://127.0.0.1:9"),
        Context::new(String::new(), vec![], vec![]),
        options,
    )
    .unwrap();
    assert_eq!(s.result_blocking().stop_reason, StopReason::Aborted);
}

#[test]
fn map_stop_reason_matches_hoocode() {
    assert_eq!(map_stop_reason("stop").0, StopReason::Stop);
    assert_eq!(map_stop_reason("end").0, StopReason::Stop);
    assert_eq!(map_stop_reason("tool_calls").0, StopReason::ToolUse);
    assert_eq!(map_stop_reason("function_call").0, StopReason::ToolUse);
    assert_eq!(map_stop_reason("length").0, StopReason::Length);
    assert_eq!(
        map_stop_reason("content_filter"),
        (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".into())
        )
    );
}

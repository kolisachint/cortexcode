//! Provider tests.
//!
//! Ported from `openai-responses-copilot-provider.test.ts`; the live
//! `openai-responses-cache-affinity-e2e.test.ts` and
//! `openai-responses-reasoning-replay-e2e.test.ts` cases are `#[ignore]`d
//! (hoocode v0.5.89).

use super::*;
use crate::shared::tests::{catalog, user};
use cortexcode_ai_stream::testing::{serve_script, ScriptedServer};
use cortexcode_ai_types::{AssistantMessage, Content, Message, StopReason};

const DONE_ONLY: &str = "data: [DONE]\n\n";

fn serve(body: &'static str) -> ScriptedServer {
    serve_script(vec![("HTTP/1.1 200 OK", "text/event-stream", body)])
}

fn hi() -> Context {
    Context::new("sys".into(), vec![user("hi")], vec![])
}

fn key(options: ResponsesOptions) -> ResponsesOptions {
    ResponsesOptions {
        api_key: Some("test-key".into()),
        ..options
    }
}

/// Stream `model` against a mock server and return the request it received.
fn capture(
    mut model: Model,
    options: ResponsesOptions,
) -> (cortexcode_ai_stream::testing::Recorded, AssistantMessage) {
    let server = serve(DONE_ONLY);
    model.base_url = server.base_url.clone();
    let message = stream_responses(model, hi(), key(options)).result_blocking();
    (server.requests().remove(0), message)
}

#[test]
fn omits_reasoning_when_none_requested_for_copilot() {
    let (req, _) = capture(
        catalog("github-copilot", "gpt-5-mini"),
        ResponsesOptions::default(),
    );
    assert!(req.json().get("reasoning").is_none());
}

#[test]
fn sends_none_reasoning_effort_when_off_is_supported() {
    for id in [
        "gpt-5.1",
        "gpt-5.2",
        "gpt-5.3-codex",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.4-nano",
        "gpt-5.5",
    ] {
        let (req, _) = capture(catalog("openai", id), ResponsesOptions::default());
        assert_eq!(req.json()["reasoning"], json!({"effort": "none"}), "{id}");
    }
}

#[test]
fn omits_reasoning_effort_when_off_is_unsupported() {
    for id in [
        "gpt-5",
        "gpt-5-mini",
        "gpt-5-nano",
        "gpt-5-pro",
        "gpt-5.2-pro",
        "gpt-5.4-pro",
        "gpt-5.5-pro",
    ] {
        let (req, _) = capture(catalog("openai", id), ResponsesOptions::default());
        assert!(req.json().get("reasoning").is_none(), "{id}");
    }
}

fn session_options(headers: Option<HashMap<String, String>>) -> ResponsesOptions {
    ResponsesOptions {
        session_id: Some("session-123".into()),
        headers,
        ..Default::default()
    }
}

fn affinity(req: &cortexcode_ai_stream::testing::Recorded) -> (Option<&str>, Option<&str>) {
    (req.header("session_id"), req.header("x-client-request-id"))
}

#[test]
fn sets_cache_affinity_headers_for_official_openai() {
    let (req, _) = capture(catalog("openai", "gpt-5.4"), session_options(None));
    assert_eq!(affinity(&req), (Some("session-123"), Some("session-123")));
}

fn proxy_model(compat: Option<Value>) -> Model {
    let mut model = catalog("openai", "gpt-5.4");
    model.provider = "opencode".into();
    model.compat = compat;
    model
}

#[test]
fn sets_cache_affinity_headers_for_proxies() {
    let (req, _) = capture(proxy_model(None), session_options(None));
    assert_eq!(affinity(&req), (Some("session-123"), Some("session-123")));
}

#[test]
fn can_omit_the_session_id_header() {
    let (req, _) = capture(
        proxy_model(Some(json!({"sendSessionIdHeader": false}))),
        session_options(None),
    );
    assert_eq!(affinity(&req), (None, Some("session-123")));
}

#[test]
fn explicit_headers_override_cache_affinity_headers() {
    let headers: HashMap<String, String> = [
        ("session_id".to_string(), "override-session".to_string()),
        (
            "x-client-request-id".to_string(),
            "override-request".to_string(),
        ),
    ]
    .into();
    let (req, _) = capture(catalog("openai", "gpt-5.4"), session_options(Some(headers)));
    assert_eq!(
        affinity(&req),
        (Some("override-session"), Some("override-request"))
    );
}

#[test]
fn omits_cache_affinity_headers_when_cache_retention_is_none() {
    let options = ResponsesOptions {
        cache_retention: Some(CacheRetention::None),
        ..session_options(None)
    };
    let (req, _) = capture(catalog("openai", "gpt-5.4"), options);
    assert_eq!(affinity(&req), (None, None));
    assert!(req.json().get("prompt_cache_key").is_none());
}

#[test]
fn applies_service_tier_cost_multipliers() {
    for (id, tier, multiplier) in [
        ("gpt-5.4", "priority", 2.0),
        ("gpt-5.5", "priority", 2.5),
        ("gpt-5.5", "flex", 0.5),
    ] {
        let event = json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "service_tier": tier,
                "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000, "total_tokens": 2_000_000,
                          "input_tokens_details": {"cached_tokens": 0}}
            }
        });
        let body: &'static str = Box::leak(format!("data: {event}\n\n").into_boxed_str());
        let server = serve(body);
        let mut model = catalog("openai", id);
        model.base_url = server.base_url.clone();
        let options = ResponsesOptions {
            service_tier: Some(tier.into()),
            ..Default::default()
        };
        let result = stream_responses(model.clone(), hi(), key(options)).result_blocking();
        assert_eq!(
            result.usage.cost.input,
            model.cost.input * multiplier,
            "{id} {tier}"
        );
        assert_eq!(result.usage.cost.output, model.cost.output * multiplier);
        assert_eq!(
            result.usage.cost.total,
            (model.cost.input + model.cost.output) * multiplier
        );
        assert_eq!(server.requests()[0].json()["service_tier"], tier);
    }
}

// --- request body ---

#[test]
fn builds_the_hoocode_request_body() {
    let mut model = catalog("openai", "gpt-5.4");
    model.base_url = "https://api.openai.com/v1".into();
    let options = ResponsesOptions {
        session_id: Some("s1".into()),
        cache_retention: Some(CacheRetention::Long),
        max_tokens: Some(1000),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    };
    let body = build_params(&model, &hi(), &options);
    assert_eq!(
        body,
        json!({
            "model": "gpt-5.4",
            "input": [
                {"role": "developer", "content": "sys"},
                {"role": "user", "content": [{"type": "input_text", "text": "hi"}]}
            ],
            "stream": true,
            "prompt_cache_key": "s1",
            "prompt_cache_retention": "24h",
            "store": false,
            "max_output_tokens": 1000,
            "reasoning": {"effort": "high", "summary": "auto"},
            "include": ["reasoning.encrypted_content"]
        })
    );
    let mut model = model;
    model.compat = Some(json!({"supportsLongCacheRetention": false}));
    let body = build_params(&model, &hi(), &options);
    assert!(body.get("prompt_cache_retention").is_none());
}

#[test]
fn simple_stream_maps_reasoning_and_requires_a_key() {
    let model = catalog("openai", "gpt-5.4");
    let options = simple_options(
        &model,
        &SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..Default::default()
        },
        "k".into(),
    );
    assert_eq!(options.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(options.max_tokens, Some(model.max_tokens.min(32_000)));
    let mut model = catalog("openai", "gpt-5.4");
    model.provider = "no-such-provider".into();
    let err = stream(model, hi(), SimpleStreamOptions::default())
        .err()
        .unwrap();
    assert_eq!(err.to_string(), "No API key for provider: no-such-provider");
}

#[test]
fn streams_text_over_http() {
    let events = [
        json!({"type": "response.created", "response": {"id": "resp_1"}}),
        json!({"type": "response.output_item.added", "item": {"type": "message", "id": "msg_1", "content": []}}),
        json!({"type": "response.content_part.added", "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "delta": "Hello"}),
        json!({"type": "response.output_item.done", "item": {"type": "message", "id": "msg_1",
            "content": [{"type": "output_text", "text": "Hello"}]}}),
        json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed",
            "usage": {"input_tokens": 5, "output_tokens": 1, "total_tokens": 6}}}),
    ];
    let body: String = events
        .iter()
        .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
        .collect();
    let server = serve(Box::leak(body.into_boxed_str()));
    let mut model = catalog("openai", "gpt-5.4");
    model.base_url = server.base_url.clone();
    let mut s = stream_responses(model, hi(), key(ResponsesOptions::default()));
    let mut kinds = Vec::new();
    while let Some(e) = s.next_blocking() {
        kinds.push(match e {
            cortexcode_ai_types::AssistantMessageEvent::Start { .. } => "start",
            cortexcode_ai_types::AssistantMessageEvent::TextStart { .. } => "text_start",
            cortexcode_ai_types::AssistantMessageEvent::TextDelta { .. } => "text_delta",
            cortexcode_ai_types::AssistantMessageEvent::TextEnd { .. } => "text_end",
            cortexcode_ai_types::AssistantMessageEvent::Done { .. } => "done",
            _ => "other",
        });
    }
    assert_eq!(
        kinds,
        ["start", "text_start", "text_delta", "text_end", "done"]
    );
    let m = s.result_blocking();
    assert_eq!(m.response_id.as_deref(), Some("resp_1"));
    assert_eq!(m.stop_reason, StopReason::Stop);
    assert_eq!(server.requests()[0].path, "/responses");
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some("Bearer test-key")
    );
}

#[test]
fn http_errors_and_failed_responses_end_in_error() {
    let server = serve_script(vec![(
        "HTTP/1.1 401 Unauthorized",
        "application/json",
        r#"{"error":{"message":"bad key"}}"#,
    )]);
    let mut model = catalog("openai", "gpt-5.4");
    model.base_url = server.base_url.clone();
    let m =
        stream_responses(model.clone(), hi(), key(ResponsesOptions::default())).result_blocking();
    assert_eq!(m.stop_reason, StopReason::Error);
    assert_eq!(m.error_message.as_deref(), Some("401 bad key"));

    let failed = json!({"type": "response.completed", "response": {"status": "failed"}});
    let server = serve(Box::leak(format!("data: {failed}\n\n").into_boxed_str()));
    model.base_url = server.base_url.clone();
    let m = stream_responses(model, hi(), key(ResponsesOptions::default())).result_blocking();
    assert_eq!(m.stop_reason, StopReason::Error);
    assert_eq!(
        m.error_message.as_deref(),
        Some("An unknown error occurred")
    );
}

// --- live e2e (need OPENAI_API_KEY) ---

fn live_model(id: &str) -> Option<(Model, String)> {
    let key = std::env::var("OPENAI_API_KEY").ok()?;
    Some((catalog("openai", id), key))
}

/// `openai-responses-cache-affinity-e2e.test.ts`.
#[test]
#[ignore = "live: needs OPENAI_API_KEY and network"]
fn live_cache_affinity() {
    let (model, key) = live_model("gpt-5.4").expect("OPENAI_API_KEY");
    let context = Context::new(
        "You are a helpful assistant. Reply exactly as requested.".into(),
        vec![user(
            "Reply with exactly: openai cache affinity e2e success",
        )],
        vec![],
    );
    let options = SimpleStreamOptions {
        api_key: Some(key),
        session_id: Some("0195d6e4-4cf9-7f44-a2d8-f8f7f49ee9d3".into()),
        ..Default::default()
    };
    let m = stream(model, context, options).unwrap().result_blocking();
    assert_ne!(m.stop_reason, StopReason::Error, "{:?}", m.error_message);
    let text: String = m
        .content
        .iter()
        .filter_map(|b| match b {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(text.contains("openai cache affinity e2e success"));
}

/// `openai-responses-reasoning-replay-e2e.test.ts` (first case): a
/// reasoning-only aborted turn is skipped on replay.
#[test]
#[ignore = "live: needs OPENAI_API_KEY and network"]
fn live_skips_reasoning_only_history_after_an_aborted_turn() {
    let (model, key) = live_model("gpt-5-mini").expect("OPENAI_API_KEY");
    let tool = cortexcode_ai_types::Tool {
        defer_loading: None,
        name: "double_number".into(),
        description: "Doubles a number and returns the result".into(),
        parameters: json!({"type": "object", "properties": {"value": {"type": "number", "description": "A number to double"}}, "required": ["value"]}),
    };
    let first_context = Context::new(
        "You are a helpful assistant. Use the tool.".into(),
        vec![user("Use the double_number tool to double 21.")],
        vec![tool.clone()],
    );
    let options = ResponsesOptions {
        api_key: Some(key.clone()),
        reasoning_effort: Some("high".into()),
        ..Default::default()
    };
    let response =
        stream_responses(model.clone(), first_context, options.clone()).result_blocking();
    let thinking = response
        .content
        .iter()
        .find(|b| matches!(b, Content::Thinking(t) if t.signature.is_some()))
        .cloned()
        .expect("thinking signature");
    let corrupted = AssistantMessage {
        content: vec![thinking],
        stop_reason: StopReason::Aborted,
        ..response
    };
    let context = Context::new(
        "You are a helpful assistant.".into(),
        vec![
            user("Use the double_number tool to double 21."),
            Message::Assistant(corrupted),
            user("Say hello to confirm you can continue."),
        ],
        vec![tool],
    );
    let m = stream_responses(model, context, options).result_blocking();
    assert_ne!(m.stop_reason, StopReason::Error, "{:?}", m.error_message);
}

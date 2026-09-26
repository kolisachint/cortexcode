//! Ported from hoocode `google-gemini-cli.test.ts` (request envelope and
//! `extractRetryDelay`; the OAuth half lives in `cortexcode-ai-oauth-google`),
//! plus stream, retry and option-mapping cases against mock servers.

use super::*;
use cortexcode_ai_stream::testing::serve_script;
use cortexcode_ai_types::{Message, ModelCost, Tool, UserMessage};
use reqwest::header::{HeaderName, HeaderValue};

fn model() -> Model {
    Model {
        id: "gemini-3.1-pro-preview".into(),
        name: "Gemini 3.1 Pro Preview (Cloud Code Assist)".into(),
        api: "google-gemini-cli".into(),
        provider: "google-gemini-cli".into(),
        base_url: "https://cloudcode-pa.googleapis.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost::default(),
        context_window: 1_048_576,
        max_tokens: 65535,
        headers: None,
        compat: None,
    }
}

fn antigravity(id: &str) -> Model {
    Model {
        provider: "google-antigravity".into(),
        id: id.into(),
        ..model()
    }
}

fn context() -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![Message::User(UserMessage {
            content: "Hello".into(),
            timestamp: 1,
        })],
        vec![],
    )
}

fn calculator_tool() -> Tool {
    Tool {
        name: "math_operation".into(),
        description: "Perform basic arithmetic operations".into(),
        parameters: json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"],
        }),
        defer_loading: None,
    }
}

// --- google-gemini-cli.test.ts: "Cloud Code Assist request envelope" ---

#[test]
fn wraps_the_gemini_request_with_project_model_and_agent_metadata() {
    let request = build_request(
        &model(),
        &context(),
        "my-project",
        &GeminiCliOptions::default(),
        false,
    );
    assert_eq!(request["project"], "my-project");
    assert_eq!(request["model"], "gemini-3.1-pro-preview");
    assert_eq!(request["userAgent"], "hoocode");
    assert!(request.get("requestType").is_none());
    assert_eq!(
        request["request"]["systemInstruction"]["parts"],
        json!([{"text": "You are a helpful assistant."}])
    );
    assert_eq!(request["request"]["contents"].as_array().unwrap().len(), 1);
    let id = request["requestId"].as_str().unwrap();
    assert!(id.starts_with("hoo-"), "{id}");
    assert_eq!(id.rsplit('-').next().unwrap().len(), 9, "{id}");
    // Key order as the TS object literal builds it.
    let keys: Vec<&String> = request.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["project", "model", "request", "userAgent", "requestId"]
    );
}

#[test]
fn marks_antigravity_requests_as_agent_requests_and_prepends_its_system_instruction() {
    let request = build_request(
        &antigravity("gemini-3.1-pro-high"),
        &context(),
        "my-project",
        &GeminiCliOptions::default(),
        true,
    );
    assert_eq!(request["requestType"], "agent");
    assert_eq!(request["userAgent"], "antigravity");
    assert!(request["requestId"].as_str().unwrap().starts_with("agent-"));
    let instruction = &request["request"]["systemInstruction"];
    assert_eq!(instruction["role"], "user");
    let parts = instruction["parts"].as_array().unwrap();
    assert!(parts[0]["text"]
        .as_str()
        .unwrap()
        .contains("You are Antigravity"));
    assert_eq!(
        parts.last().unwrap()["text"],
        "You are a helpful assistant."
    );
}

#[test]
fn sends_claude_tool_schemas_as_parameters_and_gemini_ones_as_parameters_json_schema() {
    let mut with_tools = context();
    with_tools.tools = vec![calculator_tool()];
    let claude = build_request(
        &antigravity("claude-sonnet-4-6"),
        &with_tools,
        "my-project",
        &GeminiCliOptions::default(),
        true,
    );
    let gemini = build_request(
        &model(),
        &with_tools,
        "my-project",
        &GeminiCliOptions::default(),
        false,
    );
    let declaration = |r: &Value| r["request"]["tools"][0]["functionDeclarations"][0].clone();
    assert!(declaration(&claude).get("parameters").is_some());
    assert!(declaration(&gemini).get("parametersJsonSchema").is_some());
}

#[test]
fn maps_a_thinking_level_onto_the_generation_config_for_gemini_3_models() {
    let options = GeminiCliOptions {
        thinking: Some(GoogleThinking {
            enabled: true,
            level: Some("HIGH".into()),
            budget_tokens: None,
        }),
        ..Default::default()
    };
    let request = build_request(&model(), &context(), "my-project", &options, false);
    assert_eq!(
        request["request"]["generationConfig"]["thinkingConfig"],
        json!({"includeThoughts": true, "thinkingLevel": "HIGH"})
    );
}

#[test]
fn request_body_key_order_tool_choice_and_session_id() {
    let mut with_tools = context();
    with_tools.tools = vec![calculator_tool()];
    with_tools.system_prompt = String::new();
    let options = GeminiCliOptions {
        temperature: Some(0.5),
        max_tokens: Some(100),
        session_id: Some("s1".into()),
        tool_choice: Some("any".into()),
        thinking: Some(GoogleThinking {
            enabled: true,
            budget_tokens: Some(2048),
            level: None,
        }),
        ..Default::default()
    };
    let request = build_request(&antigravity("x"), &with_tools, "p", &options, true);
    let inner = request["request"].as_object().unwrap();
    let keys: Vec<&String> = inner.keys().collect();
    // No system prompt: Antigravity's instruction is added last.
    assert_eq!(
        keys,
        [
            "contents",
            "sessionId",
            "generationConfig",
            "tools",
            "toolConfig",
            "systemInstruction"
        ]
    );
    assert_eq!(
        inner["generationConfig"],
        json!({"temperature": 0.5, "maxOutputTokens": 100,
               "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 2048}})
    );
    assert_eq!(
        inner["toolConfig"],
        json!({"functionCallingConfig": {"mode": "ANY"}})
    );
    assert_eq!(
        inner["systemInstruction"]["parts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Thinking is left out for a non-reasoning model.
    let plain = Model {
        reasoning: false,
        ..model()
    };
    let request = build_request(&plain, &context(), "p", &options, false);
    assert!(request["request"]["generationConfig"]
        .get("thinkingConfig")
        .is_none());
}

// --- google-gemini-cli.test.ts: "extractRetryDelay" ---

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        map.insert(
            HeaderName::from_static(k),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    map
}

#[test]
fn reads_a_quota_reset_duration_from_the_error_body() {
    assert_eq!(
        extract_retry_delay("Your quota will reset after 39s", None),
        Some(40000)
    );
    assert_eq!(
        extract_retry_delay("Your quota will reset after 1h2m3s", None),
        Some(3_724_000)
    );
}

#[test]
fn prefers_the_retry_after_header() {
    let h = headers(&[("retry-after", "5")]);
    assert_eq!(
        extract_retry_delay("Your quota will reset after 39s", Some(&h)),
        Some(6000)
    );
}

#[test]
fn returns_none_when_no_delay_is_advertised() {
    assert_eq!(extract_retry_delay("Internal error", None), None);
}

#[test]
fn other_retry_delay_forms() {
    assert_eq!(
        extract_retry_delay("Please retry in 250ms", None),
        Some(1250)
    );
    assert_eq!(
        extract_retry_delay("please RETRY in 1.5s", None),
        Some(2500)
    );
    assert_eq!(
        extract_retry_delay(r#"{"retryDelay": "34.074824224s"}"#, None),
        Some(35075)
    );
    let h = headers(&[("x-ratelimit-reset-after", "2")]);
    assert_eq!(extract_retry_delay("", Some(&h)), Some(3000));
    // A reset time in the past, then an unparsable Retry-After, fall
    // through to the body.
    let h = headers(&[("x-ratelimit-reset", "1"), ("retry-after", "soon")]);
    assert_eq!(extract_retry_delay("reset after 1s", Some(&h)), Some(2000));
    let future = chrono::Utc::now() + chrono::Duration::seconds(30);
    let h = headers(&[("retry-after", &future.to_rfc2822())]);
    let delay = extract_retry_delay("", Some(&h)).unwrap();
    assert!((29_000..=31_000).contains(&delay), "{delay}");
}

#[test]
fn retryable_errors_and_error_messages() {
    for status in [429, 500, 502, 503, 504] {
        assert!(is_retryable_error(status, ""));
    }
    assert!(is_retryable_error(400, "RESOURCE_EXHAUSTED"));
    assert!(is_retryable_error(400, "other side closed"));
    assert!(!is_retryable_error(400, "bad request"));
    assert_eq!(
        extract_error_message(r#"{"error":{"message":"quota"}}"#),
        "quota"
    );
    assert_eq!(extract_error_message("plain"), "plain");
}

// --- streamSimpleGoogleGeminiCli option mapping ---

fn simple(model: &Model, reasoning: Option<ThinkingLevel>) -> GeminiCliOptions {
    let options = SimpleStreamOptions {
        reasoning,
        ..Default::default()
    };
    simple_options(model, &options, "k".into())
}

#[test]
fn simple_options_map_effort_to_levels_and_budgets() {
    let off = simple(&model(), None);
    assert_eq!(off.thinking, Some(GoogleThinking::default()));
    assert_eq!(off.max_tokens, Some(32_000));

    let level = |id: &str, effort| {
        let m = Model {
            id: id.into(),
            ..model()
        };
        simple(&m, Some(effort)).thinking.unwrap().level.unwrap()
    };
    assert_eq!(
        level("gemini-3.1-pro-preview", ThinkingLevel::Minimal),
        "LOW"
    );
    assert_eq!(
        level("gemini-3.1-pro-preview", ThinkingLevel::Medium),
        "HIGH"
    );
    assert_eq!(level("gemini-pro-agent", ThinkingLevel::XHigh), "HIGH");
    assert_eq!(
        level("gemini-3-flash-preview", ThinkingLevel::Minimal),
        "MINIMAL"
    );
    assert_eq!(level("gemini-3.8-flash", ThinkingLevel::Minimal), "LOW");
    assert_eq!(
        level("gemini-3.1-flash-lite-preview", ThinkingLevel::Medium),
        "MEDIUM"
    );

    let flash_25 = Model {
        id: "gemini-2.5-flash".into(),
        ..model()
    };
    let o = simple(&flash_25, Some(ThinkingLevel::Medium));
    assert_eq!(o.max_tokens, Some(32_000 + 8192));
    assert_eq!(o.thinking.unwrap().budget_tokens, Some(8192));

    // The budget shrinks to leave 1024 output tokens under the model cap.
    let small = Model {
        max_tokens: 4000,
        ..flash_25
    };
    let o = simple(&small, Some(ThinkingLevel::High));
    assert_eq!(o.max_tokens, Some(4000));
    assert_eq!(o.thinking.unwrap().budget_tokens, Some(2976));
}

#[test]
fn endpoint_order_for_both_providers() {
    assert_eq!(endpoints(&model()), ["https://cloudcode-pa.googleapis.com"]);
    let blank = Model {
        base_url: " ".into(),
        ..model()
    };
    assert_eq!(endpoints(&blank), [DEFAULT_ENDPOINT]);
    let ag = Model {
        base_url: ANTIGRAVITY_DAILY_ENDPOINT.into(),
        ..antigravity("x")
    };
    assert_eq!(endpoints(&ag), ANTIGRAVITY_ENDPOINT_FALLBACKS);
    let custom = Model {
        base_url: "http://proxy".into(),
        ..antigravity("x")
    };
    assert_eq!(endpoints(&custom)[0], "http://proxy");
    assert_eq!(endpoints(&custom).len(), 4);
}

// --- streaming against a mock server ---

const API_KEY: &str = r#"{"token":"ya29.token","projectId":"proj"}"#;

fn sse(chunks: &[Value]) -> String {
    chunks
        .iter()
        .map(|c| format!("data: {c}\n\n"))
        .collect::<String>()
}

fn run_stream(model: Model, api_key: &str) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let options = SimpleStreamOptions {
        api_key: Some(api_key.into()),
        ..Default::default()
    };
    let mut s = stream(model, context(), options).unwrap();
    let mut events = Vec::new();
    while let Some(e) = s.next_blocking() {
        events.push(e);
    }
    let message = match events.last() {
        Some(AssistantMessageEvent::Done { message }) => message.clone(),
        Some(AssistantMessageEvent::Error { error }) => error.clone(),
        other => panic!("no terminal event: {other:?}"),
    };
    (events, message)
}

fn kinds(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
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
        })
        .collect()
}

#[test]
fn streams_thinking_text_and_tool_calls_with_usage() {
    let body = sse(&[
        json!({"response": {"responseId": "r1", "candidates": [{"content": {"role": "model", "parts": [
            {"text": "plan", "thought": true, "thoughtSignature": "sig"}]}}]}}),
        json!({"response": {"responseId": "r2", "candidates": [{"content": {"role": "model", "parts": [
            {"text": "Hi"}, {"functionCall": {"name": "read", "args": {"path": "a"}}}]},
            "finishReason": "STOP"}],
            "usageMetadata": {"promptTokenCount": 10, "cachedContentTokenCount": 4,
                              "candidatesTokenCount": 3, "thoughtsTokenCount": 2, "totalTokenCount": 15}}}),
    ]) + "data: {not json}\n\n";
    let server = serve_script(vec![("HTTP/1.1 200 OK", "text/event-stream", &body)]);
    let mut m = model();
    m.base_url = server.base_url.clone();
    let (events, message) = run_stream(m, API_KEY);
    assert_eq!(
        kinds(&events),
        [
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "done"
        ]
    );
    assert_eq!(message.stop_reason, StopReason::ToolUse);
    assert_eq!(message.response_id.as_deref(), Some("r1"));
    assert_eq!(message.api, "google-gemini-cli");
    assert_eq!(
        (
            message.usage.input,
            message.usage.output,
            message.usage.cache_read
        ),
        (6, 5, 4)
    );
    let Content::Thinking(thinking) = &message.content[0] else {
        panic!()
    };
    assert_eq!(thinking.signature.as_deref(), Some("sig"));
    let Content::ToolCall(call) = &message.content[2] else {
        panic!()
    };
    assert!(call.id.starts_with("read_"), "{}", call.id);
    assert_eq!(call.arguments, json!({"path": "a"}));

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, "/v1internal:streamGenerateContent?alt=sse");
    assert_eq!(request.header("authorization"), Some("Bearer ya29.token"));
    assert_eq!(request.header("accept"), Some("text/event-stream"));
    assert_eq!(request.header("x-goog-api-client"), Some("gl-node/22.17.0"));
    assert_eq!(
        request.header("client-metadata"),
        Some(
            r#"{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}"#
        )
    );
    assert_eq!(request.json()["project"], "proj");
}

#[test]
fn antigravity_claude_gets_its_user_agent_and_the_thinking_beta() {
    let m = antigravity("claude-sonnet-4-6");
    let headers = build_headers(&m, "t", &GeminiCliOptions::default());
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    assert!(get("user-agent").unwrap().starts_with("antigravity/"));
    assert!(get("user-agent").unwrap().ends_with(" darwin/arm64"));
    assert_eq!(get("anthropic-beta"), Some(CLAUDE_THINKING_BETA_HEADER));
    assert_eq!(get("x-goog-api-client"), None);
    let gemini = build_headers(&antigravity("gemini-pro-agent"), "t", &Default::default());
    assert!(!gemini.iter().any(|(k, _)| k == "anthropic-beta"));
}

#[test]
fn credential_errors() {
    let err = stream(model(), context(), SimpleStreamOptions::default())
        .err()
        .unwrap();
    assert_eq!(err.to_string(), NO_AUTH);
    let (_, message) = run_stream(model(), "not json");
    assert_eq!(
        message.error_message.as_deref(),
        Some("Invalid Google Cloud Code Assist credentials. Use /login to re-authenticate.")
    );
    let (_, message) = run_stream(model(), r#"{"token":"t"}"#);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(message
        .error_message
        .unwrap()
        .starts_with("Missing token or projectId"));
}

#[test]
fn retries_a_rate_limit_then_streams() {
    let ok = sse(&[
        json!({"response": {"candidates": [{"content": {"parts": [{"text": "ok"}]},
        "finishReason": "STOP"}]}}),
    ]);
    let server = serve_script(vec![
        (
            "HTTP/1.1 429 Too Many Requests",
            "application/json",
            r#"{"error":{"message":"slow down"}}"#,
        ),
        ("HTTP/1.1 200 OK", "text/event-stream", &ok),
    ]);
    let mut m = model();
    m.base_url = server.base_url.clone();
    let (_, message) = run_stream(m, API_KEY);
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_server_delay_over_the_cap_fails_after_the_catch_block_retries() {
    // The cap error is thrown inside the TS try block, so it is caught and
    // retried with backoff like any other error: one request per attempt.
    let body = r#"{"error":{"message":"Your quota will reset after 1h0m0s"}}"#;
    let server = serve_script(vec![
        ("HTTP/1.1 429 Too Many Requests", "application/json", body),
        ("HTTP/1.1 429 Too Many Requests", "application/json", body),
    ]);
    let mut m = model();
    m.base_url = server.base_url.clone();
    let options = SimpleStreamOptions {
        api_key: Some(API_KEY.into()),
        max_retry_delay_ms: Some(1000),
        ..Default::default()
    };
    // Abort after the second request so the test does not sit through the
    // whole backoff.
    let signal = AbortSignal::new();
    let options = SimpleStreamOptions {
        signal: Some(signal.clone()),
        ..options
    };
    let mut s = stream(m, context(), options).unwrap();
    let start = std::time::Instant::now();
    while server.requests().len() < 2 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(server.requests().len(), 2);
    signal.abort();
    let mut last = None;
    while let Some(e) = s.next_blocking() {
        last = Some(e);
    }
    let Some(AssistantMessageEvent::Error { error }) = last else {
        panic!()
    };
    assert_eq!(error.stop_reason, StopReason::Aborted);
    assert_eq!(error.error_message.as_deref(), Some(ABORTED));
}

#[test]
fn an_empty_stream_is_fetched_again_then_fails() {
    let empty = sse(&[json!({"response": {"candidates": [{"finishReason": "STOP"}]}})]);
    let server = serve_script(vec![
        ("HTTP/1.1 200 OK", "text/event-stream", &empty),
        ("HTTP/1.1 200 OK", "text/event-stream", &empty),
        ("HTTP/1.1 200 OK", "text/event-stream", &empty),
    ]);
    let mut m = model();
    m.base_url = server.base_url.clone();
    let (events, message) = run_stream(m, API_KEY);
    assert_eq!(kinds(&events), ["error"]);
    assert_eq!(
        message.error_message.as_deref(),
        Some("Cloud Code Assist API returned an empty response")
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_safety_stop_is_an_unknown_error() {
    let body = sse(&[
        json!({"response": {"candidates": [{"content": {"parts": [{"text": "x"}]},
        "finishReason": "SAFETY"}]}}),
    ]);
    let server = serve_script(vec![("HTTP/1.1 200 OK", "text/event-stream", &body)]);
    let mut m = model();
    m.base_url = server.base_url.clone();
    let (_, message) = run_stream(m, API_KEY);
    assert_eq!(message.stop_reason, StopReason::Error);
    assert_eq!(
        message.error_message.as_deref(),
        Some("An unknown error occurred")
    );
    assert_eq!(message.content.len(), 1);
}

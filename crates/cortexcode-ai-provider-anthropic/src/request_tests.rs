//! Request-shape tests: the TS tests capture the payload (`onPayload` or a
//! mocked SDK client); here the same pipeline (`simple_options`,
//! `build_headers`, `build_params`) runs without HTTP.
//!
//! Ported from the request-format cases of `claude-5-models.test.ts`,
//! `anthropic-thinking-disable.test.ts` and `anthropic-tool-search.test.ts`
//! (hoocode v0.5.89), plus convertMessages behaviour.

use super::*;
use cortexcode_ai_types::{
    ImageContent, TextContent, ThinkingContent, ToolCallContent, ToolResultMessage, UserMessage,
};

pub(crate) fn catalog(provider: &str, id: &str) -> Model {
    cortexcode_ai_models::get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} in catalog"))
        .clone()
}

/// `{ role: "user", content: text }`.
pub(crate) fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: text.into(),
        timestamp: 1,
    })
}

pub(crate) fn hello() -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![user("Hello")],
        vec![],
    )
}

/// `streamSimpleAnthropic`'s request: `(headers, params)`.
fn simple_request(
    model: &Model,
    context: &Context,
    api_key: &str,
    options: SimpleStreamOptions,
) -> (Vec<(String, String)>, Value) {
    let options = simple_options(model, &options, api_key.to_string());
    let (headers, oauth) = build_headers(model, context, api_key, &options);
    (headers, build_params(model, context, oauth, &options))
}

fn reasoning(level: ThinkingLevel) -> SimpleStreamOptions {
    SimpleStreamOptions {
        reasoning: Some(level),
        ..Default::default()
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

// --- claude-5-models.test.ts: "Claude Opus 5 request format" ---

#[test]
fn opus_5_uses_adaptive_thinking_instead_of_a_budget() {
    let (_, params) = simple_request(
        &catalog("anthropic", "claude-opus-5"),
        &hello(),
        "sk-ant-test",
        reasoning(ThinkingLevel::XHigh),
    );
    assert_eq!(params["model"], "claude-opus-5");
    assert_eq!(
        params["thinking"],
        json!({"type": "adaptive", "display": "omitted"})
    );
    assert_eq!(params["output_config"], json!({"effort": "xhigh"}));
}

#[test]
fn opus_5_on_copilot_uses_adaptive_thinking_and_copilot_auth_headers() {
    let (headers, params) = simple_request(
        &catalog("github-copilot", "claude-opus-5"),
        &hello(),
        "tid_copilot_session_test_token",
        reasoning(ThinkingLevel::High),
    );
    // `apiKey: null, authToken: …`: Bearer auth, no x-api-key.
    assert_eq!(header(&headers, "x-api-key"), None);
    assert_eq!(
        header(&headers, "authorization"),
        Some("Bearer tid_copilot_session_test_token")
    );
    assert!(header(&headers, "User-Agent")
        .unwrap()
        .contains("GitHubCopilotChat"));
    assert_eq!(
        header(&headers, "Copilot-Integration-Id"),
        Some("vscode-chat")
    );
    assert_eq!(params["model"], "claude-opus-5");
    assert_eq!(
        params["thinking"],
        json!({"type": "adaptive", "display": "omitted"})
    );
    assert_eq!(params["output_config"], json!({"effort": "high"}));
}

#[test]
fn sonnet_5_and_fable_5_use_adaptive_summarized_thinking() {
    for (provider, id) in [
        ("anthropic", "claude-sonnet-5"),
        ("github-copilot", "claude-sonnet-5"),
        ("anthropic", "claude-fable-5"),
        ("github-copilot", "claude-fable-5"),
    ] {
        let (_, params) = simple_request(
            &catalog(provider, id),
            &hello(),
            "test-token",
            reasoning(ThinkingLevel::XHigh),
        );
        assert_eq!(params["model"], id);
        assert_eq!(
            params["thinking"],
            json!({"type": "adaptive", "display": "summarized"}),
            "{provider}/{id}"
        );
        assert_eq!(params["output_config"], json!({"effort": "xhigh"}));
    }
}

// --- anthropic-thinking-disable.test.ts: payload cases ---

fn thinking_payload(provider: &str, id: &str, options: SimpleStreamOptions) -> Value {
    let mut model = catalog(provider, id);
    model.base_url = "http://127.0.0.1:9".into();
    let context = Context::new(String::new(), vec![user("Hello")], vec![]);
    simple_request(&model, &context, "fake-key", options).1
}

#[test]
fn thinking_off_sends_disabled_for_budget_and_adaptive_models() {
    for id in [
        "claude-sonnet-4-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-opus-5",
    ] {
        let payload = thinking_payload("anthropic", id, Default::default());
        assert_eq!(payload["thinking"], json!({"type": "disabled"}), "{id}");
        assert!(payload.get("output_config").is_none(), "{id}");
    }
}

#[test]
fn thinking_off_omits_thinking_and_asks_for_low_effort_on_opus_5_5() {
    for (provider, id) in [
        ("anthropic", "claude-opus-5-5"),
        ("github-copilot", "claude-opus-5.5"),
    ] {
        let payload = thinking_payload(provider, id, Default::default());
        assert!(payload.get("thinking").is_none(), "{provider}/{id}");
        assert_eq!(payload["output_config"], json!({"effort": "low"}));
    }
}

#[test]
fn adaptive_thinking_display_and_effort_per_model() {
    let cases = [
        ("claude-opus-5-5", ThinkingLevel::High, "omitted", "high"),
        ("claude-opus-4-7", ThinkingLevel::High, "summarized", "high"),
        (
            "claude-opus-4-7",
            ThinkingLevel::XHigh,
            "summarized",
            "xhigh",
        ),
        ("claude-opus-4-8", ThinkingLevel::High, "omitted", "high"),
        ("claude-opus-4-8", ThinkingLevel::XHigh, "omitted", "xhigh"),
    ];
    for (id, level, display, effort) in cases {
        let payload = thinking_payload("anthropic", id, reasoning(level));
        assert_eq!(
            payload["thinking"],
            json!({"type": "adaptive", "display": display}),
            "{id}"
        );
        assert_eq!(payload["output_config"], json!({"effort": effort}), "{id}");
    }
}

#[test]
fn explicit_thinking_display_overrides_the_default() {
    let payload = thinking_payload(
        "anthropic",
        "claude-opus-4-8",
        SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            thinking_display: Some(ThinkingDisplay::Summarized),
            ..Default::default()
        },
    );
    assert_eq!(
        payload["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(payload["output_config"], json!({"effort": "high"}));
}

// --- buildParams / streamSimpleAnthropic behaviour not covered by a TS test ---

#[test]
fn budget_models_get_enabled_thinking_and_adjusted_max_tokens() {
    // claude-sonnet-4-5: maxTokens 64000 -> base 32000; high budget 16384.
    let payload = thinking_payload(
        "anthropic",
        "claude-sonnet-4-5",
        reasoning(ThinkingLevel::High),
    );
    assert_eq!(
        payload["thinking"],
        json!({"type": "enabled", "budget_tokens": 16384, "display": "summarized"})
    );
    assert_eq!(payload["max_tokens"], 32000 + 16384);
    // xhigh uses the high budget (clampReasoning).
    let payload = thinking_payload(
        "anthropic",
        "claude-sonnet-4-5",
        reasoning(ThinkingLevel::XHigh),
    );
    assert_eq!(payload["thinking"]["budget_tokens"], 16384);
}

#[test]
fn thinking_level_map_picks_the_effort() {
    // claude-opus-4-6 maps xhigh -> "max".
    let payload = thinking_payload(
        "anthropic",
        "claude-opus-4-6",
        reasoning(ThinkingLevel::XHigh),
    );
    assert_eq!(payload["output_config"], json!({"effort": "max"}));
    let payload = thinking_payload(
        "anthropic",
        "claude-opus-4-6",
        reasoning(ThinkingLevel::Minimal),
    );
    assert_eq!(payload["output_config"], json!({"effort": "low"}));
}

#[test]
fn temperature_only_without_thinking() {
    let with = |model: &str, level: Option<ThinkingLevel>| {
        thinking_payload(
            "anthropic",
            model,
            SimpleStreamOptions {
                temperature: Some(0.0),
                reasoning: level,
                ..Default::default()
            },
        )
    };
    assert_eq!(with("claude-sonnet-4-5", None)["temperature"], json!(0.0));
    assert!(with("claude-sonnet-4-5", Some(ThinkingLevel::Low))
        .get("temperature")
        .is_none());
    // Always-on thinking models never take a temperature.
    assert!(with("claude-opus-5-5", None).get("temperature").is_none());
}

#[test]
fn oauth_tokens_get_claude_code_identity_and_tool_names() {
    let model = catalog("anthropic", "claude-sonnet-4-5");
    let mut context = hello();
    context.tools = vec![Tool {
        name: "todowrite".into(),
        description: "Write todos".into(),
        parameters: json!({"type": "object", "properties": {}}),
        defer_loading: None,
    }];
    let (headers, params) = simple_request(
        &model,
        &context,
        "sk-ant-oat01-token",
        SimpleStreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        },
    );
    assert_eq!(
        header(&headers, "authorization"),
        Some("Bearer sk-ant-oat01-token")
    );
    assert_eq!(
        header(&headers, "anthropic-beta"),
        Some("claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14")
    );
    assert_eq!(header(&headers, "user-agent"), Some("claude-cli/2.1.280"));
    assert_eq!(header(&headers, "x-app"), Some("cli"));
    assert_eq!(
        params["system"],
        json!([
            {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
            {"type": "text", "text": "You are a helpful assistant."},
        ])
    );
    assert_eq!(params["tools"][0]["name"], "TodoWrite");
    assert_eq!(
        from_claude_code_name("TodoWrite", &context.tools),
        "todowrite"
    );
    // No CC tool is named "find": it passes through unchanged both ways.
    assert_eq!(to_claude_code_name("find"), "find");
    assert_eq!(to_claude_code_name("read"), "Read");
}

#[test]
fn api_key_requests_default_to_max_tokens_over_three_without_options() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let params = build_params(&model, &hello(), false, &AnthropicOptions::default());
    assert_eq!(params["max_tokens"], 64000 / 3);
    assert!(params.get("thinking").is_none());
    // Default retention is long: 1h TTL on the system prompt and last user turn.
    let cc = json!({"type": "ephemeral", "ttl": "1h"});
    assert_eq!(params["system"][0]["cache_control"], cc);
    assert_eq!(
        params["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "Hello", "cache_control": cc}]}])
    );
}

#[test]
fn tool_choice_and_metadata_user_id() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let options = AnthropicOptions {
        tool_choice: Some(json!("any")),
        metadata: Some(json!({"user_id": "u1", "other": 1})),
        ..Default::default()
    };
    let params = build_params(&model, &hello(), false, &options);
    assert_eq!(params["tool_choice"], json!({"type": "any"}));
    assert_eq!(params["metadata"], json!({"user_id": "u1"}));
    let options = AnthropicOptions {
        tool_choice: Some(json!({"type": "tool", "name": "read"})),
        metadata: Some(json!({"user_id": 5})),
        ..Default::default()
    };
    let params = build_params(&model, &hello(), false, &options);
    assert_eq!(
        params["tool_choice"],
        json!({"type": "tool", "name": "read"})
    );
    assert!(params.get("metadata").is_none());
}

// --- anthropic-tool-search.test.ts ---

fn tool(name: &str, defer_loading: Option<bool>) -> Tool {
    Tool {
        name: name.into(),
        description: format!("Do {name}"),
        parameters: json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
        }),
        defer_loading,
    }
}

/// `createModel` of the TS tests: claude-opus-4-7 on a test provider.
pub(crate) fn test_model(base_url: &str, compat: Option<Value>) -> Model {
    Model {
        id: "claude-opus-4-7".into(),
        name: "Claude Opus 4.7".into(),
        api: "anthropic-messages".into(),
        provider: "test-anthropic".into(),
        base_url: base_url.into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 200_000,
        max_tokens: 32_000,
        headers: None,
        compat,
    }
}

fn capture_tools(tools: Vec<Tool>, compat: Option<Value>, retention: CacheRetention) -> Vec<Value> {
    let model = test_model("http://127.0.0.1:9", compat);
    let context = Context::new(String::new(), vec![user("Use a tool")], tools);
    let options = AnthropicOptions {
        api_key: Some("test-key".into()),
        cache_retention: Some(retention),
        ..Default::default()
    };
    let params = build_params(&model, &context, false, &options);
    params["tools"].as_array().cloned().unwrap_or_default()
}

const SEARCH_TOOL_TYPE: &str = "tool_search_tool_bm25_20251119";

#[test]
fn tool_search_changes_nothing_when_no_tool_opts_in() {
    let tools = capture_tools(
        vec![tool("alpha", None), tool("beta", None)],
        None,
        CacheRetention::None,
    );
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().all(|t| t.get("defer_loading").is_none()));
    assert!(!tools.iter().any(|t| t["type"] == SEARCH_TOOL_TYPE));
}

#[test]
fn tool_search_marks_only_opted_in_tools_and_appends_the_search_tool() {
    let tools = capture_tools(
        vec![tool("eager", None), tool("heavy", Some(true))],
        None,
        CacheRetention::None,
    );
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["name"], "eager");
    assert!(tools[0].get("defer_loading").is_none());
    assert_eq!(tools[1]["name"], "heavy");
    assert_eq!(tools[1]["defer_loading"], true);
    assert_eq!(
        tools[2],
        json!({"type": SEARCH_TOOL_TYPE, "name": "tool_search_tool_bm25"})
    );
}

#[test]
fn tool_search_keeps_a_non_deferred_tool_when_every_caller_tool_defers() {
    let tools = capture_tools(
        vec![tool("a", Some(true)), tool("b", Some(true))],
        None,
        CacheRetention::None,
    );
    assert_eq!(
        tools.iter().filter(|t| t["defer_loading"] == true).count(),
        2
    );
    assert!(tools.iter().any(|t| t.get("defer_loading").is_none()));
    assert_eq!(tools.last().unwrap()["type"], SEARCH_TOOL_TYPE);
}

#[test]
fn tool_search_degrades_to_eager_schemas_when_the_endpoint_cannot_defer() {
    let tools = capture_tools(
        vec![tool("heavy", Some(true))],
        Some(json!({"supportsToolSearch": false})),
        CacheRetention::None,
    );
    assert_eq!(tools.len(), 1);
    assert!(tools[0].get("defer_loading").is_none());
}

#[test]
fn tool_search_puts_the_cache_breakpoint_on_the_last_tool() {
    let with_search = capture_tools(vec![tool("heavy", Some(true))], None, CacheRetention::Short);
    let last = with_search.last().unwrap();
    assert_eq!(last["type"], SEARCH_TOOL_TYPE);
    assert_eq!(last["cache_control"], json!({"type": "ephemeral"}));
    assert!(with_search[0].get("cache_control").is_none());

    let without = capture_tools(
        vec![tool("a", None), tool("b", None)],
        None,
        CacheRetention::Short,
    );
    assert_eq!(without[1]["name"], "b");
    assert_eq!(without[1]["cache_control"], json!({"type": "ephemeral"}));
    assert!(without[0].get("cache_control").is_none());
}

#[test]
fn tools_carry_eager_input_streaming_and_a_normalized_input_schema() {
    let tools = capture_tools(vec![tool("lookup", None)], None, CacheRetention::None);
    assert_eq!(
        tools[0],
        json!({
            "name": "lookup",
            "description": "Do lookup",
            "eager_input_streaming": true,
            "input_schema": {
                "type": "object",
                "properties": {"value": {"type": "string"}},
                "required": ["value"],
            },
        })
    );
}

// --- convertMessages ---

fn assistant(content: Vec<Content>) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        model: "claude-haiku-4-5".into(),
        stop_reason: cortexcode_ai_types::StopReason::ToolUse,
        ..Default::default()
    })
}

fn tool_result(id: &str, content: Vec<Content>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        content,
        details: None,
        is_error: false,
        timestamp: 2,
    })
}

#[test]
fn converts_turns_and_merges_consecutive_tool_results() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let image = Content::Image(ImageContent {
        data: "aGk=".into(),
        media_type: "image/png".into(),
    });
    let messages = vec![
        user("   "),
        Message::User(UserMessage {
            content: vec![Content::text("look"), Content::text(" "), image.clone()].into(),
            timestamp: 1,
        }),
        assistant(vec![
            Content::Thinking(ThinkingContent {
                thinking: "plan".into(),
                signature: Some("sig".into()),
                redacted: false,
            }),
            Content::Thinking(ThinkingContent {
                thinking: "unsigned".into(),
                signature: Some(" ".into()),
                redacted: false,
            }),
            Content::Thinking(ThinkingContent {
                thinking: "[Reasoning redacted]".into(),
                signature: Some("opaque".into()),
                redacted: true,
            }),
            Content::Text(TextContent::new("")),
            Content::ToolCall(ToolCallContent {
                id: "call|1".into(),
                name: "read".into(),
                arguments: json!({"path": "a"}),
                thought_signature: None,
            }),
            Content::ToolCall(ToolCallContent {
                id: "call_2".into(),
                name: "read".into(),
                arguments: json!({"path": "b"}),
                thought_signature: None,
            }),
        ]),
        tool_result("call|1", vec![Content::text("one"), Content::text("two")]),
        tool_result("call_2", vec![image]),
    ];
    let converted = convert_messages(&messages, &model, false, None);
    assert_eq!(
        converted,
        vec![
            json!({"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGk="}},
            ]}),
            json!({"role": "assistant", "content": [
                {"type": "thinking", "thinking": "plan", "signature": "sig"},
                {"type": "text", "text": "unsigned"},
                {"type": "redacted_thinking", "data": "opaque"},
                // Same model: transformMessages leaves ids alone.
                {"type": "tool_use", "id": "call|1", "name": "read", "input": {"path": "a"}},
                {"type": "tool_use", "id": "call_2", "name": "read", "input": {"path": "b"}},
            ]}),
            json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call|1", "content": "one\ntwo", "is_error": false},
                {"type": "tool_result", "tool_use_id": "call_2", "content": [
                    {"type": "text", "text": "(see attached image)"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "aGk="}},
                ], "is_error": false},
            ]}),
        ]
    );
}

#[test]
fn string_user_content_stays_a_string_until_it_takes_the_cache_marker() {
    let model = catalog("anthropic", "claude-haiku-4-5");
    let messages = vec![
        user("first"),
        assistant(vec![Content::text("ok")]),
        user("second"),
    ];
    let cc = json!({"type": "ephemeral"});
    let converted = convert_messages(&messages, &model, false, Some(&cc));
    assert_eq!(converted[0], json!({"role": "user", "content": "first"}));
    assert_eq!(
        converted[2],
        json!({"role": "user", "content": [{"type": "text", "text": "second", "cache_control": cc}]})
    );
    // No marker after an assistant turn.
    let converted = convert_messages(&messages[..2], &model, false, Some(&cc));
    assert!(converted[1]["content"][0].get("cache_control").is_none());
}

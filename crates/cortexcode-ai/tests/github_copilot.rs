//! GitHub Copilot routing: Copilot serves Claude over `anthropic-messages`
//! and GPT/other models over `openai-responses` / `openai-completions`, all
//! with Bearer auth, the catalog's static Copilot headers and the per-request
//! dynamic headers (`X-Initiator`, `Openai-Intent`, `Copilot-Vision-Request`).
//!
//! Ledger task 8.4b. Ported from hoocode v0.5.89
//! `github-copilot-anthropic.test.ts` (the TS test mocks the Anthropic SDK
//! client; here the same `build_headers` / `build_params` pipeline runs
//! without HTTP) and `transform-messages-copilot-openai-to-anthropic.test.ts`.
//! `github-copilot-oauth.test.ts` lives in `cortexcode-ai-oauth-github-copilot`
//! and `openai-responses-copilot-provider.test.ts` in
//! `cortexcode-ai-provider-openai-responses`.

use std::collections::HashMap;

use cortexcode_ai::provider_anthropic::{build_headers, build_params, AnthropicOptions};
use cortexcode_ai::registry::stream_simple;
use cortexcode_ai::types::{
    AssistantMessage, Content, Context, ImageContent, Message, Model, ModelCost,
    SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent,
    ToolResultMessage, UserMessage,
};
use cortexcode_ai::util::transform_messages;
use cortexcode_ai_stream::testing::serve_script;
use serde_json::{json, Value};

fn copilot_headers() -> HashMap<String, String> {
    [
        ("User-Agent", "GitHubCopilotChat/0.35.0"),
        ("Editor-Version", "vscode/1.107.0"),
        ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
        ("Copilot-Integration-Id", "vscode-chat"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

/// `makeCopilotAnthropicModel`.
fn copilot_anthropic_model(id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        api: "anthropic-messages".into(),
        provider: "github-copilot".into(),
        base_url: "https://api.individual.githubcopilot.com".into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into(), "image".into()],
        cost: ModelCost::default(),
        context_window: 128000,
        max_tokens: 16000,
        headers: Some(copilot_headers()),
        compat: None,
    }
}

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: text.into(),
        timestamp: 1,
    })
}

fn hello() -> Context {
    Context::new(
        "You are a helpful assistant.".into(),
        vec![user("Hello")],
        vec![],
    )
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `streamAnthropic(model, context, options)`'s client headers and params.
fn anthropic_request(model: &Model, options: AnthropicOptions) -> (Vec<(String, String)>, Value) {
    let api_key = "tid_copilot_session_test_token";
    let options = AnthropicOptions {
        api_key: Some(api_key.into()),
        ..options
    };
    let (headers, oauth) = build_headers(model, &hello(), api_key, &options);
    let params = build_params(model, &hello(), oauth, &options);
    (headers, params)
}

// --- github-copilot-anthropic.test.ts ---

#[test]
fn uses_bearer_auth_copilot_headers_and_a_valid_messages_payload() {
    let model = copilot_anthropic_model("claude-sonnet-4.5", "Claude Sonnet 4.5");
    let (headers, params) = anthropic_request(&model, AnthropicOptions::default());

    // `apiKey: null, authToken: …`.
    assert_eq!(header(&headers, "x-api-key"), None);
    assert_eq!(
        header(&headers, "authorization"),
        Some("Bearer tid_copilot_session_test_token")
    );
    // Static headers from `model.headers`.
    assert!(header(&headers, "User-Agent")
        .unwrap()
        .contains("GitHubCopilotChat"));
    assert_eq!(
        header(&headers, "Copilot-Integration-Id"),
        Some("vscode-chat")
    );
    // Dynamic headers.
    assert_eq!(header(&headers, "X-Initiator"), Some("user"));
    assert_eq!(
        header(&headers, "Openai-Intent"),
        Some("conversation-edits")
    );
    // No fine-grained tool streaming beta (Copilot does not support it).
    assert!(!header(&headers, "anthropic-beta")
        .unwrap_or_default()
        .contains("fine-grained-tool-streaming"));

    assert_eq!(params["model"], "claude-sonnet-4.5");
    assert_eq!(params["stream"], true);
    assert!(params["max_tokens"].as_u64().unwrap() > 0);
    assert!(params["messages"].is_array());
}

#[test]
fn uses_adaptive_thinking_not_enabled_for_copilot_claude_opus_4_8() {
    // Regression: `"thinking.type.enabled" is not supported for this model.`
    let model = copilot_anthropic_model("claude-opus-4.8", "Claude Opus 4.8");
    let (_, params) = anthropic_request(
        &model,
        AnthropicOptions {
            thinking_enabled: Some(true),
            effort: Some("high".into()),
            ..Default::default()
        },
    );
    assert_eq!(
        params["thinking"],
        json!({"type": "adaptive", "display": "omitted"})
    );
    assert_eq!(params["output_config"], json!({"effort": "high"}));
}

#[test]
fn includes_the_interleaved_thinking_beta_when_reasoning_is_enabled() {
    let model = copilot_anthropic_model("claude-sonnet-4.5", "Claude Sonnet 4.5");
    let (headers, _) = anthropic_request(
        &model,
        AnthropicOptions {
            interleaved_thinking: Some(true),
            ..Default::default()
        },
    );
    assert!(header(&headers, "anthropic-beta")
        .unwrap()
        .contains("interleaved-thinking-2025-05-14"));
}

// --- transform-messages-copilot-openai-to-anthropic.test.ts ---

/// The normalizer `anthropic.ts` passes to `transformMessages`.
fn anthropic_normalize_tool_call_id(id: &str, _: &Model, _: &AssistantMessage) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn copilot_claude_model() -> Model {
    Model {
        headers: None,
        ..copilot_anthropic_model("claude-sonnet-4.5", "Claude Sonnet 4")
    }
}

fn assistant(api: &str, model: &str, content: Vec<Content>, stop: StopReason) -> Message {
    Message::Assistant(AssistantMessage {
        content,
        api: api.into(),
        provider: "github-copilot".into(),
        model: model.into(),
        stop_reason: stop,
        timestamp: 1,
        ..Default::default()
    })
}

fn tool_call(id: &str, name: &str, arguments: Value) -> Content {
    Content::ToolCall(ToolCallContent {
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
    })
}

fn tool_result(id: &str, name: &str, text: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: name.into(),
        content: vec![Content::Text(TextContent::new(text))],
        details: None,
        is_error: false,
        timestamp: 1,
    })
}

fn transform(messages: &[Message]) -> Vec<Message> {
    transform_messages(
        messages,
        &copilot_claude_model(),
        Some(&anthropic_normalize_tool_call_id),
    )
}

fn first_assistant(messages: &[Message]) -> &AssistantMessage {
    messages
        .iter()
        .find_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
        .expect("an assistant message")
}

#[test]
fn converts_thinking_blocks_to_plain_text_when_the_source_model_differs() {
    let messages = vec![
        user("hello"),
        assistant(
            "openai-completions",
            "gpt-4o",
            vec![
                Content::Thinking(ThinkingContent {
                    thinking: "Let me think about this...".into(),
                    signature: Some("reasoning_content".into()),
                    redacted: false,
                }),
                Content::Text(TextContent::new("Hi there!")),
            ],
            StopReason::Stop,
        ),
    ];
    let result = transform(&messages);
    let content = &first_assistant(&result).content;
    assert!(!content.iter().any(|b| matches!(b, Content::Thinking(_))));
    assert!(
        content
            .iter()
            .filter(|b| matches!(b, Content::Text(_)))
            .count()
            >= 2
    );
}

#[test]
fn removes_thought_signature_from_tool_calls_when_migrating_between_models() {
    let call = ToolCallContent {
        id: "call_123".into(),
        name: "bash".into(),
        arguments: json!({"command": "ls"}),
        thought_signature: Some(
            json!({"type": "reasoning.encrypted", "id": "call_123", "data": "encrypted"})
                .to_string(),
        ),
    };
    let messages = vec![
        user("run a command"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![Content::ToolCall(call)],
            StopReason::ToolUse,
        ),
        tool_result("call_123", "bash", "output"),
    ];
    let result = transform(&messages);
    let Some(Content::ToolCall(call)) = first_assistant(&result)
        .content
        .iter()
        .find(|b| matches!(b, Content::ToolCall(_)))
    else {
        panic!("a tool call")
    };
    assert_eq!(call.thought_signature, None);
}

fn assert_synthetic(message: &Message, id: &str, name: &str) {
    let Message::ToolResult(r) = message else {
        panic!("expected a tool result, got {message:?}")
    };
    assert_eq!(
        (r.tool_call_id.as_str(), r.tool_name.as_str(), r.is_error),
        (id, name, true)
    );
    assert_eq!(
        r.content,
        vec![Content::Text(TextContent::new("No result provided"))]
    );
}

#[test]
fn adds_synthetic_tool_results_for_trailing_orphaned_tool_calls() {
    let messages = vec![
        user("read the file"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![tool_call(
                "call_123|fc_123",
                "read",
                json!({"path": "README.md"}),
            )],
            StopReason::ToolUse,
        ),
    ];
    let result = transform(&messages);
    assert_synthetic(result.last().unwrap(), "call_123_fc_123", "read");
}

#[test]
fn adds_synthetic_results_only_for_trailing_calls_still_missing_results() {
    let messages = vec![
        user("run commands"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![
                tool_call("call_1|fc_1", "read", json!({"path": "README.md"})),
                tool_call("call_2|fc_2", "bash", json!({"command": "pwd"})),
            ],
            StopReason::ToolUse,
        ),
        tool_result("call_1|fc_1", "read", "done"),
    ];
    let result = transform(&messages);
    let synthetic: Vec<&Message> = result
        .iter()
        .filter(|m| matches!(m, Message::ToolResult(r) if r.is_error))
        .collect();
    assert_eq!(synthetic.len(), 1);
    assert_synthetic(synthetic[0], "call_2_fc_2", "bash");
}

// --- Routing: every Copilot backend carries the Copilot headers ---

/// The first catalog Copilot model on `api`.
fn copilot_model(api: &str) -> Model {
    cortexcode_ai::models::get_models("github-copilot")
        .into_iter()
        .find(|m| m.api == api)
        .unwrap_or_else(|| panic!("github-copilot has an {api} model"))
        .clone()
}

/// Stream `context` to a Copilot model on `api` against a mock server and
/// return the one request it made.
fn routed_request(api: &str, context: Context) -> cortexcode_ai_stream::testing::Recorded {
    let mut model = copilot_model(api);
    let server = serve_script(vec![(
        "HTTP/1.1 400 Bad Request",
        "application/json",
        r#"{"error":{"message":"routed"}}"#,
    )]);
    model.base_url = server.base_url.clone();
    let options = SimpleStreamOptions {
        api_key: Some("tid=copilot".into()),
        ..Default::default()
    };
    let message = stream_simple(model, context, options)
        .unwrap_or_else(|e| panic!("{api}: {e}"))
        .result_blocking();
    assert_eq!(message.stop_reason, StopReason::Error, "{api}");
    assert_eq!(message.provider, "github-copilot", "{api}");
    let requests = server.requests();
    assert_eq!(requests.len(), 1, "{api}");
    requests[0].clone()
}

const COPILOT_APIS: [&str; 3] = [
    "anthropic-messages",
    "openai-responses",
    "openai-completions",
];

#[test]
fn every_copilot_backend_sends_bearer_auth_and_copilot_headers() {
    for api in COPILOT_APIS {
        let request = routed_request(api, hello());
        assert_eq!(
            request.header("authorization"),
            Some("Bearer tid=copilot"),
            "{api}"
        );
        assert_eq!(request.header("x-api-key"), None, "{api}");
        for (name, value) in copilot_headers() {
            assert_eq!(request.header(&name), Some(value.as_str()), "{api} {name}");
        }
        assert_eq!(request.header("X-Initiator"), Some("user"), "{api}");
        assert_eq!(
            request.header("Openai-Intent"),
            Some("conversation-edits"),
            "{api}"
        );
        assert_eq!(request.header("Copilot-Vision-Request"), None, "{api}");
    }
}

#[test]
fn copilot_initiator_is_agent_after_a_tool_result_and_images_set_the_vision_header() {
    let image = Content::Image(ImageContent {
        data: "aGk=".into(),
        media_type: "image/png".into(),
    });
    let messages = vec![
        user("look"),
        assistant(
            "openai-responses",
            "gpt-5",
            vec![tool_call("call_1", "read", json!({"path": "a.png"}))],
            StopReason::ToolUse,
        ),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_1".into(),
            tool_name: "read".into(),
            content: vec![image],
            details: None,
            is_error: false,
            timestamp: 1,
        }),
    ];
    for api in COPILOT_APIS {
        let context = Context::new(String::new(), messages.clone(), vec![]);
        let request = routed_request(api, context);
        assert_eq!(request.header("X-Initiator"), Some("agent"), "{api}");
        assert_eq!(
            request.header("Copilot-Vision-Request"),
            Some("true"),
            "{api}"
        );
    }
}

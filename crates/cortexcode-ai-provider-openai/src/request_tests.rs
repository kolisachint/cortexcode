//! Request-shape tests. The TS tests capture the payload handed to the
//! `openai` SDK; here the same pipeline (`streamSimple` option mapping,
//! `getCompat`, `resolveCacheRetention`, `buildParams`) runs without HTTP.
//!
//! Ported from `openai-completions-cache-control-format.test.ts`,
//! `-empty-tools.test.ts`, `-prompt-cache.test.ts`, `-prompt-suffix.test.ts`,
//! `-thinking-as-text.test.ts`, `-tool-result-images.test.ts` and the payload
//! cases of `-tool-choice.test.ts` (hoocode v0.5.89).

use super::*;
use cortexcode_ai_types::{
    AssistantMessage, ImageContent, StopReason, TextContent, ThinkingContent, ToolCallContent,
    ToolResultMessage, UserMessage,
};
use std::sync::{Mutex, MutexGuard};

/// Serializes tests that read or set the cache-retention env vars.
pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn catalog(provider: &str, id: &str) -> Model {
    cortexcode_ai_models::get_model(provider, id)
        .unwrap_or_else(|| panic!("{provider}/{id} in catalog"))
        .clone()
}

/// `const { compat: _compat, ...baseModel } = getModel(...)` with
/// `api: "openai-completions"`.
pub(crate) fn completions(provider: &str, id: &str) -> Model {
    let mut m = catalog(provider, id);
    m.compat = None;
    m.api = "openai-completions".into();
    m
}

fn custom_model(provider: &str, base_url: &str, reasoning: bool, compat: Option<Value>) -> Model {
    Model {
        id: "custom".into(),
        name: "Custom".into(),
        api: "openai-completions".into(),
        provider: provider.into(),
        base_url: base_url.into(),
        reasoning,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 128_000,
        max_tokens: 32_000,
        headers: None,
        compat,
    }
}

pub(crate) fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: vec![Content::Text(TextContent::new(text))],
        timestamp: 1,
    })
}

fn tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("{name} tool"),
        parameters: json!({
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"]
        }),
    }
}

/// The payload `streamSimple` would send.
fn payload(model: &Model, context: &Context, options: SimpleStreamOptions) -> Value {
    let options = simple_options(model, &options, "test".into());
    let compat = get_compat(model);
    let retention = cortexcode_ai_util::resolve_cache_retention(options.cache_retention);
    build_params(model, context, &options, &compat, &retention)
}

fn with_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = env_lock();
    let saved: Vec<_> = ["CORTEXCODE_CACHE_RETENTION", "HOOCODE_CACHE_RETENTION"]
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
        .collect();
    std::env::remove_var("CORTEXCODE_CACHE_RETENTION");
    match value {
        Some(v) => std::env::set_var("HOOCODE_CACHE_RETENTION", v),
        None => std::env::remove_var("HOOCODE_CACHE_RETENTION"),
    }
    let out = f();
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    out
}

// --- openai-completions-cache-control-format.test.ts ---

fn cache_context() -> Context {
    Context::new(
        "System prompt".into(),
        vec![user("Hello")],
        vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
        }],
    )
}

fn expect_anthropic_cache_markers(params: &Value) {
    // Default cache retention is "long" → ttl 1h on supported models.
    let expected = json!({"type": "ephemeral", "ttl": "1h"});
    let messages = params["messages"].as_array().unwrap();
    let instruction = messages
        .iter()
        .find(|m| m["role"] == "system" || m["role"] == "developer")
        .expect("instruction message");
    assert_eq!(instruction["content"][0]["cache_control"], expected);
    assert_eq!(params["tools"].as_array().unwrap().len(), 1);
    assert_eq!(params["tools"][0]["cache_control"], expected);
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "user");
    assert_eq!(last["content"][0]["cache_control"], expected);
}

#[test]
fn applies_anthropic_cache_markers_when_model_compat_enables_them() {
    let model = custom_model(
        "openrouter",
        "https://example.com/v1",
        true,
        Some(json!({"cacheControlFormat": "anthropic"})),
    );
    let params = with_env(None, || {
        payload(&model, &cache_context(), SimpleStreamOptions::default())
    });
    expect_anthropic_cache_markers(&params);
}

#[test]
fn preserves_anthropic_cache_markers_for_openrouter_anthropic_models() {
    let model = catalog("openrouter", "anthropic/claude-sonnet-4");
    let params = with_env(None, || {
        payload(&model, &cache_context(), SimpleStreamOptions::default())
    });
    expect_anthropic_cache_markers(&params);
}

#[test]
fn omits_anthropic_cache_markers_when_cache_retention_is_none() {
    let model = custom_model(
        "openrouter",
        "https://example.com/v1",
        true,
        Some(json!({"cacheControlFormat": "anthropic"})),
    );
    let params = payload(
        &model,
        &cache_context(),
        SimpleStreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        },
    );
    let messages = params["messages"].as_array().unwrap();
    assert!(messages[0]["content"].is_string());
    assert!(params["tools"][0].get("cache_control").is_none());
    // TS keeps the string content of `content: "Hello"`; UserMessage keeps
    // blocks, so the check is that no part got a marker.
    assert!(messages.last().unwrap()["content"][0]
        .get("cache_control")
        .is_none());
}

// --- openai-completions-empty-tools.test.ts ---

#[test]
fn omits_tools_field_when_context_tools_is_empty() {
    let model = completions("openai", "gpt-4o-mini");
    let params = payload(
        &model,
        &Context::new(String::new(), vec![user("hi")], vec![]),
        SimpleStreamOptions::default(),
    );
    assert!(params.get("tools").is_none());
}

#[test]
fn still_emits_empty_tools_when_conversation_has_tool_history() {
    let model = completions("openai", "gpt-4o-mini");
    let messages = vec![
        user("use the tool"),
        Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCallContent {
                id: "t1".into(),
                name: "noop".into(),
                arguments: json!({}),
                thought_signature: None,
            })],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-4o-mini".into(),
            stop_reason: StopReason::ToolUse,
            ..Default::default()
        }),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "t1".into(),
            tool_name: "noop".into(),
            content: vec![Content::Text(TextContent::new("done"))],
            details: None,
            is_error: false,
            timestamp: 0,
        }),
    ];
    let params = payload(
        &model,
        &Context::new(String::new(), messages, vec![]),
        SimpleStreamOptions::default(),
    );
    assert_eq!(params["tools"], json!([]));
}

// --- openai-completions-prompt-cache.test.ts ---

fn cache_request(
    model: &Model,
    retention: Option<CacheRetention>,
    session_id: &str,
    headers: Option<HashMap<String, String>>,
) -> (Value, Vec<(String, String)>) {
    let options = SimpleStreamOptions {
        cache_retention: retention,
        session_id: Some(session_id.into()),
        headers,
        ..Default::default()
    };
    let context = Context::new("sys".into(), vec![user("hi")], vec![]);
    let params = payload(model, &context, options.clone());
    let compat = get_compat(model);
    let retention = cortexcode_ai_util::resolve_cache_retention(options.cache_retention);
    let cache_session = (retention != CacheRetention::None)
        .then_some(options.session_id.as_deref())
        .flatten();
    let headers = build_headers(
        model,
        &context,
        "test-key",
        options.headers.as_ref(),
        cache_session,
        &compat,
    );
    (params, headers)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn with_base_url(mut model: Model, base_url: &str, compat: Value) -> Model {
    model.base_url = base_url.into();
    model.compat = Some(compat);
    model
}

#[test]
fn sets_prompt_cache_key_and_24h_retention_for_direct_openai_by_default() {
    let (p, _) = with_env(None, || {
        cache_request(
            &completions("openai", "gpt-4o-mini"),
            None,
            "session-123",
            None,
        )
    });
    assert_eq!(p["prompt_cache_key"], "session-123");
    assert_eq!(p["prompt_cache_retention"], "24h");
}

#[test]
fn omits_prompt_cache_retention_when_env_is_short() {
    let (p, _) = with_env(Some("short"), || {
        cache_request(
            &completions("openai", "gpt-4o-mini"),
            None,
            "session-short",
            None,
        )
    });
    assert_eq!(p["prompt_cache_key"], "session-short");
    assert!(p.get("prompt_cache_retention").is_none());
}

#[test]
fn sets_24h_retention_for_direct_openai_when_long() {
    let (p, _) = cache_request(
        &completions("openai", "gpt-4o-mini"),
        Some(CacheRetention::Long),
        "session-456",
        None,
    );
    assert_eq!(p["prompt_cache_key"], "session-456");
    assert_eq!(p["prompt_cache_retention"], "24h");
}

#[test]
fn omits_prompt_cache_fields_when_none() {
    let (p, _) = cache_request(
        &completions("openai", "gpt-4o-mini"),
        Some(CacheRetention::None),
        "session-789",
        None,
    );
    assert!(p.get("prompt_cache_key").is_none());
    assert!(p.get("prompt_cache_retention").is_none());
}

#[test]
fn omits_prompt_cache_fields_for_proxies_without_long_retention() {
    let model = with_base_url(
        completions("openai", "gpt-4o-mini"),
        "https://proxy.example.com/v1",
        json!({"supportsLongCacheRetention": false}),
    );
    let (p, _) = cache_request(&model, Some(CacheRetention::Long), "session-proxy", None);
    assert!(p.get("prompt_cache_key").is_none());
    assert!(p.get("prompt_cache_retention").is_none());
}

#[test]
fn uses_env_long_for_direct_openai() {
    let (p, _) = with_env(Some("long"), || {
        cache_request(
            &completions("openai", "gpt-4o-mini"),
            None,
            "session-env",
            None,
        )
    });
    assert_eq!(p["prompt_cache_key"], "session-env");
    assert_eq!(p["prompt_cache_retention"], "24h");
}

fn affinity_model() -> Model {
    with_base_url(
        completions("openai", "gpt-4o-mini"),
        "https://proxy.example.com/v1",
        json!({"sendSessionAffinityHeaders": true}),
    )
}

#[test]
fn sends_session_affinity_headers_when_compat_enables_them() {
    let (_, h) = with_env(None, || {
        cache_request(&affinity_model(), None, "session-affinity", None)
    });
    assert_eq!(header(&h, "session_id"), Some("session-affinity"));
    assert_eq!(header(&h, "x-client-request-id"), Some("session-affinity"));
    assert_eq!(header(&h, "x-session-affinity"), Some("session-affinity"));
}

#[test]
fn omits_session_affinity_headers_when_cache_retention_is_none() {
    let (_, h) = cache_request(
        &affinity_model(),
        Some(CacheRetention::None),
        "session-affinity",
        None,
    );
    assert_eq!(header(&h, "session_id"), None);
    assert_eq!(header(&h, "x-client-request-id"), None);
    assert_eq!(header(&h, "x-session-affinity"), None);
}

#[test]
fn explicit_headers_override_session_affinity_headers() {
    let overrides: HashMap<String, String> = [
        ("session_id", "override-session"),
        ("x-client-request-id", "override-request"),
        ("x-session-affinity", "override-affinity"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let (_, h) = with_env(None, || {
        cache_request(&affinity_model(), None, "session-affinity", Some(overrides))
    });
    assert_eq!(header(&h, "session_id"), Some("override-session"));
    assert_eq!(header(&h, "x-client-request-id"), Some("override-request"));
    assert_eq!(header(&h, "x-session-affinity"), Some("override-affinity"));
    assert_eq!(
        h.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("session_id"))
            .count(),
        1
    );
}

// --- openai-completions-prompt-suffix.test.ts ---

fn last_user_text(params: &Value) -> String {
    let last = params["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .unwrap();
    match &last["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

#[test]
fn appends_prompt_suffix_to_last_user_message() {
    let context = Context::new(String::new(), vec![user("summarize this")], vec![]);
    let model = custom_model(
        "suffix-provider",
        "http://127.0.0.1:1",
        false,
        Some(json!({"promptSuffix": "/no_think"})),
    );
    let p = payload(&model, &context, SimpleStreamOptions::default());
    assert_eq!(last_user_text(&p), "summarize this /no_think");
    let model = custom_model("suffix-provider", "http://127.0.0.1:1", false, None);
    let p = payload(&model, &context, SimpleStreamOptions::default());
    assert_eq!(last_user_text(&p), "summarize this");
}

// --- openai-completions-thinking-as-text.test.ts ---

fn thinking_compat() -> ResolvedCompat {
    ResolvedCompat {
        supports_store: true,
        supports_developer_role: true,
        supports_reasoning_effort: true,
        supports_usage_in_streaming: true,
        max_tokens_field: "max_completion_tokens".into(),
        requires_tool_result_name: false,
        requires_assistant_after_tool_result: false,
        requires_thinking_as_text: true,
        requires_reasoning_content_on_assistant_messages: false,
        thinking_format: "openai".into(),
        open_router_routing: json!({}),
        vercel_gateway_routing: json!({}),
        zai_tool_stream: false,
        supports_strict_mode: true,
        tool_call_constraint: "none".into(),
        cache_control_format: None,
        send_session_affinity_headers: false,
        supports_long_cache_retention: true,
        prompt_suffix: None,
    }
}

pub(crate) fn repro_model(base_url: &str) -> Model {
    Model {
        id: "repro-model".into(),
        name: "Repro Model".into(),
        api: "openai-completions".into(),
        provider: "repro-provider".into(),
        base_url: base_url.into(),
        reasoning: true,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 128_000,
        max_tokens: 4096,
        headers: None,
        compat: Some(json!({"requiresThinkingAsText": true})),
    }
}

pub(crate) fn repro_context(content: Vec<Content>) -> Context {
    Context::new(
        String::new(),
        vec![
            user("hello"),
            Message::Assistant(AssistantMessage {
                content,
                api: "openai-completions".into(),
                provider: "repro-provider".into(),
                model: "repro-model".into(),
                stop_reason: StopReason::Stop,
                timestamp: 2,
                ..Default::default()
            }),
            user("continue"),
        ],
        vec![],
    )
}

pub(crate) fn thinking(text: &str) -> Content {
    Content::Thinking(ThinkingContent {
        thinking: text.into(),
        signature: None,
        redacted: false,
    })
}

#[test]
fn serializes_same_model_thinking_plus_text_as_text_parts() {
    let messages = convert_messages(
        &repro_model("http://127.0.0.1:1"),
        &repro_context(vec![
            thinking("internal reasoning"),
            Content::Text(TextContent::new("visible answer")),
        ]),
        &thinking_compat(),
    );
    assert_eq!(
        messages[1],
        json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "internal reasoning"},
                {"type": "text", "text": "visible answer"}
            ]
        })
    );
}

#[test]
fn serializes_same_model_thinking_only_as_text_parts() {
    let messages = convert_messages(
        &repro_model("http://127.0.0.1:1"),
        &repro_context(vec![thinking("internal reasoning")]),
        &thinking_compat(),
    );
    assert_eq!(
        messages[1],
        json!({"role": "assistant", "content": [{"type": "text", "text": "internal reasoning"}]})
    );
}

// --- openai-completions-tool-result-images.test.ts ---

fn image_tool_result(id: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        content: vec![
            Content::Text(TextContent::new("Read image file [image/png]")),
            Content::Image(ImageContent {
                data: "ZmFrZQ==".into(),
                media_type: "image/png".into(),
            }),
        ],
        details: None,
        is_error: false,
        timestamp: 0,
    })
}

#[test]
fn batches_tool_result_images_after_consecutive_tool_results() {
    let mut model = completions("openai", "gpt-4o-mini");
    model.input = vec!["text".into(), "image".into()];
    let call = |id: &str, path: &str| {
        Content::ToolCall(ToolCallContent {
            id: id.into(),
            name: "read".into(),
            arguments: json!({"path": path}),
            thought_signature: None,
        })
    };
    let context = Context::new(
        String::new(),
        vec![
            user("Read the images"),
            Message::Assistant(AssistantMessage {
                content: vec![call("tool-1", "img-1.png"), call("tool-2", "img-2.png")],
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
            image_tool_result("tool-1"),
            image_tool_result("tool-2"),
        ],
        vec![],
    );
    let mut compat = thinking_compat();
    compat.requires_thinking_as_text = false;
    compat.cache_control_format = Some("anthropic".into());
    let messages = convert_messages(&model, &context, &compat);
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "tool", "tool", "user"]);
    let parts = messages.last().unwrap()["content"].as_array().unwrap();
    assert_eq!(parts.iter().filter(|p| p["type"] == "image_url").count(), 2);
}

// --- openai-completions-tool-choice.test.ts (payload cases) ---

fn hi() -> Context {
    Context::new(String::new(), vec![user("Hi")], vec![])
}

#[test]
fn forwards_tool_choice_from_simple_options() {
    let model = completions("openai", "gpt-4o-mini");
    let p = payload(
        &model,
        &Context::new(String::new(), vec![user("Call ping")], vec![tool("ping")]),
        SimpleStreamOptions {
            tool_choice: Some(json!("required")),
            ..Default::default()
        },
    );
    assert_eq!(p["tool_choice"], "required");
    assert!(!p["tools"].as_array().unwrap().is_empty());
}

#[test]
fn omits_strict_when_compat_disables_strict_mode() {
    let mut model = completions("openai", "gpt-4o-mini");
    model.compat = Some(json!({"supportsStrictMode": false}));
    let p = payload(
        &model,
        &Context::new(String::new(), vec![user("Call ping")], vec![tool("ping")]),
        SimpleStreamOptions::default(),
    );
    assert!(p["tools"][0]["function"].is_object());
    assert!(p["tools"][0]["function"].get("strict").is_none());
}

fn reasoning(level: ThinkingLevel) -> SimpleStreamOptions {
    SimpleStreamOptions {
        reasoning: Some(level),
        ..Default::default()
    }
}

#[test]
fn maps_groq_qwen3_reasoning_levels_to_default() {
    let p = payload(
        &catalog("groq", "qwen/qwen3.8-27b"),
        &hi(),
        reasoning(ThinkingLevel::Medium),
    );
    assert_eq!(p["reasoning_effort"], "default");
}

#[test]
fn keeps_normal_reasoning_effort_for_groq_without_mapping() {
    let p = payload(
        &catalog("groq", "openai/gpt-oss-20b"),
        &hi(),
        reasoning(ThinkingLevel::Medium),
    );
    assert_eq!(p["reasoning_effort"], "medium");
}

fn ping_context() -> Context {
    Context::new(String::new(), vec![user("Call ping")], vec![tool("ping")])
}

#[test]
fn enables_tool_stream_for_supported_zai_models_with_tools() {
    let p = payload(
        &catalog("zai", "glm-5.2"),
        &ping_context(),
        SimpleStreamOptions::default(),
    );
    assert_eq!(p["tool_stream"], true);
}

#[test]
fn stores_zai_tool_stream_support_in_model_compat() {
    for id in ["glm-5.2", "glm-4.7", "glm-5-turbo"] {
        assert_eq!(catalog("zai", id).compat.unwrap()["zaiToolStream"], true);
    }
}

#[test]
fn omits_tool_stream_for_unsupported_zai_models() {
    let mut model = catalog("zai", "glm-5.2");
    model.compat.as_mut().unwrap()["zaiToolStream"] = json!(false);
    let p = payload(&model, &ping_context(), SimpleStreamOptions::default());
    assert!(p.get("tool_stream").is_none());
}

#[test]
fn respects_explicit_zai_tool_stream_override() {
    let mut model = catalog("zai", "glm-5.2");
    model.compat.as_mut().unwrap()["zaiToolStream"] = json!(true);
    let p = payload(&model, &ping_context(), SimpleStreamOptions::default());
    assert_eq!(p["tool_stream"], true);
}

#[test]
fn omits_tool_stream_without_tools() {
    let p = payload(
        &catalog("zai", "glm-5.2"),
        &hi(),
        SimpleStreamOptions::default(),
    );
    assert!(p.get("tool_stream").is_none());
}

#[test]
fn uses_openrouter_reasoning_object_instead_of_reasoning_effort() {
    let p = payload(
        &catalog("openrouter", "deepseek/deepseek-r1"),
        &hi(),
        reasoning(ThinkingLevel::High),
    );
    assert_eq!(p["reasoning"], json!({"effort": "high"}));
    assert!(p.get("reasoning_effort").is_none());
}

// --- behaviour of buildParams / convertMessages not covered by a TS test ---

#[test]
fn local_endpoint_payload_matches_hoocode() {
    // What hoocode sends to the parity harness's mock provider.
    let model = Model {
        id: "mock-model".into(),
        name: "Mock Model".into(),
        api: "openai-completions".into(),
        provider: "mock".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 128_000,
        max_tokens: 4096,
        headers: None,
        compat: None,
    };
    let p = with_env(None, || {
        payload(
            &model,
            &Context::new("sys".into(), vec![user("hi")], vec![]),
            SimpleStreamOptions {
                session_id: Some("s1".into()),
                ..Default::default()
            },
        )
    });
    assert_eq!(
        p,
        json!({
            "model": "mock-model",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": [{"type": "text", "text": "hi"}]}
            ],
            "stream": true,
            "prompt_cache_key": "s1",
            "prompt_cache_retention": "24h",
            "stream_options": {"include_usage": true},
            "store": false,
            "max_completion_tokens": 4096
        })
    );
}

#[test]
fn detect_compat_for_known_providers() {
    let c = detect_compat(&catalog("moonshotai", "kimi-k2.6"));
    assert_eq!(c.max_tokens_field, "max_tokens");
    assert!(!c.supports_store && !c.supports_strict_mode && !c.supports_reasoning_effort);
    let c = detect_compat(&catalog("deepseek", "deepseek-v4-flash"));
    assert_eq!(c.thinking_format, "deepseek");
    assert!(c.requires_reasoning_content_on_assistant_messages);
    let c = detect_compat(&catalog("xai", "grok-4.3"));
    assert!(!c.supports_reasoning_effort && !c.supports_developer_role);
    let c = detect_compat(&completions("openai", "gpt-4o-mini"));
    assert_eq!(c.tool_call_constraint, "strict");
    let c = detect_compat(&catalog("together", "Qwen/Qwen3.5-9B"));
    assert_eq!(c.thinking_format, "together");
    assert!(!c.supports_long_cache_retention);
}

#[test]
fn deepseek_thinking_and_reasoning_content_replay() {
    let model = catalog("deepseek", "deepseek-v4-pro");
    let context = repro_context(vec![Content::Text(TextContent::new("answer"))]);
    let p = payload(&model, &context, reasoning(ThinkingLevel::High));
    assert_eq!(p["thinking"], json!({"type": "enabled"}));
    assert_eq!(p["reasoning_effort"], "high");
    assert_eq!(p["messages"][1]["reasoning_content"], "");
    let p = payload(&model, &context, SimpleStreamOptions::default());
    assert_eq!(p["thinking"], json!({"type": "disabled"}));
    assert!(p.get("reasoning_effort").is_none());
}

#[test]
fn strict_tools_use_closed_schemas_when_constrained() {
    let model = completions("openai", "gpt-4o-mini");
    let mut t = tool("ping");
    t.parameters = json!({"type": "object", "properties": {"n": {"type": "number", "minimum": 1}}});
    let p = payload(
        &model,
        &Context::new(String::new(), vec![user("x")], vec![t]),
        SimpleStreamOptions {
            constrain_tool_calls: Some(true),
            ..Default::default()
        },
    );
    assert_eq!(
        p["tools"][0]["function"],
        json!({
            "name": "ping",
            "description": "ping tool",
            "parameters": {
                "type": "object",
                "properties": {"n": {"type": ["number", "null"]}},
                "required": ["n"],
                "additionalProperties": false
            },
            "strict": true
        })
    );
}

#[test]
fn assistant_replay_uses_signature_field_and_bridges_tool_results() {
    let mut model = repro_model("http://127.0.0.1:1");
    model.compat =
        Some(json!({"requiresAssistantAfterToolResult": true, "requiresToolResultName": true}));
    let context = Context::new(
        String::new(),
        vec![
            user("q"),
            Message::Assistant(AssistantMessage {
                content: vec![
                    Content::Thinking(ThinkingContent {
                        thinking: "plan".into(),
                        signature: Some("reasoning_content".into()),
                        redacted: false,
                    }),
                    Content::ToolCall(ToolCallContent {
                        id: "c1".into(),
                        name: "read".into(),
                        arguments: json!({"path": "a"}),
                        thought_signature: None,
                    }),
                ],
                api: "openai-completions".into(),
                provider: "repro-provider".into(),
                model: "repro-model".into(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "c1".into(),
                tool_name: "read".into(),
                content: vec![Content::Text(TextContent::new("data"))],
                details: None,
                is_error: false,
                timestamp: 0,
            }),
            user("next"),
        ],
        vec![],
    );
    let messages = convert_messages(&model, &context, &get_compat(&model));
    assert_eq!(
        messages[1],
        json!({
            "role": "assistant",
            "content": "",
            "reasoning_content": "plan",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]
        })
    );
    assert_eq!(
        messages[2],
        json!({"role": "tool", "content": "data", "tool_call_id": "c1", "name": "read"})
    );
    assert_eq!(
        messages[3],
        json!({"role": "assistant", "content": "I have processed the tool results."})
    );
    assert_eq!(messages[4]["role"], "user");
}

#[test]
fn pipe_separated_tool_call_ids_are_normalized_for_other_models() {
    let model = completions("openai", "gpt-4o-mini");
    let long_id = format!("call_abc|{}", "x+/=".repeat(100));
    let context = Context::new(
        String::new(),
        vec![
            Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: long_id.clone(),
                    name: "read".into(),
                    arguments: json!({}),
                    thought_signature: None,
                })],
                api: "openai-responses".into(),
                provider: "openai".into(),
                model: "gpt-5".into(),
                stop_reason: StopReason::ToolUse,
                ..Default::default()
            }),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: long_id,
                tool_name: "read".into(),
                content: vec![Content::Text(TextContent::new("ok"))],
                details: None,
                is_error: false,
                timestamp: 0,
            }),
        ],
        vec![],
    );
    let messages = convert_messages(&model, &context, &get_compat(&model));
    assert_eq!(messages[0]["tool_calls"][0]["id"], "call_abc");
    assert_eq!(messages[1]["tool_call_id"], "call_abc");
}

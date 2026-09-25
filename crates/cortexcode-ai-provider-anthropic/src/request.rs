//! Request construction for the Anthropic Messages API.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/anthropic.ts`
//! (request-building portion).

use cortexcode_ai_types::{
    CacheRetention, Content, Context, Message, Model, SimpleStreamOptions, ThinkingBudgets,
    ThinkingLevel, Tool,
};

/// Resolved credential used to authenticate against the Anthropic API.
///
/// `ANTHROPIC_OAUTH_TOKEN` (Claude Pro/Max subscription auth) takes precedence
/// over `ANTHROPIC_API_KEY` and uses a different header scheme.
pub enum Credential {
    ApiKey(String),
    OAuth(String),
}

/// Resolve API credentials from explicit options, then environment variables.
pub fn resolve_credentials(options: &SimpleStreamOptions) -> Result<Credential, String> {
    if let Some(key) = &options.api_key {
        if !key.is_empty() {
            return Ok(Credential::ApiKey(key.clone()));
        }
    }
    if let Ok(token) = std::env::var("ANTHROPIC_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Ok(Credential::OAuth(token));
        }
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Ok(Credential::ApiKey(key));
        }
    }
    Err("no Anthropic credentials found (set ANTHROPIC_API_KEY or ANTHROPIC_OAUTH_TOKEN)".into())
}

/// Build the HTTP headers for an Anthropic Messages API request.
pub fn build_headers(model: &Model, cred: &Credential) -> Vec<(String, String)> {
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
    ];

    match cred {
        Credential::ApiKey(key) => headers.push(("x-api-key".to_string(), key.clone())),
        Credential::OAuth(token) => {
            headers.push(("authorization".to_string(), format!("Bearer {token}")));
            headers.push(("anthropic-beta".to_string(), "oauth-2025-04-20".to_string()));
        }
    }

    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            headers.push((k.clone(), v.clone()));
        }
    }

    headers
}

/// Build the JSON request body for the Anthropic Messages API.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> serde_json::Value {
    let cache_control = get_cache_control(model, options.cache_retention);
    let mut body = serde_json::json!({
        "model": model.id,
        "max_tokens": model.max_tokens,
        "messages": convert_messages(&context.messages, cache_control.as_ref()),
        "stream": true,
    });

    if let Some(system) = build_system(&context.system_prompt, cache_control.as_ref()) {
        body["system"] = system;
    }

    if !context.tools.is_empty() {
        body["tools"] =
            serde_json::Value::Array(convert_tools(&context.tools, cache_control.as_ref()));
    }

    if model.reasoning {
        if let Some(level) = &options.reasoning {
            if let Some(budget) = resolve_thinking_budget(level, options.thinking_budgets.as_ref())
            {
                body["thinking"] = serde_json::json!({"type": "enabled", "budget_tokens": budget});
                // Anthropic requires max_tokens to exceed the thinking budget.
                let max_tokens = body["max_tokens"].as_u64().unwrap_or(model.max_tokens);
                if max_tokens <= budget {
                    body["max_tokens"] = serde_json::json!(budget + model.max_tokens.max(1024));
                }
            }
        }
    }

    body
}

fn resolve_thinking_budget(
    level: &ThinkingLevel,
    budgets: Option<&ThinkingBudgets>,
) -> Option<u64> {
    let pick = |explicit: Option<u64>, fallback: u64| explicit.unwrap_or(fallback);
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some(pick(budgets.and_then(|b| b.minimal), 1024)),
        ThinkingLevel::Low => Some(pick(budgets.and_then(|b| b.low), 4096)),
        ThinkingLevel::Medium => Some(pick(budgets.and_then(|b| b.medium), 8192)),
        ThinkingLevel::High => Some(pick(budgets.and_then(|b| b.high), 16384)),
        ThinkingLevel::XHigh => Some(pick(budgets.and_then(|b| b.xhigh), 32768)),
    }
}

/// `getCacheControl`: the `cache_control` marker for the resolved retention,
/// or none when caching is off. Long retention asks for a 1h TTL unless the
/// model's `compat.supportsLongCacheRetention` is false.
fn get_cache_control(
    model: &Model,
    cache_retention: Option<CacheRetention>,
) -> Option<serde_json::Value> {
    match cortexcode_ai_util::resolve_cache_retention(cache_retention) {
        CacheRetention::None => None,
        CacheRetention::Short => Some(serde_json::json!({"type": "ephemeral"})),
        CacheRetention::Long => {
            let supports_long = model
                .compat
                .as_ref()
                .and_then(|c| c.get("supportsLongCacheRetention"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            Some(if supports_long {
                serde_json::json!({"type": "ephemeral", "ttl": "1h"})
            } else {
                serde_json::json!({"type": "ephemeral"})
            })
        }
    }
}

/// The system prompt as one text block, carrying the cache marker (the
/// non-OAuth branch of `buildParams`).
fn build_system(
    system_prompt: &str,
    cache_control: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    if system_prompt.is_empty() {
        return None;
    }
    let mut block = serde_json::json!({"type": "text", "text": system_prompt});
    if let Some(cc) = cache_control {
        block["cache_control"] = cc.clone();
    }
    Some(serde_json::json!([block]))
}

/// Convert user/tool-result content blocks (text + image only).
fn content_blocks(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(serde_json::json!({"type": "text", "text": t.text})),
            Content::Image(img) => Some(serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": img.media_type, "data": img.data},
            })),
            // Thinking/tool-call blocks never appear in user or tool-result content.
            Content::Thinking(_) | Content::ToolCall(_) => None,
        })
        .collect()
}

/// Convert assistant content blocks (text, thinking, tool-use, image).
fn assistant_content_blocks(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .map(|c| match c {
            Content::Text(t) => serde_json::json!({"type": "text", "text": t.text}),
            Content::Thinking(th) => {
                let mut v = serde_json::json!({"type": "thinking", "thinking": th.thinking});
                if let Some(sig) = &th.signature {
                    v["signature"] = serde_json::json!(sig);
                }
                v
            }
            Content::ToolCall(tc) => serde_json::json!({
                "type": "tool_use",
                "id": tc.id,
                "name": tc.name,
                "input": tc.arguments,
            }),
            Content::Image(img) => serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": img.media_type, "data": img.data},
            }),
        })
        .collect()
}

/// Convert tools; the cache marker goes on the last one (`convertTools`).
fn convert_tools(
    tools: &[Tool],
    cache_control: Option<&serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut converted: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.parameters,
            })
        })
        .collect();
    if let (Some(cc), Some(last)) = (cache_control, converted.last_mut()) {
        last["cache_control"] = cc.clone();
    }
    converted
}

/// Convert the context's message list into Anthropic API messages, merging
/// adjacent messages that resolve to the same role (Anthropic requires
/// alternating `user`/`assistant` turns; tool-result messages map to `user`).
fn convert_messages(
    messages: &[Message],
    cache_control: Option<&serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut raw: Vec<(&'static str, Vec<serde_json::Value>)> = Vec::new();

    for msg in messages {
        match msg {
            Message::User(m) => raw.push(("user", content_blocks(&m.content))),
            Message::Assistant(m) => raw.push(("assistant", assistant_content_blocks(&m.content))),
            Message::ToolResult(m) => {
                let block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id,
                    "is_error": m.is_error,
                    "content": content_blocks(&m.content),
                });
                raw.push(("user", vec![block]));
            }
        }
    }

    let mut merged: Vec<(&'static str, Vec<serde_json::Value>)> = Vec::new();
    for (role, blocks) in raw {
        if let Some(last) = merged.last_mut() {
            if last.0 == role {
                last.1.extend(blocks);
                continue;
            }
        }
        merged.push((role, blocks));
    }

    // Cache the conversation history: the marker goes on the last block of
    // the final message when it is a user turn ending in text, image or a
    // tool result.
    if let (Some(cc), Some(("user", blocks))) = (cache_control, merged.last_mut()) {
        if let Some(last) = blocks.last_mut() {
            if matches!(
                last["type"].as_str(),
                Some("text" | "image" | "tool_result")
            ) {
                last["cache_control"] = cc.clone();
            }
        }
    }

    merged
        .into_iter()
        .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{
        ImageContent, TextContent, ToolCallContent, ToolResultMessage, UserMessage,
    };

    fn text_user(text: &str) -> Message {
        Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: text.to_string(),
            })],
            timestamp: 0,
        })
    }

    #[test]
    fn test_build_system_plain() {
        let v = build_system("be helpful", None).unwrap();
        assert_eq!(
            v,
            serde_json::json!([{"type": "text", "text": "be helpful"}])
        );
    }

    #[test]
    fn test_build_system_none_when_empty() {
        assert!(build_system("", None).is_none());
    }

    #[test]
    fn test_build_system_cache_control() {
        let cc = serde_json::json!({"type": "ephemeral"});
        let v = build_system("be helpful", Some(&cc)).unwrap();
        assert_eq!(v[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(v[0]["text"], "be helpful");
    }

    #[test]
    fn test_get_cache_control_by_retention() {
        let mut model = default_model();
        assert_eq!(
            get_cache_control(&model, Some(CacheRetention::Long)),
            Some(serde_json::json!({"type": "ephemeral", "ttl": "1h"}))
        );
        assert_eq!(
            get_cache_control(&model, Some(CacheRetention::Short)),
            Some(serde_json::json!({"type": "ephemeral"}))
        );
        assert_eq!(get_cache_control(&model, Some(CacheRetention::None)), None);
        model.compat = Some(serde_json::json!({"supportsLongCacheRetention": false}));
        assert_eq!(
            get_cache_control(&model, Some(CacheRetention::Long)),
            Some(serde_json::json!({"type": "ephemeral"}))
        );
    }

    /// Default (long) retention marks the system prompt, the last tool and the
    /// last user block, as `buildParams` does.
    #[test]
    fn test_request_body_cache_breakpoints() {
        let model = default_model();
        let tool = |name: &str| Tool {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let context = Context::new(
            "sys".into(),
            vec![text_user("first"), text_user("second")],
            vec![tool("a"), tool("b")],
        );
        let options = SimpleStreamOptions {
            cache_retention: Some(CacheRetention::Long),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, &options);
        let cc = serde_json::json!({"type": "ephemeral", "ttl": "1h"});
        assert_eq!(body["system"][0]["cache_control"], cc);
        assert!(body["tools"][0].get("cache_control").is_none());
        assert_eq!(body["tools"][1]["cache_control"], cc);
        let content = &body["messages"][0]["content"];
        assert!(content[0].get("cache_control").is_none());
        assert_eq!(content[1]["cache_control"], cc);

        let options = SimpleStreamOptions {
            cache_retention: Some(CacheRetention::None),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, &options);
        assert!(!body.to_string().contains("cache_control"));
    }

    #[test]
    fn test_no_cache_marker_after_an_assistant_turn() {
        let messages = vec![
            text_user("hi"),
            Message::Assistant(cortexcode_ai_types::AssistantMessage {
                content: vec![Content::text("hello")],
                ..Default::default()
            }),
        ];
        let cc = serde_json::json!({"type": "ephemeral"});
        let out = convert_messages(&messages, Some(&cc));
        assert!(!serde_json::Value::Array(out)
            .to_string()
            .contains("cache_control"));
    }

    #[test]
    fn test_convert_messages_basic_roundtrip() {
        let messages = vec![text_user("hi")];
        let out = convert_messages(&messages, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"][0]["type"], "text");
        assert_eq!(out[0]["content"][0]["text"], "hi");
    }

    #[test]
    fn test_convert_messages_merges_adjacent_tool_results() {
        let messages = vec![
            Message::ToolResult(ToolResultMessage {
                details: None,
                content: vec![Content::Text(TextContent {
                    text_signature: None,
                    text: "result 1".into(),
                })],
                tool_call_id: "call_1".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: 0,
            }),
            Message::ToolResult(ToolResultMessage {
                details: None,
                content: vec![Content::Text(TextContent {
                    text_signature: None,
                    text: "result 2".into(),
                })],
                tool_call_id: "call_2".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: 0,
            }),
        ];
        let out = convert_messages(&messages, None);
        assert_eq!(
            out.len(),
            1,
            "adjacent tool results should merge into one user turn"
        );
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(out[0]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(out[0]["content"][1]["tool_use_id"], "call_2");
    }

    #[test]
    fn test_convert_messages_tool_call_block() {
        let messages = vec![Message::Assistant(cortexcode_ai_types::AssistantMessage {
            provider: String::new(),
            response_id: None,
            response_model: None,
            api: String::new(),
            diagnostics: None,
            model: String::new(),
            content: vec![Content::ToolCall(ToolCallContent {
                thought_signature: None,
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "a.rs"}),
            })],
            stop_reason: cortexcode_ai_types::StopReason::Stop,

            usage: Default::default(),
            timestamp: 0,
            error_message: None,
        })];
        let out = convert_messages(&messages, None);
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["content"][0]["type"], "tool_use");
        assert_eq!(out[0]["content"][0]["name"], "read_file");
        assert_eq!(out[0]["content"][0]["input"]["path"], "a.rs");
    }

    #[test]
    fn test_convert_messages_image_block() {
        let messages = vec![Message::User(UserMessage {
            content: vec![Content::Image(ImageContent {
                data: "base64data".into(),
                media_type: "image/png".into(),
            })],
            timestamp: 0,
        })];
        let out = convert_messages(&messages, None);
        assert_eq!(out[0]["content"][0]["type"], "image");
        assert_eq!(out[0]["content"][0]["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_resolve_thinking_budget_defaults() {
        assert_eq!(resolve_thinking_budget(&ThinkingLevel::Off, None), None);
        assert_eq!(
            resolve_thinking_budget(&ThinkingLevel::Low, None),
            Some(4096)
        );
        assert_eq!(
            resolve_thinking_budget(&ThinkingLevel::High, None),
            Some(16384)
        );
    }

    #[test]
    fn test_resolve_thinking_budget_explicit() {
        let budgets = ThinkingBudgets {
            minimal: None,
            low: Some(2000),
            medium: None,
            high: None,
            xhigh: None,
        };
        assert_eq!(
            resolve_thinking_budget(&ThinkingLevel::Low, Some(&budgets)),
            Some(2000)
        );
        assert_eq!(
            resolve_thinking_budget(&ThinkingLevel::Medium, Some(&budgets)),
            Some(8192)
        );
    }

    #[test]
    fn test_resolve_credentials_explicit_api_key() {
        let options = SimpleStreamOptions {
            api_key: Some("sk-explicit".into()),
            ..Default::default()
        };
        match resolve_credentials(&options).unwrap() {
            Credential::ApiKey(k) => assert_eq!(k, "sk-explicit"),
            Credential::OAuth(_) => panic!("expected api key credential"),
        }
    }

    #[test]
    fn test_build_headers_api_key() {
        let model = default_model();
        let headers = build_headers(&model, &Credential::ApiKey("sk-test".into()));
        assert!(headers.contains(&("x-api-key".to_string(), "sk-test".to_string())));
        assert!(headers
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
    }

    #[test]
    fn test_build_headers_oauth() {
        let model = default_model();
        let headers = build_headers(&model, &Credential::OAuth("token123".into()));
        assert!(headers.contains(&("authorization".to_string(), "Bearer token123".to_string())));
        assert!(headers.iter().any(|(k, _)| k == "anthropic-beta"));
    }

    #[test]
    fn test_build_request_body_includes_tools() {
        let model = default_model();
        let context = Context::new(
            "".into(),
            vec![text_user("hi")],
            vec![Tool {
                name: "read_file".into(),
                description: "reads a file".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        );
        let options = SimpleStreamOptions::default();
        let body = build_request_body(&model, &context, &options);
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["tools"][0]["name"], "read_file");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn test_build_request_body_thinking_expands_max_tokens() {
        let mut model = default_model();
        model.reasoning = true;
        model.max_tokens = 1000;
        let context = Context::new("".into(), vec![text_user("hi")], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, &options);
        assert_eq!(body["thinking"]["budget_tokens"], 16384);
        assert!(body["max_tokens"].as_u64().unwrap() > 16384);
    }

    fn default_model() -> Model {
        Model {
            compat: None,
            id: "claude-test".into(),
            name: "Claude Test".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            headers: None,
        }
    }
}

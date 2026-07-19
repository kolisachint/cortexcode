//! Request construction for the OpenAI Chat Completions API.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/openai-completions.ts` (request-building portion). Provider-
//! specific quirk handling (`compat` overrides for zai/together/moonshot/
//! openrouter/etc.) is intentionally not ported yet — this covers the
//! standard OpenAI-compatible Chat Completions wire format.

use cortexcode_ai_types::{
    Content, Context, Message, Model, SimpleStreamOptions, ThinkingLevel, Tool,
};

/// Resolve the OpenAI API key from explicit options, then the environment.
pub fn resolve_credentials(options: &SimpleStreamOptions) -> Result<String, String> {
    if let Some(key) = &options.api_key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }
    Err("no OpenAI credentials found (set OPENAI_API_KEY)".into())
}

/// Build the HTTP headers for a Chat Completions request.
pub fn build_headers(model: &Model, api_key: &str) -> Vec<(String, String)> {
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("authorization".to_string(), format!("Bearer {api_key}")),
    ];
    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            headers.push((k.clone(), v.clone()));
        }
    }
    headers
}

/// Build the JSON request body for the Chat Completions API.
pub fn build_request_body(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> serde_json::Value {
    let mut messages = Vec::new();
    if !context.system_prompt.is_empty() {
        messages.push(serde_json::json!({"role": "system", "content": context.system_prompt}));
    }
    messages.extend(convert_messages(&context.messages));

    let mut body = serde_json::json!({
        "model": model.id,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_completion_tokens": model.max_tokens,
    });

    if !context.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(convert_tools(&context.tools));
    }

    if model.reasoning {
        if let Some(level) = &options.reasoning {
            if let Some(effort) = resolve_reasoning_effort(model, level) {
                body["reasoning_effort"] = serde_json::json!(effort);
            }
        }
    }

    body
}

fn level_key(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

fn resolve_reasoning_effort(model: &Model, level: &ThinkingLevel) -> Option<String> {
    if let Some(map) = &model.thinking_level_map {
        if let Some(v) = map.get(level_key(level)) {
            if v.is_null() {
                return None;
            }
            return Some(
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            );
        }
    }
    match level {
        ThinkingLevel::Off => None,
        ThinkingLevel::Minimal => Some("minimal".to_string()),
        ThinkingLevel::Low => Some("low".to_string()),
        ThinkingLevel::Medium => Some("medium".to_string()),
        ThinkingLevel::High | ThinkingLevel::XHigh => Some("high".to_string()),
    }
}

fn image_url_block(media_type: &str, data: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{media_type};base64,{data}")},
    })
}

fn user_message(content: &[Content]) -> serde_json::Value {
    if content.iter().all(|c| matches!(c, Content::Text(_))) {
        let text: String = content
            .iter()
            .map(|c| match c {
                Content::Text(t) => t.text.as_str(),
                _ => "",
            })
            .collect();
        return serde_json::json!({"role": "user", "content": text});
    }

    let parts: Vec<serde_json::Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(serde_json::json!({"type": "text", "text": t.text})),
            Content::Image(img) => Some(image_url_block(&img.media_type, &img.data)),
            Content::Thinking(_) | Content::ToolCall(_) => None,
        })
        .collect();
    serde_json::json!({"role": "user", "content": parts})
}

fn assistant_message(content: &[Content]) -> Option<serde_json::Value> {
    let text: String = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) if !t.text.trim().is_empty() => Some(t.text.as_str()),
            _ => None,
        })
        .collect();

    let tool_calls: Vec<serde_json::Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(serde_json::json!({
                "id": tc.id,
                "type": "function",
                "function": {"name": tc.name, "arguments": tc.arguments.to_string()},
            })),
            _ => None,
        })
        .collect();

    if text.is_empty() && tool_calls.is_empty() {
        return None;
    }

    let mut v = serde_json::json!({"role": "assistant"});
    v["content"] = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(text)
    };
    if !tool_calls.is_empty() {
        v["tool_calls"] = serde_json::Value::Array(tool_calls);
    }
    Some(v)
}

/// Convert the context's message list into Chat Completions API messages.
///
/// Tool results map one-to-one onto `role: "tool"` messages; any image
/// content in a tool result is hoisted into a synthetic follow-up `user`
/// message, since the `tool` role does not accept image parts.
fn convert_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        match &messages[i] {
            Message::User(m) => {
                out.push(user_message(&m.content));
                i += 1;
            }
            Message::Assistant(m) => {
                if let Some(v) = assistant_message(&m.content) {
                    out.push(v);
                }
                i += 1;
            }
            Message::ToolResult(_) => {
                let mut image_blocks = Vec::new();
                while let Some(Message::ToolResult(m)) = messages.get(i) {
                    let text: String = m
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text(t) => Some(t.text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let content = if text.is_empty() {
                        "(see attached image)".to_string()
                    } else {
                        text
                    };
                    out.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": m.tool_call_id,
                        "content": content,
                    }));
                    for c in &m.content {
                        if let Content::Image(img) = c {
                            image_blocks.push(image_url_block(&img.media_type, &img.data));
                        }
                    }
                    i += 1;
                }
                if !image_blocks.is_empty() {
                    let mut parts = vec![
                        serde_json::json!({"type": "text", "text": "Attached image(s) from tool result:"}),
                    ];
                    parts.extend(image_blocks);
                    out.push(serde_json::json!({"role": "user", "content": parts}));
                }
            }
        }
    }

    out
}

fn convert_tools(tools: &[Tool]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{
        AssistantMessage as AiAssistantMessage, ImageContent, TextContent, ToolCallContent,
        ToolResultMessage, UserMessage,
    };
    use std::collections::HashMap;

    fn default_model() -> Model {
        Model {
            id: "gpt-test".into(),
            name: "GPT Test".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 128_000,
            max_tokens: 4096,
            headers: None,
        }
    }

    #[test]
    fn test_resolve_credentials_explicit() {
        let options = SimpleStreamOptions {
            api_key: Some("sk-abc".into()),
            ..Default::default()
        };
        assert_eq!(resolve_credentials(&options).unwrap(), "sk-abc");
    }

    #[test]
    fn test_build_headers() {
        let model = default_model();
        let headers = build_headers(&model, "sk-abc");
        assert!(headers.contains(&("authorization".to_string(), "Bearer sk-abc".to_string())));
    }

    #[test]
    fn test_user_message_text_only() {
        let content = vec![Content::Text(TextContent {
            text: "hi".into(),
            cache_control: None,
        })];
        let v = user_message(&content);
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "hi");
    }

    #[test]
    fn test_user_message_with_image() {
        let content = vec![
            Content::Text(TextContent {
                text: "look".into(),
                cache_control: None,
            }),
            Content::Image(ImageContent {
                data: "abc123".into(),
                media_type: "image/png".into(),
                cache_control: None,
            }),
        ];
        let v = user_message(&content);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][1]["type"], "image_url");
        assert!(v["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_assistant_message_with_tool_call() {
        let content = vec![Content::ToolCall(ToolCallContent {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "a.rs"}),
        })];
        let v = assistant_message(&content).unwrap();
        assert_eq!(v["content"], serde_json::Value::Null);
        assert_eq!(v["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(
            v["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a.rs\"}"
        );
    }

    #[test]
    fn test_assistant_message_empty_is_skipped() {
        assert!(assistant_message(&[]).is_none());
    }

    #[test]
    fn test_convert_messages_tool_result_with_image_hoists_user_message() {
        let messages = vec![Message::ToolResult(ToolResultMessage {
            content: vec![Content::Image(ImageContent {
                data: "xyz".into(),
                media_type: "image/png".into(),
                cache_control: None,
            })],
            tool_call_id: "call_1".into(),
            is_error: false,
            timestamp: None,
        })];
        let out = convert_messages(&messages);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "tool");
        assert_eq!(out[0]["content"], "(see attached image)");
        assert_eq!(out[1]["role"], "user");
        assert_eq!(out[1]["content"][1]["type"], "image_url");
    }

    #[test]
    fn test_convert_messages_plain_tool_result() {
        let messages = vec![Message::ToolResult(ToolResultMessage {
            content: vec![Content::Text(TextContent {
                text: "output here".into(),
                cache_control: None,
            })],
            tool_call_id: "call_1".into(),
            is_error: false,
            timestamp: None,
        })];
        let out = convert_messages(&messages);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["content"], "output here");
        assert_eq!(out[0]["tool_call_id"], "call_1");
    }

    #[test]
    fn test_build_request_body_basic() {
        let model = default_model();
        let context = Context::new(
            "be nice".into(),
            vec![Message::User(UserMessage {
                content: vec![Content::Text(TextContent {
                    text: "hi".into(),
                    cache_control: None,
                })],
                timestamp: None,
            })],
            vec![],
        );
        let body = build_request_body(&model, &context, &SimpleStreamOptions::default());
        assert_eq!(body["model"], "gpt-test");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["stream"], true);
        assert_eq!(body["max_completion_tokens"], 4096);
    }

    #[test]
    fn test_build_request_body_reasoning_effort_default() {
        let mut model = default_model();
        model.reasoning = true;
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Medium),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, &options);
        assert_eq!(body["reasoning_effort"], "medium");
    }

    #[test]
    fn test_build_request_body_reasoning_effort_from_map() {
        let mut model = default_model();
        model.reasoning = true;
        let mut map = HashMap::new();
        map.insert("high".to_string(), serde_json::json!("max"));
        model.thinking_level_map = Some(map);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::High),
            ..Default::default()
        };
        let body = build_request_body(&model, &context, &options);
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn test_build_request_body_includes_tools() {
        let model = default_model();
        let context = Context::new(
            "".into(),
            vec![],
            vec![Tool {
                name: "read_file".into(),
                description: "reads a file".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
        );
        let body = build_request_body(&model, &context, &SimpleStreamOptions::default());
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn test_assistant_content_roundtrip() {
        let msg = AiAssistantMessage {
            content: vec![Content::Text(TextContent {
                text: "hello".into(),
                cache_control: None,
            })],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        };
        let v = assistant_message(&msg.content).unwrap();
        assert_eq!(v["content"], "hello");
    }
}

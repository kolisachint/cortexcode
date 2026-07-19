//! Shared request/response conversion for Google Generative AI and Vertex AI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/google-shared.ts`.

use cortexcode_ai_types::{Content, Context, Message, Model, StopReason, Tool};

/// Whether a model requires explicit tool-call IDs in function calls/responses
/// (non-Gemini models proxied through Google's Cloud Code Assist API).
pub fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-") || model_id.starts_with("gpt-oss-")
}

fn normalize_tool_call_id(model_id: &str, id: &str) -> String {
    if !requires_tool_call_id(model_id) {
        return id.to_string();
    }
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitized.chars().take(64).collect()
}

fn gemini_major_version(model_id: &str) -> Option<u32> {
    let lower = model_id.to_lowercase();
    let rest = lower
        .strip_prefix("gemini-live-")
        .or_else(|| lower.strip_prefix("gemini-"))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
    match gemini_major_version(model_id) {
        Some(v) => v >= 3,
        None => true,
    }
}

/// Convert cortex messages into Gemini `Content[]` JSON.
pub fn convert_messages(model: &Model, context: &Context) -> Vec<serde_json::Value> {
    let mut contents: Vec<serde_json::Value> = Vec::new();

    for msg in &context.messages {
        match msg {
            Message::User(m) => {
                let parts: Vec<serde_json::Value> = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(serde_json::json!({"text": t.text})),
                        Content::Image(img) => Some(serde_json::json!({
                            "inlineData": {"mimeType": img.media_type, "data": img.data}
                        })),
                        Content::Thinking(_) | Content::ToolCall(_) => None,
                    })
                    .collect();
                if parts.is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({"role": "user", "parts": parts}));
            }
            Message::Assistant(m) => {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                for block in &m.content {
                    match block {
                        Content::Text(t) => {
                            if t.text.trim().is_empty() {
                                continue;
                            }
                            parts.push(serde_json::json!({"text": t.text}));
                        }
                        Content::Thinking(th) => {
                            if th.thinking.trim().is_empty() {
                                continue;
                            }
                            // Thought signatures are only meaningful on a replay to the
                            // same provider/model; we don't track message provenance
                            // here, so thinking blocks round-trip as plain thought parts.
                            parts.push(serde_json::json!({"thought": true, "text": th.thinking}));
                        }
                        Content::ToolCall(tc) => {
                            let mut fc = serde_json::json!({"name": tc.name, "args": tc.arguments});
                            if requires_tool_call_id(&model.id) {
                                fc["id"] =
                                    serde_json::json!(normalize_tool_call_id(&model.id, &tc.id));
                            }
                            parts.push(serde_json::json!({"functionCall": fc}));
                        }
                        Content::Image(_) => {}
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({"role": "model", "parts": parts}));
            }
            Message::ToolResult(m) => {
                let text_result: String = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_parts: Vec<serde_json::Value> =
                    if model.input.iter().any(|s| s == "image") {
                        m.content
                            .iter()
                            .filter_map(|c| match c {
                                Content::Image(img) => Some(serde_json::json!({
                                    "inlineData": {"mimeType": img.media_type, "data": img.data}
                                })),
                                _ => None,
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                let has_text = !text_result.is_empty();
                let has_images = !image_parts.is_empty();
                let response_value = if has_text {
                    text_result
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };

                let response_key = if m.is_error { "error" } else { "output" };
                let mut function_response = serde_json::json!({
                    "name": m.tool_name,
                    "response": {response_key: response_value},
                });
                let supports_multimodal = supports_multimodal_function_response(&model.id);
                if has_images && supports_multimodal {
                    function_response["parts"] = serde_json::Value::Array(image_parts.clone());
                }
                if requires_tool_call_id(&model.id) {
                    function_response["id"] =
                        serde_json::json!(normalize_tool_call_id(&model.id, &m.tool_call_id));
                }
                let function_response_part =
                    serde_json::json!({"functionResponse": function_response});

                // Cloud Code Assist requires all function responses in a single user turn.
                let merged = if let Some(last) = contents.last_mut() {
                    if last["role"] == "user"
                        && last["parts"].as_array().is_some_and(|parts| {
                            parts.iter().any(|p| p.get("functionResponse").is_some())
                        })
                    {
                        last["parts"]
                            .as_array_mut()
                            .unwrap()
                            .push(function_response_part.clone());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !merged {
                    contents.push(
                        serde_json::json!({"role": "user", "parts": [function_response_part]}),
                    );
                }

                if has_images && !supports_multimodal {
                    let mut parts = vec![serde_json::json!({"text": "Tool result image:"})];
                    parts.extend(image_parts);
                    contents.push(serde_json::json!({"role": "user", "parts": parts}));
                }
            }
        }
    }

    contents
}

const JSON_SCHEMA_META_DECLARATIONS: &[&str] = &[
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions",
];

fn sanitize_for_openapi(schema: &serde_json::Value) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (k, v) in map {
                if JSON_SCHEMA_META_DECLARATIONS.contains(&k.as_str()) {
                    continue;
                }
                result.insert(k.clone(), sanitize_for_openapi(v));
            }
            serde_json::Value::Object(result)
        }
        other => other.clone(),
    }
}

/// Convert tools to Gemini `functionDeclarations` format.
///
/// `use_parameters` selects the legacy OpenAPI-3.03 `parameters` field
/// (needed for Cloud Code Assist proxying to non-Gemini models) instead of
/// the default `parametersJsonSchema` field.
pub fn convert_tools(tools: &[Tool], use_parameters: bool) -> Option<serde_json::Value> {
    if tools.is_empty() {
        return None;
    }
    let declarations: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            let mut v = serde_json::json!({"name": t.name, "description": t.description});
            if use_parameters {
                v["parameters"] = sanitize_for_openapi(&t.parameters);
            } else {
                v["parametersJsonSchema"] = t.parameters.clone();
            }
            v
        })
        .collect();
    Some(serde_json::json!([{"functionDeclarations": declarations}]))
}

/// Map a Gemini `finishReason` string to a cortex `StopReason`.
pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        _ => StopReason::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{
        AssistantMessage, ImageContent, TextContent, ToolCallContent, ToolResultMessage,
        UserMessage,
    };

    fn model(id: &str, input: &[&str]) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: "google-generative-ai".into(),
            provider: "google".into(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            reasoning: false,
            thinking_level_map: None,
            input: input.iter().map(|s| s.to_string()).collect(),
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 1_000_000,
            max_tokens: 8192,
            headers: None,
        }
    }

    #[test]
    fn test_requires_tool_call_id() {
        assert!(requires_tool_call_id("claude-sonnet-5"));
        assert!(requires_tool_call_id("gpt-oss-120b"));
        assert!(!requires_tool_call_id("gemini-2.0-flash"));
    }

    #[test]
    fn test_gemini_major_version() {
        assert_eq!(gemini_major_version("gemini-3-pro"), Some(3));
        assert_eq!(gemini_major_version("gemini-2.5-flash"), Some(2));
        assert_eq!(gemini_major_version("claude-sonnet-5"), None);
    }

    #[test]
    fn test_convert_messages_user_text() {
        let m = model("gemini-2.0-flash", &["text"]);
        let ctx = Context::new(
            "".into(),
            vec![Message::User(UserMessage {
                content: vec![Content::Text(TextContent {
                    text: "hi".into(),
                    cache_control: None,
                })],
                timestamp: None,
            })],
            vec![],
        );
        let out = convert_messages(&m, &ctx);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["parts"][0]["text"], "hi");
    }

    #[test]
    fn test_convert_messages_assistant_tool_call() {
        let m = model("gemini-2.0-flash", &["text"]);
        let ctx = Context::new(
            "".into(),
            vec![Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                })],
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                timestamp: None,
                error_message: None,
            })],
            vec![],
        );
        let out = convert_messages(&m, &ctx);
        assert_eq!(out[0]["role"], "model");
        assert_eq!(out[0]["parts"][0]["functionCall"]["name"], "read_file");
        // Gemini models don't need explicit tool-call IDs on the wire.
        assert!(out[0]["parts"][0]["functionCall"].get("id").is_none());
    }

    #[test]
    fn test_convert_messages_assistant_tool_call_claude_needs_id() {
        let m = model("claude-sonnet-5", &["text"]);
        let ctx = Context::new(
            "".into(),
            vec![Message::Assistant(AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({}),
                })],
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                timestamp: None,
                error_message: None,
            })],
            vec![],
        );
        let out = convert_messages(&m, &ctx);
        assert_eq!(out[0]["parts"][0]["functionCall"]["id"], "call_1");
    }

    #[test]
    fn test_convert_messages_tool_result_merges_into_single_user_turn() {
        let m = model("gemini-2.0-flash", &["text"]);
        let messages = vec![
            Message::ToolResult(ToolResultMessage {
                content: vec![Content::Text(TextContent {
                    text: "result 1".into(),
                    cache_control: None,
                })],
                tool_call_id: "call_1".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: None,
            }),
            Message::ToolResult(ToolResultMessage {
                content: vec![Content::Text(TextContent {
                    text: "result 2".into(),
                    cache_control: None,
                })],
                tool_call_id: "call_2".into(),
                tool_name: "read_file".into(),
                is_error: false,
                timestamp: None,
            }),
        ];
        let ctx = Context::new("".into(), messages, vec![]);
        let out = convert_messages(&m, &ctx);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["parts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_convert_messages_tool_result_error() {
        let m = model("gemini-2.0-flash", &["text"]);
        let messages = vec![Message::ToolResult(ToolResultMessage {
            content: vec![Content::Text(TextContent {
                text: "boom".into(),
                cache_control: None,
            })],
            tool_call_id: "call_1".into(),
            tool_name: "read_file".into(),
            is_error: true,
            timestamp: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let out = convert_messages(&m, &ctx);
        assert_eq!(
            out[0]["parts"][0]["functionResponse"]["response"]["error"],
            "boom"
        );
    }

    #[test]
    fn test_convert_messages_tool_result_with_image_gemini3_inline() {
        let m = model("gemini-3-pro", &["text", "image"]);
        let messages = vec![Message::ToolResult(ToolResultMessage {
            content: vec![Content::Image(ImageContent {
                data: "abc".into(),
                media_type: "image/png".into(),
                cache_control: None,
            })],
            tool_call_id: "call_1".into(),
            tool_name: "read_file".into(),
            is_error: false,
            timestamp: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let out = convert_messages(&m, &ctx);
        assert_eq!(
            out.len(),
            1,
            "gemini 3 inlines images in functionResponse.parts"
        );
        assert!(out[0]["parts"][0]["functionResponse"]["parts"][0]["inlineData"].is_object());
    }

    #[test]
    fn test_convert_messages_tool_result_with_image_gemini2_separate_turn() {
        let m = model("gemini-2.0-flash", &["text", "image"]);
        let messages = vec![Message::ToolResult(ToolResultMessage {
            content: vec![Content::Image(ImageContent {
                data: "abc".into(),
                media_type: "image/png".into(),
                cache_control: None,
            })],
            tool_call_id: "call_1".into(),
            tool_name: "read_file".into(),
            is_error: false,
            timestamp: None,
        })];
        let ctx = Context::new("".into(), messages, vec![]);
        let out = convert_messages(&m, &ctx);
        assert_eq!(
            out.len(),
            2,
            "gemini < 3 sends images in a separate user turn"
        );
        assert!(out[1]["parts"][1]["inlineData"].is_object());
    }

    #[test]
    fn test_convert_tools() {
        let tools = vec![Tool {
            name: "read_file".into(),
            description: "reads a file".into(),
            parameters: serde_json::json!({"type": "object", "$schema": "x"}),
        }];
        let v = convert_tools(&tools, false).unwrap();
        assert_eq!(v[0]["functionDeclarations"][0]["name"], "read_file");
        assert!(v[0]["functionDeclarations"][0]["parametersJsonSchema"]["$schema"].is_string());
    }

    #[test]
    fn test_convert_tools_use_parameters_strips_schema_meta() {
        let tools = vec![Tool {
            name: "read_file".into(),
            description: "reads a file".into(),
            parameters: serde_json::json!({"type": "object", "$schema": "x"}),
        }];
        let v = convert_tools(&tools, true).unwrap();
        assert!(v[0]["functionDeclarations"][0]["parameters"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn test_convert_tools_empty() {
        assert!(convert_tools(&[], false).is_none());
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason("STOP"), StopReason::EndTurn);
        assert_eq!(map_stop_reason("MAX_TOKENS"), StopReason::MaxTokens);
        assert_eq!(map_stop_reason("SAFETY"), StopReason::Error);
    }
}

//! Port of hoocode `providers/google-shared.ts` (v0.5.89): message and tool
//! conversion shared by Google Generative AI and Vertex AI. Values are the
//! `@google/genai` request objects (`Content[]`, `Tool[]`); the SDK's REST
//! mapping is in `request::sdk_body`.

use cortexcode_ai_types::{
    AssistantMessage, Content, Context, Message, Model, StopReason, Tool, UserContent,
};
use cortexcode_ai_util::transform_messages;
use serde_json::{json, Map, Value};

/// `isThinkingPart`: only `thought: true` marks thinking; a
/// `thoughtSignature` can ride on any part.
pub fn is_thinking_part(part: &Value) -> bool {
    part["thought"] == Value::Bool(true)
}

/// `retainThoughtSignature`: keep the last non-empty signature of a block.
pub fn retain_thought_signature(
    existing: Option<String>,
    incoming: Option<&str>,
) -> Option<String> {
    match incoming {
        Some(signature) if !signature.is_empty() => Some(signature.to_string()),
        _ => existing,
    }
}

/// Thought signatures must be base64 (`TYPE_BYTES`):
/// `/^[A-Za-z0-9+/]+={0,2}$/` with a length that is a multiple of 4.
fn is_valid_thought_signature(signature: Option<&str>) -> bool {
    let Some(signature) = signature.filter(|s| !s.is_empty()) else {
        return false;
    };
    if signature.len() % 4 != 0 {
        return false;
    }
    let body = signature.trim_end_matches('=');
    signature.len() - body.len() <= 2
        && !body.is_empty()
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// `resolveThoughtSignature`: only same provider/model, valid base64.
fn resolve_thought_signature(
    same_provider_and_model: bool,
    signature: Option<&str>,
) -> Option<String> {
    (same_provider_and_model && is_valid_thought_signature(signature))
        .then(|| signature.unwrap_or_default().to_string())
}

/// `requiresToolCallId`: models behind Google APIs that need explicit ids.
pub fn requires_tool_call_id(model_id: &str) -> bool {
    model_id.starts_with("claude-") || model_id.starts_with("gpt-oss-")
}

/// `getGeminiMajorVersion`: `/^gemini(?:-live)?-(\d+)/` on the lowercased id.
pub(crate) fn gemini_major_version(model_id: &str) -> Option<u32> {
    let lower = model_id.to_lowercase();
    let rest = lower.strip_prefix("gemini-")?;
    let rest = rest
        .strip_prefix("live-")
        .filter(|r| r.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(rest);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn supports_multimodal_function_response(model_id: &str) -> bool {
    gemini_major_version(model_id).is_none_or(|v| v >= 3)
}

fn inline_data(media_type: &str, data: &str) -> Value {
    json!({"inlineData": {"mimeType": media_type, "data": data}})
}

fn with_signature(mut part: Value, signature: Option<String>) -> Value {
    if let Some(signature) = signature {
        part["thoughtSignature"] = json!(signature);
    }
    part
}

fn assistant_parts(message: &AssistantMessage, model: &Model) -> Vec<Value> {
    // Thinking stays thinking only for the same provider and model.
    let same = message.provider == model.provider && message.model == model.id;
    let mut parts = Vec::new();
    for block in &message.content {
        match block {
            Content::Text(t) => {
                if t.text.trim().is_empty() {
                    continue;
                }
                let signature = resolve_thought_signature(same, t.text_signature.as_deref());
                parts.push(with_signature(json!({"text": t.text}), signature));
            }
            Content::Thinking(t) => {
                if t.thinking.trim().is_empty() {
                    continue;
                }
                if same {
                    let signature = resolve_thought_signature(same, t.signature.as_deref());
                    parts.push(with_signature(
                        json!({"thought": true, "text": t.thinking}),
                        signature,
                    ));
                } else {
                    // Plain text, no tags, so the model does not mimic them.
                    parts.push(json!({"text": t.thinking}));
                }
            }
            Content::ToolCall(call) => {
                let signature = resolve_thought_signature(same, call.thought_signature.as_deref());
                let args = if call.arguments.is_null() {
                    json!({})
                } else {
                    call.arguments.clone()
                };
                let mut function_call = json!({"name": call.name, "args": args});
                if requires_tool_call_id(&model.id) {
                    function_call["id"] = json!(call.id);
                }
                parts.push(with_signature(
                    json!({"functionCall": function_call}),
                    signature,
                ));
            }
            Content::Image(_) => {}
        }
    }
    parts
}

/// `convertMessages`: internal messages to Gemini `Content[]`.
pub fn convert_messages(model: &Model, context: &Context) -> Vec<Value> {
    let needs_ids = requires_tool_call_id(&model.id);
    let normalize = move |id: &str, _: &Model, _: &AssistantMessage| -> String {
        if !needs_ids {
            return id.to_string();
        }
        id.encode_utf16()
            .map(|unit| match char::from_u32(unit as u32) {
                Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
                _ => '_',
            })
            .take(64)
            .collect()
    };
    let transformed = transform_messages(&context.messages, model, Some(&normalize));
    let mut contents: Vec<Value> = Vec::new();

    for msg in &transformed {
        match msg {
            Message::User(m) => match &m.content {
                UserContent::Text(text) => {
                    contents.push(json!({"role": "user", "parts": [{"text": text}]}));
                }
                UserContent::Blocks(blocks) => {
                    let parts: Vec<Value> = blocks
                        .iter()
                        .filter_map(|item| match item {
                            Content::Text(t) => Some(json!({"text": t.text})),
                            Content::Image(img) => Some(inline_data(&img.media_type, &img.data)),
                            _ => None,
                        })
                        .collect();
                    if !parts.is_empty() {
                        contents.push(json!({"role": "user", "parts": parts}));
                    }
                }
            },
            Message::Assistant(m) => {
                let parts = assistant_parts(m, model);
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Message::ToolResult(m) => {
                let text_result = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let image_parts: Vec<Value> = if model.input.iter().any(|i| i == "image") {
                    m.content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Image(img) => Some(inline_data(&img.media_type, &img.data)),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                let has_images = !image_parts.is_empty();
                // Gemini 3+ nests images in the function response; older
                // Gemini and non-Gemini models get a separate user turn.
                let multimodal = supports_multimodal_function_response(&model.id);
                let value = if !text_result.is_empty() {
                    text_result
                } else if has_images {
                    "(see attached image)".to_string()
                } else {
                    String::new()
                };
                let key = if m.is_error { "error" } else { "output" };
                let mut response = Map::new();
                response.insert(key.to_string(), json!(value));
                let mut function_response = json!({"name": m.tool_name, "response": response});
                if has_images && multimodal {
                    function_response["parts"] = json!(image_parts);
                }
                if needs_ids {
                    function_response["id"] = json!(m.tool_call_id);
                }
                let part = json!({"functionResponse": function_response});

                // Cloud Code Assist wants all function responses in one user turn.
                let merge = contents.last().is_some_and(|last| {
                    last["role"] == "user"
                        && last["parts"].as_array().is_some_and(|parts| {
                            parts.iter().any(|p| p.get("functionResponse").is_some())
                        })
                });
                if merge {
                    if let Some(parts) = contents
                        .last_mut()
                        .and_then(|last| last["parts"].as_array_mut())
                    {
                        parts.push(part);
                    }
                } else {
                    contents.push(json!({"role": "user", "parts": [part]}));
                }

                if has_images && !multimodal {
                    let mut parts = vec![json!({"text": "Tool result image:"})];
                    parts.extend(image_parts);
                    contents.push(json!({"role": "user", "parts": parts}));
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

/// `sanitizeForOpenApi`: drop meta declarations, recursively through objects
/// (arrays are left as they are).
fn sanitize_for_open_api(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !JSON_SCHEMA_META_DECLARATIONS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), sanitize_for_open_api(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `convertTools`: `parametersJsonSchema`, or the legacy (sanitized)
/// `parameters` when `use_parameters`. `None` without tools.
pub fn convert_tools(tools: &[Tool], use_parameters: bool) -> Option<Value> {
    if tools.is_empty() {
        return None;
    }
    let declarations: Vec<Value> = tools
        .iter()
        .map(|tool| {
            let mut decl = json!({"name": tool.name, "description": tool.description});
            if use_parameters {
                decl["parameters"] = sanitize_for_open_api(&tool.parameters);
            } else {
                decl["parametersJsonSchema"] = tool.parameters.clone();
            }
            decl
        })
        .collect();
    Some(json!([{"functionDeclarations": declarations}]))
}

/// `mapToolChoice`: the `FunctionCallingConfigMode`.
pub fn map_tool_choice(choice: &str) -> &'static str {
    match choice {
        "none" => "NONE",
        "any" => "ANY",
        _ => "AUTO",
    }
}

/// `mapStopReason` over `FinishReason`: STOP and MAX_TOKENS map, every other
/// known reason is an error, unknown values throw.
pub fn map_stop_reason(reason: &str) -> Result<StopReason, String> {
    Ok(match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "BLOCKLIST"
        | "PROHIBITED_CONTENT"
        | "SPII"
        | "SAFETY"
        | "IMAGE_SAFETY"
        | "IMAGE_PROHIBITED_CONTENT"
        | "IMAGE_RECITATION"
        | "IMAGE_OTHER"
        | "RECITATION"
        | "FINISH_REASON_UNSPECIFIED"
        | "OTHER"
        | "LANGUAGE"
        | "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL"
        | "NO_IMAGE" => StopReason::Error,
        other => return Err(format!("Unhandled stop reason: {other}")),
    })
}

/// `mapStopReasonString` (raw API responses).
pub fn map_stop_reason_string(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        _ => StopReason::Error,
    }
}

#[cfg(test)]
#[path = "shared_tests.rs"]
mod tests;

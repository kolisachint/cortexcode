//! Cross-provider message normalization.
//!
//! Port of hoocode `providers/transform-messages.ts` (v0.5.89).

use std::collections::{HashMap, HashSet};

use cortexcode_ai_types::{
    AssistantMessage, Content, Message, Model, StopReason, TextContent, ToolCallContent,
    ToolResultMessage,
};

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// `normalizeToolCallId(id, model, source)`.
pub type NormalizeToolCallId<'a> = &'a dyn Fn(&str, &Model, &AssistantMessage) -> String;

fn replace_images_with_placeholder(content: &[Content], placeholder: &str) -> Vec<Content> {
    let mut result = Vec::with_capacity(content.len());
    let mut previous_was_placeholder = false;
    for block in content {
        if let Content::Image(_) = block {
            if !previous_was_placeholder {
                result.push(Content::Text(TextContent::new(placeholder)));
            }
            previous_was_placeholder = true;
            continue;
        }
        previous_was_placeholder = matches!(block, Content::Text(t) if t.text == placeholder);
        result.push(block.clone());
    }
    result
}

fn downgrade_unsupported_images(messages: &[Message], model: &Model) -> Vec<Message> {
    if model.input.iter().any(|i| i == "image") {
        return messages.to_vec();
    }
    messages
        .iter()
        .map(|msg| match msg {
            Message::User(m) => {
                let mut m = m.clone();
                m.content =
                    replace_images_with_placeholder(&m.content, NON_VISION_USER_IMAGE_PLACEHOLDER);
                Message::User(m)
            }
            Message::ToolResult(m) => {
                let mut m = m.clone();
                m.content =
                    replace_images_with_placeholder(&m.content, NON_VISION_TOOL_IMAGE_PLACEHOLDER);
                Message::ToolResult(m)
            }
            other => other.clone(),
        })
        .collect()
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// `transformMessages`: downgrade images for non-vision models, convert
/// thinking from other models to text, normalize tool call ids (and the
/// matching tool results), drop errored/aborted assistant turns, and
/// synthesize `No result provided` results for orphaned tool calls.
pub fn transform_messages(
    messages: &[Message],
    model: &Model,
    normalize_tool_call_id: Option<NormalizeToolCallId<'_>>,
) -> Vec<Message> {
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
    let image_aware = downgrade_unsupported_images(messages, model);

    let mut transformed = Vec::with_capacity(image_aware.len());
    for msg in image_aware {
        match msg {
            Message::User(_) => transformed.push(msg),
            Message::ToolResult(mut m) => {
                if let Some(id) = tool_call_id_map.get(&m.tool_call_id) {
                    m.tool_call_id = id.clone();
                }
                transformed.push(Message::ToolResult(m));
            }
            Message::Assistant(assistant) => {
                let is_same_model = assistant.provider == model.provider
                    && assistant.api == model.api
                    && assistant.model == model.id;
                let mut content = Vec::with_capacity(assistant.content.len());
                for block in &assistant.content {
                    match block {
                        Content::Thinking(t) => {
                            if t.redacted {
                                if is_same_model {
                                    content.push(block.clone());
                                }
                                continue;
                            }
                            if is_same_model
                                && t.signature.as_deref().is_some_and(|s| !s.is_empty())
                            {
                                content.push(block.clone());
                                continue;
                            }
                            if t.thinking.trim().is_empty() {
                                continue;
                            }
                            if is_same_model {
                                content.push(block.clone());
                            } else {
                                content.push(Content::Text(TextContent::new(t.thinking.clone())));
                            }
                        }
                        Content::Text(t) => {
                            if is_same_model {
                                content.push(block.clone());
                            } else {
                                content.push(Content::Text(TextContent::new(t.text.clone())));
                            }
                        }
                        Content::ToolCall(tc) => {
                            let mut normalized: ToolCallContent = tc.clone();
                            if !is_same_model {
                                normalized.thought_signature = None;
                                if let Some(normalize) = normalize_tool_call_id {
                                    let id = normalize(&tc.id, model, &assistant);
                                    if id != tc.id {
                                        tool_call_id_map.insert(tc.id.clone(), id.clone());
                                        normalized.id = id;
                                    }
                                }
                            }
                            content.push(Content::ToolCall(normalized));
                        }
                        Content::Image(_) => content.push(block.clone()),
                    }
                }
                let mut assistant = assistant;
                assistant.content = content;
                transformed.push(Message::Assistant(assistant));
            }
        }
    }

    let mut result: Vec<Message> = Vec::with_capacity(transformed.len());
    let mut pending_tool_calls: Vec<ToolCallContent> = Vec::new();
    let mut existing_tool_result_ids: HashSet<String> = HashSet::new();
    let insert_synthetic = |result: &mut Vec<Message>,
                            pending: &mut Vec<ToolCallContent>,
                            existing: &mut HashSet<String>| {
        if pending.is_empty() {
            return;
        }
        for tc in pending.iter() {
            if !existing.contains(&tc.id) {
                result.push(Message::ToolResult(ToolResultMessage {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    content: vec![Content::Text(TextContent::new("No result provided"))],
                    details: None,
                    is_error: true,
                    timestamp: now_millis(),
                }));
            }
        }
        pending.clear();
        existing.clear();
    };

    for msg in transformed {
        match msg {
            Message::Assistant(a) => {
                insert_synthetic(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                );
                if matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let tool_calls: Vec<ToolCallContent> = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        Content::ToolCall(tc) => Some(tc.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending_tool_calls = tool_calls;
                    existing_tool_result_ids = HashSet::new();
                }
                result.push(Message::Assistant(a));
            }
            Message::ToolResult(m) => {
                existing_tool_result_ids.insert(m.tool_call_id.clone());
                result.push(Message::ToolResult(m));
            }
            Message::User(m) => {
                insert_synthetic(
                    &mut result,
                    &mut pending_tool_calls,
                    &mut existing_tool_result_ids,
                );
                result.push(Message::User(m));
            }
        }
    }
    insert_synthetic(
        &mut result,
        &mut pending_tool_calls,
        &mut existing_tool_result_ids,
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::{ImageContent, ThinkingContent, UserMessage};

    fn model(input: &[&str]) -> Model {
        Model {
            id: "m".into(),
            name: "M".into(),
            api: "openai-completions".into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: input.iter().map(|s| s.to_string()).collect(),
            cost: Default::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    fn assistant(provider: &str, content: Vec<Content>, stop: StopReason) -> Message {
        Message::Assistant(AssistantMessage {
            content,
            api: "openai-completions".into(),
            provider: provider.into(),
            model: "m".into(),
            stop_reason: stop,
            ..Default::default()
        })
    }

    fn call(id: &str) -> Content {
        Content::ToolCall(ToolCallContent {
            id: id.into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
            thought_signature: Some("sig".into()),
        })
    }

    #[test]
    fn images_become_one_placeholder_for_non_vision_models() {
        let img = Content::Image(ImageContent {
            data: "x".into(),
            media_type: "image/png".into(),
        });
        let msgs = vec![Message::User(UserMessage {
            content: vec![img.clone(), img, Content::Text(TextContent::new("hi"))],
            timestamp: 0,
        })];
        let out = transform_messages(&msgs, &model(&["text"]), None);
        let Message::User(u) = &out[0] else { panic!() };
        assert_eq!(
            u.content,
            vec![
                Content::Text(TextContent::new(NON_VISION_USER_IMAGE_PLACEHOLDER)),
                Content::Text(TextContent::new("hi")),
            ]
        );
        let out = transform_messages(&msgs, &model(&["text", "image"]), None);
        assert_eq!(out, msgs);
    }

    #[test]
    fn other_model_thinking_becomes_text_and_ids_are_normalized() {
        let msgs = vec![
            assistant(
                "other",
                vec![
                    Content::Thinking(ThinkingContent {
                        thinking: "hmm".into(),
                        signature: Some("s".into()),
                        redacted: false,
                    }),
                    Content::Thinking(ThinkingContent {
                        thinking: "secret".into(),
                        signature: None,
                        redacted: true,
                    }),
                    call("a|b"),
                ],
                StopReason::ToolUse,
            ),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "a|b".into(),
                tool_name: "read".into(),
                content: vec![],
                details: None,
                is_error: false,
                timestamp: 0,
            }),
        ];
        let normalize = |id: &str, _: &Model, _: &AssistantMessage| id.replace('|', "_");
        let out = transform_messages(&msgs, &model(&["text"]), Some(&normalize));
        let Message::Assistant(a) = &out[0] else {
            panic!()
        };
        assert_eq!(a.content.len(), 2);
        assert_eq!(a.content[0], Content::Text(TextContent::new("hmm")));
        let Content::ToolCall(tc) = &a.content[1] else {
            panic!()
        };
        assert_eq!(tc.id, "a_b");
        assert_eq!(tc.thought_signature, None);
        let Message::ToolResult(r) = &out[1] else {
            panic!()
        };
        assert_eq!(r.tool_call_id, "a_b");
    }

    #[test]
    fn orphaned_calls_get_synthetic_results_and_errored_turns_are_dropped() {
        let msgs = vec![
            assistant("p", vec![call("c1")], StopReason::ToolUse),
            Message::User(UserMessage {
                content: vec![Content::Text(TextContent::new("next"))],
                timestamp: 0,
            }),
            assistant("p", vec![call("c2")], StopReason::Error),
            assistant("p", vec![call("c3")], StopReason::ToolUse),
        ];
        let out = transform_messages(&msgs, &model(&["text"]), None);
        let roles: Vec<&str> = out
            .iter()
            .map(|m| match m {
                Message::User(_) => "user",
                Message::Assistant(_) => "assistant",
                Message::ToolResult(_) => "toolResult",
            })
            .collect();
        assert_eq!(
            roles,
            ["assistant", "toolResult", "user", "assistant", "toolResult"]
        );
        let Message::ToolResult(r) = &out[4] else {
            panic!()
        };
        assert_eq!(r.tool_call_id, "c3");
        assert!(r.is_error);
        assert_eq!(
            r.content,
            vec![Content::Text(TextContent::new("No result provided"))]
        );
    }
}

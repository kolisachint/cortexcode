//! Anthropic provider for cortex AI.
//!
//! Provides streaming access to Claude models via the Anthropic Messages API.
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/anthropic.ts`.

use cortexcode_ai_env;
use cortexcode_ai_stream::{AiMessageEventSender, AiMessageEventStream};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, Context,
    Model, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent,
    Usage, Cost,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ---------------------------------------------------------------------------
// API Request/Response types
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u64,
    stream: bool,
    system: Option<String>,
    messages: Vec<Message>,
    tools: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
}

#[derive(Debug, Serialize)]
struct ThinkingConfig {
    #[serde(rename = "type")]
    thinking_type: String,
    budget_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<Delta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_block: Option<ContentBlockResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<MessageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    error: Option<ErrorResponse>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    delta_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlockResponse {
    #[serde(rename = "type")]
    content_type: String,
}

#[derive(Debug, Deserialize)]
struct MessageResponse {
    stop_reason: String,
    usage: UsageResponse,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ErrorResponse {
    #[allow(dead_code)]
    message: String,
}

// ---------------------------------------------------------------------------
// Provider function
// ---------------------------------------------------------------------------

/// Stream a completion from Anthropic's Messages API.
///
/// This is the main entry point for using Anthropic as an LLM provider.
pub async fn stream(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>, Box<dyn std::error::Error + Send + Sync>> {
    let api_key = options
        .api_key
        .clone()
        .or_else(|| cortexcode_ai_env::get_env_api_key("anthropic"))
        .ok_or("No Anthropic API key found in options or environment")?;

    // Build the request
    let request = build_request(model, context, options)?;

    // Create the event stream channel
    let (sender, stream) = AiMessageEventStream::new();

    // Spawn the streaming task
    let sender_clone = sender.clone();
    let model_clone = model.clone();
    tokio::spawn(async move {
        if let Err(e) = perform_stream(api_key, request, sender_clone, &model_clone).await {
            tracing::error!("Anthropic stream error: {}", e);
        }
    });

    Ok(Box::new(stream) as Box<dyn AssistantMessageEventStream>)
}

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

fn build_request(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> Result<AnthropicRequest, Box<dyn std::error::Error + Send + Sync>> {
    // Build message list
    let mut messages = Vec::new();

    for msg in &context.messages {
        match msg {
            cortexcode_ai_types::Message::User(um) => {
                let content = content_to_anthropic_value(&um.content);
                messages.push(Message {
                    role: "user".to_string(),
                    content,
                });
            }
            cortexcode_ai_types::Message::Assistant(am) => {
                let content = content_to_anthropic_value(&am.content);
                messages.push(Message {
                    role: "assistant".to_string(),
                    content,
                });
            }
            cortexcode_ai_types::Message::ToolResult(tr) => {
                let content = content_to_anthropic_value(&tr.content);
                messages.push(Message {
                    role: "user".to_string(),
                    content: vec![json!({
                        "type": "tool_result",
                        "tool_use_id": tr.tool_call_id,
                        "content": content,
                        "is_error": tr.is_error
                    })],
                });
            }
        }
    }

    // Build tools
    let tools = if context.tools.is_empty() {
        None
    } else {
        Some(
            context
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters
                    })
                })
                .collect(),
        )
    };

    // Build thinking config if reasoning is requested
    let thinking = options.reasoning.as_ref().map(|level| {
        let budget_tokens = options
            .thinking_budgets
            .as_ref()
            .and_then(|b| match level {
                cortexcode_ai_types::ThinkingLevel::Minimal => b.minimal,
                cortexcode_ai_types::ThinkingLevel::Low => b.low,
                cortexcode_ai_types::ThinkingLevel::Medium => b.medium,
                cortexcode_ai_types::ThinkingLevel::High => b.high,
                cortexcode_ai_types::ThinkingLevel::XHigh => b.xhigh,
                cortexcode_ai_types::ThinkingLevel::Off => None,
            });

        ThinkingConfig {
            thinking_type: match level {
                cortexcode_ai_types::ThinkingLevel::Off => "disabled".to_string(),
                _ => "enabled".to_string(),
            },
            budget_tokens,
        }
    });

    Ok(AnthropicRequest {
        model: model.id.clone(),
        max_tokens: model.max_tokens,
        stream: true,
        system: if context.system_prompt.is_empty() {
            None
        } else {
            Some(context.system_prompt.clone())
        },
        messages,
        tools,
        thinking,
    })
}

fn content_to_anthropic_value(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .map(|c| match c {
            Content::Text(tc) => {
                let mut obj = json!({
                    "type": "text",
                    "text": tc.text,
                });
                if let Some(_cache) = &tc.cache_control {
                    if let serde_json::Value::Object(ref mut o) = obj {
                        o.insert(
                            "cache_control".to_string(),
                            json!({"type": "ephemeral"}),
                        );
                    }
                }
                obj
            }
            Content::Image(img) => json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": img.media_type,
                    "data": img.data,
                }
            }),
            Content::Thinking(_) => json!(null), // Thinking is not sent in requests
            Content::ToolCall(_) => json!(null),  // Tool calls are not sent in requests
        })
        .filter(|v| !v.is_null())
        .collect()
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

async fn perform_stream(
    api_key: String,
    request: AnthropicRequest,
    sender: AiMessageEventSender,
    model: &Model,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

    let res = client
        .post(&format!("{}/v1/messages", model.base_url))
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !res.status().is_success() {
        let error_text = res.text().await?;
        return Err(format!("Anthropic API error: {}", error_text).into());
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    let mut partial = AssistantMessage {
        content: vec![],
        stop_reason: None,
        stop_sequence: None,
        usage: None,
        timestamp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        error_message: None,
    };

    // Emit Start event
    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    use futures::stream::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer.drain(..=newline_pos).collect::<String>();
            let line = line.trim();

            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }

            let json_str = &line[6..]; // Skip "data: " prefix
            if json_str == "[DONE]" {
                break;
            }

            match serde_json::from_str::<StreamEvent>(json_str) {
                Ok(event) => {
                    process_event(&event, &sender, &mut partial)?;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse event: {}", e);
                }
            }
        }
    }

    // Determine stop reason
    if partial.stop_reason.is_none() {
        partial.stop_reason = Some(StopReason::EndTurn);
    }

    // Emit Done event
    sender.push(AssistantMessageEvent::Done {
        message: partial.clone(),
    });

    Ok(())
}

fn process_event(
    event: &StreamEvent,
    sender: &AiMessageEventSender,
    partial: &mut AssistantMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match event.event_type.as_str() {
        "content_block_start" => {
            if let Some(content_block) = &event.content_block {
                match content_block.content_type.as_str() {
                    "text" => {
                        let index = partial.content.len();
                        partial.content.push(Content::Text(TextContent {
                            text: String::new(),
                            cache_control: None,
                        }));
                        sender.push(AssistantMessageEvent::TextStart {
                            index,
                            partial: partial.clone(),
                        });
                    }
                    "thinking" => {
                        let index = partial.content.len();
                        partial.content.push(Content::Thinking(ThinkingContent {
                            thinking: String::new(),
                            signature: None,
                        }));
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            index,
                            partial: partial.clone(),
                        });
                    }
                    "tool_use" => {
                        let index = partial.content.len();
                        partial.content.push(Content::ToolCall(ToolCallContent {
                            id: format!("tool-{}", index),
                            name: String::new(),
                            arguments: serde_json::json!({}),
                        }));
                        sender.push(AssistantMessageEvent::ToolCallStart {
                            index,
                            partial: partial.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = &event.delta {
                if let Some(index) = event.index {
                    match delta.delta_type.as_str() {
                        "text_delta" => {
                            if let Some(text) = &delta.text {
                                if let Content::Text(ref mut tc) =
                                    &mut partial.content.get_mut(index).unwrap()
                                {
                                    tc.text.push_str(text);
                                }
                                sender.push(AssistantMessageEvent::TextDelta {
                                    index,
                                    delta: text.clone(),
                                    partial: partial.clone(),
                                });
                            }
                        }
                        "thinking_delta" => {
                            if let Some(thinking) = &delta.thinking {
                                if let Content::Thinking(ref mut th) =
                                    &mut partial.content.get_mut(index).unwrap()
                                {
                                    th.thinking.push_str(thinking);
                                }
                                sender.push(AssistantMessageEvent::ThinkingDelta {
                                    index,
                                    delta: thinking.clone(),
                                    partial: partial.clone(),
                                });
                            }
                        }
                        "input_json_delta" => {
                            if let Some(text) = &delta.text {
                                if let Content::ToolCall(ref mut tc) =
                                    &mut partial.content.get_mut(index).unwrap()
                                {
                                    // For tool calls, accumulate the JSON
                                    tc.arguments = serde_json::from_str(text)
                                        .unwrap_or_else(|_| serde_json::json!(text));
                                }
                                sender.push(AssistantMessageEvent::ToolCallDelta {
                                    index,
                                    delta: text.clone(),
                                    partial: partial.clone(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        "content_block_stop" => {
            if let Some(index) = event.index {
                if let Some(content) = partial.content.get(index) {
                    match content {
                        Content::Text(_) => {
                            sender.push(AssistantMessageEvent::TextEnd {
                                index,
                                partial: partial.clone(),
                            });
                        }
                        Content::Thinking(_) => {
                            sender.push(AssistantMessageEvent::ThinkingEnd {
                                index,
                                partial: partial.clone(),
                            });
                        }
                        Content::ToolCall(_) => {
                            sender.push(AssistantMessageEvent::ToolCallEnd {
                                index,
                                partial: partial.clone(),
                            });
                        }
                        Content::Image(_) => {}
                    }
                }
            }
        }
        "message_stop" => {
            if let Some(msg) = &event.message {
                partial.stop_reason = match msg.stop_reason.as_str() {
                    "end_turn" => Some(StopReason::EndTurn),
                    "stop_sequence" => Some(StopReason::StopSequence),
                    "max_tokens" => Some(StopReason::MaxTokens),
                    "tool_use" => Some(StopReason::ToolUse),
                    other => Some(StopReason::Other(other.to_string())),
                };
                partial.usage = Some(Usage {
                    input: msg.usage.input_tokens,
                    output: msg.usage.output_tokens,
                    cache_read: 0,
                    cache_write: 0,
                    total_tokens: msg.usage.input_tokens + msg.usage.output_tokens,
                    cost: Cost {
                        input: 0.0,
                        output: 0.0,
                        cache_read: 0.0,
                        cache_write: 0.0,
                        total: 0.0,
                    },
                });
            }
        }
        "message_delta" => {}
        "message_start" => {}
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_to_anthropic_value_text() {
        let content = vec![Content::Text(TextContent {
            text: "hello".to_string(),
            cache_control: None,
        })];
        let values = content_to_anthropic_value(&content);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["type"], "text");
        assert_eq!(values[0]["text"], "hello");
    }

    #[test]
    fn test_content_to_anthropic_value_image() {
        let content = vec![Content::Image(cortexcode_ai_types::ImageContent {
            data: "iVBORw0KG".to_string(),
            media_type: "image/png".to_string(),
            cache_control: None,
        })];
        let values = content_to_anthropic_value(&content);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["type"], "image");
    }
}

//! Anthropic provider for cortex AI: port of hoocode
//! `packages/ai/src/providers/anthropic.ts` (v0.5.89).
//!
//! [`stream`] is `streamSimpleAnthropic` (API key, option mapping, thinking
//! mode); [`stream_anthropic`] is `streamAnthropic`, which reports every
//! failure as a terminal `error` event.

mod request;
mod sse;

use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Model, OnPayload,
    OnResponse, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, Tool,
    ToolCallContent,
};
use futures_util::StreamExt;
use serde_json::Value;

pub use request::{
    build_headers, build_params, convert_messages, convert_tools, from_claude_code_name,
    is_oauth_token, simple_options, supports_adaptive_thinking, to_claude_code_name,
    AnthropicOptions,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `streamSimpleAnthropic`: fails before streaming only when there is no
/// API key (`No API key for provider: …`).
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = options
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
        .ok_or_else(|| format!("No API key for provider: {}", model.provider))?;
    let options = simple_options(&model, &options, api_key);
    Ok(stream_anthropic(model, context, options))
}

/// `streamAnthropic`.
pub fn stream_anthropic(
    model: Model,
    context: Context,
    options: AnthropicOptions,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, run(model, context, options, sender));
    stream
}

async fn run(
    model: Model,
    context: Context,
    options: AnthropicOptions,
    sender: AssistantMessageEventStream,
) {
    let mut state = StreamState::new(&model);
    let signal = options.signal.clone();
    let outcome = match &signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled() => Err("Request was aborted".to_string()),
            r = drive(&model, &context, &options, &mut state, &sender) => r,
        },
        None => drive(&model, &context, &options, &mut state, &sender).await,
    };
    match outcome {
        Ok(()) => {
            let message = state.output;
            sender.push(AssistantMessageEvent::Done {
                message: message.clone(),
            });
            sender.end(Some(message));
        }
        Err(message) => {
            let mut output = state.output;
            output.stop_reason = if signal.as_ref().is_some_and(AbortSignal::aborted) {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            output.error_message = Some(message);
            sender.push(AssistantMessageEvent::Error {
                error: output.clone(),
            });
            sender.end(Some(output));
        }
    }
}

/// The body of `streamAnthropic`'s try block. `Err` is the `errorMessage`
/// (already through `describeProviderError`).
async fn drive(
    model: &Model,
    context: &Context,
    options: &AnthropicOptions,
    state: &mut StreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), String> {
    let api_key = options
        .api_key
        .clone()
        .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
        .unwrap_or_default();
    let (headers, is_oauth) = build_headers(model, context, &api_key, options);
    state.is_oauth = is_oauth;
    let params = build_params(model, context, is_oauth, options);
    let mut params = OnPayload::apply(options.on_payload.as_ref(), params, model).await;
    // `{ ...params, stream: true }`.
    if let Some(object) = params.as_object_mut() {
        object.insert("stream".into(), Value::Bool(true));
    }
    let url = format!("{}/v1/messages", model.base_url.trim_end_matches('/'));

    let mut builder = reqwest::Client::builder();
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }
    let client = builder
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let response = cortexcode_ai_util::post_json_with_sdk_retries(
        &client,
        &url,
        &headers,
        &params,
        options.max_retries,
        options.max_retry_delay_ms,
    )
    .await
    .map_err(|failure| failure.message().to_string())?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let response_headers = cortexcode_ai_util::response_headers(&response);
        let text = response.text().await.unwrap_or_default();
        return Err(cortexcode_ai_util::describe_provider_error(
            &api_error_message(status, &text),
            Some(&response_headers),
            options.max_retry_delay_ms,
        ));
    }
    OnResponse::notify(
        options.on_response.as_ref(),
        cortexcode_ai_util::provider_response(&response),
        model,
    )
    .await;

    sender.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut decoder = sse::SseDecoder::default();
    let mut filter = sse::AnthropicEventFilter::default();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("error reading response stream: {e}"))?;
        for sse in decoder.push(&chunk) {
            if let Some(event) = filter.accept(&sse)? {
                state.handle_event(model, &context.tools, &event, sender)?;
            }
        }
    }
    for sse in decoder.finish() {
        if let Some(event) = filter.accept(&sse)? {
            state.handle_event(model, &context.tools, &event, sender)?;
        }
    }
    filter.finish()?;

    if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err("Request was aborted".to_string());
    }
    if matches!(
        state.output.stop_reason,
        StopReason::Aborted | StopReason::Error
    ) {
        return Err("An unknown error occurred".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

/// `output` of `streamAnthropic` plus the per-block scratch fields (`index`,
/// `partialJson`) that TS keeps on the blocks and strips at the end.
struct StreamState {
    output: AssistantMessage,
    /// The Anthropic block index of each content block, until it stops.
    indexes: Vec<Option<u64>>,
    partial_json: Vec<String>,
    is_oauth: bool,
}

impl StreamState {
    fn new(model: &Model) -> Self {
        Self {
            output: AssistantMessage::for_model(model),
            indexes: Vec::new(),
            partial_json: Vec::new(),
            is_oauth: false,
        }
    }

    /// `blocks.findIndex((b) => b.index === event.index)`.
    fn position(&self, event: &Value) -> Option<usize> {
        let index = event["index"].as_u64()?;
        self.indexes.iter().position(|i| *i == Some(index))
    }

    fn push_block(&mut self, block: Content, index: Option<u64>) -> usize {
        self.output.content.push(block);
        self.indexes.push(index);
        self.partial_json.push(String::new());
        self.output.content.len() - 1
    }

    fn update_cost(&mut self, model: &Model) {
        let usage = &mut self.output.usage;
        usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
        usage.cost = cortexcode_ai_models::calculate_cost(model, usage);
    }

    /// One event of the `for await` loop. `Err` ends the stream (an unknown
    /// stop reason throws in TS).
    fn handle_event(
        &mut self,
        model: &Model,
        tools: &[Tool],
        event: &Value,
        sender: &AssistantMessageEventStream,
    ) -> Result<(), String> {
        match event["type"].as_str().unwrap_or("") {
            "message_start" => {
                let message = &event["message"];
                if let Some(id) = message["id"].as_str() {
                    self.output.response_id = Some(id.to_string());
                }
                let usage = &message["usage"];
                let count = |key: &str| usage[key].as_u64().unwrap_or(0);
                self.output.usage.input = count("input_tokens");
                self.output.usage.output = count("output_tokens");
                self.output.usage.cache_read = count("cache_read_input_tokens");
                self.output.usage.cache_write = count("cache_creation_input_tokens");
                self.update_cost(model);
            }
            "content_block_start" => {
                let index = event["index"].as_u64();
                let block = &event["content_block"];
                match block["type"].as_str().unwrap_or("") {
                    "text" => {
                        let at = self.push_block(Content::Text(TextContent::new("")), index);
                        sender.push(AssistantMessageEvent::TextStart {
                            index: at,
                            partial: self.output.clone(),
                        });
                    }
                    "thinking" => {
                        let at = self.push_block(
                            Content::Thinking(ThinkingContent {
                                thinking: String::new(),
                                signature: Some(String::new()),
                                redacted: false,
                            }),
                            index,
                        );
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            index: at,
                            partial: self.output.clone(),
                        });
                    }
                    "redacted_thinking" => {
                        let at = self.push_block(
                            Content::Thinking(ThinkingContent {
                                thinking: "[Reasoning redacted]".to_string(),
                                signature: block["data"].as_str().map(str::to_string),
                                redacted: true,
                            }),
                            index,
                        );
                        sender.push(AssistantMessageEvent::ThinkingStart {
                            index: at,
                            partial: self.output.clone(),
                        });
                    }
                    "tool_use" => {
                        let name = block["name"].as_str().unwrap_or_default();
                        let name = if self.is_oauth {
                            from_claude_code_name(name, tools)
                        } else {
                            name.to_string()
                        };
                        let arguments = match &block["input"] {
                            Value::Null => serde_json::json!({}),
                            input => input.clone(),
                        };
                        let at = self.push_block(
                            Content::ToolCall(ToolCallContent {
                                id: block["id"].as_str().unwrap_or_default().to_string(),
                                name,
                                arguments,
                                thought_signature: None,
                            }),
                            index,
                        );
                        sender.push(AssistantMessageEvent::ToolCallStart {
                            index: at,
                            partial: self.output.clone(),
                        });
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let Some(at) = self.position(event) else {
                    return Ok(());
                };
                let delta = &event["delta"];
                let text = |key: &str| delta[key].as_str().unwrap_or_default().to_string();
                match (
                    delta["type"].as_str().unwrap_or(""),
                    &mut self.output.content[at],
                ) {
                    ("text_delta", Content::Text(block)) => {
                        let chunk = text("text");
                        block.text.push_str(&chunk);
                        sender.push(AssistantMessageEvent::TextDelta {
                            index: at,
                            delta: chunk,
                            partial: self.output.clone(),
                        });
                    }
                    ("thinking_delta", Content::Thinking(block)) => {
                        let chunk = text("thinking");
                        block.thinking.push_str(&chunk);
                        sender.push(AssistantMessageEvent::ThinkingDelta {
                            index: at,
                            delta: chunk,
                            partial: self.output.clone(),
                        });
                    }
                    ("input_json_delta", Content::ToolCall(block)) => {
                        let chunk = text("partial_json");
                        self.partial_json[at].push_str(&chunk);
                        block.arguments = cortexcode_ai_util::parse_streaming_json::<Value>(Some(
                            &self.partial_json[at],
                        ));
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            index: at,
                            delta: chunk,
                            partial: self.output.clone(),
                        });
                    }
                    ("signature_delta", Content::Thinking(block)) => {
                        block
                            .signature
                            .get_or_insert_with(String::new)
                            .push_str(&text("signature"));
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let Some(at) = self.position(event) else {
                    return Ok(());
                };
                self.indexes[at] = None;
                match &mut self.output.content[at] {
                    Content::Text(_) => sender.push(AssistantMessageEvent::TextEnd {
                        index: at,
                        partial: self.output.clone(),
                    }),
                    Content::Thinking(_) => sender.push(AssistantMessageEvent::ThinkingEnd {
                        index: at,
                        partial: self.output.clone(),
                    }),
                    Content::ToolCall(block) => {
                        block.arguments = cortexcode_ai_util::parse_streaming_json::<Value>(Some(
                            &std::mem::take(&mut self.partial_json[at]),
                        ));
                        sender.push(AssistantMessageEvent::ToolCallEnd {
                            index: at,
                            partial: self.output.clone(),
                        });
                    }
                    Content::Image(_) => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"]
                    .as_str()
                    .filter(|r| !r.is_empty())
                {
                    self.output.stop_reason = map_stop_reason(reason)?;
                }
                // Only fields present (not null): proxies may omit input_tokens here.
                let usage = &event["usage"];
                let usage_out = &mut self.output.usage;
                for (key, slot) in [
                    ("input_tokens", &mut usage_out.input),
                    ("output_tokens", &mut usage_out.output),
                    ("cache_read_input_tokens", &mut usage_out.cache_read),
                    ("cache_creation_input_tokens", &mut usage_out.cache_write),
                ] {
                    if let Some(n) = usage[key].as_u64() {
                        *slot = n;
                    }
                }
                self.update_cost(model);
            }
            _ => {}
        }
        Ok(())
    }
}

/// `mapStopReason`: unknown values throw.
fn map_stop_reason(reason: &str) -> Result<StopReason, String> {
    Ok(match reason {
        "end_turn" => StopReason::Stop,
        "max_tokens" => StopReason::Length,
        "tool_use" => StopReason::ToolUse,
        "refusal" => StopReason::Error,
        // Stop is good enough -> resubmit
        "pause_turn" => StopReason::Stop,
        // We don't supply stop sequences, so this should never happen
        "stop_sequence" => StopReason::Stop,
        // Content flagged by safety filters
        "sensitive" => StopReason::Error,
        other => return Err(format!("Unhandled stop reason: {other}")),
    })
}

/// The message of the `@anthropic-ai/sdk` `APIError` for a non-2xx
/// response: `APIError.makeMessage` over the whole parsed body (or the raw
/// text when it is not JSON).
pub fn api_error_message(status: u16, body: &str) -> String {
    fn truthy(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Null => false,
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
            serde_json::Value::String(s) => !s.is_empty(),
            _ => true,
        }
    }
    // `safeJSON`: the parsed body is the error; unparseable text is the message.
    let msg = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(error) => match error.get("message").filter(|m| truthy(m)) {
            Some(serde_json::Value::String(m)) => m.clone(),
            Some(m) => m.to_string(),
            None if truthy(&error) => error.to_string(),
            None => String::new(),
        },
        Err(_) => body.to_string(),
    };
    if msg.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {msg}")
    }
}

#[cfg(test)]
mod tests;

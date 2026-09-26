//! Google Gemini / Vertex AI provider for cortex AI: port of hoocode
//! `providers/google.ts`, `providers/google-vertex.ts` and
//! `providers/google-shared.ts` (v0.5.89), with the `@google/genai` 1.52
//! client's REST mapping (`request`) in place of the SDK.
//!
//! [`stream`] / [`stream_vertex`] are the `streamSimple*` functions;
//! [`stream_google`] / [`stream_google_vertex`] take [`GoogleOptions`].

mod adc;
mod request;
mod shared;

use std::sync::atomic::{AtomicU64, Ordering};

use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Model,
    SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent,
};
use futures_util::StreamExt;
use serde_json::Value;

pub use request::{
    build_params, gemini_client_config, sdk_body, simple_google_options, simple_vertex_options,
    vertex_client_config, Auth, ClientConfig, Endpoint, GoogleOptions, GoogleThinking, HttpOptions,
};
pub use shared::{
    convert_messages, convert_tools, is_thinking_part, map_stop_reason, map_stop_reason_string,
    map_tool_choice, requires_tool_call_id, retain_thought_signature,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `toolCallCounter`: module-wide, as in TS.
static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `streamSimpleGoogle`: fails before streaming only without an API key.
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
    let options = simple_google_options(&model, &options, api_key);
    Ok(stream_google(model, context, options))
}

/// `streamSimpleGoogleVertex`: credentials are resolved when the request is
/// made, so a missing one is an `error` event.
pub fn stream_vertex(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let options = simple_vertex_options(&model, &options);
    Ok(stream_google_vertex(model, context, options))
}

/// `streamGoogle`.
pub fn stream_google(
    model: Model,
    context: Context,
    options: GoogleOptions,
) -> AssistantMessageEventStream {
    spawn(model, context, options, false)
}

/// `streamGoogleVertex`.
pub fn stream_google_vertex(
    model: Model,
    context: Context,
    options: GoogleOptions,
) -> AssistantMessageEventStream {
    spawn(model, context, options, true)
}

fn spawn(
    model: Model,
    context: Context,
    options: GoogleOptions,
    vertex: bool,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, run(model, context, options, vertex, sender));
    stream
}

async fn run(
    model: Model,
    context: Context,
    options: GoogleOptions,
    vertex: bool,
    sender: AssistantMessageEventStream,
) {
    let mut state = StreamState::new(&model, vertex);
    let signal = options.signal.clone();
    let outcome = match &signal {
        // fetch's AbortError when the signal fires mid-request.
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled(), if !signal.aborted() => Err("This operation was aborted".to_string()),
            r = drive(&model, &context, &options, vertex, &mut state, &sender) => r,
        },
        None => drive(&model, &context, &options, vertex, &mut state, &sender).await,
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

/// The try block of `streamGoogle` / `streamGoogleVertex`.
async fn drive(
    model: &Model,
    context: &Context,
    options: &GoogleOptions,
    vertex: bool,
    state: &mut StreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), String> {
    let config = if vertex {
        vertex_client_config(model, options)?
    } else {
        let api_key = options
            .api_key
            .clone()
            .filter(|k| !k.is_empty())
            .or_else(|| cortexcode_ai_env::get_env_api_key(&model.provider))
            .unwrap_or_default();
        gemini_client_config(model, &api_key, options)
    };
    let params = build_params(model, context, options, vertex)?;
    let endpoint = config.endpoint(&model.id);
    let body = sdk_body(&params, vertex);

    let mut headers = endpoint.headers.clone();
    match &endpoint.auth {
        Auth::ApiKey(key) => headers.push(("x-goog-api-key".into(), key.clone())),
        Auth::Adc => headers.push((
            "Authorization".into(),
            format!("Bearer {}", adc::access_token().await?),
        )),
    }
    let client = reqwest::Client::new();
    let mut request = client.post(&endpoint.url).body(body.to_string());
    for (k, v) in &headers {
        request = request.header(k, v);
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("fetch failed: {e}"))?;
    if !response.status().is_success() {
        return Err(api_error_message(response).await);
    }

    sender.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });
    let mut decoder = ChunkDecoder::default();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| format!("error reading response stream: {e}"))?;
        for data in decoder.push(&chunk)? {
            let value: Value = serde_json::from_str(&data)
                .map_err(|e| format!("exception parsing stream chunk {data}. {e}"))?;
            state.handle_chunk(model, &value, sender)?;
        }
    }
    decoder.finish()?;
    state.close_block(sender);

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

/// `throwErrorIfNotOK`: the `ApiError` message is the JSON body, or a
/// synthesized `{error: {message, code, status}}` for non-JSON bodies.
async fn api_error_message(response: reqwest::Response) -> String {
    let status = response.status();
    let is_json = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"));
    let text = response.text().await.unwrap_or_default();
    if is_json {
        if let Ok(body) = serde_json::from_str::<Value>(&text) {
            return body.to_string();
        }
    }
    serde_json::json!({
        "error": {
            "message": text,
            "code": status.as_u16(),
            "status": status.canonical_reason().unwrap_or_default(),
        }
    })
    .to_string()
}

/// `processStreamResponse` of the SDK's `ApiClient`: events split on the
/// earliest of `\n\n`, `\r\r`, `\r\n\r\n`; `data:` payloads are yielded.
#[derive(Default)]
struct ChunkDecoder {
    buffer: String,
    pending: Vec<u8>,
}

impl ChunkDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        // TextDecoder with `stream: true`: hold back an incomplete UTF-8 tail.
        self.pending.extend_from_slice(chunk);
        let valid = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => self.pending.len(),
        };
        let text = String::from_utf8_lossy(&self.pending[..valid]).into_owned();
        self.pending.drain(..valid);

        // A chunk that is itself an error JSON is an ApiError.
        if let Ok(json) = serde_json::from_str::<Value>(&text) {
            if let Some(error) = json.get("error") {
                if error["code"]
                    .as_u64()
                    .is_some_and(|c| (400..600).contains(&c))
                {
                    return Err(format!(
                        "got status: {}. {json}",
                        error["status"].as_str().unwrap_or("undefined")
                    ));
                }
            }
        }

        self.buffer.push_str(&text);
        let mut events = Vec::new();
        loop {
            let found = ["\n\n", "\r\r", "\r\n\r\n"]
                .iter()
                .filter_map(|d| self.buffer.find(d).map(|i| (i, d.len())))
                .min_by_key(|(i, _)| *i);
            let Some((index, length)) = found else {
                break;
            };
            let event = self.buffer[..index].trim().to_string();
            self.buffer.drain(..index + length);
            if let Some(data) = event.strip_prefix("data:") {
                events.push(data.trim().to_string());
            }
        }
        Ok(events)
    }

    fn finish(&self) -> Result<(), String> {
        if !self.buffer.trim().is_empty() {
            return Err("Incomplete JSON segment at the end".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Streaming state
// ---------------------------------------------------------------------------

struct StreamState {
    output: AssistantMessage,
    /// Index of the open text/thinking block (`currentBlock`).
    current: Option<usize>,
}

impl StreamState {
    fn new(model: &Model, vertex: bool) -> Self {
        let mut output = AssistantMessage::for_model(model);
        output.api = if vertex {
            "google-vertex"
        } else {
            "google-generative-ai"
        }
        .to_string();
        Self {
            output,
            current: None,
        }
    }

    /// Push the end event of the open text/thinking block.
    fn close_block(&mut self, sender: &AssistantMessageEventStream) {
        let Some(index) = self.current.take() else {
            return;
        };
        let partial = self.output.clone();
        match &self.output.content[index] {
            Content::Thinking(_) => {
                sender.push(AssistantMessageEvent::ThinkingEnd { index, partial })
            }
            _ => sender.push(AssistantMessageEvent::TextEnd { index, partial }),
        }
    }

    fn handle_chunk(
        &mut self,
        model: &Model,
        chunk: &Value,
        sender: &AssistantMessageEventStream,
    ) -> Result<(), String> {
        if self.output.response_id.as_deref().unwrap_or("").is_empty() {
            if let Some(id) = chunk["responseId"].as_str().filter(|id| !id.is_empty()) {
                self.output.response_id = Some(id.to_string());
            }
        }
        let candidate = &chunk["candidates"][0];
        if let Some(parts) = candidate["content"]["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    self.handle_text(part, text, sender);
                }
                if let Some(call) = part.get("functionCall").filter(|c| !c.is_null()) {
                    self.handle_function_call(part, call, sender);
                }
            }
        }

        if let Some(reason) = candidate["finishReason"].as_str().filter(|r| !r.is_empty()) {
            self.output.stop_reason = map_stop_reason(reason)?;
            if self
                .output
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)))
            {
                self.output.stop_reason = StopReason::ToolUse;
            }
        }

        if let Some(usage) = chunk.get("usageMetadata").filter(|u| !u.is_null()) {
            let count = |key: &str| usage[key].as_u64().unwrap_or(0);
            let u = &mut self.output.usage;
            u.input = count("promptTokenCount").saturating_sub(count("cachedContentTokenCount"));
            u.output = count("candidatesTokenCount") + count("thoughtsTokenCount");
            u.cache_read = count("cachedContentTokenCount");
            u.cache_write = 0;
            u.total_tokens = count("totalTokenCount");
            u.cost = cortexcode_ai_models::calculate_cost(model, u);
        }
        Ok(())
    }

    fn handle_text(&mut self, part: &Value, text: &str, sender: &AssistantMessageEventStream) {
        let thinking = is_thinking_part(part);
        let open_kind_matches = self
            .current
            .is_some_and(|i| matches!(self.output.content[i], Content::Thinking(_)) == thinking);
        if !open_kind_matches {
            self.close_block(sender);
            let block = if thinking {
                Content::Thinking(ThinkingContent::default())
            } else {
                Content::Text(TextContent::new(""))
            };
            self.output.content.push(block);
            let index = self.output.content.len() - 1;
            self.current = Some(index);
            let partial = self.output.clone();
            sender.push(if thinking {
                AssistantMessageEvent::ThinkingStart { index, partial }
            } else {
                AssistantMessageEvent::TextStart { index, partial }
            });
        }
        let index = self.current.unwrap_or_default();
        let signature = part["thoughtSignature"].as_str();
        match &mut self.output.content[index] {
            Content::Thinking(block) => {
                block.thinking.push_str(text);
                block.signature = retain_thought_signature(block.signature.take(), signature);
            }
            Content::Text(block) => {
                block.text.push_str(text);
                block.text_signature =
                    retain_thought_signature(block.text_signature.take(), signature);
            }
            _ => {}
        }
        let partial = self.output.clone();
        sender.push(if thinking {
            AssistantMessageEvent::ThinkingDelta {
                index,
                delta: text.to_string(),
                partial,
            }
        } else {
            AssistantMessageEvent::TextDelta {
                index,
                delta: text.to_string(),
                partial,
            }
        });
    }

    fn handle_function_call(
        &mut self,
        part: &Value,
        call: &Value,
        sender: &AssistantMessageEventStream,
    ) {
        self.close_block(sender);
        let name = call["name"].as_str().unwrap_or_default().to_string();
        // A new id when none is given or it repeats one in this message.
        let provided = call["id"].as_str().filter(|id| !id.is_empty());
        let duplicate = provided.is_some_and(|id| {
            self.output
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(t) if t.id == id))
        });
        let id = match provided {
            Some(id) if !duplicate => id.to_string(),
            _ => format!(
                "{name}_{}_{}",
                cortexcode_ai_types::now_ms(),
                TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
            ),
        };
        let arguments = match call.get("args") {
            None | Some(Value::Null) => serde_json::json!({}),
            Some(args) => args.clone(),
        };
        let thought_signature = part["thoughtSignature"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self.output.content.push(Content::ToolCall(ToolCallContent {
            id,
            name,
            arguments: arguments.clone(),
            thought_signature,
        }));
        let index = self.output.content.len() - 1;
        sender.push(AssistantMessageEvent::ToolCallStart {
            index,
            partial: self.output.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallDelta {
            index,
            delta: arguments.to_string(),
            partial: self.output.clone(),
        });
        sender.push(AssistantMessageEvent::ToolCallEnd {
            index,
            partial: self.output.clone(),
        });
    }
}

#[cfg(test)]
mod tests;

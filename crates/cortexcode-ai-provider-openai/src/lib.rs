//! OpenAI Chat Completions provider (`openai-completions` API) for cortex AI.
//!
//! Port of hoocode `providers/openai-completions.ts` (v0.5.89):
//! `streamSimpleOpenAICompletions` / `streamOpenAICompletions`. Serves every
//! OpenAI-compatible provider; per-provider quirks come from the resolved
//! compat ([`request::get_compat`]).
//!
//! Known deviations: the `openai` SDK's own client retries (2 by default,
//! with the `retryDelayCapFetch` cap) and `describeProviderError`'s
//! retry-after suffix are not ported yet (retry utilities, 8.5), and the
//! `onPayload` / `onResponse` hooks are not supported.

pub mod request;

use std::collections::HashMap;

use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Model,
    SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent, Usage,
};
use cortexcode_ai_util::{droppable_params_named_by, note_rejected_params, DROPPABLE_PARAMS};
use futures_util::StreamExt;
use request::CompletionsOptions;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `streamSimpleOpenAICompletions`: resolve the API key (explicit, else the
/// provider's environment variable), map the simple options and stream.
/// A missing key is an immediate `Err`, as the TS function throws.
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
    let options = request::simple_options(&model, &options, api_key);
    Ok(stream_completions(model, context, options))
}

/// `streamOpenAICompletions`: every failure (HTTP errors, provider error
/// stop reasons, aborts) ends the stream with an `error` event.
pub fn stream_completions(
    model: Model,
    context: Context,
    options: CompletionsOptions,
) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(&stream, run(model, context, options, sender));
    stream
}

async fn run(
    model: Model,
    context: Context,
    options: CompletionsOptions,
    sender: AssistantMessageEventStream,
) {
    let mut state = StreamState::new(&model);
    let signal = options.signal.clone();
    let outcome = match &signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled() => Err(RequestError::plain("Request was aborted")),
            r = drive(&model, &context, &options, &mut state, &sender) => r,
        },
        None => drive(&model, &context, &options, &mut state, &sender).await,
    };
    match outcome {
        Ok(()) => {
            let message = state.output.clone();
            sender.push(AssistantMessageEvent::Done {
                message: message.clone(),
            });
            sender.end(Some(message));
        }
        Err(error) => {
            let aborted = signal.as_ref().is_some_and(AbortSignal::aborted);
            let mut output = state.output;
            output.stop_reason = if aborted {
                StopReason::Aborted
            } else {
                StopReason::Error
            };
            let mut message = error.message;
            // Some providers behind OpenRouter explain themselves here.
            if let Some(raw) = error.raw_metadata {
                message.push('\n');
                message.push_str(&raw);
            }
            output.error_message = Some(message);
            sender.push(AssistantMessageEvent::Error {
                error: output.clone(),
            });
            sender.end(Some(output));
        }
    }
}

/// A failed request: the message for `errorMessage`, plus what the param
/// fallback and the OpenRouter metadata suffix need.
struct RequestError {
    message: String,
    status: Option<u16>,
    body: String,
    raw_metadata: Option<String>,
}

impl RequestError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            body: String::new(),
            raw_metadata: None,
        }
    }

    fn http(status: u16, body: String) -> Self {
        let parsed: Option<serde_json::Value> = serde_json::from_str(&body).ok();
        let raw_metadata = parsed
            .as_ref()
            .map(|v| &v["error"]["metadata"]["raw"])
            .and_then(|raw| match raw {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) if s.is_empty() => None,
                serde_json::Value::String(s) => Some(s.clone()),
                other => Some(other.to_string()),
            });
        Self {
            message: api_error_message(status, &body),
            status: Some(status),
            body,
            raw_metadata,
        }
    }
}

async fn drive(
    model: &Model,
    context: &Context,
    options: &CompletionsOptions,
    state: &mut StreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), RequestError> {
    let compat = request::get_compat(model);
    let cache_retention = cortexcode_ai_util::resolve_cache_retention(options.cache_retention);
    let cache_session_id = (cache_retention != cortexcode_ai_types::CacheRetention::None)
        .then_some(options.session_id.as_deref())
        .flatten();
    let api_key = options.api_key.clone().unwrap_or_default();
    let headers = request::build_headers(
        model,
        context,
        &api_key,
        options.headers.as_ref(),
        cache_session_id,
        &compat,
    );
    let params = request::build_params(model, context, options, &compat, &cache_retention);
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));

    let response = create_with_param_fallback(model, &url, &headers, params, options).await?;
    sender.push(AssistantMessageEvent::Start {
        partial: state.output.clone(),
    });

    let mut events = cortexcode_ai_sse::sse_events(response.bytes_stream());
    while let Some(frame) = events.next().await {
        let frame = frame
            .map_err(|e| RequestError::plain(format!("error reading response stream: {e}")))?;
        let payload = frame.data.trim();
        if payload.is_empty() {
            continue;
        }
        if payload == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        state.handle_chunk(model, &chunk, sender);
    }

    state.finish_blocks(sender);
    if options.signal.as_ref().is_some_and(AbortSignal::aborted) {
        return Err(RequestError::plain("Request was aborted"));
    }
    match state.output.stop_reason {
        StopReason::Aborted => Err(RequestError::plain("Request was aborted")),
        StopReason::Error => Err(RequestError::plain(
            state
                .output
                .error_message
                .clone()
                .unwrap_or_else(|| "Provider returned an error stop reason".to_string()),
        )),
        _ => Ok(()),
    }
}

/// `createWithParamFallback`: send the request; when a 4xx names optional
/// params we added, drop them, remember the refusal for this base URL and
/// send again (at most once per droppable param).
async fn create_with_param_fallback(
    model: &Model,
    url: &str,
    headers: &[(String, String)],
    params: serde_json::Value,
    options: &CompletionsOptions,
) -> Result<reqwest::Response, RequestError> {
    let mut builder = reqwest::Client::builder();
    if let Some(ms) = options.timeout_ms {
        builder = builder.timeout(std::time::Duration::from_millis(ms));
    }
    let client = builder
        .build()
        .map_err(|e| RequestError::plain(format!("failed to build HTTP client: {e}")))?;

    let mut attempt = params;
    let mut passes = DROPPABLE_PARAMS.len();
    loop {
        let mut request = client.post(url);
        for (k, v) in headers {
            request = request.header(k, v);
        }
        let response = request
            .json(&attempt)
            .send()
            .await
            .map_err(|e| RequestError::plain(format!("Connection error: {e}")))?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        let error = RequestError::http(status, body);
        let text = format!("{} {}", error.message, error.body);
        let named: Vec<&'static str> = droppable_params_named_by(error.status, &text)
            .into_iter()
            .filter(|p| attempt.get(*p).is_some_and(|v| !v.is_null()))
            .collect();
        if named.is_empty() || passes == 0 {
            return Err(error);
        }
        passes -= 1;
        note_rejected_params(&model.base_url, &named);
        if let Some(obj) = attempt.as_object_mut() {
            for p in named {
                obj.shift_remove(p);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streaming state machine
// ---------------------------------------------------------------------------

/// Scratch state of a streaming tool-call block (`partialArgs`, `streamIndex`).
struct ToolCallScratch {
    partial_args: String,
}

struct StreamState {
    output: AssistantMessage,
    text_block: Option<usize>,
    thinking_block: Option<usize>,
    tool_calls_by_index: HashMap<i64, usize>,
    tool_calls_by_id: HashMap<String, usize>,
    /// Stream index of each tool-call block, by content index.
    stream_index: HashMap<usize, i64>,
    scratch: HashMap<usize, ToolCallScratch>,
}

impl StreamState {
    fn new(model: &Model) -> Self {
        let mut output = AssistantMessage::for_model(model);
        output.content.clear();
        output.stop_reason = StopReason::Stop;
        output.usage = Usage::default();
        output.timestamp = now_millis();
        Self {
            output,
            text_block: None,
            thinking_block: None,
            tool_calls_by_index: HashMap::new(),
            tool_calls_by_id: HashMap::new(),
            stream_index: HashMap::new(),
            scratch: HashMap::new(),
        }
    }

    fn ensure_text_block(&mut self, sender: &AssistantMessageEventStream) -> usize {
        if let Some(idx) = self.text_block {
            return idx;
        }
        let idx = self.output.content.len();
        self.output
            .content
            .push(Content::Text(TextContent::new("")));
        self.text_block = Some(idx);
        sender.push(AssistantMessageEvent::TextStart {
            index: idx,
            partial: self.output.clone(),
        });
        idx
    }

    fn ensure_thinking_block(
        &mut self,
        signature: &str,
        sender: &AssistantMessageEventStream,
    ) -> usize {
        if let Some(idx) = self.thinking_block {
            return idx;
        }
        let idx = self.output.content.len();
        self.output.content.push(Content::Thinking(ThinkingContent {
            thinking: String::new(),
            signature: Some(signature.to_string()),
            redacted: false,
        }));
        self.thinking_block = Some(idx);
        sender.push(AssistantMessageEvent::ThinkingStart {
            index: idx,
            partial: self.output.clone(),
        });
        idx
    }

    /// `ensureToolCallBlock`: find the block by stream index, else by id,
    /// else start a new one.
    fn ensure_tool_call_block(
        &mut self,
        delta: &serde_json::Value,
        sender: &AssistantMessageEventStream,
    ) -> usize {
        let stream_index = delta["index"].as_i64();
        let id = delta["id"].as_str().filter(|s| !s.is_empty());
        let mut block = stream_index.and_then(|i| self.tool_calls_by_index.get(&i).copied());
        if block.is_none() {
            block = id.and_then(|id| self.tool_calls_by_id.get(id).copied());
        }
        let idx = match block {
            Some(idx) => idx,
            None => {
                let idx = self.output.content.len();
                self.output.content.push(Content::ToolCall(ToolCallContent {
                    id: id.unwrap_or_default().to_string(),
                    name: delta["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                }));
                self.scratch.insert(
                    idx,
                    ToolCallScratch {
                        partial_args: String::new(),
                    },
                );
                if let Some(i) = stream_index {
                    self.tool_calls_by_index.insert(i, idx);
                    self.stream_index.insert(idx, i);
                }
                if let Some(id) = id {
                    self.tool_calls_by_id.insert(id.to_string(), idx);
                }
                sender.push(AssistantMessageEvent::ToolCallStart {
                    index: idx,
                    partial: self.output.clone(),
                });
                idx
            }
        };
        if let Some(i) = stream_index {
            if let std::collections::hash_map::Entry::Vacant(e) = self.stream_index.entry(idx) {
                e.insert(i);
                self.tool_calls_by_index.insert(i, idx);
            }
        }
        if let Some(id) = id {
            self.tool_calls_by_id.insert(id.to_string(), idx);
        }
        idx
    }

    fn handle_chunk(
        &mut self,
        model: &Model,
        chunk: &serde_json::Value,
        sender: &AssistantMessageEventStream,
    ) {
        if !chunk.is_object() {
            return;
        }
        if self.output.response_id.is_none() {
            if let Some(id) = chunk["id"].as_str().filter(|s| !s.is_empty()) {
                self.output.response_id = Some(id.to_string());
            }
        }
        if let Some(m) = chunk["model"].as_str() {
            if !m.is_empty() && m != model.id && self.output.response_model.is_none() {
                self.output.response_model = Some(m.to_string());
            }
        }
        let chunk_usage = chunk.get("usage").filter(|u| u.is_object());
        if let Some(usage) = chunk_usage {
            self.output.usage = parse_chunk_usage(usage, model);
        }

        let Some(choice) = chunk["choices"].as_array().and_then(|c| c.first()) else {
            return;
        };
        if !choice.is_object() {
            return;
        }
        // Some providers (e.g. Moonshot) report usage on the choice.
        if chunk_usage.is_none() {
            if let Some(usage) = choice.get("usage").filter(|u| u.is_object()) {
                self.output.usage = parse_chunk_usage(usage, model);
            }
        }

        if let Some(reason) = choice["finish_reason"].as_str() {
            let (stop_reason, error_message) = map_stop_reason(reason);
            self.output.stop_reason = stop_reason;
            if let Some(msg) = error_message {
                self.output.error_message = Some(msg);
            }
        }

        let delta = &choice["delta"];
        if !delta.is_object() {
            return;
        }

        if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            let idx = self.ensure_text_block(sender);
            if let Content::Text(t) = &mut self.output.content[idx] {
                t.text.push_str(text);
            }
            sender.push(AssistantMessageEvent::TextDelta {
                index: idx,
                delta: text.to_string(),
                partial: self.output.clone(),
            });
        }

        // llama.cpp uses `reasoning_content`, other endpoints `reasoning` or
        // `reasoning_text`; take the first non-empty one (chutes.ai sends two).
        let reasoning = ["reasoning_content", "reasoning", "reasoning_text"]
            .into_iter()
            .find_map(|field| {
                delta[field]
                    .as_str()
                    .filter(|v| !v.is_empty())
                    .map(|v| (field, v))
            });
        if let Some((field, text)) = reasoning {
            let idx = self.ensure_thinking_block(field, sender);
            if let Content::Thinking(t) = &mut self.output.content[idx] {
                t.thinking.push_str(text);
            }
            sender.push(AssistantMessageEvent::ThinkingDelta {
                index: idx,
                delta: text.to_string(),
                partial: self.output.clone(),
            });
        }

        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                let idx = self.ensure_tool_call_block(tc, sender);
                let id = tc["id"].as_str().filter(|s| !s.is_empty());
                let name = tc["function"]["name"].as_str().filter(|s| !s.is_empty());
                let args = tc["function"]["arguments"]
                    .as_str()
                    .filter(|s| !s.is_empty());
                let mut parsed = None;
                if let Some(args) = args {
                    let scratch = self.scratch.get_mut(&idx).expect("tool call scratch");
                    scratch.partial_args.push_str(args);
                    parsed = Some(
                        cortexcode_ai_util::parse_streaming_json::<serde_json::Value>(Some(
                            &scratch.partial_args,
                        )),
                    );
                }
                if let Content::ToolCall(block) = &mut self.output.content[idx] {
                    if block.id.is_empty() {
                        if let Some(id) = id {
                            block.id = id.to_string();
                            self.tool_calls_by_id.insert(id.to_string(), idx);
                        }
                    }
                    if block.name.is_empty() {
                        if let Some(name) = name {
                            block.name = name.to_string();
                        }
                    }
                    if let Some(parsed) = parsed {
                        block.arguments = normalize_arguments(parsed);
                    }
                }
                sender.push(AssistantMessageEvent::ToolCallDelta {
                    index: idx,
                    delta: args.unwrap_or_default().to_string(),
                    partial: self.output.clone(),
                });
            }
        }

        if let Some(details) = delta["reasoning_details"].as_array() {
            for detail in details {
                let (Some(id), Some(_)) = (
                    detail["id"].as_str().filter(|s| !s.is_empty()),
                    detail["data"].as_str().filter(|s| !s.is_empty()),
                ) else {
                    continue;
                };
                if detail["type"] != "reasoning.encrypted" {
                    continue;
                }
                let matching = self.output.content.iter_mut().find_map(|b| match b {
                    Content::ToolCall(tc) if tc.id == id => Some(tc),
                    _ => None,
                });
                if let Some(tc) = matching {
                    tc.thought_signature = Some(detail.to_string());
                }
            }
        }
    }

    /// `finishBlock` for every block, in content order.
    fn finish_blocks(&mut self, sender: &AssistantMessageEventStream) {
        for idx in 0..self.output.content.len() {
            match &mut self.output.content[idx] {
                Content::Text(_) => sender.push(AssistantMessageEvent::TextEnd {
                    index: idx,
                    partial: self.output.clone(),
                }),
                Content::Thinking(_) => sender.push(AssistantMessageEvent::ThinkingEnd {
                    index: idx,
                    partial: self.output.clone(),
                }),
                Content::ToolCall(block) => {
                    let partial = self
                        .scratch
                        .remove(&idx)
                        .map(|s| s.partial_args)
                        .unwrap_or_default();
                    block.arguments =
                        normalize_arguments(cortexcode_ai_util::parse_streaming_json::<
                            serde_json::Value,
                        >(Some(&partial)));
                    sender.push(AssistantMessageEvent::ToolCallEnd {
                        index: idx,
                        partial: self.output.clone(),
                    });
                }
                Content::Image(_) => {}
            }
        }
    }
}

/// `parseStreamingJson` yields `{}` for nothing parseable; keep arguments an
/// object as TS does.
fn normalize_arguments(v: serde_json::Value) -> serde_json::Value {
    if v.is_null() {
        serde_json::json!({})
    } else {
        v
    }
}

/// `parseChunkUsage`: normalize to cacheRead = hits from earlier requests,
/// cacheWrite = tokens written now (OpenRouter counts writes in
/// `cached_tokens`), then price it.
fn parse_chunk_usage(raw: &serde_json::Value, model: &Model) -> Usage {
    let prompt_tokens = raw["prompt_tokens"].as_u64().unwrap_or(0);
    let reported_cached = raw["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| raw["prompt_cache_hit_tokens"].as_u64())
        .unwrap_or(0);
    let cache_write = raw["prompt_tokens_details"]["cache_write_tokens"]
        .as_u64()
        .unwrap_or(0);
    let cache_read = if cache_write > 0 {
        reported_cached.saturating_sub(cache_write)
    } else {
        reported_cached
    };
    let input = prompt_tokens
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    // completion_tokens already includes reasoning tokens.
    let output = raw["completion_tokens"].as_u64().unwrap_or(0);
    let mut usage = Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output + cache_read + cache_write,
        cost: Default::default(),
    };
    usage.cost = cortexcode_ai_models::calculate_cost(model, &usage);
    usage
}

/// `mapStopReason`.
fn map_stop_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub use cortexcode_ai_util::openai_api_error_message as api_error_message;

#[cfg(test)]
mod tests;

//! Anthropic provider for cortex AI.
//!
//! Implements streaming against the Anthropic Messages API
//! (`POST {base_url}/v1/messages`, `stream: true`), translating the
//! provider's SSE event stream into [`AssistantMessageEvent`]s.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/anthropic.ts`.

mod request;

use std::collections::HashMap;

use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Context, Cost, Model,
    SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent, Usage,
};
use futures_util::StreamExt;

pub use request::Credential;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Stream a completion from the Anthropic Messages API.
///
/// Setup failures (e.g. missing credentials) are returned as `Err` before any
/// network call is made; everything else (network errors, HTTP error
/// responses, malformed payloads, abort) is reported as an `Error` event on
/// the returned stream.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let credential = request::resolve_credentials(&options).map_err(BoxError::from)?;
    let headers = request::build_headers(&model, &credential);
    let body = request::build_request_body(&model, &context, &options);
    let url = format!("{}/v1/messages", model.base_url.trim_end_matches('/'));

    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    let template = AssistantMessage::for_model(&model);
    spawn_producer(
        &stream,
        run_stream(url, headers, body, sender, template, options.signal),
    );
    Ok(stream)
}

// ---------------------------------------------------------------------------
// HTTP + SSE driving
// ---------------------------------------------------------------------------

async fn run_stream(
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
    sender: AssistantMessageEventStream,
    template: AssistantMessage,
    signal: Option<AbortSignal>,
) {
    let mut state = StreamState::new(template);
    let outcome = match &signal {
        Some(signal) => tokio::select! {
            biased;
            _ = signal.cancelled() => Err("Request was aborted".to_string()),
            r = drive(&url, &headers, &body, &mut state, &sender) => r,
        },
        None => drive(&url, &headers, &body, &mut state, &sender).await,
    };
    if let Err(message) = outcome {
        let aborted = signal.as_ref().is_some_and(AbortSignal::aborted);
        let error = state.error_output(message, aborted);
        sender.push(AssistantMessageEvent::Error {
            error: error.clone(),
        });
        sender.end(Some(error));
    }
}

/// Send the request and feed the SSE events to `state`. `Err` carries the
/// error message for the terminal `error` event.
async fn drive(
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
    state: &mut StreamState,
    sender: &AssistantMessageEventStream,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;
    let mut request = client.post(url);
    for (k, v) in headers {
        request = request.header(k, v);
    }
    let response = request
        .json(body)
        .send()
        .await
        .map_err(|e| format!("request to Anthropic API failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic API returned {status}: {text}"));
    }

    let mut events = cortexcode_ai_sse::sse_events(response.bytes_stream());
    while let Some(frame) = events.next().await {
        let frame = frame.map_err(|e| format!("error reading response stream: {e}"))?;
        if frame.data.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&frame.data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if state.handle_event(&value, sender) {
            // Terminal event (message_stop or error) — stop reading.
            return Ok(());
        }
    }

    // Connection closed without an explicit terminal event.
    state.finish(sender);
    Ok(())
}

// ---------------------------------------------------------------------------
// Streaming state machine
// ---------------------------------------------------------------------------

enum BlockKind {
    Text,
    Thinking,
    ToolUse { id: String, name: String },
}

struct StreamState {
    partial: AssistantMessage,
    block_kinds: HashMap<usize, BlockKind>,
    text_buffers: HashMap<usize, String>,
    thinking_buffers: HashMap<usize, String>,
    tool_json_buffers: HashMap<usize, String>,
    thinking_signatures: HashMap<usize, String>,
    started: bool,
}

impl StreamState {
    fn new(template: AssistantMessage) -> Self {
        Self {
            partial: AssistantMessage {
                provider: template.provider.clone(),
                response_id: None,
                response_model: None,
                api: template.api.clone(),
                diagnostics: None,
                model: template.model.clone(),
                content: vec![],
                stop_reason: StopReason::Stop,

                usage: Default::default(),
                timestamp: now_millis(),
                error_message: None,
            },
            block_kinds: HashMap::new(),
            text_buffers: HashMap::new(),
            thinking_buffers: HashMap::new(),
            tool_json_buffers: HashMap::new(),
            thinking_signatures: HashMap::new(),
            started: false,
        }
    }

    /// Handle one decoded SSE JSON payload. Returns `true` if this was a
    /// terminal event and the caller should stop reading.
    fn handle_event(
        &mut self,
        value: &serde_json::Value,
        sender: &AssistantMessageEventStream,
    ) -> bool {
        let event_type = value["type"].as_str().unwrap_or("");

        match event_type {
            "message_start" => {
                if !self.started {
                    self.started = true;
                    if let Some(usage) = value["message"].get("usage") {
                        self.partial.usage = parse_usage(usage, None);
                    }
                    sender.push(AssistantMessageEvent::Start {
                        partial: self.partial.clone(),
                    });
                }
                false
            }
            "content_block_start" => {
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                let block = &value["content_block"];
                let kind = match block["type"].as_str().unwrap_or("") {
                    "thinking" => BlockKind::Thinking,
                    "tool_use" => BlockKind::ToolUse {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                    },
                    _ => BlockKind::Text,
                };
                let start_event = match kind {
                    BlockKind::Text => AssistantMessageEvent::TextStart {
                        index,
                        partial: self.partial.clone(),
                    },
                    BlockKind::Thinking => AssistantMessageEvent::ThinkingStart {
                        index,
                        partial: self.partial.clone(),
                    },
                    BlockKind::ToolUse { .. } => AssistantMessageEvent::ToolCallStart {
                        index,
                        partial: self.partial.clone(),
                    },
                };
                self.block_kinds.insert(index, kind);
                sender.push(start_event);
                false
            }
            "content_block_delta" => {
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                let delta = &value["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        self.text_buffers.entry(index).or_default().push_str(text);
                        sender.push(AssistantMessageEvent::TextDelta {
                            index,
                            delta: text.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                    "thinking_delta" => {
                        let text = delta["thinking"].as_str().unwrap_or_default();
                        self.thinking_buffers
                            .entry(index)
                            .or_default()
                            .push_str(text);
                        sender.push(AssistantMessageEvent::ThinkingDelta {
                            index,
                            delta: text.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                    "signature_delta" => {
                        let sig = delta["signature"].as_str().unwrap_or_default().to_string();
                        self.thinking_signatures
                            .entry(index)
                            .or_default()
                            .push_str(&sig);
                    }
                    "input_json_delta" => {
                        let chunk = delta["partial_json"].as_str().unwrap_or_default();
                        let buf = self.tool_json_buffers.entry(index).or_default();
                        buf.push_str(chunk);
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            index,
                            delta: chunk.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                    _ => {}
                }
                false
            }
            "content_block_stop" => {
                let index = value["index"].as_u64().unwrap_or(0) as usize;
                self.finalize_block(index, sender);
                false
            }
            "message_delta" => {
                if let Some(stop_reason) = value["delta"]["stop_reason"].as_str() {
                    match map_stop_reason(stop_reason) {
                        Ok(reason) => self.partial.stop_reason = reason,
                        Err(message) => {
                            self.partial.stop_reason = StopReason::Error;
                            self.partial.error_message = Some(message);
                        }
                    }
                }
                if let Some(usage) = value.get("usage") {
                    self.partial.usage = parse_usage(usage, Some(&self.partial.usage));
                }
                false
            }
            "message_stop" => {
                self.finish(sender);
                true
            }
            "error" => {
                let message = value["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown Anthropic API error")
                    .to_string();
                self.partial.stop_reason = StopReason::Error;
                self.partial.error_message = Some(message);
                sender.push(AssistantMessageEvent::Error {
                    error: self.partial.clone(),
                });
                sender.end(Some(self.partial.clone()));
                true
            }
            _ => false,
        }
    }

    fn finalize_block(&mut self, index: usize, sender: &AssistantMessageEventStream) {
        let Some(kind) = self.block_kinds.remove(&index) else {
            return;
        };
        match kind {
            BlockKind::Text => {
                let text = self.text_buffers.remove(&index).unwrap_or_default();
                self.partial.content.push(Content::Text(TextContent {
                    text_signature: None,
                    text,
                    cache_control: None,
                }));
                sender.push(AssistantMessageEvent::TextEnd {
                    index,
                    partial: self.partial.clone(),
                });
            }
            BlockKind::Thinking => {
                let thinking = self.thinking_buffers.remove(&index).unwrap_or_default();
                let signature = self.thinking_signatures.remove(&index);
                self.partial
                    .content
                    .push(Content::Thinking(ThinkingContent {
                        redacted: false,
                        thinking,
                        signature,
                    }));
                sender.push(AssistantMessageEvent::ThinkingEnd {
                    index,
                    partial: self.partial.clone(),
                });
            }
            BlockKind::ToolUse { id, name } => {
                let raw = self.tool_json_buffers.remove(&index).unwrap_or_default();
                let arguments: serde_json::Value = if raw.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    cortexcode_ai_util::parse_json_with_repair(&raw)
                        .unwrap_or_else(|_| serde_json::json!({}))
                };
                self.partial
                    .content
                    .push(Content::ToolCall(ToolCallContent {
                        thought_signature: None,
                        id,
                        name,
                        arguments,
                    }));
                sender.push(AssistantMessageEvent::ToolCallEnd {
                    index,
                    partial: self.partial.clone(),
                });
            }
        }
    }

    /// The message for a terminal `error` event: everything streamed so far,
    /// including blocks still open (as hoocode's in-place `output`), with
    /// `stopReason` `aborted` when the signal fired and `error` otherwise.
    fn error_output(&self, message: String, aborted: bool) -> AssistantMessage {
        let mut output = self.partial.clone();
        let mut open: Vec<_> = self.block_kinds.iter().collect();
        open.sort_by_key(|(index, _)| **index);
        for (index, kind) in open {
            output.content.push(match kind {
                BlockKind::Text => Content::Text(TextContent {
                    text_signature: None,
                    text: self.text_buffers.get(index).cloned().unwrap_or_default(),
                    cache_control: None,
                }),
                BlockKind::Thinking => Content::Thinking(ThinkingContent {
                    redacted: false,
                    thinking: self
                        .thinking_buffers
                        .get(index)
                        .cloned()
                        .unwrap_or_default(),
                    signature: self.thinking_signatures.get(index).cloned(),
                }),
                BlockKind::ToolUse { id, name } => Content::ToolCall(ToolCallContent {
                    thought_signature: None,
                    id: id.clone(),
                    name: name.clone(),
                    arguments: self
                        .tool_json_buffers
                        .get(index)
                        .filter(|raw| !raw.trim().is_empty())
                        .and_then(|raw| cortexcode_ai_util::parse_json_with_repair(raw).ok())
                        .unwrap_or_else(|| serde_json::json!({})),
                }),
            });
        }
        output.stop_reason = if aborted {
            StopReason::Aborted
        } else {
            StopReason::Error
        };
        output.error_message = Some(message);
        output
    }

    fn finish(&mut self, sender: &AssistantMessageEventStream) {
        sender.push(AssistantMessageEvent::Done {
            message: self.partial.clone(),
        });
        sender.end(Some(self.partial.clone()));
    }
}

fn parse_usage(value: &serde_json::Value, previous: Option<&Usage>) -> Usage {
    let input = value["input_tokens"]
        .as_u64()
        .or_else(|| previous.map(|u| u.input))
        .unwrap_or(0);
    let cache_read = value["cache_read_input_tokens"]
        .as_u64()
        .or_else(|| previous.map(|u| u.cache_read))
        .unwrap_or(0);
    let cache_write = value["cache_creation_input_tokens"]
        .as_u64()
        .or_else(|| previous.map(|u| u.cache_write))
        .unwrap_or(0);
    let output = value["output_tokens"].as_u64().unwrap_or(0);

    Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output + cache_read + cache_write,
        cost: Cost::default(),
    }
}

/// Port of `mapStopReason` in `providers/anthropic.ts`. Unknown values are an
/// error there (`throw`), which ends the stream with `stopReason: "error"`.
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

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_stream::testing::{serve_error, serve_sse, serve_sse_then_hang};
    use std::time::Duration;

    fn spawn_mock_server(sse_body: &'static str) -> String {
        serve_sse(sse_body)
    }

    fn spawn_mock_error_server(status_line: &'static str, body: &'static str) -> String {
        serve_error(status_line, body)
    }

    fn test_model(base_url: String) -> Model {
        Model {
            compat: None,
            id: "claude-test".into(),
            name: "Claude Test".into(),
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            base_url,
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 200_000,
            max_tokens: 4096,
            headers: None,
        }
    }

    fn collect(mut s: AssistantMessageEventStream) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        while let Some(e) = s.next_blocking() {
            events.push(e);
        }
        events
    }

    #[test]
    fn test_stream_missing_credentials_errors_immediately() {
        std::env::remove_var("ANTHROPIC_API_KEY");
        std::env::remove_var("ANTHROPIC_OAUTH_TOKEN");
        let model = test_model("http://127.0.0.1:0".into());
        let context = Context::new("".into(), vec![], vec![]);
        let result = stream(model, context, SimpleStreamOptions::default());
        assert!(result.is_err());
    }

    #[test]
    fn test_stream_text_response() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\", world\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);

        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        let deltas: String = events
            .iter()
            .filter_map(|e| match e {
                AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, "Hello, world");

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert_eq!(message.stop_reason, StopReason::Stop);
                let usage = &message.usage;
                assert_eq!(usage.input, 10);
                assert_eq!(usage.output, 5);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_tool_call_response() {
        let sse = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":8}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);

        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert_eq!(message.stop_reason, StopReason::ToolUse);
                assert_eq!(message.content.len(), 1);
                match &message.content[0] {
                    Content::ToolCall(tc) => {
                        assert_eq!(tc.name, "read_file");
                        assert_eq!(tc.id, "call_1");
                        assert_eq!(tc.arguments["path"], "a.rs");
                    }
                    other => panic!("expected tool call content, got {other:?}"),
                }
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_http_error_response() {
        let base_url = spawn_mock_error_server(
            "HTTP/1.1 429 Too Many Requests",
            "{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"rate limited\"}}",
        );
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error } => {
                assert_eq!(error.stop_reason, StopReason::Error);
                assert!(error.error_message.as_ref().unwrap().contains("429"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_api_level_error_event() {
        let sse = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"overloaded\"}}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error } => {
                assert_eq!(error.error_message, Some("overloaded".to_string()));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// `testAbortSignal` in `abort.test.ts`, against a server that stalls
    /// mid-message: aborting ends the stream with `aborted` and keeps what
    /// was streamed so far.
    #[test]
    fn test_abort_mid_stream_keeps_partial_content() {
        let head = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"15 + 27 = 42. \"}}\n\n",
        );
        let base_url = serve_sse_then_hang(head, Duration::from_secs(30));
        let signal = AbortSignal::new();
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            signal: Some(signal.clone()),
            ..Default::default()
        };
        let mut s = stream(
            test_model(base_url),
            Context::new("".into(), vec![], vec![]),
            options,
        )
        .unwrap();

        let started = std::time::Instant::now();
        let mut text = String::new();
        while let Some(event) = s.next_blocking() {
            if let AssistantMessageEvent::TextDelta { delta, .. } = &event {
                text.push_str(delta);
                if text.len() >= 10 {
                    signal.abort();
                }
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "abort must not wait for the server"
        );

        let msg = s.result_blocking();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
        assert_eq!(msg.error_message.as_deref(), Some("Request was aborted"));
        assert_eq!(msg.usage.input, 10);
        match msg.content.as_slice() {
            [Content::Text(t)] => assert_eq!(t.text, "15 + 27 = 42. "),
            other => panic!("expected the partial text block, got {other:?}"),
        }
    }

    /// `testImmediateAbort` in `abort.test.ts`.
    #[test]
    fn test_immediate_abort() {
        let signal = AbortSignal::new();
        signal.abort();
        let options = SimpleStreamOptions {
            api_key: Some("sk-test".into()),
            signal: Some(signal),
            ..Default::default()
        };
        let s = stream(
            test_model("http://127.0.0.1:9".into()),
            Context::new("".into(), vec![], vec![]),
            options,
        )
        .unwrap();
        let msg = s.result_blocking();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
        assert!(msg.content.is_empty());
    }

    #[test]
    fn test_map_stop_reason() {
        assert_eq!(map_stop_reason("end_turn"), Ok(StopReason::Stop));
        assert_eq!(map_stop_reason("tool_use"), Ok(StopReason::ToolUse));
        assert_eq!(map_stop_reason("max_tokens"), Ok(StopReason::Length));
        assert_eq!(map_stop_reason("refusal"), Ok(StopReason::Error));
        assert_eq!(map_stop_reason("pause_turn"), Ok(StopReason::Stop));
        assert_eq!(map_stop_reason("sensitive"), Ok(StopReason::Error));
        assert_eq!(
            map_stop_reason("weird"),
            Err("Unhandled stop reason: weird".to_string())
        );
    }
}

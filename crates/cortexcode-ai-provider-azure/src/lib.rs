//! Azure OpenAI provider for cortex AI.
//!
//! Implements streaming against Azure's OpenAI Responses API
//! (`POST {base_url}/responses?api-version={version}`, `stream: true`),
//! translating the SSE event stream into [`AssistantMessageEvent`]s.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/azure-openai-responses.ts` and
//! `providers/openai-responses-shared.ts`. Reasoning-item ID pairing is
//! ported: streamed function calls carry their Responses item id encoded as
//! `call_id|item_id` on [`ToolCallContent::id`], and reasoning items are
//! stored verbatim in [`ThinkingContent::signature`] so both can be replayed
//! on the next turn (see `request.rs`). The cross-provider / different-model
//! foreign-id remapping from the TS source is not ported, since the Rust
//! `AssistantMessage` does not carry the originating provider/model.

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

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Stream a completion from the Azure OpenAI Responses API.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = request::resolve_credentials(&options).map_err(BoxError::from)?;
    let (base_url, api_version) = request::resolve_azure_config(&model).map_err(BoxError::from)?;
    let deployment_name = request::resolve_deployment_name(&model.id);
    let body = request::build_request_body(&model, &context, &deployment_name);
    let url = format!(
        "{}/responses?api-version={}",
        base_url.trim_end_matches('/'),
        api_version
    );

    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("api-key".to_string(), api_key),
    ];
    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            headers.push((k.clone(), v.clone()));
        }
    }

    let template = AssistantMessage::for_model(&model);
    let signal = options.signal.clone();
    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    spawn_producer(
        &stream,
        run_stream(url, headers, body, sender, template, signal),
    );
    Ok(stream)
}

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
        .map_err(|e| format!("request to Azure OpenAI API failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Azure OpenAI API returned {status}: {text}"));
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
            return Ok(());
        }
    }

    state.finish(sender);
    Ok(())
}

// ---------------------------------------------------------------------------
// Streaming state machine
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum BlockKind {
    Text,
    Thinking,
    ToolCall {
        call_id: String,
        name: String,
        /// The Responses API item id (e.g. `fc_...`) for this function call.
        /// Preserved so it can be paired back with its `reasoning` item on
        /// replay via the `call_id|item_id` encoding.
        item_id: String,
    },
}

struct StreamState {
    partial: AssistantMessage,
    started: bool,
    finished: bool,
    block_kind: HashMap<i64, BlockKind>,
    content_index: HashMap<i64, usize>,
    text_buf: HashMap<i64, String>,
    thinking_buf: HashMap<i64, String>,
    tool_json_buf: HashMap<i64, String>,
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
            started: false,
            finished: false,
            block_kind: HashMap::new(),
            content_index: HashMap::new(),
            text_buf: HashMap::new(),
            thinking_buf: HashMap::new(),
            tool_json_buf: HashMap::new(),
        }
    }

    fn ensure_started(&mut self, sender: &AssistantMessageEventStream) {
        if !self.started {
            self.started = true;
            sender.push(AssistantMessageEvent::Start {
                partial: self.partial.clone(),
            });
        }
    }

    /// Handle one decoded SSE JSON payload. Returns `true` if this was a
    /// terminal event and the caller should stop reading.
    fn handle_event(
        &mut self,
        value: &serde_json::Value,
        sender: &AssistantMessageEventStream,
    ) -> bool {
        self.ensure_started(sender);
        let event_type = value["type"].as_str().unwrap_or("");
        let output_index = value["output_index"].as_i64().unwrap_or(0);

        match event_type {
            "response.output_item.added" => {
                let item = &value["item"];
                let kind = match item["type"].as_str().unwrap_or("") {
                    "reasoning" => BlockKind::Thinking,
                    "function_call" => BlockKind::ToolCall {
                        call_id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                        item_id: item["id"].as_str().unwrap_or_default().to_string(),
                    },
                    _ => BlockKind::Text,
                };
                let index = self.partial.content.len();
                self.content_index.insert(output_index, index);
                let start_event = match &kind {
                    BlockKind::Text => AssistantMessageEvent::TextStart {
                        index,
                        partial: self.partial.clone(),
                    },
                    BlockKind::Thinking => AssistantMessageEvent::ThinkingStart {
                        index,
                        partial: self.partial.clone(),
                    },
                    BlockKind::ToolCall { .. } => AssistantMessageEvent::ToolCallStart {
                        index,
                        partial: self.partial.clone(),
                    },
                };
                if let BlockKind::ToolCall { .. } = &kind {
                    if let Some(args) = item["arguments"].as_str() {
                        self.tool_json_buf.insert(output_index, args.to_string());
                    }
                }
                self.block_kind.insert(output_index, kind);
                sender.push(start_event);
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = value["delta"].as_str() {
                    if let Some(&index) = self.content_index.get(&output_index) {
                        self.thinking_buf
                            .entry(output_index)
                            .or_default()
                            .push_str(delta);
                        sender.push(AssistantMessageEvent::ThinkingDelta {
                            index,
                            delta: delta.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = value["delta"].as_str() {
                    if let Some(&index) = self.content_index.get(&output_index) {
                        self.text_buf
                            .entry(output_index)
                            .or_default()
                            .push_str(delta);
                        sender.push(AssistantMessageEvent::TextDelta {
                            index,
                            delta: delta.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(delta) = value["delta"].as_str() {
                    if let Some(&index) = self.content_index.get(&output_index) {
                        self.tool_json_buf
                            .entry(output_index)
                            .or_default()
                            .push_str(delta);
                        sender.push(AssistantMessageEvent::ToolCallDelta {
                            index,
                            delta: delta.to_string(),
                            partial: self.partial.clone(),
                        });
                    }
                }
            }
            "response.output_item.done" => {
                self.finalize_block(output_index, &value["item"], sender);
            }
            "response.completed" => {
                let response = &value["response"];
                if let Some(usage) = response.get("usage") {
                    self.partial.usage = parse_usage(usage);
                }
                let mut stop_reason = map_response_status(response["status"].as_str());
                if self
                    .partial
                    .content
                    .iter()
                    .any(|c| matches!(c, Content::ToolCall(_)))
                    && stop_reason == StopReason::Stop
                {
                    stop_reason = StopReason::ToolUse;
                }
                self.partial.stop_reason = stop_reason;
            }
            "response.failed" => {
                let error = &value["response"]["error"];
                let message = if error.is_object() {
                    format!(
                        "{}: {}",
                        error["code"].as_str().unwrap_or("unknown"),
                        error["message"].as_str().unwrap_or("no message")
                    )
                } else {
                    "Unknown error (no error details in response)".to_string()
                };
                self.emit_error(message, sender);
                return true;
            }
            "error" => {
                let message = format!(
                    "Error Code {}: {}",
                    value["code"].as_str().unwrap_or("unknown"),
                    value["message"].as_str().unwrap_or("unknown error")
                );
                self.emit_error(message, sender);
                return true;
            }
            _ => {}
        }

        false
    }

    fn finalize_block(
        &mut self,
        output_index: i64,
        item: &serde_json::Value,
        sender: &AssistantMessageEventStream,
    ) {
        let Some(kind) = self.block_kind.remove(&output_index) else {
            return;
        };
        let Some(&index) = self.content_index.get(&output_index) else {
            return;
        };

        match kind {
            BlockKind::Thinking => {
                let summary_text = item["summary"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n")
                    })
                    .unwrap_or_default();
                let thinking = if !summary_text.is_empty() {
                    summary_text
                } else {
                    self.thinking_buf.remove(&output_index).unwrap_or_default()
                };
                // Preserve the full Responses `reasoning` item as the signature so
                // it can be replayed verbatim on the next turn (Azure pairs each
                // `rs_...` reasoning item with the `fc_...` function call that
                // follows it). Only meaningful items carry an id.
                let signature = if item.get("id").and_then(|v| v.as_str()).is_some() {
                    Some(item.to_string())
                } else {
                    None
                };
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
            BlockKind::Text => {
                let text = item["content"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|c| c["text"].as_str().or_else(|| c["refusal"].as_str()))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| self.text_buf.remove(&output_index).unwrap_or_default());
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
            BlockKind::ToolCall {
                call_id,
                name,
                item_id,
            } => {
                let raw = self.tool_json_buf.remove(&output_index).unwrap_or_default();
                let arguments = if raw.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    cortexcode_ai_util::parse_json_with_repair(&raw)
                        .unwrap_or_else(|_| serde_json::json!({}))
                };
                // Encode the wire `call_id` together with the Responses item id
                // as `call_id|item_id` so the pairing survives a replay round
                // trip. When the item id is absent, fall back to the bare
                // `call_id`.
                let id = if item_id.is_empty() {
                    call_id
                } else {
                    format!("{call_id}|{item_id}")
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
    /// including output items still open, with `stopReason` `aborted` when
    /// the signal fired and `error` otherwise.
    fn error_output(&self, message: String, aborted: bool) -> AssistantMessage {
        let mut output = self.partial.clone();
        let mut open: Vec<_> = self.block_kind.iter().collect();
        open.sort_by_key(|(output_index, _)| self.content_index.get(output_index).copied());
        for (output_index, kind) in open {
            output.content.push(match kind {
                BlockKind::Text => Content::Text(TextContent {
                    text_signature: None,
                    text: self.text_buf.get(output_index).cloned().unwrap_or_default(),
                    cache_control: None,
                }),
                BlockKind::Thinking => Content::Thinking(ThinkingContent {
                    redacted: false,
                    thinking: self
                        .thinking_buf
                        .get(output_index)
                        .cloned()
                        .unwrap_or_default(),
                    signature: None,
                }),
                BlockKind::ToolCall {
                    call_id,
                    name,
                    item_id,
                } => Content::ToolCall(ToolCallContent {
                    thought_signature: None,
                    id: if item_id.is_empty() {
                        call_id.clone()
                    } else {
                        format!("{call_id}|{item_id}")
                    },
                    name: name.clone(),
                    arguments: self
                        .tool_json_buf
                        .get(output_index)
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

    fn emit_error(&mut self, message: String, sender: &AssistantMessageEventStream) {
        self.partial.stop_reason = StopReason::Error;
        self.partial.error_message = Some(message);
        sender.push(AssistantMessageEvent::Error {
            error: self.partial.clone(),
        });
        sender.end(Some(self.partial.clone()));
    }

    fn finish(&mut self, sender: &AssistantMessageEventStream) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(sender);

        match &self.partial.stop_reason {
            StopReason::Error => {
                sender.push(AssistantMessageEvent::Error {
                    error: self.partial.clone(),
                });
            }
            _ => {
                sender.push(AssistantMessageEvent::Done {
                    message: self.partial.clone(),
                });
            }
        }
        sender.end(Some(self.partial.clone()));
    }
}

fn parse_usage(value: &serde_json::Value) -> Usage {
    let input_tokens = value["input_tokens"].as_u64().unwrap_or(0);
    let cached = value["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    let output = value["output_tokens"].as_u64().unwrap_or(0);
    let total = value["total_tokens"].as_u64().unwrap_or(0);

    Usage {
        input: input_tokens.saturating_sub(cached),
        output,
        cache_read: cached,
        cache_write: 0,
        total_tokens: total,
        cost: Cost::default(),
    }
}

fn map_response_status(status: Option<&str>) -> StopReason {
    match status {
        Some("completed") | Some("in_progress") | Some("queued") => StopReason::Stop,
        Some("incomplete") => StopReason::Length,
        Some("failed") | Some("cancelled") => StopReason::Error,
        _ => StopReason::Stop,
    }
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
    use cortexcode_ai_types::AbortSignal;
    use std::time::Duration;

    fn spawn_mock_server(sse_body: &'static str) -> String {
        serve_sse(sse_body)
    }

    #[allow(dead_code)]
    fn spawn_mock_error_server(status_line: &'static str, body: &'static str) -> String {
        serve_error(status_line, body)
    }

    fn test_model(base_url: String) -> Model {
        Model {
            compat: None,
            id: "gpt-5".into(),
            name: "GPT-5".into(),
            api: "azure-openai-responses".into(),
            provider: "azure-openai-responses".into(),
            base_url,
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
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
        std::env::remove_var("AZURE_OPENAI_API_KEY");
        let model = test_model("http://127.0.0.1:0".into());
        let context = Context::new("".into(), vec![], vec![]);
        assert!(stream(model, context, SimpleStreamOptions::default()).is_err());
    }

    #[test]
    fn test_stream_text_response() {
        let sse = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"Hello\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\", world\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello, world\"}]}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);

        assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.content.len(), 1);
                match &message.content[0] {
                    Content::Text(t) => assert_eq!(t.text, "Hello, world"),
                    other => panic!("expected text, got {other:?}"),
                }
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
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\"}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"\\\"a.rs\\\"}\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read_file\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert_eq!(message.stop_reason, StopReason::ToolUse);
                match &message.content[0] {
                    Content::ToolCall(tc) => {
                        assert_eq!(tc.name, "read_file");
                        assert_eq!(tc.id, "call_1");
                        assert_eq!(tc.arguments["path"], "a.rs");
                    }
                    other => panic!("expected tool call, got {other:?}"),
                }
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_pairs_reasoning_item_with_tool_call() {
        // A reasoning item followed by a function_call carrying an `fc_...`
        // item id: the reasoning item is preserved verbatim in the thinking
        // signature, and the tool call id is pipe-encoded as `call_id|item_id`.
        let sse = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\"}}\n\n",
            "data: {\"type\":\"response.reasoning_text.delta\",\"output_index\":0,\"delta\":\"pondering\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"summary\":[]}}\n\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"id\":\"fc_1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{}\"}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"id\":\"fc_1\",\"name\":\"read_file\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                match &message.content[0] {
                    Content::Thinking(t) => {
                        let sig = t.signature.as_ref().expect("reasoning signature");
                        let item: serde_json::Value = serde_json::from_str(sig).unwrap();
                        assert_eq!(item["type"], "reasoning");
                        assert_eq!(item["id"], "rs_1");
                    }
                    other => panic!("expected thinking, got {other:?}"),
                }
                match &message.content[1] {
                    Content::ToolCall(tc) => {
                        // call_id|item_id pairing survives into ToolCallContent::id.
                        assert_eq!(tc.id, "call_1|fc_1");
                    }
                    other => panic!("expected tool call, got {other:?}"),
                }
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// `testAbortSignal` in `abort.test.ts`, against a server that stalls
    /// mid-message.
    #[test]
    fn test_abort_mid_stream_keeps_partial_content() {
        let head = concat!(
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"15 + 27 = 42. \"}\n\n",
        );
        let base_url = serve_sse_then_hang(head, Duration::from_secs(30));
        let signal = AbortSignal::new();
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
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
        while let Some(event) = s.next_blocking() {
            if let AssistantMessageEvent::TextDelta { .. } = &event {
                signal.abort();
            }
        }
        assert!(started.elapsed() < Duration::from_secs(10));

        let msg = s.result_blocking();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
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
            api_key: Some("azkey".into()),
            signal: Some(signal),
            ..Default::default()
        };
        let s = stream(
            test_model("http://127.0.0.1:9".into()),
            Context::new("".into(), vec![], vec![]),
            options,
        )
        .unwrap();
        assert_eq!(s.result_blocking().stop_reason, StopReason::Aborted);
    }

    #[test]
    fn test_stream_failed_response() {
        let sse = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limited\",\"message\":\"too many requests\"}}}\n\n";
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("azkey".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error } => {
                assert!(error
                    .error_message
                    .as_ref()
                    .unwrap()
                    .contains("rate_limited"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_map_response_status() {
        assert_eq!(map_response_status(Some("completed")), StopReason::Stop);
        assert_eq!(map_response_status(Some("incomplete")), StopReason::Length);
        assert_eq!(map_response_status(Some("failed")), StopReason::Error);
    }
}

//! Azure OpenAI provider for cortex AI.
//!
//! Implements streaming against Azure's OpenAI Responses API
//! (`POST {base_url}/responses?api-version={version}`, `stream: true`),
//! translating the SSE event stream into [`AssistantMessageEvent`]s.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/azure-openai-responses.ts` and
//! `providers/openai-responses-shared.ts`. See `request.rs` for the
//! simplifications made relative to the TS source (no cross-provider
//! reasoning-item ID pairing).

mod request;
mod sse;

use std::collections::HashMap;
use std::io::BufReader;

use cortexcode_ai_stream::{AiMessageEventSender, AiMessageEventStream};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, Context, Cost,
    Model, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent, Usage,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Stream a completion from the Azure OpenAI Responses API.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>, BoxError> {
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

    let (sender, recv_stream) = AiMessageEventStream::new();
    std::thread::spawn(move || {
        run_stream(url, headers, body, sender);
    });
    Ok(Box::new(recv_stream))
}

fn run_stream(
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
    sender: AiMessageEventSender,
) {
    let client = match reqwest::blocking::Client::builder().build() {
        Ok(c) => c,
        Err(e) => {
            fail(&sender, format!("failed to build HTTP client: {e}"));
            return;
        }
    };

    let mut request = client.post(&url);
    for (k, v) in &headers {
        request = request.header(k, v);
    }

    let response = match request.json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            fail(&sender, format!("request to Azure OpenAI API failed: {e}"));
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        fail(
            &sender,
            format!("Azure OpenAI API returned {status}: {text}"),
        );
        return;
    }

    let reader = BufReader::new(response);
    let events = sse::SseEvents::new(reader);

    let mut state = StreamState::new();
    for frame in events {
        let payload = match frame {
            Ok(p) => p,
            Err(e) => {
                fail(&sender, format!("error reading response stream: {e}"));
                return;
            }
        };
        if payload.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if state.handle_event(&value, &sender) {
            return;
        }
    }

    state.finish(&sender);
}

fn fail(sender: &AiMessageEventSender, message: String) {
    let error = AssistantMessage {
        content: vec![],
        stop_reason: Some(StopReason::Error),
        stop_sequence: None,
        usage: None,
        timestamp: None,
        error_message: Some(message),
    };
    sender.push(AssistantMessageEvent::Error {
        error: error.clone(),
    });
    sender.end(error);
}

// ---------------------------------------------------------------------------
// Streaming state machine
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum BlockKind {
    Text,
    Thinking,
    ToolCall { call_id: String, name: String },
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
    fn new() -> Self {
        Self {
            partial: AssistantMessage {
                content: vec![],
                stop_reason: None,
                stop_sequence: None,
                usage: None,
                timestamp: Some(now_millis()),
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

    fn ensure_started(&mut self, sender: &AiMessageEventSender) {
        if !self.started {
            self.started = true;
            sender.push(AssistantMessageEvent::Start {
                partial: self.partial.clone(),
            });
        }
    }

    /// Handle one decoded SSE JSON payload. Returns `true` if this was a
    /// terminal event and the caller should stop reading.
    fn handle_event(&mut self, value: &serde_json::Value, sender: &AiMessageEventSender) -> bool {
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
                    self.partial.usage = Some(parse_usage(usage));
                }
                let mut stop_reason = map_response_status(response["status"].as_str());
                if self
                    .partial
                    .content
                    .iter()
                    .any(|c| matches!(c, Content::ToolCall(_)))
                    && stop_reason == StopReason::EndTurn
                {
                    stop_reason = StopReason::ToolUse;
                }
                self.partial.stop_reason = Some(stop_reason);
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
        sender: &AiMessageEventSender,
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
                self.partial
                    .content
                    .push(Content::Thinking(ThinkingContent {
                        thinking,
                        signature: None,
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
                    text,
                    cache_control: None,
                }));
                sender.push(AssistantMessageEvent::TextEnd {
                    index,
                    partial: self.partial.clone(),
                });
            }
            BlockKind::ToolCall { call_id, name } => {
                let raw = self.tool_json_buf.remove(&output_index).unwrap_or_default();
                let arguments = if raw.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    cortexcode_ai_util::parse_json_with_repair(&raw)
                        .unwrap_or_else(|_| serde_json::json!({}))
                };
                self.partial
                    .content
                    .push(Content::ToolCall(ToolCallContent {
                        id: call_id,
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

    fn emit_error(&mut self, message: String, sender: &AiMessageEventSender) {
        self.partial.stop_reason = Some(StopReason::Error);
        self.partial.error_message = Some(message);
        sender.push(AssistantMessageEvent::Error {
            error: self.partial.clone(),
        });
        sender.end(self.partial.clone());
    }

    fn finish(&mut self, sender: &AiMessageEventSender) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(sender);

        if self.partial.stop_reason.is_none() {
            self.partial.stop_reason = Some(StopReason::EndTurn);
        }

        match &self.partial.stop_reason {
            Some(StopReason::Error) => {
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
        sender.end(self.partial.clone());
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
        Some("completed") | Some("in_progress") | Some("queued") => StopReason::EndTurn,
        Some("incomplete") => StopReason::MaxTokens,
        Some("failed") | Some("cancelled") => StopReason::Error,
        _ => StopReason::EndTurn,
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
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn spawn_mock_server(sse_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    sse_body.len(),
                    sse_body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn test_model(base_url: String) -> Model {
        Model {
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

    fn collect(mut s: Box<dyn AssistantMessageEventStream>) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        while let Some(e) = s.next_event() {
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
                assert_eq!(message.stop_reason, Some(StopReason::EndTurn));
                assert_eq!(message.content.len(), 1);
                match &message.content[0] {
                    Content::Text(t) => assert_eq!(t.text, "Hello, world"),
                    other => panic!("expected text, got {other:?}"),
                }
                let usage = message.usage.as_ref().unwrap();
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
                assert_eq!(message.stop_reason, Some(StopReason::ToolUse));
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
        assert_eq!(map_response_status(Some("completed")), StopReason::EndTurn);
        assert_eq!(
            map_response_status(Some("incomplete")),
            StopReason::MaxTokens
        );
        assert_eq!(map_response_status(Some("failed")), StopReason::Error);
    }
}

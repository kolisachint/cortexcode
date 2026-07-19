//! OpenAI provider for cortex AI.
//!
//! Implements streaming against the OpenAI Chat Completions API
//! (`POST {base_url}/chat/completions`, `stream: true`), translating the
//! SSE event stream into [`AssistantMessageEvent`]s.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/openai-completions.ts`. Provider-specific `compat` quirk
//! overrides (zai/together/moonshot/openrouter/deepseek-specific request
//! shaping) are not yet ported — see the migration design doc's stated
//! non-goal of full parity in the initial pass.

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

/// Stream a completion from the OpenAI Chat Completions API.
///
/// Matches the `stream_fn` shape expected by `AgentLoopConfig`. Setup
/// failures (missing credentials) return `Err`; everything else (network
/// errors, HTTP error responses) is reported as an `Error` event on the
/// returned stream.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>, BoxError> {
    let api_key = request::resolve_credentials(&options).map_err(BoxError::from)?;
    let headers = request::build_headers(&model, &api_key);
    let body = request::build_request_body(&model, &context, &options);
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));

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
            fail(&sender, format!("request to OpenAI API failed: {e}"));
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        fail(&sender, format!("OpenAI API returned {status}: {text}"));
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
        let payload = payload.trim();
        if payload.is_empty() {
            continue;
        }
        if payload == "[DONE]" {
            state.finish(&sender);
            return;
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        state.handle_chunk(&value, &sender);
    }

    // Connection closed without an explicit [DONE] sentinel.
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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum BlockRef {
    Text,
    Thinking,
    ToolCall(i64),
}

struct ToolCallBuf {
    id: String,
    name: String,
    args: String,
}

struct StreamState {
    partial: AssistantMessage,
    order: Vec<BlockRef>,
    content_index: HashMap<BlockRef, usize>,
    text_buf: String,
    thinking_buf: String,
    tool_calls: HashMap<i64, ToolCallBuf>,
    started: bool,
    finished: bool,
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
            order: Vec::new(),
            content_index: HashMap::new(),
            text_buf: String::new(),
            thinking_buf: String::new(),
            tool_calls: HashMap::new(),
            started: false,
            finished: false,
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

    fn ensure_block(&mut self, block: BlockRef, sender: &AiMessageEventSender) -> usize {
        if let Some(&idx) = self.content_index.get(&block) {
            return idx;
        }
        let idx = self.order.len();
        self.order.push(block);
        self.content_index.insert(block, idx);
        let start_event = match block {
            BlockRef::Text => AssistantMessageEvent::TextStart {
                index: idx,
                partial: self.partial.clone(),
            },
            BlockRef::Thinking => AssistantMessageEvent::ThinkingStart {
                index: idx,
                partial: self.partial.clone(),
            },
            BlockRef::ToolCall(_) => AssistantMessageEvent::ToolCallStart {
                index: idx,
                partial: self.partial.clone(),
            },
        };
        sender.push(start_event);
        idx
    }

    fn handle_chunk(&mut self, value: &serde_json::Value, sender: &AiMessageEventSender) {
        self.ensure_started(sender);

        if let Some(usage) = value.get("usage") {
            if !usage.is_null() {
                self.partial.usage = Some(parse_usage(usage));
            }
        }

        let Some(choice) = value["choices"].get(0) else {
            return;
        };

        if let Some(reason) = choice["finish_reason"].as_str() {
            let (stop_reason, error_message) = map_finish_reason(reason);
            self.partial.stop_reason = Some(stop_reason);
            if let Some(msg) = error_message {
                self.partial.error_message = Some(msg);
            }
        }

        let delta = &choice["delta"];
        if delta.is_null() {
            return;
        }

        if let Some(text) = delta["content"].as_str() {
            if !text.is_empty() {
                let idx = self.ensure_block(BlockRef::Text, sender);
                self.text_buf.push_str(text);
                sender.push(AssistantMessageEvent::TextDelta {
                    index: idx,
                    delta: text.to_string(),
                    partial: self.partial.clone(),
                });
            }
        }

        for field in ["reasoning_content", "reasoning", "reasoning_text"] {
            if let Some(text) = delta[field].as_str() {
                if !text.is_empty() {
                    let idx = self.ensure_block(BlockRef::Thinking, sender);
                    self.thinking_buf.push_str(text);
                    sender.push(AssistantMessageEvent::ThinkingDelta {
                        index: idx,
                        delta: text.to_string(),
                        partial: self.partial.clone(),
                    });
                    break;
                }
            }
        }

        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                let stream_index = tc["index"].as_i64().unwrap_or(0);
                let block = BlockRef::ToolCall(stream_index);
                let idx = self.ensure_block(block, sender);

                let buf = self.tool_calls.entry(stream_index).or_insert(ToolCallBuf {
                    id: String::new(),
                    name: String::new(),
                    args: String::new(),
                });
                if let Some(id) = tc["id"].as_str() {
                    if !id.is_empty() {
                        buf.id = id.to_string();
                    }
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    if !name.is_empty() {
                        buf.name = name.to_string();
                    }
                }
                let mut arg_delta = "";
                if let Some(args) = tc["function"]["arguments"].as_str() {
                    arg_delta = args;
                    buf.args.push_str(args);
                }
                sender.push(AssistantMessageEvent::ToolCallDelta {
                    index: idx,
                    delta: arg_delta.to_string(),
                    partial: self.partial.clone(),
                });
            }
        }
    }

    fn finish(&mut self, sender: &AiMessageEventSender) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(sender);

        for block in self.order.clone() {
            let idx = self.content_index[&block];
            match block {
                BlockRef::Text => {
                    self.partial.content.push(Content::Text(TextContent {
                        text: std::mem::take(&mut self.text_buf),
                        cache_control: None,
                    }));
                    sender.push(AssistantMessageEvent::TextEnd {
                        index: idx,
                        partial: self.partial.clone(),
                    });
                }
                BlockRef::Thinking => {
                    self.partial
                        .content
                        .push(Content::Thinking(ThinkingContent {
                            thinking: std::mem::take(&mut self.thinking_buf),
                            signature: None,
                        }));
                    sender.push(AssistantMessageEvent::ThinkingEnd {
                        index: idx,
                        partial: self.partial.clone(),
                    });
                }
                BlockRef::ToolCall(stream_index) => {
                    if let Some(buf) = self.tool_calls.remove(&stream_index) {
                        let arguments = if buf.args.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            cortexcode_ai_util::parse_json_with_repair(&buf.args)
                                .unwrap_or_else(|_| serde_json::json!({}))
                        };
                        self.partial
                            .content
                            .push(Content::ToolCall(ToolCallContent {
                                id: buf.id,
                                name: buf.name,
                                arguments,
                            }));
                    }
                    sender.push(AssistantMessageEvent::ToolCallEnd {
                        index: idx,
                        partial: self.partial.clone(),
                    });
                }
            }
        }

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
    let prompt_tokens = value["prompt_tokens"].as_u64().unwrap_or(0);
    let completion_tokens = value["completion_tokens"].as_u64().unwrap_or(0);
    let reported_cached = value["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| value["prompt_cache_hit_tokens"].as_u64())
        .unwrap_or(0);
    let cache_write = value["prompt_tokens_details"]["cache_write_tokens"]
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

    Usage {
        input,
        output: completion_tokens,
        cache_read,
        cache_write,
        total_tokens: input + completion_tokens + cache_read + cache_write,
        cost: Cost::default(),
    }
}

fn map_finish_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::EndTurn, None),
        "length" => (StopReason::MaxTokens, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        "content_filter" => (
            StopReason::Error,
            Some("Provider finish_reason: content_filter".to_string()),
        ),
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

    fn spawn_mock_error_server(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "{}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    status_line,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn test_model(base_url: String) -> Model {
        Model {
            id: "gpt-test".into(),
            name: "GPT Test".into(),
            api: "openai-completions".into(),
            provider: "openai".into(),
            base_url,
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 128_000,
            max_tokens: 4096,
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
        std::env::remove_var("OPENAI_API_KEY");
        let model = test_model("http://127.0.0.1:0".into());
        let context = Context::new("".into(), vec![], vec![]);
        assert!(stream(model, context, SimpleStreamOptions::default()).is_err());
    }

    #[test]
    fn test_stream_text_response() {
        let sse = concat!(
            "data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\", world\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
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
                assert_eq!(message.stop_reason, Some(StopReason::EndTurn));
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
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.rs\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":8}}\n\n",
            "data: [DONE]\n\n",
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
                assert_eq!(message.stop_reason, Some(StopReason::ToolUse));
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
            "{\"error\":{\"message\":\"rate limited\"}}",
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
                assert_eq!(error.stop_reason, Some(StopReason::Error));
                assert!(error.error_message.as_ref().unwrap().contains("429"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_map_finish_reason() {
        assert_eq!(map_finish_reason("stop").0, StopReason::EndTurn);
        assert_eq!(map_finish_reason("tool_calls").0, StopReason::ToolUse);
        assert_eq!(map_finish_reason("length").0, StopReason::MaxTokens);
        assert_eq!(map_finish_reason("content_filter").0, StopReason::Error);
    }
}

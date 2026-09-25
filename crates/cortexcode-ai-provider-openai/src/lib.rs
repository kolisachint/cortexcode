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
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = request::resolve_credentials(&options).map_err(BoxError::from)?;
    let headers = request::build_headers(&model, &api_key);
    let body = request::build_request_body(&model, &context, &options);
    let url = format!("{}/chat/completions", model.base_url.trim_end_matches('/'));

    let stream = create_assistant_message_event_stream();
    let sender = stream.clone();
    let template = AssistantMessage::for_model(&model);
    spawn_producer(
        &stream,
        run_stream(url, headers, body, sender, template, options.signal),
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

/// Send the request and feed the SSE chunks to `state`. `Err` carries the
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
        .map_err(|e| format!("request to OpenAI API failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(api_error_message(status, &text));
    }

    let mut events = cortexcode_ai_sse::sse_events(response.bytes_stream());
    while let Some(frame) = events.next().await {
        let frame = frame.map_err(|e| format!("error reading response stream: {e}"))?;
        let payload = frame.data.trim();
        if payload.is_empty() {
            continue;
        }
        if payload == "[DONE]" {
            state.finish(sender);
            return Ok(());
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        state.handle_chunk(&value, sender);
    }

    // Connection closed without an explicit [DONE] sentinel.
    state.finish(sender);
    Ok(())
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
            order: Vec::new(),
            content_index: HashMap::new(),
            text_buf: String::new(),
            thinking_buf: String::new(),
            tool_calls: HashMap::new(),
            started: false,
            finished: false,
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

    fn ensure_block(&mut self, block: BlockRef, sender: &AssistantMessageEventStream) -> usize {
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

    fn handle_chunk(&mut self, value: &serde_json::Value, sender: &AssistantMessageEventStream) {
        self.ensure_started(sender);

        if let Some(usage) = value.get("usage") {
            if !usage.is_null() {
                self.partial.usage = parse_usage(usage);
            }
        }

        let Some(choice) = value["choices"].get(0) else {
            return;
        };

        if let Some(reason) = choice["finish_reason"].as_str() {
            let (stop_reason, error_message) = map_finish_reason(reason);
            self.partial.stop_reason = stop_reason;
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

    /// The message for a terminal `error` event: everything streamed so far
    /// (blocks are only materialized at `finish`, so all open blocks are
    /// added here in stream order), with `stopReason` `aborted` when the
    /// signal fired and `error` otherwise.
    fn error_output(&self, message: String, aborted: bool) -> AssistantMessage {
        let mut output = self.partial.clone();
        for block in &self.order {
            output.content.push(match block {
                BlockRef::Text => Content::Text(TextContent {
                    text_signature: None,
                    text: self.text_buf.clone(),
                    cache_control: None,
                }),
                BlockRef::Thinking => Content::Thinking(ThinkingContent {
                    redacted: false,
                    thinking: self.thinking_buf.clone(),
                    signature: None,
                }),
                BlockRef::ToolCall(stream_index) => {
                    let Some(buf) = self.tool_calls.get(stream_index) else {
                        continue;
                    };
                    Content::ToolCall(ToolCallContent {
                        thought_signature: None,
                        id: buf.id.clone(),
                        name: buf.name.clone(),
                        arguments: Some(&buf.args)
                            .filter(|raw| !raw.trim().is_empty())
                            .and_then(|raw| cortexcode_ai_util::parse_json_with_repair(raw).ok())
                            .unwrap_or_else(|| serde_json::json!({})),
                    })
                }
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
                        text_signature: None,
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
                            redacted: false,
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
                                thought_signature: None,
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
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
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

/// The message of the `openai` SDK's `APIError` for a non-2xx response
/// (`APIError.makeMessage` with the body parsed by `safeJSON`), which hoocode
/// reports as the assistant `errorMessage`. The retry-after suffix of
/// `describeProviderError` arrives with the retry utilities (8.5).
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
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .filter(|e| truthy(e));
    let msg = match error {
        Some(e) => match e.get("message").filter(|m| truthy(m)) {
            Some(serde_json::Value::String(m)) => m.clone(),
            Some(m) => m.to_string(),
            None => e.to_string(),
        },
        None if parsed.is_none() => body.to_string(),
        None => String::new(),
    };
    if msg.is_empty() {
        format!("{status} status code (no body)")
    } else {
        format!("{status} {msg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_message_matches_the_openai_sdk() {
        assert_eq!(
            api_error_message(400, r#"{"error": {"message": "bad input"}}"#),
            "400 bad input"
        );
        assert_eq!(
            api_error_message(500, r#"{"error": {"code": 1}}"#),
            r#"500 {"code":1}"#
        );
        assert_eq!(api_error_message(502, "upstream down"), "502 upstream down");
        assert_eq!(
            api_error_message(503, r#"{"detail": "x"}"#),
            "503 status code (no body)"
        );
        assert_eq!(api_error_message(429, ""), "429 status code (no body)");
    }
    use cortexcode_ai_stream::testing::{serve_error, serve_sse, serve_sse_then_hang};
    use cortexcode_ai_types::AbortSignal;
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

    fn collect(mut s: AssistantMessageEventStream) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        while let Some(e) = s.next_blocking() {
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
                assert_eq!(error.stop_reason, StopReason::Error);
                assert!(error.error_message.as_ref().unwrap().contains("429"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    /// `testAbortSignal` in `abort.test.ts`, against a server that stalls
    /// mid-message.
    #[test]
    fn test_abort_mid_stream_keeps_partial_content() {
        let head = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"15 + 27 = 42. \"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Names: Ann\"},\"finish_reason\":null}]}\n\n",
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
                if text.len() >= 20 {
                    signal.abort();
                }
            }
        }
        assert!(started.elapsed() < Duration::from_secs(10));

        let msg = s.result_blocking();
        assert_eq!(msg.stop_reason, StopReason::Aborted);
        match msg.content.as_slice() {
            [Content::Text(t)] => assert_eq!(t.text, "15 + 27 = 42. Names: Ann"),
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
        assert_eq!(s.result_blocking().stop_reason, StopReason::Aborted);
    }

    #[test]
    fn test_map_finish_reason() {
        assert_eq!(map_finish_reason("stop").0, StopReason::Stop);
        assert_eq!(map_finish_reason("tool_calls").0, StopReason::ToolUse);
        assert_eq!(map_finish_reason("length").0, StopReason::Length);
        assert_eq!(map_finish_reason("content_filter").0, StopReason::Error);
    }
}

//! Google Gemini / Vertex AI provider for cortex AI.
//!
//! Implements streaming against the `:streamGenerateContent?alt=sse` REST
//! endpoint shared by Google Generative AI and Vertex AI, translating the
//! SSE event stream into [`AssistantMessageEvent`]s.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` →
//! `providers/google.ts`, `providers/google-vertex.ts`,
//! `providers/google-shared.ts`.
//!
//! Vertex AI credential support in this pass is limited to an already-minted
//! OAuth2 access token (`GOOGLE_VERTEX_ACCESS_TOKEN` or an explicit
//! `api_key`). Full Application Default Credentials — service-account JSON
//! key parsing, RS256 JWT signing, and token exchange — is not yet ported.

mod request;
mod shared;
mod sse;

use std::io::BufReader;

use cortexcode_ai_stream::{AiMessageEventSender, AiMessageEventStream};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, Context, Cost,
    Model, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent, Usage,
};

pub use request::{resolve_gemini_credentials, resolve_vertex_credentials, VertexCredentials};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Stream a completion from the Google Generative AI API (Gemini).
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>, BoxError> {
    let api_key = request::resolve_gemini_credentials(&options).map_err(BoxError::from)?;
    let body = request::build_request_body(&model, &context, &options);
    let url = format!(
        "{}/models/{}:streamGenerateContent?alt=sse&key={}",
        model.base_url.trim_end_matches('/'),
        model.id,
        api_key
    );
    let headers: Vec<(String, String)> = model
        .headers
        .clone()
        .map(|h| h.into_iter().collect())
        .unwrap_or_default();

    spawn_stream(url, headers, body)
}

/// Stream a completion from Vertex AI.
pub fn stream_vertex(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>, BoxError> {
    let creds = request::resolve_vertex_credentials(&options).map_err(BoxError::from)?;
    let body = request::build_request_body(&model, &context, &options);
    let base_url = model.base_url.replace("{location}", &creds.location);
    let url = format!(
        "{}/v1/projects/{}/locations/{}/publishers/google/models/{}:streamGenerateContent?alt=sse",
        base_url.trim_end_matches('/'),
        creds.project,
        creds.location,
        model.id,
    );
    let mut headers = vec![(
        "authorization".to_string(),
        format!("Bearer {}", creds.access_token),
    )];
    if let Some(extra) = &model.headers {
        for (k, v) in extra {
            headers.push((k.clone(), v.clone()));
        }
    }

    spawn_stream(url, headers, body)
}

fn spawn_stream(
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
) -> Result<Box<dyn AssistantMessageEventStream>, BoxError> {
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

    let mut request = client.post(&url).header("content-type", "application/json");
    for (k, v) in &headers {
        request = request.header(k, v);
    }

    let response = match request.json(&body).send() {
        Ok(r) => r,
        Err(e) => {
            fail(&sender, format!("request to Google API failed: {e}"));
            return;
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().unwrap_or_default();
        fail(&sender, format!("Google API returned {status}: {text}"));
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
        state.handle_chunk(&value, &sender);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentKind {
    Text,
    Thinking,
}

struct StreamState {
    partial: AssistantMessage,
    started: bool,
    finished: bool,
    current: Option<(CurrentKind, usize, String)>,
    tool_call_counter: u64,
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
            current: None,
            tool_call_counter: 0,
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

    fn close_current(&mut self, sender: &AiMessageEventSender) {
        if let Some((kind, index, text)) = self.current.take() {
            match kind {
                CurrentKind::Text => {
                    self.partial.content.push(Content::Text(TextContent {
                        text,
                        cache_control: None,
                    }));
                    sender.push(AssistantMessageEvent::TextEnd {
                        index,
                        partial: self.partial.clone(),
                    });
                }
                CurrentKind::Thinking => {
                    self.partial
                        .content
                        .push(Content::Thinking(ThinkingContent {
                            thinking: text,
                            signature: None,
                        }));
                    sender.push(AssistantMessageEvent::ThinkingEnd {
                        index,
                        partial: self.partial.clone(),
                    });
                }
            }
        }
    }

    fn handle_chunk(&mut self, value: &serde_json::Value, sender: &AiMessageEventSender) {
        self.ensure_started(sender);

        let Some(candidate) = value["candidates"].get(0) else {
            if let Some(usage) = value.get("usageMetadata") {
                self.partial.usage = Some(parse_usage(usage));
            }
            return;
        };

        if let Some(parts) = candidate["content"]["parts"].as_array() {
            for part in parts {
                if let Some(text) = part["text"].as_str() {
                    let is_thinking = part["thought"].as_bool().unwrap_or(false);
                    let want_kind = if is_thinking {
                        CurrentKind::Thinking
                    } else {
                        CurrentKind::Text
                    };
                    let needs_new_block = match &self.current {
                        Some((kind, _, _)) => *kind != want_kind,
                        None => true,
                    };
                    if needs_new_block {
                        self.close_current(sender);
                        // `close_current` finalizes any in-flight block into
                        // `partial.content`, so its length is the next index.
                        let index = self.partial.content.len();
                        let start_event = match want_kind {
                            CurrentKind::Text => AssistantMessageEvent::TextStart {
                                index,
                                partial: self.partial.clone(),
                            },
                            CurrentKind::Thinking => AssistantMessageEvent::ThinkingStart {
                                index,
                                partial: self.partial.clone(),
                            },
                        };
                        sender.push(start_event);
                        self.current = Some((want_kind, index, String::new()));
                    }
                    if let Some((kind, index, buf)) = &mut self.current {
                        buf.push_str(text);
                        let index = *index;
                        let event = match kind {
                            CurrentKind::Text => AssistantMessageEvent::TextDelta {
                                index,
                                delta: text.to_string(),
                                partial: self.partial.clone(),
                            },
                            CurrentKind::Thinking => AssistantMessageEvent::ThinkingDelta {
                                index,
                                delta: text.to_string(),
                                partial: self.partial.clone(),
                            },
                        };
                        sender.push(event);
                    }
                }

                if let Some(fc) = part.get("functionCall") {
                    self.close_current(sender);

                    let name = fc["name"].as_str().unwrap_or_default().to_string();
                    let provided_id = fc["id"].as_str().map(str::to_string);
                    let is_duplicate = provided_id.as_ref().is_some_and(|id| {
                        self.partial
                            .content
                            .iter()
                            .any(|c| matches!(c, Content::ToolCall(tc) if &tc.id == id))
                    });
                    let id = match provided_id {
                        Some(id) if !is_duplicate => id,
                        _ => {
                            self.tool_call_counter += 1;
                            format!("{name}_{}_{}", now_millis(), self.tool_call_counter)
                        }
                    };
                    let arguments = fc
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));

                    let index = self.partial.content.len();
                    self.partial
                        .content
                        .push(Content::ToolCall(ToolCallContent {
                            id,
                            name,
                            arguments: arguments.clone(),
                        }));
                    sender.push(AssistantMessageEvent::ToolCallStart {
                        index,
                        partial: self.partial.clone(),
                    });
                    sender.push(AssistantMessageEvent::ToolCallDelta {
                        index,
                        delta: arguments.to_string(),
                        partial: self.partial.clone(),
                    });
                    sender.push(AssistantMessageEvent::ToolCallEnd {
                        index,
                        partial: self.partial.clone(),
                    });
                }
            }
        }

        if let Some(reason) = candidate["finishReason"].as_str() {
            let mut stop_reason = shared::map_stop_reason(reason);
            if self
                .partial
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)))
            {
                stop_reason = StopReason::ToolUse;
            }
            self.partial.stop_reason = Some(stop_reason);
        }

        if let Some(usage) = value.get("usageMetadata") {
            self.partial.usage = Some(parse_usage(usage));
        }
    }

    fn finish(&mut self, sender: &AiMessageEventSender) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(sender);
        self.close_current(sender);

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
    let prompt = value["promptTokenCount"].as_u64().unwrap_or(0);
    let cached = value["cachedContentTokenCount"].as_u64().unwrap_or(0);
    let candidates = value["candidatesTokenCount"].as_u64().unwrap_or(0);
    let thoughts = value["thoughtsTokenCount"].as_u64().unwrap_or(0);
    let total = value["totalTokenCount"].as_u64().unwrap_or(0);

    Usage {
        input: prompt.saturating_sub(cached),
        output: candidates + thoughts,
        cache_read: cached,
        cache_write: 0,
        total_tokens: total,
        cost: Cost::default(),
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
            id: "gemini-2.0-flash".into(),
            name: "Gemini Test".into(),
            api: "google-generative-ai".into(),
            provider: "google".into(),
            base_url,
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into()],
            cost: cortexcode_ai_types::ModelCost::default(),
            context_window: 1_000_000,
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
        std::env::remove_var("GEMINI_API_KEY");
        let model = test_model("http://127.0.0.1:0".into());
        let context = Context::new("".into(), vec![], vec![]);
        assert!(stream(model, context, SimpleStreamOptions::default()).is_err());
    }

    #[test]
    fn test_stream_text_response() {
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}],\"role\":\"model\"}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\", world\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5,\"totalTokenCount\":15}}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("key123".into()),
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
    fn test_stream_thinking_then_text() {
        let sse = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"let me think\",\"thought\":true}],\"role\":\"model\"}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}]}\n\n",
        );
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("key123".into()),
            ..Default::default()
        };

        let s = stream(model, context, options).expect("stream should start");
        let events = collect(s);
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert_eq!(message.content.len(), 2);
                match &message.content[0] {
                    Content::Thinking(t) => assert_eq!(t.thinking, "let me think"),
                    other => panic!("expected thinking first, got {other:?}"),
                }
                match &message.content[1] {
                    Content::Text(t) => assert_eq!(t.text, "answer"),
                    other => panic!("expected text second, got {other:?}"),
                }
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_tool_call_response() {
        let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a.rs\"}}}]},\"finishReason\":\"STOP\"}]}\n\n";
        let base_url = spawn_mock_server(sse);
        let model = test_model(base_url);
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions {
            api_key: Some("key123".into()),
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
                        assert_eq!(tc.arguments["path"], "a.rs");
                    }
                    other => panic!("expected tool call, got {other:?}"),
                }
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_vertex_missing_credentials() {
        std::env::remove_var("GOOGLE_VERTEX_ACCESS_TOKEN");
        let model = test_model("http://127.0.0.1:0".into());
        let context = Context::new("".into(), vec![], vec![]);
        assert!(stream_vertex(model, context, SimpleStreamOptions::default()).is_err());
    }
}

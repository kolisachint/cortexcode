//! Faux / test provider for cortex AI.
//!
//! Provides a mock LLM provider for testing and development.
//! Pre-configure responses and the provider streams them back
//! as if they came from a real model.
//!
//! Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/faux.ts`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cortexcode_ai_stream::{AiMessageEventSender, AiMessageEventStream};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, Content, Context, Cost,
    Model, SimpleStreamOptions, StopReason, TextContent, ThinkingContent, ToolCallContent, Usage,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_MIN_TOKEN_SIZE: usize = 3;
const DEFAULT_MAX_TOKEN_SIZE: usize = 5;

const DEFAULT_USAGE: Usage = Usage {
    input: 0,
    output: 0,
    cache_read: 0,
    cache_write: 0,
    total_tokens: 0,
    cost: Cost {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
        total: 0.0,
    },
};

// ---------------------------------------------------------------------------
// FauxResponseStep
// ---------------------------------------------------------------------------

/// A factory function that produces an `AssistantMessage` for a given context.
pub type FauxResponseFactory =
    Box<dyn Fn(&Context, &SimpleStreamOptions, &Model) -> AssistantMessage + Send + Sync>;

/// A single response step: either a pre-built message or a factory.
pub enum FauxResponseStep {
    /// A pre-built assistant message.
    Message(AssistantMessage),
    /// A factory function that generates a message based on context/model.
    Factory(FauxResponseFactory),
}

// ---------------------------------------------------------------------------
// FauxProvider
// ---------------------------------------------------------------------------

/// A mock LLM provider for testing.
///
/// ## Usage
///
/// ```ignore
/// let provider = Arc::new(FauxProvider::new());
/// provider.set_responses(vec![
///     FauxResponseStep::Message(faux_text_message("Hello, world!", None)),
/// ]);
/// let stream_fn = provider.stream_fn();
/// // pass `stream_fn` to an Agent or call it directly
/// ```
pub struct FauxProvider {
    responses: Mutex<Vec<FauxResponseStep>>,
    call_count: AtomicUsize,
    min_token_size: usize,
    max_token_size: usize,
}

impl Default for FauxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FauxProvider {
    /// Create a new `FauxProvider` with no pre-configured responses.
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
            min_token_size: DEFAULT_MIN_TOKEN_SIZE,
            max_token_size: DEFAULT_MAX_TOKEN_SIZE,
        }
    }

    /// Set the response queue (replaces any existing responses).
    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        *self.responses.lock().unwrap() = responses;
    }

    /// Append responses to the queue.
    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.responses.lock().unwrap().extend(responses);
    }

    /// Get the number of pending responses.
    pub fn pending_count(&self) -> usize {
        self.responses.lock().unwrap().len()
    }

    /// Get the total number of calls made through this provider.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }

    /// Reset the call counter.
    pub fn reset_call_count(&self) {
        self.call_count.store(0, Ordering::Relaxed);
    }

    /// Return a closure compatible with `AgentLoopConfig.stream_fn`.
    ///
    /// Each invocation pops the next response from the queue and streams it
    /// through an [`AiMessageEventStream`].
    #[allow(clippy::type_complexity)]
    pub fn stream_fn(
        self: &Arc<Self>,
    ) -> Box<
        dyn Fn(
                Model,
                Context,
                SimpleStreamOptions,
            ) -> Result<
                Box<dyn AssistantMessageEventStream>,
                Box<dyn std::error::Error + Send + Sync>,
            > + Send
            + Sync,
    > {
        let this = Arc::clone(self);
        Box::new(move |model, context, options| {
            this.call_count.fetch_add(1, Ordering::Relaxed);

            let step = {
                let mut responses = this.responses.lock().unwrap();
                if responses.is_empty() {
                    return Err("No more faux responses queued".into());
                }
                responses.remove(0)
            };

            let message = match step {
                FauxResponseStep::Message(msg) => msg,
                FauxResponseStep::Factory(factory) => factory(&context, &options, &model),
            };

            let (sender, stream) = AiMessageEventStream::new();
            stream_message(
                sender,
                &message,
                &context,
                this.min_token_size,
                this.max_token_size,
            );
            Ok(Box::new(stream) as Box<dyn AssistantMessageEventStream>)
        })
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Stream the given message through the sender, emitting start/delta/end events
/// for each content block.
fn stream_message(
    sender: AiMessageEventSender,
    message: &AssistantMessage,
    context: &Context,
    min_token_size: usize,
    max_token_size: usize,
) {
    let usage = message
        .usage
        .clone()
        .or_else(|| Some(estimate_usage(message, context)));

    let mut partial = AssistantMessage {
        content: vec![],
        stop_reason: message.stop_reason.clone(),
        stop_sequence: message.stop_sequence.clone(),
        usage,
        timestamp: message.timestamp,
        error_message: message.error_message.clone(),
    };

    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    for (index, block) in message.content.iter().enumerate() {
        match block {
            Content::Text(tc) => {
                sender.push(AssistantMessageEvent::TextStart {
                    index,
                    partial: partial.clone(),
                });
                let chunks = split_into_chunks(&tc.text, min_token_size, max_token_size);
                for chunk in &chunks {
                    sender.push(AssistantMessageEvent::TextDelta {
                        index,
                        delta: chunk.clone(),
                        partial: partial.clone(),
                    });
                }
                sender.push(AssistantMessageEvent::TextEnd {
                    index,
                    partial: partial.clone(),
                });
                // Accumulate content into partial
                partial.content.push(Content::Text(tc.clone()));
            }
            Content::Thinking(th) => {
                sender.push(AssistantMessageEvent::ThinkingStart {
                    index,
                    partial: partial.clone(),
                });
                let chunks = split_into_chunks(&th.thinking, min_token_size, max_token_size);
                for chunk in &chunks {
                    sender.push(AssistantMessageEvent::ThinkingDelta {
                        index,
                        delta: chunk.clone(),
                        partial: partial.clone(),
                    });
                }
                sender.push(AssistantMessageEvent::ThinkingEnd {
                    index,
                    partial: partial.clone(),
                });
                partial.content.push(Content::Thinking(th.clone()));
            }
            Content::ToolCall(tc) => {
                sender.push(AssistantMessageEvent::ToolCallStart {
                    index,
                    partial: partial.clone(),
                });
                let args_str = tc.arguments.to_string();
                let chunks = split_into_chunks(&args_str, min_token_size, max_token_size);
                for chunk in &chunks {
                    sender.push(AssistantMessageEvent::ToolCallDelta {
                        index,
                        delta: chunk.clone(),
                        partial: partial.clone(),
                    });
                }
                sender.push(AssistantMessageEvent::ToolCallEnd {
                    index,
                    partial: partial.clone(),
                });
                partial.content.push(Content::ToolCall(tc.clone()));
            }
            Content::Image(img) => {
                partial.content.push(Content::Image(img.clone()));
            }
        }
    }

    match &message.stop_reason {
        Some(StopReason::Error) | Some(StopReason::Aborted) => {
            sender.push(AssistantMessageEvent::Error {
                error: partial.clone(),
            });
        }
        _ => {
            sender.push(AssistantMessageEvent::Done {
                message: partial.clone(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Usage estimation
// ---------------------------------------------------------------------------

/// Estimate token usage from a message and its context.
fn estimate_usage(message: &AssistantMessage, context: &Context) -> Usage {
    let prompt_text = serialize_context(context);
    let prompt_tokens = estimate_tokens(&prompt_text);
    let output_tokens = estimate_tokens(&content_to_text(&message.content));

    Usage {
        input: prompt_tokens,
        output: output_tokens,
        cache_read: 0,
        cache_write: 0,
        total_tokens: prompt_tokens + output_tokens,
        cost: Cost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        },
    }
}

fn estimate_tokens(text: &str) -> u64 {
    (text.len() as f64 / 4.0).ceil() as u64
}

fn content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .map(|c| match c {
            Content::Text(t) => t.text.clone(),
            Content::Thinking(th) => th.thinking.clone(),
            Content::ToolCall(tc) => format!("{}:{}", tc.name, tc.arguments),
            Content::Image(img) => format!("[image:{}:{}]", img.media_type, img.data.len()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn serialize_context(context: &Context) -> String {
    let mut parts = Vec::new();
    if !context.system_prompt.is_empty() {
        parts.push(format!("system:{}", context.system_prompt));
    }
    for message in &context.messages {
        match message {
            cortexcode_ai_types::Message::User(m) => {
                parts.push(format!("user:{}", content_to_text(&m.content)));
            }
            cortexcode_ai_types::Message::Assistant(m) => {
                parts.push(format!("assistant:{}", content_to_text(&m.content)));
            }
            cortexcode_ai_types::Message::ToolResult(m) => {
                parts.push(format!("tool_result:{}", content_to_text(&m.content)));
            }
        }
    }
    if !context.tools.is_empty() {
        parts.push(format!("tools:{}", context.tools.len()));
    }
    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

/// Split text into deterministic-size chunks (simulated tokens).
fn split_into_chunks(text: &str, min_token_size: usize, max_token_size: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut index = 0;
    let range = max_token_size - min_token_size + 1;
    while index < text.len() {
        // Deterministic chunk size based on position
        let token_size = min_token_size + ((index / 4) % range);
        let char_size = std::cmp::max(1, token_size * 4);
        let end = std::cmp::min(index + char_size, text.len());
        chunks.push(text[index..end].to_string());
        index = end;
    }
    chunks
}

// ---------------------------------------------------------------------------
// Public helper functions for building faux messages
// ---------------------------------------------------------------------------

/// Create a `TextContent` block.
pub fn faux_text(text: &str) -> Content {
    Content::Text(TextContent {
        text: text.to_string(),
        cache_control: None,
    })
}

/// Create a `ThinkingContent` block.
pub fn faux_thinking(thinking: &str) -> Content {
    Content::Thinking(ThinkingContent {
        thinking: thinking.to_string(),
        signature: None,
    })
}

/// Create a `ToolCallContent` block.
pub fn faux_tool_call(name: &str, arguments: serde_json::Value, id: Option<String>) -> Content {
    Content::ToolCall(ToolCallContent {
        id: id.unwrap_or_else(|| format!("tool:{}", fast_hash(name))),
        name: name.to_string(),
        arguments,
    })
}

/// Build an `AssistantMessage` with text content.
pub fn faux_text_message(text: &str, stop_reason: Option<StopReason>) -> AssistantMessage {
    AssistantMessage {
        content: vec![faux_text(text)],
        stop_reason,
        stop_sequence: None,
        usage: Some(DEFAULT_USAGE),
        timestamp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        error_message: None,
    }
}

/// Build an `AssistantMessage` with multiple content blocks.
pub fn faux_message(
    content: Vec<Content>,
    stop_reason: Option<StopReason>,
    error_message: Option<String>,
) -> AssistantMessage {
    AssistantMessage {
        content,
        stop_reason,
        stop_sequence: None,
        usage: Some(DEFAULT_USAGE),
        timestamp: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        error_message,
    }
}

/// Build an error `AssistantMessage`.
pub fn faux_error(error: &str) -> AssistantMessage {
    faux_message(vec![], Some(StopReason::Error), Some(error.to_string()))
}

/// Build an aborted `AssistantMessage`.
pub fn faux_aborted() -> AssistantMessage {
    faux_message(
        vec![],
        Some(StopReason::Aborted),
        Some("Request was aborted".to_string()),
    )
}

// ---------------------------------------------------------------------------
// Utility: fast hash
// ---------------------------------------------------------------------------

fn fast_hash(input: &str) -> String {
    let mut h1: u32 = 0xdeadbeef;
    let mut h2: u32 = 0x41c6ce57;

    for byte in input.bytes() {
        let ch = byte as u32;
        h1 = h1.wrapping_mul(2654435761) ^ ch;
        h2 = h2.wrapping_mul(1597334677) ^ ch;
    }

    h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
    h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);

    format!("{:x}{:x}", h2, h1)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- helper content builders ---

    #[test]
    fn test_faux_text_content() {
        let content = faux_text("hello world");
        match content {
            Content::Text(t) => assert_eq!(t.text, "hello world"),
            _ => panic!("expected text content"),
        }
    }

    #[test]
    fn test_faux_thinking_content() {
        let content = faux_thinking("thinking text");
        match content {
            Content::Thinking(t) => assert_eq!(t.thinking, "thinking text"),
            _ => panic!("expected thinking content"),
        }
    }

    #[test]
    fn test_faux_tool_call_content() {
        let args = serde_json::json!({"a": 1});
        let content = faux_tool_call("test_tool", args.clone(), Some("id-1".into()));
        match content {
            Content::ToolCall(tc) => {
                assert_eq!(tc.name, "test_tool");
                assert_eq!(tc.arguments, args);
                assert_eq!(tc.id, "id-1");
            }
            _ => panic!("expected tool call content"),
        }
    }

    // --- message builders ---

    #[test]
    fn test_faux_text_message() {
        let msg = faux_text_message("hello", Some(StopReason::EndTurn));
        assert_eq!(msg.content.len(), 1);
        assert_eq!(msg.stop_reason, Some(StopReason::EndTurn));
        assert!(msg.timestamp.is_some());
    }

    #[test]
    fn test_faux_error_message() {
        let msg = faux_error("something went wrong");
        assert_eq!(msg.stop_reason, Some(StopReason::Error));
        assert_eq!(msg.error_message, Some("something went wrong".to_string()));
    }

    #[test]
    fn test_faux_aborted_message() {
        let msg = faux_aborted();
        assert_eq!(msg.stop_reason, Some(StopReason::Aborted));
        assert_eq!(msg.error_message, Some("Request was aborted".to_string()));
    }

    // --- basic streaming ---

    #[test]
    fn test_faux_provider_basic_stream() {
        let provider = Arc::new(FauxProvider::new());
        provider.set_responses(vec![FauxResponseStep::Message(faux_text_message(
            "Hello!",
            Some(StopReason::EndTurn),
        ))]);

        let stream_fn = provider.stream_fn();
        let model = default_faux_model();
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions::default();

        let mut stream = stream_fn(model, context, options).unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Some(event) = stream.next_event() {
            events.push(event);
        }

        assert!(!events.is_empty(), "should have at least one event");
        assert!(
            events.len() >= 3,
            "should have start, text events, and done"
        );

        // First event should be Start
        match &events[0] {
            AssistantMessageEvent::Start { partial: _ } => {}
            _ => panic!("expected Start event"),
        }

        // Last event should be Done
        match events.last().unwrap() {
            AssistantMessageEvent::Done { message } => {
                assert!(!message.content.is_empty());
            }
            other => panic!("expected Done event, got {:?}", other),
        }
    }

    #[test]
    fn test_faux_provider_error_stream() {
        let provider = Arc::new(FauxProvider::new());
        provider.set_responses(vec![FauxResponseStep::Message(faux_error("oops"))]);

        let stream_fn = provider.stream_fn();
        let model = default_faux_model();
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions::default();

        let mut stream = stream_fn(model, context, options).unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next_event() {
            events.push(event);
        }

        // Last event should be Error
        match events.last().unwrap() {
            AssistantMessageEvent::Error { error } => {
                assert_eq!(error.error_message, Some("oops".to_string()));
            }
            other => panic!("expected Error event, got {:?}", other),
        }
    }

    #[test]
    fn test_faux_provider_no_responses() {
        let provider = Arc::new(FauxProvider::new());
        let stream_fn = provider.stream_fn();
        let model = default_faux_model();
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions::default();

        let result = stream_fn(model, context, options);
        assert!(result.is_err());
        // Just check it's an error — the error type doesn't implement Debug
        // so we can't unwrap_err(). Instead verify via match.
        match result {
            Err(msg) => {
                let msg_str = msg.to_string();
                assert!(
                    msg_str.contains("No more faux responses"),
                    "expected 'No more faux responses', got: {msg_str}"
                );
            }
            Ok(_) => panic!("expected error"),
        }
    }

    // --- call count ---

    #[test]
    fn test_faux_provider_call_count() {
        let provider = Arc::new(FauxProvider::new());
        provider.set_responses(vec![
            FauxResponseStep::Message(faux_text_message("A", Some(StopReason::EndTurn))),
            FauxResponseStep::Message(faux_text_message("B", Some(StopReason::EndTurn))),
        ]);

        assert_eq!(provider.call_count(), 0);

        let stream_fn = provider.stream_fn();
        let model = default_faux_model();
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions::default();

        let _ = stream_fn(model.clone(), context.clone(), options.clone());
        assert_eq!(provider.call_count(), 1);

        let _ = stream_fn(model, context, options);
        assert_eq!(provider.call_count(), 2);
    }

    // --- factory responses ---

    #[test]
    fn test_faux_provider_with_factory() {
        let provider = Arc::new(FauxProvider::new());
        provider.set_responses(vec![FauxResponseStep::Factory(Box::new(
            |_ctx, _opts, _model| faux_text_message("Factory response", Some(StopReason::EndTurn)),
        ))]);

        let stream_fn = provider.stream_fn();
        let model = default_faux_model();
        let context = Context::new("".into(), vec![], vec![]);
        let options = SimpleStreamOptions::default();

        let mut stream = stream_fn(model, context, options).unwrap();
        // Collect all events (Start, TextStart, TextDelta*, TextEnd, Done)
        let mut event_count = 0;
        while let Some(event) = stream.next_event() {
            event_count += 1;
            if let AssistantMessageEvent::Done { message } = &event {
                assert!(message.content.len() == 1, "expected 1 content block");
            }
        }
        assert!(event_count > 1, "factory should produce multiple events");
    }

    // --- chunking ---

    #[test]
    fn test_split_into_chunks_empty() {
        let chunks = split_into_chunks("", 3, 5);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn test_split_into_chunks_short() {
        let chunks = split_into_chunks("hi", 3, 5);
        assert_eq!(chunks, vec!["hi"]);
    }

    #[test]
    fn test_split_into_chunks_long() {
        let text = "hello world this is a test of the chunking function";
        let chunks = split_into_chunks(text, 3, 5);
        assert!(chunks.len() > 1);
        // Verify the chunks reassemble to the original
        assert_eq!(chunks.concat(), text);
    }

    // --- usage estimation ---

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 2); // 5/4 = 1.25 → 2
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens(""), 0);
    }

    // --- helpers ---

    fn default_faux_model() -> Model {
        Model {
            id: "faux-1".into(),
            name: "Faux Model".into(),
            api: "faux".into(),
            provider: "faux".into(),
            base_url: "http://localhost:0".into(),
            reasoning: false,
            thinking_level_map: None,
            input: vec!["text".into(), "image".into()],
            cost: cortexcode_ai_types::ModelCost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
            context_window: 128_000,
            max_tokens: 16_384,
            headers: None,
        }
    }
}

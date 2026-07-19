//! Streaming response utilities for cortex AI.
//!
//! Provides a simple channel-based `AssistantMessageEventStream` implementation
//! that the agent loop uses to consume LLM responses event-by-event.

use cortexcode_ai_types::{AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream};
use std::sync::mpsc;

// ---------------------------------------------------------------------------
// AiMessageEventStream
// ---------------------------------------------------------------------------

/// A simple channel-backed implementation of `AssistantMessageEventStream`.
///
/// The producer sends events via the `Sender` handle. The consumer calls
/// `next_event()` to get events synchronously, and `result()` to get the
/// final `AssistantMessage` when the stream completes.
pub struct AiMessageEventStream {
    rx: mpsc::Receiver<StreamMessage>,
}

enum StreamMessage {
    Event(AssistantMessageEvent),
    Done(AssistantMessage),
}

impl AiMessageEventStream {
    /// Create a new stream, returning (producer, consumer).
    pub fn new() -> (AiMessageEventSender, Self) {
        let (tx, rx) = mpsc::channel();
        let stream = AiMessageEventStream { rx };
        let sender = AiMessageEventSender { tx };
        (sender, stream)
    }
}

impl AssistantMessageEventStream for AiMessageEventStream {
    fn next_event(&mut self) -> Option<AssistantMessageEvent> {
        match self.rx.recv() {
            Ok(StreamMessage::Event(event)) => Some(event),
            Ok(StreamMessage::Done(_)) => None,
            Err(_) => None,
        }
    }

    fn result(&mut self) -> AssistantMessage {
        // Drain remaining events until we find the Done message
        loop {
            match self.rx.recv() {
                Ok(StreamMessage::Done(msg)) => return msg,
                Ok(StreamMessage::Event(_)) => continue,
                Err(_) => {
                    return AssistantMessage {
                        content: vec![],
                        stop_reason: Some(cortexcode_ai_types::StopReason::Error),
                        stop_sequence: None,
                        usage: None,
                        timestamp: None,
                        error_message: Some("Stream ended without result".into()),
                    };
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AiMessageEventSender
// ---------------------------------------------------------------------------

/// Producer handle for `AiMessageEventStream`.
#[derive(Clone)]
pub struct AiMessageEventSender {
    tx: mpsc::Sender<StreamMessage>,
}

impl AiMessageEventSender {
    /// Push an event into the stream.
    pub fn push(&self, event: AssistantMessageEvent) {
        let _ = self.tx.send(StreamMessage::Event(event));
    }

    /// End the stream with a final result.
    pub fn end(&self, result: AssistantMessage) {
        let _ = self.tx.send(StreamMessage::Done(result));
    }
}

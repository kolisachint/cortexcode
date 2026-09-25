//! Server-Sent Events decoder shared by every cortex AI provider.
//!
//! hoocode reads provider streams with the vendor SDKs (and, for Anthropic,
//! with its own `iterateSseMessages` in `providers/anthropic.ts`). All of them
//! follow the WHATWG event-stream rules, which `eventsource-stream` implements.
//! One difference is kept from hoocode: a final event that is not followed by a
//! blank line is still delivered when the body ends (`flushSseEvent`), where the
//! spec would drop it.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use eventsource_stream::{EventStream, EventStreamError};
use futures_util::stream::{self, Chain, Once, Stream};

/// One decoded SSE message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, or `"message"` when the frame had none.
    pub event: String,
    /// The `data:` lines joined with `\n`.
    pub data: String,
    /// The last `id:` seen, or empty.
    pub id: String,
}

/// Why the event stream stopped.
#[derive(Debug)]
pub enum SseError<E> {
    /// The body was not valid UTF-8.
    Utf8(String),
    /// A line could not be parsed as an event-stream field.
    Parser(String),
    /// The underlying byte stream failed (network error, reset connection).
    Transport(E),
}

impl<E: fmt::Display> fmt::Display for SseError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8(e) => write!(f, "UTF8 error: {e}"),
            Self::Parser(e) => write!(f, "Parse error: {e}"),
            Self::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl<E: fmt::Display + fmt::Debug> std::error::Error for SseError<E> {}

/// A body chunk, or the synthetic blank line appended at the end of the body.
pub enum Chunk<B> {
    Body(B),
    Terminator,
}

impl<B: AsRef<[u8]>> AsRef<[u8]> for Chunk<B> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Body(b) => b.as_ref(),
            Self::Terminator => b"\n\n",
        }
    }
}

type Body<S, B, E> = Chain<
    stream::Map<S, fn(Result<B, E>) -> Result<Chunk<B>, E>>,
    Once<std::future::Ready<Result<Chunk<B>, E>>>,
>;

/// Stream of [`SseEvent`]s decoded from a byte stream such as
/// `reqwest::Response::bytes_stream()`.
pub struct SseStream<S, B, E>
where
    S: Stream<Item = Result<B, E>>,
{
    inner: Pin<Box<EventStream<Body<S, B, E>>>>,
}

/// Decode `body` as an event stream.
pub fn sse_events<S, B, E>(body: S) -> SseStream<S, B, E>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
{
    use futures_util::StreamExt;
    let wrap: fn(Result<B, E>) -> Result<Chunk<B>, E> = |r| r.map(Chunk::Body);
    let body = body
        .map(wrap)
        .chain(stream::once(std::future::ready(Ok(Chunk::Terminator))));
    SseStream {
        inner: Box::pin(EventStream::new(body)),
    }
}

impl<S, B, E> Stream for SseStream<S, B, E>
where
    S: Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
{
    type Item = Result<SseEvent, SseError<E>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx).map(|item| {
            item.map(|r| {
                r.map(|e| SseEvent {
                    event: e.event,
                    data: e.data,
                    id: e.id,
                })
                .map_err(|e| match e {
                    EventStreamError::Utf8(e) => SseError::Utf8(e.to_string()),
                    EventStreamError::Parser(e) => SseError::Parser(e.to_string()),
                    EventStreamError::Transport(e) => SseError::Transport(e),
                })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use futures_util::StreamExt;

    fn decode_chunks(chunks: &[&'static str]) -> Vec<SseEvent> {
        let body = stream::iter(
            chunks
                .iter()
                .map(|c| Ok::<_, std::convert::Infallible>(c.as_bytes())),
        );
        block_on(sse_events(body).map(|r| r.unwrap()).collect())
    }

    fn data(events: &[SseEvent]) -> Vec<&str> {
        events.iter().map(|e| e.data.as_str()).collect()
    }

    #[test]
    fn single_event_with_name() {
        let events = decode_chunks(&["event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message_stop");
        assert_eq!(events[0].data, "{\"type\":\"message_stop\"}");
    }

    #[test]
    fn multiple_events() {
        let events = decode_chunks(&["event: a\ndata: {\"x\":1}\n\nevent: b\ndata: {\"x\":2}\n\n"]);
        assert_eq!(data(&events), ["{\"x\":1}", "{\"x\":2}"]);
        assert_eq!(events[1].event, "b");
    }

    #[test]
    fn unnamed_event_is_message() {
        assert_eq!(decode_chunks(&["data: x\n\n"])[0].event, "message");
    }

    #[test]
    fn multiline_data_is_joined() {
        assert_eq!(
            data(&decode_chunks(&["data: line1\ndata: line2\n\n"])),
            ["line1\nline2"]
        );
    }

    #[test]
    fn trailing_event_without_blank_line_is_flushed() {
        assert_eq!(data(&decode_chunks(&["data: {\"x\":1}\n"])), ["{\"x\":1}"]);
        assert_eq!(data(&decode_chunks(&["data: {\"x\":1}"])), ["{\"x\":1}"]);
    }

    #[test]
    fn comments_ids_and_retry_are_not_events() {
        let events = decode_chunks(&[": keepalive\nid: 1\nretry: 5\ndata: {\"x\":1}\n\n"]);
        assert_eq!(data(&events), ["{\"x\":1}"]);
        assert_eq!(events[0].id, "1");
    }

    #[test]
    fn empty_body_has_no_events() {
        assert!(decode_chunks(&[]).is_empty());
        assert!(decode_chunks(&[""]).is_empty());
    }

    #[test]
    fn crlf_line_endings() {
        assert_eq!(
            data(&decode_chunks(&["data: a\r\n\r\ndata: b\r\n\r\n"])),
            ["a", "b"]
        );
    }

    #[test]
    fn events_split_across_chunks_and_inside_utf8() {
        // "é" is 0xC3 0xA9; split it between chunks.
        let body = stream::iter(vec![
            Ok::<_, std::convert::Infallible>(b"da".to_vec()),
            Ok(b"ta: caf\xC3".to_vec()),
            Ok(b"\xA9\n".to_vec()),
            Ok(b"\ndata: [DONE]\n\n".to_vec()),
        ]);
        let events: Vec<_> = block_on(sse_events(body).map(|r| r.unwrap()).collect());
        assert_eq!(data(&events), ["café", "[DONE]"]);
    }

    #[test]
    fn transport_errors_surface() {
        let body = stream::iter(vec![Ok(b"data: a\n\n".to_vec()), Err("connection reset")]);
        let items: Vec<_> = block_on(sse_events(body).collect());
        assert_eq!(items[0].as_ref().unwrap().data, "a");
        assert_eq!(
            items[1].as_ref().unwrap_err().to_string(),
            "connection reset"
        );
    }
}

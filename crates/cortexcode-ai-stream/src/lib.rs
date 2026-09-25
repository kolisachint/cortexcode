//! Event streams for cortex AI.
//!
//! Port of hoocode `packages/ai/src/utils/event-stream.ts`: an [`EventStream`]
//! is a queue that a producer `push`es into and a consumer reads as a
//! `futures::Stream`, plus a [`EventStream::final_result`] future that resolves
//! with the terminal event's result. [`AssistantMessageEventStream`] is the
//! instance every provider returns.
//!
//! Providers run their HTTP work on tokio through [`spawn_producer`], which
//! also ends the stream if the producer task stops without doing so (a panic).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use cortexcode_ai_types::{AssistantMessage, AssistantMessageEvent, StopReason};
use futures_util::{FutureExt, Stream, StreamExt};

// ---------------------------------------------------------------------------
// EventStream
// ---------------------------------------------------------------------------

struct Inner<T, R> {
    queue: VecDeque<T>,
    done: bool,
    result: Option<R>,
    stream_wakers: Vec<Waker>,
    result_wakers: Vec<Waker>,
}

type Predicate<T> = Arc<dyn Fn(&T) -> bool + Send + Sync>;
type Extract<T, R> = Arc<dyn Fn(&T) -> R + Send + Sync>;

/// `EventStream<T, R>`: a push-based async event queue with a final result.
///
/// Clones share the same queue, so a producer can keep a clone to `push` into
/// while the consumer iterates the original, as in TypeScript where both sides
/// hold the same object.
pub struct EventStream<T, R> {
    inner: Arc<Mutex<Inner<T, R>>>,
    is_complete: Predicate<T>,
    extract_result: Extract<T, R>,
}

impl<T, R> Clone for EventStream<T, R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            is_complete: self.is_complete.clone(),
            extract_result: self.extract_result.clone(),
        }
    }
}

impl<T, R: Clone> EventStream<T, R> {
    pub fn new(
        is_complete: impl Fn(&T) -> bool + Send + Sync + 'static,
        extract_result: impl Fn(&T) -> R + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                queue: VecDeque::new(),
                done: false,
                result: None,
                stream_wakers: Vec::new(),
                result_wakers: Vec::new(),
            })),
            is_complete: Arc::new(is_complete),
            extract_result: Arc::new(extract_result),
        }
    }

    /// `push()`: queue an event. A completing event resolves the final result;
    /// events pushed after the stream is done are dropped.
    pub fn push(&self, event: T) {
        let mut inner = self.inner.lock().unwrap();
        if inner.done {
            return;
        }
        if (self.is_complete)(&event) {
            inner.done = true;
            if inner.result.is_none() {
                inner.result = Some((self.extract_result)(&event));
            }
            wake_all(&mut inner.result_wakers);
        }
        inner.queue.push_back(event);
        wake_all(&mut inner.stream_wakers);
    }

    /// `end()`: mark the stream done, resolving the final result if one is
    /// given and none was resolved yet.
    pub fn end(&self, result: Option<R>) {
        let mut inner = self.inner.lock().unwrap();
        inner.done = true;
        if inner.result.is_none() {
            inner.result = result;
        }
        wake_all(&mut inner.stream_wakers);
        wake_all(&mut inner.result_wakers);
    }

    /// `result()`: resolves with the final result. Unlike the TypeScript
    /// promise, which never settles when the stream ends without one, this
    /// resolves to `None` in that case.
    pub fn final_result(&self) -> FinalResult<T, R> {
        FinalResult {
            inner: self.inner.clone(),
        }
    }
}

fn wake_all(wakers: &mut Vec<Waker>) {
    for w in wakers.drain(..) {
        w.wake();
    }
}

fn register(wakers: &mut Vec<Waker>, cx: &Context<'_>) {
    if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
        wakers.push(cx.waker().clone());
    }
}

impl<T, R> Stream for EventStream<T, R> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(event) = inner.queue.pop_front() {
            return Poll::Ready(Some(event));
        }
        if inner.done {
            return Poll::Ready(None);
        }
        register(&mut inner.stream_wakers, cx);
        Poll::Pending
    }
}

/// Future returned by [`EventStream::final_result`].
pub struct FinalResult<T, R> {
    inner: Arc<Mutex<Inner<T, R>>>,
}

impl<T, R: Clone> Future for FinalResult<T, R> {
    type Output = Option<R>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<R>> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(r) = &inner.result {
            return Poll::Ready(Some(r.clone()));
        }
        if inner.done {
            return Poll::Ready(None);
        }
        register(&mut inner.result_wakers, cx);
        Poll::Pending
    }
}

// ---------------------------------------------------------------------------
// AssistantMessageEventStream
// ---------------------------------------------------------------------------

/// `AssistantMessageEventStream`: completes on `done` or `error`, whose
/// message is the final result.
pub type AssistantMessageEventStream = EventStream<AssistantMessageEvent, AssistantMessage>;

/// `createAssistantMessageEventStream()`.
pub fn create_assistant_message_event_stream() -> AssistantMessageEventStream {
    EventStream::new(
        |e| {
            matches!(
                e,
                AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. }
            )
        },
        |e| match e {
            AssistantMessageEvent::Done { message } => message.clone(),
            AssistantMessageEvent::Error { error } => error.clone(),
            _ => unreachable!("only done/error complete the stream"),
        },
    )
}

impl EventStream<AssistantMessageEvent, AssistantMessage> {
    /// `await stream.result()`. A stream that ended without a terminal event
    /// yields an error message instead of hanging.
    pub async fn result(&self) -> AssistantMessage {
        self.final_result()
            .await
            .unwrap_or_else(|| AssistantMessage {
                provider: String::new(),
                response_id: None,
                response_model: None,
                api: String::new(),
                diagnostics: None,
                model: String::new(),
                content: vec![],
                stop_reason: StopReason::Error,
                usage: Default::default(),
                timestamp: cortexcode_ai_types::now_ms(),
                error_message: Some("Stream ended without result".into()),
            })
    }

    /// Blocking `next()` for synchronous callers (the agent loop until it
    /// turns async in ledger 7.3b). Must not be called on a tokio worker.
    pub fn next_blocking(&mut self) -> Option<AssistantMessageEvent> {
        futures_executor::block_on(self.next())
    }

    /// Blocking [`Self::result`] for synchronous callers and tests.
    pub fn result_blocking(&self) -> AssistantMessage {
        futures_executor::block_on(self.result())
    }
}

// ---------------------------------------------------------------------------
// Producer tasks
// ---------------------------------------------------------------------------

/// Run a provider's producer future on tokio: the current runtime when there
/// is one, otherwise a shared background runtime (synchronous callers).
///
/// If the future stops without ending `stream` (it panicked), the stream is
/// ended so consumers do not wait forever.
pub fn spawn_producer<T, R>(
    stream: &EventStream<T, R>,
    producer: impl Future<Output = ()> + Send + 'static,
) where
    T: Send + 'static,
    R: Clone + Send + 'static,
{
    let stream = stream.clone();
    let task = async move {
        let _ = std::panic::AssertUnwindSafe(producer).catch_unwind().await;
        stream.end(None);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(task);
        }
        Err(_) => {
            shared_runtime().spawn(task);
        }
    }
}

fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("cortex-ai-stream")
            .enable_all()
            .build()
            .expect("failed to start the provider runtime")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::StopReason;
    use futures_executor::block_on;

    fn message(text_marker: &str) -> AssistantMessage {
        AssistantMessage {
            provider: "p".into(),
            response_id: None,
            response_model: None,
            api: "a".into(),
            diagnostics: None,
            model: text_marker.into(),
            content: vec![],
            stop_reason: StopReason::Stop,
            usage: Default::default(),
            timestamp: 1,
            error_message: None,
        }
    }

    fn numbers() -> EventStream<i32, i32> {
        EventStream::new(|e| *e < 0, |e| -e)
    }

    #[test]
    fn events_are_delivered_in_order_then_end() {
        let s = numbers();
        s.push(1);
        s.push(2);
        s.end(None);
        let got: Vec<i32> = block_on(s.clone().collect());
        assert_eq!(got, [1, 2]);
        assert_eq!(block_on(s.final_result()), None);
    }

    #[test]
    fn completing_event_resolves_result_and_is_still_delivered() {
        let s = numbers();
        s.push(1);
        s.push(-7);
        // Pushed after completion: dropped.
        s.push(3);
        assert_eq!(block_on(s.final_result()), Some(7));
        let got: Vec<i32> = block_on(s.clone().collect());
        assert_eq!(got, [1, -7]);
    }

    #[test]
    fn end_does_not_override_a_resolved_result() {
        let s = numbers();
        s.push(-2);
        s.end(Some(99));
        assert_eq!(block_on(s.final_result()), Some(2));
    }

    #[test]
    fn end_with_result() {
        let s = numbers();
        s.end(Some(5));
        assert_eq!(block_on(s.final_result()), Some(5));
        assert_eq!(block_on(s.clone().collect::<Vec<_>>()), Vec::<i32>::new());
    }

    #[test]
    fn waiting_consumer_is_woken_by_a_producer_thread() {
        let s = numbers();
        let producer = s.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            producer.push(4);
            producer.push(-1);
        });
        let got: Vec<i32> = block_on(s.clone().collect());
        t.join().unwrap();
        assert_eq!(got, [4, -1]);
        assert_eq!(block_on(s.final_result()), Some(1));
    }

    #[test]
    fn assistant_stream_completes_on_done_and_error() {
        let s = create_assistant_message_event_stream();
        s.push(AssistantMessageEvent::Start {
            partial: message("partial"),
        });
        s.push(AssistantMessageEvent::Done {
            message: message("final"),
        });
        assert_eq!(s.result_blocking().model, "final");

        let s = create_assistant_message_event_stream();
        s.push(AssistantMessageEvent::Error {
            error: message("failed"),
        });
        assert_eq!(s.result_blocking().model, "failed");
    }

    #[test]
    fn assistant_stream_without_terminal_event_yields_an_error() {
        let s = create_assistant_message_event_stream();
        s.end(None);
        let r = s.result_blocking();
        assert_eq!(r.stop_reason, StopReason::Error);
        assert_eq!(
            r.error_message.as_deref(),
            Some("Stream ended without result")
        );
    }

    #[test]
    fn spawn_producer_without_a_runtime() {
        let mut s = create_assistant_message_event_stream();
        let p = s.clone();
        spawn_producer(&s, async move {
            tokio::task::yield_now().await;
            p.push(AssistantMessageEvent::Done {
                message: message("from task"),
            });
        });
        assert!(matches!(
            s.next_blocking(),
            Some(AssistantMessageEvent::Done { .. })
        ));
        assert_eq!(s.next_blocking().map(|_| ()), None);
    }

    #[test]
    fn panicking_producer_ends_the_stream() {
        let s = create_assistant_message_event_stream();
        spawn_producer(&s, async move { panic!("provider bug") });
        assert_eq!(s.result_blocking().stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn spawn_producer_inside_a_runtime() {
        let s = create_assistant_message_event_stream();
        let p = s.clone();
        spawn_producer(&s, async move {
            p.push(AssistantMessageEvent::Done {
                message: message("rt"),
            });
        });
        assert_eq!(s.result().await.model, "rt");
    }
}

/// Test support: a one-shot HTTP server for provider tests.
#[cfg(feature = "testing")]
pub mod testing {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// One piece of a scripted response body.
    pub enum Part {
        /// Bytes written (and flushed) as they are.
        Bytes(String),
        /// Pause before the next part.
        Sleep(Duration),
    }

    /// Serve one request on `127.0.0.1`: `status_line` (e.g. `HTTP/1.1 200 OK`),
    /// `content_type`, then `parts` in order, then close. The body is sent without
    /// a length, so the client reads until the connection closes. Returns the
    /// base URL.
    pub fn serve_once(status_line: &str, content_type: &str, parts: Vec<Part>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        let head =
            format!("{status_line}\r\ncontent-type: {content_type}\r\nconnection: close\r\n\r\n");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request (headers + body) without parsing it.
                let mut buf = [0u8; 65536];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(head.as_bytes());
                for part in parts {
                    match part {
                        Part::Bytes(b) => {
                            if stream.write_all(b.as_bytes()).is_err() {
                                return;
                            }
                            let _ = stream.flush();
                        }
                        Part::Sleep(d) => std::thread::sleep(d),
                    }
                }
            }
        });
        format!("http://{addr}")
    }

    /// Serve `body` as a complete `text/event-stream` response.
    pub fn serve_sse(body: &str) -> String {
        serve_once(
            "HTTP/1.1 200 OK",
            "text/event-stream",
            vec![Part::Bytes(body.to_string())],
        )
    }

    /// Serve `head` as an event stream, then hold the connection open for
    /// `hold` (long enough for a client to abort mid-stream).
    pub fn serve_sse_then_hang(head: &str, hold: Duration) -> String {
        serve_once(
            "HTTP/1.1 200 OK",
            "text/event-stream",
            vec![Part::Bytes(head.to_string()), Part::Sleep(hold)],
        )
    }

    /// Serve an error response with a JSON body.
    pub fn serve_error(status_line: &str, body: &str) -> String {
        serve_once(
            status_line,
            "application/json",
            vec![Part::Bytes(body.to_string())],
        )
    }
}

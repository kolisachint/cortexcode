//! The SSE decoding of `providers/anthropic.ts`: `iterateSseMessages` (a
//! line decoder that splits on `\r`, `\n` or `\r\n`) and the event filter of
//! `iterateAnthropicEvents`.

use serde_json::Value;

/// `ServerSentEvent`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSentEvent {
    pub event: Option<String>,
    pub data: String,
    pub raw: Vec<String>,
}

/// `SseDecoderState` plus the undecoded buffer.
#[derive(Debug, Default)]
pub struct SseDecoder {
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<String>,
    buffer: Vec<u8>,
}

impl SseDecoder {
    /// Feed a chunk; returns the events completed by it.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<ServerSentEvent> {
        self.buffer.extend_from_slice(chunk);
        self.drain_lines()
    }

    /// End of the body: the remaining lines, a last unterminated line, then
    /// whatever event is still pending.
    pub fn finish(&mut self) -> Vec<ServerSentEvent> {
        let mut events = self.drain_lines();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
            events.extend(self.decode_line(&line));
        }
        events.extend(self.flush());
        events
    }

    /// `consumeLine` until no line break is left.
    fn drain_lines(&mut self) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\r' || *b == b'\n') {
            let mut next = pos + 1;
            if self.buffer[pos] == b'\r' && self.buffer.get(next) == Some(&b'\n') {
                next += 1;
            }
            let line = String::from_utf8_lossy(&self.buffer[..pos]).into_owned();
            self.buffer.drain(..next);
            events.extend(self.decode_line(&line));
        }
        events
    }

    /// `decodeSseLine`.
    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }
        self.raw.push(line.to_string());
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.find(':') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, ""),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
        None
    }

    /// `flushSseEvent`.
    fn flush(&mut self) -> Option<ServerSentEvent> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let event = ServerSentEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            raw: std::mem::take(&mut self.raw),
        };
        Some(event)
    }
}

const ANTHROPIC_MESSAGE_EVENTS: &[&str] = &[
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

/// The per-event part of `iterateAnthropicEvents`, tracking whether the
/// message started and stopped.
#[derive(Debug, Default)]
pub struct AnthropicEventFilter {
    saw_message_start: bool,
    saw_message_end: bool,
}

impl AnthropicEventFilter {
    /// `Ok(Some(event))` for a message event, `Ok(None)` for one to skip,
    /// `Err` for an `error` event or unparseable data.
    pub fn accept(&mut self, sse: &ServerSentEvent) -> Result<Option<Value>, String> {
        let name = sse.event.as_deref().unwrap_or("");
        if name == "error" {
            return Err(sse.data.clone());
        }
        if !ANTHROPIC_MESSAGE_EVENTS.contains(&name) {
            return Ok(None);
        }
        match cortexcode_ai_util::parse_json_with_repair::<Value>(&sse.data) {
            Ok(event) => {
                match event["type"].as_str() {
                    Some("message_start") => self.saw_message_start = true,
                    Some("message_stop") => self.saw_message_end = true,
                    _ => {}
                }
                Ok(Some(event))
            }
            // The message is serde's, not V8's JSON.parse text.
            Err(error) => Err(format!(
                "Could not parse Anthropic SSE event {name}: {error}; data={}; raw={}",
                sse.data,
                sse.raw.join("\\n")
            )),
        }
    }

    /// End of the stream: a started message must have stopped.
    pub fn finish(&self) -> Result<(), String> {
        if self.saw_message_start && !self.saw_message_end {
            return Err("Anthropic stream ended before message_stop".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(chunks: &[&str]) -> Vec<ServerSentEvent> {
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for chunk in chunks {
            events.extend(decoder.push(chunk.as_bytes()));
        }
        events.extend(decoder.finish());
        events
    }

    #[test]
    fn splits_on_any_line_break_and_joins_data_lines() {
        let events = decode(&["event: a\r\ndata: 1\rdata: 2\n\n: comment\nevent:b\ndata:x\n\n"]);
        assert_eq!(
            events,
            vec![
                ServerSentEvent {
                    event: Some("a".into()),
                    data: "1\n2".into(),
                    raw: vec!["event: a".into(), "data: 1".into(), "data: 2".into()],
                },
                ServerSentEvent {
                    event: Some("b".into()),
                    data: "x".into(),
                    raw: vec![": comment".into(), "event:b".into(), "data:x".into()],
                },
            ]
        );
    }

    #[test]
    fn events_may_span_chunks_and_the_last_one_needs_no_blank_line() {
        let events = decode(&[
            "event: message_st",
            "art\ndata: {\"a\"",
            ":1}\n\nevent: x\ndata: y",
        ]);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "{\"a\":1}");
        assert_eq!(events[1].event.as_deref(), Some("x"));
        assert_eq!(events[1].data, "y");
    }

    #[test]
    fn filter_skips_foreign_events_and_fails_on_error_events() {
        let mut filter = AnthropicEventFilter::default();
        let sse = |event: &str, data: &str| ServerSentEvent {
            event: Some(event.into()),
            data: data.into(),
            raw: vec![],
        };
        assert_eq!(filter.accept(&sse("done", "[DONE]")), Ok(None));
        assert_eq!(filter.accept(&sse("proxy.stats", "not json")), Ok(None));
        assert_eq!(
            filter.accept(&sse("error", "{\"type\":\"error\"}")),
            Err("{\"type\":\"error\"}".into())
        );
        assert!(filter
            .accept(&sse("message_start", "{\"type\":\"message_start\"}"))
            .unwrap()
            .is_some());
        assert_eq!(
            filter.finish(),
            Err("Anthropic stream ended before message_stop".into())
        );
        filter
            .accept(&sse("message_stop", "{\"type\":\"message_stop\"}"))
            .unwrap();
        assert_eq!(filter.finish(), Ok(()));
    }
}

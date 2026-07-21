//! Output formatting for the cortex coding agent.
//!
//! Mirrors the output-formatting side of `modes/print-mode.ts` from the
//! TypeScript `packages/coding-agent` package. It provides two render targets:
//!
//! * **Text mode** — prints the final assistant response as plain text.
//! * **JSON mode** — streams every agent event as a JSON line (`\n`-delimited).
//!
//! These helpers are used by the non-interactive `cortex -p` / `cortex --mode json`
//! CLI entry points.

use cortexcode_agent_types::{AgentEvent, AgentMessage, AgentToolResult};
use cortexcode_ai_types::{Content, Message, StopReason, TextContent};
use serde::Serialize;
use std::io::Write;

/// Output target for print mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrintMode {
    /// Print the final assistant response only.
    #[default]
    Text,
    /// Stream every agent event as a JSON line.
    Json,
}

impl std::str::FromStr for PrintMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(PrintMode::Text),
            "json" => Ok(PrintMode::Json),
            _ => Err(format!("unknown print mode: {}", s)),
        }
    }
}

/// Error type for print-mode formatting operations.
#[derive(Debug)]
pub enum PrintError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for PrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrintError::Io(e) => write!(f, "io error: {}", e),
            PrintError::Json(e) => write!(f, "json error: {}", e),
        }
    }
}

impl std::error::Error for PrintError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PrintError::Io(e) => Some(e),
            PrintError::Json(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for PrintError {
    fn from(e: std::io::Error) -> Self {
        PrintError::Io(e)
    }
}

impl From<serde_json::Error> for PrintError {
    fn from(e: serde_json::Error) -> Self {
        PrintError::Json(e)
    }
}

/// Extract all text from an assistant message's content blocks.
pub fn assistant_text(message: &cortexcode_ai_types::AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(TextContent { text, .. }) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format the final assistant message as plain text.
///
/// If the last message is an assistant message that stopped with an error or
/// abort, the returned string contains the error text instead.
pub fn format_text_output(messages: &[AgentMessage]) -> String {
    match messages.last().and_then(|m| m.extract_message()) {
        Some(Message::Assistant(am)) => match am.stop_reason {
            Some(StopReason::Error | StopReason::Aborted) => {
                format!(
                    "Error: {}",
                    am.error_message.as_deref().unwrap_or("request failed")
                )
            }
            _ => assistant_text(&am),
        },
        _ => String::new(),
    }
}

/// Write the final assistant response as plain text to `output`.
pub fn write_text_output(
    messages: &[AgentMessage],
    output: &mut dyn Write,
) -> Result<(), PrintError> {
    let text = format_text_output(messages);
    if !text.is_empty() {
        writeln!(output, "{}", text)?;
    }
    Ok(())
}

/// JSON-serializable view of a tool result.
#[derive(Debug, Serialize)]
struct JsonToolResult {
    content: Vec<JsonContent>,
    details: serde_json::Value,
    terminate: bool,
}

impl From<&AgentToolResult> for JsonToolResult {
    fn from(result: &AgentToolResult) -> Self {
        Self {
            content: result.content.iter().map(JsonContent::from).collect(),
            details: result.details.clone(),
            terminate: result.terminate,
        }
    }
}

/// JSON-serializable view of a content block.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "value")]
enum JsonContent {
    Text(String),
    Image {
        data: String,
        media_type: String,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

impl From<&Content> for JsonContent {
    fn from(content: &Content) -> Self {
        match content {
            Content::Text(t) => JsonContent::Text(t.text.clone()),
            Content::Image(img) => JsonContent::Image {
                data: img.data.clone(),
                media_type: img.media_type.clone(),
            },
            Content::Thinking(t) => JsonContent::Thinking {
                thinking: t.thinking.clone(),
                signature: t.signature.clone(),
            },
            Content::ToolCall(tc) => JsonContent::ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            },
        }
    }
}

/// JSON-serializable view of an agent event.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub struct JsonEvent {
    #[serde(flatten)]
    payload: JsonEventPayload,
}

#[derive(Debug, Serialize)]
#[serde(tag = "event", content = "data")]
enum JsonEventPayload {
    AgentStart,
    TurnStart,
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        assistant_message_event: JsonAssistantMessageEvent,
        message: AgentMessage,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        result: JsonToolResult,
        is_error: bool,
    },
    TurnEnd {
        message: cortexcode_ai_types::AssistantMessage,
        tool_results: Vec<Message>,
    },
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
enum JsonAssistantMessageEvent {
    TextStart { index: usize },
    TextDelta { index: usize, delta: String },
    TextEnd { index: usize },
    ThinkingStart { index: usize },
    ThinkingDelta { index: usize, delta: String },
    ThinkingEnd { index: usize },
    ToolCallStart { index: usize },
    ToolCallDelta { index: usize, delta: String },
    ToolCallEnd { index: usize },
}

impl From<&cortexcode_agent_types::AssistantMessagePartialEvent> for JsonAssistantMessageEvent {
    fn from(event: &cortexcode_agent_types::AssistantMessagePartialEvent) -> Self {
        use cortexcode_agent_types::AssistantMessagePartialEvent as E;
        match event {
            E::TextStart { index } => JsonAssistantMessageEvent::TextStart { index: *index },
            E::TextDelta { index, delta } => JsonAssistantMessageEvent::TextDelta {
                index: *index,
                delta: delta.clone(),
            },
            E::TextEnd { index } => JsonAssistantMessageEvent::TextEnd { index: *index },
            E::ThinkingStart { index } => {
                JsonAssistantMessageEvent::ThinkingStart { index: *index }
            }
            E::ThinkingDelta { index, delta } => JsonAssistantMessageEvent::ThinkingDelta {
                index: *index,
                delta: delta.clone(),
            },
            E::ThinkingEnd { index } => JsonAssistantMessageEvent::ThinkingEnd { index: *index },
            E::ToolCallStart { index } => {
                JsonAssistantMessageEvent::ToolCallStart { index: *index }
            }
            E::ToolCallDelta { index, delta } => JsonAssistantMessageEvent::ToolCallDelta {
                index: *index,
                delta: delta.clone(),
            },
            E::ToolCallEnd { index } => JsonAssistantMessageEvent::ToolCallEnd { index: *index },
        }
    }
}

impl From<&AgentEvent> for JsonEvent {
    fn from(event: &AgentEvent) -> Self {
        use cortexcode_agent_types::AgentEvent as E;
        let payload = match event {
            E::AgentStart => JsonEventPayload::AgentStart,
            E::TurnStart => JsonEventPayload::TurnStart,
            E::MessageStart { message } => JsonEventPayload::MessageStart {
                message: message.clone(),
            },
            E::MessageUpdate {
                assistant_message_event,
                message,
            } => JsonEventPayload::MessageUpdate {
                assistant_message_event: JsonAssistantMessageEvent::from(assistant_message_event),
                message: message.clone(),
            },
            E::MessageEnd { message } => JsonEventPayload::MessageEnd {
                message: message.clone(),
            },
            E::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => JsonEventPayload::ToolExecutionStart {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
            },
            E::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                args,
                result,
                is_error,
            } => JsonEventPayload::ToolExecutionEnd {
                tool_call_id: tool_call_id.clone(),
                tool_name: tool_name.clone(),
                args: args.clone(),
                result: JsonToolResult::from(result),
                is_error: *is_error,
            },
            E::TurnEnd {
                message,
                tool_results,
            } => JsonEventPayload::TurnEnd {
                message: message.clone(),
                tool_results: tool_results.clone(),
            },
            E::AgentEnd { messages } => JsonEventPayload::AgentEnd {
                messages: messages.clone(),
            },
        };
        JsonEvent { payload }
    }
}

/// Serialize a single agent event to a JSON line.
pub fn format_json_event(event: &AgentEvent) -> Result<String, PrintError> {
    let json = JsonEvent::from(event);
    Ok(serde_json::to_string(&json)?)
}

/// Write a JSON line for each agent event to `output`.
pub fn write_json_output(events: &[AgentEvent], output: &mut dyn Write) -> Result<(), PrintError> {
    for event in events {
        writeln!(output, "{}", format_json_event(event)?)?;
    }
    Ok(())
}

/// Convenience formatter that collects events and renders the final output.
#[derive(Debug, Default)]
pub struct PrintFormatter {
    mode: PrintMode,
    events: Vec<AgentEvent>,
}

impl PrintFormatter {
    /// Create a formatter for the given mode.
    pub fn new(mode: PrintMode) -> Self {
        Self {
            mode,
            events: Vec::new(),
        }
    }

    /// Record an event while the agent is running.
    pub fn record(&mut self, event: AgentEvent) {
        self.events.push(event);
    }

    /// Consume the formatter and write the final output.
    pub fn finalize(self, output: &mut dyn Write) -> Result<(), PrintError> {
        match self.mode {
            PrintMode::Text => {
                let messages: Vec<AgentMessage> = self
                    .events
                    .iter()
                    .filter_map(|e| match e {
                        AgentEvent::AgentEnd { messages } => Some(messages.clone()),
                        _ => None,
                    })
                    .next()
                    .unwrap_or_default();
                write_text_output(&messages, output)
            }
            PrintMode::Json => write_json_output(&self.events, output),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_agent_types::AgentEvent;
    use cortexcode_ai_types::{AssistantMessage, StopReason, TextContent, UserMessage};

    fn make_text_assistant(text: &str, stop: Option<StopReason>) -> AgentMessage {
        make_text_assistant_with_error(text, stop, None)
    }

    fn make_text_assistant_with_error(
        text: &str,
        stop: Option<StopReason>,
        error_message: Option<&str>,
    ) -> AgentMessage {
        AgentMessage::from_message(Message::Assistant(AssistantMessage {
            content: vec![Content::Text(TextContent {
                text: text.to_string(),
                cache_control: None,
            })],
            stop_reason: stop,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: error_message.map(|s| s.to_string()),
        }))
    }

    fn make_user(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text: text.to_string(),
                cache_control: None,
            })],
            timestamp: None,
        }))
    }

    #[test]
    fn test_format_text_output() {
        let messages = vec![make_user("hi"), make_text_assistant("hello", None)];
        assert_eq!(format_text_output(&messages), "hello");
    }

    #[test]
    fn test_format_text_output_error() {
        let msg =
            make_text_assistant_with_error("oops", Some(StopReason::Error), Some("model error"));
        let messages = vec![make_user("hi"), msg];
        assert_eq!(format_text_output(&messages), "Error: model error");
    }

    #[test]
    fn test_json_event_roundtrip() {
        let event = AgentEvent::AgentStart;
        let line = format_json_event(&event).unwrap();
        assert!(line.contains("AgentStart"));
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["event"], "AgentStart");
    }

    #[test]
    fn test_print_formatter_text() {
        let mut formatter = PrintFormatter::new(PrintMode::Text);
        formatter.record(AgentEvent::AgentStart);
        formatter.record(AgentEvent::AgentEnd {
            messages: vec![make_user("hi"), make_text_assistant("done", None)],
        });
        let mut buf = Vec::new();
        formatter.finalize(&mut buf).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap().trim(), "done");
    }

    #[test]
    fn test_print_formatter_json() {
        let mut formatter = PrintFormatter::new(PrintMode::Json);
        formatter.record(AgentEvent::AgentStart);
        formatter.record(AgentEvent::TurnStart);
        let mut buf = Vec::new();
        formatter.finalize(&mut buf).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&buf).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("AgentStart"));
        assert!(lines[1].contains("TurnStart"));
    }

    #[test]
    fn test_print_mode_from_str() {
        assert_eq!("text".parse::<PrintMode>().unwrap(), PrintMode::Text);
        assert_eq!("json".parse::<PrintMode>().unwrap(), PrintMode::Json);
        assert!("html".parse::<PrintMode>().is_err());
    }
}

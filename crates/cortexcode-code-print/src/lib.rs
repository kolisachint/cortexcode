//! Output formatting for the cortex coding agent.
//!
//! Mirrors the output-formatting side of `modes/print-mode.ts` from the
//! TypeScript `packages/coding-agent` package. It provides two render targets:
//!
//! * **Text mode** — prints the final assistant response as plain text.
//! * **JSON mode** — the session header, then every session event as one JSON
//!   line, in hoocode's wire shape (`docs/json.md`).
//!
//! These helpers are used by the non-interactive `cortex -p` / `cortex --mode json`
//! CLI entry points.

#[cfg(doc)]
use cortexcode_agent_types::AgentEvent;
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, Message, StopReason, TextContent};
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
            StopReason::Error | StopReason::Aborted => {
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

/// What `--print` text mode writes once all prompts have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextResult {
    /// Written to stdout as-is.
    pub stdout: String,
    /// One line written to stderr (without the trailing newline).
    pub stderr: Option<String>,
    pub exit_code: i32,
}

/// The `mode === "text"` tail of `runPrintMode` (print-mode.ts), applied to the
/// final transcript. When the last message is an assistant message:
/// - `stopReason` `error`/`aborted`: `errorMessage || "Request <reason>"` on stderr, exit 1;
/// - otherwise each text block followed by `\n` on stdout.
///
/// Any other last message prints nothing and exits 0.
pub fn text_result(transcript: &[AgentMessage]) -> TextResult {
    let mut result = TextResult {
        stdout: String::new(),
        stderr: None,
        exit_code: 0,
    };
    if let Some(AgentMessage::Assistant(am)) = transcript.last() {
        match am.stop_reason {
            StopReason::Error | StopReason::Aborted => {
                let reason = if am.stop_reason == StopReason::Error {
                    "error"
                } else {
                    "aborted"
                };
                let message = am
                    .error_message
                    .clone()
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| format!("Request {reason}"));
                result.stderr = Some(message);
                result.exit_code = 1;
            }
            _ => {
                for content in &am.content {
                    if let Content::Text(TextContent { text, .. }) = content {
                        result.stdout.push_str(text);
                        result.stdout.push('\n');
                    }
                }
            }
        }
    }
    result
}

/// One line of the `--mode json` stream: `JSON.stringify(value) + "\n"`.
/// Events come from `AgentSessionEvent::to_json` / [`AgentEvent::to_json`],
/// the first line is the session header.
pub fn json_line(value: &serde_json::Value) -> String {
    format!("{value}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(content: Vec<Content>, stop: StopReason, error: Option<&str>) -> AgentMessage {
        AgentMessage::Assistant(cortexcode_ai_types::AssistantMessage {
            content,
            stop_reason: stop,
            error_message: error.map(String::from),
            ..Default::default()
        })
    }

    fn text(t: &str) -> Content {
        Content::Text(TextContent {
            text: t.into(),
            text_signature: None,
        })
    }

    #[test]
    fn text_result_prints_each_text_block_on_its_own_line() {
        let r = text_result(&[assistant(
            vec![text("a"), text("b")],
            StopReason::Stop,
            None,
        )]);
        assert_eq!(r.stdout, "a\nb\n");
        assert_eq!((r.stderr, r.exit_code), (None, 0));
    }

    #[test]
    fn text_result_reports_errors_on_stderr() {
        let r = text_result(&[assistant(
            vec![text("partial")],
            StopReason::Error,
            Some("boom"),
        )]);
        assert_eq!(
            r,
            TextResult {
                stdout: String::new(),
                stderr: Some("boom".into()),
                exit_code: 1
            }
        );
        let r = text_result(&[assistant(vec![], StopReason::Aborted, None)]);
        assert_eq!(r.stderr.as_deref(), Some("Request aborted"));
        let r = text_result(&[assistant(vec![], StopReason::Error, Some(""))]);
        assert_eq!(r.stderr.as_deref(), Some("Request error"));
    }

    #[test]
    fn text_result_ignores_a_non_assistant_last_message() {
        let r = text_result(&[AgentMessage::user_text("hi")]);
        assert_eq!((r.stdout.as_str(), r.stderr, r.exit_code), ("", None, 0));
    }
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
            provider: String::new(),
            response_id: None,
            response_model: None,
            api: String::new(),
            diagnostics: None,
            model: String::new(),
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: text.to_string(),
            })],
            stop_reason: stop.unwrap_or_default(),

            usage: Default::default(),
            timestamp: 0,
            error_message: error_message.map(|s| s.to_string()),
        }))
    }

    fn make_user(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text_signature: None,
                text: text.to_string(),
            })]
            .into(),
            timestamp: 0,
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
    fn test_print_mode_from_str() {
        assert_eq!("text".parse::<PrintMode>().unwrap(), PrintMode::Text);
        assert_eq!("json".parse::<PrintMode>().unwrap(), PrintMode::Json);
        assert!("html".parse::<PrintMode>().is_err());
    }
}

//! Helpers for converting and inspecting agent messages.

use cortexcode_agent_types::{
    AgentMessage, AgentToolCall, BackgroundToolResult, BashExecutionMessage, BranchSummaryMessage,
    CompactionSummaryMessage, CustomMessage,
};
use cortexcode_ai_types::{AssistantMessage, Content, Message, UserContent, UserMessage};
use serde_json::{json, Value};

// Constants and conversions ported from hoocode `packages/agent/src/harness/messages.ts`.

pub const COMPACTION_SUMMARY_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";
/// `customType` of the follow-up message a finished background tool injects.
pub const BACKGROUND_TASK_CUSTOM_TYPE: &str = "backgroundTask";

/// `bashExecutionToText()`: how a `!` command is shown to the LLM.
pub fn bash_execution_to_text(msg: &BashExecutionMessage) -> String {
    let mut text = format!("Ran `{}`\n", msg.command);
    if !msg.output.is_empty() {
        text.push_str(&format!("```\n{}\n```", msg.output));
    } else {
        text.push_str("(no output)");
    }
    if msg.cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = msg.exit_code.filter(|c| *c != 0) {
        text.push_str(&format!("\n\nCommand exited with code {code}"));
    }
    if msg.truncated {
        if let Some(path) = &msg.full_output_path {
            text.push_str(&format!("\n\n[Output truncated. Full output: {path}]"));
        }
    }
    text
}

/// `new Date(iso).getTime()` for the entry timestamps (ISO 8601); 0 when
/// unparsable (JS would give NaN).
fn iso_to_ms(timestamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// `createBranchSummaryMessage`.
pub fn create_branch_summary_message(
    summary: impl Into<String>,
    from_id: impl Into<String>,
    timestamp: &str,
) -> BranchSummaryMessage {
    BranchSummaryMessage {
        summary: summary.into(),
        from_id: from_id.into(),
        timestamp: iso_to_ms(timestamp),
    }
}

/// `createCompactionSummaryMessage`.
pub fn create_compaction_summary_message(
    summary: impl Into<String>,
    tokens_before: u64,
    timestamp: &str,
    tokens_after: Option<u64>,
) -> CompactionSummaryMessage {
    CompactionSummaryMessage {
        summary: summary.into(),
        tokens_before,
        tokens_after,
        timestamp: iso_to_ms(timestamp),
    }
}

/// `createCustomMessage`: a `CustomMessageEntry` as an agent message.
pub fn create_custom_message(
    custom_type: impl Into<String>,
    content: UserContent,
    display: bool,
    details: Option<Value>,
    timestamp: &str,
) -> CustomMessage {
    CustomMessage {
        custom_type: custom_type.into(),
        content,
        display,
        details,
        timestamp: iso_to_ms(timestamp),
    }
}

/// A consistent description of a background tool call (`BackgroundToolInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundToolInfo {
    /// MCP server tools are registered as `mcp_<server>_<tool>`.
    pub is_mcp_tool: bool,
    /// The subagent type of `Task` calls; the tool name otherwise.
    pub subagent_type: String,
    /// Label used verbatim in both the start and finish messages.
    pub label: String,
    /// One-line summary of the call's arguments (may be empty).
    pub summary: Option<String>,
}

/// `str.length` (UTF-16 code units).
fn js_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// `str.slice(0, n)` in UTF-16 code units.
fn js_slice(text: &str, n: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// JS `trimEnd()`/`trim()` whitespace (differs from Rust's in U+FEFF / U+0085).
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `firstLine`: the first non-blank line, trimmed and capped at `max`.
fn first_line(text: &str, max: usize) -> String {
    let line = text
        .split('\n')
        .find(|l| !l.trim_matches(is_js_space).is_empty())
        .unwrap_or("")
        .trim_matches(is_js_space);
    if js_len(line) > max {
        format!(
            "{}…",
            js_slice(line, max.saturating_sub(1)).trim_end_matches(is_js_space)
        )
    } else {
        line.to_string()
    }
}

/// `summarizeArgs`: up to three `key: value` pairs.
pub fn summarize_args(args: &Value) -> Option<String> {
    let mut parts = Vec::new();
    for (key, value) in args.as_object().into_iter().flatten() {
        if value.is_null() || value.as_str() == Some("") {
            continue;
        }
        let rendered = match value.as_str() {
            Some(text) => text.to_string(),
            None => value.to_string(),
        };
        parts.push(format!("{key}: {}", first_line(&rendered, 48)));
        if parts.len() == 3 {
            break;
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// `describeBackgroundTool`: `Task` calls by `subagent_type`, MCP tools by
/// their `mcp_` prefix.
pub fn describe_background_tool(tool_call: &AgentToolCall) -> BackgroundToolInfo {
    let args = &tool_call.arguments;
    if let Some(pretty) = tool_call.name.strip_prefix("mcp_") {
        return BackgroundToolInfo {
            is_mcp_tool: true,
            subagent_type: tool_call.name.clone(),
            label: format!("MCP tool `{pretty}`"),
            summary: summarize_args(args),
        };
    }
    let subagent_type = args["subagent_type"]
        .as_str()
        .unwrap_or(&tool_call.name)
        .to_string();
    let description = args["description"]
        .as_str()
        .map(|d| d.trim_matches(is_js_space))
        .filter(|d| !d.is_empty());
    let summary = match description {
        Some(description) => Some(description.to_string()),
        None => args["prompt"].as_str().map(|p| first_line(p, 120)),
    };
    BackgroundToolInfo {
        is_mcp_tool: false,
        label: format!("subagent `{subagent_type}`"),
        subagent_type,
        summary,
    }
}

/// `createBackgroundPlaceholderText`: the line shown when a background tool
/// is dispatched.
pub fn create_background_placeholder_text(tool_call: &AgentToolCall) -> String {
    let info = describe_background_tool(tool_call);
    let what = info
        .summary
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| format!(" — {}", first_line(s, 80)))
        .unwrap_or_default();
    if info.is_mcp_tool {
        return format!(
            "Started {} in the background{what}. Its result arrives as a follow-up; keep working.",
            info.label
        );
    }
    format!(
        "Delegated to {} in the background{what}. I'll be notified when it finishes; use TaskOutput to check progress or read the result.",
        info.label
    )
}

/// `createBackgroundTaskMessage`: the follow-up a finished background tool
/// injects. A subagent's result is already a compact notification; an MCP
/// result gets a header line before its body.
pub fn create_background_task_message(result: &BackgroundToolResult) -> CustomMessage {
    let info = describe_background_tool(&result.tool_call);
    let details = json!({
        "subagentType": info.subagent_type,
        "isMcpTool": info.is_mcp_tool,
        "isError": result.is_error,
    });
    let content = if info.is_mcp_tool {
        let verb = if result.is_error {
            "failed"
        } else {
            "finished"
        };
        let summary = info
            .summary
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!(" ({s})"))
            .unwrap_or_default();
        let mut blocks = vec![Content::text(format!(
            "Background {}{summary} {verb}:",
            info.label
        ))];
        blocks.extend(result.result.content.iter().cloned());
        blocks
    } else {
        result.result.content.clone()
    };
    CustomMessage {
        custom_type: BACKGROUND_TASK_CUSTOM_TYPE.to_string(),
        content: content.into(),
        display: true,
        details: Some(details),
        timestamp: cortexcode_ai_types::now_ms(),
    }
}

fn user(content: Vec<Content>, timestamp: i64) -> Message {
    Message::User(UserMessage {
        content: content.into(),
        timestamp,
    })
}

/// `convertToLlm()`: harness messages become user messages; `!!` bash runs are dropped.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::BashExecution(b) => {
                if b.exclude_from_context == Some(true) {
                    None
                } else {
                    Some(user(
                        vec![Content::text(bash_execution_to_text(b))],
                        b.timestamp,
                    ))
                }
            }
            // A string becomes one text block, as in TS.
            AgentMessage::Custom(c) => Some(user(c.content.clone().into_blocks(), c.timestamp)),
            AgentMessage::BranchSummary(b) => Some(user(
                vec![Content::text(format!(
                    "{BRANCH_SUMMARY_PREFIX}{}{BRANCH_SUMMARY_SUFFIX}",
                    b.summary
                ))],
                b.timestamp,
            )),
            AgentMessage::CompactionSummary(c) => Some(user(
                vec![Content::text(format!(
                    "{COMPACTION_SUMMARY_PREFIX}{}{COMPACTION_SUMMARY_SUFFIX}",
                    c.summary
                ))],
                c.timestamp,
            )),
            other => other.extract_message(),
        })
        .collect()
}

/// Convert a collection of `AgentMessage` values into the LLM-native `Message`
/// format, dropping any custom messages.
pub fn to_llm_messages(
    messages: Vec<AgentMessage>,
) -> Result<Vec<Message>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(messages
        .into_iter()
        .filter_map(|msg| msg.extract_message())
        .collect())
}

/// Convert a collection of `AgentMessage` values into a single text block,
/// concatenating textual content from each message.
pub fn to_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .filter_map(|msg| msg.extract_message())
        .map(|m| match m {
            Message::User(u) => content_to_text(&u.content.blocks()),
            Message::Assistant(a) => content_to_text(&a.content),
            Message::ToolResult(t) => content_to_text(&t.content),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract plain text from a slice of `Content`.
fn content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a user `AgentMessage` from a plain text prompt.
pub fn user_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::user_text(text)
}

/// Build an assistant `AgentMessage` from an `AssistantMessage`.
pub fn assistant_message(message: AssistantMessage) -> AgentMessage {
    AgentMessage::from_message(Message::Assistant(message))
}

/// Return only the messages whose standard role matches the given role name.
///
/// Supported roles are `"user"`, `"assistant"`, and `"tool"`.
pub fn filter_by_role(messages: &[AgentMessage], role: &str) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter(|msg| {
            matches!(
                (msg, role),
                (AgentMessage::User(_), "user")
                    | (AgentMessage::Assistant(_), "assistant")
                    | (AgentMessage::ToolResult(_), "tool")
            )
        })
        .cloned()
        .collect()
}

/// Count the number of tokens approximately by splitting on whitespace.
///
/// This is a fast, provider-agnostic approximation. Production code should
/// use a provider-specific tokenizer.
pub fn approximate_token_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Append a user message to an existing conversation.
pub fn append_user(messages: &mut Vec<AgentMessage>, text: impl Into<String>) {
    messages.push(user_message(text));
}

/// Append a tool result message to an existing conversation.
pub fn append_tool_result(
    messages: &mut Vec<AgentMessage>,
    tool_call_id: impl Into<String>,
    tool_name: impl Into<String>,
    content: impl Into<String>,
    is_error: bool,
) {
    messages.push(AgentMessage::from_message(Message::ToolResult(
        cortexcode_ai_types::ToolResultMessage {
            details: None,
            content: vec![Content::text(content)],
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            is_error,
            timestamp: cortexcode_ai_types::now_ms(),
        },
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_roundtrip() {
        let msg = user_message("hello");
        let std = msg.extract_message().unwrap();
        assert!(matches!(std, Message::User(_)));
    }

    #[test]
    fn test_to_text_concatenates() {
        let messages = vec![user_message("hello"), user_message("world")];
        let text = to_text(&messages);
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }

    #[test]
    fn test_filter_by_role() {
        let messages = vec![user_message("hi")];
        let user = filter_by_role(&messages, "user");
        assert_eq!(user.len(), 1);
        assert!(filter_by_role(&messages, "assistant").is_empty());
    }

    #[test]
    fn test_approximate_token_count() {
        assert_eq!(approximate_token_count("one two three"), 3);
    }

    #[test]
    fn test_to_llm_messages_drops_custom() {
        let mut messages = vec![user_message("hello")];
        messages.push(AgentMessage::Custom(
            cortexcode_agent_types::CustomMessage {
                custom_type: "note".into(),
                content: vec![Content::text("hidden")].into(),
                display: true,
                details: None,
                timestamp: 1,
            },
        ));
        let llm = to_llm_messages(messages).unwrap();
        assert_eq!(llm.len(), 1);
    }

    #[test]
    fn test_convert_to_llm_harness_roles() {
        use cortexcode_agent_types::{
            BranchSummaryMessage, CompactionSummaryMessage, CustomMessage,
        };
        let bash = |exclude: Option<bool>, exit_code: Option<i64>| {
            AgentMessage::BashExecution(BashExecutionMessage {
                command: "ls".into(),
                output: String::new(),
                exit_code,
                cancelled: false,
                truncated: false,
                full_output_path: None,
                timestamp: 5,
                exclude_from_context: exclude,
            })
        };
        let messages = vec![
            bash(None, Some(2)),
            bash(Some(true), Some(0)),
            AgentMessage::Custom(CustomMessage {
                custom_type: BACKGROUND_TASK_CUSTOM_TYPE.into(),
                content: vec![Content::text("done")].into(),
                display: true,
                details: None,
                timestamp: 6,
            }),
            AgentMessage::BranchSummary(BranchSummaryMessage {
                summary: "S".into(),
                from_id: "abc".into(),
                timestamp: 7,
            }),
            AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: "C".into(),
                tokens_before: 10,
                tokens_after: None,
                timestamp: 8,
            }),
        ];
        let text_of = |m: &Message| match m {
            Message::User(u) => match &u.content.blocks()[0] {
                Content::Text(t) => t.text.clone(),
                _ => panic!(),
            },
            _ => panic!("expected user message"),
        };
        let llm = convert_to_llm(&messages);
        assert_eq!(llm.len(), 4, "!! bash execution is excluded");
        assert_eq!(
            text_of(&llm[0]),
            "Ran `ls`\n(no output)\n\nCommand exited with code 2"
        );
        assert_eq!(text_of(&llm[1]), "done");
        assert_eq!(
            text_of(&llm[2]),
            format!("{BRANCH_SUMMARY_PREFIX}S{BRANCH_SUMMARY_SUFFIX}")
        );
        assert_eq!(
            text_of(&llm[3]),
            format!("{COMPACTION_SUMMARY_PREFIX}C{COMPACTION_SUMMARY_SUFFIX}")
        );
    }
}

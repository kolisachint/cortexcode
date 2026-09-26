//! Helpers for converting and inspecting agent messages.

use cortexcode_agent_types::{AgentMessage, BashExecutionMessage};
use cortexcode_ai_types::{AssistantMessage, Content, Message, UserMessage};

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

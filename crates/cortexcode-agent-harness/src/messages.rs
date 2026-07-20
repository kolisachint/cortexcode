//! Helpers for converting and inspecting agent messages.

use cortexcode_agent_types::{AgentMessage, AgentMessageInner};
use cortexcode_ai_types::{AssistantMessage, Content, Message, TextContent, UserMessage};

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
            Message::User(u) => content_to_text(&u.content),
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
    AgentMessage::from_message(Message::User(UserMessage {
        content: vec![Content::Text(TextContent {
            text: text.into(),
            cache_control: None,
        })],
        timestamp: None,
    }))
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
                (&msg.inner, role),
                (AgentMessageInner::Standard(Message::User(_)), "user")
                    | (AgentMessageInner::Standard(Message::Assistant(_)), "assistant")
                    | (AgentMessageInner::Standard(Message::ToolResult(_)), "tool")
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
            content: vec![Content::Text(TextContent {
                text: content.into(),
                cache_control: None,
            })],
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            is_error,
            timestamp: None,
        },
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_ai_types::TextContent;

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
        messages.push(AgentMessage::new(AgentMessageInner::Custom {
            role: "system".into(),
            content: vec![Content::Text(TextContent {
                text: "hidden".into(),
                cache_control: None,
            })],
            timestamp: None,
        }));
        let llm = to_llm_messages(messages).unwrap();
        assert_eq!(llm.len(), 1);
    }
}

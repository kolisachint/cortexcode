//! Context window compaction and summarization for agent conversations.
//!
//! Mirrors the `harness/compaction/` directory from the TypeScript
//! `@kolisachint/hoocode-agent-core` package.

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, Message, TextContent};

/// Approximate the number of tokens in a conversation using whitespace
/// splitting. This is a fast, provider-agnostic estimate.
pub fn approximate_token_count(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .filter_map(|msg| msg.extract_message())
        .map(|msg| match msg {
            Message::User(u) => content_text(&u.content),
            Message::Assistant(a) => content_text(&a.content),
            Message::ToolResult(t) => content_text(&t.content),
        })
        .map(|text| text.split_whitespace().count())
        .sum()
}

fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A strategy for compacting a message list.
pub trait CompactionStrategy {
    /// Compact `messages` in place, returning the compacted list.
    fn compact(&self, messages: Vec<AgentMessage>) -> Vec<AgentMessage>;
}

/// Keep only the most recent `keep_count` messages.
#[derive(Debug, Clone, Copy)]
pub struct KeepRecentStrategy {
    /// Number of recent messages to retain.
    pub keep_count: usize,
}

impl CompactionStrategy for KeepRecentStrategy {
    fn compact(&self, mut messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        if messages.len() <= self.keep_count {
            messages
        } else {
            messages.split_off(messages.len().saturating_sub(self.keep_count))
        }
    }
}

/// Summarize older messages into a single summary message once a token budget
/// is exceeded.
#[derive(Debug, Clone)]
pub struct SummaryStrategy {
    /// Maximum approximate tokens before summarizing older messages.
    pub max_tokens: usize,
    /// Number of recent messages to always keep verbatim.
    pub keep_recent: usize,
}

impl Default for SummaryStrategy {
    fn default() -> Self {
        Self {
            max_tokens: 8_000,
            keep_recent: 4,
        }
    }
}

impl CompactionStrategy for SummaryStrategy {
    fn compact(&self, messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        if messages.len() <= self.keep_recent
            || approximate_token_count(&messages) <= self.max_tokens
        {
            return messages;
        }

        let split_at = messages.len().saturating_sub(self.keep_recent);
        let (older, recent) = messages.split_at(split_at);

        let summary_text = summarize_messages(older);
        let summary = AgentMessage::from_message(Message::User(
            cortexcode_ai_types::UserMessage {
                content: vec![Content::Text(TextContent {
                    text: format!(
                        "[Summary of earlier conversation]\n{}",
                        summary_text
                    ),
                    cache_control: None,
                })],
                timestamp: None,
            },
        ));

        let mut compacted = Vec::with_capacity(recent.len() + 1);
        compacted.push(summary);
        compacted.extend_from_slice(recent);
        compacted
    }
}

/// Produce a simple text summary of a slice of messages by concatenating
/// their text content. Production implementations may call an LLM here.
pub fn summarize_messages(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .filter_map(|msg| msg.extract_message())
        .map(|msg| match msg {
            Message::User(u) => format!("User: {}", content_text(&u.content)),
            Message::Assistant(a) => format!("Assistant: {}", content_text(&a.content)),
            Message::ToolResult(t) => format!("Tool {}: {}", t.tool_name, content_text(&t.content)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(cortexcode_ai_types::UserMessage {
            content: vec![Content::Text(TextContent {
                text: text.into(),
                cache_control: None,
            })],
            timestamp: None,
        }))
    }

    #[test]
    fn test_keep_recent() {
        let strategy = KeepRecentStrategy { keep_count: 2 };
        let messages: Vec<_> = (0..5).map(|i| user_msg(&format!("msg {}", i))).collect();
        let compacted = strategy.compact(messages);
        assert_eq!(compacted.len(), 2);
    }

    #[test]
    fn test_summary_strategy_under_budget() {
        let strategy = SummaryStrategy {
            max_tokens: 10_000,
            keep_recent: 2,
        };
        let messages = vec![user_msg("hello"), user_msg("world")];
        let compacted = strategy.compact(messages.clone());
        assert_eq!(compacted.len(), 2);
    }

    #[test]
    fn test_summary_strategy_compacts() {
        let strategy = SummaryStrategy {
            max_tokens: 1,
            keep_recent: 1,
        };
        let messages = vec![user_msg("one two"), user_msg("three four")];
        let compacted = strategy.compact(messages);
        assert_eq!(compacted.len(), 2); // summary + 1 recent
        assert!(approximate_token_count(&compacted) <= 10);
    }

    #[test]
    fn test_summarize_messages() {
        let messages = vec![user_msg("hello"), user_msg("world")];
        let summary = summarize_messages(&messages);
        assert!(summary.contains("User: hello"));
        assert!(summary.contains("User: world"));
    }
}

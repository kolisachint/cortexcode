//! Build the resolved conversation context from a branch of session entries.
//!
//! The context is what gets sent to the LLM: it follows the path from the root
//! to the current leaf, applies compaction boundaries, and converts special
//! entries (custom messages, branch summaries) into `AgentMessage`s.

use crate::entry::{CustomMessageContent, FileEntry};
use cortexcode_agent_types::{AgentMessage, AgentMessageInner};
use cortexcode_ai_types::{Content, TextContent};

/// Resolved model reference extracted from the session branch.
#[derive(Debug, Clone)]
pub struct ModelRef {
    /// Provider identifier.
    pub provider: String,
    /// Model identifier.
    pub model_id: String,
}

/// Resolved session context for the LLM.
#[derive(Debug, Clone)]
pub struct SessionContext {
    /// Messages that should be included in the LLM request.
    pub messages: Vec<AgentMessage>,
    /// Effective thinking level for the branch.
    pub thinking_level: String,
    /// Effective model for the branch.
    pub model: Option<ModelRef>,
}

const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";

const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
const BRANCH_SUMMARY_SUFFIX: &str = "\n</summary>";

/// Build a `SessionContext` from a root-to-leaf list of entries.
pub fn build_session_context(entries: &[FileEntry]) -> SessionContext {
    let mut thinking_level = "off".to_string();
    let mut model: Option<ModelRef> = None;
    let mut compaction: Option<&FileEntry> = None;

    for entry in entries {
        match entry {
            FileEntry::ThinkingLevelChange {
                thinking_level: level,
                ..
            } => {
                thinking_level = level.clone();
            }
            FileEntry::ModelChange {
                provider, model_id, ..
            } => {
                model = Some(ModelRef {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                });
            }
            FileEntry::Compaction { .. } => {
                compaction = Some(entry);
            }
            _ => {}
        }
    }

    let mut messages = Vec::new();

    if let Some(comp) = compaction {
        if let FileEntry::Compaction {
            summary,
            timestamp,
            first_kept_entry_id,
            ..
        } = comp
        {
            messages.push(make_custom_message(
                "compactionSummary",
                format!("{COMPACTION_SUMMARY_PREFIX}{summary}{COMPACTION_SUMMARY_SUFFIX}"),
                timestamp,
            ));

            let compaction_id = comp.id().unwrap_or_default();
            let first_kept_id = first_kept_entry_id;

            let mut found_first_kept = false;
            for entry in entries {
                if entry.id() == Some(compaction_id) {
                    break;
                }
                if entry.id() == Some(first_kept_id) {
                    found_first_kept = true;
                }
                if found_first_kept {
                    append_message(entry, &mut messages);
                }
            }

            let mut after_compaction = false;
            for entry in entries {
                if entry.id() == Some(compaction_id) {
                    after_compaction = true;
                    continue;
                }
                if after_compaction {
                    append_message(entry, &mut messages);
                }
            }
        }
    } else {
        for entry in entries {
            append_message(entry, &mut messages);
        }
    }

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}

fn append_message(entry: &FileEntry, messages: &mut Vec<AgentMessage>) {
    match entry {
        FileEntry::Message { message, .. } => {
            messages.push(message.clone());
        }
        FileEntry::CustomMessage {
            custom_type,
            content,
            timestamp,
            details,
            ..
        } => {
            let content_blocks = match content {
                CustomMessageContent::Text(text) => {
                    vec![Content::Text(TextContent {
                        text: text.clone(),
                        cache_control: None,
                    })]
                }
                CustomMessageContent::Blocks(blocks) => blocks.clone(),
            };
            let mut extra = Vec::new();
            if let Some(details) = details {
                extra.push(Content::Text(TextContent {
                    text: format!(
                        "<details>{}</details>",
                        serde_json::to_string(details).unwrap_or_default()
                    ),
                    cache_control: None,
                }));
            }
            let mut all = content_blocks;
            all.extend(extra);
            messages.push(AgentMessage::new(AgentMessageInner::Custom {
                role: custom_type.clone(),
                content: all,
                timestamp: parse_timestamp(timestamp),
            }));
        }
        FileEntry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } => {
            messages.push(make_custom_message(
                "branchSummary",
                format!(
                    "{BRANCH_SUMMARY_PREFIX}{summary}\n(from branch {from_id}){BRANCH_SUMMARY_SUFFIX}"
                ),
                timestamp,
            ));
        }
        _ => {}
    }
}

fn make_custom_message(role: &str, text: String, timestamp: &str) -> AgentMessage {
    AgentMessage::new(AgentMessageInner::Custom {
        role: role.to_string(),
        content: vec![Content::Text(TextContent {
            text,
            cache_control: None,
        })],
        timestamp: parse_timestamp(timestamp),
    })
}

fn parse_timestamp(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc).timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::FileEntry;
    use cortexcode_agent_types::AgentMessage;
    use cortexcode_ai_types::{Message, UserMessage};

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text: text.into(),
                cache_control: None,
            })],
            timestamp: None,
        }))
    }

    #[test]
    fn context_collects_messages() {
        let entries = vec![
            FileEntry::Message {
                id: "m1".into(),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                message: user_message("hello"),
            },
            FileEntry::Message {
                id: "m2".into(),
                parent_id: Some("m1".into()),
                timestamp: "2026-01-01T00:00:01.000Z".into(),
                message: user_message("world"),
            },
        ];
        let ctx = build_session_context(&entries);
        assert_eq!(ctx.messages.len(), 2);
        assert_eq!(ctx.thinking_level, "off");
        assert!(ctx.model.is_none());
    }

    #[test]
    fn context_applies_compaction() {
        let entries = vec![
            FileEntry::Message {
                id: "m1".into(),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                message: user_message("old"),
            },
            FileEntry::Message {
                id: "m2".into(),
                parent_id: Some("m1".into()),
                timestamp: "2026-01-01T00:00:01.000Z".into(),
                message: user_message("keep"),
            },
            FileEntry::Compaction {
                id: "c1".into(),
                parent_id: Some("m2".into()),
                timestamp: "2026-01-01T00:00:02.000Z".into(),
                summary: "summary".into(),
                first_kept_entry_id: "m2".into(),
                tokens_before: 100,
                tokens_after: None,
                details: None,
                from_hook: None,
            },
            FileEntry::Message {
                id: "m3".into(),
                parent_id: Some("c1".into()),
                timestamp: "2026-01-01T00:00:03.000Z".into(),
                message: user_message("new"),
            },
        ];
        let ctx = build_session_context(&entries);
        assert_eq!(ctx.messages.len(), 3); // compaction summary + keep + new
    }
}

//! Build the resolved conversation context from a branch of session entries.
//!
//! The context is what gets sent to the LLM: it follows the path from the root
//! to the current leaf, applies compaction boundaries, and converts special
//! entries (custom messages, branch summaries) into `AgentMessage`s.

use crate::entry::FileEntry;
use cortexcode_agent_types::{
    AgentMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomMessage,
};

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

/// Build a `SessionContext` from a root-to-leaf list of entries.
///
/// Port of `buildSessionContext` in hoocode `packages/agent/src/harness/session/session.ts`:
/// settings (thinking level, model) come from the whole path; messages start at the
/// compaction summary (if any), then the kept entries before it, then everything after.
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
            FileEntry::Message {
                message: AgentMessage::Assistant(a),
                ..
            } => {
                model = Some(ModelRef {
                    provider: a.provider.clone(),
                    model_id: a.model.clone(),
                });
            }
            FileEntry::Compaction { .. } => {
                compaction = Some(entry);
            }
            _ => {}
        }
    }

    let mut messages = Vec::new();

    match compaction {
        Some(
            comp @ FileEntry::Compaction {
                summary,
                timestamp,
                first_kept_entry_id,
                tokens_before,
                tokens_after,
                ..
            },
        ) => {
            messages.push(AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: summary.clone(),
                tokens_before: *tokens_before,
                tokens_after: *tokens_after,
                timestamp: parse_timestamp(timestamp),
            }));
            let compaction_idx = entries
                .iter()
                .position(|e| std::ptr::eq(e, comp))
                .unwrap_or(entries.len());
            let mut found_first_kept = false;
            for entry in &entries[..compaction_idx] {
                if entry.id() == Some(first_kept_entry_id.as_str()) {
                    found_first_kept = true;
                }
                if found_first_kept {
                    append_message(entry, &mut messages);
                }
            }
            for entry in entries.iter().skip(compaction_idx + 1) {
                append_message(entry, &mut messages);
            }
        }
        _ => {
            for entry in entries {
                append_message(entry, &mut messages);
            }
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
        FileEntry::Message { message, .. } => messages.push(message.clone()),
        // createCustomMessage()
        FileEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            timestamp,
            ..
        } => messages.push(AgentMessage::Custom(CustomMessage {
            custom_type: custom_type.clone(),
            content: content.clone(),
            display: *display,
            details: details.clone(),
            timestamp: parse_timestamp(timestamp),
        })),
        // createBranchSummaryMessage(), only for non-empty summaries
        FileEntry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } if !summary.is_empty() => {
            messages.push(AgentMessage::BranchSummary(BranchSummaryMessage {
                summary: summary.clone(),
                from_id: from_id.clone(),
                timestamp: parse_timestamp(timestamp),
            }))
        }
        _ => {}
    }
}

/// `new Date(timestamp).getTime()`; invalid dates become 0 (NaN in TS).
fn parse_timestamp(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::FileEntry;
    use cortexcode_agent_types::AgentMessage;
    use cortexcode_ai_types::{Content, Message, UserMessage};

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::text(text)].into(),
            timestamp: 0,
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
        match &ctx.messages[0] {
            AgentMessage::CompactionSummary(c) => {
                assert_eq!(c.summary, "summary");
                assert_eq!(c.tokens_before, 100);
                assert_eq!(c.timestamp, 1767225602000);
            }
            other => panic!("expected compaction summary first, got {other:?}"),
        }
    }

    #[test]
    fn context_model_from_assistant_and_branch_summary_messages() {
        let assistant = AgentMessage::Assistant(cortexcode_ai_types::AssistantMessage {
            provider: "anthropic".into(),
            model: "claude-x".into(),
            ..Default::default()
        });
        let entries = vec![
            FileEntry::Message {
                id: "m1".into(),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                message: assistant,
            },
            FileEntry::BranchSummary {
                id: "b1".into(),
                parent_id: Some("m1".into()),
                timestamp: "2026-01-01T00:00:01.000Z".into(),
                from_id: "x".into(),
                summary: "left branch".into(),
                details: None,
                from_hook: None,
            },
            FileEntry::BranchSummary {
                id: "b2".into(),
                parent_id: Some("b1".into()),
                timestamp: "2026-01-01T00:00:02.000Z".into(),
                from_id: "y".into(),
                summary: String::new(),
                details: None,
                from_hook: None,
            },
        ];
        let ctx = build_session_context(&entries);
        let m = ctx.model.expect("model from assistant message");
        assert_eq!(
            (m.provider.as_str(), m.model_id.as_str()),
            ("anthropic", "claude-x")
        );
        assert_eq!(ctx.messages.len(), 2, "empty branch summaries are skipped");
        assert!(matches!(&ctx.messages[1], AgentMessage::BranchSummary(b) if b.from_id == "x"));
    }
}

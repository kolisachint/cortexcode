//! Session entry types for the JSONL session tree.
//!
//! Each line in a session file is a JSON object with a `type` discriminator. The
//! first line is always a `session` header; the remaining lines are tree entries
//! that form an append-only conversation history.

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::Content;
use serde::{Deserialize, Serialize};

/// Current session file format version.
pub const CURRENT_SESSION_VERSION: u32 = 3;

/// Session header written as the first line of every `.jsonl` session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// Format version. Older sessions may omit this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Unique session identifier.
    pub id: String,
    /// ISO 8601 timestamp when the session was created.
    pub timestamp: String,
    /// Working directory captured when the session started.
    pub cwd: String,
    /// Path to the parent session when this session was forked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}

/// Content of a `custom_message` entry, mirroring the TypeScript
/// `string | (TextContent | ImageContent)[]` union.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CustomMessageContent {
    /// Plain text content.
    Text(String),
    /// Structured content blocks.
    Blocks(Vec<Content>),
}

impl From<String> for CustomMessageContent {
    fn from(value: String) -> Self {
        CustomMessageContent::Text(value)
    }
}

impl From<&str> for CustomMessageContent {
    fn from(value: &str) -> Self {
        CustomMessageContent::Text(value.to_string())
    }
}

impl From<Vec<Content>> for CustomMessageContent {
    fn from(value: Vec<Content>) -> Self {
        CustomMessageContent::Blocks(value)
    }
}

/// A single line in a session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FileEntry {
    /// Session header. Always appears as the first line.
    #[serde(rename = "session")]
    Session(Header),

    /// Conversation message.
    #[serde(rename = "message")]
    Message {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        message: AgentMessage,
    },

    /// Thinking level change.
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        thinking_level: String,
    },

    /// Model change.
    #[serde(rename = "model_change")]
    ModelChange {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        provider: String,
        model_id: String,
    },

    /// Compaction boundary.
    #[serde(rename = "compaction")]
    Compaction {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens_after: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },

    /// Branch summary injected when returning to an earlier point.
    #[serde(rename = "branch_summary")]
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        from_id: String,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from_hook: Option<bool>,
    },

    /// Custom extension entry.
    #[serde(rename = "custom")]
    Custom {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },

    /// Custom message that participates in LLM context.
    #[serde(rename = "custom_message")]
    CustomMessage {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        custom_type: String,
        content: CustomMessageContent,
        display: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },

    /// Label change for an entry.
    #[serde(rename = "label")]
    Label {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        target_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },

    /// Session metadata update (e.g. display name).
    #[serde(rename = "session_info")]
    SessionInfo {
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl FileEntry {
    /// Entry id, if this is a tree entry.
    pub fn id(&self) -> Option<&str> {
        match self {
            FileEntry::Session(_) => None,
            FileEntry::Message { id, .. }
            | FileEntry::ThinkingLevelChange { id, .. }
            | FileEntry::ModelChange { id, .. }
            | FileEntry::Compaction { id, .. }
            | FileEntry::BranchSummary { id, .. }
            | FileEntry::Custom { id, .. }
            | FileEntry::CustomMessage { id, .. }
            | FileEntry::Label { id, .. }
            | FileEntry::SessionInfo { id, .. } => Some(id),
        }
    }

    /// Parent entry id, if this is a tree entry.
    pub fn parent_id(&self) -> Option<&str> {
        match self {
            FileEntry::Session(_) => None,
            FileEntry::Message { parent_id, .. }
            | FileEntry::ThinkingLevelChange { parent_id, .. }
            | FileEntry::ModelChange { parent_id, .. }
            | FileEntry::Compaction { parent_id, .. }
            | FileEntry::BranchSummary { parent_id, .. }
            | FileEntry::Custom { parent_id, .. }
            | FileEntry::CustomMessage { parent_id, .. }
            | FileEntry::Label { parent_id, .. }
            | FileEntry::SessionInfo { parent_id, .. } => parent_id.as_deref(),
        }
    }

    /// Entry timestamp, if this is a tree entry.
    pub fn timestamp(&self) -> Option<&str> {
        match self {
            FileEntry::Session(Header { timestamp, .. }) => Some(timestamp),
            FileEntry::Message { timestamp, .. }
            | FileEntry::ThinkingLevelChange { timestamp, .. }
            | FileEntry::ModelChange { timestamp, .. }
            | FileEntry::Compaction { timestamp, .. }
            | FileEntry::BranchSummary { timestamp, .. }
            | FileEntry::Custom { timestamp, .. }
            | FileEntry::CustomMessage { timestamp, .. }
            | FileEntry::Label { timestamp, .. }
            | FileEntry::SessionInfo { timestamp, .. } => Some(timestamp),
        }
    }

    /// Returns true if this entry is a label entry.
    pub fn is_label(&self) -> bool {
        matches!(self, FileEntry::Label { .. })
    }

    /// Returns true if this entry is a message entry.
    pub fn is_message(&self) -> bool {
        matches!(self, FileEntry::Message { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_agent_types::AgentMessage;
    use cortexcode_ai_types::{Message, UserMessage};

    #[test]
    fn header_roundtrips() {
        let header = Header {
            version: Some(CURRENT_SESSION_VERSION),
            id: "sess-1".into(),
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            cwd: "/tmp".into(),
            parent_session: None,
        };
        let line = serde_json::to_string(&FileEntry::Session(header.clone())).unwrap();
        let parsed: FileEntry = serde_json::from_str(&line).unwrap();
        assert!(matches!(parsed, FileEntry::Session(h) if h.id == header.id));
    }

    #[test]
    fn message_entry_roundtrips() {
        let msg = AgentMessage::from_message(Message::User(UserMessage {
            content: vec![],
            timestamp: None,
        }));
        let entry = FileEntry::Message {
            id: "m1".into(),
            parent_id: None,
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            message: msg,
        };
        let line = serde_json::to_string(&entry).unwrap();
        let parsed: FileEntry = serde_json::from_str(&line).unwrap();
        assert!(matches!(parsed, FileEntry::Message { id, .. } if id == "m1"));
    }
}

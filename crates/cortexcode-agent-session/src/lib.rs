//! Session persistence for agent conversations.
//!
//! Mirrors the `harness/session/` directory from the TypeScript
//! `@kolisachint/hoocode-agent-core` package.
//!
//! Because `AgentTool` contains execution callbacks, `AgentContext` itself is
//! not serializable. Instead we persist the serializable parts of a session
//! (`system_prompt` and `messages`) and rebuild the context on load by
//! supplying the tool set.

use cortexcode_agent_types::{AgentContext, AgentMessage, AgentTool};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod store;

pub use store::*;

/// Metadata stored alongside a persisted session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// When the session was created (ISO 8601).
    pub created_at: String,
    /// When the session was last updated (ISO 8601).
    pub updated_at: String,
    /// Optional human-readable title.
    pub title: Option<String>,
}

/// Serializable representation of a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    /// Unique session identifier.
    pub id: String,
    /// Session metadata.
    pub metadata: SessionMetadata,
    /// System prompt used for the session.
    pub system_prompt: String,
    /// Conversation messages.
    pub messages: Vec<AgentMessage>,
}

impl SessionData {
    /// Create new session data from an `AgentContext`.
    pub fn from_context(id: impl Into<String>, context: &AgentContext) -> Self {
        let now = now_iso8601();
        Self {
            id: id.into(),
            metadata: SessionMetadata {
                created_at: now.clone(),
                updated_at: now,
                title: None,
            },
            system_prompt: context.system_prompt.clone(),
            messages: context.messages.clone(),
        }
    }

    /// Set the session title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.metadata.title = Some(title.into());
        self
    }

    /// Rebuild an `AgentContext` using the supplied tools.
    pub fn to_context(&self, tools: Vec<AgentTool>) -> AgentContext {
        AgentContext::new(self.system_prompt.clone(), self.messages.clone(), tools)
    }
}

fn now_iso8601() -> String {
    // Simple ISO 8601 approximation suitable for sorting.
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", secs)
}

/// Return a default session directory under the user's home directory.
///
/// Uses `$HOME/.cortexcode/sessions` on Unix-like systems. Falls back to
/// `./.cortexcode/sessions` if `HOME` is not set.
pub fn default_session_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(".cortexcode").join("sessions"))
        .unwrap_or_else(|| PathBuf::from(".cortexcode/sessions"))
}

/// Ensure the parent directory of `path` exists.
pub fn ensure_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_agent_types::AgentTools;

    #[test]
    fn test_session_data_from_context() {
        let ctx = AgentContext::new("system".into(), vec![], vec![]);
        let data = SessionData::from_context("test", &ctx);
        assert_eq!(data.id, "test");
        assert_eq!(data.system_prompt, "system");
        assert!(data.metadata.title.is_none());
    }

    #[test]
    fn test_session_data_roundtrip_context() {
        let ctx = AgentContext::new("system".into(), vec![], vec![]);
        let data = SessionData::from_context("test", &ctx).with_title("My Session");
        let rebuilt = data.to_context(vec![]);
        assert_eq!(rebuilt.system_prompt, "system");
        assert_eq!(rebuilt.messages.len(), 0);
        assert_eq!(data.metadata.title.as_deref(), Some("My Session"));
    }
}

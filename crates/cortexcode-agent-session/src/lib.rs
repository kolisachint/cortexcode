//! Session trees for cortex agents.
//!
//! Port of hoocode `packages/agent/src/harness/session/` and the session types
//! in `harness/types.ts`: the JSONL entry format ([`entry`]),
//! `buildSessionContext` ([`context`]), the storage trait with in-memory and
//! JSONL backends ([`storage`]), `Session` ([`session`]) and the repositories
//! ([`repo`]). `cortexcode-code-session` (hoocode `session-manager.ts`) builds
//! on these types.

pub mod context;
pub mod entry;
pub mod repo;
pub mod session;
pub mod storage;

pub use context::{build_session_context, ModelRef, SessionContext};
pub use entry::{CustomMessageContent, FileEntry, Header, CURRENT_SESSION_VERSION};
pub use repo::{
    get_entries_to_fork, ForkOptions, ForkPosition, InMemorySessionRepo, JsonlSessionCreateOptions,
    JsonlSessionRepo, SharedSession,
};
pub use session::{BranchSummaryInput, Session};
pub use storage::{
    entry_type_of, load_jsonl_session_metadata, InMemorySessionStorage, JsonlSessionMetadata,
    JsonlSessionStorage, SessionMetadata, SessionStorage, SessionTreeEntry,
};

/// Errors from session storage and repositories. Messages match hoocode's.
#[derive(Debug)]
pub enum SessionError {
    /// `Entry <id> not found`.
    EntryNotFound(String),
    /// `Session not found: <id or path>`.
    SessionNotFound(String),
    /// A malformed session file or an invalid request (message as in TS).
    Invalid(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::EntryNotFound(id) => write!(f, "Entry {id} not found"),
            SessionError::SessionNotFound(what) => write!(f, "Session not found: {what}"),
            SessionError::Invalid(message) => f.write_str(message),
            SessionError::Io(e) => write!(f, "{e}"),
            SessionError::Json(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        SessionError::Io(e)
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        SessionError::Json(e)
    }
}

/// `createSessionId()`: a UUIDv7.
pub fn create_session_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// `createTimestamp()` / `new Date().toISOString()`.
pub fn create_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `generateEntryId`: the first 8 characters of a random UUID that `exists`
/// does not report as taken; a full UUID after 100 collisions.
pub fn generate_entry_id(exists: impl Fn(&str) -> bool) -> String {
    for _ in 0..100 {
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        if !exists(&id) {
            return id;
        }
    }
    uuid::Uuid::new_v4().to_string()
}

/// `encodeCwd`: `--<cwd without one leading separator, with / \ : as ->--`.
pub fn encode_cwd(cwd: &str) -> String {
    let trimmed = cwd
        .strip_prefix(['/', '\\'])
        .unwrap_or(cwd)
        .replace(['/', '\\', ':'], "-");
    format!("--{trimmed}--")
}

#[cfg(test)]
pub(crate) mod test_utils {
    //! `session-test-utils.ts`.

    use cortexcode_agent_types::AgentMessage;
    use cortexcode_ai_types::{AssistantMessage, Content, Message, StopReason, UserMessage};

    pub fn user_message(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::text(text)],
            timestamp: cortexcode_ai_types::now_ms(),
        }))
    }

    pub fn assistant_message(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::Assistant(AssistantMessage {
            content: vec![Content::text(text)],
            api: "anthropic-messages".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            stop_reason: StopReason::Stop,
            timestamp: cortexcode_ai_types::now_ms(),
            ..Default::default()
        }))
    }

    /// A temporary directory removed on drop (`createTempDir` + `afterEach`).
    pub struct TempDir(std::path::PathBuf);

    impl TempDir {
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn temp_dir() -> TempDir {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cortex-agent-session-{}-{}-{}",
            std::process::id(),
            cortexcode_ai_types::now_ms(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_cwd_matches_hoocode() {
        assert_eq!(encode_cwd("/tmp/my-project"), "--tmp-my-project--");
        assert_eq!(encode_cwd("C:\\Users\\me"), "--C--Users-me--");
        // Only one leading separator is dropped (`/^[/\\]/`).
        assert_eq!(encode_cwd("//srv/x"), "---srv-x--");
        assert_eq!(encode_cwd("rel/dir"), "--rel-dir--");
    }

    #[test]
    fn ids_and_timestamps_have_hoocode_shapes() {
        let id = generate_entry_id(|_| false);
        assert_eq!(id.len(), 8);
        assert_eq!(generate_entry_id(|_| true).len(), 36);
        let ts = create_timestamp();
        assert!(ts.ends_with('Z') && ts.len() == 24, "{ts}");
        assert_eq!(create_session_id().len(), 36);
    }
}

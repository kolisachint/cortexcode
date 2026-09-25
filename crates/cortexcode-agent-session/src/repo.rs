//! Session repositories: create, open, list, delete and fork sessions.
//!
//! Port of hoocode `packages/agent/src/harness/session/repo/{shared,memory,jsonl}.ts`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cortexcode_agent_types::AgentMessage;

use crate::entry::FileEntry;
use crate::session::Session;
use crate::storage::{
    load_jsonl_session_metadata, InMemorySessionStorage, JsonlSessionMetadata, JsonlSessionStorage,
    SessionMetadata, SessionStorage, SessionTreeEntry,
};
use crate::{create_session_id, create_timestamp, encode_cwd, SessionError};

/// Where a fork ends relative to `entry_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForkPosition {
    /// Up to the parent of the entry, which must be a user message (default).
    #[default]
    Before,
    /// Up to and including the entry.
    At,
}

/// `SessionForkOptions`.
#[derive(Debug, Clone, Default)]
pub struct ForkOptions {
    pub entry_id: Option<String>,
    pub position: ForkPosition,
    pub id: Option<String>,
}

/// `getEntriesToFork`: all entries, or the path to the fork point.
pub fn get_entries_to_fork(
    storage: &impl SessionStorage,
    entry_id: Option<&str>,
    position: ForkPosition,
) -> Result<Vec<SessionTreeEntry>, SessionError> {
    let Some(entry_id) = entry_id else {
        return Ok(storage.entries());
    };
    let target = storage
        .entry(entry_id)
        .ok_or_else(|| SessionError::EntryNotFound(entry_id.to_string()))?;
    let effective_leaf = match position {
        ForkPosition::At => target.id().map(str::to_string),
        ForkPosition::Before => match target {
            FileEntry::Message {
                message: AgentMessage::User(_),
                parent_id,
                ..
            } => parent_id.clone(),
            _ => {
                return Err(SessionError::Invalid(format!(
                    "Entry {entry_id} is not a user message"
                )))
            }
        },
    };
    Ok(storage.path_to_root(effective_leaf.as_deref()))
}

// ---------------------------------------------------------------------------
// InMemorySessionRepo
// ---------------------------------------------------------------------------

/// A session shared between the repo and its callers (`open` returns the
/// same session that `create` did, as in TypeScript).
pub type SharedSession<S> = Arc<Mutex<Session<S>>>;

/// `InMemorySessionRepo`.
#[derive(Default)]
pub struct InMemorySessionRepo {
    /// Insertion-ordered, like the TypeScript `Map`.
    sessions: Mutex<Vec<(String, SharedSession<InMemorySessionStorage>)>>,
}

impl InMemorySessionRepo {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(
        &self,
        session: Session<InMemorySessionStorage>,
    ) -> SharedSession<InMemorySessionStorage> {
        let id = session.metadata().id;
        let shared = Arc::new(Mutex::new(session));
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|(existing, _)| *existing != id);
        sessions.push((id, shared.clone()));
        shared
    }

    /// `create({ id? })`.
    pub fn create(&self, id: Option<&str>) -> SharedSession<InMemorySessionStorage> {
        let metadata = SessionMetadata {
            id: id.map(str::to_string).unwrap_or_else(create_session_id),
            created_at: create_timestamp(),
        };
        let storage = InMemorySessionStorage::new(Vec::new(), None, Some(metadata))
            .expect("an empty tree has no leaf to validate");
        self.insert(Session::new(storage))
    }

    /// `open(metadata)`: `Session not found: <id>` when unknown.
    pub fn open(
        &self,
        metadata: &SessionMetadata,
    ) -> Result<SharedSession<InMemorySessionStorage>, SessionError> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == metadata.id)
            .map(|(_, s)| s.clone())
            .ok_or_else(|| SessionError::SessionNotFound(metadata.id.clone()))
    }

    pub fn list(&self) -> Vec<SessionMetadata> {
        self.sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(_, s)| s.lock().unwrap().metadata())
            .collect()
    }

    pub fn delete(&self, metadata: &SessionMetadata) {
        self.sessions
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != metadata.id);
    }

    /// `fork(source, options)`: a new session holding the forked path.
    pub fn fork(
        &self,
        source: &SessionMetadata,
        options: ForkOptions,
    ) -> Result<SharedSession<InMemorySessionStorage>, SessionError> {
        let source = self.open(source)?;
        let entries = {
            let source = source.lock().unwrap();
            get_entries_to_fork(
                source.storage(),
                options.entry_id.as_deref(),
                options.position,
            )?
        };
        let metadata = SessionMetadata {
            id: options.id.unwrap_or_else(create_session_id),
            created_at: create_timestamp(),
        };
        let leaf = entries.last().and_then(|e| e.id()).map(str::to_string);
        let storage = InMemorySessionStorage::new(entries, leaf, Some(metadata))?;
        Ok(self.insert(Session::new(storage)))
    }
}

// ---------------------------------------------------------------------------
// JsonlSessionRepo
// ---------------------------------------------------------------------------

/// `JsonlSessionCreateOptions`.
#[derive(Debug, Clone, Default)]
pub struct JsonlSessionCreateOptions {
    pub cwd: String,
    pub id: Option<String>,
    pub parent_session_path: Option<String>,
}

/// `JsonlSessionRepo`: one JSONL file per session under
/// `<sessionsRoot>/--<encoded cwd>--/<timestamp>_<id>.jsonl`.
#[derive(Debug, Clone)]
pub struct JsonlSessionRepo {
    sessions_root: PathBuf,
}

impl JsonlSessionRepo {
    pub fn new(sessions_root: impl AsRef<Path>) -> Self {
        let root = sessions_root.as_ref();
        Self {
            sessions_root: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()),
        }
    }

    fn session_dir(&self, cwd: &str) -> PathBuf {
        self.sessions_root.join(encode_cwd(cwd))
    }

    fn session_file_path(&self, cwd: &str, session_id: &str, timestamp: &str) -> PathBuf {
        self.session_dir(cwd).join(format!(
            "{}_{session_id}.jsonl",
            timestamp.replace([':', '.'], "-")
        ))
    }

    fn new_storage(
        &self,
        options: &JsonlSessionCreateOptions,
        parent_session_path: Option<&str>,
    ) -> Result<JsonlSessionStorage, SessionError> {
        std::fs::create_dir_all(&self.sessions_root)?;
        let id = options.id.clone().unwrap_or_else(create_session_id);
        let path = self.session_file_path(&options.cwd, &id, &create_timestamp());
        JsonlSessionStorage::create(path, &options.cwd, &id, parent_session_path)
    }

    pub fn create(
        &self,
        options: JsonlSessionCreateOptions,
    ) -> Result<Session<JsonlSessionStorage>, SessionError> {
        let parent = options.parent_session_path.clone();
        Ok(Session::new(self.new_storage(&options, parent.as_deref())?))
    }

    /// `open(metadata)`: `Session not found: <path>` when the file is gone.
    pub fn open(
        &self,
        metadata: &JsonlSessionMetadata,
    ) -> Result<Session<JsonlSessionStorage>, SessionError> {
        if !metadata.path.exists() {
            return Err(SessionError::SessionNotFound(
                metadata.path.display().to_string(),
            ));
        }
        Ok(Session::new(JsonlSessionStorage::open(&metadata.path)?))
    }

    /// `list({ cwd? })`: valid session files, newest first. Unreadable or
    /// malformed files are skipped.
    pub fn list(&self, cwd: Option<&str>) -> Result<Vec<JsonlSessionMetadata>, SessionError> {
        let dirs = match cwd {
            Some(cwd) => vec![self.session_dir(cwd)],
            None => self.list_session_dirs()?,
        };
        let mut sessions = Vec::new();
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(&dir) else {
                continue;
            };
            for file in read.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "jsonl") {
                    if let Ok(metadata) = load_jsonl_session_metadata(&path) {
                        sessions.push(metadata);
                    }
                }
            }
        }
        let time = |m: &JsonlSessionMetadata| {
            chrono::DateTime::parse_from_rfc3339(&m.created_at)
                .map(|t| t.timestamp_millis())
                .unwrap_or(0)
        };
        sessions.sort_by_key(|m| std::cmp::Reverse(time(m)));
        Ok(sessions)
    }

    /// `delete(metadata)`: remove the file (a missing file is fine).
    pub fn delete(&self, metadata: &JsonlSessionMetadata) -> Result<(), SessionError> {
        match std::fs::remove_file(&metadata.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// `fork(source, options)`: a new file in `options.cwd` holding the forked
    /// path; its parent is `options.parent_session_path`, else the source file.
    pub fn fork(
        &self,
        source: &JsonlSessionMetadata,
        options: JsonlSessionCreateOptions,
        fork: ForkOptions,
    ) -> Result<Session<JsonlSessionStorage>, SessionError> {
        let source_session = self.open(source)?;
        let entries = get_entries_to_fork(
            source_session.storage(),
            fork.entry_id.as_deref(),
            fork.position,
        )?;
        let options = JsonlSessionCreateOptions {
            id: fork.id.or(options.id),
            ..options
        };
        let source_path = source.path.display().to_string();
        let parent = options.parent_session_path.clone().unwrap_or(source_path);
        let mut storage = self.new_storage(&options, Some(&parent))?;
        for entry in entries {
            storage.append_entry(entry)?;
        }
        Ok(Session::new(storage))
    }

    fn list_session_dirs(&self) -> Result<Vec<PathBuf>, SessionError> {
        let Ok(read) = std::fs::read_dir(&self.sessions_root) else {
            return Ok(Vec::new());
        };
        Ok(read
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{assistant_message, temp_dir, user_message};

    fn ids(entries: &[SessionTreeEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|e| e.id().map(str::to_string))
            .collect()
    }

    #[test]
    fn in_memory_opens_deletes_and_forks_by_metadata() {
        let repo = InMemorySessionRepo::new();
        let session = repo.create(Some("session-1"));
        let metadata = session.lock().unwrap().metadata();
        let (user1, assistant1, user2) = {
            let mut s = session.lock().unwrap();
            (
                s.append_message(user_message("one")).unwrap(),
                s.append_message(assistant_message("two")).unwrap(),
                s.append_message(user_message("three")).unwrap(),
            )
        };
        assert!(Arc::ptr_eq(&repo.open(&metadata).unwrap(), &session));
        let listed: Vec<String> = repo.list().into_iter().map(|m| m.id).collect();
        assert_eq!(listed, ["session-1"]);

        let fork = repo
            .fork(
                &metadata,
                ForkOptions {
                    entry_id: Some(user2.clone()),
                    id: Some("session-2".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            ids(&fork.lock().unwrap().entries()),
            [user1.clone(), assistant1.clone()]
        );
        let full = repo
            .fork(
                &metadata,
                ForkOptions {
                    id: Some("session-3".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            ids(&full.lock().unwrap().entries()),
            [user1, assistant1, user2]
        );

        repo.delete(&metadata);
        assert_eq!(
            repo.open(&metadata).unwrap_err().to_string(),
            "Session not found: session-1"
        );
    }

    #[test]
    fn fork_before_requires_a_user_message() {
        let repo = InMemorySessionRepo::new();
        let session = repo.create(Some("s"));
        let metadata = session.lock().unwrap().metadata();
        let assistant = session
            .lock()
            .unwrap()
            .append_message(assistant_message("a"))
            .unwrap();
        let fork = |entry: &str, position| {
            repo.fork(
                &metadata,
                ForkOptions {
                    entry_id: Some(entry.into()),
                    position,
                    id: None,
                },
            )
        };
        assert_eq!(
            fork(&assistant, ForkPosition::Before)
                .err()
                .unwrap()
                .to_string(),
            format!("Entry {assistant} is not a user message")
        );
        let at = fork(&assistant, ForkPosition::At).unwrap();
        assert_eq!(ids(&at.lock().unwrap().entries()), [assistant]);
        assert_eq!(
            fork("missing", ForkPosition::At).err().unwrap().to_string(),
            "Entry missing not found"
        );
    }

    #[test]
    fn jsonl_stores_sessions_below_encoded_cwd_directories_and_lists_by_cwd() {
        let root = temp_dir();
        let repo = JsonlSessionRepo::new(root.path());
        let session = repo
            .create(JsonlSessionCreateOptions {
                cwd: "/tmp/my-project".into(),
                id: Some("019de8c2-de29-73e9-ae0c-e134db34c447".into()),
                ..Default::default()
            })
            .unwrap();
        let other = repo
            .create(JsonlSessionCreateOptions {
                cwd: "/tmp/other-project".into(),
                id: Some("other-session".into()),
                ..Default::default()
            })
            .unwrap();
        let metadata = session.metadata();
        let other_metadata = other.metadata();
        assert!(metadata
            .path
            .to_string_lossy()
            .contains("--tmp-my-project--"));
        assert!(other_metadata
            .path
            .to_string_lossy()
            .contains("--tmp-other-project--"));
        assert!(metadata.path.exists());
        let by_cwd: Vec<String> = repo
            .list(Some("/tmp/my-project"))
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(by_cwd, std::slice::from_ref(&metadata.id));
        let mut all: Vec<String> = repo.list(None).unwrap().into_iter().map(|m| m.id).collect();
        all.sort();
        let mut expected = vec![metadata.id, other_metadata.id];
        expected.sort();
        assert_eq!(all, expected);
    }

    #[test]
    fn jsonl_opens_deletes_and_forks_by_metadata() {
        let root = temp_dir();
        let repo = JsonlSessionRepo::new(root.path());
        let mut source = repo
            .create(JsonlSessionCreateOptions {
                cwd: "/tmp/source".into(),
                id: Some("source-session".into()),
                ..Default::default()
            })
            .unwrap();
        let source_metadata = source.metadata();
        let user1 = source.append_message(user_message("one")).unwrap();
        let assistant1 = source.append_message(assistant_message("two")).unwrap();
        let user2 = source.append_message(user_message("three")).unwrap();
        assert_eq!(
            repo.open(&source_metadata).unwrap().metadata(),
            source_metadata
        );

        let target = || JsonlSessionCreateOptions {
            cwd: "/tmp/target".into(),
            ..Default::default()
        };
        let fork = repo
            .fork(
                &source_metadata,
                target(),
                ForkOptions {
                    entry_id: Some(user2.clone()),
                    id: Some("fork-session".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let fork_metadata = fork.metadata();
        assert_eq!(fork_metadata.cwd, "/tmp/target");
        assert_eq!(
            fork_metadata.parent_session_path.as_deref(),
            Some(source_metadata.path.to_string_lossy().as_ref())
        );
        assert_eq!(ids(&fork.entries()), [user1.clone(), assistant1.clone()]);
        let full = repo
            .fork(
                &source_metadata,
                target(),
                ForkOptions {
                    id: Some("full-fork-session".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(ids(&full.entries()), [user1, assistant1, user2]);

        repo.delete(&source_metadata).unwrap();
        assert!(!source_metadata.path.exists());
        assert!(repo
            .open(&source_metadata)
            .err()
            .unwrap()
            .to_string()
            .contains("Session not found"));
    }
}

//! Session storage backends.

use crate::{ensure_parent, SessionData};
use std::path::PathBuf;

/// Error type for session storage operations.
#[derive(Debug)]
pub enum SessionStoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotFound(String),
}

impl std::fmt::Display for SessionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStoreError::Io(e) => write!(f, "io error: {}", e),
            SessionStoreError::Json(e) => write!(f, "json error: {}", e),
            SessionStoreError::NotFound(id) => write!(f, "session not found: {}", id),
        }
    }
}

impl std::error::Error for SessionStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionStoreError::Io(e) => Some(e),
            SessionStoreError::Json(e) => Some(e),
            SessionStoreError::NotFound(_) => None,
        }
    }
}

impl From<std::io::Error> for SessionStoreError {
    fn from(e: std::io::Error) -> Self {
        SessionStoreError::Io(e)
    }
}

impl From<serde_json::Error> for SessionStoreError {
    fn from(e: serde_json::Error) -> Self {
        SessionStoreError::Json(e)
    }
}

/// Trait for session storage backends.
pub trait SessionStore {
    /// Save or update session data.
    fn save(&self, data: &SessionData) -> Result<(), SessionStoreError>;

    /// Load session data by id.
    fn load(&self, id: &str) -> Result<SessionData, SessionStoreError>;

    /// List all session ids.
    fn list(&self) -> Result<Vec<String>, SessionStoreError>;

    /// Delete a session by id.
    fn delete(&self, id: &str) -> Result<(), SessionStoreError>;
}

/// File-based session store using JSON files.
#[derive(Debug, Clone)]
pub struct FileSessionStore {
    dir: PathBuf,
}

impl FileSessionStore {
    /// Create a new store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Return the path for a given session id.
    fn path(&self, id: &str) -> PathBuf {
        let safe_id: String = id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{}.json", safe_id))
    }
}

impl SessionStore for FileSessionStore {
    fn save(&self, data: &SessionData) -> Result<(), SessionStoreError> {
        let path = self.path(&data.id);
        ensure_parent(&path)?;
        let json = serde_json::to_string_pretty(data)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    fn load(&self, id: &str) -> Result<SessionData, SessionStoreError> {
        let path = self.path(id);
        if !path.exists() {
            return Err(SessionStoreError::NotFound(id.to_string()));
        }
        let json = std::fs::read_to_string(&path)?;
        let data: SessionData = serde_json::from_str(&json)?;
        Ok(data)
    }

    fn list(&self) -> Result<Vec<String>, SessionStoreError> {
        let mut ids = Vec::new();
        if self.dir.exists() {
            for entry in std::fs::read_dir(&self.dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        ids.push(stem.to_string());
                    }
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn delete(&self, id: &str) -> Result<(), SessionStoreError> {
        let path = self.path(id);
        if !path.exists() {
            return Err(SessionStoreError::NotFound(id.to_string()));
        }
        std::fs::remove_file(&path)?;
        Ok(())
    }
}

/// In-memory session store for tests and ephemeral usage.
#[derive(Debug, Default, Clone)]
pub struct MemorySessionStore {
    sessions: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, SessionData>>>,
}

impl MemorySessionStore {
    /// Create a new empty memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionStore for MemorySessionStore {
    fn save(&self, data: &SessionData) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(data.id.clone(), data.clone());
        Ok(())
    }

    fn load(&self, id: &str) -> Result<SessionData, SessionStoreError> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))
    }

    fn list(&self) -> Result<Vec<String>, SessionStoreError> {
        let sessions = self.sessions.lock().unwrap();
        let mut ids: Vec<String> = sessions.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }

    fn delete(&self, id: &str) -> Result<(), SessionStoreError> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.remove(id).is_none() {
            return Err(SessionStoreError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionData;
    use cortexcode_agent_types::AgentContext;

    fn sample_data() -> SessionData {
        SessionData::from_context("abc", &AgentContext::new("system".into(), vec![], vec![]))
    }

    #[test]
    fn test_memory_store_save_load() {
        let store = MemorySessionStore::new();
        let data = sample_data();
        store.save(&data).unwrap();
        let loaded = store.load("abc").unwrap();
        assert_eq!(loaded.id, "abc");
    }

    #[test]
    fn test_memory_store_delete() {
        let store = MemorySessionStore::new();
        let data = sample_data();
        store.save(&data).unwrap();
        store.delete("abc").unwrap();
        assert!(store.load("abc").is_err());
    }

    #[test]
    fn test_file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cortex-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSessionStore::new(&dir);
        let data = sample_data().with_title("Test");
        store.save(&data).unwrap();
        let loaded = store.load("abc").unwrap();
        assert_eq!(loaded.metadata.title.as_deref(), Some("Test"));
        let ids = store.list().unwrap();
        assert!(ids.contains(&"abc".to_string()));
        store.delete("abc").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_file_store_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "cortex-session-test-missing-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = FileSessionStore::new(&dir);
        assert!(matches!(
            store.load("missing").unwrap_err(),
            SessionStoreError::NotFound(_)
        ));
    }
}

//! Session storage: `SessionStorage` plus the in-memory and JSONL backends.
//!
//! Port of hoocode `packages/agent/src/harness/session/storage/{memory,jsonl}.ts`.
//! The TypeScript methods return promises over in-process state and local
//! file appends; here they are synchronous.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entry::{FileEntry, Header, CURRENT_SESSION_VERSION};
use crate::{create_session_id, create_timestamp, generate_entry_id, SessionError};

/// A tree entry (`SessionTreeEntry`). Storage never holds `FileEntry::Session`
/// headers; those are the first line of a JSONL file only.
pub type SessionTreeEntry = FileEntry;

/// `SessionMetadata`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: String,
}

/// `JsonlSessionMetadata`: the header of a JSONL session file plus its path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlSessionMetadata {
    pub id: String,
    pub created_at: String,
    pub cwd: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_path: Option<String>,
}

/// `SessionStorage`: an append-only entry tree with a movable leaf.
pub trait SessionStorage {
    type Metadata: Clone;

    fn metadata(&self) -> Self::Metadata;
    fn leaf_id(&self) -> Option<String>;
    /// Move the leaf; `Entry <id> not found` if it is not in the tree.
    fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), SessionError>;
    /// A fresh 8-character entry id not used in this session.
    fn create_entry_id(&self) -> String;
    /// Append an entry; it becomes the leaf.
    fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError>;
    fn entry(&self, id: &str) -> Option<&SessionTreeEntry>;
    /// Entries whose `type` tag is `entry_type` (e.g. `"session_info"`), in order.
    fn find_entries(&self, entry_type: &str) -> Vec<&SessionTreeEntry>;
    /// The current label of entry `id`, if any.
    fn label(&self, id: &str) -> Option<&str>;
    /// Entries from the root down to `leaf_id` (empty for `None`).
    fn path_to_root(&self, leaf_id: Option<&str>) -> Vec<SessionTreeEntry>;
    fn entries(&self) -> Vec<SessionTreeEntry>;
}

/// The in-memory tree shared by both backends.
#[derive(Debug, Clone, Default)]
struct Tree {
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, usize>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl Tree {
    fn new(entries: Vec<SessionTreeEntry>) -> Self {
        let mut tree = Tree::default();
        for entry in entries {
            tree.push(entry);
        }
        tree
    }

    /// Record `entry` (updating the id index and label cache) and make it the leaf.
    fn push(&mut self, entry: SessionTreeEntry) {
        if let FileEntry::Label {
            target_id, label, ..
        } = &entry
        {
            match label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                Some(label) => {
                    self.labels_by_id
                        .insert(target_id.clone(), label.to_string());
                }
                None => {
                    self.labels_by_id.remove(target_id);
                }
            }
        }
        if let Some(id) = entry.id() {
            self.by_id.insert(id.to_string(), self.entries.len());
            self.leaf_id = Some(id.to_string());
        }
        self.entries.push(entry);
    }

    fn get(&self, id: &str) -> Option<&SessionTreeEntry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }

    fn set_leaf(&mut self, leaf_id: Option<&str>) -> Result<(), SessionError> {
        if let Some(id) = leaf_id {
            if !self.by_id.contains_key(id) {
                return Err(SessionError::EntryNotFound(id.to_string()));
            }
        }
        self.leaf_id = leaf_id.map(str::to_string);
        Ok(())
    }

    fn find(&self, entry_type: &str) -> Vec<&SessionTreeEntry> {
        self.entries
            .iter()
            .filter(|e| entry_type_of(e) == entry_type)
            .collect()
    }

    fn path_to_root(&self, leaf_id: Option<&str>) -> Vec<SessionTreeEntry> {
        let mut path = Vec::new();
        let mut current = leaf_id.and_then(|id| self.get(id));
        while let Some(entry) = current {
            path.push(entry.clone());
            current = entry.parent_id().and_then(|p| self.get(p));
        }
        path.reverse();
        path
    }

    fn new_id(&self) -> String {
        generate_entry_id(|id| self.by_id.contains_key(id))
    }
}

/// The `type` tag of an entry as it appears on the wire.
pub fn entry_type_of(entry: &FileEntry) -> &'static str {
    match entry {
        FileEntry::Session(_) => "session",
        FileEntry::Message { .. } => "message",
        FileEntry::ThinkingLevelChange { .. } => "thinking_level_change",
        FileEntry::ModelChange { .. } => "model_change",
        FileEntry::Compaction { .. } => "compaction",
        FileEntry::BranchSummary { .. } => "branch_summary",
        FileEntry::Custom { .. } => "custom",
        FileEntry::CustomMessage { .. } => "custom_message",
        FileEntry::Label { .. } => "label",
        FileEntry::SessionInfo { .. } => "session_info",
    }
}

macro_rules! delegate_tree {
    () => {
        fn leaf_id(&self) -> Option<String> {
            self.tree.leaf_id.clone()
        }

        fn set_leaf_id(&mut self, leaf_id: Option<&str>) -> Result<(), SessionError> {
            self.tree.set_leaf(leaf_id)
        }

        fn create_entry_id(&self) -> String {
            self.tree.new_id()
        }

        fn entry(&self, id: &str) -> Option<&SessionTreeEntry> {
            self.tree.get(id)
        }

        fn find_entries(&self, entry_type: &str) -> Vec<&SessionTreeEntry> {
            self.tree.find(entry_type)
        }

        fn label(&self, id: &str) -> Option<&str> {
            self.tree.labels_by_id.get(id).map(String::as_str)
        }

        fn path_to_root(&self, leaf_id: Option<&str>) -> Vec<SessionTreeEntry> {
            self.tree.path_to_root(leaf_id)
        }

        fn entries(&self) -> Vec<SessionTreeEntry> {
            self.tree.entries.clone()
        }
    };
}

// ---------------------------------------------------------------------------
// InMemorySessionStorage
// ---------------------------------------------------------------------------

/// `InMemorySessionStorage`.
#[derive(Debug, Clone)]
pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    tree: Tree,
}

impl InMemorySessionStorage {
    /// `new InMemorySessionStorage({ entries, leafId, metadata })`. The leaf
    /// defaults to the last entry; `Entry <id> not found` for an unknown leaf.
    pub fn new(
        entries: Vec<SessionTreeEntry>,
        leaf_id: Option<String>,
        metadata: Option<SessionMetadata>,
    ) -> Result<Self, SessionError> {
        let mut tree = Tree::new(entries);
        if leaf_id.is_some() {
            tree.set_leaf(leaf_id.as_deref())?;
        }
        Ok(Self {
            metadata: metadata.unwrap_or_else(|| SessionMetadata {
                id: create_session_id(),
                created_at: create_timestamp(),
            }),
            tree,
        })
    }
}

impl Default for InMemorySessionStorage {
    fn default() -> Self {
        Self::new(Vec::new(), None, None).expect("an empty tree has no leaf to validate")
    }
}

impl SessionStorage for InMemorySessionStorage {
    type Metadata = SessionMetadata;

    fn metadata(&self) -> SessionMetadata {
        self.metadata.clone()
    }

    fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        self.tree.push(entry);
        Ok(())
    }

    delegate_tree!();
}

// ---------------------------------------------------------------------------
// JsonlSessionStorage
// ---------------------------------------------------------------------------

/// `JsonlSessionStorage`: a header line, then one JSON entry per line.
#[derive(Debug, Clone)]
pub struct JsonlSessionStorage {
    path: PathBuf,
    metadata: JsonlSessionMetadata,
    tree: Tree,
}

fn invalid_header(path: &Path) -> SessionError {
    SessionError::Invalid(format!(
        "Invalid JSONL session file {}: first line is not a valid session header",
        path.display()
    ))
}

fn missing_header(path: &Path) -> SessionError {
    SessionError::Invalid(format!(
        "Invalid JSONL session file {}: missing session header",
        path.display()
    ))
}

fn header_to_metadata(header: &Header, path: &Path) -> JsonlSessionMetadata {
    JsonlSessionMetadata {
        id: header.id.clone(),
        created_at: header.timestamp.clone(),
        cwd: header.cwd.clone(),
        path: path.to_path_buf(),
        parent_session_path: header.parent_session.clone(),
    }
}

fn resolve(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `loadJsonlSessionMetadata`: read only the header line.
pub fn load_jsonl_session_metadata(
    path: impl AsRef<Path>,
) -> Result<JsonlSessionMetadata, SessionError> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first)?;
    if first.trim().is_empty() {
        return Err(missing_header(path));
    }
    let header: Header =
        serde_json::from_str(first.trim_end()).map_err(|_| invalid_header(path))?;
    Ok(header_to_metadata(&header, &resolve(path)))
}

impl JsonlSessionStorage {
    /// `JsonlSessionStorage.open`: load the header and every parseable entry;
    /// the leaf is the last entry. Malformed entry lines are skipped.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let path = resolve(path.as_ref());
        let content = std::fs::read_to_string(&path)?;
        let mut lines = content.split('\n').filter(|l| !l.trim().is_empty());
        let first = lines.next().ok_or_else(|| missing_header(&path))?;
        let header: Header = serde_json::from_str(first).map_err(|_| invalid_header(&path))?;
        let entries = lines
            .filter_map(|line| serde_json::from_str::<SessionTreeEntry>(line).ok())
            .filter(|e| !matches!(e, FileEntry::Session(_)))
            .collect();
        Ok(Self {
            metadata: header_to_metadata(&header, &path),
            path,
            tree: Tree::new(entries),
        })
    }

    /// `JsonlSessionStorage.create`: write a fresh version-3 header.
    pub fn create(
        path: impl AsRef<Path>,
        cwd: &str,
        session_id: &str,
        parent_session_path: Option<&str>,
    ) -> Result<Self, SessionError> {
        let path = resolve(path.as_ref());
        let header = Header {
            version: Some(CURRENT_SESSION_VERSION),
            id: session_id.to_string(),
            timestamp: create_timestamp(),
            cwd: cwd.to_string(),
            parent_session: parent_session_path.map(str::to_string),
            branch: None,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let line = serde_json::to_string(&FileEntry::Session(header.clone()))?;
        std::fs::write(&path, format!("{line}\n"))?;
        Ok(Self {
            metadata: header_to_metadata(&header, &path),
            path,
            tree: Tree::default(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SessionStorage for JsonlSessionStorage {
    type Metadata = JsonlSessionMetadata;

    fn metadata(&self) -> JsonlSessionMetadata {
        self.metadata.clone()
    }

    fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        let line = serde_json::to_string(&entry)?;
        let mut file = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(format!("{line}\n").as_bytes())?;
        self.tree.push(entry);
        Ok(())
    }

    delegate_tree!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{assistant_message, temp_dir, user_message};

    fn message_entry(id: &str, parent: Option<&str>, text: &str) -> SessionTreeEntry {
        FileEntry::Message {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            message: user_message(text),
        }
    }

    fn label_entry(id: &str, parent: &str, target: &str, label: Option<&str>) -> SessionTreeEntry {
        FileEntry::Label {
            id: id.into(),
            parent_id: Some(parent.into()),
            timestamp: "2026-01-01T00:00:01.000Z".into(),
            target_id: target.into(),
            label: label.map(str::to_string),
        }
    }

    fn ids(entries: &[SessionTreeEntry]) -> Vec<&str> {
        entries.iter().filter_map(|e| e.id()).collect()
    }

    // --- InMemorySessionStorage ---

    #[test]
    fn memory_returns_configured_metadata() {
        let metadata = SessionMetadata {
            id: "session-1".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
        };
        let storage = InMemorySessionStorage::new(vec![], None, Some(metadata.clone())).unwrap();
        assert_eq!(storage.metadata(), metadata);
    }

    #[test]
    fn memory_copies_initial_entries_and_tracks_leaf_independently() {
        let mut initial = vec![message_entry("entry-1", None, "one")];
        let mut storage = InMemorySessionStorage::new(initial.clone(), None, None).unwrap();
        initial.push(message_entry("entry-2", None, "one"));
        assert_eq!(ids(&storage.entries()), ["entry-1"]);
        assert_eq!(storage.leaf_id().as_deref(), Some("entry-1"));
        storage.set_leaf_id(None).unwrap();
        assert_eq!(storage.leaf_id(), None);
    }

    #[test]
    fn memory_rejects_invalid_leaf_ids() {
        let mut storage = InMemorySessionStorage::default();
        assert_eq!(
            storage
                .set_leaf_id(Some("missing"))
                .unwrap_err()
                .to_string(),
            "Entry missing not found"
        );
        assert_eq!(
            InMemorySessionStorage::new(vec![], Some("missing".into()), None)
                .unwrap_err()
                .to_string(),
            "Entry missing not found"
        );
    }

    #[test]
    fn memory_finds_entries_by_type() {
        let storage =
            InMemorySessionStorage::new(vec![message_entry("entry-1", None, "one")], None, None)
                .unwrap();
        let found: Vec<_> = storage
            .find_entries("message")
            .into_iter()
            .cloned()
            .collect();
        assert_eq!(ids(&found), ["entry-1"]);
        assert!(storage.find_entries("session_info").is_empty());
    }

    #[test]
    fn memory_maintains_label_lookup() {
        let mut storage =
            InMemorySessionStorage::new(vec![message_entry("entry-1", None, "one")], None, None)
                .unwrap();
        assert_eq!(storage.label("entry-1"), None);
        storage
            .append_entry(label_entry(
                "label-1",
                "entry-1",
                "entry-1",
                Some("checkpoint"),
            ))
            .unwrap();
        assert_eq!(storage.label("entry-1"), Some("checkpoint"));
        storage
            .append_entry(label_entry("label-2", "label-1", "entry-1", None))
            .unwrap();
        assert_eq!(storage.label("entry-1"), None);
    }

    #[test]
    fn memory_walks_paths_to_root() {
        let root = message_entry("root", None, "root");
        let child = FileEntry::Message {
            id: "child".into(),
            parent_id: Some("root".into()),
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            message: assistant_message("child"),
        };
        let storage = InMemorySessionStorage::new(vec![root, child], None, None).unwrap();
        assert_eq!(ids(&storage.path_to_root(Some("child"))), ["root", "child"]);
        assert!(storage.path_to_root(None).is_empty());
    }

    // --- JsonlSessionStorage ---

    #[test]
    fn jsonl_open_missing_file_is_enoent() {
        let dir = temp_dir();
        let err = JsonlSessionStorage::open(dir.path().join("session.jsonl")).unwrap_err();
        match err {
            SessionError::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            other => panic!("expected ENOENT, got {other}"),
        }
    }

    #[test]
    fn jsonl_writes_the_header_on_create() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let cwd = dir.path().to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(&path, &cwd, "session-1", None).unwrap();
        let read_lines = || -> Vec<String> {
            std::fs::read_to_string(&path)
                .unwrap()
                .trim()
                .split('\n')
                .map(str::to_string)
                .collect()
        };
        assert_eq!(read_lines().len(), 1);
        assert_eq!(storage.leaf_id(), None);
        assert!(storage.entries().is_empty());
        storage
            .append_entry(message_entry("user-1", None, "one"))
            .unwrap();
        let lines = read_lines();
        let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(first["type"], "session");
        assert_eq!(second["id"], "user-1");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn jsonl_header_matches_hoocode_key_order() {
        let dir = temp_dir();
        let path = dir.path().join("s.jsonl");
        JsonlSessionStorage::create(&path, "/w", "session-1", Some("/tmp/parent.jsonl")).unwrap();
        let line = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["type", "version", "id", "timestamp", "cwd", "parentSession"]
        );
        assert_eq!(value["version"], 3);
    }

    #[test]
    fn jsonl_rejects_malformed_headers() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "not json\n").unwrap();
        let err = JsonlSessionStorage::open(&path).unwrap_err().to_string();
        assert!(
            err.contains("first line is not a valid session header"),
            "{err}"
        );
    }

    #[test]
    fn jsonl_ignores_malformed_entry_lines() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let header = serde_json::json!({
            "type": "session", "version": 3, "id": "session-1",
            "timestamp": "2026-01-01T00:00:00.000Z", "cwd": dir.path(),
        });
        let entry = serde_json::to_string(&message_entry("entry-1", None, "one")).unwrap();
        std::fs::write(&path, format!("{header}\nnot json\n{entry}\n")).unwrap();
        let storage = JsonlSessionStorage::open(&path).unwrap();
        assert_eq!(ids(&storage.entries()), ["entry-1"]);
        assert_eq!(storage.leaf_id().as_deref(), Some("entry-1"));
    }

    #[test]
    fn jsonl_creates_and_reads_metadata_from_the_header() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let cwd = dir.path().to_string_lossy().to_string();
        let mut storage =
            JsonlSessionStorage::create(&path, &cwd, "session-1", Some("/tmp/parent.jsonl"))
                .unwrap();
        let metadata = storage.metadata();
        assert_eq!(metadata.id, "session-1");
        assert_eq!(metadata.cwd, cwd);
        assert_eq!(metadata.path, path);
        assert_eq!(
            metadata.parent_session_path.as_deref(),
            Some("/tmp/parent.jsonl")
        );
        storage
            .append_entry(message_entry("user-1", None, "one"))
            .unwrap();
        assert_eq!(load_jsonl_session_metadata(&path).unwrap(), metadata);
    }

    #[test]
    fn jsonl_loads_existing_entries_and_reconstructs_leaf() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let mut storage = JsonlSessionStorage::create(&path, "/w", "session-1", None).unwrap();
        storage
            .append_entry(message_entry("root", None, "root"))
            .unwrap();
        storage
            .append_entry(FileEntry::Message {
                id: "child".into(),
                parent_id: Some("root".into()),
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                message: assistant_message("child"),
            })
            .unwrap();
        let loaded = JsonlSessionStorage::open(&path).unwrap();
        assert_eq!(loaded.leaf_id().as_deref(), Some("child"));
        assert_eq!(ids(&loaded.entries()), ["root", "child"]);
        assert_eq!(ids(&loaded.path_to_root(Some("child"))), ["root", "child"]);
    }

    #[test]
    fn jsonl_finds_entries_by_type() {
        let dir = temp_dir();
        let mut storage =
            JsonlSessionStorage::create(dir.path().join("s.jsonl"), "/w", "session-1", None)
                .unwrap();
        storage
            .append_entry(message_entry("entry-1", None, "one"))
            .unwrap();
        assert_eq!(storage.find_entries("message").len(), 1);
        assert!(storage.find_entries("session_info").is_empty());
    }

    #[test]
    fn jsonl_maintains_label_lookup_across_reload() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let mut storage = JsonlSessionStorage::create(&path, "/w", "session-1", None).unwrap();
        storage
            .append_entry(message_entry("entry-1", None, "one"))
            .unwrap();
        assert_eq!(storage.label("entry-1"), None);
        storage
            .append_entry(label_entry(
                "label-1",
                "entry-1",
                "entry-1",
                Some("checkpoint"),
            ))
            .unwrap();
        assert_eq!(storage.label("entry-1"), Some("checkpoint"));
        storage
            .append_entry(label_entry("label-2", "label-1", "entry-1", None))
            .unwrap();
        assert_eq!(storage.label("entry-1"), None);
        assert_eq!(
            JsonlSessionStorage::open(&path).unwrap().label("entry-1"),
            None
        );
    }

    #[test]
    fn jsonl_metadata_reads_only_the_first_line() {
        let dir = temp_dir();
        let path = dir.path().join("session.jsonl");
        let header = serde_json::json!({
            "type": "session", "version": 3, "id": "session-1",
            "timestamp": "2026-01-01T00:00:00.000Z", "cwd": "/w",
        });
        std::fs::write(&path, format!("{header}\n{}\n", "{".repeat(10000))).unwrap();
        assert_eq!(
            load_jsonl_session_metadata(&path).unwrap(),
            JsonlSessionMetadata {
                id: "session-1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                cwd: "/w".into(),
                path: path.clone(),
                parent_session_path: None,
            }
        );
    }
}

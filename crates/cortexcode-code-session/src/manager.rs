//! JSONL session manager.
//!
//! Conversations are stored as append-only trees. Each entry has an `id` and a
//! `parent_id`; the leaf pointer tracks the current position. Branching moves
//! the leaf to an earlier entry without modifying history.

use crate::context::{build_session_context, SessionContext};
use crate::entry::{CustomMessageContent, FileEntry, Header, CURRENT_SESSION_VERSION};
use chrono::{TimeZone, Utc};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{Content, Message, UserMessage};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Error type for session manager operations.
#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    NotFound(String),
    InvalidSession(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "io error: {e}"),
            SessionError::Json(e) => write!(f, "json error: {e}"),
            SessionError::NotFound(id) => write!(f, "entry not found: {id}"),
            SessionError::InvalidSession(msg) => write!(f, "invalid session: {msg}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SessionError::Io(e) => Some(e),
            SessionError::Json(e) => Some(e),
            SessionError::NotFound(_) | SessionError::InvalidSession(_) => None,
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

/// Generate a new session id (UUIDv7).
pub fn create_session_id() -> String {
    Uuid::now_v7().to_string()
}

/// Generate a short, collision-checked entry id.
pub fn generate_id(existing: &std::collections::HashSet<String>) -> String {
    for _ in 0..100 {
        let id = Uuid::new_v4().to_string()[..8].to_string();
        if !existing.contains(&id) {
            return id;
        }
    }
    Uuid::new_v4().to_string()
}

/// Encode a working directory into a safe directory name.
pub fn encode_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_start_matches(['/', '\\']);
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{safe}--")
}

/// Default session directory for a project.
pub fn default_session_dir(cwd: &str) -> PathBuf {
    cortexcode_code_config::default_config_dir()
        .join("sessions")
        .join(encode_cwd(cwd))
}

/// Root session directory containing per-project subdirectories.
pub fn default_sessions_root() -> PathBuf {
    cortexcode_code_config::default_config_dir().join("sessions")
}

/// Make an ISO 8601 timestamp suitable for file names.
fn iso_timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Replace `:` and `.` in a timestamp so it can be used in file names.
fn safe_timestamp(ts: &str) -> String {
    ts.replace([':', '.'], "-")
}

/// Options for creating a new session.
#[derive(Debug, Clone, Default)]
pub struct NewSessionOptions {
    /// Explicit session id. A fresh UUIDv7 is generated if omitted.
    pub id: Option<String>,
    /// Path to the parent session when forking.
    pub parent_session: Option<String>,
}

/// A node in the session tree returned by `get_tree`.
#[derive(Debug, Clone)]
pub struct SessionTreeNode {
    /// The entry at this node.
    pub entry: FileEntry,
    /// Child nodes.
    pub children: Vec<SessionTreeNode>,
    /// Resolved label, if any.
    pub label: Option<String>,
    /// Timestamp of the latest label change, if any.
    pub label_timestamp: Option<String>,
}

/// Metadata returned when listing sessions.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Path to the session file.
    pub path: PathBuf,
    /// Session id.
    pub id: String,
    /// Working directory stored in the session header.
    pub cwd: String,
    /// User-defined display name, if any.
    pub name: Option<String>,
    /// Path to the parent session, if this session was forked.
    pub parent_session_path: Option<String>,
    /// Creation time from the session header.
    pub created: chrono::DateTime<Utc>,
    /// Last activity time.
    pub modified: chrono::DateTime<Utc>,
    /// Number of message entries.
    pub message_count: usize,
    /// Text of the first user message.
    pub first_message: String,
    /// All user/assistant message text concatenated.
    pub all_messages_text: String,
}

/// Progress callback for session listing: `(loaded, total)`.
pub type SessionListProgress = dyn Fn(usize, usize) + Send + Sync;

/// Manages a conversation session stored as an append-only JSONL tree.
#[derive(Debug, Clone)]
pub struct SessionManager {
    header: Header,
    session_file: Option<PathBuf>,
    session_dir: PathBuf,
    cwd: String,
    persist: bool,
    flushed: bool,
    entries: Vec<FileEntry>,
    by_id: HashMap<String, FileEntry>,
    labels_by_id: HashMap<String, String>,
    label_timestamps_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl SessionManager {
    fn new(
        cwd: String,
        session_dir: PathBuf,
        session_file: Option<PathBuf>,
        persist: bool,
    ) -> Self {
        let mut manager = Self {
            header: Header {
                version: Some(CURRENT_SESSION_VERSION),
                id: create_session_id(),
                timestamp: iso_timestamp(),
                cwd: cwd.clone(),
                parent_session: None,
            },
            session_file,
            session_dir,
            cwd,
            persist,
            flushed: false,
            entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            label_timestamps_by_id: HashMap::new(),
            leaf_id: None,
        };

        if persist && !manager.session_dir.as_os_str().is_empty() {
            let _ = fs::create_dir_all(&manager.session_dir);
        }

        if let Some(path) = manager.session_file.clone() {
            manager.set_session_file(path);
        } else {
            manager.new_session(NewSessionOptions::default());
        }

        manager
    }

    /// Create a new persisted session for `cwd`.
    pub fn create(cwd: impl Into<String>, session_dir: Option<PathBuf>) -> Self {
        let cwd = cwd.into();
        let dir = session_dir.unwrap_or_else(|| default_session_dir(&cwd));
        Self::new(cwd, dir, None, true)
    }

    /// Open an existing session file.
    pub fn open(
        path: impl Into<PathBuf>,
        session_dir: Option<PathBuf>,
        cwd_override: Option<String>,
    ) -> Self {
        let path = path.into();
        let entries = load_entries_from_file(&path);
        let cwd = cwd_override
            .or_else(|| {
                entries.iter().find_map(|e| match e {
                    FileEntry::Session(h) => Some(h.cwd.clone()),
                    _ => None,
                })
            })
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let dir = session_dir.unwrap_or_else(|| {
            path.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(default_sessions_root)
        });
        Self::new(cwd, dir, Some(path), true)
    }

    /// Continue the most recent session for `cwd`, or create one if none exists.
    pub fn continue_recent(cwd: impl Into<String>, session_dir: Option<PathBuf>) -> Self {
        let cwd = cwd.into();
        let dir = session_dir.unwrap_or_else(|| default_session_dir(&cwd));
        let most_recent = find_most_recent_session(&dir);
        Self::new(cwd, dir, most_recent, true)
    }

    /// Create an in-memory session with no file persistence.
    pub fn in_memory(cwd: impl Into<String>) -> Self {
        let cwd = cwd.into();
        Self::new(cwd, PathBuf::new(), None, false)
    }

    /// Fork a session from another project into `target_cwd`.
    pub fn fork_from(
        source_path: impl AsRef<Path>,
        target_cwd: impl Into<String>,
        session_dir: Option<PathBuf>,
    ) -> Result<Self, SessionError> {
        let source_path = source_path.as_ref();
        let source_entries = load_entries_from_file(source_path);
        if source_entries.is_empty() {
            return Err(SessionError::InvalidSession(format!(
                "source session is empty or invalid: {}",
                source_path.display()
            )));
        }
        source_entries
            .iter()
            .find_map(|e| match e {
                FileEntry::Session(_) => Some(()),
                _ => None,
            })
            .ok_or_else(|| {
                SessionError::InvalidSession(format!(
                    "source session has no header: {}",
                    source_path.display()
                ))
            })?;

        let target_cwd = target_cwd.into();
        let dir = session_dir.unwrap_or_else(|| default_session_dir(&target_cwd));
        fs::create_dir_all(&dir)?;

        let new_id = create_session_id();
        let timestamp = iso_timestamp();
        let file_path = dir.join(format!("{}_{}.jsonl", safe_timestamp(&timestamp), new_id));

        let header = FileEntry::Session(Header {
            version: Some(CURRENT_SESSION_VERSION),
            id: new_id,
            timestamp: timestamp.clone(),
            cwd: target_cwd.clone(),
            parent_session: Some(source_path.to_string_lossy().into_owned()),
        });

        let mut lines = vec![serde_json::to_string(&header)?];
        for entry in &source_entries {
            if !matches!(entry, FileEntry::Session(_)) {
                lines.push(serde_json::to_string(entry)?);
            }
        }
        fs::write(&file_path, lines.join("\n") + "\n")?;

        Ok(Self::new(target_cwd, dir, Some(file_path), true))
    }

    /// Switch to a different session file.
    pub fn set_session_file(&mut self, session_file: impl Into<PathBuf>) {
        let session_file = session_file.into();
        self.session_file = Some(session_file.clone());
        if session_file.exists() {
            let loaded = load_entries_from_file(&session_file);
            if loaded.is_empty() {
                let explicit = self.session_file.clone();
                self.new_session(NewSessionOptions::default());
                self.session_file = explicit;
                let _ = self.rewrite_file();
                self.flushed = true;
                return;
            }

            let mut iter = loaded.into_iter();
            if let Some(FileEntry::Session(header)) = iter.next() {
                self.header = header;
            } else {
                let explicit = self.session_file.clone();
                self.new_session(NewSessionOptions::default());
                self.session_file = explicit;
                let _ = self.rewrite_file();
                self.flushed = true;
                return;
            }

            self.entries = iter.collect();
            if self.header.version.is_none() {
                self.header.version = Some(CURRENT_SESSION_VERSION);
                let _ = self.rewrite_file();
            }
            self.build_index();
            self.flushed = true;
        } else {
            let explicit = self.session_file.clone();
            self.new_session(NewSessionOptions::default());
            self.session_file = explicit;
        }
    }

    /// Start a new empty session.
    pub fn new_session(&mut self, options: NewSessionOptions) -> Option<PathBuf> {
        self.header = Header {
            version: Some(CURRENT_SESSION_VERSION),
            id: options.id.unwrap_or_else(create_session_id),
            timestamp: iso_timestamp(),
            cwd: self.cwd.clone(),
            parent_session: options.parent_session,
        };
        self.entries.clear();
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        self.flushed = false;

        if self.persist {
            let ts = safe_timestamp(&self.header.timestamp);
            let file = self
                .session_dir
                .join(format!("{}_{}.jsonl", ts, self.header.id));
            self.session_file = Some(file.clone());
            Some(file)
        } else {
            self.session_file = None;
            None
        }
    }

    fn build_index(&mut self) {
        self.by_id.clear();
        self.labels_by_id.clear();
        self.label_timestamps_by_id.clear();
        self.leaf_id = None;
        for entry in &self.entries {
            if let Some(id) = entry.id().map(String::from) {
                self.by_id.insert(id.clone(), entry.clone());
                self.leaf_id = Some(id);
            }
            if let FileEntry::Label {
                target_id,
                label,
                timestamp,
                ..
            } = entry
            {
                if let Some(label) = label {
                    self.labels_by_id.insert(target_id.clone(), label.clone());
                    self.label_timestamps_by_id
                        .insert(target_id.clone(), timestamp.clone());
                } else {
                    self.labels_by_id.remove(target_id);
                    self.label_timestamps_by_id.remove(target_id);
                }
            }
        }
    }

    fn rewrite_file(&self) -> Result<(), SessionError> {
        if !self.persist {
            return Ok(());
        }
        if let Some(path) = &self.session_file {
            let mut lines = vec![serde_json::to_string(&FileEntry::Session(
                self.header.clone(),
            ))?];
            for entry in &self.entries {
                lines.push(serde_json::to_string(entry)?);
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, lines.join("\n") + "\n")?;
        }
        Ok(())
    }

    fn persist(&mut self, entry: &FileEntry) -> Result<(), SessionError> {
        if !self.persist {
            return Ok(());
        }
        let Some(path) = self.session_file.clone() else {
            return Ok(());
        };

        let has_assistant = self.entries.iter().any(|e| {
            if let FileEntry::Message { message, .. } = e {
                is_assistant_message(message)
            } else {
                false
            }
        });

        if !has_assistant {
            self.flushed = false;
            return Ok(());
        }

        if !self.flushed {
            self.rewrite_file()?;
            self.flushed = true;
        } else {
            let line = serde_json::to_string(entry)?;
            use std::io::Write;
            let mut file = fs::OpenOptions::new().append(true).open(&path)?;
            writeln!(file, "{line}")?;
        }
        Ok(())
    }

    fn append(&mut self, entry: FileEntry) {
        if let Some(id) = entry.id().map(String::from) {
            self.by_id.insert(id.clone(), entry.clone());
            self.leaf_id = Some(id);
        }
        self.entries.push(entry.clone());
        let _ = self.persist(&entry);
    }

    fn id_set(&self) -> std::collections::HashSet<String> {
        self.by_id.keys().cloned().collect()
    }

    /// Append a message and advance the leaf. Returns the new entry id.
    pub fn append_message(&mut self, message: AgentMessage) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::Message {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            message,
        };
        self.append(entry);
        id
    }

    /// Append a thinking level change.
    pub fn append_thinking_level_change(&mut self, thinking_level: impl Into<String>) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::ThinkingLevelChange {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            thinking_level: thinking_level.into(),
        };
        self.append(entry);
        id
    }

    /// Append a model change.
    pub fn append_model_change(
        &mut self,
        provider: impl Into<String>,
        model_id: impl Into<String>,
    ) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::ModelChange {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            provider: provider.into(),
            model_id: model_id.into(),
        };
        self.append(entry);
        id
    }

    /// Append a compaction boundary.
    pub fn append_compaction(
        &mut self,
        summary: impl Into<String>,
        first_kept_entry_id: impl Into<String>,
        tokens_before: u64,
        tokens_after: Option<u64>,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::Compaction {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            summary: summary.into(),
            first_kept_entry_id: first_kept_entry_id.into(),
            tokens_before,
            tokens_after,
            details,
            from_hook,
        };
        self.append(entry);
        id
    }

    /// Append a custom extension entry.
    pub fn append_custom_entry(
        &mut self,
        custom_type: impl Into<String>,
        data: Option<serde_json::Value>,
    ) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::Custom {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            custom_type: custom_type.into(),
            data,
        };
        self.append(entry);
        id
    }

    /// Append session info (e.g. display name).
    pub fn append_session_info(&mut self, name: impl Into<String>) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::SessionInfo {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            name: Some(name.into().trim().to_string()),
        };
        self.append(entry);
        id
    }

    /// Append a custom message that participates in LLM context.
    pub fn append_custom_message(
        &mut self,
        custom_type: impl Into<String>,
        content: impl Into<CustomMessageContent>,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> String {
        let id = generate_id(&self.id_set());
        let entry = FileEntry::CustomMessage {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: iso_timestamp(),
            custom_type: custom_type.into(),
            content: content.into(),
            display,
            details,
        };
        self.append(entry);
        id
    }

    /// Set or clear a label on an entry.
    pub fn append_label_change(
        &mut self,
        target_id: impl Into<String>,
        label: Option<impl Into<String>>,
    ) -> Result<String, SessionError> {
        let target_id = target_id.into();
        if !self.by_id.contains_key(&target_id) {
            return Err(SessionError::NotFound(target_id));
        }
        let id = generate_id(&self.id_set());
        let label = label.map(|l| l.into());
        let timestamp = iso_timestamp();
        let entry = FileEntry::Label {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp: timestamp.clone(),
            target_id: target_id.clone(),
            label: label.clone(),
        };
        if let Some(label) = label {
            self.labels_by_id.insert(target_id.clone(), label);
            self.label_timestamps_by_id.insert(target_id, timestamp);
        } else {
            self.labels_by_id.remove(&target_id);
            self.label_timestamps_by_id.remove(&target_id);
        }
        self.append(entry);
        Ok(id)
    }

    /// Returns true when this session is persisted to disk.
    pub fn is_persisted(&self) -> bool {
        self.persist
    }

    /// Current working directory stored in the session.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Directory containing session files.
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    /// Session id from the header.
    pub fn session_id(&self) -> &str {
        &self.header.id
    }

    /// Path to the current session file, if persisted.
    pub fn session_file(&self) -> Option<&Path> {
        self.session_file.as_deref()
    }

    /// Current leaf entry id.
    pub fn leaf_id(&self) -> Option<&str> {
        self.leaf_id.as_deref()
    }

    /// Current leaf entry.
    pub fn leaf_entry(&self) -> Option<&FileEntry> {
        self.leaf_id.as_ref().and_then(|id| self.by_id.get(id))
    }

    /// Get any entry by id.
    pub fn get_entry(&self, id: &str) -> Option<&FileEntry> {
        self.by_id.get(id)
    }

    /// Direct children of an entry.
    pub fn children(&self, parent_id: &str) -> Vec<&FileEntry> {
        self.by_id
            .values()
            .filter(|e| e.parent_id() == Some(parent_id))
            .collect()
    }

    /// Label for an entry, if any.
    pub fn label(&self, id: &str) -> Option<&str> {
        self.labels_by_id.get(id).map(String::as_str)
    }

    /// Walk from an entry to the root, returning entries in root-to-leaf order.
    pub fn branch(&self, from_id: Option<&str>) -> Vec<&FileEntry> {
        let start = from_id.or(self.leaf_id.as_deref());
        let mut path = Vec::new();
        let mut current = start.and_then(|id| self.by_id.get(id));
        while let Some(entry) = current {
            path.push(entry);
            current = entry.parent_id().and_then(|id| self.by_id.get(id));
        }
        path.reverse();
        path
    }

    /// Build the resolved LLM context from the current leaf.
    pub fn build_context(&self) -> SessionContext {
        let branch = self.branch(None);
        let owned: Vec<FileEntry> = branch.into_iter().cloned().collect();
        build_session_context(&owned)
    }

    /// Session header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// All tree entries, excluding the header.
    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Session tree structure with resolved labels.
    pub fn tree(&self) -> Vec<SessionTreeNode> {
        #[derive(Debug)]
        struct Builder {
            entry: FileEntry,
            label: Option<String>,
            label_timestamp: Option<String>,
            children: Vec<usize>,
        }

        let mut builders: Vec<Option<Builder>> = Vec::new();
        let mut index_by_id: HashMap<String, usize> = HashMap::new();
        for entry in &self.entries {
            let id = entry.id().unwrap_or_default().to_string();
            index_by_id.insert(id.clone(), builders.len());
            builders.push(Some(Builder {
                entry: entry.clone(),
                label: self.labels_by_id.get(&id).cloned(),
                label_timestamp: self.label_timestamps_by_id.get(&id).cloned(),
                children: Vec::new(),
            }));
        }

        let mut root_indices: Vec<usize> = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            let id = entry.id().unwrap_or_default();
            let parent_id = entry.parent_id();
            let is_root = parent_id.is_none() || parent_id == Some(id);
            if is_root {
                root_indices.push(idx);
            } else if let Some(parent_id) = parent_id {
                if let Some(&parent_idx) = index_by_id.get(parent_id) {
                    builders[parent_idx].as_mut().unwrap().children.push(idx);
                } else {
                    root_indices.push(idx);
                }
            }
        }

        fn timestamp_ms(entry: &FileEntry) -> i64 {
            entry.timestamp().and_then(parse_timestamp_ms).unwrap_or(0)
        }

        root_indices.sort_by_key(|&idx| timestamp_ms(&builders[idx].as_ref().unwrap().entry));
        let timestamps: Vec<i64> = builders
            .iter()
            .map(|b| b.as_ref().map(|b| timestamp_ms(&b.entry)).unwrap_or(0))
            .collect();
        for builder in builders.iter_mut().flatten() {
            builder
                .children
                .sort_by_key(|&child_idx| timestamps[child_idx]);
        }

        fn build_node(idx: usize, builders: &mut Vec<Option<Builder>>) -> SessionTreeNode {
            let builder = builders[idx].take().expect("node already consumed");
            let children: Vec<SessionTreeNode> = builder
                .children
                .into_iter()
                .map(|child_idx| build_node(child_idx, builders))
                .collect();
            SessionTreeNode {
                entry: builder.entry,
                children,
                label: builder.label,
                label_timestamp: builder.label_timestamp,
            }
        }

        let mut roots: Vec<SessionTreeNode> = Vec::new();
        for idx in root_indices {
            if builders[idx].is_some() {
                roots.push(build_node(idx, &mut builders));
            }
        }

        // Any remaining nodes were not reachable; keep them as extra roots.
        for builder in builders.into_iter().flatten() {
            if !matches!(builder.entry, FileEntry::Session(_)) {
                roots.push(SessionTreeNode {
                    entry: builder.entry,
                    children: Vec::new(),
                    label: builder.label,
                    label_timestamp: builder.label_timestamp,
                });
            }
        }

        roots
    }

    /// Start a new branch from an earlier entry.
    pub fn branch_to(&mut self, branch_from_id: impl Into<String>) -> Result<(), SessionError> {
        let id = branch_from_id.into();
        if !self.by_id.contains_key(&id) {
            return Err(SessionError::NotFound(id));
        }
        self.leaf_id = Some(id);
        Ok(())
    }

    /// Reset the leaf so the next append creates a new root.
    pub fn reset_leaf(&mut self) {
        self.leaf_id = None;
    }

    /// Branch and append a summary of the abandoned path.
    pub fn branch_with_summary(
        &mut self,
        branch_from_id: Option<impl Into<String>>,
        summary: impl Into<String>,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
    ) -> Result<String, SessionError> {
        let branch_from_id = branch_from_id.map(Into::into);
        if let Some(id) = &branch_from_id {
            if !self.by_id.contains_key(id) {
                return Err(SessionError::NotFound(id.clone()));
            }
        }
        self.leaf_id = branch_from_id.clone();
        let id = generate_id(&self.id_set());
        let from_id = branch_from_id.clone().unwrap_or_else(|| "root".to_string());
        let entry = FileEntry::BranchSummary {
            id: id.clone(),
            parent_id: branch_from_id,
            timestamp: iso_timestamp(),
            from_id,
            summary: summary.into(),
            details,
            from_hook,
        };
        self.append(entry);
        Ok(id)
    }

    /// Create a new session file from the path to the specified leaf.
    pub fn create_branched_session(
        &mut self,
        leaf_id: impl Into<String>,
    ) -> Result<Option<PathBuf>, SessionError> {
        let leaf_id = leaf_id.into();
        if !self.by_id.contains_key(&leaf_id) {
            return Err(SessionError::NotFound(leaf_id));
        }

        let branch = self.branch(Some(&leaf_id));
        if branch.is_empty() {
            return Err(SessionError::NotFound(leaf_id));
        }

        let path_without_labels: Vec<FileEntry> = branch
            .into_iter()
            .filter(|e| !e.is_label())
            .cloned()
            .collect();

        let new_id = create_session_id();
        let timestamp = iso_timestamp();
        let previous_file = self.session_file.clone();

        let new_header = Header {
            version: Some(CURRENT_SESSION_VERSION),
            id: new_id.clone(),
            timestamp: timestamp.clone(),
            cwd: self.cwd.clone(),
            parent_session: if self.persist {
                previous_file
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
            } else {
                None
            },
        };

        let mut new_entries = vec![FileEntry::Session(new_header.clone())];
        new_entries.extend(path_without_labels.iter().cloned());

        // Append label entries in a chain after the path.
        let mut used_ids: std::collections::HashSet<String> = new_entries
            .iter()
            .filter_map(|e| e.id().map(String::from))
            .collect();
        let mut labels_to_write = Vec::new();
        for (target_id, label) in &self.labels_by_id {
            if path_without_labels
                .iter()
                .any(|e| e.id() == Some(target_id))
            {
                labels_to_write.push((target_id.clone(), label.clone()));
            }
        }

        let mut parent_id = path_without_labels
            .last()
            .and_then(|e| e.id().map(String::from));
        for (target_id, label) in labels_to_write {
            let label_id = generate_id(&used_ids);
            used_ids.insert(label_id.clone());
            let timestamp = self
                .label_timestamps_by_id
                .get(&target_id)
                .cloned()
                .unwrap_or_else(iso_timestamp);
            new_entries.push(FileEntry::Label {
                id: label_id.clone(),
                parent_id,
                timestamp,
                target_id,
                label: Some(label),
            });
            parent_id = Some(label_id);
        }

        self.header = new_header;
        self.entries = new_entries
            .into_iter()
            .filter(|e| !matches!(e, FileEntry::Session(_)))
            .collect();
        self.session_file = if self.persist {
            Some(
                self.session_dir
                    .join(format!("{}_{}.jsonl", safe_timestamp(&timestamp), new_id)),
            )
        } else {
            None
        };
        self.build_index();

        if self.persist {
            let has_assistant = self.entries.iter().any(|e| {
                if let FileEntry::Message { message, .. } = e {
                    is_assistant_message(message)
                } else {
                    false
                }
            });
            if has_assistant {
                self.rewrite_file()?;
                self.flushed = true;
            } else {
                self.flushed = false;
            }
        }

        Ok(self.session_file.clone())
    }

    /// Current session name from the latest `session_info` entry.
    pub fn session_name(&self) -> Option<String> {
        for entry in self.entries.iter().rev() {
            if let FileEntry::SessionInfo { name, .. } = entry {
                return name
                    .as_ref()
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty());
            }
        }
        None
    }
}

fn is_assistant_message(message: &AgentMessage) -> bool {
    matches!(message.extract_message(), Some(Message::Assistant(_)))
}

fn parse_timestamp_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}

/// Load entries from a session file, skipping malformed lines.
pub fn load_entries_from_file(path: impl AsRef<Path>) -> Vec<FileEntry> {
    let path = path.as_ref();
    if !path.exists() {
        return Vec::new();
    }
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<FileEntry>(line) {
            entries.push(entry);
        }
    }
    if entries.is_empty() {
        return entries;
    }
    if !matches!(entries.first(), Some(FileEntry::Session(h)) if !h.id.is_empty()) {
        return Vec::new();
    }
    entries
}

fn is_valid_session_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    let Some(first) = content.lines().next() else {
        return false;
    };
    serde_json::from_str::<FileEntry>(first)
        .map(|e| matches!(e, FileEntry::Session(h) if !h.id.is_empty()))
        .unwrap_or(false)
}

/// Find the most recently modified valid session file in a directory.
pub fn find_most_recent_session(dir: impl AsRef<Path>) -> Option<PathBuf> {
    let dir = dir.as_ref();
    let Ok(files) = fs::read_dir(dir) else {
        return None;
    };
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in files.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && is_valid_session_file(&path)
        {
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    candidates.push((path, mtime));
                }
            }
        }
    }
    candidates.sort_by_key(|b| std::cmp::Reverse(b.1));
    candidates.into_iter().next().map(|(p, _)| p)
}

fn extract_text_from_message(message: &AgentMessage) -> Option<String> {
    let msg = message.extract_message()?;
    let content = match msg {
        Message::User(UserMessage { content, .. }) => content,
        Message::Assistant(a) => a.content,
        Message::ToolResult(t) => t.content,
    };
    let text: String = content
        .into_iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn get_last_activity_time(entries: &[FileEntry]) -> Option<chrono::DateTime<Utc>> {
    let mut last: Option<chrono::DateTime<Utc>> = None;
    for entry in entries {
        if let FileEntry::Message {
            message, timestamp, ..
        } = entry
        {
            if let Some(msg_ts) = message.extract_message().and_then(|m| match m {
                Message::User(u) => u.timestamp,
                Message::Assistant(a) => a.timestamp,
                Message::ToolResult(t) => t.timestamp,
            }) {
                let dt = Utc.timestamp_millis_opt(msg_ts).unwrap();
                last = Some(last.map_or(dt, |l| l.max(dt)));
                continue;
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
                let dt = dt.with_timezone(&Utc);
                last = Some(last.map_or(dt, |l| l.max(dt)));
            }
        }
    }
    last
}

fn parse_header_time(header: &Header) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(&header.timestamp)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Build `SessionInfo` from a session file path.
pub fn build_session_info(path: impl AsRef<Path>) -> Option<SessionInfo> {
    let path = path.as_ref();
    let entries = load_entries_from_file(path);
    if entries.is_empty() {
        return None;
    }
    let header = match entries.first()? {
        FileEntry::Session(h) => h.clone(),
        _ => return None,
    };

    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let mtime_dt: chrono::DateTime<Utc> = mtime.into();

    let mut message_count = 0;
    let mut first_message = String::new();
    let mut all_messages: Vec<String> = Vec::new();
    let mut name: Option<String> = None;

    for entry in &entries {
        if let FileEntry::SessionInfo { name: n, .. } = entry {
            name = n
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
        if let FileEntry::Message { message, .. } = entry {
            message_count += 1;
            let Some(text) = extract_text_from_message(message) else {
                continue;
            };
            if let Some(Message::User(_)) = message.extract_message() {
                all_messages.push(text.clone());
                if first_message.is_empty() {
                    first_message = text;
                }
            } else if let Some(Message::Assistant(_)) = message.extract_message() {
                all_messages.push(text);
            }
        }
    }

    let modified =
        get_last_activity_time(&entries).unwrap_or(parse_header_time(&header).unwrap_or(mtime_dt));
    let created = parse_header_time(&header).unwrap_or(mtime_dt);

    Some(SessionInfo {
        path: path.to_path_buf(),
        id: header.id,
        cwd: header.cwd,
        name,
        parent_session_path: header.parent_session,
        created,
        modified,
        message_count,
        first_message: if first_message.is_empty() {
            "(no messages)".to_string()
        } else {
            first_message
        },
        all_messages_text: all_messages.join(" "),
    })
}

/// List sessions in a directory.
pub fn list_sessions(
    dir: impl AsRef<Path>,
    on_progress: Option<&SessionListProgress>,
) -> Result<Vec<SessionInfo>, SessionError> {
    let dir = dir.as_ref();
    let mut infos = Vec::new();
    if !dir.exists() {
        return Ok(infos);
    }
    let files: Vec<PathBuf> = fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .map(|e| e.path())
        .collect();
    let total = files.len();
    for (loaded, file) in files.into_iter().enumerate() {
        if let Some(info) = build_session_info(&file) {
            infos.push(info);
        }
        if let Some(cb) = on_progress {
            cb(loaded + 1, total);
        }
    }
    infos.sort_by_key(|b| std::cmp::Reverse(b.modified));
    Ok(infos)
}

/// List all sessions across all project directories.
pub fn list_all_sessions(
    on_progress: Option<&SessionListProgress>,
) -> Result<Vec<SessionInfo>, SessionError> {
    let root = default_sessions_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let dirs: Vec<PathBuf> = fs::read_dir(&root)?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();

    let mut all_files = Vec::new();
    for dir in &dirs {
        if let Ok(files) = fs::read_dir(dir) {
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    all_files.push(path);
                }
            }
        }
    }

    let total = all_files.len();
    let mut infos = Vec::new();
    for (loaded, file) in all_files.into_iter().enumerate() {
        if let Some(info) = build_session_info(&file) {
            infos.push(info);
        }
        if let Some(cb) = on_progress {
            cb(loaded + 1, total);
        }
    }
    infos.sort_by_key(|b| std::cmp::Reverse(b.modified));
    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortexcode_agent_types::AgentMessage;
    use cortexcode_ai_types::{Content, Message, TextContent, UserMessage};

    fn user(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::User(UserMessage {
            content: vec![Content::Text(TextContent {
                text: text.into(),
                cache_control: None,
            })],
            timestamp: None,
        }))
    }

    fn assistant(text: &str) -> AgentMessage {
        AgentMessage::from_message(Message::Assistant(cortexcode_ai_types::AssistantMessage {
            content: vec![Content::Text(TextContent {
                text: text.into(),
                cache_control: None,
            })],
            stop_reason: None,
            stop_sequence: None,
            usage: None,
            timestamp: None,
            error_message: None,
        }))
    }

    #[test]
    fn in_memory_session_appends() {
        let mut mgr = SessionManager::in_memory("/tmp");
        let m1 = mgr.append_message(user("hello"));
        let m2 = mgr.append_message(assistant("hi"));
        assert_eq!(mgr.entries().len(), 2);
        assert_eq!(mgr.leaf_id(), Some(m2.as_str()));
        assert!(mgr.branch(None).len() == 2);
        assert!(mgr.get_entry(&m1).is_some());
    }

    #[test]
    fn persisted_session_creates_file_on_assistant() {
        let dir = std::env::temp_dir().join(format!("cortex-session-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut mgr = SessionManager::create("/tmp", Some(dir.clone()));
        mgr.append_message(user("hello"));
        assert!(!mgr.session_file().unwrap().exists());
        mgr.append_message(assistant("hi"));
        assert!(mgr.session_file().unwrap().exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branching_creates_two_roots() {
        let mut mgr = SessionManager::in_memory("/tmp");
        let m1 = mgr.append_message(user("a"));
        let _m2 = mgr.append_message(user("b"));
        mgr.branch_to(&m1).unwrap();
        let _m3 = mgr.append_message(user("c"));
        let tree = mgr.tree();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 2);
    }

    #[test]
    fn labels_roundtrip() {
        let mut mgr = SessionManager::in_memory("/tmp");
        let m1 = mgr.append_message(user("a"));
        mgr.append_label_change(&m1, Some("bookmark")).unwrap();
        assert_eq!(mgr.label(&m1), Some("bookmark"));
        mgr.append_label_change(&m1, None::<String>).unwrap();
        assert_eq!(mgr.label(&m1), None);
    }

    #[test]
    fn session_name_from_info_entries() {
        let mut mgr = SessionManager::in_memory("/tmp");
        mgr.append_session_info("My Session");
        assert_eq!(mgr.session_name(), Some("My Session".to_string()));
    }
}

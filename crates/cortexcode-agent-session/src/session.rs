//! `Session`: typed appends and context building over a [`SessionStorage`].
//!
//! Port of hoocode `packages/agent/src/harness/session/session.ts`.

use cortexcode_agent_types::AgentMessage;

use crate::context::{build_session_context, SessionContext};
use crate::entry::{CustomMessageContent, FileEntry};
use crate::storage::{SessionStorage, SessionTreeEntry};
use crate::{create_timestamp, SessionError};

/// The summary written when [`Session::move_to`] leaves a branch.
#[derive(Debug, Clone, Default)]
pub struct BranchSummaryInput {
    pub summary: String,
    pub details: Option<serde_json::Value>,
    pub from_hook: Option<bool>,
}

/// `Session<TMetadata>`.
#[derive(Debug, Clone)]
pub struct Session<S: SessionStorage> {
    storage: S,
}

impl<S: SessionStorage> Session<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }

    pub fn metadata(&self) -> S::Metadata {
        self.storage.metadata()
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.storage
    }

    pub fn into_storage(self) -> S {
        self.storage
    }

    pub fn leaf_id(&self) -> Option<String> {
        self.storage.leaf_id()
    }

    pub fn entry(&self, id: &str) -> Option<&SessionTreeEntry> {
        self.storage.entry(id)
    }

    pub fn entries(&self) -> Vec<SessionTreeEntry> {
        self.storage.entries()
    }

    /// `getBranch(fromId?)`: root-to-entry path, from the leaf by default.
    pub fn branch(&self, from_id: Option<&str>) -> Vec<SessionTreeEntry> {
        let leaf = match from_id {
            Some(id) => Some(id.to_string()),
            None => self.storage.leaf_id(),
        };
        self.storage.path_to_root(leaf.as_deref())
    }

    /// `buildContext()`: the context for the current branch.
    pub fn build_context(&self) -> SessionContext {
        build_session_context(&self.branch(None))
    }

    pub fn label(&self, id: &str) -> Option<&str> {
        self.storage.label(id)
    }

    /// `getSessionName()`: the trimmed name of the last `session_info` entry.
    pub fn session_name(&self) -> Option<String> {
        match self.storage.find_entries("session_info").last() {
            Some(FileEntry::SessionInfo { name: Some(n), .. }) if !n.trim().is_empty() => {
                Some(n.trim().to_string())
            }
            _ => None,
        }
    }

    /// Stamp a new entry with id, parent (the leaf) and timestamp, then append it.
    fn append_with(
        &mut self,
        make: impl FnOnce(String, Option<String>, String) -> SessionTreeEntry,
    ) -> Result<String, SessionError> {
        let id = self.storage.create_entry_id();
        let entry = make(id.clone(), self.storage.leaf_id(), create_timestamp());
        self.storage.append_entry(entry)?;
        Ok(id)
    }

    pub fn append_message(&mut self, message: AgentMessage) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::Message {
            id,
            parent_id,
            timestamp,
            message,
        })
    }

    pub fn append_thinking_level_change(
        &mut self,
        thinking_level: &str,
    ) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::ThinkingLevelChange {
            id,
            parent_id,
            timestamp,
            thinking_level: thinking_level.to_string(),
        })
    }

    pub fn append_model_change(
        &mut self,
        provider: &str,
        model_id: &str,
    ) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::ModelChange {
            id,
            parent_id,
            timestamp,
            provider: provider.to_string(),
            model_id: model_id.to_string(),
        })
    }

    pub fn append_compaction(
        &mut self,
        summary: &str,
        first_kept_entry_id: &str,
        tokens_before: u64,
        details: Option<serde_json::Value>,
        from_hook: Option<bool>,
        tokens_after: Option<u64>,
    ) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::Compaction {
            id,
            parent_id,
            timestamp,
            summary: summary.to_string(),
            first_kept_entry_id: first_kept_entry_id.to_string(),
            tokens_before,
            tokens_after,
            details,
            from_hook,
        })
    }

    pub fn append_custom_entry(
        &mut self,
        custom_type: &str,
        data: Option<serde_json::Value>,
    ) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::Custom {
            id,
            parent_id,
            timestamp,
            custom_type: custom_type.to_string(),
            data,
        })
    }

    pub fn append_custom_message_entry(
        &mut self,
        custom_type: &str,
        content: impl Into<CustomMessageContent>,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> Result<String, SessionError> {
        let content = content.into();
        self.append_with(|id, parent_id, timestamp| FileEntry::CustomMessage {
            id,
            parent_id,
            timestamp,
            custom_type: custom_type.to_string(),
            content,
            display,
            details,
        })
    }

    /// `appendLabel`: `None` clears the label. `Entry <id> not found` for an
    /// unknown target.
    pub fn append_label(
        &mut self,
        target_id: &str,
        label: Option<&str>,
    ) -> Result<String, SessionError> {
        if self.storage.entry(target_id).is_none() {
            return Err(SessionError::EntryNotFound(target_id.to_string()));
        }
        self.append_with(|id, parent_id, timestamp| FileEntry::Label {
            id,
            parent_id,
            timestamp,
            target_id: target_id.to_string(),
            label: label.map(str::to_string),
        })
    }

    pub fn append_session_name(&mut self, name: &str) -> Result<String, SessionError> {
        self.append_with(|id, parent_id, timestamp| FileEntry::SessionInfo {
            id,
            parent_id,
            timestamp,
            name: Some(name.trim().to_string()),
            color: None,
        })
    }

    /// `moveTo(entryId, summary?)`: move the leaf (`None` = before the root).
    /// With a summary, appends a `branch_summary` child of the new leaf and
    /// returns its id.
    pub fn move_to(
        &mut self,
        entry_id: Option<&str>,
        summary: Option<BranchSummaryInput>,
    ) -> Result<Option<String>, SessionError> {
        if let Some(id) = entry_id {
            if self.storage.entry(id).is_none() {
                return Err(SessionError::EntryNotFound(id.to_string()));
            }
        }
        self.storage.set_leaf_id(entry_id)?;
        let Some(summary) = summary else {
            return Ok(None);
        };
        let id = self.storage.create_entry_id();
        self.storage.append_entry(FileEntry::BranchSummary {
            id: id.clone(),
            parent_id: entry_id.map(str::to_string),
            timestamp: create_timestamp(),
            from_id: entry_id.unwrap_or("root").to_string(),
            summary: summary.summary,
            details: summary.details,
            from_hook: summary.from_hook,
        })?;
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InMemorySessionStorage, JsonlSessionStorage};
    use crate::test_utils::{assistant_message, temp_dir, user_message, TempDir};

    fn roles(context: &SessionContext) -> Vec<&'static str> {
        context.messages.iter().map(|m| m.role()).collect()
    }

    /// `runSessionSuite` in session.test.ts, once per storage backend.
    fn suite<S: SessionStorage>(create: impl Fn() -> S, inspect: impl Fn(&S)) {
        // appends messages and builds context in order
        let mut session = Session::new(create());
        session.append_message(user_message("one")).unwrap();
        session.append_message(assistant_message("two")).unwrap();
        assert_eq!(roles(&session.build_context()), ["user", "assistant"]);

        // tracks model and thinking level changes
        let mut session = Session::new(create());
        session.append_message(user_message("one")).unwrap();
        session.append_model_change("openai", "gpt-4.1").unwrap();
        session.append_thinking_level_change("high").unwrap();
        let context = session.build_context();
        assert_eq!(context.thinking_level, "high");
        let model = context.model.unwrap();
        assert_eq!(
            (model.provider.as_str(), model.model_id.as_str()),
            ("openai", "gpt-4.1")
        );

        // supports branching by moving the leaf and appending a new branch
        let mut session = Session::new(create());
        let user1 = session.append_message(user_message("one")).unwrap();
        let assistant1 = session.append_message(assistant_message("two")).unwrap();
        session.append_message(user_message("three")).unwrap();
        session.move_to(Some(&user1), None).unwrap();
        session
            .append_message(assistant_message("branched"))
            .unwrap();
        let branch: Vec<String> = session
            .branch(None)
            .iter()
            .filter_map(|e| e.id().map(str::to_string))
            .collect();
        assert!(branch.contains(&user1));
        assert!(!branch.contains(&assistant1));
        assert_eq!(roles(&session.build_context()), ["user", "assistant"]);

        // supports moving the leaf to root
        let mut session = Session::new(create());
        session.append_message(user_message("one")).unwrap();
        session.move_to(None, None).unwrap();
        assert_eq!(session.leaf_id(), None);
        assert!(session.build_context().messages.is_empty());

        // reconstructs compaction summaries in context
        let mut session = Session::new(create());
        session.append_message(user_message("one")).unwrap();
        session.append_message(assistant_message("two")).unwrap();
        let user2 = session.append_message(user_message("three")).unwrap();
        session.append_message(assistant_message("four")).unwrap();
        session
            .append_compaction("summary", &user2, 1234, None, None, None)
            .unwrap();
        session.append_message(user_message("five")).unwrap();
        let context = session.build_context();
        assert_eq!(context.messages[0].role(), "compactionSummary");
        assert_eq!(context.messages.len(), 4);

        // supports moving with branch summary entries in context
        let mut session = Session::new(create());
        let user1 = session.append_message(user_message("one")).unwrap();
        let summary_id = session
            .move_to(
                Some(&user1),
                Some(BranchSummaryInput {
                    summary: "summary text".into(),
                    ..Default::default()
                }),
            )
            .unwrap()
            .expect("a summary entry id");
        match session.entry(&summary_id) {
            Some(FileEntry::BranchSummary {
                parent_id, from_id, ..
            }) => {
                assert_eq!(parent_id.as_deref(), Some(user1.as_str()));
                assert_eq!(from_id, &user1);
            }
            other => panic!("expected a branch_summary entry, got {other:?}"),
        }
        assert_eq!(session.build_context().messages[1].role(), "branchSummary");

        // supports custom message entries in context
        let mut session = Session::new(create());
        session.append_message(user_message("one")).unwrap();
        session
            .append_custom_message_entry(
                "custom",
                "hello",
                true,
                Some(serde_json::json!({"ok": true})),
            )
            .unwrap();
        assert_eq!(session.build_context().messages[1].role(), "custom");

        // supports labels and session info entries without affecting context
        let mut session = Session::new(create());
        let user1 = session.append_message(user_message("one")).unwrap();
        session.append_label(&user1, Some("checkpoint")).unwrap();
        session.append_session_name("name").unwrap();
        let entries = session.entries();
        assert!(entries.iter().any(|e| e.is_label()));
        assert!(entries
            .iter()
            .any(|e| matches!(e, FileEntry::SessionInfo { .. })));
        assert_eq!(session.label(&user1), Some("checkpoint"));
        assert_eq!(session.session_name().as_deref(), Some("name"));
        assert_eq!(session.build_context().messages.len(), 1);

        // rejects labels for missing entries
        let mut session = Session::new(create());
        assert_eq!(
            session
                .append_label("missing", Some("checkpoint"))
                .unwrap_err()
                .to_string(),
            "Entry missing not found"
        );

        // persists leaf changes and appended entries via storage
        let mut session = Session::new(create());
        let user1 = session.append_message(user_message("one")).unwrap();
        session.append_message(assistant_message("two")).unwrap();
        session.append_label(&user1, Some("checkpoint")).unwrap();
        session.append_session_name("name").unwrap();
        session.move_to(Some(&user1), None).unwrap();
        session
            .append_message(assistant_message("branched"))
            .unwrap();
        let session2 = Session::new(session.into_storage());
        assert_eq!(roles(&session2.build_context()), ["user", "assistant"]);
        assert_eq!(session2.label(&user1), Some("checkpoint"));
        assert_eq!(session2.session_name().as_deref(), Some("name"));
        inspect(session2.storage());
    }

    #[test]
    fn session_with_in_memory_storage() {
        suite(InMemorySessionStorage::default, |_| {});
    }

    #[test]
    fn session_with_jsonl_storage() {
        let dirs: std::cell::RefCell<Vec<TempDir>> = Default::default();
        suite(
            || {
                let dir = temp_dir();
                let path = dir.path().join("session.jsonl");
                let cwd = dir.path().to_string_lossy().to_string();
                dirs.borrow_mut().push(dir);
                JsonlSessionStorage::create(path, &cwd, "session-1", None).unwrap()
            },
            |storage| {
                let content = std::fs::read_to_string(storage.path()).unwrap();
                let lines: Vec<&str> = content.trim().split('\n').collect();
                assert!(lines.len() > 1);
                let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
                assert_eq!(header["type"], "session");
                assert_eq!(header["version"], 3);
                for line in &lines[1..] {
                    let entry: serde_json::Value = serde_json::from_str(line).unwrap();
                    assert_ne!(entry["type"], "entry");
                    assert_ne!(entry["type"], "leaf");
                    assert!(entry["id"].is_string());
                }
                // A reopened file resumes at the last appended entry.
                let reopened = Session::new(JsonlSessionStorage::open(storage.path()).unwrap());
                assert_eq!(roles(&reopened.build_context()), ["user", "assistant"]);
            },
        );
    }
}

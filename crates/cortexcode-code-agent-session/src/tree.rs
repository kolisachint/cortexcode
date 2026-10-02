//! `core/agent-session-tree-navigation.ts`: move to another node of the
//! session tree in the same session file (unlike fork), optionally
//! summarizing the abandoned branch at the new position.

use cortexcode_agent_compaction::{
    collect_entries_for_branch_summary, generate_branch_summary, BranchEntrySource,
    GenerateBranchSummaryOptions,
};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::AbortSignal;
use cortexcode_code_session::{FileEntry, SessionManager};

use crate::hooks::{SessionEvent, TreePreparation};
use crate::session::{AgentSession, AgentSessionError};
use crate::stats::extract_user_message_text;

/// `NavigateTreeOptions`.
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeOptions {
    /// Summarize the abandoned branch.
    pub summarize: bool,
    /// Custom instructions for the summarizer.
    pub custom_instructions: Option<String>,
    /// `custom_instructions` replace the default prompt.
    pub replace_instructions: Option<bool>,
    /// Label for the branch summary entry (or the target, without a summary).
    pub label: Option<String>,
}

/// `NavigateTreeResult`.
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeResult {
    /// The selected user message's text, for the editor.
    pub editor_text: Option<String>,
    pub cancelled: bool,
    pub aborted: bool,
    pub summary_entry: Option<FileEntry>,
}

impl NavigateTreeResult {
    fn cancelled(aborted: bool) -> Self {
        Self {
            cancelled: true,
            aborted,
            ..Default::default()
        }
    }
}

/// The session manager as branch collection reads it.
struct ManagerSource<'a>(&'a SessionManager);

impl BranchEntrySource for ManagerSource<'_> {
    fn get_branch(&self, id: &str) -> Vec<FileEntry> {
        self.0.branch(Some(id)).into_iter().cloned().collect()
    }
    fn get_entry(&self, id: &str) -> Option<FileEntry> {
        self.0.get_entry(id).cloned()
    }
}

/// Clears the branch-summary abort signal when navigation ends.
struct SummaryGuard<'a>(&'a AgentSession);

impl Drop for SummaryGuard<'_> {
    fn drop(&mut self) {
        self.0.compaction_state().branch_summary_abort = None;
    }
}

impl AgentSession {
    /// Cancel in-progress branch summarization.
    pub fn abort_branch_summary(&self) {
        if let Some(signal) = &self.compaction_state().branch_summary_abort {
            signal.abort();
        }
    }

    /// Navigate to `target_id` in the session tree (`navigateTree`). A user
    /// or custom message puts its text back in the editor and moves the leaf
    /// to its parent; any other entry becomes the leaf.
    pub async fn navigate_tree(
        &self,
        target_id: &str,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeResult, AgentSessionError> {
        let extensions = self.extensions().clone();
        let old_leaf_id = self.session_manager().leaf_id().map(str::to_string);
        if old_leaf_id.as_deref() == Some(target_id) {
            return Ok(NavigateTreeResult::default());
        }
        if options.summarize && self.model().is_none() {
            return Err(AgentSessionError(
                "No model available for summarization".into(),
            ));
        }
        let (target_entry, collected) = {
            let manager = self.session_manager();
            let Some(target) = manager.get_entry(target_id).cloned() else {
                return Err(AgentSessionError(format!("Entry {target_id} not found")));
            };
            let collected = collect_entries_for_branch_summary(
                &ManagerSource(&manager),
                old_leaf_id.as_deref(),
                target_id,
            );
            (target, collected)
        };

        let mut custom_instructions = options.custom_instructions.clone();
        let mut replace_instructions = options.replace_instructions;
        let mut label = options.label.clone();

        let signal = AbortSignal::new();
        self.compaction_state().branch_summary_abort = Some(signal.clone());
        let _guard = SummaryGuard(self);

        let mut extension_summary = None;
        if extensions.has_handlers("session_before_tree") {
            let result = extensions
                .emit_session_event(SessionEvent::BeforeTree {
                    preparation: TreePreparation {
                        target_id: target_id.to_string(),
                        old_leaf_id: old_leaf_id.clone(),
                        common_ancestor_id: collected.common_ancestor_id.clone(),
                        entries_to_summarize: collected.entries.clone(),
                        user_wants_summary: options.summarize,
                        custom_instructions: custom_instructions.clone(),
                        replace_instructions,
                        label: label.clone(),
                    },
                    signal: signal.clone(),
                })
                .await;
            if result.cancel {
                return Ok(NavigateTreeResult::cancelled(false));
            }
            if options.summarize {
                extension_summary = result.summary;
            }
            if result.custom_instructions.is_some() {
                custom_instructions = result.custom_instructions;
            }
            if result.replace_instructions.is_some() {
                replace_instructions = result.replace_instructions;
            }
            if result.label.is_some() {
                label = result.label;
            }
        }
        let from_extension = extension_summary.is_some();

        let (summary_text, summary_details) = if let Some(summary) = extension_summary {
            (Some(summary.summary), summary.details)
        } else if options.summarize && !collected.entries.is_empty() {
            let model = self.model().expect("checked above");
            let (api_key, headers) = self.required_request_auth(&model)?;
            let reserve_tokens = self.settings().branch_summary_settings().reserve_tokens;
            let options = GenerateBranchSummaryOptions {
                model,
                api_key: Some(api_key),
                headers,
                // OpenCode Go routes on x-opencode-session, which the branch
                // summary request needs because it is a request of its own.
                session_id: Some(self.session_id()),
                signal: Some(signal.clone()),
                custom_instructions: custom_instructions.clone(),
                replace_instructions: replace_instructions.unwrap_or(false),
                reserve_tokens: Some(reserve_tokens),
            };
            // A provider may not observe the signal promptly; abort wins.
            let result = tokio::select! {
                result = generate_branch_summary(&collected.entries, &options) => result,
                _ = signal.cancelled() => return Ok(NavigateTreeResult::cancelled(true)),
            }
            .map_err(AgentSessionError)?;
            if result.aborted || signal.aborted() {
                return Ok(NavigateTreeResult::cancelled(true));
            }
            if let Some(error) = result.error {
                return Err(AgentSessionError(error));
            }
            let details = serde_json::json!({
                "readFiles": result.read_files.unwrap_or_default(),
                "modifiedFiles": result.modified_files.unwrap_or_default(),
            });
            (result.summary, Some(details))
        } else {
            (None, None)
        };

        let (new_leaf_id, editor_text) = match &target_entry {
            FileEntry::Message {
                parent_id,
                message: AgentMessage::User(user),
                ..
            } => (
                parent_id.clone(),
                Some(extract_user_message_text(&user.content)),
            ),
            FileEntry::CustomMessage {
                parent_id, content, ..
            } => (parent_id.clone(), Some(extract_user_message_text(content))),
            _ => (Some(target_id.to_string()), None),
        };

        let summary_entry = {
            let mut manager = self.session_manager();
            let error = |e: cortexcode_code_session::SessionError| AgentSessionError(e.to_string());
            let summary_entry = match &summary_text {
                Some(text) => {
                    let summary_id = manager
                        .branch_with_summary(
                            new_leaf_id.clone(),
                            text.clone(),
                            summary_details,
                            Some(from_extension),
                        )
                        .map_err(error)?;
                    if let Some(label) = &label {
                        manager
                            .append_label_change(summary_id.clone(), Some(label.clone()))
                            .map_err(error)?;
                    }
                    manager.get_entry(&summary_id).cloned()
                }
                None => {
                    match &new_leaf_id {
                        None => manager.reset_leaf(),
                        Some(id) => manager.branch_to(id.clone()).map_err(error)?,
                    }
                    if let Some(label) = &label {
                        manager
                            .append_label_change(target_id, Some(label.clone()))
                            .map_err(error)?;
                    }
                    None
                }
            };
            let messages = manager.build_context().messages;
            self.agent().set_messages(messages);
            summary_entry
        };

        let new_leaf = self.session_manager().leaf_id().map(str::to_string);
        extensions
            .emit_session_event(SessionEvent::Tree {
                new_leaf_id: new_leaf,
                old_leaf_id,
                summary_entry: summary_entry.clone(),
                from_extension: summary_text.as_ref().map(|_| from_extension),
            })
            .await;

        Ok(NavigateTreeResult {
            editor_text,
            cancelled: false,
            aborted: false,
            summary_entry,
        })
    }
}

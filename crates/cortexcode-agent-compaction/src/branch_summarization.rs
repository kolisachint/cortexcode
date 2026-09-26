//! Branch summarization for tree navigation: hoocode
//! `harness/compaction/branch-summarization.ts` (v0.5.89). Leaving a branch
//! summarizes it so its context is not lost.

use std::collections::{HashMap, HashSet};

use cortexcode_agent_harness::{
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
    create_custom_message,
};
use cortexcode_agent_session::FileEntry;
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{
    AbortSignal, Content, Context, Message, Model, SimpleStreamOptions, StopReason, UserMessage,
};
use serde::{Deserialize, Serialize};

use crate::compaction::{estimate_tokens, response_text};
use crate::utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
    serialize_conversation, FileOperations, SUMMARIZATION_SYSTEM_PROMPT,
};

/// `BranchSummaryResult`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BranchSummaryResult {
    pub summary: Option<String>,
    pub read_files: Option<Vec<String>>,
    pub modified_files: Option<Vec<String>>,
    pub aborted: bool,
    pub error: Option<String>,
}

/// `BranchSummaryDetails`: what a branch summary entry records about files.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// `BranchPreparation`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BranchPreparation {
    /// Messages to summarize, oldest first.
    pub messages: Vec<AgentMessage>,
    pub file_ops: FileOperations,
    /// Estimated tokens of `messages`.
    pub total_tokens: u64,
}

/// `CollectEntriesResult`.
#[derive(Debug, Clone, Default)]
pub struct CollectEntriesResult {
    /// Entries to summarize, oldest first.
    pub entries: Vec<FileEntry>,
    /// Deepest entry on both the old and the new path.
    pub common_ancestor_id: Option<String>,
}

/// `GenerateBranchSummaryOptions`.
#[derive(Debug, Clone)]
pub struct GenerateBranchSummaryOptions {
    pub model: Model,
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub signal: Option<AbortSignal>,
    pub custom_instructions: Option<String>,
    /// `custom_instructions` replace the default prompt instead of extending it.
    pub replace_instructions: bool,
    /// Tokens reserved for prompt and response (default 16384).
    pub reserve_tokens: Option<u64>,
}

/// `BranchEntrySource`: the read access branch collection needs.
pub trait BranchEntrySource {
    /// Root-to-`id` path.
    fn get_branch(&self, id: &str) -> Vec<FileEntry>;
    fn get_entry(&self, id: &str) -> Option<FileEntry>;
}

/// `collectEntriesForBranchSummary`: the entries from `old_leaf_id` back to
/// the common ancestor with `target_id`, oldest first (compaction entries
/// included).
pub fn collect_entries_for_branch_summary(
    session: &dyn BranchEntrySource,
    old_leaf_id: Option<&str>,
    target_id: &str,
) -> CollectEntriesResult {
    let Some(old_leaf_id) = old_leaf_id.filter(|id| !id.is_empty()) else {
        return CollectEntriesResult::default();
    };
    let old_path: HashSet<String> = session
        .get_branch(old_leaf_id)
        .iter()
        .filter_map(|e| e.id().map(str::to_string))
        .collect();
    let common_ancestor_id = session
        .get_branch(target_id)
        .iter()
        .rev()
        .filter_map(|e| e.id())
        .find(|id| old_path.contains(*id))
        .map(str::to_string);

    let mut entries = Vec::new();
    let mut current = Some(old_leaf_id.to_string());
    while let Some(id) = current.filter(|id| Some(id) != common_ancestor_id.as_ref()) {
        let Some(entry) = session.get_entry(&id) else {
            break;
        };
        current = entry.parent_id().map(str::to_string);
        entries.push(entry);
    }
    entries.reverse();
    CollectEntriesResult {
        entries,
        common_ancestor_id,
    }
}

/// `getMessageFromEntry` for branches: tool results are left out (the call
/// carries their context) and compactions become summaries.
fn get_message_from_entry(entry: &FileEntry) -> Option<AgentMessage> {
    match entry {
        FileEntry::Message {
            message: AgentMessage::ToolResult(_),
            ..
        } => None,
        FileEntry::Message { message, .. } => Some(message.clone()),
        FileEntry::CustomMessage {
            custom_type,
            content,
            display,
            details,
            timestamp,
            ..
        } => Some(AgentMessage::Custom(create_custom_message(
            custom_type.clone(),
            content.clone(),
            *display,
            details.clone(),
            timestamp,
        ))),
        FileEntry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } => Some(AgentMessage::BranchSummary(create_branch_summary_message(
            summary.clone(),
            from_id.clone(),
            timestamp,
        ))),
        FileEntry::Compaction {
            summary,
            tokens_before,
            tokens_after,
            timestamp,
            ..
        } => Some(AgentMessage::CompactionSummary(
            create_compaction_summary_message(
                summary.clone(),
                *tokens_before,
                timestamp,
                *tokens_after,
            ),
        )),
        _ => None,
    }
}

/// `prepareBranchEntries`: the newest messages that fit `token_budget` (0 =
/// no limit), with the file operations of every entry (including earlier
/// non-hook branch summaries).
pub fn prepare_branch_entries(entries: &[FileEntry], token_budget: i64) -> BranchPreparation {
    let mut file_ops = create_file_ops();
    for entry in entries {
        if let FileEntry::BranchSummary {
            details: Some(details),
            from_hook,
            ..
        } = entry
        {
            if *from_hook != Some(true) {
                for (key, set) in [
                    ("readFiles", &mut file_ops.read),
                    ("modifiedFiles", &mut file_ops.edited),
                ] {
                    if let Some(items) = details[key].as_array() {
                        set.extend(items.iter().filter_map(|v| v.as_str().map(str::to_string)));
                    }
                }
            }
        }
    }

    let mut messages = Vec::new();
    let mut total_tokens: u64 = 0;
    for entry in entries.iter().rev() {
        let Some(message) = get_message_from_entry(entry) else {
            continue;
        };
        extract_file_ops_from_message(&message, &mut file_ops);
        let tokens = estimate_tokens(&message);
        if token_budget > 0 && (total_tokens + tokens) as i64 > token_budget {
            // A summary is important context: keep it if under 90% of budget.
            let is_summary = matches!(
                entry,
                FileEntry::Compaction { .. } | FileEntry::BranchSummary { .. }
            );
            if is_summary && (total_tokens as f64) < token_budget as f64 * 0.9 {
                messages.push(message);
                total_tokens += tokens;
            }
            break;
        }
        messages.push(message);
        total_tokens += tokens;
    }
    messages.reverse();
    BranchPreparation {
        messages,
        file_ops,
        total_tokens,
    }
}

const BRANCH_SUMMARY_PREAMBLE: &str =
    "The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n";

const BRANCH_SUMMARY_PROMPT: &str =
    "Create a structured summary of this conversation branch for context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

/// `generateBranchSummary`: summarize abandoned `entries` (oldest first)
/// within the model's window minus `reserve_tokens`. An `Err` is a request
/// that could not be made; provider failures come back in the result.
pub async fn generate_branch_summary(
    entries: &[FileEntry],
    options: &GenerateBranchSummaryOptions,
) -> Result<BranchSummaryResult, String> {
    let reserve_tokens = options.reserve_tokens.unwrap_or(16384);
    let context_window = if options.model.context_window > 0 {
        options.model.context_window
    } else {
        128_000
    };
    let token_budget = context_window as i64 - reserve_tokens as i64;
    let preparation = prepare_branch_entries(entries, token_budget);
    if preparation.messages.is_empty() {
        return Ok(BranchSummaryResult {
            summary: Some("No content to summarize".to_string()),
            ..Default::default()
        });
    }

    let conversation = serialize_conversation(&convert_to_llm(&preparation.messages));
    let custom = options
        .custom_instructions
        .as_deref()
        .filter(|s| !s.is_empty());
    let instructions = match custom {
        Some(custom) if options.replace_instructions => custom.to_string(),
        Some(custom) => format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {custom}"),
        None => BRANCH_SUMMARY_PROMPT.to_string(),
    };
    let prompt_text = format!("<conversation>\n{conversation}\n</conversation>\n\n{instructions}");
    let context = Context::new(
        SUMMARIZATION_SYSTEM_PROMPT.to_string(),
        vec![Message::User(UserMessage {
            content: vec![Content::text(prompt_text)].into(),
            timestamp: cortexcode_ai_types::now_ms(),
        })],
        vec![],
    );
    let stream_options = SimpleStreamOptions {
        api_key: options.api_key.clone(),
        headers: options.headers.clone(),
        signal: options.signal.clone(),
        max_tokens: Some(2048),
        ..Default::default()
    };
    let response =
        cortexcode_ai_registry::complete_simple(options.model.clone(), context, stream_options)
            .await
            .map_err(|e| e.to_string())?;
    match response.stop_reason {
        StopReason::Aborted => {
            return Ok(BranchSummaryResult {
                aborted: true,
                ..Default::default()
            })
        }
        StopReason::Error => {
            return Ok(BranchSummaryResult {
                error: Some(
                    response
                        .error_message
                        .clone()
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| "Summarization failed".to_string()),
                ),
                ..Default::default()
            })
        }
        _ => {}
    }

    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    let summary = format!(
        "{BRANCH_SUMMARY_PREAMBLE}{}{}",
        response_text(&response),
        format_file_operations(&read_files, &modified_files)
    );
    Ok(BranchSummaryResult {
        summary: Some(summary),
        read_files: Some(read_files),
        modified_files: Some(modified_files),
        ..Default::default()
    })
}

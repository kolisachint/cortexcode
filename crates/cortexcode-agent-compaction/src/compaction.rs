//! Context compaction: hoocode `harness/compaction/compaction.ts` (v0.5.89).
//! Pure functions over session tree entries; the session layer persists the
//! result and reloads.

use std::collections::HashMap;

use cortexcode_agent_harness::{
    convert_to_llm, create_branch_summary_message, create_compaction_summary_message,
    create_custom_message,
};
use cortexcode_agent_session::{build_session_context, FileEntry};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{
    AbortSignal, Content, Context, Message, Model, SimpleStreamOptions, StopReason, ThinkingLevel,
    Usage, UserContent, UserMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::utils::{
    compute_file_lists, create_file_ops, extract_file_ops_from_message, format_file_operations,
    js_len, serialize_conversation, FileOperations, SUMMARIZATION_SYSTEM_PROMPT,
};

/// `CompactionDetails`: what a compaction entry records about files.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// String entries of `details[key]`, when it is an array.
fn detail_strings(details: &Value, key: &str) -> Vec<String> {
    details[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `extractFileOperations`: the previous (non-hook) compaction's files plus
/// the tool calls in `messages`.
fn extract_file_operations(
    messages: &[AgentMessage],
    entries: &[FileEntry],
    prev_compaction_index: Option<usize>,
) -> FileOperations {
    let mut file_ops = create_file_ops();
    if let Some(FileEntry::Compaction {
        details: Some(details),
        from_hook,
        ..
    }) = prev_compaction_index.map(|i| &entries[i])
    {
        if *from_hook != Some(true) {
            file_ops.read.extend(detail_strings(details, "readFiles"));
            file_ops
                .edited
                .extend(detail_strings(details, "modifiedFiles"));
        }
    }
    for message in messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }
    file_ops
}

/// `getMessageFromEntry`: the agent message an entry contributes, if any.
pub(crate) fn get_message_from_entry(entry: &FileEntry) -> Option<AgentMessage> {
    match entry {
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

fn get_message_from_entry_for_compaction(entry: &FileEntry) -> Option<AgentMessage> {
    match entry {
        FileEntry::Compaction { .. } => None,
        other => get_message_from_entry(other),
    }
}

/// `CompactionResult`; the session layer adds the entry id and parent.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: u64,
    /// Estimated context tokens after compaction.
    pub tokens_after: Option<u64>,
    /// [`CompactionDetails`] (extensions may store their own shape).
    pub details: Option<Value>,
}

/// `CompactionSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
    /// Optional soft trigger: compact past this fraction of the window even
    /// before the reserve rule fires (only values in (0, 1) apply).
    pub max_context_ratio: Option<f64>,
}

/// `DEFAULT_COMPACTION_SETTINGS`.
pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
    max_context_ratio: None,
};

impl Default for CompactionSettings {
    fn default() -> Self {
        DEFAULT_COMPACTION_SETTINGS
    }
}

/// `calculateContextTokens`: `totalTokens`, else the sum of the parts.
pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        return usage.total_tokens;
    }
    usage.input + usage.output + usage.cache_read + usage.cache_write
}

/// `getAssistantUsage`: usage of an assistant message that did not abort or
/// fail.
fn get_assistant_usage(message: &AgentMessage) -> Option<&Usage> {
    match message {
        AgentMessage::Assistant(a)
            if !matches!(a.stop_reason, StopReason::Aborted | StopReason::Error) =>
        {
            Some(&a.usage)
        }
        _ => None,
    }
}

/// `getLastAssistantUsage`.
pub fn get_last_assistant_usage(entries: &[FileEntry]) -> Option<Usage> {
    entries.iter().rev().find_map(|entry| match entry {
        FileEntry::Message { message, .. } => get_assistant_usage(message).cloned(),
        _ => None,
    })
}

/// `ContextUsageEstimate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageEstimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    pub last_usage_index: Option<usize>,
}

/// `estimateContextTokens`: the last assistant usage plus estimates for the
/// messages after it (all estimated without one).
pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let last = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, m)| get_assistant_usage(m).map(|u| (u, i)));
    let Some((usage, index)) = last else {
        let estimated = messages.iter().map(estimate_tokens).sum();
        return ContextUsageEstimate {
            tokens: estimated,
            usage_tokens: 0,
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };
    let usage_tokens = calculate_context_tokens(usage);
    let trailing_tokens: u64 = messages[index + 1..].iter().map(estimate_tokens).sum();
    ContextUsageEstimate {
        tokens: usage_tokens + trailing_tokens,
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

/// `shouldCompact`: past `window - reserve`, or past `ratio * window` when a
/// ratio in (0, 1) is set, whichever is lower.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    let mut trigger = context_window as i64 - settings.reserve_tokens as i64;
    if let Some(ratio) = settings.max_context_ratio.filter(|r| *r > 0.0 && *r < 1.0) {
        trigger = trigger.min((ratio * context_window as f64).floor() as i64);
    }
    context_tokens as i64 > trigger
}

fn ceil_div4(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

/// Text (and image) characters of user-style content.
fn content_chars(content: &UserContent, images: bool) -> usize {
    match content {
        UserContent::Text(text) => js_len(text),
        UserContent::Blocks(blocks) => blocks_chars(blocks, images),
    }
}

fn blocks_chars(blocks: &[Content], images: bool) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Content::Text(t) => js_len(&t.text),
            // Images count as 4800 characters (1200 tokens).
            Content::Image(_) if images => 4800,
            _ => 0,
        })
        .sum()
}

/// `estimateTokens`: characters / 4, rounded up (an overestimate).
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    let chars = match message {
        AgentMessage::User(u) => content_chars(&u.content, false),
        AgentMessage::Assistant(a) => a
            .content
            .iter()
            .map(|block| match block {
                Content::Text(t) => js_len(&t.text),
                Content::Thinking(t) => js_len(&t.thinking),
                Content::ToolCall(call) => js_len(&call.name) + js_len(&call.arguments.to_string()),
                Content::Image(_) => 0,
            })
            .sum(),
        AgentMessage::Custom(c) => content_chars(&c.content, true),
        AgentMessage::ToolResult(t) => blocks_chars(&t.content, true),
        AgentMessage::BashExecution(b) => js_len(&b.command) + js_len(&b.output),
        AgentMessage::BranchSummary(b) => js_len(&b.summary),
        AgentMessage::CompactionSummary(c) => js_len(&c.summary),
    };
    ceil_div4(chars)
}

/// Entries that can start a user-role turn: a user-ish message, a branch
/// summary or a custom message.
fn is_user_role_entry(entry: &FileEntry, bash_counts: bool) -> bool {
    match entry {
        FileEntry::BranchSummary { .. } | FileEntry::CustomMessage { .. } => true,
        FileEntry::Message { message, .. } => {
            matches!(message, AgentMessage::User(_))
                || (bash_counts && matches!(message, AgentMessage::BashExecution(_)))
        }
        _ => false,
    }
}

/// `findValidCutPoints`: every message but tool results (they must follow
/// their call), plus branch summaries and custom messages.
fn find_valid_cut_points(entries: &[FileEntry], start: usize, end: usize) -> Vec<usize> {
    (start..end)
        .filter(|&i| match &entries[i] {
            FileEntry::Message { message, .. } => !matches!(message, AgentMessage::ToolResult(_)),
            FileEntry::BranchSummary { .. } | FileEntry::CustomMessage { .. } => true,
            _ => false,
        })
        .collect()
}

/// `findTurnStartIndex`: the user message (or bash run, branch summary,
/// custom message) starting the turn that holds `entry_index`.
pub fn find_turn_start_index(
    entries: &[FileEntry],
    entry_index: usize,
    start_index: usize,
) -> Option<usize> {
    (start_index..=entry_index)
        .rev()
        .find(|&i| is_user_role_entry(&entries[i], true))
}

/// `CutPointResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutPointResult {
    /// First entry to keep.
    pub first_kept_entry_index: usize,
    /// The user message starting the turn being split (`-1` in TS).
    pub turn_start_index: Option<usize>,
    /// The cut is not at a user message.
    pub is_split_turn: bool,
}

/// `findCutPoint`: walk back from the newest entry until about
/// `keep_recent_tokens` are kept, and cut at the closest valid point at or
/// after that (never at a tool result). Considers `start..end` only.
pub fn find_cut_point(
    entries: &[FileEntry],
    start_index: usize,
    end_index: usize,
    keep_recent_tokens: u64,
) -> CutPointResult {
    let cut_points = find_valid_cut_points(entries, start_index, end_index);
    let Some(&first_cut) = cut_points.first() else {
        return CutPointResult {
            first_kept_entry_index: start_index,
            turn_start_index: None,
            is_split_turn: false,
        };
    };

    let mut accumulated = 0;
    let mut cut_index = first_cut;
    for i in (start_index..end_index).rev() {
        let FileEntry::Message { message, .. } = &entries[i] else {
            continue;
        };
        accumulated += estimate_tokens(message);
        if accumulated >= keep_recent_tokens {
            if let Some(&c) = cut_points.iter().find(|&&c| c >= i) {
                cut_index = c;
            }
            break;
        }
    }

    // Keep the non-message entries (settings changes etc.) just before the cut.
    while cut_index > start_index {
        match &entries[cut_index - 1] {
            FileEntry::Compaction { .. } | FileEntry::Message { .. } => break,
            _ => cut_index -= 1,
        }
    }

    let is_user_message = matches!(
        &entries[cut_index],
        FileEntry::Message {
            message: AgentMessage::User(_),
            ..
        }
    );
    let turn_start_index = if is_user_message {
        None
    } else {
        find_turn_start_index(entries, cut_index, start_index)
    };
    CutPointResult {
        first_kept_entry_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user_message && turn_start_index.is_some(),
    }
}

const SUMMARIZATION_PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

const UPDATE_SUMMARIZATION_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

const TURN_PREFIX_SUMMARIZATION_PROMPT: &str =
    "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained.

Summarize the prefix to provide context for the retained suffix:

## Original Request
[What did the user ask for in this turn?]

## Early Progress
- [Key decisions and work done in the prefix]

## Context for Suffix
- [Information needed to understand the retained recent work]

Be concise. Focus on what's needed to understand the kept suffix.";

/// The request knobs shared by the summarization calls (`apiKey`,
/// `headers`, `signal`, `thinkingLevel`).
#[derive(Debug, Clone, Default)]
pub struct SummarizeOptions {
    pub api_key: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub signal: Option<AbortSignal>,
    /// Sent as `reasoning` only for reasoning models, and never `off`.
    pub thinking_level: Option<ThinkingLevel>,
}

/// One summarization request: the conversation in `<conversation>` tags
/// followed by `prompt`, under the summarization system prompt. Returns the
/// joined text blocks, or the response when it failed.
async fn summarize(
    messages: &[AgentMessage],
    model: &Model,
    max_tokens: u64,
    previous_summary: Option<&str>,
    prompt: &str,
    options: &SummarizeOptions,
) -> Result<cortexcode_ai_types::AssistantMessage, String> {
    let conversation = serialize_conversation(&convert_to_llm(messages));
    let mut prompt_text = format!("<conversation>\n{conversation}\n</conversation>\n\n");
    if let Some(previous) = previous_summary {
        prompt_text.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    prompt_text.push_str(prompt);
    let context = Context::new(
        SUMMARIZATION_SYSTEM_PROMPT.to_string(),
        vec![Message::User(UserMessage {
            content: vec![Content::text(prompt_text)].into(),
            timestamp: cortexcode_ai_types::now_ms(),
        })],
        vec![],
    );
    let reasoning = options
        .thinking_level
        .clone()
        .filter(|level| model.reasoning && *level != ThinkingLevel::Off);
    let stream_options = SimpleStreamOptions {
        max_tokens: Some(max_tokens),
        signal: options.signal.clone(),
        api_key: options.api_key.clone(),
        headers: options.headers.clone(),
        reasoning,
        ..Default::default()
    };
    cortexcode_ai_registry::complete_simple(model.clone(), context, stream_options)
        .await
        .map_err(|e| e.to_string())
}

/// The response's text blocks joined with newlines.
pub(crate) fn response_text(response: &cortexcode_ai_types::AssistantMessage) -> String {
    response
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn error_text(response: &cortexcode_ai_types::AssistantMessage) -> &str {
    response
        .error_message
        .as_deref()
        .filter(|m| !m.is_empty())
        .unwrap_or("Unknown error")
}

/// `generateSummary`: summarize `current_messages` (merging into
/// `previous_summary` when given) with up to 80% of `reserve_tokens`.
pub async fn generate_summary(
    current_messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    custom_instructions: Option<&str>,
    previous_summary: Option<&str>,
    options: &SummarizeOptions,
) -> Result<String, String> {
    let max_tokens = (0.8 * reserve_tokens as f64).floor() as u64;
    let previous = previous_summary.filter(|s| !s.is_empty());
    let mut prompt = if previous.is_some() {
        UPDATE_SUMMARIZATION_PROMPT.to_string()
    } else {
        SUMMARIZATION_PROMPT.to_string()
    };
    if let Some(extra) = custom_instructions.filter(|s| !s.is_empty()) {
        prompt = format!("{prompt}\n\nAdditional focus: {extra}");
    }
    let response = summarize(
        current_messages,
        model,
        max_tokens,
        previous,
        &prompt,
        options,
    )
    .await?;
    if response.stop_reason == StopReason::Error {
        return Err(format!("Summarization failed: {}", error_text(&response)));
    }
    let text = response_text(&response);
    // An empty summary would silently drop the history being compacted.
    if text.trim().is_empty() {
        return Err("Summarization produced an empty summary".to_string());
    }
    Ok(text)
}

/// `generateTurnPrefixSummary`: the kept turn's prefix, with half of
/// `reserve_tokens`.
async fn generate_turn_prefix_summary(
    messages: &[AgentMessage],
    model: &Model,
    reserve_tokens: u64,
    options: &SummarizeOptions,
) -> Result<String, String> {
    let max_tokens = (0.5 * reserve_tokens as f64).floor() as u64;
    let response = summarize(
        messages,
        model,
        max_tokens,
        None,
        TURN_PREFIX_SUMMARIZATION_PROMPT,
        options,
    )
    .await?;
    if response.stop_reason == StopReason::Error {
        return Err(format!(
            "Turn prefix summarization failed: {}",
            error_text(&response)
        ));
    }
    let text = response_text(&response);
    if text.trim().is_empty() {
        return Err("Turn prefix summarization produced an empty summary".to_string());
    }
    Ok(text)
}

/// `CompactionPreparation`: what [`compact`] summarizes and keeps.
#[derive(Debug, Clone)]
pub struct CompactionPreparation {
    /// Id of the first entry to keep.
    pub first_kept_entry_id: String,
    /// Messages summarized and then dropped.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// The start of a split turn, summarized separately.
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u64,
    /// The previous compaction's summary, for an iterative update.
    pub previous_summary: Option<String>,
    pub file_ops: FileOperations,
    pub settings: CompactionSettings,
}

/// `prepareCompaction`: `None` when the path already ends in a compaction or
/// the kept entry has no id (an unmigrated session).
pub fn prepare_compaction(
    path_entries: &[FileEntry],
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if matches!(path_entries.last(), Some(FileEntry::Compaction { .. })) {
        return None;
    }
    let prev_compaction_index = path_entries
        .iter()
        .rposition(|e| matches!(e, FileEntry::Compaction { .. }));
    let mut previous_summary = None;
    let mut boundary_start = 0;
    if let Some(index) = prev_compaction_index {
        if let FileEntry::Compaction {
            summary,
            first_kept_entry_id,
            ..
        } = &path_entries[index]
        {
            previous_summary = Some(summary.clone());
            boundary_start = path_entries
                .iter()
                .position(|e| e.id() == Some(first_kept_entry_id.as_str()))
                .unwrap_or(index + 1);
        }
    }
    let boundary_end = path_entries.len();
    let tokens_before =
        estimate_context_tokens(&build_session_context(path_entries).messages).tokens;
    let cut = find_cut_point(
        path_entries,
        boundary_start,
        boundary_end,
        settings.keep_recent_tokens,
    );
    let first_kept_entry_id = path_entries
        .get(cut.first_kept_entry_index)?
        .id()
        .filter(|id| !id.is_empty())?
        .to_string();

    let history_end = match (cut.is_split_turn, cut.turn_start_index) {
        (true, Some(turn_start)) => turn_start,
        _ => cut.first_kept_entry_index,
    };
    let messages_to_summarize: Vec<AgentMessage> = path_entries
        .get(boundary_start..history_end)
        .unwrap_or_default()
        .iter()
        .filter_map(get_message_from_entry_for_compaction)
        .collect();
    let turn_prefix_messages: Vec<AgentMessage> = match (cut.is_split_turn, cut.turn_start_index) {
        (true, Some(turn_start)) => path_entries[turn_start..cut.first_kept_entry_index]
            .iter()
            .filter_map(get_message_from_entry_for_compaction)
            .collect(),
        _ => Vec::new(),
    };

    let mut file_ops =
        extract_file_operations(&messages_to_summarize, path_entries, prev_compaction_index);
    for message in &turn_prefix_messages {
        extract_file_ops_from_message(message, &mut file_ops);
    }
    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings: *settings,
    })
}

/// `compact`: summarize the prepared history (and a split turn's prefix, in
/// parallel), append the file lists, and estimate the tokens left.
pub async fn compact(
    preparation: &CompactionPreparation,
    model: &Model,
    custom_instructions: Option<&str>,
    options: &SummarizeOptions,
) -> Result<CompactionResult, String> {
    let settings = &preparation.settings;
    let history = async {
        if preparation.messages_to_summarize.is_empty() && preparation.is_split_turn {
            return Ok("No prior history.".to_string());
        }
        generate_summary(
            &preparation.messages_to_summarize,
            model,
            settings.reserve_tokens,
            custom_instructions,
            preparation.previous_summary.as_deref(),
            options,
        )
        .await
    };
    let mut summary = if preparation.is_split_turn && !preparation.turn_prefix_messages.is_empty() {
        let prefix = generate_turn_prefix_summary(
            &preparation.turn_prefix_messages,
            model,
            settings.reserve_tokens,
            options,
        );
        let (history, prefix) = futures_util::join!(history, prefix);
        format!(
            "{}\n\n---\n\n**Turn Context (split turn):**\n\n{}",
            history?, prefix?
        )
    } else {
        generate_summary(
            &preparation.messages_to_summarize,
            model,
            settings.reserve_tokens,
            custom_instructions,
            preparation.previous_summary.as_deref(),
            options,
        )
        .await?
    };

    let (read_files, modified_files) = compute_file_lists(&preparation.file_ops);
    summary.push_str(&format_file_operations(&read_files, &modified_files));
    if preparation.first_kept_entry_id.is_empty() {
        return Err("First kept entry has no UUID - session may need migration".to_string());
    }

    let discarded: u64 = preparation
        .messages_to_summarize
        .iter()
        .chain(&preparation.turn_prefix_messages)
        .map(estimate_tokens)
        .sum();
    let summary_tokens = ceil_div4(js_len(&summary));
    let tokens_after =
        (preparation.tokens_before as i64 - discarded as i64 + summary_tokens as i64).max(0) as u64;
    Ok(CompactionResult {
        summary,
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        tokens_after: Some(tokens_after),
        details: Some(json!({"readFiles": read_files, "modifiedFiles": modified_files})),
    })
}

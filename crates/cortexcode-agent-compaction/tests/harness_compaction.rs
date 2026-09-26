//! Port of hoocode `packages/agent/test/harness/compaction.test.ts`
//! (v0.5.89), plus branch summarization and split-turn cases.

use std::sync::{Arc, Mutex};

use cortexcode_agent_compaction::*;
use cortexcode_agent_session::{build_session_context, FileEntry};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_provider_faux::{
    faux_assistant_message, register_faux_provider, FauxModelDefinition, FauxProviderRegistration,
    FauxResponseStep, RegisterFauxProviderOptions,
};
use cortexcode_ai_types::{
    AssistantMessage, Content, Message, Model, StopReason, ThinkingLevel, ToolCallContent,
    ToolResultMessage, Usage, UserMessage,
};
use serde_json::json;

/// Numbers entries `entry-0`, `entry-1`, ... like the TS `createId`.
#[derive(Default)]
struct Ids(usize);

impl Ids {
    fn next(&mut self) -> String {
        let id = format!("entry-{}", self.0);
        self.0 += 1;
        id
    }
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output + cache_read + cache_write,
        ..Default::default()
    }
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: vec![Content::text(text)].into(),
        timestamp: 1,
    })
}

fn assistant_with(content: Vec<Content>, usage: Usage) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content,
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
        usage,
        stop_reason: StopReason::Stop,
        timestamp: 1,
        ..Default::default()
    })
}

fn assistant(text: &str, usage: Usage) -> AgentMessage {
    assistant_with(vec![Content::text(text)], usage)
}

const NOW: &str = "2026-09-26T00:00:00.000Z";

fn message(ids: &mut Ids, message: AgentMessage, parent: Option<&FileEntry>) -> FileEntry {
    FileEntry::Message {
        id: ids.next(),
        parent_id: parent.and_then(|p| p.id()).map(str::to_string),
        timestamp: NOW.into(),
        message,
    }
}

fn compaction(
    ids: &mut Ids,
    summary: &str,
    first_kept: &FileEntry,
    parent: &FileEntry,
) -> FileEntry {
    FileEntry::Compaction {
        id: ids.next(),
        parent_id: parent.id().map(str::to_string),
        timestamp: NOW.into(),
        summary: summary.into(),
        first_kept_entry_id: first_kept.id().unwrap().to_string(),
        tokens_before: 1234,
        tokens_after: None,
        details: None,
        from_hook: None,
    }
}

/// Unregisters on drop.
struct Faux(FauxProviderRegistration);

impl Drop for Faux {
    fn drop(&mut self) {
        self.0.unregister();
    }
}

fn faux_model(reasoning: bool) -> (Faux, Model) {
    let id = if reasoning {
        "reasoning-model"
    } else {
        "non-reasoning-model"
    };
    let registration = register_faux_provider(RegisterFauxProviderOptions {
        models: vec![FauxModelDefinition {
            reasoning: Some(reasoning),
            context_window: Some(200_000),
            max_tokens: Some(8192),
            ..FauxModelDefinition::new(id)
        }],
        ..Default::default()
    });
    let model = registration.get_model();
    (Faux(registration), model)
}

fn summary_reply() -> FauxResponseStep {
    faux_assistant_message("## Goal\nTest summary", Default::default()).into()
}

#[test]
fn calculates_total_context_tokens_from_usage() {
    assert_eq!(calculate_context_tokens(&usage(1000, 500, 200, 100)), 1800);
    assert_eq!(calculate_context_tokens(&usage(0, 0, 0, 0)), 0);
}

#[test]
fn checks_compaction_threshold() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10000,
        keep_recent_tokens: 20000,
        max_context_ratio: None,
    };
    assert!(should_compact(95000, 100000, &settings));
    assert!(!should_compact(89000, 100000, &settings));
    let disabled = CompactionSettings {
        enabled: false,
        ..settings
    };
    assert!(!should_compact(95000, 100000, &disabled));
}

#[test]
fn applies_the_soft_ratio_trigger_before_the_reserve_rule_fires() {
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10000,
        keep_recent_tokens: 20000,
        max_context_ratio: Some(0.75),
    };
    assert!(should_compact(80000, 100000, &settings));
    assert!(!should_compact(70000, 100000, &settings));
    let unset = CompactionSettings {
        max_context_ratio: None,
        ..settings
    };
    assert!(!should_compact(89000, 100000, &unset));
    let out_of_range = CompactionSettings {
        max_context_ratio: Some(1.5),
        ..settings
    };
    assert!(!should_compact(80000, 100000, &out_of_range));
}

#[test]
fn finds_a_cut_point_based_on_token_differences() {
    let mut ids = Ids::default();
    let mut entries: Vec<FileEntry> = Vec::new();
    for i in 0..10u64 {
        let u = message(&mut ids, user(&format!("User {i}")), entries.last());
        entries.push(u);
        let a = message(
            &mut ids,
            assistant(&format!("Assistant {i}"), usage(0, 100, (i + 1) * 1000, 0)),
            entries.last(),
        );
        entries.push(a);
    }
    let result = find_cut_point(&entries, 0, entries.len(), 2500);
    assert!(entries[result.first_kept_entry_index].is_message());
}

#[test]
fn builds_session_context_with_a_compaction_entry() {
    let mut ids = Ids::default();
    let u1 = message(&mut ids, user("1"), None);
    let a1 = message(&mut ids, assistant("a", usage(100, 50, 0, 0)), Some(&u1));
    let u2 = message(&mut ids, user("2"), Some(&a1));
    let a2 = message(&mut ids, assistant("b", usage(100, 50, 0, 0)), Some(&u2));
    let c = compaction(&mut ids, "Summary of 1,a,2,b", &u2, &a2);
    let u3 = message(&mut ids, user("3"), Some(&c));
    let a3 = message(&mut ids, assistant("c", usage(100, 50, 0, 0)), Some(&u3));
    let loaded = build_session_context(&[u1, a1, u2, a2, c, u3, a3]);
    assert_eq!(loaded.messages.len(), 5);
    assert!(matches!(
        loaded.messages[0],
        AgentMessage::CompactionSummary(_)
    ));
}

#[test]
fn prepares_compaction_using_the_latest_compaction_summary_as_previous_summary() {
    let mut ids = Ids::default();
    let u1 = message(&mut ids, user("user msg 1"), None);
    let a1 = message(
        &mut ids,
        assistant("assistant msg 1", usage(100, 50, 0, 0)),
        Some(&u1),
    );
    let u2 = message(&mut ids, user("user msg 2"), Some(&a1));
    let a2 = message(
        &mut ids,
        assistant("assistant msg 2", usage(5000, 1000, 0, 0)),
        Some(&u2),
    );
    let c1 = compaction(&mut ids, "First summary", &u2, &a2);
    let u3 = message(&mut ids, user("user msg 3"), Some(&c1));
    let a3 = message(
        &mut ids,
        assistant("assistant msg 3", usage(8000, 2000, 0, 0)),
        Some(&u3),
    );
    let path = vec![u1, a1, u2, a2, c1, u3, a3];
    let preparation = prepare_compaction(&path, &DEFAULT_COMPACTION_SETTINGS).unwrap();
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
    assert!(!preparation.first_kept_entry_id.is_empty());
    assert_eq!(
        preparation.tokens_before,
        estimate_context_tokens(&build_session_context(&path).messages).tokens
    );
    // A path ending in a compaction has nothing new to compact.
    let mut ended = path.clone();
    let last = ended.last().unwrap().clone();
    ended.push(compaction(&mut ids, "again", &last, &last));
    assert!(prepare_compaction(&ended, &DEFAULT_COMPACTION_SETTINGS).is_none());
}

#[test]
fn serializes_conversation_with_truncated_tool_results() {
    let messages = vec![Message::ToolResult(ToolResultMessage {
        tool_call_id: "tc1".into(),
        tool_name: "read".into(),
        content: vec![Content::text("x".repeat(5000))],
        details: None,
        is_error: false,
        timestamp: 1,
    })];
    let result = serialize_conversation(&messages);
    assert!(result.contains("[Tool result]:"));
    assert!(result.contains("[... 3000 more characters truncated]"));
}

/// `(reasoning, api_key)` of each summarization request.
type Seen = Arc<Mutex<Vec<(Option<ThinkingLevel>, Option<String>)>>>;

#[tokio::test]
async fn passes_reasoning_through_generate_summary_only_for_reasoning_models_with_thinking_enabled()
{
    let messages = vec![user("Summarize this.")];
    let messages = &messages;
    let seen: Seen = Default::default();
    let recording = |seen: &Seen| {
        let seen = seen.clone();
        FauxResponseStep::factory(move |_context, options, _state, _model| {
            seen.lock()
                .unwrap()
                .push((options.reasoning.clone(), options.api_key.clone()));
            Ok(faux_assistant_message(
                "## Goal\nTest summary",
                Default::default(),
            ))
        })
    };
    let run = |model: Model, level: ThinkingLevel| async move {
        let options = SummarizeOptions {
            api_key: Some("test-key".into()),
            thinking_level: Some(level),
            ..Default::default()
        };
        generate_summary(messages, &model, 2000, None, None, &options)
            .await
            .unwrap()
    };

    let (reasoning, reasoning_model) = faux_model(true);
    reasoning.0.set_responses(vec![recording(&seen)]);
    run(reasoning_model, ThinkingLevel::Medium).await;
    let (off, off_model) = faux_model(true);
    off.0.set_responses(vec![recording(&seen)]);
    run(off_model, ThinkingLevel::Off).await;
    let (plain, plain_model) = faux_model(false);
    plain.0.set_responses(vec![recording(&seen)]);
    run(plain_model, ThinkingLevel::Medium).await;

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[0],
        (Some(ThinkingLevel::Medium), Some("test-key".into()))
    );
    assert_eq!(seen[1].0, None);
    assert_eq!(seen[2].0, None);
}

#[tokio::test]
async fn returns_a_compaction_result_with_file_details() {
    let mut ids = Ids::default();
    let u1 = message(&mut ids, user("read a file"), None);
    let a1 = message(
        &mut ids,
        assistant_with(
            vec![Content::ToolCall(ToolCallContent {
                id: "tool-1".into(),
                name: "read".into(),
                arguments: json!({"path": "src/index.ts"}),
                thought_signature: None,
            })],
            usage(1000, 200, 0, 0),
        ),
        Some(&u1),
    );
    let u2 = message(&mut ids, user("continue"), Some(&a1));
    let a2 = message(
        &mut ids,
        assistant("done", usage(4000, 500, 0, 0)),
        Some(&u2),
    );
    let preparation = prepare_compaction(&[u1, a1, u2, a2], &DEFAULT_COMPACTION_SETTINGS).unwrap();
    let (faux, model) = faux_model(false);
    faux.0.set_responses(vec![summary_reply()]);
    let options = SummarizeOptions {
        api_key: Some("test-key".into()),
        ..Default::default()
    };
    let result = compact(&preparation, &model, None, &options).await.unwrap();
    assert!(!result.summary.is_empty());
    assert!(!result.first_kept_entry_id.is_empty());
    assert!(result.details.is_some());
    assert!(result.tokens_after.is_some());
}

// --- beyond the TS file ---

#[tokio::test]
async fn empty_and_failed_summaries_are_errors() {
    let (faux, model) = faux_model(false);
    faux.0.set_responses(vec![
        faux_assistant_message("  ", Default::default()).into(),
        AssistantMessage {
            stop_reason: StopReason::Error,
            error_message: Some("boom".into()),
            ..faux_assistant_message("", Default::default())
        }
        .into(),
    ]);
    let options = SummarizeOptions::default();
    let messages = vec![user("x")];
    assert_eq!(
        generate_summary(&messages, &model, 1000, None, None, &options)
            .await
            .unwrap_err(),
        "Summarization produced an empty summary"
    );
    assert_eq!(
        generate_summary(&messages, &model, 1000, None, None, &options)
            .await
            .unwrap_err(),
        "Summarization failed: boom"
    );
}

#[tokio::test]
async fn a_split_turn_gets_a_turn_prefix_summary() {
    let mut ids = Ids::default();
    let u1 = message(&mut ids, user("start a big task"), None);
    let mut entries = vec![u1];
    for i in 0..6 {
        let a = message(
            &mut ids,
            assistant(&"y".repeat(400), usage(100 * i, 10, 0, 0)),
            entries.last(),
        );
        entries.push(a);
    }
    let settings = CompactionSettings {
        keep_recent_tokens: 250,
        ..DEFAULT_COMPACTION_SETTINGS
    };
    let preparation = prepare_compaction(&entries, &settings).unwrap();
    assert!(preparation.is_split_turn);
    assert!(preparation.messages_to_summarize.is_empty());
    assert!(!preparation.turn_prefix_messages.is_empty());

    let (faux, model) = faux_model(false);
    faux.0.set_responses(vec![
        faux_assistant_message("prefix", Default::default()).into()
    ]);
    let result = compact(&preparation, &model, None, &SummarizeOptions::default())
        .await
        .unwrap();
    assert_eq!(
        result.summary,
        "No prior history.\n\n---\n\n**Turn Context (split turn):**\n\nprefix"
    );
}

struct Tree(Vec<FileEntry>);

impl BranchEntrySource for Tree {
    fn get_branch(&self, id: &str) -> Vec<FileEntry> {
        let mut path = Vec::new();
        let mut current = self.get_entry(id);
        while let Some(entry) = current {
            current = entry.parent_id().and_then(|p| self.get_entry(p));
            path.push(entry);
        }
        path.reverse();
        path
    }
    fn get_entry(&self, id: &str) -> Option<FileEntry> {
        self.0.iter().find(|e| e.id() == Some(id)).cloned()
    }
}

#[tokio::test]
async fn collects_and_summarizes_an_abandoned_branch() {
    let mut ids = Ids::default();
    let root = message(&mut ids, user("root"), None);
    let old1 = message(
        &mut ids,
        assistant_with(
            vec![Content::ToolCall(ToolCallContent {
                id: "t".into(),
                name: "edit".into(),
                arguments: json!({"path": "a.rs"}),
                thought_signature: None,
            })],
            usage(10, 1, 0, 0),
        ),
        Some(&root),
    );
    let old2 = message(&mut ids, user("old branch"), Some(&old1));
    let new1 = message(&mut ids, user("new branch"), Some(&root));
    let tree = Tree(vec![root.clone(), old1, old2.clone(), new1.clone()]);

    let collected = collect_entries_for_branch_summary(&tree, old2.id(), new1.id().unwrap());
    assert_eq!(collected.common_ancestor_id.as_deref(), root.id());
    let ids_collected: Vec<_> = collected.entries.iter().filter_map(|e| e.id()).collect();
    assert_eq!(ids_collected, ["entry-1", "entry-2"]);
    assert!(collect_entries_for_branch_summary(&tree, None, "x")
        .entries
        .is_empty());

    let (faux, model) = faux_model(false);
    faux.0.set_responses(vec![faux_assistant_message(
        "## Goal\nexplore",
        Default::default(),
    )
    .into()]);
    let options = GenerateBranchSummaryOptions {
        model,
        api_key: None,
        headers: None,
        signal: None,
        custom_instructions: None,
        replace_instructions: false,
        reserve_tokens: None,
    };
    let result = generate_branch_summary(&collected.entries, &options)
        .await
        .unwrap();
    assert_eq!(
        result.summary.as_deref(),
        Some("The user explored a different conversation branch before returning here.\nSummary of that exploration:\n\n## Goal\nexplore\n\n<modified-files>\na.rs\n</modified-files>")
    );
    assert_eq!(result.modified_files, Some(vec!["a.rs".to_string()]));

    let nothing = generate_branch_summary(&[], &options).await.unwrap();
    assert_eq!(nothing.summary.as_deref(), Some("No content to summarize"));
}

#[test]
fn branch_preparation_keeps_the_newest_messages_within_budget() {
    let mut ids = Ids::default();
    let entries: Vec<FileEntry> = (0..4)
        .map(|i| message(&mut ids, user(&"z".repeat(40 * (i + 1))), None))
        .collect();
    // 10, 20, 30, 40 tokens: 70 fits the newest two.
    let prepared = prepare_branch_entries(&entries, 75);
    assert_eq!(prepared.messages.len(), 2);
    assert_eq!(prepared.total_tokens, 70);
    assert_eq!(prepare_branch_entries(&entries, 0).messages.len(), 4);
}

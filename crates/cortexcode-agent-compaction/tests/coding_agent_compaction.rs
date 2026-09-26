//! Port of hoocode `packages/coding-agent/test/compaction.test.ts` (v0.5.89):
//! the harness compaction functions over coding-agent sessions, including the
//! v1 `large-session.jsonl` fixture (read from the pinned hoocode checkout,
//! migrated by `cortexcode-code-session`). The two LLM cases are `#[ignore]`d
//! and need `ANTHROPIC_OAUTH_TOKEN`.

use std::path::PathBuf;

use cortexcode_agent_compaction::*;
use cortexcode_agent_session::{build_session_context, FileEntry};
use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{AssistantMessage, Content, StopReason, Usage, UserContent, UserMessage};

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
        content: text.into(),
        timestamp: 1,
    })
}

fn assistant(text: &str, usage: Option<Usage>) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        usage: usage.unwrap_or_else(|| self::usage(100, 50, 0, 0)),
        stop_reason: StopReason::Stop,
        timestamp: 1,
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-5".into(),
        ..Default::default()
    })
}

/// `test-id-N` entries chained through `parentId`, like the TS helpers.
#[derive(Default)]
struct Chain {
    counter: usize,
    last: Option<String>,
}

const NOW: &str = "2026-09-26T00:00:00.000Z";

impl Chain {
    fn id(&mut self) -> (String, Option<String>) {
        let id = format!("test-id-{}", self.counter);
        self.counter += 1;
        let parent = self.last.replace(id.clone());
        (id, parent)
    }

    fn message(&mut self, message: AgentMessage) -> FileEntry {
        let (id, parent_id) = self.id();
        FileEntry::Message {
            id,
            parent_id,
            timestamp: NOW.into(),
            message,
        }
    }

    fn compaction(&mut self, summary: &str, first_kept: &FileEntry) -> FileEntry {
        let (id, parent_id) = self.id();
        FileEntry::Compaction {
            id,
            parent_id,
            timestamp: NOW.into(),
            summary: summary.into(),
            first_kept_entry_id: first_kept.id().unwrap().into(),
            tokens_before: 10000,
            tokens_after: None,
            details: None,
            from_hook: None,
        }
    }

    fn model_change(&mut self, provider: &str, model_id: &str) -> FileEntry {
        let (id, parent_id) = self.id();
        FileEntry::ModelChange {
            id,
            parent_id,
            timestamp: NOW.into(),
            provider: provider.into(),
            model_id: model_id.into(),
        }
    }

    fn thinking_level(&mut self, level: &str) -> FileEntry {
        let (id, parent_id) = self.id();
        FileEntry::ThinkingLevelChange {
            id,
            parent_id,
            timestamp: NOW.into(),
            thinking_level: level.into(),
        }
    }
}

fn blocks_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks_text(blocks),
    }
}

/// `extractText`.
fn extract_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .map(|m| match m {
            AgentMessage::User(u) => user_text(&u.content),
            AgentMessage::Assistant(a) => blocks_text(&a.content),
            AgentMessage::BranchSummary(b) => b.summary.clone(),
            AgentMessage::CompactionSummary(c) => c.summary.clone(),
            AgentMessage::Custom(c) => user_text(&c.content),
            AgentMessage::ToolResult(t) => blocks_text(&t.content),
            AgentMessage::BashExecution(b) => format!("{}\n{}", b.command, b.output),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn role(entry: &FileEntry) -> &'static str {
    match entry {
        FileEntry::Message { message, .. } => match message {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            _ => "other",
        },
        _ => "not a message",
    }
}

/// `loadLargeSessionEntries`.
fn load_large_session_entries() -> Vec<FileEntry> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/hoocode-pin/packages/coding-agent/test/fixtures/large-session.jsonl");
    assert!(
        path.exists(),
        "{} missing: run migration/tui-parity/setup_hoocode.sh",
        path.display()
    );
    let loaded = cortexcode_code_session::load_session_file(&path);
    loaded
        .entries
        .into_iter()
        .filter(|e| !matches!(e, FileEntry::Session(_)))
        .collect()
}

// --- Token calculation ---

#[test]
fn calculates_total_context_tokens_from_usage() {
    assert_eq!(calculate_context_tokens(&usage(1000, 500, 200, 100)), 1800);
    assert_eq!(calculate_context_tokens(&usage(0, 0, 0, 0)), 0);
}

// --- getLastAssistantUsage ---

#[test]
fn finds_the_last_non_aborted_assistant_usage() {
    let mut c = Chain::default();
    let entries = vec![
        c.message(user("Hello")),
        c.message(assistant("Hi", Some(usage(100, 50, 0, 0)))),
        c.message(user("How are you?")),
        c.message(assistant("Good", Some(usage(200, 100, 0, 0)))),
    ];
    assert_eq!(get_last_assistant_usage(&entries).unwrap().input, 200);

    let mut c = Chain::default();
    let aborted = match assistant("Aborted", Some(usage(300, 150, 0, 0))) {
        AgentMessage::Assistant(a) => AgentMessage::Assistant(AssistantMessage {
            stop_reason: StopReason::Aborted,
            ..a
        }),
        _ => unreachable!(),
    };
    let entries = vec![
        c.message(user("Hello")),
        c.message(assistant("Hi", Some(usage(100, 50, 0, 0)))),
        c.message(user("How are you?")),
        c.message(aborted),
    ];
    assert_eq!(get_last_assistant_usage(&entries).unwrap().input, 100);

    let mut c = Chain::default();
    assert!(get_last_assistant_usage(&[c.message(user("Hello"))]).is_none());
}

// --- shouldCompact ---

#[test]
fn should_compact_past_the_reserve_unless_disabled() {
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

// --- findCutPoint ---

#[test]
fn cuts_at_a_user_or_assistant_message() {
    let mut c = Chain::default();
    let mut entries = Vec::new();
    for i in 0..10u64 {
        entries.push(c.message(user(&format!("User {i}"))));
        entries.push(c.message(assistant(
            &format!("Assistant {i}"),
            Some(usage(0, 100, (i + 1) * 1000, 0)),
        )));
    }
    let result = find_cut_point(&entries, 0, entries.len(), 2500);
    assert!(matches!(
        role(&entries[result.first_kept_entry_index]),
        "user" | "assistant"
    ));
}

#[test]
fn returns_start_index_without_valid_cut_points_or_when_everything_fits() {
    let mut c = Chain::default();
    let single = vec![c.message(assistant("a", None))];
    assert_eq!(
        find_cut_point(&single, 0, 1, 1000).first_kept_entry_index,
        0
    );

    let mut c = Chain::default();
    let entries = vec![
        c.message(user("1")),
        c.message(assistant("a", Some(usage(0, 50, 500, 0)))),
        c.message(user("2")),
        c.message(assistant("b", Some(usage(0, 50, 1000, 0)))),
    ];
    assert_eq!(
        find_cut_point(&entries, 0, entries.len(), 50000).first_kept_entry_index,
        0
    );
}

#[test]
fn indicates_a_split_turn_when_cutting_at_an_assistant_message() {
    let mut c = Chain::default();
    let entries = vec![
        c.message(user("Turn 1")),
        c.message(assistant("A1", Some(usage(0, 100, 1000, 0)))),
        c.message(user("Turn 2")),
        c.message(assistant("A2-1", Some(usage(0, 100, 5000, 0)))),
        c.message(assistant("A2-2", Some(usage(0, 100, 8000, 0)))),
        c.message(assistant("A2-3", Some(usage(0, 100, 10000, 0)))),
    ];
    let result = find_cut_point(&entries, 0, entries.len(), 3000);
    if role(&entries[result.first_kept_entry_index]) == "assistant" {
        assert!(result.is_split_turn);
        assert_eq!(result.turn_start_index, Some(2));
    }
}

// --- buildSessionContext ---

#[test]
fn loads_all_messages_without_compaction() {
    let mut c = Chain::default();
    let entries = vec![
        c.message(user("1")),
        c.message(assistant("a", None)),
        c.message(user("2")),
        c.message(assistant("b", None)),
    ];
    let loaded = build_session_context(&entries);
    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.thinking_level, "off");
    let model = loaded.model.unwrap();
    assert_eq!(
        (model.provider.as_str(), model.model_id.as_str()),
        ("anthropic", "claude-sonnet-4-5")
    );
}

#[test]
fn handles_single_and_multiple_compactions() {
    let mut c = Chain::default();
    let u1 = c.message(user("1"));
    let a1 = c.message(assistant("a", None));
    let u2 = c.message(user("2"));
    let a2 = c.message(assistant("b", None));
    let compaction = c.compaction("Summary of 1,a,2,b", &u2);
    let u3 = c.message(user("3"));
    let a3 = c.message(assistant("c", None));
    let loaded = build_session_context(&[u1, a1, u2, a2, compaction, u3, a3]);
    assert_eq!(loaded.messages.len(), 5);
    let AgentMessage::CompactionSummary(summary) = &loaded.messages[0] else {
        panic!("{:?}", loaded.messages[0])
    };
    assert!(summary.summary.contains("Summary of 1,a,2,b"));

    let mut c = Chain::default();
    let u1 = c.message(user("1"));
    let a1 = c.message(assistant("a", None));
    let compact1 = c.compaction("First summary", &u1);
    let u2 = c.message(user("2"));
    let b = c.message(assistant("b", None));
    let u3 = c.message(user("3"));
    let cc = c.message(assistant("c", None));
    let compact2 = c.compaction("Second summary", &u3);
    let u4 = c.message(user("4"));
    let d = c.message(assistant("d", None));
    let loaded = build_session_context(&[u1, a1, compact1, u2, b, u3, cc, compact2, u4, d]);
    assert_eq!(loaded.messages.len(), 5);
    assert!(extract_text(&loaded.messages[..1]).contains("Second summary"));
}

#[test]
fn keeps_all_messages_when_the_first_entry_is_kept() {
    let mut c = Chain::default();
    let u1 = c.message(user("1"));
    let a1 = c.message(assistant("a", None));
    let compact1 = c.compaction("First summary", &u1);
    let u2 = c.message(user("2"));
    let b = c.message(assistant("b", None));
    assert_eq!(
        build_session_context(&[u1, a1, compact1, u2, b])
            .messages
            .len(),
        5
    );
}

#[test]
fn tracks_model_and_thinking_level_changes() {
    let mut c = Chain::default();
    let entries = vec![
        c.message(user("1")),
        c.model_change("openai", "gpt-4"),
        c.message(assistant("a", None)),
        c.thinking_level("high"),
    ];
    let loaded = build_session_context(&entries);
    // The assistant message's model overrides the model_change.
    let model = loaded.model.unwrap();
    assert_eq!(
        (model.provider.as_str(), model.model_id.as_str()),
        ("anthropic", "claude-sonnet-4-5")
    );
    assert_eq!(loaded.thinking_level, "high");
}

// --- prepareCompaction with previous compaction ---

#[test]
fn preserves_kept_messages_across_repeated_compactions_when_they_still_fit() {
    let mut c = Chain::default();
    let u1 = c.message(user("user msg 1 (summarized by compaction1)"));
    let a1 = c.message(assistant("assistant msg 1", None));
    let u2 = c.message(user("user msg 2 - kept by compaction1"));
    let a2 = c.message(assistant("assistant msg 2", None));
    let u3 = c.message(user("user msg 3 - kept by compaction1"));
    let a3 = c.message(assistant("assistant msg 3", Some(usage(5000, 1000, 0, 0))));
    let compaction1 = c.compaction("First summary", &u2);
    let u4 = c.message(user("user msg 4 (new after compaction1)"));
    let a4 = c.message(assistant("assistant msg 4", Some(usage(8000, 2000, 0, 0))));
    let path = vec![u1, a1, u2.clone(), a2, u3, a3, compaction1, u4, a4.clone()];
    let context_before = build_session_context(&path);
    let preparation = prepare_compaction(&path, &DEFAULT_COMPACTION_SETTINGS).unwrap();
    assert_eq!(preparation.first_kept_entry_id, u2.id().unwrap());
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
    assert!(!extract_text(&preparation.messages_to_summarize).contains("First summary"));
    assert_eq!(
        preparation.tokens_before,
        estimate_context_tokens(&context_before.messages).tokens
    );

    let compaction2 = FileEntry::Compaction {
        id: "compaction2-id".into(),
        parent_id: a4.id().map(str::to_string),
        timestamp: NOW.into(),
        summary: "Second summary".into(),
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        tokens_after: None,
        details: None,
        from_hook: None,
    };
    let mut after = path.clone();
    after.push(compaction2);
    let text = extract_text(&build_session_context(&after).messages);
    assert!(text.contains("user msg 2 - kept by compaction1"));
    assert!(text.contains("user msg 3 - kept by compaction1"));
}

#[test]
fn re_summarizes_previously_kept_messages_when_the_window_moves_past_them() {
    let mut c = Chain::default();
    let u1 = c.message(user(&"user msg 1 (summarized by compaction1)".repeat(4)));
    let a1 = c.message(assistant(&"assistant msg 1".repeat(4), None));
    let u2 = c.message(user(&"user msg 2 - kept by compaction1 ".repeat(12)));
    let a2 = c.message(assistant(&"assistant msg 2 ".repeat(12), None));
    let u3 = c.message(user(&"user msg 3 - kept by compaction1 ".repeat(12)));
    let a3 = c.message(assistant(
        &"assistant msg 3 ".repeat(12),
        Some(usage(5000, 1000, 0, 0)),
    ));
    let compaction1 = c.compaction("First summary", &u2);
    let u4 = c.message(user(&"user msg 4 (new after compaction1) ".repeat(12)));
    let a4 = c.message(assistant(
        &"assistant msg 4 ".repeat(12),
        Some(usage(8000, 2000, 0, 0)),
    ));
    let settings = CompactionSettings {
        keep_recent_tokens: 100,
        ..DEFAULT_COMPACTION_SETTINGS
    };
    let preparation =
        prepare_compaction(&[u1, a1, u2, a2, u3, a3, compaction1, u4, a4], &settings).unwrap();
    let summarized = extract_text(&preparation.messages_to_summarize);
    assert!(summarized.contains("user msg 2 - kept by compaction1"));
    assert!(summarized.contains("user msg 3 - kept by compaction1"));
    assert!(!summarized.contains("First summary"));
    assert_eq!(
        preparation.previous_summary.as_deref(),
        Some("First summary")
    );
}

// --- Large session fixture ---

#[test]
fn parses_the_large_session() {
    let entries = load_large_session_entries();
    assert!(entries.len() > 100);
    assert!(entries.iter().filter(|e| e.is_message()).count() > 100);
}

#[test]
fn finds_a_cut_point_in_the_large_session() {
    let entries = load_large_session_entries();
    let result = find_cut_point(
        &entries,
        0,
        entries.len(),
        DEFAULT_COMPACTION_SETTINGS.keep_recent_tokens,
    );
    assert!(matches!(
        role(&entries[result.first_kept_entry_index]),
        "user" | "assistant"
    ));
}

#[test]
fn loads_the_large_session() {
    let loaded = build_session_context(&load_large_session_entries());
    assert!(loaded.messages.len() > 100);
    assert!(loaded.model.is_some());
}

// --- LLM summarization (live) ---

fn live_options() -> Option<(cortexcode_ai_types::Model, SummarizeOptions)> {
    let token = std::env::var("ANTHROPIC_OAUTH_TOKEN").ok()?;
    let model = cortexcode_ai_models::get_model("anthropic", "claude-sonnet-4-5")?.clone();
    Some((
        model,
        SummarizeOptions {
            api_key: Some(token),
            ..Default::default()
        },
    ))
}

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_OAUTH_TOKEN"]
async fn generates_a_compaction_result_for_the_large_session_and_reloads() {
    let Some((model, options)) = live_options() else {
        return;
    };
    let entries = load_large_session_entries();
    let loaded = build_session_context(&entries);
    let preparation = prepare_compaction(&entries, &DEFAULT_COMPACTION_SETTINGS).unwrap();
    let result = compact(&preparation, &model, None, &options).await.unwrap();
    assert!(result.summary.len() > 100);
    assert!(!result.first_kept_entry_id.is_empty());
    assert!(result.tokens_before > 0);

    let mut reloaded = entries.clone();
    reloaded.push(FileEntry::Compaction {
        id: "compaction-test-id".into(),
        parent_id: entries.last().and_then(|e| e.id()).map(str::to_string),
        timestamp: NOW.into(),
        summary: result.summary.clone(),
        first_kept_entry_id: result.first_kept_entry_id.clone(),
        tokens_before: result.tokens_before,
        tokens_after: result.tokens_after,
        details: result.details.clone(),
        from_hook: None,
    });
    let reloaded = build_session_context(&reloaded);
    assert!(reloaded.messages.len() < loaded.messages.len());
    assert!(extract_text(&reloaded.messages[..1]).contains(&result.summary));
}

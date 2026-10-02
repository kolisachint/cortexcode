//! Ports `test/agent-session-compaction.test.ts`. The TS file runs against a
//! live Anthropic model; here the faux provider answers (prompts and the
//! summary request alike), so the cases run offline.

use crate::common::{Harness, HarnessOptions};
use cortexcode_ai_provider_faux::{faux_assistant_message, FauxMessageOptions, FauxResponseStep};
use cortexcode_code_agent_session::{AgentSessionEvent, CompactionReason, PromptOptions};
use cortexcode_code_session::{FileEntry, SessionManager};

fn text(t: &str) -> FauxResponseStep {
    FauxResponseStep::Message(faux_assistant_message(t, FauxMessageOptions::default()))
}

/// `createSession(inMemory)`: `keepRecentTokens: 1` so small conversations
/// have something to summarize. The cut then lands inside the last turn, so
/// compaction makes two summary requests (history and turn prefix).
fn create_session(in_memory: bool) -> Harness {
    let mut settings = serde_json::Map::new();
    settings.insert(
        "compaction".into(),
        serde_json::json!({"keepRecentTokens": 1}),
    );
    Harness::new(HarnessOptions {
        settings,
        session_manager: Some(Box::new(move |dir: &std::path::Path| {
            if in_memory {
                SessionManager::in_memory(dir.to_string_lossy())
            } else {
                SessionManager::create(dir.to_string_lossy(), Some(dir.join("sessions")))
            }
        })),
        ..Default::default()
    })
}

async fn prompt(h: &Harness, message: &str) {
    h.session
        .prompt(message, PromptOptions::default())
        .await
        .unwrap();
}

fn compaction_entries(h: &Harness) -> Vec<FileEntry> {
    h.session
        .session_manager()
        .entries()
        .iter()
        .filter(|e| matches!(e, FileEntry::Compaction { .. }))
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn should_trigger_manual_compaction_via_compact() {
    let h = create_session(false);
    h.set_responses(vec![
        text("4"),
        text("6"),
        text("Summary of the math."),
        text("Turn prefix summary."),
    ]);
    prompt(&h, "What is 2+2? Reply with just the number.").await;
    prompt(&h, "What is 3+3? Reply with just the number.").await;

    let result = h.session.compact(None).await.unwrap();
    assert!(!result.summary.is_empty());
    assert!(result.tokens_before > 0);

    let roles = h.roles();
    assert!(!roles.is_empty());
    assert_eq!(roles[0], "compactionSummary");
}

#[tokio::test(flavor = "multi_thread")]
async fn should_maintain_valid_session_state_after_compaction() {
    let h = create_session(false);
    h.set_responses(vec![
        text("Paris"),
        text("Berlin"),
        text("Summary of capitals."),
        text("Turn prefix summary."),
        text("Rome"),
    ]);
    prompt(&h, "What is the capital of France? One word answer.").await;
    prompt(&h, "What is the capital of Germany? One word answer.").await;
    h.session.compact(None).await.unwrap();

    prompt(&h, "What is the capital of Italy? One word answer.").await;
    assert!(!h.session.messages().is_empty());
    assert!(h.roles().iter().any(|r| r == "assistant"));
    assert_eq!(h.assistant_texts().last().unwrap(), "Rome");
}

#[tokio::test(flavor = "multi_thread")]
async fn should_persist_compaction_to_session_file() {
    let h = create_session(false);
    h.set_responses(vec![
        text("hello"),
        text("goodbye"),
        text("Summary."),
        text("Prefix."),
    ]);
    prompt(&h, "Say hello").await;
    prompt(&h, "Say goodbye").await;
    h.session.compact(None).await.unwrap();

    let compactions = compaction_entries(&h);
    assert_eq!(compactions.len(), 1);
    let FileEntry::Compaction {
        summary,
        first_kept_entry_id,
        tokens_before,
        ..
    } = &compactions[0]
    else {
        unreachable!()
    };
    assert!(!summary.is_empty());
    assert!(!first_kept_entry_id.is_empty());
    assert!(*tokens_before > 0);

    // And it is on disk: reopening the file sees the same entry.
    let file = h
        .session
        .session_manager()
        .session_file()
        .unwrap()
        .to_path_buf();
    let reopened = SessionManager::open(&file, None, None);
    assert_eq!(
        reopened
            .entries()
            .iter()
            .filter(|e| matches!(e, FileEntry::Compaction { .. }))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn should_work_with_no_session_mode_in_memory_only() {
    let h = create_session(true);
    h.set_responses(vec![
        text("4"),
        text("6"),
        text("Summary."),
        text("Prefix."),
    ]);
    prompt(&h, "What is 2+2? Reply with just the number.").await;
    prompt(&h, "What is 3+3? Reply with just the number.").await;

    let result = h.session.compact(None).await.unwrap();
    assert!(!result.summary.is_empty());
    assert!(h.session.session_manager().session_file().is_none());
    assert_eq!(compaction_entries(&h).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn should_emit_compaction_events_during_manual_compaction() {
    let h = create_session(false);
    h.set_responses(vec![text("hello"), text("Summary."), text("Prefix.")]);
    prompt(&h, "Say hello").await;
    h.session.compact(None).await.unwrap();

    let events = h.events.lock().unwrap();
    let compaction: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentSessionEvent::CompactionStart { .. } | AgentSessionEvent::CompactionEnd { .. }
            )
        })
        .collect();
    assert_eq!(compaction.len(), 2);
    assert!(matches!(
        compaction[0],
        AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Manual
        }
    ));
    assert!(matches!(
        compaction[1],
        AgentSessionEvent::CompactionEnd {
            reason: CompactionReason::Manual,
            aborted: false,
            will_retry: false,
            ..
        }
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentSessionEvent::Agent(cortexcode_agent_types::AgentEvent::MessageEnd { .. })
    )));
}

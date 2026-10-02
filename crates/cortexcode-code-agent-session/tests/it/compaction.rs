//! Ports `test/suite/agent-session-compaction.test.ts`, whose auto-compaction
//! cases are also the six of `test/agent-session-auto-compaction-queue.test.ts`. The TS cases spy on
//! `_runAutoCompaction`; here they assert [`AgentSession::plan_compaction`],
//! the decision `checkCompaction` acts on. Summaries come from the faux
//! provider (the `session_before_compact` extension hook is 12.3's).

use std::sync::Mutex;

use crate::common::{Harness, HarnessOptions};
use cortexcode_agent_types::{AgentMessage, CustomMessage};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, FauxMessageOptions, FauxModelDefinition, FauxResponseStep,
};
use cortexcode_ai_types::{AssistantMessage, StopReason, Usage, UserContent, UserMessage};
use cortexcode_code_agent_session::{
    AgentSessionEvent, CompactionPlan, CompactionReason, PromptOptions,
};
use cortexcode_code_session::FileEntry;

fn now() -> i64 {
    cortexcode_ai_types::now_ms()
}

fn assistant(
    h: &Harness,
    stop_reason: StopReason,
    error: Option<&str>,
    total_tokens: u64,
    timestamp: i64,
) -> AssistantMessage {
    let model = h.faux.get_model();
    let mut message = faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(stop_reason),
            error_message: error.map(String::from),
            timestamp: Some(timestamp),
            ..Default::default()
        },
    );
    message.api = model.api;
    message.provider = model.provider;
    message.model = model.id;
    message.usage = Usage {
        input: total_tokens,
        total_tokens,
        ..Default::default()
    };
    message
}

fn user(text: &str, timestamp: i64) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: UserContent::Text(text.into()),
        timestamp,
    })
}

fn text(t: &str) -> FauxResponseStep {
    faux_assistant_message(t, Default::default()).into()
}

fn compaction_settings(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut settings = serde_json::Map::new();
    settings.insert("compaction".into(), value);
    settings
}

fn compaction_entries(h: &Harness) -> usize {
    h.session
        .session_manager()
        .entries()
        .iter()
        .filter(|e| matches!(e, FileEntry::Compaction { .. }))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn manually_compacts_with_a_generated_summary() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![text("one"), text("two"), text("summary from model")]);
    h.session
        .prompt("one", PromptOptions::default())
        .await
        .unwrap();
    h.session
        .prompt("two", PromptOptions::default())
        .await
        .unwrap();

    let result = h.session.compact(None).await.unwrap();
    assert!(
        result.summary.starts_with("summary from model"),
        "{}",
        result.summary
    );
    assert_eq!(compaction_entries(&h), 1);
    assert_eq!(h.roles()[0], "compactionSummary");
    let ends: Vec<(CompactionReason, bool)> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::CompactionStart { reason } => Some((*reason, true)),
            AgentSessionEvent::CompactionEnd { reason, result, .. } => {
                Some((*reason, result.is_some()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        ends,
        [
            (CompactionReason::Manual, true),
            (CompactionReason::Manual, true)
        ]
    );
    // Compacting again right away has nothing new to fold in.
    assert_eq!(
        h.session.compact(None).await.unwrap_err().0,
        "Already compacted"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_compacting_without_a_model() {
    let h = Harness::new(HarnessOptions::default());
    h.session
        .agent()
        .set_model(cortexcode_agent_core::Agent::new().state().model);
    let error = h.session.compact(None).await.unwrap_err();
    assert!(error.0.starts_with("No model selected"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_compacting_without_configured_auth() {
    let h = Harness::new(HarnessOptions {
        without_auth: true,
        ..Default::default()
    });
    let provider = h.faux.get_model().provider;
    let error = h.session.compact(None).await.unwrap_err();
    assert!(
        error
            .0
            .starts_with(&format!("No API key found for {provider}.")),
        "{error}"
    );
}

/// The pin's test cancels through a `session_before_compact` extension that
/// answers `{cancel: true}` on abort ("Compaction cancelled"); extension hooks
/// arrive with 12.3. Without one, the pin awaits the summary request, which the
/// abort ends with no text, so `compact()` fails with the empty-summary error
/// (what the pinned TUI shows, L2 `compact-cancel`) and nothing is written.
#[tokio::test(flavor = "multi_thread")]
async fn cancels_in_progress_manual_compaction_when_abort_compaction_is_called() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![
        text("one"),
        text("two"),
        FauxResponseStep::async_factory(|_, _, _, _| async {
            // Long enough to be mid-request when the abort lands.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            Ok(faux_assistant_message("too late", Default::default()))
        }),
    ]);
    h.session
        .prompt("one", PromptOptions::default())
        .await
        .unwrap();
    h.session
        .prompt("two", PromptOptions::default())
        .await
        .unwrap();

    let session = h.session.clone();
    let run = tokio::spawn(async move { session.compact(None).await });
    while !h.session.is_compacting() {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    h.session.abort_compaction();
    assert_eq!(
        run.await.unwrap().unwrap_err().0,
        "Summarization produced an empty summary"
    );
    assert!(!h.session.is_compacting());
    assert_eq!(compaction_entries(&h), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn resumes_after_threshold_compaction_when_only_agent_level_queued_messages_exist() {
    let h = Harness::new(HarnessOptions {
        settings: compaction_settings(serde_json::json!({"keepRecentTokens": 1})),
        ..Default::default()
    });
    h.set_responses(vec![text("one"), text("two")]);
    h.session
        .prompt("first", PromptOptions::default())
        .await
        .unwrap();
    h.session
        .prompt("second", PromptOptions::default())
        .await
        .unwrap();

    h.session
        .agent()
        .follow_up(AgentMessage::Custom(CustomMessage {
            custom_type: "test".into(),
            content: UserContent::Text("queued custom".into()),
            display: false,
            details: None,
            timestamp: now(),
        }));
    // Summaries (history, and a split turn's prefix) then the resumed turn.
    h.set_responses(vec![text("summary"), text("summary"), text("after queued")]);
    h.session
        .run_auto_compaction(CompactionReason::Threshold, false)
        .await;
    assert_eq!(compaction_entries(&h), 1);

    // The loop is kicked (after 100ms) to deliver the queued message, and the
    // model answers it.
    let delivered = |h: &Harness| {
        let messages = h.session.messages();
        messages
            .iter()
            .position(|m| matches!(m, AgentMessage::Custom(c) if c.custom_type == "test"))
            .is_some_and(|i| matches!(messages.get(i + 1), Some(AgentMessage::Assistant(_))))
    };
    for _ in 0..200 {
        if delivered(&h) && !h.session.is_streaming() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(delivered(&h), "{:?}", h.roles());
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_retry_overflow_recovery_more_than_once() {
    let h = Harness::new(HarnessOptions::default());
    let errors = std::sync::Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    let _sub = h.session.subscribe(move |event| {
        if let AgentSessionEvent::CompactionEnd {
            error_message: Some(message),
            ..
        } = event
        {
            sink.lock().unwrap().push(message.clone());
        }
    });
    let overflow = assistant(&h, StopReason::Error, Some("prompt is too long"), 0, now());
    assert_eq!(
        h.session.plan_compaction(&overflow, true),
        Some(CompactionPlan::Overflow)
    );
    let mut again = overflow.clone();
    again.timestamp = now() + 1;
    assert_eq!(h.session.plan_compaction(&again, true), None);
    assert!(errors.lock().unwrap().contains(
        &"Context overflow recovery failed after one compact-and-retry attempt. Try reducing context or switching to a larger-context model."
            .to_string()
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn ignores_stale_pre_compaction_assistant_usage_on_pre_prompt_checks() {
    let h = Harness::new(HarnessOptions::default());
    let stale_ts = now() - 10_000;
    let stale = assistant(&h, StopReason::Stop, None, 610_000, stale_ts);
    {
        let mut sm = h.session.session_manager();
        sm.append_message(user("before compaction", stale_ts - 1000));
        sm.append_message(AgentMessage::Assistant(stale.clone()));
        let first_kept = sm.entries()[0].id().unwrap().to_string();
        sm.append_compaction("summary", first_kept, 610_000, None, None, Some(false));
        sm.append_message(user("after compaction", now()));
    }
    assert_eq!(h.session.plan_compaction(&stale, false), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn triggers_threshold_compaction_for_error_messages_using_the_last_successful_usage() {
    let h = Harness::new(HarnessOptions::default());
    let t = now();
    let ok = assistant(&h, StopReason::Stop, None, 190_000, t);
    let failed = assistant(&h, StopReason::Error, Some("529 overloaded"), 0, t + 1000);
    h.session.agent().set_messages(vec![
        user("hello", t - 1000),
        AgentMessage::Assistant(ok),
        user("retry", t + 500),
        AgentMessage::Assistant(failed.clone()),
    ]);
    assert_eq!(
        h.session.plan_compaction(&failed, true),
        Some(CompactionPlan::Threshold)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_trigger_threshold_compaction_for_error_messages_without_prior_usage() {
    let h = Harness::new(HarnessOptions::default());
    let failed = assistant(&h, StopReason::Error, Some("529 overloaded"), 0, now());
    h.session.agent().set_messages(vec![
        user("hello", now() - 1000),
        AgentMessage::Assistant(failed.clone()),
    ]);
    assert_eq!(h.session.plan_compaction(&failed, true), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_trigger_threshold_compaction_when_only_kept_pre_compaction_usage_exists() {
    let h = Harness::new(HarnessOptions::default());
    let pre = now() - 10_000;
    let kept = assistant(&h, StopReason::Stop, None, 190_000, pre);
    {
        let mut sm = h.session.session_manager();
        sm.append_message(user("before compaction", pre - 1000));
        sm.append_message(AgentMessage::Assistant(kept.clone()));
        let first_kept = sm.entries()[0].id().unwrap().to_string();
        sm.append_compaction("summary", first_kept, 190_000, None, None, Some(false));
    }
    let failed = assistant(&h, StopReason::Error, Some("529 overloaded"), 0, now());
    h.session.agent().set_messages(vec![
        user("kept user", pre - 1000),
        AgentMessage::Assistant(kept),
        user("new prompt", now() - 500),
        AgentMessage::Assistant(failed.clone()),
    ]);
    assert_eq!(h.session.plan_compaction(&failed, true), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_trigger_threshold_compaction_below_the_threshold_or_when_disabled() {
    let below = Harness::new(HarnessOptions {
        settings: compaction_settings(serde_json::json!({"enabled": true, "reserveTokens": 1000})),
        models: vec![FauxModelDefinition {
            id: "faux-1".into(),
            context_window: Some(200_000),
            ..Default::default()
        }],
        ..Default::default()
    });
    let disabled = Harness::new(HarnessOptions {
        settings: compaction_settings(serde_json::json!({"enabled": false})),
        ..Default::default()
    });
    let small = assistant(&below, StopReason::Stop, None, 1_000, now());
    assert_eq!(below.session.plan_compaction(&small, true), None);
    let huge = assistant(&disabled, StopReason::Stop, None, 1_000_000, now());
    assert_eq!(disabled.session.plan_compaction(&huge, true), None);
    // …while the same usage over the threshold does compact.
    let big = assistant(&below, StopReason::Stop, None, 199_500, now());
    assert_eq!(
        below.session.plan_compaction(&big, true),
        Some(CompactionPlan::Threshold)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn overflow_after_a_prompt_compacts_and_retries_the_turn() {
    let h = Harness::new(HarnessOptions {
        settings: compaction_settings(serde_json::json!({"keepRecentTokens": 1})),
        ..Default::default()
    });
    h.set_responses(vec![text("first answer")]);
    h.session
        .prompt("first", PromptOptions::default())
        .await
        .unwrap();
    let model = h.faux.get_model();
    h.set_responses(vec![
        FauxResponseStep::factory(move |_, _, _, _| {
            let mut m = faux_assistant_message(
                "",
                FauxMessageOptions {
                    stop_reason: Some(StopReason::Error),
                    error_message: Some("prompt is too long".into()),
                    ..Default::default()
                },
            );
            m.provider = model.provider.clone();
            m.model = model.id.clone();
            Ok(m)
        }),
        text("compacted summary"),
        text("answer after retry"),
    ]);
    h.session
        .prompt("second", PromptOptions::default())
        .await
        .unwrap();
    for _ in 0..300 {
        if h.assistant_texts().last().map(String::as_str) == Some("answer after retry") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(compaction_entries(&h), 1);
    assert_eq!(
        h.assistant_texts().last().map(String::as_str),
        Some("answer after retry")
    );
    let will_retry = h.events.lock().unwrap().iter().find_map(|e| match e {
        AgentSessionEvent::CompactionEnd {
            reason: CompactionReason::Overflow,
            will_retry,
            result,
            ..
        } => Some((*will_retry, result.is_some())),
        _ => None,
    });
    assert_eq!(will_retry, Some((true, true)));
}

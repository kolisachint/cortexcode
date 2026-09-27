//! Ports `test/suite/agent-session-retry-events.test.ts` and
//! `test/agent-session-retry.test.ts` (the extension-ordering cases wait for
//! the extension runtime, 12.3).

mod common;

use std::sync::{Arc, Mutex};

use common::{text_result, tool, Harness, HarnessOptions};
use cortexcode_agent_types::{AgentEvent, AgentMessage};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_text, faux_thinking, faux_tool_call, FauxMessageOptions,
    FauxResponseStep,
};
use cortexcode_ai_types::{AssistantMessageEvent, StopReason};
use cortexcode_code_agent_session::{AgentSessionEvent, PromptOptions};

fn retry_settings(
    enabled: bool,
    max_retries: u64,
    base_delay_ms: u64,
) -> serde_json::Map<String, serde_json::Value> {
    let mut settings = serde_json::Map::new();
    settings.insert(
        "retry".into(),
        serde_json::json!({"enabled": enabled, "maxRetries": max_retries, "baseDelayMs": base_delay_ms}),
    );
    settings
}

fn harness(settings: serde_json::Map<String, serde_json::Value>) -> Harness {
    Harness::new(HarnessOptions {
        settings,
        ..Default::default()
    })
}

fn error(message: &str) -> FauxResponseStep {
    faux_assistant_message(
        "",
        FauxMessageOptions {
            stop_reason: Some(StopReason::Error),
            error_message: Some(message.into()),
            ..Default::default()
        },
    )
    .into()
}

fn text(t: &str) -> FauxResponseStep {
    faux_assistant_message(t, Default::default()).into()
}

/// `start:<attempt>` / `end:<success>` for each retry event.
fn retry_events(h: &Harness) -> Vec<String> {
    h.events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::AutoRetryStart { attempt, .. } => Some(format!("start:{attempt}")),
            AgentSessionEvent::AutoRetryEnd { success, .. } => Some(format!("end:{success}")),
            _ => None,
        })
        .collect()
}

async fn prompt(h: &Harness, text: &str) {
    h.session
        .prompt(text, PromptOptions::default())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn retries_after_a_transient_error_and_succeeds() {
    let h = harness(retry_settings(true, 3, 1));
    h.set_responses(vec![error("overloaded_error"), text("recovered")]);
    prompt(&h, "test").await;
    assert_eq!(retry_events(&h), ["start:1", "end:true"]);
    assert_eq!(h.faux.call_count(), 2);
    assert!(!h.session.is_retrying());
    // The error stays in the session for history, not in the context.
    assert_eq!(h.roles(), ["user", "assistant"]);
    assert_eq!(h.assistant_texts(), ["recovered"]);
    assert_eq!(h.entry_types().len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn retries_multiple_transient_failures_and_succeeds_on_the_final_attempt() {
    let h = harness(retry_settings(true, 3, 1));
    h.set_responses(vec![
        error("overloaded_error"),
        error("overloaded_error"),
        text("success"),
    ]);
    prompt(&h, "test").await;
    assert_eq!(retry_events(&h), ["start:1", "start:2", "end:true"]);
    assert_eq!(h.faux.call_count(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn exhausts_max_retries_and_emits_a_failure_event() {
    let h = harness(retry_settings(true, 2, 1));
    h.set_responses(vec![
        error("overloaded_error"),
        error("overloaded_error"),
        error("overloaded_error"),
    ]);
    prompt(&h, "test").await;
    assert_eq!(retry_events(&h), ["start:1", "start:2", "end:false"]);
    assert_eq!(h.faux.call_count(), 3);
    assert!(!h.session.is_retrying());
    let final_error = h.events.lock().unwrap().iter().find_map(|e| match e {
        AgentSessionEvent::AutoRetryEnd {
            attempt,
            final_error,
            ..
        } => Some((*attempt, final_error.clone())),
        _ => None,
    });
    assert_eq!(final_error, Some((2, Some("overloaded_error".into()))));
}

#[tokio::test(flavor = "multi_thread")]
async fn retries_provider_network_error_failures() {
    let h = harness(retry_settings(true, 3, 1));
    h.set_responses(vec![
        error("Provider finish_reason: network_error"),
        text("Recovered after retry"),
    ]);
    prompt(&h, "Test").await;
    assert_eq!(h.faux.call_count(), 2);
    assert_eq!(retry_events(&h), ["start:1", "end:true"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_retry_when_retry_is_disabled() {
    let h = harness(retry_settings(false, 3, 1));
    h.set_responses(vec![error("overloaded_error")]);
    prompt(&h, "test").await;
    assert_eq!(h.faux.call_count(), 1);
    assert!(retry_events(&h).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_retry_non_retryable_errors() {
    let h = harness(retry_settings(true, 3, 1));
    h.set_responses(vec![error("invalid_api_key")]);
    prompt(&h, "test").await;
    assert_eq!(h.faux.call_count(), 1);
    assert!(retry_events(&h).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancels_retry_sleep_when_abort_retry_is_called() {
    let h = harness(retry_settings(true, 3, 100));
    h.set_responses(vec![error("overloaded_error")]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let _sub = h.session.subscribe(move |event| {
        if let AgentSessionEvent::AutoRetryStart { .. } = event {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    });
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("test", PromptOptions::default()).await });
    rx.await.unwrap();
    h.session.abort_retry();
    run.await.unwrap().unwrap();
    // The cancelled end event follows the sleep's wake-up.
    for _ in 0..100 {
        if retry_events(&h).len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(!h.session.is_retrying());
    let finals: Vec<Option<String>> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::AutoRetryEnd { final_error, .. } => Some(final_error.clone()),
            _ => None,
        })
        .collect();
    assert!(finals.contains(&Some("Retry cancelled".into())));
    assert_eq!(h.faux.call_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn waits_for_the_full_loop_when_retry_recovery_produces_tool_calls() {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let seen = runs.clone();
    let echo = tool("echo", move |params| {
        let t = params["text"].as_str().unwrap_or_default().to_string();
        seen.lock().unwrap().push(t.clone());
        text_result(&format!("echo:{t}"))
    });
    let h = Harness::new(HarnessOptions {
        tools: vec![echo],
        settings: retry_settings(true, 3, 1),
        ..Default::default()
    });
    h.set_responses(vec![
        error("overloaded_error"),
        faux_assistant_message(
            faux_tool_call("echo", serde_json::json!({"text": "hello"}), None),
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into(),
        text("final answer"),
    ]);
    prompt(&h, "test").await;
    assert_eq!(h.faux.call_count(), 3);
    assert_eq!(*runs.lock().unwrap(), ["hello"]);
    assert!(!h.session.is_streaming());
    h.set_responses(vec![text("follow-up answer")]);
    prompt(&h, "follow-up").await;
    assert_eq!(h.faux.call_count(), 4);
}

/// `normalizeEventOrder`: agent events by name (role / tool), runs of
/// `message_update` collapsed.
fn event_order(h: &Harness) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for event in h.events.lock().unwrap().iter() {
        let AgentSessionEvent::Agent(event) = event else {
            continue;
        };
        let label = match event {
            AgentEvent::AgentStart => "agent_start".into(),
            AgentEvent::AgentEnd { .. } => "agent_end".into(),
            AgentEvent::TurnStart => "turn_start".into(),
            AgentEvent::TurnEnd { .. } => "turn_end".into(),
            AgentEvent::MessageStart { message } => format!("message_start:{}", message.role()),
            AgentEvent::MessageUpdate { .. } => "message_update".into(),
            AgentEvent::MessageEnd { message } => format!("message_end:{}", message.role()),
            AgentEvent::ToolExecutionStart { tool_name, .. } => {
                format!("tool_execution_start:{tool_name}")
            }
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update".into(),
            AgentEvent::ToolExecutionEnd { tool_name, .. } => {
                format!("tool_execution_end:{tool_name}")
            }
        };
        if label == "message_update" && out.last().map(String::as_str) == Some("message_update") {
            continue;
        }
        out.push(label);
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_the_expected_event_order_for_a_single_prompt() {
    let h = harness(Default::default());
    h.set_responses(vec![text("hello")]);
    prompt(&h, "hi").await;
    assert_eq!(
        event_order(&h),
        [
            "agent_start",
            "turn_start",
            "message_start:user",
            "message_end:user",
            "message_start:assistant",
            "message_update",
            "message_end:assistant",
            "turn_end",
            "agent_end",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_the_expected_event_order_for_a_tool_call_turn() {
    let echo = tool("echo", |params| {
        text_result(&format!(
            "echo:{}",
            params["text"].as_str().unwrap_or_default()
        ))
    });
    let h = Harness::new(HarnessOptions {
        tools: vec![echo],
        ..Default::default()
    });
    h.set_responses(vec![
        faux_assistant_message(
            faux_tool_call("echo", serde_json::json!({"text": "hello"}), None),
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into(),
        text("done"),
    ]);
    prompt(&h, "hi").await;
    assert_eq!(
        event_order(&h),
        [
            "agent_start",
            "turn_start",
            "message_start:user",
            "message_end:user",
            "message_start:assistant",
            "message_update",
            "message_end:assistant",
            "tool_execution_start:echo",
            "tool_execution_end:echo",
            "message_start:toolResult",
            "message_end:toolResult",
            "turn_end",
            "turn_start",
            "message_start:assistant",
            "message_update",
            "message_end:assistant",
            "turn_end",
            "agent_end",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_streaming_deltas_for_text_thinking_and_tool_calls() {
    let h = harness(retry_settings(false, 0, 1));
    h.set_responses(vec![faux_assistant_message(
        vec![
            faux_thinking("plan"),
            faux_text("answer"),
            faux_tool_call("echo", serde_json::json!({"text": "hello"}), None),
        ],
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
    .into()]);
    let _ = h.session.prompt("hi", PromptOptions::default()).await;
    let kinds: Vec<&'static str> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::Agent(AgentEvent::MessageUpdate {
                assistant_message_event,
                ..
            }) => Some(match **assistant_message_event {
                AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
                AssistantMessageEvent::TextDelta { .. } => "text_delta",
                AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
                _ => "other",
            }),
            _ => None,
        })
        .collect();
    for kind in ["thinking_delta", "text_delta", "toolcall_delta"] {
        assert!(kinds.contains(&kind), "{kind}: {kinds:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_agent_end_for_error_responses() {
    let h = harness(Default::default());
    h.set_responses(vec![error("broken")]);
    prompt(&h, "hi").await;
    let last = h.events.lock().unwrap().last().cloned();
    assert!(matches!(
        last,
        Some(AgentSessionEvent::Agent(AgentEvent::AgentEnd { .. }))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn emits_agent_end_for_aborted_runs_and_keeps_the_aborted_message() {
    let h = harness(Default::default());
    h.set_responses(vec![text(&"x".repeat(20_000))]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let tx = Mutex::new(Some(tx));
    let _sub = h.session.subscribe(move |event| {
        if let AgentSessionEvent::Agent(AgentEvent::MessageUpdate { .. }) = event {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    });
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("hi", PromptOptions::default()).await });
    rx.await.unwrap();
    h.session.abort().await;
    run.await.unwrap().unwrap();
    let last = h.events.lock().unwrap().last().cloned();
    assert!(matches!(
        last,
        Some(AgentSessionEvent::Agent(AgentEvent::AgentEnd { .. }))
    ));
    match h.session.messages().last() {
        Some(AgentMessage::Assistant(a)) => assert_eq!(a.stop_reason, StopReason::Aborted),
        other => panic!("{other:?}"),
    }
}

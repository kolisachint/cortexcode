//! Session-level events in hoocode's `--mode json` / RPC wire shape
//! (`AgentSessionEvent` in agent-session.ts; key order as emitted by
//! agent-session.ts, agent-session-compaction.ts and agent-session-retry.ts).

use cortexcode_agent_compaction::CompactionResult;
use cortexcode_agent_types::AgentEvent;
use cortexcode_ai_types::ThinkingLevel;
use cortexcode_code_agent_session::{AgentSessionEvent, CompactionReason};

fn json(event: AgentSessionEvent) -> String {
    event.to_json().to_string()
}

#[test]
fn agent_events_pass_through() {
    assert_eq!(
        json(AgentSessionEvent::Agent(AgentEvent::AgentStart)),
        r#"{"type":"agent_start"}"#
    );
}

#[test]
fn queue_session_info_and_thinking_level() {
    assert_eq!(
        json(AgentSessionEvent::QueueUpdate {
            steering: vec!["s".into()],
            follow_up: vec![],
        }),
        r#"{"type":"queue_update","steering":["s"],"followUp":[]}"#
    );
    assert_eq!(
        json(AgentSessionEvent::SessionInfoChanged {
            name: Some("work".into())
        }),
        r#"{"type":"session_info_changed","name":"work"}"#
    );
    // `name: undefined` disappears from JSON.stringify output.
    assert_eq!(
        json(AgentSessionEvent::SessionInfoChanged { name: None }),
        r#"{"type":"session_info_changed"}"#
    );
    assert_eq!(
        json(AgentSessionEvent::ThinkingLevelChanged {
            level: ThinkingLevel::XHigh
        }),
        r#"{"type":"thinking_level_changed","level":"xhigh"}"#
    );
}

#[test]
fn compaction_events() {
    assert_eq!(
        json(AgentSessionEvent::CompactionStart {
            reason: CompactionReason::Threshold
        }),
        r#"{"type":"compaction_start","reason":"threshold"}"#
    );
    assert_eq!(
        json(AgentSessionEvent::CompactionEnd {
            reason: CompactionReason::Manual,
            result: Some(CompactionResult {
                summary: "sum".into(),
                first_kept_entry_id: "e1".into(),
                tokens_before: 1000,
                tokens_after: Some(200),
                details: None,
            }),
            aborted: false,
            will_retry: false,
            error_message: None,
        }),
        r#"{"type":"compaction_end","reason":"manual","result":{"summary":"sum","firstKeptEntryId":"e1","tokensBefore":1000,"tokensAfter":200},"aborted":false,"willRetry":false}"#
    );
    assert_eq!(
        json(AgentSessionEvent::CompactionEnd {
            reason: CompactionReason::Overflow,
            result: None,
            aborted: false,
            will_retry: false,
            error_message: Some("Compaction failed: boom".into()),
        }),
        r#"{"type":"compaction_end","reason":"overflow","aborted":false,"willRetry":false,"errorMessage":"Compaction failed: boom"}"#
    );
}

#[test]
fn auto_retry_events() {
    assert_eq!(
        json(AgentSessionEvent::AutoRetryStart {
            attempt: 1,
            max_attempts: 3,
            delay_ms: 2000,
            error_message: "overloaded".into(),
        }),
        r#"{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":2000,"errorMessage":"overloaded"}"#
    );
    assert_eq!(
        json(AgentSessionEvent::AutoRetryEnd {
            success: true,
            attempt: 1,
            final_error: None,
        }),
        r#"{"type":"auto_retry_end","success":true,"attempt":1}"#
    );
    assert_eq!(
        json(AgentSessionEvent::AutoRetryEnd {
            success: false,
            attempt: 2,
            final_error: Some("Retry cancelled".into()),
        }),
        r#"{"type":"auto_retry_end","success":false,"attempt":2,"finalError":"Retry cancelled"}"#
    );
}

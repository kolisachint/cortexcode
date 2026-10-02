//! subagent-events.test.ts and subagent-line-classify.test.ts.

use cortexcode_code_subagents::events::*;
use serde_json::{json, Value};

#[test]
fn parent_progress_forwarding_stays_in_lockstep_with_the_shared_set() {
    assert_eq!(FORWARDED_SUBAGENT_EVENTS, SUBAGENT_PROGRESS_EVENTS);
}

#[test]
fn the_child_emits_every_event_the_parent_consumes() {
    for t in SUBAGENT_PROGRESS_EVENTS {
        assert!(SUBAGENT_STDOUT_EVENT_TYPES.contains(t));
    }
    assert!(SUBAGENT_STDOUT_EVENT_TYPES.contains(&"message_end"));
}

#[test]
fn the_child_drops_the_per_delta_firehose_at_the_source() {
    for t in [
        "message_start",
        "message_update",
        "tool_execution_update",
        "turn_start",
    ] {
        assert!(!SUBAGENT_STDOUT_EVENT_TYPES.contains(&t));
    }
}

fn progress(event: Value) -> SubagentStdoutLine {
    SubagentStdoutLine::Progress(event.as_object().unwrap().clone())
}

#[test]
fn treats_a_ping_line_as_a_heartbeat() {
    assert_eq!(
        classify_subagent_line(r#"{"ping":true}"#),
        SubagentStdoutLine::Heartbeat
    );
}

#[test]
fn forwards_coarse_lifecycle_events_as_progress() {
    for t in ["turn_end", "tool_execution_start", "tool_execution_end"] {
        let event = json!({"type": t, "foo": 1});
        assert_eq!(classify_subagent_line(&event.to_string()), progress(event));
    }
}

#[test]
fn drops_per_delta_and_large_body_events() {
    for t in [
        "message_start",
        "message_update",
        "message_end",
        "tool_execution_update",
        "turn_start",
    ] {
        assert_eq!(
            classify_subagent_line(&json!({"type": t}).to_string()),
            SubagentStdoutLine::Ignore
        );
    }
}

#[test]
fn ignores_non_json_non_object_and_empty_lines() {
    for line in ["", "   ", "not json", "[1,2,3]", r#"{"type": "turn_end""#] {
        assert_eq!(classify_subagent_line(line), SubagentStdoutLine::Ignore);
    }
}

#[test]
fn ignores_an_event_whose_type_is_not_a_string() {
    assert_eq!(
        classify_subagent_line(r#"{"type":42}"#),
        SubagentStdoutLine::Ignore
    );
}

#[test]
fn tolerates_surrounding_whitespace() {
    assert_eq!(
        classify_subagent_line(r#"  {"type":"turn_end"}  "#),
        progress(json!({"type": "turn_end"}))
    );
}

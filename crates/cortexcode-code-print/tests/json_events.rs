//! `--mode json` wire shapes: each event must print exactly as hoocode's
//! `JSON.stringify(event)` (docs/json.md; lines recorded from the pinned
//! hoocode with `migration/tui-parity/scenarios/json-basic.json`).

use cortexcode_agent_types::{AgentEvent, AgentMessage, AgentToolResult};
use cortexcode_ai_types::{
    AssistantMessage, AssistantMessageEvent, Content, StopReason, TextContent, ThinkingContent,
    ToolCallContent, ToolResultMessage, Usage,
};
use cortexcode_code_print::json_line;
use serde_json::json;

fn line(event: &AgentEvent) -> String {
    json_line(&event.to_json())
}

fn text(t: &str) -> Content {
    Content::Text(TextContent {
        text: t.into(),
        text_signature: None,
    })
}

fn read_call() -> Content {
    Content::ToolCall(ToolCallContent {
        id: "call_read_1".into(),
        name: "read".into(),
        arguments: json!({"path": "notes.txt", "limit": 2}),
        thought_signature: None,
    })
}

fn assistant(content: Vec<Content>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-completions".into(),
        provider: "mock".into(),
        model: "mock-model".into(),
        usage: Usage {
            input: 100,
            output: 20,
            total_tokens: 120,
            ..Default::default()
        },
        stop_reason,
        timestamp: 1,
        response_id: Some("chatcmpl-mock".into()),
        ..Default::default()
    }
}

const ASSISTANT: &str = r#"{"role":"assistant","content":[{"type":"text","text":"Let me read it."},{"type":"toolCall","id":"call_read_1","name":"read","arguments":{"path":"notes.txt","limit":2}}],"api":"openai-completions","provider":"mock","model":"mock-model","usage":{"input":100,"output":20,"cacheRead":0,"cacheWrite":0,"totalTokens":120,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"toolUse","timestamp":1,"responseId":"chatcmpl-mock"}"#;

const TOOL_RESULT: &str = r#"{"role":"toolResult","toolCallId":"call_read_1","toolName":"read","content":[{"type":"text","text":"alpha"}],"isError":false,"timestamp":2}"#;

fn keys(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn lifecycle_events_are_bare_types() {
    assert_eq!(
        line(&AgentEvent::AgentStart),
        "{\"type\":\"agent_start\"}\n"
    );
    assert_eq!(line(&AgentEvent::TurnStart), "{\"type\":\"turn_start\"}\n");
}

#[test]
fn assistant_messages_keep_hoocode_key_order_and_integral_costs() {
    let message = assistant(
        vec![text("Let me read it."), read_call()],
        StopReason::ToolUse,
    );
    assert_eq!(
        line(&AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(message)
        }),
        format!("{{\"type\":\"message_end\",\"message\":{ASSISTANT}}}\n")
    );
}

#[test]
fn message_update_carries_content_index_and_end_payloads() {
    let partial = assistant(
        vec![text("Let me read it."), read_call()],
        StopReason::ToolUse,
    );
    let update = |event: AssistantMessageEvent| {
        let value = AgentEvent::MessageUpdate {
            assistant_message_event: Box::new(event),
            message: AgentMessage::Assistant(partial.clone()),
        }
        .to_json();
        assert_eq!(keys(&value), ["type", "assistantMessageEvent", "message"]);
        let mut event = value["assistantMessageEvent"].clone();
        assert_eq!(event["partial"], value["message"]);
        assert_eq!(keys(&event).last(), Some(&"partial"));
        event.as_object_mut().unwrap().shift_remove("partial");
        event.to_string()
    };
    assert_eq!(
        update(AssistantMessageEvent::TextStart {
            partial: partial.clone(),
            index: 0
        }),
        r#"{"type":"text_start","contentIndex":0}"#
    );
    assert_eq!(
        update(AssistantMessageEvent::TextDelta {
            partial: partial.clone(),
            index: 0,
            delta: "Let me r".into()
        }),
        r#"{"type":"text_delta","contentIndex":0,"delta":"Let me r"}"#
    );
    assert_eq!(
        update(AssistantMessageEvent::TextEnd {
            partial: partial.clone(),
            index: 0
        }),
        r#"{"type":"text_end","contentIndex":0,"content":"Let me read it."}"#
    );
    assert_eq!(
        update(AssistantMessageEvent::ToolCallStart {
            partial: partial.clone(),
            index: 1
        }),
        r#"{"type":"toolcall_start","contentIndex":1}"#
    );
    assert_eq!(
        update(AssistantMessageEvent::ToolCallDelta {
            partial: partial.clone(),
            index: 1,
            delta: "{\"path\":".into()
        }),
        r#"{"type":"toolcall_delta","contentIndex":1,"delta":"{\"path\":"}"#
    );
    assert_eq!(
        update(AssistantMessageEvent::ToolCallEnd {
            partial: partial.clone(),
            index: 1
        }),
        r#"{"type":"toolcall_end","contentIndex":1,"toolCall":{"type":"toolCall","id":"call_read_1","name":"read","arguments":{"path":"notes.txt","limit":2}}}"#
    );
}

#[test]
fn thinking_events_carry_the_thinking_text_on_end() {
    let partial = assistant(
        vec![Content::Thinking(ThinkingContent {
            thinking: "hmm".into(),
            ..Default::default()
        })],
        StopReason::Stop,
    );
    let start = AssistantMessageEvent::ThinkingStart {
        partial: partial.clone(),
        index: 0,
    }
    .to_json();
    assert_eq!(keys(&start), ["type", "contentIndex", "partial"]);
    assert_eq!(start["type"], "thinking_start");
    let end = AssistantMessageEvent::ThinkingEnd { partial, index: 0 }.to_json();
    assert_eq!(keys(&end), ["type", "contentIndex", "content", "partial"]);
    assert_eq!(end["type"], "thinking_end");
    assert_eq!(end["content"], "hmm");
}

#[test]
fn done_and_error_carry_the_stop_reason() {
    let done = AssistantMessageEvent::Done {
        message: assistant(vec![], StopReason::Length),
    }
    .to_json();
    assert_eq!(keys(&done), ["type", "reason", "message"]);
    assert_eq!(done["reason"], "length");
    let mut failed = assistant(vec![], StopReason::Aborted);
    failed.error_message = Some("Request was aborted".into());
    let error = AssistantMessageEvent::Error { error: failed }.to_json();
    assert_eq!(keys(&error), ["type", "reason", "error"]);
    assert_eq!(error["reason"], "aborted");
    assert_eq!(error["error"]["errorMessage"], "Request was aborted");
}

#[test]
fn tool_execution_events_match_agent_loop() {
    let result = AgentToolResult {
        content: vec![text("alpha\nbeta")],
        details: serde_json::Value::Null,
        terminate: false,
    };
    assert_eq!(
        line(&AgentEvent::ToolExecutionStart {
            tool_call_id: "call_read_1".into(),
            tool_name: "read".into(),
            args: json!({"path": "notes.txt", "limit": 2}),
        }),
        "{\"type\":\"tool_execution_start\",\"toolCallId\":\"call_read_1\",\"toolName\":\"read\",\"args\":{\"path\":\"notes.txt\",\"limit\":2}}\n"
    );
    // `details: undefined` is left out; `terminate` only when set.
    assert_eq!(
        line(&AgentEvent::ToolExecutionEnd {
            tool_call_id: "call_read_1".into(),
            tool_name: "read".into(),
            result: result.clone(),
            is_error: false,
        }),
        "{\"type\":\"tool_execution_end\",\"toolCallId\":\"call_read_1\",\"toolName\":\"read\",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"alpha\\nbeta\"}]},\"isError\":false}\n"
    );
    let partial = AgentToolResult {
        details: json!({"lines": 2}),
        terminate: true,
        ..result
    };
    assert_eq!(
        AgentEvent::ToolExecutionUpdate {
            tool_call_id: "c".into(),
            tool_name: "bash".into(),
            args: json!({}),
            partial_result: partial,
        }
        .to_json()
        .to_string(),
        r#"{"type":"tool_execution_update","toolCallId":"c","toolName":"bash","args":{},"partialResult":{"content":[{"type":"text","text":"alpha\nbeta"}],"details":{"lines":2},"terminate":true}}"#
    );
}

#[test]
fn turn_end_and_agent_end_carry_role_tagged_messages() {
    let message = assistant(
        vec![text("Let me read it."), read_call()],
        StopReason::ToolUse,
    );
    let tool_result = ToolResultMessage {
        tool_call_id: "call_read_1".into(),
        tool_name: "read".into(),
        content: vec![text("alpha")],
        details: None,
        is_error: false,
        timestamp: 2,
    };
    assert_eq!(
        line(&AgentEvent::TurnEnd {
            message: message.clone(),
            tool_results: vec![tool_result.clone()],
        }),
        format!(
            "{{\"type\":\"turn_end\",\"message\":{ASSISTANT},\"toolResults\":[{TOOL_RESULT}]}}\n"
        )
    );
    assert_eq!(
        line(&AgentEvent::AgentEnd {
            messages: vec![
                AgentMessage::Assistant(message),
                AgentMessage::ToolResult(tool_result)
            ],
        }),
        format!("{{\"type\":\"agent_end\",\"messages\":[{ASSISTANT},{TOOL_RESULT}]}}\n")
    );
}

#[test]
fn fractional_costs_stay_fractional() {
    let mut message = assistant(vec![], StopReason::Stop);
    message.usage.cost.input = 0.25;
    message.usage.cost.total = 3.0;
    let value = cortexcode_ai_types::assistant_message_json(&message).to_string();
    assert!(
        value
            .contains(r#""cost":{"input":0.25,"output":0,"cacheRead":0,"cacheWrite":0,"total":3}"#),
        "{value}"
    );
}

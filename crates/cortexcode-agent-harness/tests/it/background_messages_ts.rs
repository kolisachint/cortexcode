//! Port of the pin's `coding-agent/test/background-messages.test.ts`.

use cortexcode_agent_harness::messages::{
    create_background_placeholder_text, create_background_task_message, describe_background_tool,
    BACKGROUND_TASK_CUSTOM_TYPE,
};
use cortexcode_agent_types::{AgentToolCall, AgentToolResult, BackgroundToolResult, CustomMessage};
use cortexcode_ai_types::{Content, UserContent};
use serde_json::{json, Value};

fn call(name: &str, arguments: Value) -> AgentToolCall {
    AgentToolCall {
        id: "tc-1".into(),
        name: name.into(),
        arguments,
    }
}

fn bg_result(name: &str, args: Value, text: &str, is_error: bool) -> BackgroundToolResult {
    BackgroundToolResult {
        tool_call: call(name, args),
        result: AgentToolResult {
            content: vec![Content::text(text)],
            details: Value::Null,
            terminate: false,
        },
        is_error,
    }
}

fn texts(message: &CustomMessage) -> Vec<String> {
    match &message.content {
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                Content::Text(t) => t.text.clone(),
                other => panic!("{other:?}"),
            })
            .collect(),
        UserContent::Text(t) => vec![t.clone()],
    }
}

fn details_match(message: &CustomMessage, expected: Value) {
    let details = message.details.as_ref().unwrap();
    for (k, v) in expected.as_object().unwrap() {
        assert_eq!(&details[k], v, "{k}");
    }
}

#[test]
fn describes_a_task_call_by_subagent_type_and_description() {
    let info = describe_background_tool(&call(
        "Task",
        json!({"subagent_type": "explore", "description": "find the bug", "prompt": "long prompt..."}),
    ));
    assert!(!info.is_mcp_tool);
    assert_eq!(info.subagent_type, "explore");
    assert!(info.label.contains("explore"));
    assert_eq!(info.summary.as_deref(), Some("find the bug"));
}

#[test]
fn falls_back_to_the_prompts_first_line() {
    let info = describe_background_tool(&call(
        "Task",
        json!({"subagent_type": "explore", "prompt": "first line\nsecond line"}),
    ));
    assert_eq!(info.summary.as_deref(), Some("first line"));
}

#[test]
fn describes_an_mcp_tool_by_its_de_prefixed_name_and_args() {
    let info = describe_background_tool(&call(
        "mcp_web_fetch",
        json!({"url": "https://example.com", "limit": 5}),
    ));
    assert!(info.is_mcp_tool);
    assert_eq!(info.subagent_type, "mcp_web_fetch");
    assert!(info.label.contains("web_fetch") && !info.label.contains("mcp_web_fetch"));
    assert!(info.summary.unwrap().contains("url: https://example.com"));
}

#[test]
fn subagent_placeholder_is_one_line_and_finish_passes_the_notification_through() {
    let args = json!({"subagent_type": "review", "description": "review the diff"});
    let placeholder = create_background_placeholder_text(&call("Task", args.clone()));
    let finish = create_background_task_message(&bg_result(
        "Task",
        args.clone(),
        "review#1 finished ✓ — looks good",
        false,
    ));
    let label = describe_background_tool(&call("Task", args)).label;
    assert!(placeholder.contains(&label));
    assert_eq!(placeholder.split('\n').count(), 1);
    assert!(placeholder.contains("TaskOutput"));
    assert_eq!(texts(&finish), ["review#1 finished ✓ — looks good"]);
    assert_eq!(finish.custom_type, BACKGROUND_TASK_CUSTOM_TYPE);
    details_match(
        &finish,
        json!({"subagentType": "review", "isMcpTool": false, "isError": false}),
    );
}

#[test]
fn mcp_placeholder_is_compact_and_finish_keeps_header_and_body() {
    let args = json!({"url": "https://example.com"});
    let placeholder = create_background_placeholder_text(&call("mcp_web_fetch", args.clone()));
    let finish =
        create_background_task_message(&bg_result("mcp_web_fetch", args.clone(), "{}", false));
    let label = describe_background_tool(&call("mcp_web_fetch", args)).label;
    assert!(placeholder.contains(&label));
    assert_eq!(placeholder.split('\n').count(), 1);
    let t = texts(&finish);
    assert!(t[0].contains(&label));
    assert_eq!(t[1], "{}");
    details_match(&finish, json!({"isMcpTool": true}));
}

#[test]
fn marks_a_failed_subagent_finish_via_details() {
    let finish = create_background_task_message(&bg_result(
        "Task",
        json!({"subagent_type": "explore"}),
        "explore#1 failed ✗ — boom",
        true,
    ));
    assert_eq!(texts(&finish)[0], "explore#1 failed ✗ — boom");
    details_match(&finish, json!({"isError": true}));
}

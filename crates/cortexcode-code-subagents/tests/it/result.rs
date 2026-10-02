//! subagent-result.test.ts: `buildSubagentResult`, `writeSubagentResult`
//! and `buildTaskForest`.

use cortexcode_agent_types::AgentMessage;
use cortexcode_ai_types::{AssistantMessage, Content, StopReason, ToolCallContent};
use cortexcode_code_subagents::output_verifier::OutputVerifier;
use cortexcode_code_subagents::result::*;
use cortexcode_code_task_store::{Task, TaskSource, TaskStatus};
use serde_json::json;

fn assistant(content: Vec<Content>, stop_reason: StopReason, error: Option<&str>) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content,
        stop_reason,
        error_message: error.map(String::from),
        ..Default::default()
    })
}

fn assistant_text(text: &str) -> AgentMessage {
    assistant(vec![Content::text(text)], StopReason::Stop, None)
}

fn assistant_tool_call(name: &str, args: serde_json::Value) -> AgentMessage {
    assistant(
        vec![Content::ToolCall(ToolCallContent {
            id: "x".into(),
            name: name.into(),
            arguments: args,
            thought_signature: None,
        })],
        StopReason::ToolUse,
        None,
    )
}

fn build(messages: &[AgentMessage]) -> SubagentResultFile {
    build_subagent_result(messages, None, BuildSubagentResultOptions::default())
}

#[test]
fn uses_the_last_assistant_text_as_the_summary() {
    let result = build(&[assistant_text("first"), assistant_text("final answer")]);
    assert_eq!(result.summary, "final answer");
    assert_eq!(result.status, ResultStatus::Complete);
    assert!(result.confidence >= 0.5);
}

#[test]
fn collects_changed_files_from_edit_write_tool_calls() {
    let result = build(&[
        assistant_tool_call("edit", json!({"path": "src/a.ts"})),
        assistant_tool_call("write", json!({"file_path": "src/b.ts"})),
        assistant_tool_call("read", json!({"path": "src/c.ts"})),
        assistant_text("done"),
    ]);
    let mut files = result.files_changed.clone();
    files.sort();
    assert_eq!(files, vec!["src/a.ts", "src/b.ts"]);
}

#[test]
fn marks_failed_when_the_final_assistant_message_is_an_error() {
    let errored = assistant(vec![], StopReason::Error, Some("boom"));
    let result = build(&[assistant_text("partial"), errored]);
    assert_eq!(result.status, ResultStatus::Failed);
}

#[test]
fn surfaces_the_provider_error_message_in_the_summary_on_failure() {
    let errored = assistant(
        vec![Content::text("partial progress")],
        StopReason::Error,
        Some("Anthropic usage limit reached. Please try again later."),
    );
    let result = build(&[errored]);
    assert_eq!(result.status, ResultStatus::Failed);
    assert!(result.summary.contains("usage limit reached"));
    assert!(result.confidence >= 0.5);
}

#[test]
fn falls_back_to_a_placeholder_summary_without_assistant_text() {
    let result = build(&[assistant_tool_call("edit", json!({"path": "x.ts"}))]);
    assert!(!result.summary.is_empty());
    assert!(result.confidence >= 0.5);
}

#[test]
fn reports_a_partial_result_when_stopped_at_the_turn_cap() {
    let aborted = assistant(
        vec![Content::text(
            "Found the bug in auth.ts; ran out of turns before fixing it.",
        )],
        StopReason::Aborted,
        None,
    );
    let result = build_subagent_result(
        &[assistant_text("investigating"), aborted],
        None,
        BuildSubagentResultOptions {
            reached_max_turns: true,
        },
    );
    assert_eq!(result.status, ResultStatus::Partial);
    assert!(result.confidence >= 0.5);
    assert!(result.summary.contains("Found the bug"));
}

#[test]
fn yields_a_verifier_passing_partial_result_without_assistant_text() {
    let result = build_subagent_result(
        &[assistant_tool_call("edit", json!({"path": "x.ts"}))],
        None,
        BuildSubagentResultOptions {
            reached_max_turns: true,
        },
    );
    assert_eq!(result.status, ResultStatus::Partial);
    assert!(!result.summary.is_empty());
    assert!(result.confidence >= 0.5);
}

#[test]
fn produces_output_that_passes_output_verifier() {
    let cwd = tempfile::tempdir().unwrap();
    let result = build(&[assistant_text("all good")]);
    write_subagent_result(cwd.path(), "task-1", &result);

    let path = cwd
        .path()
        .join(cortexcode_code_paths::CONFIG_DIR_NAME)
        .join("dispatch/task-1/result.json");
    let text = std::fs::read_to_string(&path).unwrap();
    // JSON.stringify(result, null, 2)
    assert_eq!(
        text,
        "{\n  \"summary\": \"all good\",\n  \"files_changed\": [],\n  \"confidence\": 0.9,\n  \"status\": \"complete\"\n}"
    );
    assert!(OutputVerifier::new(cwd.path()).verify("task-1", None).valid);
}

fn task(id: u64, source: Option<TaskSource>, mode: Option<&str>, parent: Option<u64>) -> Task {
    Task {
        id,
        title: format!("task-{id}"),
        status: TaskStatus::Done,
        source,
        subagent_mode: mode.map(String::from),
        agent: None,
        parent_task_id: parent,
        linked_task_id: None,
        note: None,
        todo_content: None,
        created_at: 0,
        updated_at: 0,
        usage: None,
    }
}

#[test]
fn forest_nests_children_under_their_parent() {
    let tasks = vec![
        task(1, Some(TaskSource::Subagent), Some("explore"), None),
        task(2, Some(TaskSource::Subagent), Some("review"), Some(1)),
        task(3, Some(TaskSource::Mcp), Some("web"), Some(2)),
    ];
    let forest = build_task_forest(&tasks);
    assert_eq!(forest.len(), 1);
    assert_eq!(forest[0].id, 1);
    assert_eq!(forest[0].subagent_mode.as_deref(), Some("explore"));
    assert_eq!(
        forest[0].children.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![2]
    );
    let grandchild = &forest[0].children[0].children[0];
    assert_eq!(grandchild.id, 3);
    assert_eq!(grandchild.source, Some(TaskSource::Mcp));
    assert_eq!(
        forest[0].to_json()["children"][0]["children"][0],
        json!({"id": 3, "title": "task-3", "status": "done", "source": "mcp",
               "subagentMode": "web", "children": []})
    );
}

#[test]
fn forest_returns_multiple_roots_and_preserves_sibling_order() {
    let tasks = vec![
        task(1, Some(TaskSource::Subagent), None, None),
        task(2, Some(TaskSource::Subagent), None, None),
        task(3, Some(TaskSource::Subagent), None, Some(1)),
        task(4, Some(TaskSource::Subagent), None, Some(1)),
    ];
    let forest = build_task_forest(&tasks);
    assert_eq!(forest.iter().map(|n| n.id).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(
        forest[0].children.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![3, 4]
    );
}

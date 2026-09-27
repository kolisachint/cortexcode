//! Ports `test/suite/agent-session-bash-persistence.test.ts`.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use common::{text_result, tool, Harness, HarnessOptions, WaitTool};
use cortexcode_agent_types::{AgentEvent, AgentMessage};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_tool_call, FauxMessageOptions, FauxResponseStep,
};
use cortexcode_ai_types::{StopReason, UserContent};
use cortexcode_code_agent_session::{AgentSessionEvent, PromptOptions};
use cortexcode_code_session::FileEntry;
use cortexcode_code_tool_api::ToolError;
use cortexcode_code_tool_bash::{BashExecOptions, BashOperations, BashResult};

fn ok_result() -> BashResult {
    BashResult {
        output: "hi".into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
    }
}

fn last_role(h: &Harness) -> String {
    h.roles().last().cloned().unwrap_or_default()
}

fn tool_use(name: &str, args: serde_json::Value) -> FauxResponseStep {
    faux_assistant_message(
        faux_tool_call(name, args, None),
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
    .into()
}

#[tokio::test(flavor = "multi_thread")]
async fn records_bash_results_immediately_while_idle() {
    let h = Harness::new(HarnessOptions::default());
    h.session.record_bash_result("echo hi", &ok_result(), false);
    assert!(!h.session.has_pending_bash_messages());
    assert_eq!(last_role(&h), "bashExecution");
    assert!(h.entry_types().contains(&"message".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn defers_bash_results_while_streaming_and_flushes_them_before_the_next_prompt() {
    let wait = WaitTool::new();
    let h = Harness::new(HarnessOptions {
        tools: vec![wait.definition.clone()],
        ..Default::default()
    });
    h.set_responses(vec![
        tool_use("wait", serde_json::json!({})),
        faux_assistant_message("done", Default::default()).into(),
        faux_assistant_message("after flush", Default::default()).into(),
    ]);
    let (started, _sub) = common::wait_for_tool_start(&h.session, "wait");
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("start", PromptOptions::default()).await });
    started.await.unwrap();

    h.session.record_bash_result("echo hi", &ok_result(), false);
    let has_bash = |h: &Harness| h.roles().iter().any(|r| r == "bashExecution");
    assert!(h.session.has_pending_bash_messages());
    assert!(!has_bash(&h));

    wait.release();
    run.await.unwrap().unwrap();
    assert!(h.session.has_pending_bash_messages());
    assert!(!has_bash(&h));

    h.session
        .prompt("next turn", PromptOptions::default())
        .await
        .unwrap();
    assert!(!h.session.has_pending_bash_messages());
    assert!(has_bash(&h));
    assert!(h.entry_types().iter().filter(|t| *t == "message").count() > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn executes_bash_commands_and_records_the_result() {
    let h = Harness::new(HarnessOptions::default());
    let result = h
        .session
        .execute_bash("printf 'hello'", None, false, None)
        .unwrap();
    assert!(result.output.contains("hello"));
    assert_eq!(last_role(&h), "bashExecution");
    let Some(AgentMessage::BashExecution(message)) = h.session.messages().pop() else {
        panic!("no bash message");
    };
    assert_eq!(message.command, "printf 'hello'");
    assert_eq!(message.exit_code, Some(0));
}

/// Operations that wait for the abort signal.
struct HangUntilAborted;

impl BashOperations for HangUntilAborted {
    fn exec(
        &self,
        _command: &str,
        _cwd: &Path,
        options: BashExecOptions<'_>,
    ) -> Result<Option<i32>, ToolError> {
        let signal = options.signal.expect("signal");
        while !signal.aborted() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err("aborted".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancels_running_bash_commands_with_abort_bash() {
    let h = Harness::new(HarnessOptions::default());
    let session = h.session.clone();
    let run = std::thread::spawn(move || {
        session
            .execute_bash("sleep", None, false, Some(&HangUntilAborted))
            .unwrap()
    });
    while !h.session.is_bash_running() {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    h.session.abort_bash();
    let result = run.join().unwrap();
    assert!(result.cancelled);
    assert!(!h.session.is_bash_running());
}

#[tokio::test(flavor = "multi_thread")]
async fn persists_user_assistant_tool_result_and_custom_messages_in_order() {
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
        tool_use("echo", serde_json::json!({"text": "hello"})),
        faux_assistant_message("done", Default::default()).into(),
    ]);
    h.session
        .send_custom_message(
            "note",
            UserContent::Text("hello".into()),
            true,
            Some(serde_json::json!({"a": 1})),
            false,
            None,
        )
        .await
        .unwrap();
    h.session
        .prompt("start", PromptOptions::default())
        .await
        .unwrap();
    assert_eq!(
        h.entry_types(),
        ["custom_message", "message", "message", "message", "message"]
    );
    assert_eq!(
        h.roles(),
        ["custom", "user", "assistant", "toolResult", "assistant"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn does_not_emit_message_end_for_bash_execution_messages() {
    let h = Harness::new(HarnessOptions::default());
    let roles = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = roles.clone();
    let _sub = h.session.subscribe(move |event| {
        if let AgentSessionEvent::Agent(AgentEvent::MessageEnd { message }) = event {
            sink.lock().unwrap().push(message.role().to_string());
        }
    });
    h.session.record_bash_result("echo hi", &ok_result(), false);
    assert!(roles.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn persists_aborted_assistant_messages() {
    let h = Harness::new(HarnessOptions::default());
    h.set_responses(vec![faux_assistant_message(
        "x".repeat(20_000),
        Default::default(),
    )
    .into()]);
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

    let manager = h.session.session_manager();
    match manager.entries().last() {
        Some(FileEntry::Message {
            message: AgentMessage::Assistant(assistant),
            ..
        }) => assert_eq!(assistant.stop_reason, StopReason::Aborted),
        other => panic!("last entry: {other:?}"),
    }
}

/// Operations that emit fixed output.
struct CustomOps;

impl BashOperations for CustomOps {
    fn exec(
        &self,
        _command: &str,
        _cwd: &Path,
        options: BashExecOptions<'_>,
    ) -> Result<Option<i32>, ToolError> {
        (options.on_data)(b"hello from custom ops");
        Ok(Some(0))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn records_bash_output_through_custom_operations() {
    let h = Harness::new(HarnessOptions::default());
    let result = h
        .session
        .execute_bash("custom", None, false, Some(&CustomOps))
        .unwrap();
    assert!(result.output.contains("hello from custom ops"));
    assert_eq!(last_role(&h), "bashExecution");
}

#[tokio::test(flavor = "multi_thread")]
async fn applies_the_command_prefix_and_marks_excluded_runs() {
    let mut settings = serde_json::Map::new();
    settings.insert("shellCommandPrefix".into(), "X=prefixed".into());
    let h = Harness::new(HarnessOptions {
        settings,
        ..Default::default()
    });
    let mut chunks = String::new();
    let mut on_chunk = |c: &str| chunks.push_str(c);
    let result = h
        .session
        .execute_bash("echo $X", Some(&mut on_chunk), true, None)
        .unwrap();
    assert_eq!(result.output.trim(), "prefixed");
    assert!(chunks.contains("prefixed"));
    let Some(AgentMessage::BashExecution(message)) = h.session.messages().pop() else {
        panic!("no bash message");
    };
    // The recorded command is the user's, without the prefix.
    assert_eq!(message.command, "echo $X");
    assert_eq!(message.exclude_from_context, Some(true));
}

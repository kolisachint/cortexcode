//! Ports `test/suite/agent-session-queue.test.ts` (extension-origin messages
//! go through `send_user_message`, as `hoo.sendUserMessage` does).

use std::sync::{Arc, Mutex};

use crate::common::{message_text, Harness, HarnessOptions, WaitTool};
use cortexcode_agent_types::{AgentEvent, AgentMessage};
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_tool_call, FauxMessageOptions, FauxResponseStep,
};
use cortexcode_ai_types::{Message, StopReason, UserContent};
use cortexcode_code_agent_session::{
    AgentSession, AgentSessionEvent, DeliverAs, PromptOptions, StreamingBehavior,
};
use cortexcode_code_settings::QueueMode;

fn text(t: &str) -> FauxResponseStep {
    faux_assistant_message(t, Default::default()).into()
}

fn wait_call() -> FauxResponseStep {
    faux_assistant_message(
        faux_tool_call("wait", serde_json::json!({}), None),
        FauxMessageOptions {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        },
    )
    .into()
}

struct Waiting {
    h: Harness,
    wait: WaitTool,
    run: tokio::task::JoinHandle<Result<(), cortexcode_code_agent_session::AgentSessionError>>,
}

/// `createWaitingHarness`: a prompt blocked inside the `wait` tool.
async fn waiting(responses: Vec<FauxResponseStep>, prepare: impl FnOnce(&AgentSession)) -> Waiting {
    let wait = WaitTool::new();
    let h = Harness::new(HarnessOptions {
        tools: vec![wait.definition.clone()],
        ..Default::default()
    });
    prepare(&h.session);
    h.set_responses(responses);
    let (started, _sub) = crate::common::wait_for_tool_start(&h.session, "wait");
    let session = h.session.clone();
    let run = tokio::spawn(async move { session.prompt("start", PromptOptions::default()).await });
    started.await.unwrap();
    Waiting { h, wait, run }
}

impl Waiting {
    async fn finish(self) -> Harness {
        self.wait.release();
        self.run.await.unwrap().unwrap();
        self.h
    }
}

fn users_in_context(sink: Arc<Mutex<Vec<String>>>, reply: &'static str) -> FauxResponseStep {
    FauxResponseStep::factory(move |context, _, _, _| {
        *sink.lock().unwrap() = context
            .messages
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .map(message_text)
            .collect();
        Ok(faux_assistant_message(reply, Default::default()))
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_extension_origin_steering_messages_before_the_next_llm_call() {
    let w = waiting(
        vec![
            wait_call(),
            FauxResponseStep::factory(|context, _, _, _| {
                let saw = context
                    .messages
                    .iter()
                    .any(|m| matches!(m, Message::User(_)) && message_text(m) == "steer now");
                Ok(faux_assistant_message(
                    if saw { "saw steer" } else { "missing steer" },
                    Default::default(),
                ))
            }),
        ],
        |_| {},
    )
    .await;
    w.h.session
        .send_user_message(
            UserContent::Text("steer now".into()),
            Some(StreamingBehavior::Steer),
        )
        .await
        .unwrap();
    let h = w.finish().await;
    assert_eq!(h.user_texts(), ["start", "steer now"]);
    assert!(h.assistant_texts().contains(&"saw steer".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_follow_up_messages_only_after_the_current_run_finishes() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let w = waiting(
        vec![
            wait_call(),
            FauxResponseStep::factory(move |context, _, _, _| {
                sink.lock().unwrap().extend(
                    context
                        .messages
                        .iter()
                        .filter(|m| matches!(m, Message::Assistant(_)))
                        .map(message_text),
                );
                Ok(faux_assistant_message(
                    "follow-up response",
                    Default::default(),
                ))
            }),
        ],
        |_| {},
    )
    .await;
    w.h.session.follow_up("after current run", &[]).unwrap();
    let h = w.finish().await;
    assert_eq!(h.user_texts(), ["start", "after current run"]);
    assert!(seen.lock().unwrap().contains(&String::new()));
    assert!(h
        .assistant_texts()
        .contains(&"follow-up response".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_multiple_steering_messages_in_order_one_at_a_time() {
    let w = waiting(
        vec![
            wait_call(),
            text("handled steer 1"),
            text("handled steer 2"),
        ],
        |_| {},
    )
    .await;
    w.h.session.steer("steer 1", &[]).unwrap();
    w.h.session.steer("steer 2", &[]).unwrap();
    let h = w.finish().await;
    assert_eq!(h.user_texts(), ["start", "steer 1", "steer 2"]);
    assert_eq!(
        h.assistant_texts(),
        ["", "handled steer 1", "handled steer 2"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_multiple_follow_up_messages_in_order_one_at_a_time() {
    let w = waiting(
        vec![
            wait_call(),
            text("original turn complete"),
            text("handled follow-up 1"),
            text("handled follow-up 2"),
        ],
        |_| {},
    )
    .await;
    w.h.session.follow_up("follow-up 1", &[]).unwrap();
    w.h.session.follow_up("follow-up 2", &[]).unwrap();
    let h = w.finish().await;
    assert_eq!(h.user_texts(), ["start", "follow-up 1", "follow-up 2"]);
    assert_eq!(
        h.assistant_texts(),
        [
            "",
            "original turn complete",
            "handled follow-up 1",
            "handled follow-up 2"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_all_steering_messages_in_one_batch_in_all_mode() {
    let batched = Arc::new(Mutex::new(Vec::new()));
    let w = waiting(
        vec![
            wait_call(),
            users_in_context(batched.clone(), "batched steer response"),
        ],
        |s| s.set_steering_mode(QueueMode::All),
    )
    .await;
    w.h.session.steer("steer 1", &[]).unwrap();
    w.h.session.steer("steer 2", &[]).unwrap();
    let h = w.finish().await;
    assert_eq!(*batched.lock().unwrap(), ["start", "steer 1", "steer 2"]);
    assert_eq!(h.assistant_texts(), ["", "batched steer response"]);
    assert_eq!(h.session.steering_mode(), QueueMode::All);
    assert_eq!(h.session.settings().steering_mode(), QueueMode::All);
}

#[tokio::test(flavor = "multi_thread")]
async fn delivers_all_follow_up_messages_in_one_batch_in_all_mode() {
    let batched = Arc::new(Mutex::new(Vec::new()));
    let w = waiting(
        vec![
            wait_call(),
            text("original turn complete"),
            users_in_context(batched.clone(), "batched follow-up response"),
        ],
        |s| s.set_follow_up_mode(QueueMode::All),
    )
    .await;
    w.h.session.follow_up("follow-up 1", &[]).unwrap();
    w.h.session.follow_up("follow-up 2", &[]).unwrap();
    let h = w.finish().await;
    assert_eq!(
        *batched.lock().unwrap(),
        ["start", "follow-up 1", "follow-up 2"]
    );
    assert_eq!(
        h.assistant_texts(),
        ["", "original turn complete", "batched follow-up response"]
    );
}

fn saw_user_text(flag: Arc<Mutex<bool>>, needle: &'static str) -> FauxResponseStep {
    FauxResponseStep::factory(move |context, _, _, _| {
        *flag.lock().unwrap() = context
            .messages
            .iter()
            .any(|m| matches!(m, Message::User(_)) && message_text(m) == needle);
        Ok(faux_assistant_message("done", Default::default()))
    })
}

fn has_custom(h: &Harness, custom_type: &str) -> bool {
    h.session
        .messages()
        .iter()
        .any(|m| matches!(m, AgentMessage::Custom(c) if c.custom_type == custom_type))
}

#[tokio::test(flavor = "multi_thread")]
async fn queues_custom_messages_with_deliver_as_steer_while_streaming() {
    let saw = Arc::new(Mutex::new(false));
    let w = waiting(
        vec![wait_call(), saw_user_text(saw.clone(), "steer custom")],
        |_| {},
    )
    .await;
    w.h.session
        .send_custom_message(
            "queue-test",
            UserContent::Text("steer custom".into()),
            true,
            Some(serde_json::json!({"value": 1})),
            false,
            Some(DeliverAs::Steer),
        )
        .await
        .unwrap();
    let h = w.finish().await;
    assert!(*saw.lock().unwrap());
    assert!(has_custom(&h, "queue-test"));
}

#[tokio::test(flavor = "multi_thread")]
async fn queues_custom_messages_with_deliver_as_follow_up_while_streaming() {
    let saw = Arc::new(Mutex::new(false));
    let w = waiting(
        vec![
            wait_call(),
            text("original turn complete"),
            saw_user_text(saw.clone(), "follow-up custom"),
        ],
        |_| {},
    )
    .await;
    w.h.session
        .send_custom_message(
            "queue-test",
            UserContent::Text("follow-up custom".into()),
            true,
            Some(serde_json::json!({"value": 1})),
            false,
            Some(DeliverAs::FollowUp),
        )
        .await
        .unwrap();
    let h = w.finish().await;
    assert!(*saw.lock().unwrap());
    assert!(has_custom(&h, "queue-test"));
}

#[tokio::test(flavor = "multi_thread")]
async fn injects_next_turn_custom_messages_into_the_next_prompt() {
    let h = Harness::new(HarnessOptions::default());
    let saw = Arc::new(Mutex::new(false));
    h.session
        .send_custom_message(
            "next-turn",
            UserContent::Text("carry this".into()),
            true,
            Some(serde_json::json!({})),
            false,
            Some(DeliverAs::NextTurn),
        )
        .await
        .unwrap();
    h.set_responses(vec![saw_user_text(saw.clone(), "carry this")]);
    h.session
        .prompt("normal prompt", PromptOptions::default())
        .await
        .unwrap();
    assert!(*saw.lock().unwrap());
    assert_eq!(h.roles(), ["user", "custom", "assistant"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn updates_pending_count_and_removes_queued_text_before_message_start() {
    let counts = Arc::new(Mutex::new(Vec::new()));
    let w = waiting(vec![wait_call(), text("done")], |_| {}).await;
    let session = w.h.session.clone();
    let sink = counts.clone();
    let _sub = w.h.session.subscribe(move |event| {
        if let AgentSessionEvent::Agent(AgentEvent::MessageStart {
            message: AgentMessage::User(user),
        }) = event
        {
            if crate::common::text_of(&user.content.blocks()) == "queued" {
                sink.lock().unwrap().push(session.pending_message_count());
            }
        }
    });
    w.h.session.steer("queued", &[]).unwrap();
    assert_eq!(w.h.session.pending_message_count(), 1);
    let h = w.finish().await;
    assert_eq!(*counts.lock().unwrap(), [0]);
    assert_eq!(h.session.pending_message_count(), 0);
    // Queue updates went out as the message was queued and delivered.
    let updates: Vec<Vec<String>> = h
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AgentSessionEvent::QueueUpdate { steering, .. } => Some(steering.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(updates, [vec!["queued".to_string()], vec![]]);
}

#[tokio::test(flavor = "multi_thread")]
async fn throws_when_queueing_an_extension_command() {
    let h = Harness::new(HarnessOptions {
        extensions: Some(Arc::new(TestCommands)),
        ..Default::default()
    });
    let expected = "Extension command \"/testcmd\" cannot be queued. Use prompt() or execute the command when not streaming.";
    assert_eq!(
        h.session.steer("/testcmd queued", &[]).unwrap_err().0,
        expected
    );
    assert_eq!(
        h.session.follow_up("/testcmd queued", &[]).unwrap_err().0,
        expected
    );
}

struct TestCommands;

impl cortexcode_code_agent_session::ExtensionHooks for TestCommands {
    fn has_command(&self, name: &str) -> bool {
        name == "testcmd"
    }
}

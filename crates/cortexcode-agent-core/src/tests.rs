//! Port of hoocode `packages/agent/test/agent.test.ts` and
//! `prepare-next-turn-refresh.test.ts`, plus transcript/tool/abort checks.

use super::*;
use cortexcode_ai_provider_faux::{
    faux_assistant_message, faux_tool_call, FauxMessageOptions, FauxProvider,
};
use cortexcode_ai_stream::create_assistant_message_event_stream;
use cortexcode_ai_types::{AssistantMessageEvent, ToolCallContent};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;
use std::time::Duration;

fn model() -> Model {
    Model {
        id: "faux-model".into(),
        name: "Faux".into(),
        api: "faux".into(),
        provider: "faux".into(),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: Default::default(),
        context_window: 1000,
        max_tokens: 100,
        headers: None,
        compat: None,
    }
}

fn state_with(tools: Vec<AgentTool>) -> AgentState {
    AgentState {
        model: model(),
        tools: AgentTools::new(tools),
        ..default_state()
    }
}

fn assistant_text(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![Content::text(text)],
        api: "openai-responses".into(),
        provider: "openai".into(),
        model: "mock".into(),
        stop_reason: StopReason::Stop,
        timestamp: ai_types::now_ms(),
        ..Default::default()
    }
}

fn user_text(text: &str) -> AgentMessage {
    AgentMessage::user_text(text)
}

fn done_stream(message: AssistantMessage) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    stream.push(AssistantMessageEvent::Done { message });
    stream
}

/// A stream that sends `start`, then an aborted `error` once the request's
/// signal fires (the TypeScript `checkAbort` polling mock).
fn stream_until_aborted(options: &SimpleStreamOptions) -> AssistantMessageEventStream {
    let stream = create_assistant_message_event_stream();
    stream.push(AssistantMessageEvent::Start {
        partial: assistant_text(""),
    });
    let signal = options.signal.clone().expect("the agent passes its signal");
    let producer = stream.clone();
    tokio::spawn(async move {
        signal.cancelled().await;
        let mut aborted = assistant_text("Aborted");
        aborted.stop_reason = StopReason::Aborted;
        producer.push(AssistantMessageEvent::Error { error: aborted });
    });
    stream
}

fn stream_fn(
    f: impl Fn(
            Model,
            ai_types::Context,
            SimpleStreamOptions,
        ) -> Result<AssistantMessageEventStream, BoxError>
        + Send
        + Sync
        + 'static,
) -> Option<SharedStreamFn> {
    Some(Arc::new(Box::new(f)))
}

fn roles(messages: &[AgentMessage]) -> Vec<&'static str> {
    messages.iter().map(|m| m.role()).collect()
}

fn event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::AgentStart => "agent_start",
        AgentEvent::TurnStart => "turn_start",
        AgentEvent::MessageStart { .. } => "message_start",
        AgentEvent::MessageUpdate { .. } => "message_update",
        AgentEvent::MessageEnd { .. } => "message_end",
        AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
        AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
        AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        AgentEvent::TurnEnd { .. } => "turn_end",
        AgentEvent::AgentEnd { .. } => "agent_end",
    }
}

// ---------------------------------------------------------------------------
// agent.test.ts
// ---------------------------------------------------------------------------

#[test]
fn creates_an_agent_with_default_state() {
    let state = Agent::new().state();
    assert_eq!(state.system_prompt, "");
    assert_eq!(state.model.id, "unknown");
    assert_eq!(state.thinking_level, ThinkingLevel::Off);
    assert!(state.tools.is_empty());
    assert!(state.messages.is_empty());
    assert!(!state.is_streaming);
    assert!(state.streaming_message.is_none());
    assert!(state.pending_tool_calls.is_empty());
    assert!(state.error_message.is_none());
}

#[test]
fn creates_an_agent_with_custom_initial_state() {
    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "You are a helpful assistant.".into(),
            thinking_level: ThinkingLevel::Low,
            ..state_with(vec![])
        }),
        ..Default::default()
    });
    let state = agent.state();
    assert_eq!(state.system_prompt, "You are a helpful assistant.");
    assert_eq!(state.model.id, "faux-model");
    assert_eq!(state.thinking_level, ThinkingLevel::Low);
}

#[test]
fn subscribes_and_state_mutators_do_not_emit() {
    let agent = Agent::new();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let subscription = agent.subscribe(move |_, _| {
        c.fetch_add(1, Ordering::SeqCst);
    });
    assert_eq!(count.load(Ordering::SeqCst), 0);
    agent.set_system_prompt("Test prompt");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(agent.state().system_prompt, "Test prompt");
    subscription.unsubscribe();
    agent.set_system_prompt("Another prompt");
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn emits_full_lifecycle_events_for_thrown_run_failures() {
    let agent = Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, _| Err("provider exploded".into())),
        ..Default::default()
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let e = events.clone();
    let _sub = agent.subscribe(move |event, _| e.lock().unwrap().push(event_type(event)));

    agent.prompt("hello").await.unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );
    let state = agent.state();
    match state.messages.last() {
        Some(AgentMessage::Assistant(a)) => {
            assert_eq!(a.stop_reason, StopReason::Error);
            assert_eq!(a.error_message.as_deref(), Some("provider exploded"));
        }
        other => panic!("expected the failure message, got {other:?}"),
    }
    assert_eq!(state.error_message.as_deref(), Some("provider exploded"));
}

/// A listener blocked on `agent_end` holds the run open (the TypeScript test
/// awaits an async subscriber).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listeners_finish_before_prompt_resolves() {
    let agent = Arc::new(Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, _| Ok(done_stream(assistant_text("ok")))),
        ..Default::default()
    }));
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let listener_finished = Arc::new(AtomicBool::new(false));
    let lf = listener_finished.clone();
    let _sub = agent.subscribe(move |event, _| {
        if let AgentEvent::AgentEnd { .. } = event {
            let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
            lf.store(true, Ordering::SeqCst);
        }
    });

    let running = agent.clone();
    let prompt = tokio::spawn(async move { running.prompt("hello").await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!prompt.is_finished());
    assert!(!listener_finished.load(Ordering::SeqCst));
    assert!(agent.state().is_streaming);

    release.send(()).unwrap();
    prompt.await.unwrap().unwrap();
    assert!(listener_finished.load(Ordering::SeqCst));
    assert!(!agent.state().is_streaming);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_idle_waits_for_listeners() {
    let agent = Arc::new(Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, _| Ok(done_stream(assistant_text("ok")))),
        ..Default::default()
    }));
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let _sub = agent.subscribe(move |event, _| {
        if let AgentEvent::MessageEnd {
            message: AgentMessage::Assistant(_),
        } = event
        {
            let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
        }
    });

    let running = agent.clone();
    let prompt = tokio::spawn(async move { running.prompt("hello").await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let waiting = agent.clone();
    let idle = tokio::spawn(async move { waiting.wait_for_idle().await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!idle.is_finished());
    assert!(agent.state().is_streaming);

    release.send(()).unwrap();
    prompt.await.unwrap().unwrap();
    idle.await.unwrap();
    assert!(!agent.state().is_streaming);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passes_the_active_abort_signal_to_subscribers() {
    let agent = Arc::new(Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, options| Ok(stream_until_aborted(&options))),
        ..Default::default()
    }));
    let received: Arc<OnceLock<AbortSignal>> = Arc::new(OnceLock::new());
    let r = received.clone();
    let _sub = agent.subscribe(move |event, signal| {
        if let AgentEvent::AgentStart = event {
            let _ = r.set(signal.clone());
        }
    });

    let running = agent.clone();
    let prompt = tokio::spawn(async move { running.prompt("hello").await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let signal = received
        .get()
        .expect("agent_start carried a signal")
        .clone();
    assert!(!signal.aborted());

    agent.abort();
    prompt.await.unwrap().unwrap();
    assert!(signal.aborted());
}

#[test]
fn updates_state_with_mutators() {
    let agent = Agent::new();
    agent.set_system_prompt("Custom prompt");
    assert_eq!(agent.state().system_prompt, "Custom prompt");

    let mut new_model = model();
    new_model.id = "gemini-2.5-flash".into();
    agent.set_model(new_model);
    assert_eq!(agent.state().model.id, "gemini-2.5-flash");

    agent.set_thinking_level(ThinkingLevel::High);
    assert_eq!(agent.state().thinking_level, ThinkingLevel::High);

    let tool = AgentTool::new(
        "test",
        "test tool",
        serde_json::json!({}),
        Box::new(|_, _, _, _| Err("unused".into())),
    );
    agent.set_tools(vec![tool]);
    assert_eq!(agent.state().tools.len(), 1);

    agent.set_messages(vec![user_text("Hello")]);
    assert_eq!(roles(&agent.state().messages), ["user"]);
    agent.append_message(AgentMessage::Assistant(assistant_text("Hi")));
    assert_eq!(agent.state().messages.len(), 2);
    agent.set_messages(vec![]);
    assert!(agent.state().messages.is_empty());
}

#[test]
fn steering_and_follow_up_messages_are_queued_not_added() {
    let agent = Agent::new();
    agent.steer(user_text("Steering message"));
    agent.follow_up(user_text("Follow-up message"));
    assert!(agent.state().messages.is_empty());
    assert!(agent.has_queued_messages());
    agent.clear_all_queues();
    assert!(!agent.has_queued_messages());
}

#[test]
fn abort_without_a_run_is_a_no_op() {
    Agent::new().abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_while_streaming_is_rejected() {
    let agent = Arc::new(Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, options| Ok(stream_until_aborted(&options))),
        ..Default::default()
    }));
    let running = agent.clone();
    let first = tokio::spawn(async move { running.prompt("First message").await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(agent.state().is_streaming);

    assert_eq!(
        agent.prompt("Second message").await.unwrap_err().to_string(),
        "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion."
    );
    agent.abort();
    first.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continue_while_streaming_is_rejected() {
    let agent = Arc::new(Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, options| Ok(stream_until_aborted(&options))),
        ..Default::default()
    }));
    let running = agent.clone();
    let first = tokio::spawn(async move { running.prompt("First message").await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(agent.state().is_streaming);

    assert_eq!(
        agent.r#continue().await.unwrap_err().to_string(),
        "Agent is already processing. Wait for completion before continuing."
    );
    agent.abort();
    first.await.unwrap().unwrap();
}

#[tokio::test]
async fn continue_processes_queued_follow_ups_after_an_assistant_turn() {
    let agent = Agent::with_options(AgentOptions {
        stream_fn: stream_fn(|_, _, _| Ok(done_stream(assistant_text("Processed")))),
        ..Default::default()
    });
    agent.set_messages(vec![
        user_text("Initial"),
        AgentMessage::Assistant(assistant_text("Initial response")),
    ]);
    agent.follow_up(user_text("Queued follow-up"));

    agent.r#continue().await.unwrap();

    let messages = agent.state().messages;
    assert!(messages.iter().any(|m| matches!(
        m,
        AgentMessage::User(u) if u.content == vec![Content::text("Queued follow-up")].into()
    )));
    assert_eq!(messages.last().unwrap().role(), "assistant");
}

#[tokio::test]
async fn continue_keeps_one_at_a_time_steering_from_an_assistant_tail() {
    let responses = Arc::new(AtomicUsize::new(0));
    let r = responses.clone();
    let agent = Agent::with_options(AgentOptions {
        stream_fn: stream_fn(move |_, _, _| {
            let n = r.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(done_stream(assistant_text(&format!("Processed {n}"))))
        }),
        ..Default::default()
    });
    agent.set_messages(vec![
        user_text("Initial"),
        AgentMessage::Assistant(assistant_text("Initial response")),
    ]);
    agent.steer(user_text("Steering 1"));
    agent.steer(user_text("Steering 2"));

    agent.r#continue().await.unwrap();

    let messages = agent.state().messages;
    assert_eq!(
        roles(&messages[messages.len() - 4..]),
        ["user", "assistant", "user", "assistant"]
    );
    assert_eq!(responses.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn continue_errors_match_hoocode() {
    let agent = Agent::new();
    assert_eq!(
        agent.r#continue().await.unwrap_err().to_string(),
        "No messages to continue from"
    );
    agent.set_messages(vec![AgentMessage::Assistant(assistant_text("x"))]);
    assert_eq!(
        agent.r#continue().await.unwrap_err().to_string(),
        "Cannot continue from message role: assistant"
    );
    assert!(!agent.state().is_streaming);
}

#[tokio::test]
async fn forwards_session_id_to_stream_options() {
    let received = Arc::new(Mutex::new(None::<String>));
    let r = received.clone();
    let agent = Agent::with_options(AgentOptions {
        session_id: Some("session-abc".into()),
        stream_fn: stream_fn(move |_, _, options| {
            *r.lock().unwrap() = options.session_id.clone();
            Ok(done_stream(assistant_text("ok")))
        }),
        ..Default::default()
    });
    agent.prompt("hello").await.unwrap();
    assert_eq!(received.lock().unwrap().as_deref(), Some("session-abc"));

    agent.set_session_id(Some("session-def".into()));
    assert_eq!(agent.session_id().as_deref(), Some("session-def"));
    agent.prompt("hello again").await.unwrap();
    assert_eq!(received.lock().unwrap().as_deref(), Some("session-def"));
}

// ---------------------------------------------------------------------------
// prepare-next-turn-refresh.test.ts
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_next_turn_delivers_a_mid_run_context_change_in_the_same_run() {
    let seen = Arc::new(Mutex::new(Vec::<(String, Vec<String>)>::new()));
    let s = seen.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let stream = stream_fn(move |_, context, _| {
        s.lock().unwrap().push((
            context.system_prompt.clone(),
            context.tools.iter().map(|t| t.name.clone()).collect(),
        ));
        let message = if c.fetch_add(1, Ordering::SeqCst) == 0 {
            AssistantMessage {
                content: vec![Content::ToolCall(ToolCallContent {
                    id: "tool-1".into(),
                    name: "install".into(),
                    arguments: serde_json::json!({}),
                    thought_signature: None,
                })],
                stop_reason: StopReason::ToolUse,
                ..assistant_text("")
            }
        } else {
            assistant_text("done")
        };
        Ok(done_stream(message))
    });

    let agent_cell: Arc<OnceLock<std::sync::Weak<Agent>>> = Arc::new(OnceLock::new());
    let context_dirty = Arc::new(AtomicBool::new(false));
    let (cell, dirty) = (agent_cell.clone(), context_dirty.clone());
    let install = AgentTool::new(
        "install",
        "Simulates InstallPlugin: mutates state mid-run",
        serde_json::json!({"type": "object"}),
        Box::new(move |_, _, _, _| {
            let agent = cell.get().unwrap().upgrade().unwrap();
            agent.set_system_prompt("UPDATED: new skill available");
            let mut tools: Vec<AgentTool> = agent.state().tools.iter().cloned().collect();
            tools.push(AgentTool::new(
                "installed-capability",
                "Appears mid-run",
                serde_json::json!({"type": "object"}),
                Box::new(|_, _, _, _| {
                    Ok(AgentToolResult {
                        content: vec![Content::text("ok")],
                        details: serde_json::Value::Null,
                        terminate: false,
                    })
                }),
            ));
            agent.set_tools(tools);
            dirty.store(true, Ordering::SeqCst);
            Ok(AgentToolResult {
                content: vec![Content::text("installed")],
                details: serde_json::Value::Null,
                terminate: false,
            })
        }),
    );
    let agent = Arc::new(Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            system_prompt: "INITIAL".into(),
            ..state_with(vec![install])
        }),
        stream_fn: stream,
        ..Default::default()
    }));
    agent_cell.set(Arc::downgrade(&agent)).unwrap();

    // Late assignment, as AgentSession wires it.
    let weak = Arc::downgrade(&agent);
    agent.set_prepare_next_turn(Some(Arc::new(move |turn, _signal| {
        if !context_dirty.swap(false, Ordering::SeqCst) {
            return Ok(None);
        }
        let agent = weak.upgrade().unwrap();
        let state = agent.state();
        Ok(Some(AgentLoopTurnUpdate {
            context: Some(AgentContext::new_with_tools(
                state.system_prompt,
                turn.context.messages,
                state.tools,
            )),
            model: None,
            thinking_level: None,
        }))
    })));

    agent.prompt("install something and use it").await.unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, "INITIAL");
    assert_eq!(seen[1].0, "UPDATED: new skill available");
    assert_eq!(seen[0].1, ["install"]);
    assert_eq!(seen[1].1, ["install", "installed-capability"]);
}

// ---------------------------------------------------------------------------
// Transcript, tool results, abort
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prompt_appends_to_the_transcript() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        faux_assistant_message("one", Default::default()).into(),
        faux_assistant_message("two", Default::default()).into(),
    ]);
    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(state_with(vec![])),
        stream_fn: Some(Arc::new(faux.stream_fn())),
        ..Default::default()
    });
    agent.prompt("a").await.unwrap();
    agent.prompt("b").await.unwrap();
    assert_eq!(
        roles(&agent.state().messages),
        ["user", "assistant", "user", "assistant"]
    );
}

#[tokio::test]
async fn tool_results_match_hoocode_messages_and_events() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("fails", serde_json::json!({}), Some("c1".into())),
                faux_tool_call("detailed", serde_json::json!({}), Some("c2".into())),
                faux_tool_call("missing", serde_json::json!({}), Some("c3".into())),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into(),
        faux_assistant_message("done", Default::default()).into(),
    ]);
    let fails = AgentTool::new(
        "fails",
        "",
        serde_json::json!({"type": "object"}),
        Box::new(|_, _, _, _| Err("ENOENT: no such file or directory, access '/x'".into())),
    );
    let detailed = AgentTool::new(
        "detailed",
        "",
        serde_json::json!({"type": "object"}),
        Box::new(|_, _, _, _| {
            Ok(AgentToolResult {
                content: vec![Content::text("ok")],
                details: serde_json::json!({"k": 1}),
                terminate: false,
            })
        }),
    );
    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(state_with(vec![fails, detailed])),
        stream_fn: Some(Arc::new(faux.stream_fn())),
        ..Default::default()
    });
    let ended = Arc::new(Mutex::new(Vec::new()));
    let sink = ended.clone();
    let _sub = agent.subscribe(move |event, _| {
        if let AgentEvent::MessageEnd {
            message: AgentMessage::ToolResult(m),
        } = event
        {
            sink.lock().unwrap().push(m.tool_call_id.clone());
        }
    });
    agent.prompt("go").await.unwrap();

    let results: Vec<_> = agent
        .state()
        .messages
        .into_iter()
        .filter_map(|m| match m {
            AgentMessage::ToolResult(r) => Some(r),
            _ => None,
        })
        .collect();
    let text = |r: &ai_types::ToolResultMessage| match &r.content[0] {
        Content::Text(t) => t.text.clone(),
        other => panic!("{other:?}"),
    };
    // A thrown error is its message, unprefixed, with empty details.
    assert_eq!(
        text(&results[0]),
        "ENOENT: no such file or directory, access '/x'"
    );
    assert!(results[0].is_error);
    assert_eq!(results[0].details, Some(serde_json::json!({})));
    // A tool's details reach the message.
    assert_eq!(results[1].details, Some(serde_json::json!({"k": 1})));
    assert!(!results[1].is_error);
    // An unknown tool is an error result.
    assert_eq!(text(&results[2]), "Tool missing not found");
    assert!(results[2].is_error);
    // Each tool result is announced with message_start/message_end.
    assert_eq!(*ended.lock().unwrap(), ["c1", "c2", "c3"]);
}

/// `agent.abort()` mid-stream: the signal reaches a real provider, which ends
/// the assistant message with `stopReason: "aborted"` and what streamed so far.
#[tokio::test(flavor = "multi_thread")]
async fn abort_reaches_the_provider_stream() {
    let head = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial \"}}]}\n\n";
    let base_url =
        cortexcode_ai_stream::testing::serve_sse_then_hang(head, Duration::from_secs(30));
    let mut m = model();
    m.api = "openai-completions".into();
    m.provider = "openai".into();
    m.base_url = base_url;
    let agent = Arc::new(Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            model: m,
            ..state_with(vec![])
        }),
        api_key: Some("sk-test".into()),
        stream_fn: Some(Arc::new(Box::new(cortexcode_ai_provider_openai::stream))),
        ..Default::default()
    }));
    let weak = Arc::downgrade(&agent);
    let _sub = agent.subscribe(move |event, _| {
        if let AgentEvent::MessageUpdate { .. } = event {
            if let Some(agent) = weak.upgrade() {
                agent.abort();
            }
        }
    });

    tokio::time::timeout(Duration::from_secs(10), agent.prompt("go"))
        .await
        .expect("abort must end the run without waiting for the server")
        .unwrap();

    match agent.state().messages.last() {
        Some(AgentMessage::Assistant(a)) => {
            assert_eq!(a.stop_reason, StopReason::Aborted);
            assert_eq!(a.content, vec![Content::text("partial ")]);
        }
        other => panic!("expected the aborted assistant message, got {other:?}"),
    }
    agent.abort();
    assert!(!agent.state().is_streaming);
}

// ---------------------------------------------------------------------------
// e2e.test.ts (packages/agent): the Agent against the faux provider. Its two
// `continue()` validation cases are `continue_errors_match_hoocode` above.
// ---------------------------------------------------------------------------

/// `test/utils/calculate.ts`: evaluates `a <op> b`.
fn calculate_tool() -> AgentTool {
    AgentTool::new(
        "calculate",
        "Evaluate mathematical expressions",
        serde_json::json!({"type": "object", "properties": {"expression": {"type": "string"}}, "required": ["expression"]}),
        Box::new(|_, args, _, _| {
            let expr = args["expression"].as_str().unwrap_or_default().to_string();
            let parts: Vec<&str> = expr.split_whitespace().collect();
            let (a, op, b) = (parts[0].parse::<i64>()?, parts[1], parts[2].parse::<i64>()?);
            let value = match op {
                "*" => a * b,
                "+" => a + b,
                "-" => a - b,
                _ => return Err(format!("unsupported operator {op}").into()),
            };
            Ok(AgentToolResult {
                content: vec![Content::text(format!("{expr} = {value}"))],
                details: serde_json::Value::Null,
                terminate: false,
            })
        }),
    )
}

fn faux_agent(faux: &Arc<FauxProvider>, tools: Vec<AgentTool>) -> Agent {
    Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            model: faux.get_model(),
            thinking_level: ThinkingLevel::Off,
            ..state_with(tools)
        }),
        stream_fn: Some(Arc::new(faux.stream_fn())),
        ..Default::default()
    })
}

fn text_of(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::Assistant(a) => &a.content,
        AgentMessage::ToolResult(r) => &r.content,
        other => panic!("no text blocks in {other:?}"),
    };
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn e2e_handles_a_basic_text_prompt() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![faux_assistant_message("4", Default::default()).into()]);
    let agent = faux_agent(&faux, vec![]);
    agent
        .prompt("What is 2+2? Answer with just the number.")
        .await
        .unwrap();
    let state = agent.state();
    assert!(!state.is_streaming);
    assert_eq!(roles(&state.messages), ["user", "assistant"]);
    assert!(text_of(&state.messages[1]).contains('4'));
}

#[tokio::test]
async fn e2e_executes_tools_and_tracks_pending_tool_calls() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        faux_assistant_message(
            vec![
                cortexcode_ai_provider_faux::faux_text("Let me calculate that."),
                faux_tool_call(
                    "calculate",
                    serde_json::json!({"expression": "123 * 456"}),
                    Some("calc-1".into()),
                ),
            ],
            FauxMessageOptions {
                stop_reason: Some(StopReason::ToolUse),
                ..Default::default()
            },
        )
        .into(),
        faux_assistant_message("The result is 56088.", Default::default()).into(),
    ]);
    let agent = Arc::new(faux_agent(&faux, vec![calculate_tool()]));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, weak) = (seen.clone(), Arc::downgrade(&agent));
    let _sub = agent.subscribe(move |event, _| {
        let kind = event_type(event);
        if kind == "tool_execution_start" || kind == "tool_execution_end" {
            let mut ids: Vec<String> = weak
                .upgrade()
                .map(|a| a.state().pending_tool_calls.into_iter().collect())
                .unwrap_or_default();
            ids.sort();
            sink.lock().unwrap().push((kind, ids));
        }
    });
    agent
        .prompt("Calculate 123 * 456 using the calculator tool.")
        .await
        .unwrap();
    let state = agent.state();
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 4);
    let result = state
        .messages
        .iter()
        .find(|m| m.role() == "toolResult")
        .expect("a tool result");
    assert!(text_of(result).contains("123 * 456 = 56088"));
    assert!(text_of(state.messages.last().unwrap()).contains("56088"));
    assert!(state.pending_tool_calls.is_empty());
    assert_eq!(
        *seen.lock().unwrap(),
        [
            ("tool_execution_start", vec!["calc-1".to_string()]),
            ("tool_execution_end", vec![]),
        ]
    );
}

#[tokio::test]
async fn e2e_handles_abort_during_streaming() {
    let faux =
        FauxProvider::with_options(cortexcode_ai_provider_faux::RegisterFauxProviderOptions {
            tokens_per_second: Some(20.0),
            token_size: Some(cortexcode_ai_provider_faux::FauxTokenSize {
                min: Some(2),
                max: Some(2),
            }),
            ..Default::default()
        });
    faux.set_responses(vec![faux_assistant_message(
        "one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen",
        Default::default(),
    )
    .into()]);
    let agent = Arc::new(faux_agent(&faux, vec![]));
    let aborter = agent.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        aborter.abort();
    });
    agent.prompt("Count slowly from 1 to 20.").await.unwrap();
    let state = agent.state();
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 2);
    match state.messages.last() {
        Some(AgentMessage::Assistant(a)) => {
            assert_eq!(a.stop_reason, StopReason::Aborted);
            assert!(a.error_message.is_some());
            assert_eq!(state.error_message, a.error_message);
        }
        other => panic!("expected an assistant message, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_emits_lifecycle_updates_while_streaming() {
    let faux =
        FauxProvider::with_options(cortexcode_ai_provider_faux::RegisterFauxProviderOptions {
            token_size: Some(cortexcode_ai_provider_faux::FauxTokenSize {
                min: Some(1),
                max: Some(1),
            }),
            ..Default::default()
        });
    faux.set_responses(vec![faux_assistant_message(
        "1 2 3 4 5",
        Default::default(),
    )
    .into()]);
    let agent = faux_agent(&faux, vec![]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let _sub = agent.subscribe(move |event, _| sink.lock().unwrap().push(event_type(event)));
    agent.prompt("Count from 1 to 5.").await.unwrap();
    let events = events.lock().unwrap().clone();
    for kind in [
        "agent_start",
        "turn_start",
        "message_start",
        "message_update",
        "message_end",
        "turn_end",
        "agent_end",
    ] {
        assert!(events.contains(&kind), "{kind} in {events:?}");
    }
    let first = |k| events.iter().position(|e| *e == k).unwrap();
    let last = |k| events.iter().rposition(|e| *e == k).unwrap();
    assert!(first("agent_start") < first("message_start"));
    assert!(first("message_start") < first("message_end"));
    assert!(first("message_end") < last("agent_end"));
    let state = agent.state();
    assert!(!state.is_streaming);
    assert_eq!(state.messages.len(), 2);
}

#[tokio::test]
async fn e2e_maintains_context_across_multiple_turns() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![
        faux_assistant_message("Nice to meet you, Alice.", Default::default()).into(),
        cortexcode_ai_provider_faux::FauxResponseStep::factory(|context, _, _, _| {
            let has_alice = context.messages.iter().any(|m| match m {
                ai_types::Message::User(u) => {
                    serde_json::to_string(&u.content).unwrap().contains("Alice")
                }
                _ => false,
            });
            Ok(faux_assistant_message(
                if has_alice {
                    "Your name is Alice."
                } else {
                    "I do not know your name."
                },
                Default::default(),
            ))
        }),
    ]);
    let agent = faux_agent(&faux, vec![]);
    agent.prompt("My name is Alice.").await.unwrap();
    assert_eq!(agent.state().messages.len(), 2);
    agent.prompt("What is my name?").await.unwrap();
    let state = agent.state();
    assert_eq!(state.messages.len(), 4);
    assert!(text_of(&state.messages[3]).to_lowercase().contains("alice"));
}

#[tokio::test]
async fn e2e_preserves_thinking_content_blocks() {
    let faux =
        FauxProvider::with_options(cortexcode_ai_provider_faux::RegisterFauxProviderOptions {
            models: vec![cortexcode_ai_provider_faux::FauxModelDefinition {
                reasoning: Some(true),
                ..cortexcode_ai_provider_faux::FauxModelDefinition::new("faux-reasoning")
            }],
            ..Default::default()
        });
    faux.set_responses(vec![faux_assistant_message(
        vec![
            cortexcode_ai_provider_faux::faux_thinking("step by step"),
            cortexcode_ai_provider_faux::faux_text("4"),
        ],
        Default::default(),
    )
    .into()]);
    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(AgentState {
            model: faux.get_model(),
            thinking_level: ThinkingLevel::Low,
            ..state_with(vec![])
        }),
        stream_fn: Some(Arc::new(faux.stream_fn())),
        ..Default::default()
    });
    agent.prompt("What is 2+2?").await.unwrap();
    match &agent.state().messages[1] {
        AgentMessage::Assistant(a) => assert_eq!(
            a.content,
            vec![
                Content::Thinking(ai_types::ThinkingContent {
                    thinking: "step by step".into(),
                    ..Default::default()
                }),
                Content::text("4"),
            ]
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn e2e_continue_gets_a_response_when_last_message_is_user() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![faux_assistant_message(
        "HELLO WORLD",
        Default::default(),
    )
    .into()]);
    let agent = faux_agent(&faux, vec![]);
    agent.set_messages(vec![user_text("Say exactly: HELLO WORLD")]);
    agent.r#continue().await.unwrap();
    let state = agent.state();
    assert!(!state.is_streaming);
    assert_eq!(roles(&state.messages), ["user", "assistant"]);
    assert!(text_of(&state.messages[1])
        .to_uppercase()
        .contains("HELLO WORLD"));
}

#[tokio::test]
async fn e2e_continue_processes_tool_results() {
    let faux = FauxProvider::new();
    faux.set_responses(vec![faux_assistant_message(
        "The answer is 8.",
        Default::default(),
    )
    .into()]);
    let agent = faux_agent(&faux, vec![calculate_tool()]);
    let model = faux.get_model();
    let assistant = AssistantMessage {
        content: vec![
            Content::text("Let me calculate that."),
            Content::ToolCall(ToolCallContent {
                id: "calc-1".into(),
                name: "calculate".into(),
                arguments: serde_json::json!({"expression": "5 + 3"}),
                ..Default::default()
            }),
        ],
        api: model.api.clone(),
        provider: model.provider.clone(),
        model: model.id.clone(),
        stop_reason: StopReason::ToolUse,
        timestamp: ai_types::now_ms(),
        ..Default::default()
    };
    let result = ai_types::ToolResultMessage {
        tool_call_id: "calc-1".into(),
        tool_name: "calculate".into(),
        content: vec![Content::text("5 + 3 = 8")],
        details: None,
        is_error: false,
        timestamp: ai_types::now_ms(),
    };
    agent.set_messages(vec![
        user_text("What is 5 + 3?"),
        AgentMessage::Assistant(assistant),
        AgentMessage::ToolResult(result),
    ]);
    agent.r#continue().await.unwrap();
    let state = agent.state();
    assert!(!state.is_streaming);
    assert!(state.messages.len() >= 4);
    let last = state.messages.last().unwrap();
    assert_eq!(last.role(), "assistant");
    assert!(text_of(last).contains('8'));
}

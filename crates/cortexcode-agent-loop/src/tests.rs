//! Port of hoocode `packages/agent/test/agent-loop.test.ts`.

use super::*;
use cortexcode_ai_stream::create_assistant_message_event_stream;
use cortexcode_ai_types::{Model, ModelCost, ToolCallContent, Usage};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn create_model() -> Model {
    Model {
        id: "mock".into(),
        name: "mock".into(),
        api: "openai-responses".into(),
        provider: "openai".into(),
        base_url: "https://example.invalid".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec!["text".into()],
        cost: ModelCost::default(),
        context_window: 8192,
        max_tokens: 2048,
        headers: None,
        compat: None,
    }
}

fn assistant(content: Vec<Content>, stop_reason: StopReason) -> AssistantMessage {
    AssistantMessage {
        content,
        api: "openai-responses".into(),
        provider: "openai".into(),
        model: "mock".into(),
        usage: Usage::default(),
        stop_reason,
        timestamp: ai_types::now_ms(),
        ..Default::default()
    }
}

fn text(t: &str) -> AssistantMessage {
    assistant(vec![Content::text(t)], StopReason::Stop)
}

fn tool_calls(calls: &[(&str, &str, serde_json::Value)]) -> AssistantMessage {
    assistant(
        calls
            .iter()
            .map(|(id, name, args)| {
                Content::ToolCall(ToolCallContent {
                    id: (*id).into(),
                    name: (*name).into(),
                    arguments: args.clone(),
                    thought_signature: None,
                })
            })
            .collect(),
        StopReason::ToolUse,
    )
}

fn user(t: &str) -> AgentMessage {
    AgentMessage::user_text(t)
}

/// `identityConverter`: keep user/assistant/toolResult.
fn identity_config() -> AgentLoopConfig {
    let mut config = AgentLoopConfig::new(create_model());
    config.convert_to_llm = Some(Box::new(default_convert_to_llm));
    config
}

/// A `MockAssistantStream` stream function: call `n` gets `respond(n, context)`
/// as its `done` message. Returns the call counter.
fn mock_stream(
    config: &mut AgentLoopConfig,
    respond: impl Fn(usize, &ai_types::Context) -> AssistantMessage + Send + Sync + 'static,
) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    config.stream_fn = Some(Box::new(move |_model, context, _options| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        let message = respond(n, &context);
        let stream = create_assistant_message_event_stream();
        stream.push(AssistantMessageEvent::Done { message });
        Ok(stream)
    }));
    calls
}

type ToolFn = dyn Fn(serde_json::Value) -> Result<AgentToolResult, BoxError> + Send + Sync;

fn tool(
    name: &str,
    run: impl Fn(serde_json::Value) -> Result<AgentToolResult, BoxError> + Send + Sync + 'static,
) -> AgentTool {
    let run: Arc<ToolFn> = Arc::new(run);
    AgentTool::new(
        name,
        format!("{name} tool"),
        serde_json::json!({"type": "object", "properties": {"value": {"type": "string"}}}),
        Box::new(move |_id, args, _signal, _update| run(args)),
    )
}

fn echo_result(prefix: &str, args: &serde_json::Value) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::text(format!("{prefix}{}", value_of(args)))],
        details: serde_json::json!({"value": args["value"]}),
        terminate: false,
    }
}

fn value_of(args: &serde_json::Value) -> String {
    match &args["value"] {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn context_with(tools: Vec<AgentTool>) -> AgentContext {
    AgentContext::new(String::new(), vec![], tools)
}

async fn collect(mut stream: AgentEventStream) -> (Vec<AgentEvent>, Vec<AgentMessage>) {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    let messages = stream.final_result().await.expect("the run succeeded");
    (events, messages)
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

fn roles(messages: &[AgentMessage]) -> Vec<&'static str> {
    messages.iter().map(|m| m.role()).collect()
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// agentLoop with AgentMessage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emits_events_with_agent_message_types() {
    let mut config = identity_config();
    mock_stream(&mut config, |_, _| text("Hi there!"));
    let context = AgentContext::new("You are helpful.".into(), vec![], vec![]);
    let (events, messages) = collect(agent_loop(vec![user("Hello")], context, config)).await;

    assert_eq!(roles(&messages), ["user", "assistant"]);
    let types: Vec<_> = events.iter().map(event_type).collect();
    for t in [
        "agent_start",
        "turn_start",
        "message_start",
        "message_end",
        "turn_end",
        "agent_end",
    ] {
        assert!(types.contains(&t), "{t}");
    }
}

#[tokio::test]
async fn handles_custom_message_types_via_convert_to_llm() {
    let converted = Arc::new(Mutex::new(Vec::<Message>::new()));
    let mut config = AgentLoopConfig::new(create_model());
    let sink = converted.clone();
    config.convert_to_llm = Some(Box::new(move |messages| {
        let out = default_convert_to_llm(messages)?;
        *sink.lock().unwrap() = out.clone();
        Ok(out)
    }));
    mock_stream(&mut config, |_, _| text("Response"));
    let notification = AgentMessage::Custom(CustomMessage {
        custom_type: "notification".into(),
        content: vec![Content::text("This is a notification")].into(),
        display: true,
        details: None,
        timestamp: 0,
    });
    let context = AgentContext::new("You are helpful.".into(), vec![notification], vec![]);
    collect(agent_loop(vec![user("Hello")], context, config)).await;

    let converted = converted.lock().unwrap();
    assert_eq!(converted.len(), 1);
    assert!(matches!(converted[0], Message::User(_)));
}

#[tokio::test]
async fn applies_transform_context_before_convert_to_llm() {
    let transformed = Arc::new(AtomicUsize::new(0));
    let converted = Arc::new(AtomicUsize::new(0));
    let mut config = AgentLoopConfig::new(create_model());
    let t = transformed.clone();
    config.transform_context = Some(Box::new(move |messages, _signal| {
        let kept = messages[messages.len() - 2..].to_vec();
        t.store(kept.len(), Ordering::SeqCst);
        Ok(kept)
    }));
    let c = converted.clone();
    config.convert_to_llm = Some(Box::new(move |messages| {
        let out = default_convert_to_llm(messages)?;
        c.store(out.len(), Ordering::SeqCst);
        Ok(out)
    }));
    mock_stream(&mut config, |_, _| text("Response"));
    let context = AgentContext::new(
        "You are helpful.".into(),
        vec![
            user("old message 1"),
            AgentMessage::Assistant(text("old response 1")),
            user("old message 2"),
            AgentMessage::Assistant(text("old response 2")),
        ],
        vec![],
    );
    collect(agent_loop(vec![user("new message")], context, config)).await;
    assert_eq!(transformed.load(Ordering::SeqCst), 2);
    assert_eq!(converted.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn handles_tool_calls_and_results() {
    let executed = Arc::new(Mutex::new(Vec::<String>::new()));
    let e = executed.clone();
    let echo = tool("echo", move |args| {
        e.lock().unwrap().push(value_of(&args));
        Ok(echo_result("echoed: ", &args))
    });
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))]),
        _ => text("done"),
    });
    let (events, _) = collect(agent_loop(
        vec![user("echo something")],
        context_with(vec![echo]),
        config,
    ))
    .await;

    assert_eq!(*executed.lock().unwrap(), ["hello"]);
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })));
    let end = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .expect("tool_execution_end");
    assert!(!end);
}

/// `validateToolArguments` in `prepareToolCall`: TypeBox-style conversion
/// before execution, and TypeBox's error text as the tool result on failure.
#[tokio::test]
async fn converts_arguments_and_reports_validation_errors() {
    let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let e = executed.clone();
    let mut count = tool("count", move |args| {
        e.lock().unwrap().push(args.clone());
        Ok(echo_result("ok: ", &args))
    });
    count.parameters = serde_json::json!({
        "type": "object",
        "properties": {"n": {"type": "integer", "minimum": 1}},
        "required": ["n"]
    });
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[
            ("tool-1", "count", serde_json::json!({"n": "3"})),
            ("tool-2", "count", serde_json::json!({"n": 0})),
        ]),
        _ => text("done"),
    });
    let (_, messages) = collect(agent_loop(
        vec![user("count")],
        context_with(vec![count]),
        config,
    ))
    .await;
    assert_eq!(*executed.lock().unwrap(), [serde_json::json!({"n": 3})]);
    let failed = messages
        .iter()
        .filter_map(|m| match m.extract_message() {
            Some(ai_types::Message::ToolResult(r)) if r.is_error => Some(text_of(&r.content)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        failed,
        ["Validation failed for tool \"count\":\n  - n: must be >= 1\n\nReceived arguments:\n{\n  \"n\": 0\n}"]
    );
}

#[tokio::test]
async fn executes_mutated_before_tool_call_args_without_revalidation() {
    let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let e = executed.clone();
    let echo = tool("echo", move |args| {
        e.lock().unwrap().push(args["value"].clone());
        Ok(echo_result("echoed: ", &args))
    });
    let mut config = identity_config();
    config.before_tool_call = Some(Box::new(|ctx, _signal| {
        ctx.args["value"] = serde_json::json!(123);
        Ok(None)
    }));
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))]),
        _ => text("done"),
    });
    collect(agent_loop(
        vec![user("echo something")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert_eq!(*executed.lock().unwrap(), [serde_json::json!(123)]);
}

#[tokio::test]
async fn prepares_tool_arguments_before_execution() {
    let executed = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let e = executed.clone();
    let mut edit = tool("edit", move |args| {
        e.lock().unwrap().push(args["edits"].clone());
        Ok(AgentToolResult {
            content: vec![Content::text("edited")],
            details: serde_json::Value::Null,
            terminate: false,
        })
    });
    edit.prepare_arguments = Some(Arc::new(|args| {
        let (Some(old), Some(new)) = (args["oldText"].as_str(), args["newText"].as_str()) else {
            return args;
        };
        let mut edits = args["edits"].as_array().cloned().unwrap_or_default();
        edits.push(serde_json::json!({"oldText": old, "newText": new}));
        serde_json::json!({ "edits": edits })
    }));
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[(
            "tool-1",
            "edit",
            serde_json::json!({"oldText": "before", "newText": "after"}),
        )]),
        _ => text("done"),
    });
    collect(agent_loop(
        vec![user("edit something")],
        context_with(vec![edit]),
        config,
    ))
    .await;
    assert_eq!(
        *executed.lock().unwrap(),
        [serde_json::json!([{"oldText": "before", "newText": "after"}])]
    );
}

/// A tool whose "first" call blocks until "second" runs (proving overlap) or a
/// timeout passes; returns (tool, parallel_observed).
/// `wait` bounds how long "first" waits: long for the parallel cases (the gate
/// opens as soon as "second" runs), short where the calls must be sequential.
fn gated_tool(name: &str, wait: Duration) -> (AgentTool, Arc<AtomicBool>) {
    let parallel = Arc::new(AtomicBool::new(false));
    let first_resolved = Arc::new(AtomicBool::new(false));
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let p = parallel.clone();
    let tool = tool(name, move |args| {
        let (lock, cvar) = &*gate;
        match value_of(&args).as_str() {
            "first" => {
                let guard = lock.lock().unwrap();
                let (guard, _) = cvar
                    .wait_timeout_while(guard, wait, |released| !*released)
                    .unwrap();
                drop(guard);
                // In TS the released promise resumes only after "second"
                // has returned; give "second" that head start here.
                std::thread::sleep(Duration::from_millis(50));
                first_resolved.store(true, Ordering::SeqCst);
            }
            "second" => {
                if !first_resolved.load(Ordering::SeqCst) {
                    p.store(true, Ordering::SeqCst);
                }
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }
            _ => {}
        }
        Ok(echo_result("echoed: ", &args))
    });
    (tool, parallel)
}

fn first_and_second(name: &'static str) -> impl Fn(usize, &ai_types::Context) -> AssistantMessage {
    move |n, _| match n {
        0 => tool_calls(&[
            ("tool-1", name, serde_json::json!({"value": "first"})),
            ("tool-2", name, serde_json::json!({"value": "second"})),
        ]),
        _ => text("done"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_tool_execution_end_in_completion_order_but_results_in_source_order() {
    let (echo, parallel) = gated_tool("echo", Duration::from_secs(10));
    let mut config = identity_config();
    config.tool_execution = ToolExecutionMode::Parallel;
    mock_stream(&mut config, first_and_second("echo"));
    let (events, _) = collect(agent_loop(
        vec![user("echo both")],
        context_with(vec![echo]),
        config,
    ))
    .await;

    let end_ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd {
                message: AgentMessage::ToolResult(r),
            } => Some(r.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    let turn_ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } => Some(tool_results),
            _ => None,
        })
        .flatten()
        .map(|r| r.tool_call_id.as_str())
        .collect();
    assert!(parallel.load(Ordering::SeqCst));
    assert_eq!(end_ids, ["tool-2", "tool-1"]);
    assert_eq!(result_ids, ["tool-1", "tool-2"]);
    assert_eq!(turn_ids, ["tool-1", "tool-2"]);
}

#[tokio::test]
async fn injects_queued_messages_after_all_tool_calls_complete() {
    let executed = Arc::new(Mutex::new(Vec::<String>::new()));
    let e = executed.clone();
    let echo = tool("echo", move |args| {
        e.lock().unwrap().push(value_of(&args));
        Ok(echo_result("ok:", &args))
    });
    let mut config = identity_config();
    config.tool_execution = ToolExecutionMode::Sequential;
    let delivered = Arc::new(AtomicBool::new(false));
    let seen = executed.clone();
    config.get_steering_messages = Some(Box::new(move || {
        if !seen.lock().unwrap().is_empty() && !delivered.swap(true, Ordering::SeqCst) {
            return Ok(vec![user("interrupt")]);
        }
        Ok(vec![])
    }));
    let saw_interrupt = Arc::new(AtomicBool::new(false));
    let saw = saw_interrupt.clone();
    mock_stream(&mut config, move |n, context| {
        if n == 1 {
            let found = context.messages.iter().any(
                |m| matches!(m, Message::User(u) if text_of(&u.content.blocks()) == "interrupt"),
            );
            saw.store(found, Ordering::SeqCst);
        }
        first_and_second("echo")(n, context)
    });
    let (events, _) = collect(agent_loop(
        vec![user("start")],
        context_with(vec![echo]),
        config,
    ))
    .await;

    assert_eq!(*executed.lock().unwrap(), ["first", "second"]);
    let ends: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionEnd { is_error, .. } => Some(*is_error),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [false, false]);
    let sequence: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageStart {
                message: AgentMessage::ToolResult(r),
            } => Some(format!("tool:{}", r.tool_call_id)),
            AgentEvent::MessageStart {
                message: AgentMessage::User(u),
            } => Some(text_of(&u.content.blocks())),
            _ => None,
        })
        .collect();
    let at = |s: &str| sequence.iter().position(|x| x == s).unwrap();
    assert!(at("tool:tool-1") < at("interrupt"));
    assert!(at("tool:tool-2") < at("interrupt"));
    assert!(saw_interrupt.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forces_sequential_execution_when_a_tool_is_sequential_under_parallel_config() {
    let (mut slow, parallel) = gated_tool("slow", Duration::from_millis(300));
    slow.execution_mode = Some(ToolExecutionMode::Sequential);
    let mut config = identity_config();
    mock_stream(&mut config, first_and_second("slow"));
    collect(agent_loop(
        vec![user("run both")],
        context_with(vec![slow]),
        config,
    ))
    .await;
    assert!(!parallel.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forces_sequential_execution_when_one_of_several_tools_is_sequential() {
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let o = order.clone();
    let mut slow = tool("slow", move |args| {
        o.lock().unwrap().push(format!("slow:{}", value_of(&args)));
        std::thread::sleep(Duration::from_millis(30));
        Ok(echo_result("slow: ", &args))
    });
    slow.execution_mode = Some(ToolExecutionMode::Sequential);
    let o = order.clone();
    let fast = tool("fast", move |args| {
        o.lock().unwrap().push(format!("fast:{}", value_of(&args)));
        Ok(echo_result("fast: ", &args))
    });
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[
            ("tool-1", "slow", serde_json::json!({"value": "a"})),
            ("tool-2", "fast", serde_json::json!({"value": "b"})),
        ]),
        _ => text("done"),
    });
    collect(agent_loop(
        vec![user("run both")],
        context_with(vec![slow, fast]),
        config,
    ))
    .await;
    let order = order.lock().unwrap();
    assert_eq!(order[0], "slow:a");
    assert!(order.contains(&"fast:b".to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allows_parallel_execution_when_all_tools_are_parallel() {
    let (mut echo, parallel) = gated_tool("echo", Duration::from_secs(10));
    echo.execution_mode = Some(ToolExecutionMode::Parallel);
    let mut config = identity_config();
    mock_stream(&mut config, first_and_second("echo"));
    collect(agent_loop(
        vec![user("echo both")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert!(parallel.load(Ordering::SeqCst));
}

#[tokio::test]
async fn uses_prepare_next_turn_snapshot_before_continuing() {
    let echo = tool("echo", |args| Ok(echo_result("echoed: ", &args)));
    let mut config = identity_config();
    let prepared = AtomicBool::new(false);
    config.prepare_next_turn = Some(Box::new(move |turn| {
        if prepared.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(AgentLoopTurnUpdate {
            context: Some(AgentContext::new_with_tools(
                "second prompt".into(),
                turn.context.messages.clone(),
                turn.context.tools.clone(),
            )),
            model: None,
            thinking_level: None,
        }))
    }));
    let second_prompt = Arc::new(Mutex::new(String::new()));
    let sp = second_prompt.clone();
    let calls = mock_stream(&mut config, move |n, context| {
        if n == 1 {
            *sp.lock().unwrap() = context.system_prompt.clone();
        }
        match n {
            0 => tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))]),
            _ => text("done"),
        }
    });
    let context = AgentContext::new("first prompt".into(), vec![], vec![echo]);
    collect(agent_loop(vec![user("echo something")], context, config)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(*second_prompt.lock().unwrap(), "second prompt");
}

#[tokio::test]
async fn prepare_next_turn_can_switch_model_and_thinking_level() {
    let echo = tool("echo", |args| Ok(echo_result("echoed: ", &args)));
    let mut config = identity_config();
    config.reasoning = Some(ThinkingLevel::High);
    config.prepare_next_turn = Some(Box::new(|_| {
        let mut model = create_model();
        model.id = "next".into();
        Ok(Some(AgentLoopTurnUpdate {
            context: None,
            model: Some(model),
            thinking_level: Some(ThinkingLevel::Off),
        }))
    }));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    config.stream_fn = Some(Box::new(move |model, _context, options| {
        s.lock()
            .unwrap()
            .push((model.id.clone(), options.reasoning.clone()));
        let message = match c.fetch_add(1, Ordering::SeqCst) {
            0 => tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "x"}))]),
            _ => text("done"),
        };
        let stream = create_assistant_message_event_stream();
        stream.push(AssistantMessageEvent::Done { message });
        Ok(stream)
    }));
    collect(agent_loop(
        vec![user("go")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert_eq!(
        *seen.lock().unwrap(),
        [
            ("mock".to_string(), Some(ThinkingLevel::High)),
            ("next".to_string(), None)
        ]
    );
}

#[tokio::test]
async fn stops_after_the_current_turn_when_should_stop_after_turn_returns_true() {
    let executed = Arc::new(Mutex::new(Vec::<String>::new()));
    let e = executed.clone();
    let echo = tool("echo", move |args| {
        e.lock().unwrap().push(value_of(&args));
        Ok(echo_result("echoed: ", &args))
    });
    let steering_polls = Arc::new(AtomicUsize::new(0));
    let follow_up_polls = Arc::new(AtomicUsize::new(0));
    let callback = Arc::new(Mutex::new((
        Vec::<String>::new(),
        Vec::<&'static str>::new(),
    )));
    let mut config = identity_config();
    let sp = steering_polls.clone();
    config.get_steering_messages = Some(Box::new(move || {
        sp.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }));
    let fp = follow_up_polls.clone();
    config.get_follow_up_messages = Some(Box::new(move || {
        fp.fetch_add(1, Ordering::SeqCst);
        Ok(vec![user("follow up should stay queued")])
    }));
    let cb = callback.clone();
    config.should_stop_after_turn = Some(Box::new(move |turn| {
        *cb.lock().unwrap() = (
            turn.tool_results
                .iter()
                .map(|r| r.tool_call_id.clone())
                .collect(),
            roles(&turn.context.messages),
        );
        Ok(true)
    }));
    let calls = mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))]),
        _ => text("should not run"),
    });
    let (events, messages) = collect(agent_loop(
        vec![user("echo something")],
        context_with(vec![echo]),
        config,
    ))
    .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(*executed.lock().unwrap(), ["hello"]);
    assert_eq!(steering_polls.load(Ordering::SeqCst), 1);
    assert_eq!(follow_up_polls.load(Ordering::SeqCst), 0);
    let (ids, context_roles) = callback.lock().unwrap().clone();
    assert_eq!(ids, ["tool-1"]);
    assert_eq!(context_roles, ["user", "assistant", "toolResult"]);
    assert_eq!(roles(&messages), ["user", "assistant", "toolResult"]);
    assert_eq!(
        events.iter().map(event_type).collect::<Vec<_>>(),
        [
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "tool_execution_start",
            "tool_execution_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );
}

#[tokio::test]
async fn stops_after_a_tool_batch_when_every_result_terminates() {
    let echo = tool("echo", |args| {
        Ok(AgentToolResult {
            terminate: true,
            ..echo_result("echoed: ", &args)
        })
    });
    let mut config = identity_config();
    let calls = mock_stream(&mut config, |_, _| {
        tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))])
    });
    let (events, messages) = collect(agent_loop(
        vec![user("echo something")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(roles(&messages), ["user", "assistant", "toolResult"]);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnEnd { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn continues_after_parallel_tool_calls_when_not_all_results_terminate() {
    let echo = tool("echo", |args| {
        Ok(AgentToolResult {
            terminate: value_of(&args) == "first",
            ..echo_result("echoed: ", &args)
        })
    });
    let mut config = identity_config();
    config.tool_execution = ToolExecutionMode::Parallel;
    let calls = mock_stream(&mut config, first_and_second("echo"));
    let (_, messages) = collect(agent_loop(
        vec![user("echo both")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        roles(&messages),
        ["user", "assistant", "toolResult", "toolResult", "assistant"]
    );
}

#[tokio::test]
async fn after_tool_call_can_mark_a_batch_as_terminating() {
    let echo = tool("echo", |args| Ok(echo_result("echoed: ", &args)));
    let mut config = identity_config();
    config.after_tool_call = Some(Box::new(|_, _| {
        Ok(Some(AfterToolCallResult {
            terminate: Some(true),
            ..Default::default()
        }))
    }));
    let calls = mock_stream(&mut config, |_, _| {
        tool_calls(&[("tool-1", "echo", serde_json::json!({"value": "hello"}))])
    });
    collect(agent_loop(
        vec![user("echo something")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tool_updates_are_emitted_between_start_and_end() {
    let progress = AgentTool::new(
        "progress",
        "",
        serde_json::json!({"type": "object"}),
        Box::new(|_id, _args, _signal, on_update| {
            if let Some(update) = on_update {
                update(AgentToolResult {
                    content: vec![Content::text("half")],
                    details: serde_json::Value::Null,
                    terminate: false,
                });
            }
            Ok(AgentToolResult {
                content: vec![Content::text("all")],
                details: serde_json::Value::Null,
                terminate: false,
            })
        }),
    );
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[("tool-1", "progress", serde_json::json!({}))]),
        _ => text("done"),
    });
    let (events, _) = collect(agent_loop(
        vec![user("go")],
        context_with(vec![progress]),
        config,
    ))
    .await;
    let tool_events: Vec<_> = events
        .iter()
        .map(event_type)
        .filter(|t| t.starts_with("tool_execution"))
        .collect();
    assert_eq!(
        tool_events,
        [
            "tool_execution_start",
            "tool_execution_update",
            "tool_execution_end"
        ]
    );
}

#[tokio::test]
async fn blocked_and_unknown_tools_become_error_results() {
    let echo = tool("echo", |args| Ok(echo_result("echoed: ", &args)));
    let mut config = identity_config();
    config.before_tool_call = Some(Box::new(|ctx, _| {
        Ok(
            (ctx.tool_call.id == "tool-1").then_some(BeforeToolCallResult {
                block: true,
                reason: None,
            }),
        )
    }));
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[
            ("tool-1", "echo", serde_json::json!({"value": "a"})),
            ("tool-2", "missing", serde_json::json!({})),
        ]),
        _ => text("done"),
    });
    let (_, messages) = collect(agent_loop(
        vec![user("go")],
        context_with(vec![echo]),
        config,
    ))
    .await;
    let results: Vec<(String, bool)> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::ToolResult(r) => Some((text_of(&r.content), r.is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results,
        [
            ("Tool execution was blocked".to_string(), true),
            ("Tool missing not found".to_string(), true)
        ]
    );
}

// ---------------------------------------------------------------------------
// agentLoopContinue with AgentMessage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continue_rejects_an_empty_context() {
    let err = agent_loop_continue(context_with(vec![]), identity_config())
        .err()
        .unwrap();
    assert_eq!(err.to_string(), "Cannot continue: no messages in context");
}

#[tokio::test]
async fn continue_rejects_an_assistant_tail() {
    let context = AgentContext::new(
        String::new(),
        vec![AgentMessage::Assistant(text("x"))],
        vec![],
    );
    let err = agent_loop_continue(context, identity_config())
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "Cannot continue from message role: assistant"
    );
}

#[tokio::test]
async fn continue_does_not_emit_user_message_events() {
    let mut config = identity_config();
    mock_stream(&mut config, |_, _| text("Response"));
    let context = AgentContext::new("You are helpful.".into(), vec![user("Hello")], vec![]);
    let (events, messages) = collect(agent_loop_continue(context, config).unwrap()).await;
    assert_eq!(roles(&messages), ["assistant"]);
    let ends: Vec<&'static str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd { message } => Some(message.role()),
            _ => None,
        })
        .collect();
    assert_eq!(ends, ["assistant"]);
}

#[tokio::test]
async fn continue_allows_custom_message_tail() {
    let mut config = AgentLoopConfig::new(create_model());
    config.convert_to_llm = Some(Box::new(|messages| {
        Ok(messages
            .into_iter()
            .filter_map(|m| match m {
                AgentMessage::Custom(c) => Some(Message::User(UserMessage {
                    content: c.content,
                    timestamp: c.timestamp,
                })),
                other => other.extract_message(),
            })
            .collect())
    }));
    mock_stream(&mut config, |_, _| text("Response to custom message"));
    let custom = AgentMessage::Custom(CustomMessage {
        custom_type: "hook".into(),
        content: vec![Content::text("Hook content")].into(),
        display: true,
        details: None,
        timestamp: 0,
    });
    let context = AgentContext::new("You are helpful.".into(), vec![custom], vec![]);
    let (_, messages) = collect(agent_loop_continue(context, config).unwrap()).await;
    assert_eq!(roles(&messages), ["assistant"]);
}

// ---------------------------------------------------------------------------
// Background tools
// ---------------------------------------------------------------------------

fn background_tool(
    name: &str,
    run: impl Fn(serde_json::Value) -> Result<AgentToolResult, BoxError> + Send + Sync + 'static,
) -> AgentTool {
    let mut t = tool(name, run);
    t.background = true;
    t
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_tools_do_not_block_and_deliver_a_follow_up_message() {
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let executed = Arc::new(AtomicBool::new(false));
    let (g, e) = (gate.clone(), executed.clone());
    let bg = background_tool("bg", move |args| {
        let (lock, cvar) = &*g;
        let guard = lock.lock().unwrap();
        let _ = cvar
            .wait_timeout_while(guard, Duration::from_secs(5), |open| !*open)
            .unwrap();
        e.store(true, Ordering::SeqCst);
        Ok(echo_result("bg-result: ", &args))
    });
    let mut config = identity_config();
    mock_stream(&mut config, move |n, _| match n {
        0 => tool_calls(&[("bg-1", "bg", serde_json::json!({"value": "hello"}))]),
        1 => {
            // Release the tool only after this non-blocking turn streamed.
            let gate = gate.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
            });
            text("still working")
        }
        _ => text("done"),
    });
    let (_, messages) = collect(agent_loop(vec![user("go")], context_with(vec![bg]), config)).await;

    assert!(executed.load(Ordering::SeqCst));
    let placeholder = messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::ToolResult(r) => Some(text_of(&r.content)),
            _ => None,
        })
        .unwrap();
    assert!(placeholder.contains("background"));
    let follow_up = messages
        .iter()
        .position(|m| matches!(m, AgentMessage::User(u) if text_of(&u.content.blocks()).contains("bg-result: hello")))
        .expect("the background result follow-up");
    let still_working = messages
        .iter()
        .position(
            |m| matches!(m, AgentMessage::Assistant(a) if text_of(&a.content) == "still working"),
        )
        .unwrap();
    assert!(follow_up > still_working);
}

#[tokio::test]
async fn foreground_and_background_results_keep_source_order() {
    let bg = background_tool("bg", |args| Ok(echo_result("bg:", &args)));
    let fg = tool("fg", |args| Ok(echo_result("fg:", &args)));
    let mut config = identity_config();
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[
            ("bg-1", "bg", serde_json::json!({"value": "B"})),
            ("fg-1", "fg", serde_json::json!({"value": "F"})),
        ]),
        _ => text("done"),
    });
    let (_, messages) = collect(agent_loop(
        vec![user("go")],
        context_with(vec![bg, fg]),
        config,
    ))
    .await;
    let results: Vec<_> = messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::ToolResult(r) => Some((r.tool_call_id.clone(), text_of(&r.content))),
            _ => None,
        })
        .collect();
    assert_eq!(results[0].0, "bg-1");
    assert!(results[0].1.contains("background"));
    assert_eq!(results[1], ("fg-1".to_string(), "fg:F".to_string()));
}

#[tokio::test]
async fn create_background_result_message_shapes_the_follow_up() {
    let bg = background_tool("bg", |args| Ok(echo_result("bg-result: ", &args)));
    let mut config = identity_config();
    config.create_background_result_message = Some(Box::new(|result| {
        AgentMessage::Custom(CustomMessage {
            custom_type: "backgroundTask".into(),
            content: result.result.content.into(),
            display: true,
            details: Some(serde_json::json!({"isError": result.is_error})),
            timestamp: ai_types::now_ms(),
        })
    }));
    mock_stream(&mut config, |n, _| match n {
        0 => tool_calls(&[("bg-1", "bg", serde_json::json!({"value": "hi"}))]),
        _ => text("done"),
    });
    let (_, messages) = collect(agent_loop(vec![user("go")], context_with(vec![bg]), config)).await;
    let custom = messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::Custom(c) => Some(c),
            _ => None,
        })
        .expect("the custom background message");
    assert_eq!(custom.custom_type, "backgroundTask");
    assert_eq!(text_of(&custom.content.blocks()), "bg-result: hi");
    assert!(!messages.iter().any(
        |m| matches!(m, AgentMessage::User(u) if text_of(&u.content.blocks()).contains("bg-result"))
    ));
}

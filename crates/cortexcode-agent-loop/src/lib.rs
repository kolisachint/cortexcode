//! Agent loop that works with `AgentMessage` throughout and converts to LLM
//! `Message`s only at the provider call.
//!
//! Port of hoocode `packages/agent/src/agent-loop.ts`. Differences forced by
//! Rust, all invisible in the event sequence:
//! - Hooks are synchronous closures; tools keep a synchronous `execute`, run
//!   on tokio's blocking pool, and stream `onUpdate` results through a channel.
//! - `beforeToolCall` gets `&mut` context and may rewrite `args` in place, as
//!   the TypeScript hook mutates its `args` object.
//! - A background tool's `afterToolCall` hook and result message run when the
//!   loop collects the result (the next turn boundary), not the instant the
//!   tool settles, because the detached task cannot borrow the config.
//! - Tool-argument validation (`validateToolArguments`) is ledger 8.5; until
//!   then prepared arguments pass through unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cortexcode_agent_types::*;
use cortexcode_ai_stream::EventStream;
use cortexcode_ai_types::{
    self as ai_types, AbortSignal, AssistantMessage, AssistantMessageEvent, Content, Message,
    SimpleStreamOptions, StopReason, ThinkingLevel, ToolResultMessage, UserMessage,
};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Callback type for emitting agent events.
pub type AgentEventSink = Box<dyn FnMut(AgentEvent) + Send>;

/// The stream returned by [`agent_loop`]: events, then the run's new messages.
pub type AgentEventStream = EventStream<AgentEvent, Vec<AgentMessage>>;

// ---------------------------------------------------------------------------
// Default convert_to_llm
// ---------------------------------------------------------------------------

/// `defaultConvertToLlm()` in agent.ts: keep user/assistant/toolResult only.
pub fn default_convert_to_llm(messages: Vec<AgentMessage>) -> Result<Vec<Message>, BoxError> {
    Ok(messages
        .into_iter()
        .filter_map(|msg| msg.extract_message())
        .collect())
}

// ---------------------------------------------------------------------------
// agentLoop / agentLoopContinue
// ---------------------------------------------------------------------------

fn create_agent_stream() -> AgentEventStream {
    EventStream::new(
        |event: &AgentEvent| matches!(event, AgentEvent::AgentEnd { .. }),
        |event: &AgentEvent| match event {
            AgentEvent::AgentEnd { messages } => messages.clone(),
            _ => Vec::new(),
        },
    )
}

/// `agentLoop()`: run with new prompts on the current tokio runtime and return
/// the event stream. Must be called inside a runtime.
pub fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
) -> AgentEventStream {
    let stream = create_agent_stream();
    let sink = stream.clone();
    let done = stream.clone();
    tokio::spawn(async move {
        let mut emit: AgentEventSink = Box::new(move |event| sink.push(event));
        let result = run_agent_loop(prompts, context, &config, &mut emit).await;
        // A failed run never resolves the TypeScript stream; end it so readers stop.
        done.end(result.ok());
    });
    stream
}

/// `agentLoopContinue()`: continue from `context` without a new message. The
/// last message must not be an assistant message.
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
) -> Result<AgentEventStream, BoxError> {
    check_continuable(&context)?;
    let stream = create_agent_stream();
    let sink = stream.clone();
    let done = stream.clone();
    tokio::spawn(async move {
        let mut context = context;
        let mut emit: AgentEventSink = Box::new(move |event| sink.push(event));
        let result = run_agent_loop_continue(&mut context, &config, &mut emit).await;
        done.end(result.ok());
    });
    Ok(stream)
}

fn check_continuable(context: &AgentContext) -> Result<(), BoxError> {
    match context.messages.last() {
        None => Err("Cannot continue: no messages in context".into()),
        Some(AgentMessage::Assistant(_)) => {
            Err("Cannot continue from message role: assistant".into())
        }
        Some(_) => Ok(()),
    }
}

/// `runAgentLoop()`: returns the run's new messages (the prompts first).
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    mut context: AgentContext,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<Vec<AgentMessage>, BoxError> {
    let mut new_messages = prompts.clone();
    context.messages.extend(prompts.iter().cloned());

    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    for prompt in &prompts {
        emit(AgentEvent::MessageStart {
            message: prompt.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: prompt.clone(),
        });
    }

    run_loop(&mut context, &mut new_messages, config, emit).await?;
    Ok(new_messages)
}

/// `runAgentLoopContinue()`: appends to `context.messages` in place (the
/// TypeScript context copy shares its message array with the caller).
pub async fn run_agent_loop_continue(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<Vec<AgentMessage>, BoxError> {
    check_continuable(context)?;
    let mut new_messages = Vec::new();
    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    run_loop(context, &mut new_messages, config, emit).await?;
    Ok(new_messages)
}

// ---------------------------------------------------------------------------
// runLoop
// ---------------------------------------------------------------------------

/// Model and reasoning for the next request (`prepareNextTurn` may change them).
struct TurnSettings {
    model: ai_types::Model,
    reasoning: Option<ThinkingLevel>,
}

async fn run_loop(
    context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<(), BoxError> {
    let mut settings = TurnSettings {
        model: config.model.clone(),
        reasoning: config.reasoning.clone(),
    };
    let mut first_turn = true;
    let background = BackgroundTaskManager::new(config.on_background_task_count_change.clone());

    // Steering typed while waiting for the run to start.
    let mut pending = collect_pending_messages(context, config, &background)?;

    loop {
        let mut has_more_tool_calls = true;

        while has_more_tool_calls || !pending.is_empty() || background.pending_count() > 0 {
            // Nothing to act on, but background work is in flight: wait for it.
            if !has_more_tool_calls && pending.is_empty() {
                background.wait_for_next().await;
                pending = collect_pending_messages(context, config, &background)?;
                if pending.is_empty() {
                    continue;
                }
            }

            if !first_turn {
                emit(AgentEvent::TurnStart);
            } else {
                first_turn = false;
            }

            for message in std::mem::take(&mut pending) {
                emit(AgentEvent::MessageStart {
                    message: message.clone(),
                });
                emit(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
                context.messages.push(message.clone());
                new_messages.push(message);
            }

            let message = stream_assistant_response(context, config, &settings, emit).await?;
            new_messages.push(AgentMessage::Assistant(message.clone()));

            if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                emit(AgentEvent::TurnEnd {
                    message,
                    tool_results: vec![],
                });
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                });
                return Ok(());
            }

            let has_tool_calls = message
                .content
                .iter()
                .any(|c| matches!(c, Content::ToolCall(_)));
            let mut tool_results = Vec::new();
            has_more_tool_calls = false;
            if has_tool_calls {
                let batch = execute_tool_calls(context, &message, config, emit, &background).await;
                has_more_tool_calls = !batch.terminate;
                for result in &batch.messages {
                    context
                        .messages
                        .push(AgentMessage::ToolResult(result.clone()));
                    new_messages.push(AgentMessage::ToolResult(result.clone()));
                }
                tool_results = batch.messages;
            }

            emit(AgentEvent::TurnEnd {
                message: message.clone(),
                tool_results: tool_results.clone(),
            });

            let turn = ShouldStopAfterTurnContext {
                message: message.clone(),
                tool_results: tool_results.clone(),
                context: context.clone(),
                new_messages: new_messages.clone(),
            };
            if let Some(prepare) = &config.prepare_next_turn {
                if let Some(update) = prepare(turn.clone())? {
                    if let Some(next) = update.context {
                        *context = next;
                    }
                    if let Some(model) = update.model {
                        settings.model = model;
                    }
                    match update.thinking_level {
                        None => {}
                        Some(ThinkingLevel::Off) => settings.reasoning = None,
                        Some(level) => settings.reasoning = Some(level),
                    }
                }
            }

            if let Some(should_stop) = &config.should_stop_after_turn {
                let turn = ShouldStopAfterTurnContext {
                    context: context.clone(),
                    ..turn
                };
                if should_stop(turn)? {
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    });
                    return Ok(());
                }
            }

            pending = collect_pending_messages(context, config, &background)?;
        }

        // The agent would stop here; queued follow-ups start another round.
        let follow_ups = match &config.get_follow_up_messages {
            Some(get) => get()?,
            None => Vec::new(),
        };
        if !follow_ups.is_empty() {
            pending = follow_ups;
            continue;
        }
        break;
    }

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });
    Ok(())
}

/// Finished background results first, then app-provided steering messages.
fn collect_pending_messages(
    context: &AgentContext,
    config: &AgentLoopConfig,
    background: &BackgroundTaskManager,
) -> Result<Vec<AgentMessage>, BoxError> {
    let mut messages: Vec<AgentMessage> = background
        .drain_results()
        .into_iter()
        .map(|settled| background_result_message(context, config, settled))
        .collect();
    if let Some(get) = &config.get_steering_messages {
        messages.extend(get()?);
    }
    Ok(messages)
}

// ---------------------------------------------------------------------------
// streamAssistantResponse
// ---------------------------------------------------------------------------

async fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    settings: &TurnSettings,
    emit: &mut AgentEventSink,
) -> Result<AssistantMessage, BoxError> {
    let signal = config.signal.clone();
    let messages = match &config.transform_context {
        Some(transform) => transform(context.messages.clone(), signal.clone())?,
        None => context.messages.clone(),
    };
    let llm_messages = match &config.convert_to_llm {
        Some(convert) => convert(messages)?,
        None => default_convert_to_llm(messages)?,
    };
    let llm_context = ai_types::Context::new(
        context.system_prompt.clone(),
        llm_messages,
        context
            .tools
            .iter()
            .map(|t| ai_types::Tool {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            })
            .collect(),
    );

    // Resolve the key per request (tokens can expire); empty falls back.
    let resolved_api_key = match &config.get_api_key {
        Some(get) => get(settings.model.provider.clone())?,
        None => None,
    }
    .filter(|k| !k.is_empty())
    .or_else(|| config.api_key.clone());

    let options = SimpleStreamOptions {
        signal,
        api_key: resolved_api_key,
        session_id: config.session_id.clone(),
        max_retry_delay_ms: config.max_retry_delay_ms,
        reasoning: settings.reasoning.clone(),
        thinking_budgets: config.thinking_budgets.clone(),
        thinking_display: config.thinking_display.clone(),
        transport: config.transport.clone(),
        cache_retention: config.cache_retention,
        send_session_affinity_headers: config.send_session_affinity_headers,
        prompt_suffix: config.prompt_suffix.clone(),
        ..Default::default()
    };

    let stream_fn = config
        .stream_fn
        .as_ref()
        .ok_or("No stream function configured")?;
    let mut response = stream_fn(settings.model.clone(), llm_context, options)?;

    let mut added_partial = false;
    while let Some(event) = response.next().await {
        match event {
            AssistantMessageEvent::Start { partial } => {
                context
                    .messages
                    .push(AgentMessage::Assistant(partial.clone()));
                added_partial = true;
                emit(AgentEvent::MessageStart {
                    message: AgentMessage::Assistant(partial),
                });
            }
            AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. } => {
                return Ok(finish_response(&response, context, added_partial, emit).await);
            }
            event => {
                if added_partial {
                    let partial = partial_of(&event).clone();
                    if let Some(last) = context.messages.last_mut() {
                        *last = AgentMessage::Assistant(partial.clone());
                    }
                    emit(AgentEvent::MessageUpdate {
                        assistant_message_event: Box::new(event),
                        message: AgentMessage::Assistant(partial),
                    });
                }
            }
        }
    }
    Ok(finish_response(&response, context, added_partial, emit).await)
}

fn partial_of(event: &AssistantMessageEvent) -> &AssistantMessage {
    match event {
        AssistantMessageEvent::Start { partial }
        | AssistantMessageEvent::TextStart { partial, .. }
        | AssistantMessageEvent::TextDelta { partial, .. }
        | AssistantMessageEvent::TextEnd { partial, .. }
        | AssistantMessageEvent::ThinkingStart { partial, .. }
        | AssistantMessageEvent::ThinkingDelta { partial, .. }
        | AssistantMessageEvent::ThinkingEnd { partial, .. }
        | AssistantMessageEvent::ToolCallStart { partial, .. }
        | AssistantMessageEvent::ToolCallDelta { partial, .. }
        | AssistantMessageEvent::ToolCallEnd { partial, .. } => partial,
        AssistantMessageEvent::Done { message } => message,
        AssistantMessageEvent::Error { error } => error,
    }
}

async fn finish_response(
    response: &cortexcode_ai_stream::AssistantMessageEventStream,
    context: &mut AgentContext,
    added_partial: bool,
    emit: &mut AgentEventSink,
) -> AssistantMessage {
    let final_message = response.result().await;
    if added_partial {
        if let Some(last) = context.messages.last_mut() {
            *last = AgentMessage::Assistant(final_message.clone());
        }
    } else {
        context
            .messages
            .push(AgentMessage::Assistant(final_message.clone()));
        emit(AgentEvent::MessageStart {
            message: AgentMessage::Assistant(final_message.clone()),
        });
    }
    emit(AgentEvent::MessageEnd {
        message: AgentMessage::Assistant(final_message.clone()),
    });
    final_message
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

struct ExecutedToolCallBatch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

/// A tool call ready to run, with its (possibly hook-rewritten) arguments.
struct PreparedToolCall {
    tool_call: AgentToolCall,
    tool: AgentTool,
    args: serde_json::Value,
}

enum Preparation {
    Prepared(Box<PreparedToolCall>),
    Immediate {
        result: AgentToolResult,
        is_error: bool,
    },
}

struct ExecutedOutcome {
    result: AgentToolResult,
    is_error: bool,
}

#[derive(Clone)]
struct FinalizedOutcome {
    tool_call: AgentToolCall,
    result: AgentToolResult,
    is_error: bool,
}

fn tool_calls_of(message: &AssistantMessage) -> Vec<AgentToolCall> {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::ToolCall(tc) => Some(AgentToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: tc.arguments.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// `executeToolCalls`: background calls are dispatched first (placeholder
/// results), foreground calls run sequentially or in parallel, and the result
/// messages come back in the assistant's tool-call order.
async fn execute_tool_calls(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
    background: &BackgroundTaskManager,
) -> ExecutedToolCallBatch {
    let tool_calls = tool_calls_of(assistant_message);
    let (background_calls, foreground_calls): (Vec<_>, Vec<_>) = tool_calls
        .iter()
        .cloned()
        .partition(|tc| context.tools.find(&tc.name).is_some_and(|t| t.background));

    let background_messages = dispatch_background_tool_calls(
        context,
        assistant_message,
        background_calls,
        config,
        emit,
        background,
    );

    let mut foreground = ExecutedToolCallBatch {
        messages: Vec::new(),
        terminate: false,
    };
    if !foreground_calls.is_empty() {
        let has_sequential = foreground_calls.iter().any(|tc| {
            context
                .tools
                .find(&tc.name)
                .is_some_and(|t| t.execution_mode == Some(ToolExecutionMode::Sequential))
        });
        foreground = if config.tool_execution == ToolExecutionMode::Sequential || has_sequential {
            execute_tool_calls_sequential(
                context,
                assistant_message,
                foreground_calls,
                config,
                emit,
            )
            .await
        } else {
            execute_tool_calls_parallel(context, assistant_message, foreground_calls, config, emit)
                .await
        };
    }

    let mut by_id: HashMap<String, ToolResultMessage> = background_messages
        .into_iter()
        .chain(foreground.messages)
        .map(|m| (m.tool_call_id.clone(), m))
        .collect();
    ExecutedToolCallBatch {
        messages: tool_calls
            .iter()
            .filter_map(|tc| by_id.remove(&tc.id))
            .collect(),
        // Only foreground results can end the run early.
        terminate: foreground.terminate,
    }
}

fn emit_tool_execution_start(tool_call: &AgentToolCall, emit: &mut AgentEventSink) {
    emit(AgentEvent::ToolExecutionStart {
        tool_call_id: tool_call.id.clone(),
        tool_name: tool_call.name.clone(),
        args: tool_call.arguments.clone(),
    });
}

fn emit_tool_execution_end(finalized: &FinalizedOutcome, emit: &mut AgentEventSink) {
    emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        result: finalized.result.clone(),
        is_error: finalized.is_error,
    });
}

/// `emitToolResultMessage`: announce a tool result as a message.
fn emit_tool_result_message(message: &ToolResultMessage, emit: &mut AgentEventSink) {
    emit(AgentEvent::MessageStart {
        message: AgentMessage::ToolResult(message.clone()),
    });
    emit(AgentEvent::MessageEnd {
        message: AgentMessage::ToolResult(message.clone()),
    });
}

fn create_tool_result_message(finalized: &FinalizedOutcome) -> ToolResultMessage {
    ToolResultMessage {
        tool_call_id: finalized.tool_call.id.clone(),
        tool_name: finalized.tool_call.name.clone(),
        content: finalized.result.content.clone(),
        // `details` is omitted when the tool set none.
        details: (!finalized.result.details.is_null()).then(|| finalized.result.details.clone()),
        is_error: finalized.is_error,
        timestamp: ai_types::now_ms(),
    }
}

/// `createErrorToolResult`: the message as the only text, empty details.
fn create_error_tool_result(message: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::text(message)],
        details: serde_json::json!({}),
        terminate: false,
    }
}

fn should_terminate_tool_batch(finalized: &[FinalizedOutcome]) -> bool {
    !finalized.is_empty() && finalized.iter().all(|f| f.result.terminate)
}

fn immediate_outcome(
    tool_call: &AgentToolCall,
    result: AgentToolResult,
    is_error: bool,
) -> FinalizedOutcome {
    FinalizedOutcome {
        tool_call: tool_call.clone(),
        result,
        is_error,
    }
}

async fn execute_tool_calls_sequential(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<AgentToolCall>,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized_calls = Vec::new();
    let mut messages = Vec::new();
    for tool_call in tool_calls {
        emit_tool_execution_start(&tool_call, emit);
        let finalized = match prepare_tool_call(context, assistant_message, &tool_call, config) {
            Preparation::Immediate { result, is_error } => {
                immediate_outcome(&tool_call, result, is_error)
            }
            Preparation::Prepared(prepared) => {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let run = execute_prepared_tool_call(&prepared, config.signal.clone(), Some(tx));
                tokio::pin!(run);
                let executed = loop {
                    tokio::select! {
                        biased;
                        Some(update) = rx.recv() => emit(update),
                        executed = &mut run => break executed,
                    }
                };
                while let Ok(update) = rx.try_recv() {
                    emit(update);
                }
                finalize_executed_tool_call(context, assistant_message, &prepared, executed, config)
            }
        };
        emit_tool_execution_end(&finalized, emit);
        let message = create_tool_result_message(&finalized);
        emit_tool_result_message(&message, emit);
        messages.push(message);
        finalized_calls.push(finalized);
    }
    ExecutedToolCallBatch {
        terminate: should_terminate_tool_batch(&finalized_calls),
        messages,
    }
}

/// `executeToolCallsParallel`: prepared tools run concurrently;
/// `tool_execution_end` is emitted as each settles, result messages after the
/// whole batch in call order.
async fn execute_tool_calls_parallel(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<AgentToolCall>,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> ExecutedToolCallBatch {
    let mut finalized: Vec<Option<FinalizedOutcome>> = vec![None; tool_calls.len()];
    let mut prepared_calls: Vec<Option<Box<PreparedToolCall>>> = Vec::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let running = FuturesUnordered::new();

    for (index, tool_call) in tool_calls.iter().enumerate() {
        emit_tool_execution_start(tool_call, emit);
        match prepare_tool_call(context, assistant_message, tool_call, config) {
            Preparation::Immediate { result, is_error } => {
                let outcome = immediate_outcome(tool_call, result, is_error);
                emit_tool_execution_end(&outcome, emit);
                finalized[index] = Some(outcome);
                prepared_calls.push(None);
            }
            Preparation::Prepared(prepared) => {
                let tool = prepared.tool.clone();
                let tool_call = prepared.tool_call.clone();
                let args = prepared.args.clone();
                let signal = config.signal.clone();
                let tx = tx.clone();
                running.push(async move {
                    let executed = run_tool(tool, tool_call, args, signal, Some(tx)).await;
                    (index, executed)
                });
                prepared_calls.push(Some(prepared));
            }
        }
    }
    drop(tx);

    tokio::pin!(running);
    let mut remaining = running.len();
    while remaining > 0 {
        tokio::select! {
            biased;
            Some(update) = rx.recv() => emit(update),
            Some((index, executed)) = running.next() => {
                remaining -= 1;
                let prepared = prepared_calls[index].as_ref().expect("a running call was prepared");
                let outcome =
                    finalize_executed_tool_call(context, assistant_message, prepared, executed, config);
                emit_tool_execution_end(&outcome, emit);
                finalized[index] = Some(outcome);
            }
        }
    }
    while let Ok(update) = rx.try_recv() {
        emit(update);
    }

    let finalized: Vec<FinalizedOutcome> = finalized.into_iter().flatten().collect();
    let mut messages = Vec::new();
    for outcome in &finalized {
        let message = create_tool_result_message(outcome);
        emit_tool_result_message(&message, emit);
        messages.push(message);
    }
    ExecutedToolCallBatch {
        terminate: should_terminate_tool_batch(&finalized),
        messages,
    }
}

/// `prepareToolCall`: find the tool, apply `prepareArguments`, validate, then
/// the permission gate and `beforeToolCall`. Failures become error results.
fn prepare_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
    config: &AgentLoopConfig,
) -> Preparation {
    let Some(tool) = context.tools.find(&tool_call.name) else {
        return Preparation::Immediate {
            result: create_error_tool_result(format!("Tool {} not found", tool_call.name)),
            is_error: true,
        };
    };
    let immediate_error = |message: String| Preparation::Immediate {
        result: create_error_tool_result(message),
        is_error: true,
    };

    let prepared_args = match &tool.prepare_arguments {
        Some(prepare) => prepare(tool_call.arguments.clone()),
        None => tool_call.arguments.clone(),
    };
    let validated_args = match validate_tool_arguments(tool, prepared_args) {
        Ok(args) => args,
        Err(message) => return immediate_error(message),
    };

    // cortex permission gate (not in hoocode's loop; its extensions block via hooks).
    if let Some(gate) = &config.permission_gate {
        if let PermissionDecision::Deny { reason } = gate.request(tool_call) {
            return immediate_error(reason);
        }
    }

    let mut args = validated_args;
    if let Some(before) = &config.before_tool_call {
        let mut before_context = BeforeToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args,
            context: context.clone(),
        };
        match before(&mut before_context, config.signal.clone()) {
            Ok(Some(result)) if result.block => {
                return immediate_error(
                    result
                        .reason
                        .filter(|r| !r.is_empty())
                        .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                );
            }
            Ok(_) => {}
            Err(e) => return immediate_error(e.to_string()),
        }
        args = before_context.args;
    }

    Preparation::Prepared(Box::new(PreparedToolCall {
        tool_call: tool_call.clone(),
        tool: tool.clone(),
        args,
    }))
}

/// `validateToolArguments`: TypeBox `Value.Convert` (or JSON-schema coercion
/// for plain-schema tools), then validation with TypeBox's error text.
fn validate_tool_arguments(
    tool: &AgentTool,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let origin = if tool.plain_json_schema {
        cortexcode_ai_util::SchemaOrigin::PlainJson
    } else {
        cortexcode_ai_util::SchemaOrigin::TypeBox
    };
    cortexcode_ai_util::validate_tool_arguments(&tool.name, &tool.parameters, &args, origin)
}

type UpdateSender = tokio::sync::mpsc::UnboundedSender<AgentEvent>;

async fn execute_prepared_tool_call(
    prepared: &PreparedToolCall,
    signal: Option<AbortSignal>,
    updates: Option<UpdateSender>,
) -> ExecutedOutcome {
    run_tool(
        prepared.tool.clone(),
        prepared.tool_call.clone(),
        prepared.args.clone(),
        signal,
        updates,
    )
    .await
}

/// Run a tool's synchronous `execute` on the blocking pool. Its `onUpdate`
/// partial results go to `updates` as `tool_execution_update` events.
async fn run_tool(
    tool: AgentTool,
    tool_call: AgentToolCall,
    args: serde_json::Value,
    signal: Option<AbortSignal>,
    updates: Option<UpdateSender>,
) -> ExecutedOutcome {
    let on_update: AgentToolUpdateCallback = {
        let tool_call = tool_call.clone();
        Box::new(move |partial_result| {
            if let Some(tx) = &updates {
                let _ = tx.send(AgentEvent::ToolExecutionUpdate {
                    tool_call_id: tool_call.id.clone(),
                    tool_name: tool_call.name.clone(),
                    args: tool_call.arguments.clone(),
                    partial_result,
                });
            }
        })
    };
    let execute = tool.execute.clone();
    let id = tool_call.id.clone();
    let outcome = tokio::task::spawn_blocking(move || execute(id, args, signal, Some(on_update)))
        .await
        .unwrap_or_else(|e| Err(format!("tool task failed: {e}").into()));
    match outcome {
        Ok(result) => ExecutedOutcome {
            result,
            is_error: false,
        },
        Err(e) => ExecutedOutcome {
            result: create_error_tool_result(e.to_string()),
            is_error: true,
        },
    }
}

/// `finalizeExecutedToolCall`: apply `afterToolCall` overrides; a failing
/// hook turns the result into an error.
fn finalize_executed_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    prepared: &PreparedToolCall,
    executed: ExecutedOutcome,
    config: &AgentLoopConfig,
) -> FinalizedOutcome {
    let mut result = executed.result;
    let mut is_error = executed.is_error;
    if let Some(after) = &config.after_tool_call {
        let after_context = AfterToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: prepared.tool_call.clone(),
            args: prepared.args.clone(),
            result: result.clone(),
            is_error,
            context: context.clone(),
        };
        match after(after_context, config.signal.clone()) {
            Ok(Some(overrides)) => {
                result = AgentToolResult {
                    content: overrides.content.unwrap_or(result.content),
                    details: overrides.details.unwrap_or(result.details),
                    terminate: overrides.terminate.unwrap_or(result.terminate),
                };
                is_error = overrides.is_error.unwrap_or(is_error);
            }
            Ok(None) => {}
            Err(e) => {
                result = create_error_tool_result(e.to_string());
                is_error = true;
            }
        }
    }
    FinalizedOutcome {
        tool_call: prepared.tool_call.clone(),
        result,
        is_error,
    }
}

// ---------------------------------------------------------------------------
// Background tools
// ---------------------------------------------------------------------------

/// `dispatchBackgroundToolCalls`: answer each call with a placeholder now and
/// run the tool detached; its result arrives as a later message.
fn dispatch_background_tool_calls(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: Vec<AgentToolCall>,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
    background: &BackgroundTaskManager,
) -> Vec<ToolResultMessage> {
    let mut messages = Vec::new();
    for tool_call in tool_calls {
        emit_tool_execution_start(&tool_call, emit);
        let prepared = match prepare_tool_call(context, assistant_message, &tool_call, config) {
            Preparation::Immediate { result, is_error } => {
                let finalized = immediate_outcome(&tool_call, result, is_error);
                emit_tool_execution_end(&finalized, emit);
                let message = create_tool_result_message(&finalized);
                emit_tool_result_message(&message, emit);
                messages.push(message);
                continue;
            }
            Preparation::Prepared(prepared) => prepared,
        };

        let placeholder = create_background_placeholder_outcome(&tool_call, config);
        emit_tool_execution_end(&placeholder, emit);
        let message = create_tool_result_message(&placeholder);
        emit_tool_result_message(&message, emit);
        messages.push(message);

        background.spawn(prepared, config.signal.clone(), assistant_message.clone());
    }
    messages
}

/// `createBackgroundPlaceholderOutcome`.
fn create_background_placeholder_outcome(
    tool_call: &AgentToolCall,
    config: &AgentLoopConfig,
) -> FinalizedOutcome {
    let text = config
        .create_background_placeholder
        .as_ref()
        .and_then(|make| make(tool_call.clone()))
        .unwrap_or_else(|| {
            format!(
                "Started \"{}\" in the background. Its result will arrive as a follow-up message once it finishes — keep working in the meantime.",
                tool_call.name
            )
        });
    FinalizedOutcome {
        tool_call: tool_call.clone(),
        result: AgentToolResult {
            content: vec![Content::text(text)],
            details: serde_json::json!({"background": true, "status": "running"}),
            terminate: false,
        },
        is_error: false,
    }
}

/// `createDefaultBackgroundResultMessage`.
fn create_default_background_result_message(finalized: &FinalizedOutcome) -> AgentMessage {
    let verb = if finalized.is_error {
        "failed"
    } else {
        "finished"
    };
    let header = format!(
        "Background tool \"{}\" (id {}) {verb}:",
        finalized.tool_call.name, finalized.tool_call.id
    );
    let mut content = vec![Content::text(header)];
    content.extend(finalized.result.content.iter().cloned());
    AgentMessage::User(UserMessage {
        content,
        timestamp: ai_types::now_ms(),
    })
}

/// A settled background task, finalized when the loop collects it.
enum SettledBackgroundTask {
    Executed {
        prepared: Box<PreparedToolCall>,
        assistant_message: Box<AssistantMessage>,
        executed: ExecutedOutcome,
    },
    /// The task itself failed (a panic in the tool).
    Failed(String),
}

fn background_result_message(
    context: &AgentContext,
    config: &AgentLoopConfig,
    settled: SettledBackgroundTask,
) -> AgentMessage {
    match settled {
        SettledBackgroundTask::Executed {
            prepared,
            assistant_message,
            executed,
        } => {
            let finalized = finalize_executed_tool_call(
                context,
                &assistant_message,
                &prepared,
                executed,
                config,
            );
            match &config.create_background_result_message {
                Some(make) => make(BackgroundToolResult {
                    tool_call: finalized.tool_call.clone(),
                    result: finalized.result.clone(),
                    is_error: finalized.is_error,
                }),
                None => create_default_background_result_message(&finalized),
            }
        }
        SettledBackgroundTask::Failed(message) => AgentMessage::User(UserMessage {
            content: vec![Content::text(format!(
                "A background tool failed unexpectedly: {message}"
            ))],
            timestamp: ai_types::now_ms(),
        }),
    }
}

/// `createBackgroundTaskManager`: detached background tool runs.
struct BackgroundTaskManager {
    inflight: Arc<AtomicUsize>,
    results: Arc<Mutex<Vec<SettledBackgroundTask>>>,
    settled: Arc<tokio::sync::Notify>,
    on_count_change: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

impl BackgroundTaskManager {
    fn new(on_count_change: Option<Arc<dyn Fn(usize) + Send + Sync>>) -> Self {
        Self {
            inflight: Arc::new(AtomicUsize::new(0)),
            results: Arc::new(Mutex::new(Vec::new())),
            settled: Arc::new(tokio::sync::Notify::new()),
            on_count_change,
        }
    }

    fn pending_count(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    fn drain_results(&self) -> Vec<SettledBackgroundTask> {
        std::mem::take(&mut *self.results.lock().unwrap())
    }

    /// Resolves once another in-flight task settles, or at once when results
    /// are waiting or nothing is in flight.
    async fn wait_for_next(&self) {
        let notified = self.settled.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.results.lock().unwrap().is_empty() || self.pending_count() == 0 {
            return;
        }
        notified.await;
    }

    fn spawn(
        &self,
        prepared: Box<PreparedToolCall>,
        signal: Option<AbortSignal>,
        assistant_message: AssistantMessage,
    ) {
        let count = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        report_count(&self.on_count_change, count);

        let inflight = self.inflight.clone();
        let results = self.results.clone();
        let settled = self.settled.clone();
        let on_count_change = self.on_count_change.clone();
        let tool = prepared.tool.clone();
        let tool_call = prepared.tool_call.clone();
        let args = prepared.args.clone();
        tokio::spawn(async move {
            // Background tools own their lifecycle: no update events.
            let task = tokio::spawn(run_tool(tool, tool_call, args, signal, None));
            let outcome = match task.await {
                Ok(executed) => SettledBackgroundTask::Executed {
                    prepared,
                    assistant_message: Box::new(assistant_message),
                    executed,
                },
                Err(e) => SettledBackgroundTask::Failed(e.to_string()),
            };
            results.lock().unwrap().push(outcome);
            let count = inflight.fetch_sub(1, Ordering::SeqCst) - 1;
            report_count(&on_count_change, count);
            settled.notify_waiters();
        });
    }
}

/// A throwing count callback must not break background dispatch.
fn report_count(callback: &Option<Arc<dyn Fn(usize) + Send + Sync>>, count: usize) {
    if let Some(callback) = callback {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(count)));
    }
}

#[cfg(test)]
mod tests;

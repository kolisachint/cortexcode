//! Low-level agent loop that processes prompts, streams responses, and executes tools.
//!
//! This crate mirrors `agent-loop.ts` from the TypeScript `@kolisachint/hoocode-agent-core`
//! package. It provides the standalone turn loop used by `cortexcode-agent-core`.

use cortexcode_agent_types::*;
use cortexcode_ai_types::{
    self as ai_types, AssistantMessage, AssistantMessageEvent, Content, Message,
    SimpleStreamOptions, StopReason, TextContent, ToolResultMessage,
};

use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Default convert_to_llm
// ---------------------------------------------------------------------------

/// Default conversion from `AgentMessage` to `Message` — filters out custom messages.
pub fn default_convert_to_llm(
    messages: Vec<AgentMessage>,
) -> Result<Vec<Message>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(messages
        .into_iter()
        // defaultConvertToLlm() in agent.ts: keep user/assistant/toolResult only.
        .filter_map(|msg| msg.extract_message())
        .collect())
}

// ---------------------------------------------------------------------------
// AgentEventSink
// ---------------------------------------------------------------------------

/// Callback type for emitting agent events.
pub type AgentEventSink = Box<dyn FnMut(AgentEvent) + Send>;

// ---------------------------------------------------------------------------
// ExecutedToolCallBatch
// ---------------------------------------------------------------------------

struct ExecutedToolCallBatch {
    messages: Vec<AgentMessage>,
    terminate: bool,
}

// ---------------------------------------------------------------------------
// BackgroundTaskManager
// ---------------------------------------------------------------------------

/// Manages background tool calls that run detached from the main loop.
struct BackgroundTaskManager {
    pending: Arc<std::sync::atomic::AtomicUsize>,
    results: Arc<Mutex<Vec<AgentMessage>>>,
    /// Notified when a background task finishes.
    notify: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl BackgroundTaskManager {
    fn new() -> Self {
        Self {
            pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            results: Arc::new(Mutex::new(Vec::new())),
            notify: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        }
    }

    fn pending_count(&self) -> usize {
        self.pending.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn drain_results(&mut self) -> Vec<AgentMessage> {
        let mut results = self.results.lock().unwrap();
        let drained = results.drain(..).collect();
        drained
    }

    /// Wait for at least one background task to complete.
    fn wait_for_next(&self) {
        let (lock, cvar) = &*self.notify;
        let mut notified = lock.lock().unwrap();
        while !*notified {
            notified = cvar.wait(notified).unwrap();
        }
        *notified = false;
    }

    #[allow(clippy::type_complexity)]
    fn spawn_background(
        &self,
        tool_call: AgentToolCall,
        execute: Box<
            dyn FnOnce(
                    String,
                    serde_json::Value,
                )
                    -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>
                + Send,
        >,
        create_message: Box<dyn FnOnce(BackgroundToolResult) -> AgentMessage + Send>,
        on_count_change: Option<Box<dyn Fn(usize) + Send>>,
    ) {
        self.pending
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let count = self.pending_count();
        if let Some(ref cb) = on_count_change {
            cb(count);
        }

        let pending = self.pending.clone();
        let results = self.results.clone();
        let notify = self.notify.clone();
        let tool_call_id = tool_call.id.clone();
        let args = tool_call.arguments.clone();

        std::thread::spawn(move || {
            let result = execute(tool_call_id, args);
            let (agent_result, is_error) = match result {
                Ok(r) => (r, false),
                Err(e) => (create_error_tool_result(e.to_string()), true),
            };

            let bg_result = BackgroundToolResult {
                tool_call,
                result: agent_result,
                is_error,
            };
            let msg = create_message(bg_result);
            {
                let mut res = results.lock().unwrap();
                res.push(msg);
            }
            let (lock, cvar) = &*notify;
            let mut notified = lock.lock().unwrap();
            *notified = true;
            cvar.notify_one();

            let remaining = pending.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
            if let Some(ref cb) = on_count_change {
                cb(remaining);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Stream assistant response
// ---------------------------------------------------------------------------

/// Stream an assistant response from the LLM.
fn stream_assistant_response(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<AssistantMessage, Box<dyn std::error::Error + Send + Sync>> {
    // Apply context transform if configured
    let messages = if let Some(ref transform) = config.transform_context {
        transform(context.messages.clone(), config.signal.clone())?
    } else {
        context.messages.clone()
    };

    // Convert to LLM-compatible messages
    let convert = config
        .convert_to_llm
        .as_ref()
        .map(|c| c as &dyn Fn(Vec<AgentMessage>) -> Result<Vec<Message>, _>)
        .unwrap_or(&|msgs| default_convert_to_llm(msgs));
    let llm_messages = convert(messages)?;

    // Build LLM context
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

    // Resolve API key
    let resolved_api_key = config
        .get_api_key
        .as_ref()
        .and_then(|get_key| get_key(config.model.provider.clone()).ok())
        .flatten()
        .or_else(|| config.api_key.clone());

    // Build stream options
    let options = SimpleStreamOptions {
        signal: config.signal.clone(),
        api_key: resolved_api_key,
        session_id: config.session_id.clone(),
        reasoning: config.reasoning.clone(),
        thinking_budgets: config.thinking_budgets.clone(),
        thinking_display: config.thinking_display.clone(),
        ..Default::default()
    };

    // We need a stream function. For now, use a simple blocking approach.
    // In production, this would be the provider's stream implementation.
    let stream_result = if let Some(ref stream_fn) = config.stream_fn {
        (stream_fn)(config.model.clone(), llm_context, options)
    } else {
        return Err("No stream function configured".into());
    };

    let mut event_stream = stream_result?;

    let mut partial_message: Option<AssistantMessage> = None;
    let mut added_partial = false;
    let mut final_message: Option<AssistantMessage> = None;

    loop {
        match event_stream.next_event() {
            Some(AssistantMessageEvent::Start { partial }) => {
                partial_message = Some(partial.clone());
                context
                    .messages
                    .push(AgentMessage::from_message(Message::Assistant(
                        partial.clone(),
                    )));
                added_partial = true;
                emit(AgentEvent::MessageStart {
                    message: AgentMessage::from_message(Message::Assistant(partial)),
                });
            }
            Some(
                event @ (AssistantMessageEvent::TextStart { .. }
                | AssistantMessageEvent::TextDelta { .. }
                | AssistantMessageEvent::TextEnd { .. }
                | AssistantMessageEvent::ThinkingStart { .. }
                | AssistantMessageEvent::ThinkingDelta { .. }
                | AssistantMessageEvent::ThinkingEnd { .. }
                | AssistantMessageEvent::ToolCallStart { .. }
                | AssistantMessageEvent::ToolCallDelta { .. }
                | AssistantMessageEvent::ToolCallEnd { .. }),
            ) => {
                if let Some(ref partial) = partial_message {
                    // The event carries the updated partial message
                    let updated = match &event {
                        AssistantMessageEvent::TextStart { partial, .. }
                        | AssistantMessageEvent::TextDelta { partial, .. }
                        | AssistantMessageEvent::TextEnd { partial, .. }
                        | AssistantMessageEvent::ThinkingStart { partial, .. }
                        | AssistantMessageEvent::ThinkingDelta { partial, .. }
                        | AssistantMessageEvent::ThinkingEnd { partial, .. }
                        | AssistantMessageEvent::ToolCallStart { partial, .. }
                        | AssistantMessageEvent::ToolCallDelta { partial, .. }
                        | AssistantMessageEvent::ToolCallEnd { partial, .. } => partial,
                        _ => partial,
                    };
                    if let Some(last) = context.messages.last_mut() {
                        *last = AgentMessage::from_message(Message::Assistant(updated.clone()));
                    }
                    emit(AgentEvent::MessageUpdate {
                        assistant_message_event:
                            cortexcode_agent_types::AssistantMessagePartialEvent::TextDelta {
                                index: 0,
                                delta: String::new(),
                            },
                        message: AgentMessage::from_message(Message::Assistant(updated.clone())),
                    });
                }
            }
            Some(AssistantMessageEvent::Done { message })
            | Some(AssistantMessageEvent::Error { error: message }) => {
                final_message = Some(message.clone());
                if added_partial {
                    if let Some(last) = context.messages.last_mut() {
                        *last = AgentMessage::from_message(Message::Assistant(message.clone()));
                    }
                } else {
                    context
                        .messages
                        .push(AgentMessage::from_message(Message::Assistant(
                            message.clone(),
                        )));
                    emit(AgentEvent::MessageStart {
                        message: AgentMessage::from_message(Message::Assistant(message.clone())),
                    });
                }
                emit(AgentEvent::MessageEnd {
                    message: AgentMessage::from_message(Message::Assistant(message)),
                });
                break;
            }
            None => break,
        }
    }

    final_message.ok_or_else(|| "Stream ended without producing a final message".into())
}

// ---------------------------------------------------------------------------
// Tool execution helpers
// ---------------------------------------------------------------------------

struct PreparedToolCall {
    kind: PreparedToolCallKind,
}

enum PreparedToolCallKind {
    /// The tool was prepared and can be executed.
    Ready {
        tool: Box<AgentTool>,
        args: serde_json::Value,
    },
    /// The tool should return an immediate error result.
    Blocked { reason: String },
    /// The tool was not found.
    NotFound,
}

fn prepare_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
    config: &AgentLoopConfig,
) -> PreparedToolCall {
    // Find the tool
    let tool = match context.tools.find(&tool_call.name) {
        Some(t) => t,
        None => {
            return PreparedToolCall {
                kind: PreparedToolCallKind::NotFound,
            };
        }
    };

    // Prepare arguments (apply compatibility shim if configured)
    let args = if let Some(ref prepare) = tool.prepare_arguments {
        prepare(tool_call.arguments.clone())
    } else {
        tool_call.arguments.clone()
    };

    // Apply permission gate if configured.
    if let Some(ref gate) = config.permission_gate {
        match gate.request(tool_call) {
            cortexcode_agent_types::PermissionDecision::Grant => {}
            cortexcode_agent_types::PermissionDecision::GrantAlways => {}
            cortexcode_agent_types::PermissionDecision::Deny { reason } => {
                return PreparedToolCall {
                    kind: PreparedToolCallKind::Blocked { reason },
                };
            }
        }
    }

    // Call before_tool_call hook
    if let Some(ref before) = config.before_tool_call {
        let before_ctx = BeforeToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args: args.clone(),
            context: context.clone(),
        };
        if let Ok(Some(result)) = before(before_ctx, config.signal.clone()) {
            if result.block {
                return PreparedToolCall {
                    kind: PreparedToolCallKind::Blocked {
                        reason: result
                            .reason
                            .filter(|r| !r.is_empty())
                            .unwrap_or_else(|| "Tool execution was blocked".to_string()),
                    },
                };
            }
        }
    }

    PreparedToolCall {
        kind: PreparedToolCallKind::Ready {
            tool: Box::new(tool.clone()),
            args,
        },
    }
}

fn execute_single_tool(
    tool: &AgentTool,
    tool_call_id: &str,
    args: serde_json::Value,
) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>> {
    (tool.execute)(tool_call_id.to_string(), args, None, None)
}

// ---------------------------------------------------------------------------
// Tool execution modes
// ---------------------------------------------------------------------------

/// Run a batch of tool calls one at a time. With `batch_result_messages`, the
/// tool result messages are announced after the whole batch, in call order,
/// the way `executeToolCallsParallel` does (siblings never see each other's
/// results); otherwise each one right after its tool (`executeToolCallsSequential`).
fn execute_tool_calls_sequential(
    context: &mut AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
    background: &mut BackgroundTaskManager,
    batch_result_messages: bool,
) -> ExecutedToolCallBatch {
    let mut messages = Vec::new();
    let mut pending_announcements: Vec<AgentMessage> = Vec::new();
    let mut terminate = false;

    // Separate foreground and background
    let background_calls: Vec<&AgentToolCall> = tool_calls
        .iter()
        .filter(|tc| {
            context
                .tools
                .find(&tc.name)
                .map(|t| t.background)
                .unwrap_or(false)
        })
        .collect();
    let foreground_calls: Vec<&AgentToolCall> = tool_calls
        .iter()
        .filter(|tc| {
            !context
                .tools
                .find(&tc.name)
                .map(|t| t.background)
                .unwrap_or(false)
        })
        .collect();

    // Process background calls
    for tc in &background_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: tc.arguments.clone(),
        });

        if let PreparedToolCallKind::Ready { tool, args: _ } =
            prepare_tool_call(context, assistant_message, tc, config).kind
        {
            let tc_owned: AgentToolCall = (*tc).clone();

            // createBackgroundPlaceholderOutcome() in agent-loop.ts (generic fallback text).
            let placeholder = AgentMessage::from_message(Message::ToolResult(ToolResultMessage {
                details: Some(serde_json::json!({"background": true, "status": "running"})),
                content: vec![Content::text(format!(
                    "Started \"{}\" in the background. Its result will arrive as a follow-up message once it finishes — keep working in the meantime.",
                    tc.name
                ))],
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                is_error: false,
                timestamp: cortexcode_ai_types::now_ms(),
            }));
            emit_tool_result_message(&placeholder, emit);
            messages.push(placeholder);

            background.spawn_background(
                tc_owned,
                Box::new(move |id, args| execute_single_tool(&tool, &id, args)),
                // createDefaultBackgroundResultMessage() in agent-loop.ts.
                Box::new(|bg: BackgroundToolResult| {
                    let verb = if bg.is_error { "failed" } else { "finished" };
                    let header = format!(
                        "Background tool \"{}\" (id {}) {}:",
                        bg.tool_call.name, bg.tool_call.id, verb
                    );
                    let mut content = vec![Content::text(header)];
                    content.extend(bg.result.content.iter().cloned());
                    AgentMessage::User(cortexcode_ai_types::UserMessage {
                        content,
                        timestamp: cortexcode_ai_types::now_ms(),
                    })
                }),
                None,
            );
        }
    }

    // Process foreground calls sequentially
    for tc in &foreground_calls {
        emit(AgentEvent::ToolExecutionStart {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: tc.arguments.clone(),
        });

        let result = execute_single_tool_call(context, assistant_message, tc, config);

        // Apply after_tool_call hook
        let (final_result, is_error, should_terminate) =
            apply_after_tool_call(&result, config, context, assistant_message, tc);

        // createToolResultMessage(): `details` is omitted when the tool set none.
        let tool_result_message =
            AgentMessage::from_message(Message::ToolResult(ToolResultMessage {
                details: (!final_result.details.is_null()).then(|| final_result.details.clone()),
                content: final_result.content.clone(),
                tool_call_id: tc.id.clone(),
                tool_name: tc.name.clone(),
                is_error,
                timestamp: cortexcode_ai_types::now_ms(),
            }));

        emit(AgentEvent::ToolExecutionEnd {
            tool_call_id: tc.id.clone(),
            tool_name: tc.name.clone(),
            args: tc.arguments.clone(),
            result: final_result,
            is_error,
        });
        if batch_result_messages {
            pending_announcements.push(tool_result_message.clone());
        } else {
            emit_tool_result_message(&tool_result_message, emit);
        }
        messages.push(tool_result_message);

        if should_terminate {
            terminate = true;
        }
    }
    for message in &pending_announcements {
        emit_tool_result_message(message, emit);
    }

    ExecutedToolCallBatch {
        messages,
        terminate,
    }
}

fn execute_tool_calls_parallel(
    context: &mut AgentContext,
    assistant_message: &AssistantMessage,
    tool_calls: &[AgentToolCall],
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
    background: &mut BackgroundTaskManager,
) -> ExecutedToolCallBatch {
    // Tools still run one at a time; only the result-message ordering is
    // parallel-mode's. Concurrent execution is not ported yet.
    execute_tool_calls_sequential(
        context,
        assistant_message,
        tool_calls,
        config,
        emit,
        background,
        true,
    )
}

/// `emitToolResultMessage`: announce a tool result as a message.
fn emit_tool_result_message(message: &AgentMessage, emit: &mut AgentEventSink) {
    emit(AgentEvent::MessageStart {
        message: message.clone(),
    });
    emit(AgentEvent::MessageEnd {
        message: message.clone(),
    });
}

fn execute_single_tool_call(
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
    config: &AgentLoopConfig,
) -> Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>> {
    let prepared = prepare_tool_call(context, assistant_message, tool_call, config);
    match prepared.kind {
        PreparedToolCallKind::Ready { tool, args } => {
            (tool.execute)(tool_call.id.clone(), args, config.signal.clone(), None)
        }
        // Immediate outcomes are error results (createErrorToolResult in agent-loop.ts).
        PreparedToolCallKind::Blocked { reason } => Err(reason.into()),
        PreparedToolCallKind::NotFound => Err(format!("Tool {} not found", tool_call.name).into()),
    }
}

/// `createErrorToolResult`: the error message as the only text, empty details.
fn create_error_tool_result(message: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: message,
            cache_control: None,
        })],
        details: serde_json::json!({}),
        terminate: false,
    }
}

fn apply_after_tool_call(
    result: &Result<AgentToolResult, Box<dyn std::error::Error + Send + Sync>>,
    config: &AgentLoopConfig,
    context: &AgentContext,
    assistant_message: &AssistantMessage,
    tool_call: &AgentToolCall,
) -> (AgentToolResult, bool, bool) {
    let (mut final_result, mut is_error) = match result {
        Ok(r) => (r.clone(), false),
        Err(e) => (create_error_tool_result(e.to_string()), true),
    };

    if let Some(ref after) = config.after_tool_call {
        let after_ctx = AfterToolCallContext {
            assistant_message: assistant_message.clone(),
            tool_call: tool_call.clone(),
            args: tool_call.arguments.clone(),
            result: final_result.clone(),
            is_error,
            context: context.clone(),
        };
        if let Ok(Some(override_result)) = after(after_ctx, config.signal.clone()) {
            if let Some(content) = override_result.content {
                final_result.content = content;
            }
            if let Some(e) = override_result.is_error {
                is_error = e;
            }
            if let Some(t) = override_result.terminate {
                final_result.terminate = t;
            }
        }
    }

    let terminate = final_result.terminate;
    (final_result, is_error, terminate)
}

// ---------------------------------------------------------------------------
// Main loop logic
// ---------------------------------------------------------------------------

/// Run the agent loop with new prompt messages.
pub fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    mut context: AgentContext,
    config: AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
    let mut new_messages: Vec<AgentMessage> = prompts.clone();

    // Add prompts to context
    for msg in &prompts {
        context.messages.push(msg.clone());
    }

    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    for msg in &prompts {
        emit(AgentEvent::MessageStart {
            message: msg.clone(),
        });
        emit(AgentEvent::MessageEnd {
            message: msg.clone(),
        });
    }

    run_loop(&mut context, &mut new_messages, &config, emit)?;

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });

    Ok(new_messages)
}

/// Run the agent loop continuing from existing context (no new prompt).
pub fn run_agent_loop_continue(
    context: &mut AgentContext,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
    if context.messages.is_empty() {
        return Err("Cannot continue: no messages in context".into());
    }

    if let Some(AgentMessage::Assistant(_)) = context.messages.last() {
        return Err("Cannot continue from message role: assistant".into());
    }

    let mut new_messages = Vec::new();

    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);

    run_loop(context, &mut new_messages, config, emit)?;

    emit(AgentEvent::AgentEnd {
        messages: new_messages.clone(),
    });

    Ok(new_messages)
}

/// Shared loop logic.
fn run_loop(
    context: &mut AgentContext,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentLoopConfig,
    emit: &mut AgentEventSink,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut background = BackgroundTaskManager::new();
    let mut first_turn = true;

    // Helper to collect pending messages (steering + background results)
    let collect_pending =
        |config: &AgentLoopConfig, bg: &mut BackgroundTaskManager| -> Vec<AgentMessage> {
            let mut results = bg.drain_results();
            let steering = config
                .get_steering_messages
                .as_ref()
                .and_then(|f| f().ok())
                .unwrap_or_default();
            results.extend(steering);
            results
        };

    let mut pending_messages: Vec<AgentMessage> = collect_pending(config, &mut background);

    // Outer loop: continues when follow-up messages arrive
    loop {
        let mut has_more_tool_calls = true;

        // Inner loop: process tool calls, steering, background results
        while has_more_tool_calls || !pending_messages.is_empty() || background.pending_count() > 0
        {
            // Nothing new to act on, but background work is in flight — wait for it
            if !has_more_tool_calls && pending_messages.is_empty() {
                background.wait_for_next();
                pending_messages = collect_pending(config, &mut background);
                if pending_messages.is_empty() {
                    continue;
                }
            }

            if !first_turn {
                emit(AgentEvent::TurnStart);
            } else {
                first_turn = false;
            }

            // Process pending messages before next assistant response
            if !pending_messages.is_empty() {
                let msgs: Vec<AgentMessage> = std::mem::take(&mut pending_messages);
                for msg in &msgs {
                    emit(AgentEvent::MessageStart {
                        message: msg.clone(),
                    });
                    emit(AgentEvent::MessageEnd {
                        message: msg.clone(),
                    });
                    context.messages.push(msg.clone());
                    new_messages.push(msg.clone());
                }
            }

            // Check for abort signal
            if let Some(ref signal) = config.signal {
                if signal.aborted() {
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    });
                    return Ok(());
                }
            }

            // Stream assistant response
            let message = stream_assistant_response(context, config, emit)?;

            new_messages.push(AgentMessage::from_message(Message::Assistant(
                message.clone(),
            )));

            if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                emit(AgentEvent::TurnEnd {
                    message: message.clone(),
                    tool_results: vec![],
                });
                emit(AgentEvent::AgentEnd {
                    messages: new_messages.clone(),
                });
                return Ok(());
            }

            // Extract tool calls
            let tool_calls: Vec<AgentToolCall> = message
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
                .collect();

            let mut tool_results: Vec<AgentMessage> = Vec::new();
            has_more_tool_calls = false;

            if !tool_calls.is_empty() {
                let batch = if config.tool_execution == ToolExecutionMode::Sequential {
                    execute_tool_calls_sequential(
                        context,
                        &message,
                        &tool_calls,
                        config,
                        emit,
                        &mut background,
                        false,
                    )
                } else {
                    execute_tool_calls_parallel(
                        context,
                        &message,
                        &tool_calls,
                        config,
                        emit,
                        &mut background,
                    )
                };

                tool_results.extend(batch.messages);
                has_more_tool_calls = !batch.terminate;

                for result in &tool_results {
                    context.messages.push(result.clone());
                    new_messages.push(result.clone());
                }
            }

            let tool_results_msg: Vec<Message> = tool_results
                .iter()
                .filter_map(|m| m.extract_message())
                .collect();

            emit(AgentEvent::TurnEnd {
                message: message.clone(),
                tool_results: tool_results_msg.clone(),
            });

            // Check should_stop_after_turn
            if let Some(ref should_stop) = config.should_stop_after_turn {
                let stop_ctx = ShouldStopAfterTurnContext {
                    message: message.clone(),
                    tool_results: tool_results_msg.clone(),
                    context: context.clone(),
                    new_messages: new_messages.clone(),
                };
                if should_stop(stop_ctx).unwrap_or(false) {
                    emit(AgentEvent::AgentEnd {
                        messages: new_messages.clone(),
                    });
                    return Ok(());
                }
            }

            // Prepare next turn
            if let Some(ref prepare) = config.prepare_next_turn {
                let next_ctx = PrepareNextTurnContext {
                    message: message.clone(),
                    tool_results: tool_results_msg.clone(),
                    context: context.clone(),
                    new_messages: new_messages.clone(),
                };
                if let Ok(Some(update)) = prepare(next_ctx) {
                    if let Some(ctx) = update.context {
                        *context = ctx;
                    }
                }
            }

            pending_messages = collect_pending(config, &mut background);
        }

        // Agent would stop here. Check for follow-up messages.
        let follow_up_messages = config
            .get_follow_up_messages
            .as_ref()
            .and_then(|f| f().ok())
            .unwrap_or_default();

        if !follow_up_messages.is_empty() {
            pending_messages = follow_up_messages;
            continue;
        }

        // No more messages, exit
        break;
    }

    Ok(())
}

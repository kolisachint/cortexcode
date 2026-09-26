//! Core agent runtime for cortex agents.
//!
//! Port of hoocode `packages/agent/src/agent.ts`: [`Agent`] is the stateful
//! wrapper around the agent loop. It owns the transcript (reduced from loop
//! events), runs one prompt at a time with its own abort signal, notifies
//! listeners, and queues steering and follow-up messages.
//!
//! Differences from TypeScript: listeners are synchronous closures (a run
//! cannot finish before they return, which is what awaiting them guarantees),
//! and state is read through [`Agent::state`] snapshots and changed through
//! setters instead of property assignment.

pub mod types;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cortexcode_agent_loop::{
    default_convert_to_llm, run_agent_loop, run_agent_loop_continue, AgentEventSink, BoxError,
};
use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{
    self as ai_types, AbortSignal, AssistantMessage, Content, ImageContent, Message, Model,
    SimpleStreamOptions, StopReason, ThinkingBudgets, ThinkingDisplay, ThinkingLevel, Transport,
    UserMessage,
};
use types::*;

// ---------------------------------------------------------------------------
// Hook types (shared, so they can be assigned after construction)
// ---------------------------------------------------------------------------

/// Function type for creating an AI stream for a given model, context, and options.
pub type StreamFn = Box<
    dyn Fn(
            Model,
            ai_types::Context,
            SimpleStreamOptions,
        ) -> Result<AssistantMessageEventStream, BoxError>
        + Send
        + Sync,
>;

/// Arc-wrapped stream function.
pub type SharedStreamFn = Arc<StreamFn>;

pub type ConvertToLlmFn =
    Arc<dyn Fn(Vec<AgentMessage>) -> Result<Vec<Message>, BoxError> + Send + Sync>;
pub type TransformContextFn = Arc<
    dyn Fn(Vec<AgentMessage>, Option<AbortSignal>) -> Result<Vec<AgentMessage>, BoxError>
        + Send
        + Sync,
>;
pub type GetApiKeyFn = Arc<dyn Fn(String) -> Result<Option<String>, BoxError> + Send + Sync>;
pub type BeforeToolCallFn = Arc<
    dyn Fn(
            &mut BeforeToolCallContext,
            Option<AbortSignal>,
        ) -> Result<Option<BeforeToolCallResult>, BoxError>
        + Send
        + Sync,
>;
pub type AfterToolCallFn = Arc<
    dyn Fn(
            AfterToolCallContext,
            Option<AbortSignal>,
        ) -> Result<Option<AfterToolCallResult>, BoxError>
        + Send
        + Sync,
>;
pub type PrepareNextTurnFn = Arc<
    dyn Fn(
            PrepareNextTurnContext,
            Option<AbortSignal>,
        ) -> Result<Option<AgentLoopTurnUpdate>, BoxError>
        + Send
        + Sync,
>;
pub type BackgroundResultMessageFn =
    Arc<dyn Fn(BackgroundToolResult) -> AgentMessage + Send + Sync>;
pub type BackgroundPlaceholderFn = Arc<dyn Fn(AgentToolCall) -> Option<String> + Send + Sync>;

/// A listener gets each event and the running prompt's abort signal.
pub type Listener = Arc<dyn Fn(&AgentEvent, &AbortSignal) + Send + Sync>;

// ---------------------------------------------------------------------------
// PendingMessageQueue
// ---------------------------------------------------------------------------

/// Controls how queued messages are drained (`QueueMode`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum QueueMode {
    /// All queued messages are drained at once.
    All,
    /// One message is drained at a time.
    #[default]
    OneAtATime,
}

struct PendingMessageQueue {
    mode: QueueMode,
    messages: Vec<AgentMessage>,
}

impl PendingMessageQueue {
    fn new(mode: QueueMode) -> Self {
        Self {
            mode,
            messages: Vec::new(),
        }
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        if self.mode == QueueMode::All || self.messages.is_empty() {
            std::mem::take(&mut self.messages)
        } else {
            self.messages.drain(..1).collect()
        }
    }
}

type Queue = Arc<Mutex<PendingMessageQueue>>;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// `DEFAULT_MODEL` in agent.ts.
fn default_model() -> Model {
    Model {
        compat: None,
        id: "unknown".into(),
        name: "unknown".into(),
        api: "unknown".into(),
        provider: "unknown".into(),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ai_types::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
    }
}

fn default_state() -> AgentState {
    AgentState {
        system_prompt: String::new(),
        model: default_model(),
        thinking_level: ThinkingLevel::Off,
        tools: AgentTools::new(vec![]),
        messages: vec![],
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: HashSet::new(),
        error_message: None,
    }
}

/// Settings forwarded to every run (public fields on the TypeScript agent).
#[derive(Clone)]
struct RunSettings {
    api_key: Option<String>,
    session_id: Option<String>,
    thinking_budgets: Option<ThinkingBudgets>,
    thinking_display: Option<ThinkingDisplay>,
    transport: Option<Transport>,
    max_retry_delay_ms: Option<u64>,
    tool_execution: ToolExecutionMode,
    on_payload: Option<cortexcode_ai_types::OnPayload>,
    on_response: Option<cortexcode_ai_types::OnResponse>,
}

#[derive(Clone, Default)]
struct Hooks {
    stream_fn: Option<SharedStreamFn>,
    convert_to_llm: Option<ConvertToLlmFn>,
    transform_context: Option<TransformContextFn>,
    get_api_key: Option<GetApiKeyFn>,
    before_tool_call: Option<BeforeToolCallFn>,
    after_tool_call: Option<AfterToolCallFn>,
    prepare_next_turn: Option<PrepareNextTurnFn>,
    create_background_result_message: Option<BackgroundResultMessageFn>,
    create_background_placeholder: Option<BackgroundPlaceholderFn>,
    on_background_task_count_change: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
}

/// State shared with the event sink of a running prompt.
struct Shared {
    state: Mutex<AgentState>,
    #[allow(clippy::type_complexity)]
    listeners: Mutex<Vec<(usize, Listener)>>,
    /// The running prompt's abort signal (`activeRun`).
    active: Mutex<Option<AbortSignal>>,
    idle: tokio::sync::Notify,
    hooks: Mutex<Hooks>,
}

impl Shared {
    /// `processEvents`: reduce state for a loop event, then notify listeners.
    fn process_event(&self, event: AgentEvent) {
        {
            let mut state = self.state.lock().unwrap();
            match &event {
                AgentEvent::MessageStart { message }
                | AgentEvent::MessageUpdate { message, .. } => {
                    state.streaming_message = Some(message.clone());
                }
                AgentEvent::MessageEnd { message } => {
                    state.streaming_message = None;
                    state.messages.push(message.clone());
                }
                AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
                    state.pending_tool_calls.insert(tool_call_id.clone());
                }
                AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
                    state.pending_tool_calls.remove(tool_call_id);
                }
                AgentEvent::TurnEnd { message, .. } => {
                    if let Some(error) = &message.error_message {
                        state.error_message = Some(error.clone());
                    }
                }
                AgentEvent::AgentEnd { .. } => state.streaming_message = None,
                _ => {}
            }
        }
        let Some(signal) = self.active.lock().unwrap().clone() else {
            // TS throws "Agent listener invoked outside active run".
            return;
        };
        // Call a copy so listeners may (un)subscribe without deadlocking.
        let listeners: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(&event, &signal);
        }
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// Errors from starting a run (the run's own failures become messages).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentError(pub String);

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AgentError {}

/// Stateful wrapper around the agent loop.
pub struct Agent {
    shared: Arc<Shared>,
    next_listener_id: Arc<AtomicUsize>,
    steering_queue: Queue,
    follow_up_queue: Queue,
    settings: Mutex<RunSettings>,
}

/// Handle returned by [`Agent::subscribe`]. Removes the listener when dropped.
pub struct Subscription {
    id: usize,
    shared: Arc<Shared>,
}

impl Subscription {
    /// Remove the listener now (the TypeScript unsubscribe function).
    pub fn unsubscribe(self) {}
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared
            .listeners
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != self.id);
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent {
    pub fn new() -> Self {
        Self::with_options(AgentOptions::default())
    }

    pub fn with_options(options: AgentOptions) -> Self {
        let mut state = options.initial_state.unwrap_or_else(default_state);
        state.is_streaming = false;
        state.streaming_message = None;
        state.pending_tool_calls = HashSet::new();
        state.error_message = None;
        Agent {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                listeners: Mutex::new(Vec::new()),
                active: Mutex::new(None),
                idle: tokio::sync::Notify::new(),
                hooks: Mutex::new(Hooks {
                    stream_fn: options.stream_fn,
                    convert_to_llm: options.convert_to_llm,
                    transform_context: options.transform_context,
                    get_api_key: options.get_api_key,
                    before_tool_call: options.before_tool_call,
                    after_tool_call: options.after_tool_call,
                    prepare_next_turn: options.prepare_next_turn,
                    create_background_result_message: options.create_background_result_message,
                    create_background_placeholder: options.create_background_placeholder,
                    on_background_task_count_change: options.on_background_task_count_change,
                    permission_gate: options.permission_gate,
                }),
            }),
            next_listener_id: Arc::new(AtomicUsize::new(1)),
            steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.steering_mode.unwrap_or_default(),
            ))),
            follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.follow_up_mode.unwrap_or_default(),
            ))),
            settings: Mutex::new(RunSettings {
                api_key: options.api_key,
                session_id: options.session_id,
                thinking_budgets: options.thinking_budgets,
                thinking_display: options.thinking_display,
                transport: options.transport,
                max_retry_delay_ms: options.max_retry_delay_ms,
                tool_execution: options.tool_execution.unwrap_or_default(),
                on_payload: options.on_payload,
                on_response: options.on_response,
            }),
        }
    }

    // -----------------------------------------------------------------------
    // State
    // -----------------------------------------------------------------------

    /// A snapshot of the current state.
    pub fn state(&self) -> AgentState {
        self.shared.state.lock().unwrap().clone()
    }

    pub fn set_system_prompt(&self, system_prompt: impl Into<String>) {
        self.shared.state.lock().unwrap().system_prompt = system_prompt.into();
    }

    pub fn set_model(&self, model: Model) {
        self.shared.state.lock().unwrap().model = model;
    }

    pub fn set_thinking_level(&self, level: ThinkingLevel) {
        self.shared.state.lock().unwrap().thinking_level = level;
    }

    /// `state.tools = tools` (copies the list).
    pub fn set_tools(&self, tools: Vec<AgentTool>) {
        self.shared.state.lock().unwrap().tools = AgentTools::new(tools);
    }

    /// `state.messages = messages` (copies the list).
    pub fn set_messages(&self, messages: Vec<AgentMessage>) {
        self.shared.state.lock().unwrap().messages = messages;
    }

    /// `state.messages.push(message)`.
    pub fn append_message(&self, message: AgentMessage) {
        self.shared.state.lock().unwrap().messages.push(message);
    }

    // -----------------------------------------------------------------------
    // Settings and hooks
    // -----------------------------------------------------------------------

    pub fn session_id(&self) -> Option<String> {
        self.settings.lock().unwrap().session_id.clone()
    }

    pub fn set_session_id(&self, session_id: Option<String>) {
        self.settings.lock().unwrap().session_id = session_id;
    }

    pub fn set_tool_execution(&self, mode: ToolExecutionMode) {
        self.settings.lock().unwrap().tool_execution = mode;
    }

    pub fn set_thinking_budgets(&self, budgets: Option<ThinkingBudgets>) {
        self.settings.lock().unwrap().thinking_budgets = budgets;
    }

    pub fn set_max_retry_delay_ms(&self, ms: Option<u64>) {
        self.settings.lock().unwrap().max_retry_delay_ms = ms;
    }

    /// `agent.onPayload`: inspect or replace each provider request body.
    pub fn set_on_payload(&self, hook: Option<cortexcode_ai_types::OnPayload>) {
        self.settings.lock().unwrap().on_payload = hook;
    }

    /// `agent.onResponse`: see each provider response's status and headers.
    pub fn set_on_response(&self, hook: Option<cortexcode_ai_types::OnResponse>) {
        self.settings.lock().unwrap().on_response = hook;
    }

    /// `agent.prepareNextTurn = …`. Takes effect for the running prompt too:
    /// the loop always calls through to the current hook.
    pub fn set_prepare_next_turn(&self, hook: Option<PrepareNextTurnFn>) {
        self.shared.hooks.lock().unwrap().prepare_next_turn = hook;
    }

    pub fn set_before_tool_call(&self, hook: Option<BeforeToolCallFn>) {
        self.shared.hooks.lock().unwrap().before_tool_call = hook;
    }

    pub fn set_after_tool_call(&self, hook: Option<AfterToolCallFn>) {
        self.shared.hooks.lock().unwrap().after_tool_call = hook;
    }

    pub fn set_transform_context(&self, hook: Option<TransformContextFn>) {
        self.shared.hooks.lock().unwrap().transform_context = hook;
    }

    /// `agent.getApiKey = …`: resolve the key per request.
    pub fn set_get_api_key(&self, hook: Option<GetApiKeyFn>) {
        self.shared.hooks.lock().unwrap().get_api_key = hook;
    }

    pub fn set_stream_fn(&self, stream_fn: Option<SharedStreamFn>) {
        self.shared.hooks.lock().unwrap().stream_fn = stream_fn;
    }

    // -----------------------------------------------------------------------
    // Listeners
    // -----------------------------------------------------------------------

    /// Subscribe to lifecycle events. Listeners run in subscription order and
    /// get the running prompt's abort signal. Dropping the handle unsubscribes.
    pub fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&AgentEvent, &AbortSignal) + Send + Sync + 'static,
    {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.shared
            .listeners
            .lock()
            .unwrap()
            .push((id, Arc::new(listener)));
        Subscription {
            id,
            shared: self.shared.clone(),
        }
    }

    // -----------------------------------------------------------------------
    // Queueing
    // -----------------------------------------------------------------------

    pub fn steering_mode(&self) -> QueueMode {
        self.steering_queue.lock().unwrap().mode
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.steering_queue.lock().unwrap().mode = mode;
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.follow_up_queue.lock().unwrap().mode
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.follow_up_queue.lock().unwrap().mode = mode;
    }

    /// Queue a message to be injected after the current assistant turn finishes.
    pub fn steer(&self, message: AgentMessage) {
        self.steering_queue.lock().unwrap().messages.push(message);
    }

    /// Queue a message to run only after the agent would otherwise stop.
    pub fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue.lock().unwrap().messages.push(message);
    }

    pub fn clear_steering_queue(&self) {
        self.steering_queue.lock().unwrap().messages.clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().unwrap().messages.clear();
    }

    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    pub fn has_queued_messages(&self) -> bool {
        !self.steering_queue.lock().unwrap().messages.is_empty()
            || !self.follow_up_queue.lock().unwrap().messages.is_empty()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// The running prompt's abort signal, if any.
    pub fn signal(&self) -> Option<AbortSignal> {
        self.shared.active.lock().unwrap().clone()
    }

    /// Abort the current run, if one is active. The signal reaches the
    /// provider stream (which ends with `stopReason: "aborted"`) and the tools.
    pub fn abort(&self) {
        if let Some(signal) = self.signal() {
            signal.abort();
        }
    }

    /// `waitForIdle()`: resolves once no prompt is running.
    pub async fn wait_for_idle(&self) {
        loop {
            let idle = self.shared.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.shared.active.lock().unwrap().is_none() {
                return;
            }
            idle.await;
        }
    }

    /// Clear transcript state, runtime state and queued messages.
    pub fn reset(&self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.messages.clear();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
            state.error_message = None;
        }
        self.clear_all_queues();
    }

    /// Start a new prompt. Only fails when a prompt is already running; run
    /// failures end the run with an error assistant message instead.
    pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<(), AgentError> {
        let messages = normalize_prompt_input(input.into());
        let signal = self.begin_run(
            "Agent is already processing a prompt. Use steer() or followUp() to queue messages, or wait for completion.",
        )?;
        self.run_prompt_messages(messages, false, signal).await;
        Ok(())
    }

    /// Continue from the current transcript. The last message must be a user
    /// or tool-result message; after an assistant message, queued steering
    /// (then follow-up) messages start a new run.
    pub async fn r#continue(&self) -> Result<(), AgentError> {
        const BUSY: &str = "Agent is already processing. Wait for completion before continuing.";
        if self.signal().is_some() {
            return Err(AgentError(BUSY.into()));
        }
        let last_role = self
            .shared
            .state
            .lock()
            .unwrap()
            .messages
            .last()
            .map(|m| m.role());
        match last_role {
            None => Err(AgentError("No messages to continue from".into())),
            Some("assistant") => {
                let steering = self.steering_queue.lock().unwrap().drain();
                if !steering.is_empty() {
                    let signal = self.begin_run(BUSY)?;
                    self.run_prompt_messages(steering, true, signal).await;
                    return Ok(());
                }
                let follow_ups = self.follow_up_queue.lock().unwrap().drain();
                if !follow_ups.is_empty() {
                    let signal = self.begin_run(BUSY)?;
                    self.run_prompt_messages(follow_ups, false, signal).await;
                    return Ok(());
                }
                Err(AgentError(
                    "Cannot continue from message role: assistant".into(),
                ))
            }
            Some(_) => {
                let signal = self.begin_run(BUSY)?;
                let config = self.create_loop_config(false, signal.clone());
                let mut context = self.create_context_snapshot();
                let mut emit = self.event_sink();
                let result = run_agent_loop_continue(&mut context, &config, &mut emit).await;
                self.finish(result.err(), &signal);
                Ok(())
            }
        }
    }

    /// Claim the single run slot (`activeRun`) with a fresh abort signal.
    fn begin_run(&self, busy: &str) -> Result<AbortSignal, AgentError> {
        let mut active = self.shared.active.lock().unwrap();
        if active.is_some() {
            return Err(AgentError(busy.to_string()));
        }
        let signal = AbortSignal::new();
        *active = Some(signal.clone());
        drop(active);
        let mut state = self.shared.state.lock().unwrap();
        state.is_streaming = true;
        state.streaming_message = None;
        state.error_message = None;
        Ok(signal)
    }

    async fn run_prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        skip_initial_steering_poll: bool,
        signal: AbortSignal,
    ) {
        let config = self.create_loop_config(skip_initial_steering_poll, signal.clone());
        let context = self.create_context_snapshot();
        let mut emit = self.event_sink();
        let result = run_agent_loop(messages, context, &config, &mut emit).await;
        self.finish(result.err(), &signal);
    }

    /// `handleRunFailure` (for a failed run) then `finishRun`.
    fn finish(&self, error: Option<BoxError>, signal: &AbortSignal) {
        if let Some(error) = error {
            let model = self.shared.state.lock().unwrap().model.clone();
            let failure = AssistantMessage {
                content: vec![Content::text("")],
                api: model.api,
                provider: model.provider,
                model: model.id,
                usage: Default::default(),
                stop_reason: if signal.aborted() {
                    StopReason::Aborted
                } else {
                    StopReason::Error
                },
                error_message: Some(error.to_string()),
                timestamp: ai_types::now_ms(),
                ..Default::default()
            };
            let message = AgentMessage::Assistant(failure.clone());
            self.shared.process_event(AgentEvent::MessageStart {
                message: message.clone(),
            });
            self.shared.process_event(AgentEvent::MessageEnd {
                message: message.clone(),
            });
            self.shared.process_event(AgentEvent::TurnEnd {
                message: failure,
                tool_results: vec![],
            });
            self.shared.process_event(AgentEvent::AgentEnd {
                messages: vec![message],
            });
        }
        {
            let mut state = self.shared.state.lock().unwrap();
            state.is_streaming = false;
            state.streaming_message = None;
            state.pending_tool_calls.clear();
        }
        self.release_run();
    }

    fn release_run(&self) {
        *self.shared.active.lock().unwrap() = None;
        {
            let mut state = self.shared.state.lock().unwrap();
            state.is_streaming = false;
        }
        self.shared.idle.notify_waiters();
    }

    fn event_sink(&self) -> AgentEventSink {
        let shared = self.shared.clone();
        Box::new(move |event| shared.process_event(event))
    }

    fn create_context_snapshot(&self) -> AgentContext {
        let state = self.shared.state.lock().unwrap();
        AgentContext::new_with_tools(
            state.system_prompt.clone(),
            state.messages.clone(),
            state.tools.clone(),
        )
    }

    fn create_loop_config(
        &self,
        skip_initial_steering_poll: bool,
        signal: AbortSignal,
    ) -> AgentLoopConfig {
        let (model, thinking_level) = {
            let state = self.shared.state.lock().unwrap();
            (state.model.clone(), state.thinking_level.clone())
        };
        let settings = self.settings.lock().unwrap().clone();
        let hooks = self.shared.hooks.lock().unwrap().clone();

        let mut config = AgentLoopConfig::new(model);
        config.reasoning = (thinking_level != ThinkingLevel::Off).then_some(thinking_level);
        config.session_id = settings.session_id;
        config.transport = settings.transport;
        config.thinking_budgets = settings.thinking_budgets;
        config.thinking_display = settings.thinking_display;
        config.max_retry_delay_ms = settings.max_retry_delay_ms;
        config.tool_execution = settings.tool_execution;
        config.on_payload = settings.on_payload;
        config.on_response = settings.on_response;
        config.api_key = settings.api_key;
        config.signal = Some(signal);
        config.permission_gate = hooks.permission_gate;
        config.on_background_task_count_change = hooks.on_background_task_count_change;
        if let Some(stream_fn) = hooks.stream_fn {
            config.stream_fn = Some(Box::new(move |m, c, o| stream_fn(m, c, o)));
        }
        config.convert_to_llm = Some(match hooks.convert_to_llm {
            Some(convert) => Box::new(move |messages| convert(messages)),
            None => Box::new(default_convert_to_llm),
        });
        if let Some(transform) = hooks.transform_context {
            config.transform_context = Some(Box::new(move |m, s| transform(m, s)));
        }
        if let Some(get) = hooks.get_api_key {
            config.get_api_key = Some(Box::new(move |provider| get(provider)));
        }
        if let Some(before) = hooks.before_tool_call {
            config.before_tool_call = Some(Box::new(move |ctx, s| before(ctx, s)));
        }
        if let Some(after) = hooks.after_tool_call {
            config.after_tool_call = Some(Box::new(move |ctx, s| after(ctx, s)));
        }
        if let Some(make) = hooks.create_background_result_message {
            config.create_background_result_message = Some(Box::new(move |r| make(r)));
        }
        if let Some(make) = hooks.create_background_placeholder {
            config.create_background_placeholder = Some(Box::new(move |tc| make(tc)));
        }
        // Always provide the hook so a late assignment reaches this run.
        let shared = self.shared.clone();
        config.prepare_next_turn = Some(Box::new(move |turn| {
            let hook = shared.hooks.lock().unwrap().prepare_next_turn.clone();
            let signal = shared.active.lock().unwrap().clone();
            match hook {
                Some(hook) => hook(turn, signal),
                None => Ok(None),
            }
        }));
        let steering = self.steering_queue.clone();
        let skip = Mutex::new(skip_initial_steering_poll);
        config.get_steering_messages = Some(Box::new(move || {
            if std::mem::take(&mut *skip.lock().unwrap()) {
                return Ok(vec![]);
            }
            Ok(steering.lock().unwrap().drain())
        }));
        let follow_ups = self.follow_up_queue.clone();
        config.get_follow_up_messages =
            Some(Box::new(move || Ok(follow_ups.lock().unwrap().drain())));
        config
    }
}

fn normalize_prompt_input(input: PromptInput) -> Vec<AgentMessage> {
    match input {
        PromptInput::Messages(messages) => messages,
        PromptInput::Text { text, images } => {
            let mut content = vec![Content::text(text)];
            content.extend(images.into_iter().map(Content::Image));
            vec![AgentMessage::User(UserMessage {
                content: content.into(),
                timestamp: ai_types::now_ms(),
            })]
        }
    }
}

// ---------------------------------------------------------------------------
// AgentOptions
// ---------------------------------------------------------------------------

/// Options for constructing an `Agent`.
#[derive(Default)]
pub struct AgentOptions {
    pub initial_state: Option<AgentState>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub stream_fn: Option<SharedStreamFn>,
    pub get_api_key: Option<GetApiKeyFn>,
    pub before_tool_call: Option<BeforeToolCallFn>,
    pub after_tool_call: Option<AfterToolCallFn>,
    pub prepare_next_turn: Option<PrepareNextTurnFn>,
    pub create_background_result_message: Option<BackgroundResultMessageFn>,
    pub create_background_placeholder: Option<BackgroundPlaceholderFn>,
    pub on_background_task_count_change: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub session_id: Option<String>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub thinking_display: Option<ThinkingDisplay>,
    pub transport: Option<Transport>,
    pub max_retry_delay_ms: Option<u64>,
    pub tool_execution: Option<ToolExecutionMode>,
    /// `onPayload`, passed to every provider request.
    pub on_payload: Option<cortexcode_ai_types::OnPayload>,
    /// `onResponse`, passed to every provider request.
    pub on_response: Option<cortexcode_ai_types::OnResponse>,
    /// cortex: key used when `get_api_key` yields none.
    pub api_key: Option<String>,
    /// cortex: gate consulted before `before_tool_call`.
    pub permission_gate: Option<Arc<dyn PermissionGate>>,
}

// ---------------------------------------------------------------------------
// PromptInput
// ---------------------------------------------------------------------------

/// Input to [`Agent::prompt`]: messages, or text with optional images.
pub enum PromptInput {
    Messages(Vec<AgentMessage>),
    Text {
        text: String,
        images: Vec<ImageContent>,
    },
}

impl PromptInput {
    pub fn text(text: impl Into<String>) -> Self {
        PromptInput::Text {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

impl From<String> for PromptInput {
    fn from(s: String) -> Self {
        PromptInput::text(s)
    }
}

impl From<&str> for PromptInput {
    fn from(s: &str) -> Self {
        PromptInput::text(s)
    }
}

impl From<Vec<AgentMessage>> for PromptInput {
    fn from(msgs: Vec<AgentMessage>) -> Self {
        PromptInput::Messages(msgs)
    }
}

impl From<AgentMessage> for PromptInput {
    fn from(msg: AgentMessage) -> Self {
        PromptInput::Messages(vec![msg])
    }
}

#[cfg(test)]
mod tests;

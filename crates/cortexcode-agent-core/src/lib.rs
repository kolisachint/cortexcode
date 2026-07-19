//! Core agent runtime for cortex agents.
//!
//! This crate provides the `Agent` struct — a stateful wrapper around the
//! low-level agent loop — plus the loop functions themselves. It mirrors the
//! TypeScript `@kolisachint/hoocode-agent-core` package.

mod agent_loop;
pub mod types;

use agent_loop::{default_convert_to_llm, run_agent_loop, run_agent_loop_continue, AgentEventSink};
use cortexcode_ai_types::{self as ai_types, Content, Message, Model, TextContent, ThinkingLevel};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use types::*;

// ---------------------------------------------------------------------------
// PendingMessageQueue
// ---------------------------------------------------------------------------

/// Controls how queued messages are drained.
#[derive(Debug, Clone, PartialEq)]
pub enum QueueMode {
    /// All queued messages are drained at once.
    All,
    /// One message is drained at a time.
    OneAtATime,
}

impl Default for QueueMode {
    fn default() -> Self {
        QueueMode::OneAtATime
    }
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

    fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        if self.mode == QueueMode::All {
            std::mem::take(&mut self.messages)
        } else {
            self.messages.drain(..1).collect()
        }
    }

    fn clear(&mut self) {
        self.messages.clear();
    }
}

// ---------------------------------------------------------------------------
// Default model
// ---------------------------------------------------------------------------

fn default_model() -> Model {
    Model {
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

// ---------------------------------------------------------------------------
// Internal mutable state
// ---------------------------------------------------------------------------

struct InnerState {
    system_prompt: String,
    model: Model,
    thinking_level: ThinkingLevel,
    tools: AgentTools,
    messages: Vec<AgentMessage>,
    is_streaming: bool,
    streaming_message: Option<AgentMessage>,
    pending_tool_calls: HashSet<String>,
    error_message: Option<String>,
}

impl InnerState {
    fn new(initial: Option<AgentState>) -> Self {
        let state = initial.unwrap_or(AgentState {
            system_prompt: String::new(),
            model: default_model(),
            thinking_level: ThinkingLevel::Off,
            tools: AgentTools::new(vec![]),
            messages: vec![],
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        });

        Self {
            system_prompt: state.system_prompt,
            model: state.model,
            thinking_level: state.thinking_level,
            tools: state.tools,
            messages: state.messages,
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
        }
    }

    fn snapshot(&self) -> AgentState {
        AgentState {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            thinking_level: self.thinking_level.clone(),
            tools: self.tools.clone(),
            messages: self.messages.clone(),
            is_streaming: self.is_streaming,
            streaming_message: self.streaming_message.clone(),
            pending_tool_calls: self.pending_tool_calls.clone(),
            error_message: self.error_message.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// Stateful wrapper around the low-level agent loop.
///
/// `Agent` owns the current transcript, emits lifecycle events, executes tools,
/// and exposes queueing APIs for steering and follow-up messages.
pub struct Agent {
    inner: Arc<Mutex<InnerState>>,
    listeners: Arc<Mutex<Vec<Box<dyn Fn(AgentEvent) + Send>>>>,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    /// A flag used to signal that the agent should stop.
    stop_requested: Arc<std::sync::atomic::AtomicBool>,
}

impl Agent {
    /// Create a new Agent with default configuration.
    pub fn new() -> Self {
        Self::with_options(AgentOptions::default())
    }

    /// Create a new Agent with the given options.
    pub fn with_options(options: AgentOptions) -> Self {
        let inner = Arc::new(Mutex::new(InnerState::new(options.initial_state)));

        Agent {
            inner,
            listeners: Arc::new(Mutex::new(Vec::new())),
            steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.steering_mode.unwrap_or_default(),
            ))),
            follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.follow_up_mode.unwrap_or_default(),
            ))),
            stop_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Read the current agent state.
    pub fn state(&self) -> AgentState {
        self.inner.lock().unwrap().snapshot()
    }

    /// Subscribe to agent lifecycle events.
    ///
    /// Returns a handle that removes the listener when dropped.
    pub fn subscribe<F>(&self, listener: F)
    where
        F: Fn(AgentEvent) + Send + 'static,
    {
        self.listeners.lock().unwrap().push(Box::new(listener));
    }

    #[allow(dead_code)]
    /// Emit an event to all subscribed listeners.
    fn emit(&self, event: AgentEvent) {
        let listeners = self.listeners.lock().unwrap();
        for listener in listeners.iter() {
            listener(event.clone());
        }
    }

    // -----------------------------------------------------------------------
    // Queueing
    // -----------------------------------------------------------------------

    /// Queue a message to be injected after the current assistant turn finishes.
    pub fn steer(&self, message: AgentMessage) {
        self.steering_queue.lock().unwrap().enqueue(message);
    }

    /// Queue a message to run only after the agent would otherwise stop.
    pub fn follow_up(&self, message: AgentMessage) {
        self.follow_up_queue.lock().unwrap().enqueue(message);
    }

    /// Remove all queued steering messages.
    pub fn clear_steering_queue(&self) {
        self.steering_queue.lock().unwrap().clear();
    }

    /// Remove all queued follow-up messages.
    pub fn clear_follow_up_queue(&self) {
        self.follow_up_queue.lock().unwrap().clear();
    }

    /// Remove all queued messages.
    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    /// Returns true when either queue still contains pending messages.
    pub fn has_queued_messages(&self) -> bool {
        self.steering_queue.lock().unwrap().has_items()
            || self.follow_up_queue.lock().unwrap().has_items()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Abort the current run, if one is active.
    pub fn abort(&self) {
        self.stop_requested.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Clear transcript state and queued messages.
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.messages.clear();
        inner.is_streaming = false;
        inner.streaming_message = None;
        inner.pending_tool_calls.clear();
        inner.error_message = None;
        self.clear_all_queues();
    }

    /// Start a new prompt with one or more messages, or from text.
    ///
    /// This is a synchronous call that processes the prompt and returns when done.
    /// Events are emitted to subscribed listeners during processing.
    pub fn prompt(&self, input: PromptInput) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let messages = self.normalize_prompt_input(input);
        let context = {
            let inner = self.inner.lock().unwrap();
            AgentContext::new_with_tools(
                inner.system_prompt.clone(),
                inner.messages.clone(),
                inner.tools.clone(),
            )
        };

        let config = self.build_loop_config()?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
            inner.streaming_message = None;
            inner.error_message = None;
        }

        // Build the event sink
        let mut emit: AgentEventSink = Box::new(|_event| {});

        let result = run_agent_loop(messages, context, config, &mut emit)?;

        // Store result messages
        {
            let mut inner = self.inner.lock().unwrap();
            inner.messages = result.clone();
            inner.is_streaming = false;
            inner.streaming_message = None;
            inner.error_message = None;
        }

        Ok(result)
    }

    /// Continue from the current transcript.
    pub fn r#continue(&self) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let context = {
            let inner = self.inner.lock().unwrap();
            AgentContext::new_with_tools(
                inner.system_prompt.clone(),
                inner.messages.clone(),
                inner.tools.clone(),
            )
        };

        // Check last message
        if let Some(last) = context.messages.last() {
            match &last.inner {
                AgentMessageInner::Standard(Message::Assistant(_)) => {
                    // Try steering/follow-up messages first
                    let steering = self.steering_queue.lock().unwrap().drain();
                    if !steering.is_empty() {
                        return self.run_prompt_messages(steering, true);
                    }
                    let follow_ups = self.follow_up_queue.lock().unwrap().drain();
                    if !follow_ups.is_empty() {
                        return self.run_prompt_messages(follow_ups, false);
                    }
                    return Err("Cannot continue from message role: assistant".into());
                }
                _ => {}
            }
        } else {
            return Err("No messages to continue from".into());
        }

        let config = self.build_loop_config()?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
        }

        let mut context_mut = context;
        let mut emit: AgentEventSink = Box::new(|_event| {});

        let result = run_agent_loop_continue(&mut context_mut, &config, &mut emit)?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.messages = context_mut.messages;
            inner.is_streaming = false;
        }

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn normalize_prompt_input(&self, input: PromptInput) -> Vec<AgentMessage> {
        match input {
            PromptInput::Messages(msgs) => msgs,
            PromptInput::Text(text) => {
                vec![AgentMessage::new(AgentMessageInner::Custom {
                    role: "user".into(),
                    content: vec![Content::Text(TextContent {
                        text,
                        cache_control: None,
                    })],
                    timestamp: None,
                })]
            }
        }
    }

    fn run_prompt_messages(
        &self,
        messages: Vec<AgentMessage>,
        _skip_initial_steering: bool,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let context = {
            let inner = self.inner.lock().unwrap();
            AgentContext::new_with_tools(
                inner.system_prompt.clone(),
                inner.messages.clone(),
                inner.tools.clone(),
            )
        };

        let config = self.build_loop_config()?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
        }

        let mut emit: AgentEventSink = Box::new(|_event| {});
        let result = run_agent_loop(messages, context, config, &mut emit)?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.messages = result.clone();
            inner.is_streaming = false;
        }

        Ok(result)
    }

    fn build_loop_config(&self) -> Result<AgentLoopConfig, Box<dyn std::error::Error + Send + Sync>> {
        let inner = self.inner.lock().unwrap();

        Ok(AgentLoopConfig {
            model: inner.model.clone(),
            reasoning: if inner.thinking_level == ThinkingLevel::Off {
                None
            } else {
                Some(inner.thinking_level.clone())
            },
            convert_to_llm: Some(Box::new(default_convert_to_llm)),
            transform_context: None,
            get_api_key: None,
            should_stop_after_turn: None,
            prepare_next_turn: None,
            get_steering_messages: {
                let queue = self.steering_queue.clone();
                Some(Box::new(move || Ok(queue.lock().unwrap().drain())))
            },
            get_follow_up_messages: {
                let queue = self.follow_up_queue.clone();
                Some(Box::new(move || Ok(queue.lock().unwrap().drain())))
            },
            create_background_result_message: None,
            create_background_placeholder: None,
            on_background_task_count_change: None,
            before_tool_call: None,
            after_tool_call: None,
            stream_fn: None,
            tool_execution: ToolExecutionMode::Parallel,
            signal: None,
            api_key: None,
            session_id: None,
            max_retry_delay_ms: None,
            thinking_budgets: None,
            thinking_display: None,
            transport: None,
            on_payload: None,
            on_response: None,
            cache_control_format: None,
            send_session_affinity_headers: None,
            supports_long_cache_retention: None,
            prompt_suffix: None,
        })
    }
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// AgentOptions
// ---------------------------------------------------------------------------

/// Options for constructing an `Agent`.
#[derive(Default)]
pub struct AgentOptions {
    pub initial_state: Option<AgentState>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
}

// ---------------------------------------------------------------------------
// PromptInput
// ---------------------------------------------------------------------------

/// Input to the `Agent::prompt` method.
pub enum PromptInput {
    /// A batch of agent messages.
    Messages(Vec<AgentMessage>),
    /// Plain text, converted to a user message.
    Text(String),
}

impl From<String> for PromptInput {
    fn from(s: String) -> Self {
        PromptInput::Text(s)
    }
}

impl From<&str> for PromptInput {
    fn from(s: &str) -> Self {
        PromptInput::Text(s.to_string())
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

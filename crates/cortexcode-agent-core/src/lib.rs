//! Core agent runtime for cortex agents.
//!
//! This crate provides the `Agent` struct — a stateful wrapper around the
//! low-level agent loop — plus the loop functions themselves. It mirrors the
//! TypeScript `@kolisachint/hoocode-agent-core` package.

pub mod types;

use cortexcode_agent_loop::{
    default_convert_to_llm, run_agent_loop, run_agent_loop_continue, AgentEventSink,
};
use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{self as ai_types, Model, SimpleStreamOptions, ThinkingLevel};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use types::*;

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Function type for creating an AI stream for a given model, context, and options.
pub type StreamFn = Box<
    dyn Fn(
            Model,
            ai_types::Context,
            SimpleStreamOptions,
        ) -> Result<AssistantMessageEventStream, Box<dyn std::error::Error + Send + Sync>>
        + Send
        + Sync,
>;

/// Arc-wrapped stream function.
pub type SharedStreamFn = Arc<StreamFn>;

// ---------------------------------------------------------------------------
// PendingMessageQueue
// ---------------------------------------------------------------------------

/// Controls how queued messages are drained.
#[derive(Debug, Clone, PartialEq, Default)]
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

    fn enqueue(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    fn has_items(&self) -> bool {
        !self.messages.is_empty()
    }

    fn drain(&mut self) -> Vec<AgentMessage> {
        if self.mode == QueueMode::All {
            std::mem::take(&mut self.messages)
        } else if self.messages.is_empty() {
            Vec::new()
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

// ---------------------------------------------------------------------------
// Internal mutable state
// ---------------------------------------------------------------------------

struct InnerState {
    system_prompt: String,
    model: Model,
    api_key: Option<String>,
    thinking_level: ThinkingLevel,
    tools: AgentTools,
    messages: Vec<AgentMessage>,
    is_streaming: bool,
    streaming_message: Option<AgentMessage>,
    pending_tool_calls: HashSet<String>,
    error_message: Option<String>,
}

impl InnerState {
    fn new(initial: Option<AgentState>, api_key: Option<String>) -> Self {
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
            api_key,
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
    #[allow(clippy::type_complexity)]
    listeners: Arc<Mutex<Vec<(usize, Box<dyn Fn(AgentEvent) + Send>)>>>,
    next_listener_id: Arc<AtomicUsize>,
    steering_queue: Arc<Mutex<PendingMessageQueue>>,
    follow_up_queue: Arc<Mutex<PendingMessageQueue>>,
    /// The running prompt's abort signal (agent.ts `abortController`).
    active_signal: Arc<Mutex<Option<ai_types::AbortSignal>>>,
    permission_gate: Option<Arc<dyn PermissionGate>>,
    stream_fn: Option<SharedStreamFn>,
}

/// Handle returned by [`Agent::subscribe`]. Removes the listener when dropped.
pub struct Subscription {
    id: usize,
    #[allow(clippy::type_complexity)]
    listeners: Arc<Mutex<Vec<(usize, Box<dyn Fn(AgentEvent) + Send>)>>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut listeners = self.listeners.lock().unwrap();
        listeners.retain(|(id, _)| *id != self.id);
    }
}

impl Agent {
    /// Create a new Agent with default configuration.
    pub fn new() -> Self {
        Self::with_options(AgentOptions::default())
    }

    /// Create a new Agent with the given options.
    pub fn with_options(options: AgentOptions) -> Self {
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(Mutex::new(InnerState::new(
            options.initial_state,
            options.api_key,
        )));

        Agent {
            inner,
            listeners: Arc::new(Mutex::new(Vec::new())),
            steering_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.steering_mode.unwrap_or_default(),
            ))),
            follow_up_queue: Arc::new(Mutex::new(PendingMessageQueue::new(
                options.follow_up_mode.unwrap_or_default(),
            ))),
            active_signal: Arc::new(Mutex::new(None)),
            next_listener_id: Arc::new(AtomicUsize::new(1)),
            permission_gate: options.permission_gate,
            stream_fn: options.stream_fn.map(|b| b as _),
        }
    }

    /// Read the current agent state.
    pub fn state(&self) -> AgentState {
        self.inner.lock().unwrap().snapshot()
    }

    /// Subscribe to agent lifecycle events.
    ///
    /// Returns a handle that removes the listener when dropped.
    pub fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(AgentEvent) + Send + 'static,
    {
        let id = self.next_listener_id.fetch_add(1, Ordering::SeqCst);
        self.listeners
            .lock()
            .unwrap()
            .push((id, Box::new(listener)));
        Subscription {
            id,
            listeners: Arc::clone(&self.listeners),
        }
    }

    fn event_sink(&self) -> AgentEventSink {
        let listeners = Arc::clone(&self.listeners);
        Box::new(move |event| {
            let listeners = listeners.lock().unwrap();
            for (_, listener) in listeners.iter() {
                listener(event.clone());
            }
        })
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

    /// Abort the current run, if one is active. The signal reaches the
    /// provider stream (which ends with `stopReason: "aborted"`) and the tools.
    pub fn abort(&self) {
        if let Some(signal) = self.active_signal.lock().unwrap().as_ref() {
            signal.abort();
        }
    }

    /// A fresh abort signal for a run, registered so [`Agent::abort`] reaches it.
    fn begin_run(&self) -> ai_types::AbortSignal {
        let signal = ai_types::AbortSignal::new();
        *self.active_signal.lock().unwrap() = Some(signal.clone());
        signal
    }

    fn end_run(&self) {
        *self.active_signal.lock().unwrap() = None;
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
    /// Resolves when the run is done. Events are emitted to subscribed
    /// listeners during processing.
    pub async fn prompt(
        &self,
        input: PromptInput,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let messages = self.normalize_prompt_input(input);
        let context = {
            let inner = self.inner.lock().unwrap();
            AgentContext::new_with_tools(
                inner.system_prompt.clone(),
                inner.messages.clone(),
                inner.tools.clone(),
            )
        };

        let config = self.build_loop_config(self.begin_run())?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
            inner.streaming_message = None;
            inner.error_message = None;
        }

        // Build the event sink
        let mut emit = self.event_sink();

        let result = run_agent_loop(messages, context, config, &mut emit).await;
        self.end_run();
        let result = result?;

        // Append this run's messages to the transcript (agent.ts pushes each
        // message on `message_end`; the transcript is never replaced).
        {
            let mut inner = self.inner.lock().unwrap();
            inner.messages.extend(result.iter().cloned());
            inner.is_streaming = false;
            inner.streaming_message = None;
            inner.error_message = None;
        }

        Ok(result)
    }

    /// Continue from the current transcript.
    pub async fn r#continue(
        &self,
    ) -> Result<Vec<AgentMessage>, Box<dyn std::error::Error + Send + Sync>> {
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
            if let AgentMessage::Assistant(_) = last {
                // Try steering/follow-up messages first
                let steering = self.steering_queue.lock().unwrap().drain();
                if !steering.is_empty() {
                    return self.run_prompt_messages(steering, true).await;
                }
                let follow_ups = self.follow_up_queue.lock().unwrap().drain();
                if !follow_ups.is_empty() {
                    return self.run_prompt_messages(follow_ups, false).await;
                }
                return Err("Cannot continue from message role: assistant".into());
            }
        } else {
            return Err("No messages to continue from".into());
        }

        let config = self.build_loop_config(self.begin_run())?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
        }

        let mut context_mut = context;
        let mut emit = self.event_sink();

        let result = run_agent_loop_continue(&mut context_mut, &config, &mut emit).await;
        self.end_run();
        let result = result?;

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
                vec![AgentMessage::user_text(text)]
            }
        }
    }

    async fn run_prompt_messages(
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

        let config = self.build_loop_config(self.begin_run())?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.is_streaming = true;
        }

        let mut emit = self.event_sink();
        let result = run_agent_loop(messages, context, config, &mut emit).await;
        self.end_run();
        let result = result?;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.messages.extend(result.iter().cloned());
            inner.is_streaming = false;
        }

        Ok(result)
    }

    fn build_loop_config(
        &self,
        signal: ai_types::AbortSignal,
    ) -> Result<AgentLoopConfig, Box<dyn std::error::Error + Send + Sync>> {
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
            permission_gate: self.permission_gate.clone(),
            stream_fn: self
                .stream_fn
                .clone()
                .map(|a| -> StreamFn { Box::new(move |m, c, o| (*a)(m, c, o)) }),
            tool_execution: ToolExecutionMode::Parallel,
            signal: Some(signal),
            api_key: inner.api_key.clone(),
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
    pub api_key: Option<String>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
    pub permission_gate: Option<Arc<dyn PermissionGate>>,
    pub stream_fn: Option<SharedStreamFn>,
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

#[cfg(test)]
mod transcript_tests {
    use super::*;
    use cortexcode_ai_provider_faux::{
        faux_message, faux_text_message, faux_tool_call, FauxProvider, FauxResponseStep,
    };
    use cortexcode_ai_types::Content;

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

    #[tokio::test]
    async fn prompt_appends_to_the_transcript() {
        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(vec![
            FauxResponseStep::Message(faux_text_message("one", None)),
            FauxResponseStep::Message(faux_text_message("two", None)),
        ]);
        let agent = Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: String::new(),
                model: model(),
                thinking_level: ThinkingLevel::Off,
                tools: cortexcode_agent_types::AgentTools::new(Vec::new()),
                messages: Vec::new(),
                is_streaming: false,
                streaming_message: None,
                pending_tool_calls: Default::default(),
                error_message: None,
            }),
            stream_fn: Some(Arc::new(faux.stream_fn())),
            ..Default::default()
        });
        agent.prompt(PromptInput::Text("a".into())).await.unwrap();
        let second = agent.prompt(PromptInput::Text("b".into())).await.unwrap();
        // The returned messages are only this run's; the transcript keeps both runs.
        assert_eq!(second.len(), 2);
        let roles: Vec<&str> = agent
            .state()
            .messages
            .iter()
            .map(|m| match m {
                AgentMessage::User(_) => "user",
                AgentMessage::Assistant(_) => "assistant",
                _ => "other",
            })
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
    }

    fn agent_with_tools(
        faux: &Arc<FauxProvider>,
        tools: Vec<cortexcode_agent_types::AgentTool>,
    ) -> Agent {
        Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: String::new(),
                model: model(),
                thinking_level: ThinkingLevel::Off,
                tools: cortexcode_agent_types::AgentTools::new(tools),
                messages: Vec::new(),
                is_streaming: false,
                streaming_message: None,
                pending_tool_calls: Default::default(),
                error_message: None,
            }),
            stream_fn: Some(Arc::new(faux.stream_fn())),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn tool_results_match_hoocode_messages_and_events() {
        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(vec![
            FauxResponseStep::Message(faux_message(
                vec![
                    faux_tool_call("fails", serde_json::json!({}), Some("c1".into())),
                    faux_tool_call("detailed", serde_json::json!({}), Some("c2".into())),
                    faux_tool_call("missing", serde_json::json!({}), Some("c3".into())),
                ],
                Some(cortexcode_ai_types::StopReason::ToolUse),
                None,
            )),
            FauxResponseStep::Message(faux_text_message("done", None)),
        ]);
        let fails = cortexcode_agent_types::AgentTool::new(
            "fails",
            "",
            serde_json::json!({"type": "object"}),
            Box::new(|_, _, _, _| Err("ENOENT: no such file or directory, access '/x'".into())),
        );
        let detailed = cortexcode_agent_types::AgentTool::new(
            "detailed",
            "",
            serde_json::json!({"type": "object"}),
            Box::new(|_, _, _, _| {
                Ok(cortexcode_agent_types::AgentToolResult {
                    content: vec![Content::text("ok")],
                    details: serde_json::json!({"k": 1}),
                    terminate: false,
                })
            }),
        );
        let agent = agent_with_tools(&faux, vec![fails, detailed]);
        let ended = Arc::new(Mutex::new(Vec::new()));
        let sink = ended.clone();
        let _sub = agent.subscribe(move |event| {
            if let AgentEvent::MessageEnd {
                message: AgentMessage::ToolResult(m),
            } = event
            {
                sink.lock().unwrap().push(m.tool_call_id.clone());
            }
        });
        agent.prompt(PromptInput::Text("go".into())).await.unwrap();

        let results: Vec<_> = agent
            .state()
            .messages
            .into_iter()
            .filter_map(|m| match m {
                AgentMessage::ToolResult(r) => Some(r),
                _ => None,
            })
            .collect();
        let text = |r: &cortexcode_ai_types::ToolResultMessage| match &r.content[0] {
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

    /// `agent.abort()` mid-stream: the signal reaches the provider, which ends
    /// the assistant message with `stopReason: "aborted"` and what streamed so far.
    #[tokio::test(flavor = "multi_thread")]
    async fn abort_reaches_the_provider_stream() {
        let head = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial \"}}]}\n\n";
        let base_url = cortexcode_ai_stream::testing::serve_sse_then_hang(
            head,
            std::time::Duration::from_secs(30),
        );
        let mut m = model();
        m.api = "openai-completions".into();
        m.provider = "openai".into();
        m.base_url = base_url;
        let agent = Arc::new(Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: String::new(),
                model: m,
                thinking_level: ThinkingLevel::Off,
                tools: cortexcode_agent_types::AgentTools::new(Vec::new()),
                messages: Vec::new(),
                is_streaming: false,
                streaming_message: None,
                pending_tool_calls: Default::default(),
                error_message: None,
            }),
            api_key: Some("sk-test".into()),
            stream_fn: Some(Arc::new(Box::new(cortexcode_ai_provider_openai::stream))),
            ..Default::default()
        }));
        let weak = Arc::downgrade(&agent);
        let _sub = agent.subscribe(move |event| {
            if let AgentEvent::MessageUpdate { .. } = event {
                if let Some(agent) = weak.upgrade() {
                    agent.abort();
                }
            }
        });

        let messages = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            agent.prompt(PromptInput::Text("go".into())),
        )
        .await
        .expect("abort must end the run without waiting for the server")
        .unwrap();

        match messages.last() {
            Some(AgentMessage::Assistant(a)) => {
                assert_eq!(a.stop_reason, cortexcode_ai_types::StopReason::Aborted);
                assert_eq!(a.content, vec![Content::text("partial ")]);
            }
            other => panic!("expected the aborted assistant message, got {other:?}"),
        }
        // The run is over; a later abort is a no-op.
        agent.abort();
        assert!(!agent.state().is_streaming);
    }
}

//! `core/agent-session.ts`: the agent lifecycle shared by every run mode —
//! event subscription with session persistence, prompting and queues, model
//! and thinking-level management, the tool registry and system prompt, and
//! user bash runs.
//!
//! Auto-retry (`retry.rs`), compaction (`compaction.rs`) and tree navigation
//! (`tree.rs`) extend it; `runtime.rs` replaces it for /new, /resume, /fork
//! and /cd. The extension runner (12.3) plugs in later.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use cortexcode_agent_core::{Agent, QueueMode as AgentQueueMode, Subscription};
use cortexcode_agent_types::{
    AgentContext, AgentEvent, AgentLoopTurnUpdate, AgentMessage, AgentTool, AgentTools,
    BashExecutionMessage, CustomMessage,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, Content, ImageContent, Model, TextContent, ThinkingLevel,
    UserContent, UserMessage,
};
use cortexcode_code_models::{AuthLookup, ModelRegistry};
use cortexcode_code_prompts::BuildSystemPromptOptions;
use cortexcode_code_session::SessionManager;
use cortexcode_code_settings::{QueueMode, SettingsManager, ThinkingLevelSetting};
use cortexcode_code_tool_api::{
    wrap_tool_definitions, SessionBranch, ToolContext, ToolContextFactory, ToolDefinition,
};
use cortexcode_code_tool_bash::{
    execute_bash_with_operations, BashExecutorOptions, BashOperations, BashResult,
    LocalBashOperations,
};

use crate::auth_guidance::{format_no_api_key_found_message, format_no_model_selected_message};
use crate::compaction::{CompactionReason, CompactionState};
use crate::hooks::{
    ExtensionError, ExtensionHooks, NoExtensions, ResourceLoader, SessionEvent, SessionStartEvent,
    TemplateKind,
};
use crate::retry::RetryState;
use crate::stats::{self, ContextUsage, ForkableMessage, SessionStats, TranscriptSelection};
use cortexcode_agent_compaction::CompactionResult;

/// `DEFAULT_THINKING_LEVEL`.
pub const DEFAULT_THINKING_LEVEL: ThinkingLevel = ThinkingLevel::Off;

/// `THINKING_LEVELS`: what a session without a model offers.
const THINKING_LEVELS: [ThinkingLevel; 5] = [
    ThinkingLevel::Off,
    ThinkingLevel::Minimal,
    ThinkingLevel::Low,
    ThinkingLevel::Medium,
    ThinkingLevel::High,
];

/// Tools active by default when the built-ins come from the factory.
pub const DEFAULT_ACTIVE_TOOL_NAMES: [&str; 5] =
    ["read", "bash", "edit", "write", "SearchCodebase"];

/// A failed session operation (the TS methods throw `Error(message)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionError(pub String);

impl std::fmt::Display for AgentSessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AgentSessionError {}

type Result<T> = std::result::Result<T, AgentSessionError>;

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(AgentSessionError(message.into()))
}

/// `AgentSessionEvent`: the agent's events plus session-level ones.
// Agent events dominate the stream; boxing them would only add an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum AgentSessionEvent {
    Agent(AgentEvent),
    QueueUpdate {
        steering: Vec<String>,
        follow_up: Vec<String>,
    },
    SessionInfoChanged {
        name: Option<String>,
    },
    ThinkingLevelChanged {
        level: ThinkingLevel,
    },
    CompactionStart {
        reason: CompactionReason,
    },
    CompactionEnd {
        reason: CompactionReason,
        result: Option<CompactionResult>,
        aborted: bool,
        will_retry: bool,
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error_message: String,
    },
    AutoRetryEnd {
        success: bool,
        attempt: u64,
        final_error: Option<String>,
    },
}

type SessionListener = Arc<dyn Fn(&AgentSessionEvent) + Send + Sync>;

/// A model to cycle through (`--models`), with an optional pinned thinking level.
pub use cortexcode_code_models::ScopedModel;

/// `ModelCycleResult`.
#[derive(Debug, Clone)]
pub struct ModelCycleResult {
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    /// Cycling through `--models` rather than every available model.
    pub is_scoped: bool,
}

/// Direction for the cycle methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CycleDirection {
    #[default]
    Forward,
    Backward,
}

/// `streamingBehavior` / `deliverAs` for a message sent while streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingBehavior {
    Steer,
    FollowUp,
}

/// `sendCustomMessage`'s `deliverAs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverAs {
    Steer,
    FollowUp,
    NextTurn,
}

/// `InputSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputSource {
    #[default]
    Interactive,
    Rpc,
    Extension,
}

/// `PromptOptions`.
#[derive(Debug, Clone)]
pub struct PromptOptions {
    /// Expand skill commands, prompt templates and extension commands.
    pub expand_prompt_templates: bool,
    pub images: Vec<ImageContent>,
    /// Required while streaming.
    pub streaming_behavior: Option<StreamingBehavior>,
    pub source: InputSource,
}

impl Default for PromptOptions {
    fn default() -> Self {
        Self {
            expand_prompt_templates: true,
            images: Vec::new(),
            streaming_behavior: None,
            source: InputSource::Interactive,
        }
    }
}

/// Where a tool in the registry came from (`SourceInfo.source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    Builtin,
    Sdk,
}

/// `ToolInfo`.
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub source: ToolSource,
    /// `sourceInfo.path` (`<builtin:read>`, `<sdk:name>`).
    pub source_path: String,
}

/// What the built-in tool factory gets (`_buildRuntime`).
pub struct BaseToolsContext<'a> {
    pub cwd: &'a Path,
    pub settings: &'a SettingsManager,
}

/// Builds the built-in tool definitions (`createAllToolDefinitions`).
pub type BaseToolsFactory = Arc<dyn Fn(&BaseToolsContext<'_>) -> Vec<ToolDefinition> + Send + Sync>;

/// The built-in tools: from a factory, or a fixed override whose tools are
/// all active by default (`baseToolsOverride`).
#[derive(Clone)]
pub enum BaseTools {
    Factory(BaseToolsFactory),
    Override(Vec<ToolDefinition>),
}

/// `AgentSessionConfig`.
pub struct AgentSessionConfig {
    pub agent: Arc<Agent>,
    pub session_manager: SessionManager,
    pub settings: Arc<Mutex<SettingsManager>>,
    pub cwd: PathBuf,
    /// Models to cycle through (`--models`).
    pub scoped_models: Vec<ScopedModel>,
    pub resource_loader: Arc<dyn ResourceLoader>,
    /// SDK tools registered outside extensions.
    pub custom_tools: Vec<ToolDefinition>,
    pub model_registry: Arc<ModelRegistry>,
    /// Stored credentials (auth.json, env, OAuth).
    pub auth: Arc<dyn AuthLookup + Send + Sync>,
    /// Default: [`DEFAULT_ACTIVE_TOOL_NAMES`], or every override tool.
    pub initial_active_tool_names: Option<Vec<String>>,
    /// When set, only these tools are exposed.
    pub allowed_tool_names: Option<Vec<String>>,
    /// Subtracted from whatever is otherwise enabled.
    pub disallowed_tool_names: Option<Vec<String>>,
    pub base_tools: BaseTools,
    pub extensions: Option<Arc<dyn ExtensionHooks>>,
    /// Emitted by [`AgentSession::bind_extensions`]; default `startup`.
    pub session_start_event: Option<SessionStartEvent>,
}

struct DefinitionEntry {
    definition: ToolDefinition,
    source: ToolSource,
}

/// Mutable session state (the TS private fields).
#[derive(Default)]
struct State {
    scoped_models: Vec<ScopedModel>,
    steering_messages: Vec<String>,
    follow_up_messages: Vec<String>,
    pending_next_turn_messages: Vec<CustomMessage>,
    pending_bash_messages: Vec<BashExecutionMessage>,
    base_tool_definitions: Vec<ToolDefinition>,
    tool_registry: Vec<AgentTool>,
    tool_definitions: Vec<(String, DefinitionEntry)>,
    tool_prompt_snippets: HashMap<String, String>,
    tool_prompt_guidelines: HashMap<String, Vec<String>>,
    base_system_prompt: String,
    runtime_context_dirty: bool,
    last_assistant_message: Option<AssistantMessage>,
    bash_abort: Option<AbortSignal>,
}

struct Inner {
    agent: Arc<Agent>,
    session_manager: Arc<Mutex<SessionManager>>,
    settings: Arc<Mutex<SettingsManager>>,
    cwd: PathBuf,
    resource_loader: Arc<dyn ResourceLoader>,
    extensions: Arc<dyn ExtensionHooks>,
    session_start_event: SessionStartEvent,
    custom_tools: Vec<ToolDefinition>,
    model_registry: Arc<ModelRegistry>,
    auth: Arc<dyn AuthLookup + Send + Sync>,
    allowed_tool_names: Option<HashSet<String>>,
    disallowed_tool_names: Option<HashSet<String>>,
    base_tools: BaseTools,
    state: Mutex<State>,
    listeners: Mutex<Vec<(usize, SessionListener)>>,
    next_listener_id: Mutex<usize>,
    agent_subscription: Mutex<Option<Subscription>>,
    retry: Mutex<RetryState>,
    retry_notify: tokio::sync::Notify,
    compaction: Mutex<CompactionState>,
}

/// The session branch as tools see it (`ctx.sessionManager.getBranch()`).
struct BranchView(Arc<Mutex<SessionManager>>);

impl SessionBranch for BranchView {
    fn get_branch(&self) -> Vec<serde_json::Value> {
        let manager = lock(&self.0);
        manager
            .branch(None)
            .into_iter()
            .filter_map(|entry| serde_json::to_value(entry).ok())
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The agent's model, unless it is agent-core's "no model" placeholder.
fn real_model(model: &Model) -> Option<&Model> {
    let placeholder =
        model.id == "unknown" && model.provider == "unknown" && model.api == "unknown";
    (!placeholder).then_some(model)
}

pub(crate) fn thinking_level_str(level: &ThinkingLevel) -> &'static str {
    ThinkingLevelSetting::from(level.clone()).as_str()
}

fn user_text(message: &UserMessage) -> String {
    stats::extract_user_message_text(&message.content)
}

fn user_content(text: &str, images: &[ImageContent]) -> UserContent {
    let mut content = vec![Content::Text(TextContent {
        text: text.to_string(),
        text_signature: None,
    })];
    content.extend(images.iter().cloned().map(Content::Image));
    content.into()
}

fn user_message(text: &str, images: &[ImageContent]) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: user_content(text, images),
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

/// `_normalizePromptSnippet`: one line, collapsed whitespace.
fn normalize_prompt_snippet(text: Option<&str>) -> Option<String> {
    let one_line = text?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!one_line.is_empty()).then_some(one_line)
}

/// `_normalizePromptGuidelines`: trimmed, non-empty, first occurrence kept.
fn normalize_prompt_guidelines(guidelines: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    guidelines
        .iter()
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty() && seen.insert(g.clone()))
        .collect()
}

fn to_agent_queue_mode(mode: QueueMode) -> AgentQueueMode {
    match mode {
        QueueMode::All => AgentQueueMode::All,
        QueueMode::OneAtATime => AgentQueueMode::OneAtATime,
    }
}

fn from_agent_queue_mode(mode: AgentQueueMode) -> QueueMode {
    match mode {
        AgentQueueMode::All => QueueMode::All,
        AgentQueueMode::OneAtATime => QueueMode::OneAtATime,
    }
}

/// `AgentSession`. Cheap to clone; clones share the session.
#[derive(Clone)]
pub struct AgentSession {
    inner: Arc<Inner>,
}

/// Handle from [`AgentSession::subscribe`]; dropping it unsubscribes.
pub struct SessionSubscription {
    id: usize,
    inner: Weak<Inner>,
}

impl Drop for SessionSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            lock(&inner.listeners).retain(|(id, _)| *id != self.id);
        }
    }
}

impl AgentSession {
    pub fn new(config: AgentSessionConfig) -> Self {
        let inner = Arc::new(Inner {
            agent: config.agent,
            session_manager: Arc::new(Mutex::new(config.session_manager)),
            settings: config.settings,
            cwd: config.cwd,
            resource_loader: config.resource_loader,
            extensions: config.extensions.unwrap_or_else(|| Arc::new(NoExtensions)),
            session_start_event: config
                .session_start_event
                .unwrap_or_else(SessionStartEvent::startup),
            custom_tools: config.custom_tools,
            model_registry: config.model_registry,
            auth: config.auth,
            allowed_tool_names: config
                .allowed_tool_names
                .map(|names| names.into_iter().collect()),
            disallowed_tool_names: config
                .disallowed_tool_names
                .filter(|names| !names.is_empty())
                .map(|names| names.into_iter().collect()),
            base_tools: config.base_tools,
            state: Mutex::new(State {
                scoped_models: config.scoped_models,
                ..Default::default()
            }),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: Mutex::new(1),
            agent_subscription: Mutex::new(None),
            retry: Mutex::new(RetryState::default()),
            retry_notify: tokio::sync::Notify::new(),
            compaction: Mutex::new(CompactionState::default()),
        });
        let session = Self { inner };
        session.connect_to_agent();

        // Hand the loop a fresh system prompt and tool list between turns when
        // they changed mid-run (keeps the loop's messages).
        let weak = Arc::downgrade(&session.inner);
        session
            .inner
            .agent
            .set_prepare_next_turn(Some(Arc::new(move |turn, _signal| {
                let Some(inner) = weak.upgrade() else {
                    return Ok(None);
                };
                if !std::mem::take(&mut lock(&inner.state).runtime_context_dirty) {
                    return Ok(None);
                }
                let (system_prompt, tools) = inner
                    .agent
                    .with_state(|s| (s.system_prompt.clone(), s.tools.clone()));
                Ok(Some(AgentLoopTurnUpdate {
                    context: Some(AgentContext {
                        system_prompt,
                        messages: turn.context.messages,
                        tools,
                    }),
                    model: None,
                    thinking_level: None,
                }))
            })));

        session.build_runtime(config.initial_active_tool_names, true);
        session
    }

    // ------------------------------------------------------------------
    // Event subscription
    // ------------------------------------------------------------------

    pub(crate) fn retry_state(&self) -> MutexGuard<'_, RetryState> {
        lock(&self.inner.retry)
    }

    pub(crate) fn retry_notify(&self) -> &tokio::sync::Notify {
        &self.inner.retry_notify
    }

    pub(crate) fn compaction_state(&self) -> MutexGuard<'_, CompactionState> {
        lock(&self.inner.compaction)
    }

    pub(crate) fn auth(&self) -> &Arc<dyn AuthLookup + Send + Sync> {
        &self.inner.auth
    }

    /// Run session work in the background (the TS event queue's async tail).
    pub(crate) fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(future);
            }
            Err(_) => {
                // No runtime (a synchronous caller): run it to completion here.
                let _ = std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map(|rt| rt.block_on(future))
                })
                .join();
            }
        }
    }

    pub(crate) fn connect_to_agent(&self) {
        let mut slot = lock(&self.inner.agent_subscription);
        if slot.is_some() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        *slot = Some(self.inner.agent.subscribe(move |event, _signal| {
            if let Some(inner) = weak.upgrade() {
                AgentSession { inner }.process_agent_event(event);
            }
        }));
    }

    pub(crate) fn disconnect_from_agent(&self) {
        lock(&self.inner.agent_subscription).take();
    }

    pub(crate) fn emit(&self, event: AgentSessionEvent) {
        let listeners: Vec<SessionListener> = lock(&self.inner.listeners)
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(&event);
        }
    }

    fn emit_queue_update(&self) {
        let (steering, follow_up) = {
            let state = lock(&self.inner.state);
            (
                state.steering_messages.clone(),
                state.follow_up_messages.clone(),
            )
        };
        self.emit(AgentSessionEvent::QueueUpdate {
            steering,
            follow_up,
        });
    }

    /// `_processAgentEvent`, run synchronously as the agent emits; the
    /// `agent_end` tail (retry backoff, compaction) runs in the background.
    fn process_agent_event(&self, event: &AgentEvent) {
        if let AgentEvent::AgentEnd { messages } = event {
            self.arm_retry_for_agent_end(messages);
        }
        // A queued user message leaves its queue before listeners see it start.
        if let AgentEvent::MessageStart {
            message: AgentMessage::User(user),
        } = event
        {
            self.reset_overflow_recovery();
            let text = user_text(user);
            if !text.is_empty() {
                let removed = {
                    let mut state = lock(&self.inner.state);
                    if let Some(i) = state.steering_messages.iter().position(|m| *m == text) {
                        state.steering_messages.remove(i);
                        true
                    } else if let Some(i) = state.follow_up_messages.iter().position(|m| *m == text)
                    {
                        state.follow_up_messages.remove(i);
                        true
                    } else {
                        false
                    }
                };
                if removed {
                    self.emit_queue_update();
                }
            }
        }

        self.emit(AgentSessionEvent::Agent(event.clone()));

        if let AgentEvent::MessageEnd { message } = event {
            {
                let mut manager = lock(&self.inner.session_manager);
                match message {
                    AgentMessage::Custom(custom) => {
                        manager.append_custom_message(
                            custom.custom_type.clone(),
                            custom.content.clone(),
                            custom.display,
                            custom.details.clone(),
                        );
                    }
                    AgentMessage::User(_)
                    | AgentMessage::Assistant(_)
                    | AgentMessage::ToolResult(_) => {
                        manager.append_message(message.clone());
                    }
                    // bashExecution, compactionSummary, branchSummary are persisted elsewhere.
                    _ => {}
                }
            }
            if let AgentMessage::Assistant(assistant) = message {
                lock(&self.inner.state).last_assistant_message = Some(assistant.clone());
                if assistant.stop_reason != cortexcode_ai_types::StopReason::Error {
                    self.reset_overflow_recovery();
                    // Provider-exhaustion bookkeeping arrives with subagents (10.9).
                    self.on_successful_assistant_response();
                }
            }
        }

        if let AgentEvent::AgentEnd { .. } = event {
            let last = lock(&self.inner.state).last_assistant_message.take();
            if let Some(message) = last {
                let session = self.clone();
                self.spawn(async move { session.after_agent_end(message).await });
            }
        }
    }

    /// The `agent_end` tail: retry a transient error, else check compaction.
    async fn after_agent_end(&self, message: AssistantMessage) {
        let model = self.model();
        if crate::retry::is_retryable_error(&message, model.as_ref())
            && self.handle_retryable_error(&message).await
        {
            return;
        }
        self.resolve_retry();
        self.check_compaction(&message, true).await;
    }

    /// Subscribe to session events. Persistence is internal (messages are
    /// saved on `message_end`). Dropping the handle unsubscribes.
    pub fn subscribe<F>(&self, listener: F) -> SessionSubscription
    where
        F: Fn(&AgentSessionEvent) + Send + Sync + 'static,
    {
        let id = {
            let mut next = lock(&self.inner.next_listener_id);
            let id = *next;
            *next += 1;
            id
        };
        lock(&self.inner.listeners).push((id, Arc::new(listener)));
        SessionSubscription {
            id,
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// `dispose()`: drop listeners, disconnect from the agent and release the
    /// session's provider resources.
    pub fn dispose(&self) {
        self.disconnect_from_agent();
        lock(&self.inner.listeners).clear();
        let _ = cortexcode_ai_registry::session_resources::cleanup_session_resources(Some(
            &self.session_id(),
        ));
    }

    // ------------------------------------------------------------------
    // Read-only state
    // ------------------------------------------------------------------

    pub fn agent(&self) -> &Arc<Agent> {
        &self.inner.agent
    }

    /// `extensionRunner`: the extension hooks this session emits to.
    pub fn extensions(&self) -> &Arc<dyn ExtensionHooks> {
        &self.inner.extensions
    }

    /// `reload()`: re-read settings and resources and rebuild the tool
    /// registry and system prompt, keeping the active tools. (The
    /// `session_start` reload event needs extension bindings, 12.3.)
    pub async fn reload(&self) {
        if self.inner.extensions.has_handlers("session_shutdown") {
            self.inner
                .extensions
                .emit_session_event(SessionEvent::Shutdown {
                    reason: crate::hooks::SessionShutdownReason::Reload,
                    target_session_file: None,
                })
                .await;
        }
        self.settings().reload();
        self.inner.resource_loader.reload();
        self.build_runtime(Some(self.get_active_tool_names()), true);
    }

    /// `bindExtensions()`: emit this session's `session_start` event (UI and
    /// command bindings arrive with the extension runtime, 12.3).
    pub async fn bind_extensions(&self) {
        self.inner
            .extensions
            .emit_session_event(SessionEvent::Start(self.inner.session_start_event.clone()))
            .await;
    }

    /// The session manager (hold the guard briefly).
    pub fn session_manager(&self) -> MutexGuard<'_, SessionManager> {
        lock(&self.inner.session_manager)
    }

    /// The settings manager (hold the guard briefly).
    pub fn settings(&self) -> MutexGuard<'_, SettingsManager> {
        lock(&self.inner.settings)
    }

    pub fn model_registry(&self) -> &Arc<ModelRegistry> {
        &self.inner.model_registry
    }

    pub fn cwd(&self) -> &Path {
        &self.inner.cwd
    }

    /// The current model; `None` when none is selected.
    pub fn model(&self) -> Option<Model> {
        self.inner
            .agent
            .with_state(|s| real_model(&s.model).cloned())
    }

    pub fn thinking_level(&self) -> ThinkingLevel {
        self.inner.agent.with_state(|s| s.thinking_level.clone())
    }

    pub fn is_streaming(&self) -> bool {
        self.inner.agent.with_state(|s| s.is_streaming)
    }

    /// The effective system prompt.
    pub fn system_prompt(&self) -> String {
        self.inner.agent.with_state(|s| s.system_prompt.clone())
    }

    /// All messages, including bash executions and custom messages.
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.inner.agent.with_state(|s| s.messages.clone())
    }

    /// Names of the tools set on the agent, in order.
    pub fn get_active_tool_names(&self) -> Vec<String> {
        self.inner
            .agent
            .with_state(|s| s.tools.iter().map(|t| t.name.clone()).collect())
    }

    /// `getAllTools()`: every configured tool with its source.
    pub fn get_all_tools(&self) -> Vec<ToolInfo> {
        lock(&self.inner.state)
            .tool_definitions
            .iter()
            .map(|(name, entry)| ToolInfo {
                name: entry.definition.name.clone(),
                description: entry.definition.description.clone(),
                parameters: entry.definition.parameters.clone(),
                source: entry.source,
                source_path: match entry.source {
                    ToolSource::Builtin => format!("<builtin:{name}>"),
                    ToolSource::Sdk => format!("<sdk:{name}>"),
                },
            })
            .collect()
    }

    pub fn get_tool_definition(&self, name: &str) -> Option<ToolDefinition> {
        lock(&self.inner.state)
            .tool_definitions
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, entry)| entry.definition.clone())
    }

    pub fn steering_mode(&self) -> QueueMode {
        from_agent_queue_mode(self.inner.agent.steering_mode())
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        from_agent_queue_mode(self.inner.agent.follow_up_mode())
    }

    /// The session file, or `None` when sessions are not persisted.
    pub fn session_file(&self) -> Option<PathBuf> {
        self.session_manager().session_file().map(Path::to_path_buf)
    }

    pub fn session_id(&self) -> String {
        self.session_manager().session_id().to_string()
    }

    pub fn session_name(&self) -> Option<String> {
        self.session_manager().session_name()
    }

    /// The chosen name, else the auto-assigned slug.
    pub fn display_name(&self) -> String {
        self.session_manager().display_name()
    }

    /// The colour slot (1-6), chosen or auto-assigned.
    pub fn session_color_slot(&self) -> u8 {
        self.session_manager().session_color_slot()
    }

    pub fn scoped_models(&self) -> Vec<ScopedModel> {
        lock(&self.inner.state).scoped_models.clone()
    }

    pub fn set_scoped_models(&self, scoped_models: Vec<ScopedModel>) {
        lock(&self.inner.state).scoped_models = scoped_models;
    }

    // ------------------------------------------------------------------
    // Tools and system prompt
    // ------------------------------------------------------------------

    fn is_allowed_tool(&self, name: &str) -> bool {
        self.inner
            .allowed_tool_names
            .as_ref()
            .is_none_or(|allowed| allowed.contains(name))
            && !self
                .inner
                .disallowed_tool_names
                .as_ref()
                .is_some_and(|denied| denied.contains(name))
    }

    fn tool_context_factory(&self) -> ToolContextFactory {
        let weak = Arc::downgrade(&self.inner);
        let branch: Arc<dyn SessionBranch> =
            Arc::new(BranchView(self.inner.session_manager.clone()));
        Arc::new(move || ToolContext {
            model: weak
                .upgrade()
                .and_then(|inner| inner.agent.with_state(|s| real_model(&s.model).cloned())),
            session_manager: Some(branch.clone()),
        })
    }

    /// `_buildRuntime`: build the built-in tools, then the registry.
    fn build_runtime(&self, active_tool_names: Option<Vec<String>>, include_all_custom: bool) {
        let (base, default_active): (Vec<ToolDefinition>, Vec<String>) =
            match &self.inner.base_tools {
                BaseTools::Override(tools) => (
                    tools.clone(),
                    tools.iter().map(|t| t.name.clone()).collect(),
                ),
                BaseTools::Factory(factory) => {
                    let settings = lock(&self.inner.settings);
                    (
                        factory(&BaseToolsContext {
                            cwd: &self.inner.cwd,
                            settings: &settings,
                        }),
                        DEFAULT_ACTIVE_TOOL_NAMES.map(String::from).to_vec(),
                    )
                }
            };
        lock(&self.inner.state).base_tool_definitions = base;
        self.refresh_tool_registry(
            Some(active_tool_names.unwrap_or(default_active)),
            include_all_custom,
        );
    }

    /// `_refreshToolRegistry`.
    fn refresh_tool_registry(
        &self,
        active_tool_names: Option<Vec<String>>,
        include_all_custom: bool,
    ) {
        let previous_registry_names: HashSet<String> = lock(&self.inner.state)
            .tool_registry
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let previous_active = self.get_active_tool_names();

        let custom: Vec<ToolDefinition> = self
            .inner
            .custom_tools
            .iter()
            .filter(|d| self.is_allowed_tool(&d.name))
            .cloned()
            .collect();
        let builtins: Vec<ToolDefinition> = lock(&self.inner.state)
            .base_tool_definitions
            .iter()
            .filter(|d| self.is_allowed_tool(&d.name))
            .cloned()
            .collect();

        // Definitions keyed by name: built-ins first, custom tools override.
        let mut definitions: Vec<(String, DefinitionEntry)> = Vec::new();
        for (definition, source) in builtins
            .iter()
            .map(|d| (d, ToolSource::Builtin))
            .chain(custom.iter().map(|d| (d, ToolSource::Sdk)))
        {
            let entry = DefinitionEntry {
                definition: definition.clone(),
                source,
            };
            match definitions.iter_mut().find(|(n, _)| *n == definition.name) {
                Some(slot) => slot.1 = entry,
                None => definitions.push((definition.name.clone(), entry)),
            }
        }
        let snippets: HashMap<String, String> = definitions
            .iter()
            .filter_map(|(name, e)| {
                normalize_prompt_snippet(e.definition.prompt_snippet.as_deref())
                    .map(|s| (name.clone(), s))
            })
            .collect();
        let guidelines: HashMap<String, Vec<String>> = definitions
            .iter()
            .filter_map(|(name, e)| {
                let g = normalize_prompt_guidelines(&e.definition.prompt_guidelines);
                (!g.is_empty()).then(|| (name.clone(), g))
            })
            .collect();

        let ctx = self.tool_context_factory();
        let wrapped_custom = wrap_tool_definitions(custom, Some(ctx.clone()));
        let wrapped_builtin = wrap_tool_definitions(builtins, Some(ctx));
        let mut registry: Vec<AgentTool> = Vec::new();
        for tool in wrapped_builtin
            .into_iter()
            .chain(wrapped_custom.iter().cloned())
        {
            match registry.iter_mut().find(|t| t.name == tool.name) {
                Some(slot) => *slot = tool,
                None => registry.push(tool),
            }
        }

        let mut next_active: Vec<String> = active_tool_names
            .clone()
            .unwrap_or(previous_active)
            .into_iter()
            .filter(|name| self.is_allowed_tool(name))
            .collect();
        if let Some(allowed) = &self.inner.allowed_tool_names {
            next_active.extend(
                registry
                    .iter()
                    .filter(|t| allowed.contains(&t.name))
                    .map(|t| t.name.clone()),
            );
        } else if include_all_custom {
            next_active.extend(wrapped_custom.iter().map(|t| t.name.clone()));
        } else if active_tool_names.is_none() {
            next_active.extend(
                registry
                    .iter()
                    .filter(|t| !previous_registry_names.contains(&t.name))
                    .map(|t| t.name.clone()),
            );
        }
        let mut seen = HashSet::new();
        next_active.retain(|name| seen.insert(name.clone()));

        {
            let mut state = lock(&self.inner.state);
            state.tool_definitions = definitions;
            state.tool_prompt_snippets = snippets;
            state.tool_prompt_guidelines = guidelines;
            state.tool_registry = registry;
        }
        self.set_active_tools_by_name(&next_active);
    }

    /// Activate registry tools by name (unknown names are ignored) and rebuild
    /// the system prompt. Takes effect on the next turn.
    pub fn set_active_tools_by_name(&self, tool_names: &[String]) {
        let (tools, valid_names): (Vec<AgentTool>, Vec<String>) = {
            let state = lock(&self.inner.state);
            tool_names
                .iter()
                .filter_map(|name| {
                    state
                        .tool_registry
                        .iter()
                        .find(|t| t.name == *name)
                        .map(|t| (t.clone(), name.clone()))
                })
                .unzip()
        };
        self.inner.agent.set_tools(tools);
        let prompt = self.rebuild_system_prompt(&valid_names);
        lock(&self.inner.state).base_system_prompt = prompt.clone();
        self.inner.agent.set_system_prompt(prompt);
        lock(&self.inner.state).runtime_context_dirty = true;
    }

    /// `_rebuildSystemPrompt`.
    fn rebuild_system_prompt(&self, tool_names: &[String]) -> String {
        let (valid, snippets, guidelines) = {
            let state = lock(&self.inner.state);
            let valid: Vec<String> = tool_names
                .iter()
                .filter(|name| state.tool_registry.iter().any(|t| t.name == **name))
                .cloned()
                .collect();
            let mut snippets = Vec::new();
            let mut guidelines = Vec::new();
            for name in &valid {
                if let Some(snippet) = state.tool_prompt_snippets.get(name) {
                    snippets.push((name.clone(), snippet.clone()));
                }
                if let Some(g) = state.tool_prompt_guidelines.get(name) {
                    guidelines.extend(g.iter().cloned());
                }
            }
            (valid, snippets, guidelines)
        };
        let loader = &self.inner.resource_loader;
        let append = loader.append_system_prompt();
        cortexcode_code_prompts::build_system_prompt(&BuildSystemPromptOptions {
            custom_prompt: loader.system_prompt(),
            append_system_prompt: (!append.is_empty()).then(|| append.join("\n\n")),
            selected_tools: Some(valid),
            tool_snippets: snippets,
            prompt_guidelines: guidelines,
            cwd: self.inner.cwd.to_string_lossy().into_owned(),
            context_files: loader.context_files(),
            skills: loader.skills(),
            // The agents list needs the Task tool and the agent registry (10.9).
            agents: Vec::new(),
            ..Default::default()
        })
    }

    // ------------------------------------------------------------------
    // Prompting
    // ------------------------------------------------------------------

    fn split_command(text: &str) -> (&str, &str) {
        let body = &text[1..];
        match body.find(' ') {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, ""),
        }
    }

    fn has_configured_auth(&self, model: &Model) -> bool {
        self.inner
            .model_registry
            .has_configured_auth(model, self.inner.auth.as_ref())
    }

    /// `prompt()`: run extension commands, expand skills and templates, queue
    /// while streaming (per `streaming_behavior`), else validate the model and
    /// its auth and run a turn.
    pub async fn prompt(&self, text: &str, options: PromptOptions) -> Result<()> {
        if options.expand_prompt_templates && text.starts_with('/') {
            let (name, args) = Self::split_command(text);
            if self.inner.extensions.has_command(name) {
                if let Err(error) = self.inner.extensions.run_command(name, args).await {
                    self.inner.extensions.emit_error(ExtensionError {
                        extension_path: format!("command:{name}"),
                        event: "command".into(),
                        error,
                    });
                }
                return Ok(());
            }
        }

        let images = options.images;
        let expanded = if options.expand_prompt_templates {
            self.inner.resource_loader.expand_input(text)
        } else {
            crate::hooks::ExpandedInput::plain(text)
        };

        if self.is_streaming() {
            match options.streaming_behavior {
                None => {
                    return err(
                        "Agent is already processing. Specify streamingBehavior ('steer' or 'followUp') to queue the message.",
                    )
                }
                Some(StreamingBehavior::FollowUp) => self.queue_follow_up(&expanded.text, &images),
                Some(StreamingBehavior::Steer) => self.queue_steer(&expanded.text, &images),
            }
            return Ok(());
        }

        self.flush_pending_bash_messages();

        let Some(model) = self.model() else {
            return err(format_no_model_selected_message());
        };
        if !self.has_configured_auth(&model) {
            if self.inner.auth.is_oauth(&model.provider) {
                return err(format!(
                    "Authentication failed for \"{0}\". Credentials may have expired or network is unavailable. Run '/login {0}' to re-authenticate.",
                    model.provider
                ));
            }
            return err(format_no_api_key_found_message(&model.provider));
        }
        // Compact first if the last response (aborted ones included) calls for it.
        let last_assistant = self.inner.agent.with_state(|s| {
            s.messages.iter().rev().find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(a.clone()),
                _ => None,
            })
        });
        if let Some(last_assistant) = last_assistant {
            self.check_compaction(&last_assistant, false).await;
        }

        let mut messages = Vec::new();
        let user_message_text = match expanded.template {
            Some(kind) if kind != TemplateKind::User && !expanded.args.trim().is_empty() => {
                expanded.args.clone()
            }
            _ => expanded.text.clone(),
        };
        if expanded.template == Some(TemplateKind::Context) {
            messages.push(AgentMessage::Custom(CustomMessage {
                custom_type: "slash_command".into(),
                content: UserContent::Text(expanded.text.clone()),
                display: false,
                details: None,
                timestamp: cortexcode_ai_types::now_ms(),
            }));
        }
        messages.push(user_message(&user_message_text, &images));
        let base_prompt = {
            let mut state = lock(&self.inner.state);
            messages.extend(
                state
                    .pending_next_turn_messages
                    .drain(..)
                    .map(AgentMessage::Custom),
            );
            state.base_system_prompt.clone()
        };
        // `before_agent_start` handlers may replace the prompt (12.3); reset to base.
        let mut system_prompt = base_prompt;
        if expanded.template == Some(TemplateKind::System) {
            if !system_prompt.is_empty() {
                system_prompt.push_str("\n\n");
            }
            system_prompt.push_str(&expanded.text);
        }
        self.inner.agent.set_system_prompt(system_prompt);

        self.inner
            .agent
            .prompt(messages)
            .await
            .map_err(|e| AgentSessionError(e.0))?;
        self.wait_for_retry().await;
        Ok(())
    }

    fn throw_if_extension_command(&self, text: &str) -> Result<()> {
        if !text.starts_with('/') {
            return Ok(());
        }
        let (name, _) = Self::split_command(text);
        if self.inner.extensions.has_command(name) {
            return err(format!(
                "Extension command \"/{name}\" cannot be queued. Use prompt() or execute the command when not streaming."
            ));
        }
        Ok(())
    }

    /// Queue a steering message: delivered after the current tool calls,
    /// before the next LLM call. Extension commands cannot be queued.
    pub fn steer(&self, text: &str, images: &[ImageContent]) -> Result<()> {
        self.throw_if_extension_command(text)?;
        let expanded = self.inner.resource_loader.expand_input(text);
        self.queue_steer(&expanded.text, images);
        Ok(())
    }

    /// Queue a follow-up: delivered once the agent would otherwise stop.
    pub fn follow_up(&self, text: &str, images: &[ImageContent]) -> Result<()> {
        self.throw_if_extension_command(text)?;
        let expanded = self.inner.resource_loader.expand_input(text);
        self.queue_follow_up(&expanded.text, images);
        Ok(())
    }

    fn queue_steer(&self, text: &str, images: &[ImageContent]) {
        lock(&self.inner.state)
            .steering_messages
            .push(text.to_string());
        self.emit_queue_update();
        self.inner.agent.steer(user_message(text, images));
    }

    fn queue_follow_up(&self, text: &str, images: &[ImageContent]) {
        lock(&self.inner.state)
            .follow_up_messages
            .push(text.to_string());
        self.emit_queue_update();
        self.inner.agent.follow_up(user_message(text, images));
    }

    /// `sendCustomMessage`: queue while streaming (steer unless `FollowUp`),
    /// hold for the next prompt (`NextTurn`), start a turn (`trigger_turn`), or
    /// append it to the transcript and session.
    pub async fn send_custom_message(
        &self,
        custom_type: &str,
        content: UserContent,
        display: bool,
        details: Option<serde_json::Value>,
        trigger_turn: bool,
        deliver_as: Option<DeliverAs>,
    ) -> Result<()> {
        let message = CustomMessage {
            custom_type: custom_type.to_string(),
            content,
            display,
            details,
            timestamp: cortexcode_ai_types::now_ms(),
        };
        if deliver_as == Some(DeliverAs::NextTurn) {
            lock(&self.inner.state)
                .pending_next_turn_messages
                .push(message);
        } else if self.is_streaming() {
            if deliver_as == Some(DeliverAs::FollowUp) {
                self.inner.agent.follow_up(AgentMessage::Custom(message));
            } else {
                self.inner.agent.steer(AgentMessage::Custom(message));
            }
        } else if trigger_turn {
            self.inner
                .agent
                .prompt(AgentMessage::Custom(message))
                .await
                .map_err(|e| AgentSessionError(e.0))?;
        } else {
            let agent_message = AgentMessage::Custom(message.clone());
            self.inner.agent.append_message(agent_message.clone());
            lock(&self.inner.session_manager).append_custom_message(
                message.custom_type,
                message.content,
                message.display,
                message.details,
            );
            self.emit(AgentSessionEvent::Agent(AgentEvent::MessageStart {
                message: agent_message.clone(),
            }));
            self.emit(AgentSessionEvent::Agent(AgentEvent::MessageEnd {
                message: agent_message,
            }));
        }
        Ok(())
    }

    /// `sendUserMessage`: always a turn; `deliver_as` queues it while streaming.
    /// Text parts are joined with newlines; images ride along.
    pub async fn send_user_message(
        &self,
        content: UserContent,
        deliver_as: Option<StreamingBehavior>,
    ) -> Result<()> {
        let (text, images) = match content {
            UserContent::Text(text) => (text, Vec::new()),
            UserContent::Blocks(blocks) => {
                let mut texts = Vec::new();
                let mut images = Vec::new();
                for block in blocks {
                    match block {
                        Content::Text(t) => texts.push(t.text),
                        Content::Image(i) => images.push(i),
                        _ => {}
                    }
                }
                (texts.join("\n"), images)
            }
        };
        self.prompt(
            &text,
            PromptOptions {
                expand_prompt_templates: false,
                images,
                streaming_behavior: deliver_as,
                source: InputSource::Extension,
            },
        )
        .await
    }

    /// `clearQueue()`: drop and return the queued texts.
    pub fn clear_queue(&self) -> (Vec<String>, Vec<String>) {
        let cleared = {
            let mut state = lock(&self.inner.state);
            (
                std::mem::take(&mut state.steering_messages),
                std::mem::take(&mut state.follow_up_messages),
            )
        };
        self.inner.agent.clear_all_queues();
        self.emit_queue_update();
        cleared
    }

    /// Steering plus follow-up messages still queued.
    pub fn pending_message_count(&self) -> usize {
        let state = lock(&self.inner.state);
        state.steering_messages.len() + state.follow_up_messages.len()
    }

    pub fn get_steering_messages(&self) -> Vec<String> {
        lock(&self.inner.state).steering_messages.clone()
    }

    pub fn get_follow_up_messages(&self) -> Vec<String> {
        lock(&self.inner.state).follow_up_messages.clone()
    }

    /// Abort the current run and wait until the agent is idle.
    pub async fn abort(&self) {
        self.abort_retry();
        self.inner.agent.abort();
        self.inner.agent.wait_for_idle().await;
    }

    // ------------------------------------------------------------------
    // Model and thinking level
    // ------------------------------------------------------------------

    fn apply_model(&self, model: &Model) {
        self.inner.agent.set_model(model.clone());
        lock(&self.inner.session_manager).append_model_change(&model.provider, &model.id);
        lock(&self.inner.settings).set_default_model_and_provider(&model.provider, &model.id);
    }

    /// `setModel`: requires configured auth; saved to the session and settings.
    pub fn set_model(&self, model: Model) -> Result<()> {
        if !self.has_configured_auth(&model) {
            return err(format!("No API key for {}/{}", model.provider, model.id));
        }
        let thinking_level = self.thinking_level_for_model_switch(None);
        self.apply_model(&model);
        self.set_thinking_level(thinking_level);
        Ok(())
    }

    /// `cycleModel`: through `--models` when set, else every available model.
    /// `None` when there is nothing to switch to.
    pub fn cycle_model(&self, direction: CycleDirection) -> Option<ModelCycleResult> {
        if lock(&self.inner.state).scoped_models.is_empty() {
            self.cycle_available_model(direction)
        } else {
            self.cycle_scoped_model(direction)
        }
    }

    fn next_index(current: Option<usize>, len: usize, direction: CycleDirection) -> usize {
        let current = current.unwrap_or(0);
        match direction {
            CycleDirection::Forward => (current + 1) % len,
            CycleDirection::Backward => (current + len - 1) % len,
        }
    }

    fn cycle_scoped_model(&self, direction: CycleDirection) -> Option<ModelCycleResult> {
        let scoped: Vec<ScopedModel> = self
            .scoped_models()
            .into_iter()
            .filter(|s| self.has_configured_auth(&s.model))
            .collect();
        if scoped.len() <= 1 {
            return None;
        }
        let current = self.model();
        let index = scoped
            .iter()
            .position(|s| cortexcode_ai_models::models_are_equal(Some(&s.model), current.as_ref()));
        let next = &scoped[Self::next_index(index, scoped.len(), direction)];
        let thinking_level = self.thinking_level_for_model_switch(next.thinking_level.clone());
        self.apply_model(&next.model);
        self.set_thinking_level(thinking_level);
        Some(ModelCycleResult {
            model: next.model.clone(),
            thinking_level: self.thinking_level(),
            is_scoped: true,
        })
    }

    fn cycle_available_model(&self, direction: CycleDirection) -> Option<ModelCycleResult> {
        let available: Vec<Model> = self
            .inner
            .model_registry
            .get_available(self.inner.auth.as_ref())
            .into_iter()
            .cloned()
            .collect();
        if available.len() <= 1 {
            return None;
        }
        let current = self.model();
        let index = available
            .iter()
            .position(|m| cortexcode_ai_models::models_are_equal(Some(m), current.as_ref()));
        let next = available[Self::next_index(index, available.len(), direction)].clone();
        let thinking_level = self.thinking_level_for_model_switch(None);
        self.apply_model(&next);
        self.set_thinking_level(thinking_level);
        Some(ModelCycleResult {
            model: next,
            thinking_level: self.thinking_level(),
            is_scoped: false,
        })
    }

    /// `setThinkingLevel`: clamped to the model; persisted only on change.
    pub fn set_thinking_level(&self, level: ThinkingLevel) {
        let available = self.get_available_thinking_levels();
        let effective = if available.contains(&level) {
            level
        } else {
            match self.model() {
                Some(model) => cortexcode_ai_models::clamp_thinking_level(&model, &level),
                None => ThinkingLevel::Off,
            }
        };
        let previous = self.thinking_level();
        self.inner.agent.set_thinking_level(effective.clone());
        if effective != previous {
            lock(&self.inner.session_manager)
                .append_thinking_level_change(thinking_level_str(&effective));
            if self.supports_thinking() || effective != ThinkingLevel::Off {
                lock(&self.inner.settings)
                    .set_default_thinking_level(ThinkingLevelSetting::from(effective.clone()));
            }
            self.emit(AgentSessionEvent::ThinkingLevelChanged { level: effective });
        }
    }

    /// `cycleThinkingLevel`: one step, wrapping; `None` without thinking support.
    pub fn cycle_thinking_level(&self, direction: CycleDirection) -> Option<ThinkingLevel> {
        if !self.supports_thinking() {
            return None;
        }
        let levels = self.get_available_thinking_levels();
        let current = levels.iter().position(|l| *l == self.thinking_level());
        let len = levels.len();
        // indexOf -1 steps to 0 (forward) or len-2 (backward), as in TS.
        let current = current.map_or(-1, |i| i as isize);
        let step = match direction {
            CycleDirection::Forward => 1,
            CycleDirection::Backward => -1,
        };
        let next = levels[((current + step + len as isize) as usize) % len].clone();
        self.set_thinking_level(next.clone());
        Some(next)
    }

    /// Thinking levels the current model supports.
    pub fn get_available_thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self.model() {
            Some(model) => cortexcode_ai_models::get_supported_thinking_levels(&model),
            None => THINKING_LEVELS.to_vec(),
        }
    }

    pub fn supports_thinking(&self) -> bool {
        self.model().is_some_and(|m| m.reasoning)
    }

    fn thinking_level_for_model_switch(&self, explicit: Option<ThinkingLevel>) -> ThinkingLevel {
        if let Some(level) = explicit {
            return level;
        }
        if !self.supports_thinking() {
            return lock(&self.inner.settings)
                .default_thinking_level()
                .map(ThinkingLevel::from)
                .unwrap_or(DEFAULT_THINKING_LEVEL);
        }
        self.thinking_level()
    }

    /// Set the steering mode; saved to settings.
    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.inner
            .agent
            .set_steering_mode(to_agent_queue_mode(mode));
        lock(&self.inner.settings).set_steering_mode(mode);
    }

    /// Set the follow-up mode; saved to settings.
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.inner
            .agent
            .set_follow_up_mode(to_agent_queue_mode(mode));
        lock(&self.inner.settings).set_follow_up_mode(mode);
    }

    // ------------------------------------------------------------------
    // Bash
    // ------------------------------------------------------------------

    /// `executeBash`: run a user command (the `!` prefix) with the configured
    /// prefix and shell, and record it. Blocks until the command ends; call
    /// [`AgentSession::abort_bash`] from elsewhere to cancel it.
    pub fn execute_bash(
        &self,
        command: &str,
        on_chunk: Option<&mut dyn FnMut(&str)>,
        exclude_from_context: bool,
        operations: Option<&dyn BashOperations>,
    ) -> Result<BashResult> {
        let signal = AbortSignal::new();
        lock(&self.inner.state).bash_abort = Some(signal.clone());
        let (prefix, shell_path) = {
            let settings = lock(&self.inner.settings);
            (settings.shell_command_prefix(), settings.shell_path())
        };
        let resolved = match prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}\n{command}"),
            _ => command.to_string(),
        };
        let cwd = PathBuf::from(self.session_manager().cwd());
        let local = LocalBashOperations::new(shell_path);
        let result = execute_bash_with_operations(
            &resolved,
            &cwd,
            operations.unwrap_or(&local),
            BashExecutorOptions {
                on_chunk,
                signal: Some(signal),
            },
        );
        lock(&self.inner.state).bash_abort = None;
        let result = result.map_err(|e| AgentSessionError(e.to_string()))?;
        self.record_bash_result(command, &result, exclude_from_context);
        Ok(result)
    }

    /// `recordBashResult`: add a bash run to the transcript and session; while
    /// streaming it waits for the next prompt so tool call/result pairs stay intact.
    pub fn record_bash_result(
        &self,
        command: &str,
        result: &BashResult,
        exclude_from_context: bool,
    ) {
        let message = BashExecutionMessage {
            command: command.to_string(),
            output: result.output.clone(),
            exit_code: result.exit_code.map(i64::from),
            cancelled: result.cancelled,
            truncated: result.truncated,
            full_output_path: result.full_output_path.clone(),
            timestamp: cortexcode_ai_types::now_ms(),
            exclude_from_context: exclude_from_context.then_some(true),
        };
        if self.is_streaming() {
            lock(&self.inner.state).pending_bash_messages.push(message);
        } else {
            let message = AgentMessage::BashExecution(message);
            self.inner.agent.append_message(message.clone());
            lock(&self.inner.session_manager).append_message(message);
        }
    }

    /// Cancel the running bash command.
    pub fn abort_bash(&self) {
        if let Some(signal) = &lock(&self.inner.state).bash_abort {
            signal.abort();
        }
    }

    pub fn is_bash_running(&self) -> bool {
        lock(&self.inner.state).bash_abort.is_some()
    }

    pub fn has_pending_bash_messages(&self) -> bool {
        !lock(&self.inner.state).pending_bash_messages.is_empty()
    }

    fn flush_pending_bash_messages(&self) {
        let pending = std::mem::take(&mut lock(&self.inner.state).pending_bash_messages);
        for message in pending {
            let message = AgentMessage::BashExecution(message);
            self.inner.agent.append_message(message.clone());
            lock(&self.inner.session_manager).append_message(message);
        }
    }

    // ------------------------------------------------------------------
    // Session info, stats, export
    // ------------------------------------------------------------------

    /// Set the session's display name.
    pub fn set_session_name(&self, name: &str) {
        let name = {
            let mut manager = self.session_manager();
            manager.append_session_info(Some(name), None);
            manager.session_name()
        };
        self.emit(AgentSessionEvent::SessionInfoChanged { name });
    }

    /// Set the colour slot (1-6) of the session's chip.
    pub fn set_session_color(&self, slot: u8) {
        let name = {
            let mut manager = self.session_manager();
            manager.append_session_info(None, Some(slot));
            manager.session_name()
        };
        self.emit(AgentSessionEvent::SessionInfoChanged { name });
    }

    /// User messages for the fork selector.
    pub fn get_user_messages_for_forking(&self) -> Vec<ForkableMessage> {
        stats::collect_user_messages_for_forking(&self.session_manager())
    }

    pub fn get_session_stats(&self) -> SessionStats {
        stats::compute_session_stats(
            &self.messages(),
            self.session_file()
                .map(|p| p.to_string_lossy().into_owned()),
            self.session_id(),
            self.get_context_usage(),
        )
    }

    pub fn get_context_usage(&self) -> Option<ContextUsage> {
        let model = self.model();
        let messages = self.messages();
        stats::compute_context_usage(model.as_ref(), &self.session_manager(), &messages)
    }

    /// Export the current branch as JSONL; returns the written path.
    pub fn export_to_jsonl(&self, output_path: Option<&Path>) -> std::io::Result<PathBuf> {
        stats::export_session_branch_to_jsonl(&self.session_manager(), output_path)
    }

    /// Text of the last assistant message (`/copy`).
    pub fn get_last_assistant_text(&self) -> Option<String> {
        stats::get_last_assistant_text(&self.messages())
    }

    /// The conversation as markdown (`/copy`).
    pub fn get_transcript_markdown(&self, selection: &TranscriptSelection) -> String {
        stats::session_to_markdown(&self.messages(), selection)
    }

    /// Tools as the agent runs them (for callers that need `AgentTools`).
    pub fn agent_tools(&self) -> AgentTools {
        self.inner.agent.with_state(|s| s.tools.clone())
    }
}

//! `AgentHarness`: hoocode `packages/agent/src/harness/agent-harness.ts`
//! (v0.5.89). Drives an [`Agent`] against a harness [`Session`]: each turn
//! rebuilds the agent's context from the session, finished messages are
//! written back, and session writes made mid-turn wait for the next save
//! point. Also runs compaction and tree navigation, invokes skills and
//! prompt templates, and exposes hooks and events.
//!
//! Hooks and the system-prompt / auth callbacks are synchronous here (the
//! agent's own hooks are); TS allows promises.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use cortexcode_agent_compaction::{
    collect_entries_for_branch_summary, compact as run_compaction, generate_branch_summary,
    prepare_compaction, BranchEntrySource, CompactionPreparation, CompactionResult,
    GenerateBranchSummaryOptions, SummarizeOptions, DEFAULT_COMPACTION_SETTINGS,
};
use cortexcode_agent_core::{Agent, AgentOptions, QueueMode, Subscription};
use cortexcode_agent_harness::{
    format_prompt_template_invocation, format_skill_invocation, ExecutionEnv, PromptTemplate, Skill,
};
use cortexcode_agent_session::{BranchSummaryInput, FileEntry, Session, SessionStorage};
use cortexcode_agent_types::{
    AfterToolCallResult, AgentContext, AgentEvent, AgentLoopTurnUpdate, AgentMessage, AgentState,
    AgentTool, AgentTools, BeforeToolCallResult,
};
use cortexcode_ai_types::{
    AbortSignal, AssistantMessage, Content, ImageContent, Model, OnPayload, OnResponse,
    ProviderResponse, ThinkingLevel, UserContent, UserMessage,
};
use serde_json::{json, Value};

/// A skill as the application models it (`TSkill extends Skill`).
pub trait SkillLike: Clone + Send + Sync + 'static {
    fn skill(&self) -> &Skill;
}

impl SkillLike for Skill {
    fn skill(&self) -> &Skill {
        self
    }
}

/// A prompt template as the application models it.
pub trait PromptTemplateLike: Clone + Send + Sync + 'static {
    fn prompt_template(&self) -> &PromptTemplate;
}

impl PromptTemplateLike for PromptTemplate {
    fn prompt_template(&self) -> &PromptTemplate {
        self
    }
}

/// `AgentHarnessResources`.
#[derive(Debug, Clone, PartialEq)]
pub struct HarnessResources<SK = Skill, PT = PromptTemplate> {
    pub skills: Option<Vec<SK>>,
    pub prompt_templates: Option<Vec<PT>>,
}

impl<SK, PT> Default for HarnessResources<SK, PT> {
    fn default() -> Self {
        Self {
            skills: None,
            prompt_templates: None,
        }
    }
}

/// `AgentHarnessPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Turn,
    Compaction,
    BranchSummary,
    Retry,
}

/// A session write deferred until the next save point.
#[derive(Debug, Clone)]
enum PendingSessionWrite {
    Message(Box<AgentMessage>),
    ModelChange { provider: String, model_id: String },
    ThinkingLevelChange(String),
}

/// `ThinkingLevel` as sessions store it.
pub fn thinking_level_str(level: &ThinkingLevel) -> &'static str {
    match level {
        ThinkingLevel::Off => "off",
        ThinkingLevel::Minimal => "minimal",
        ThinkingLevel::Low => "low",
        ThinkingLevel::Medium => "medium",
        ThinkingLevel::High => "high",
        ThinkingLevel::XHigh => "xhigh",
    }
}

/// The API key (and headers) for a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiKeyAndHeaders {
    pub api_key: String,
    pub headers: Option<HashMap<String, String>>,
}

/// `getApiKeyAndHeaders`.
pub type ApiKeyAndHeadersFn = Arc<dyn Fn(&Model) -> Option<ApiKeyAndHeaders> + Send + Sync>;

/// What a system-prompt callback sees.
pub struct SystemPromptContext<'a, S: SessionStorage, SK, PT> {
    pub env: &'a dyn ExecutionEnv,
    pub session: &'a Session<S>,
    pub model: &'a Model,
    pub thinking_level: &'a ThinkingLevel,
    pub active_tools: &'a [AgentTool],
    pub resources: &'a HarnessResources<SK, PT>,
}

/// A system-prompt callback.
pub type SystemPromptFn<S, SK, PT> =
    Arc<dyn Fn(SystemPromptContext<'_, S, SK, PT>) -> String + Send + Sync>;

/// `systemPrompt`: fixed text or built per turn.
pub enum SystemPrompt<S: SessionStorage, SK, PT> {
    Text(String),
    Build(SystemPromptFn<S, SK, PT>),
}

/// `ModelSelectEvent.source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSelectSource {
    Set,
    Restore,
}

/// `AgentHarnessEvent`: the agent's events plus the harness's own.
// Events are passed by reference to listeners and never stored in bulk.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum HarnessEvent<SK = Skill, PT = PromptTemplate> {
    Agent(AgentEvent),
    QueueUpdate {
        steer: Vec<AgentMessage>,
        follow_up: Vec<AgentMessage>,
        next_turn: Vec<AgentMessage>,
    },
    SavePoint {
        had_pending_mutations: bool,
    },
    Abort {
        cleared_steer: Vec<AgentMessage>,
        cleared_follow_up: Vec<AgentMessage>,
    },
    Settled {
        next_turn_count: usize,
    },
    AfterProviderResponse {
        status: u16,
        headers: BTreeMap<String, String>,
    },
    SessionCompact {
        compaction_entry: FileEntry,
        from_hook: bool,
    },
    SessionTree {
        new_leaf_id: Option<String>,
        old_leaf_id: Option<String>,
        summary_entry: Option<FileEntry>,
        from_hook: bool,
    },
    ModelSelect {
        model: Model,
        previous_model: Option<Model>,
        source: ModelSelectSource,
    },
    ThinkingLevelSelect {
        level: ThinkingLevel,
        previous_level: ThinkingLevel,
    },
    ResourcesUpdate {
        resources: HarnessResources<SK, PT>,
        previous_resources: HarnessResources<SK, PT>,
    },
}

// --- hook events and results ---

/// `BeforeAgentStartEvent`.
#[derive(Debug, Clone)]
pub struct BeforeAgentStartEvent<SK = Skill, PT = PromptTemplate> {
    pub prompt: String,
    pub images: Option<Vec<ImageContent>>,
    pub system_prompt: String,
    pub resources: HarnessResources<SK, PT>,
}

/// `BeforeAgentStartResult`.
#[derive(Debug, Clone, Default)]
pub struct BeforeAgentStartResult {
    pub messages: Option<Vec<AgentMessage>>,
    pub system_prompt: Option<String>,
}

/// `ToolCallEvent`.
#[derive(Debug, Clone)]
pub struct ToolCallEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
}

/// `ToolCallResult`.
#[derive(Debug, Clone, Default)]
pub struct ToolCallResult {
    pub block: Option<bool>,
    pub reason: Option<String>,
}

/// `ToolResultEvent`.
#[derive(Debug, Clone)]
pub struct ToolResultEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    pub content: Vec<Content>,
    pub details: Value,
    pub is_error: bool,
}

/// `ToolResultPatch`.
#[derive(Debug, Clone, Default)]
pub struct ToolResultPatch {
    pub content: Option<Vec<Content>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

/// `SessionBeforeCompactEvent`.
#[derive(Debug, Clone)]
pub struct SessionBeforeCompactEvent {
    pub preparation: CompactionPreparation,
    pub branch_entries: Vec<FileEntry>,
    pub custom_instructions: Option<String>,
    pub signal: AbortSignal,
}

/// `SessionBeforeCompactResult`.
#[derive(Debug, Clone, Default)]
pub struct SessionBeforeCompactResult {
    pub cancel: bool,
    /// A compaction the hook produced itself.
    pub compaction: Option<CompactionResult>,
}

/// `TreePreparation`.
#[derive(Debug, Clone)]
pub struct TreePreparation {
    pub target_id: String,
    pub old_leaf_id: Option<String>,
    pub common_ancestor_id: Option<String>,
    pub entries_to_summarize: Vec<FileEntry>,
    pub user_wants_summary: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// `SessionBeforeTreeEvent`.
#[derive(Debug, Clone)]
pub struct SessionBeforeTreeEvent {
    pub preparation: TreePreparation,
    pub signal: AbortSignal,
}

/// A hook-provided branch summary.
#[derive(Debug, Clone)]
pub struct ProvidedSummary {
    pub summary: String,
    pub details: Option<Value>,
}

/// `SessionBeforeTreeResult`.
#[derive(Debug, Clone, Default)]
pub struct SessionBeforeTreeResult {
    pub cancel: bool,
    pub summary: Option<ProvidedSummary>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// `NavigateTreeResult`.
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeResult {
    pub cancelled: bool,
    pub editor_text: Option<String>,
    pub summary_entry: Option<FileEntry>,
}

/// `navigateTree` options.
#[derive(Debug, Clone, Default)]
pub struct NavigateTreeOptions {
    pub summarize: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// `AbortResult`.
#[derive(Debug, Clone, Default)]
pub struct AbortResult {
    pub cleared_steer: Vec<AgentMessage>,
    pub cleared_follow_up: Vec<AgentMessage>,
}

type Handler<E, R> = Arc<dyn Fn(&E) -> Option<R> + Send + Sync>;

/// Handlers of one hook type, called in registration order; the last
/// non-`None` result wins (`emitHook`).
struct HookList<E, R> {
    next_id: u64,
    handlers: Vec<(u64, Handler<E, R>)>,
}

impl<E, R> Default for HookList<E, R> {
    fn default() -> Self {
        Self {
            next_id: 0,
            handlers: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Hooks<SK, PT> {
    before_agent_start: HookList<BeforeAgentStartEvent<SK, PT>, BeforeAgentStartResult>,
    context: HookList<Vec<AgentMessage>, Vec<AgentMessage>>,
    before_provider_request: HookList<Value, Value>,
    tool_call: HookList<ToolCallEvent, ToolCallResult>,
    tool_result: HookList<ToolResultEvent, ToolResultPatch>,
    session_before_compact: HookList<SessionBeforeCompactEvent, SessionBeforeCompactResult>,
    session_before_tree: HookList<SessionBeforeTreeEvent, SessionBeforeTreeResult>,
}

type Listener<SK, PT> = Arc<dyn Fn(&HarnessEvent<SK, PT>, Option<&AbortSignal>) + Send + Sync>;

/// `AgentHarnessOptions`.
pub struct AgentHarnessOptions<S: SessionStorage, SK = Skill, PT = PromptTemplate> {
    pub env: Arc<dyn ExecutionEnv>,
    pub session: Session<S>,
    pub tools: Vec<AgentTool>,
    pub resources: HarnessResources<SK, PT>,
    pub system_prompt: Option<SystemPrompt<S, SK, PT>>,
    pub get_api_key_and_headers: Option<ApiKeyAndHeadersFn>,
    pub model: Model,
    pub thinking_level: Option<ThinkingLevel>,
    pub active_tool_names: Option<Vec<String>>,
    pub steering_mode: Option<QueueMode>,
    pub follow_up_mode: Option<QueueMode>,
}

impl<S: SessionStorage, SK, PT> AgentHarnessOptions<S, SK, PT> {
    /// The required parts; everything else defaults.
    pub fn new(env: Arc<dyn ExecutionEnv>, session: Session<S>, model: Model) -> Self {
        Self {
            env,
            session,
            tools: Vec::new(),
            resources: HarnessResources::default(),
            system_prompt: None,
            get_api_key_and_headers: None,
            model,
            thinking_level: None,
            active_tool_names: None,
            steering_mode: None,
            follow_up_mode: None,
        }
    }
}

/// `AgentHarnessTurnState`.
struct TurnState<SK, PT> {
    messages: Vec<AgentMessage>,
    resources: HarnessResources<SK, PT>,
    system_prompt: String,
    model: Model,
    thinking_level: ThinkingLevel,
    active_tools: Vec<AgentTool>,
}

struct Inner<S: SessionStorage, SK, PT> {
    agent: Agent,
    env: Arc<dyn ExecutionEnv>,
    session: Mutex<Session<S>>,
    model: Mutex<Model>,
    thinking_level: Mutex<ThinkingLevel>,
    active_tool_names: Mutex<Vec<String>>,
    next_turn_queue: Mutex<Vec<AgentMessage>>,
    phase: Mutex<Phase>,
    steer_queue: Mutex<Vec<AgentMessage>>,
    follow_up_queue: Mutex<Vec<AgentMessage>>,
    pending_session_writes: Mutex<Vec<PendingSessionWrite>>,
    resources: Mutex<HarnessResources<SK, PT>>,
    system_prompt: Option<SystemPrompt<S, SK, PT>>,
    get_api_key_and_headers: Option<ApiKeyAndHeadersFn>,
    tools: Mutex<Vec<AgentTool>>,
    listeners: Mutex<Vec<(u64, Listener<SK, PT>)>>,
    next_listener_id: Mutex<u64>,
    hooks: Mutex<Hooks<SK, PT>>,
    subscription: Mutex<Option<Subscription>>,
}

/// `AgentHarness`.
pub struct AgentHarness<S: SessionStorage, SK = Skill, PT = PromptTemplate> {
    inner: Arc<Inner<S, SK, PT>>,
}

/// `createUserMessage`: the text, then any images.
fn create_user_message(text: &str, images: Option<&[ImageContent]>) -> AgentMessage {
    let mut content = vec![Content::text(text)];
    content.extend(images.into_iter().flatten().cloned().map(Content::Image));
    AgentMessage::User(UserMessage {
        content: content.into(),
        timestamp: cortexcode_ai_types::now_ms(),
    })
}

/// The text blocks of user-style content, joined without a separator.
fn content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                Content::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect(),
    }
}

/// Read access to a session for branch collection.
struct SessionSource<'a, S: SessionStorage>(&'a Session<S>);

impl<S: SessionStorage> BranchEntrySource for SessionSource<'_, S> {
    fn get_branch(&self, id: &str) -> Vec<FileEntry> {
        self.0.branch(Some(id))
    }
    fn get_entry(&self, id: &str) -> Option<FileEntry> {
        self.0.entry(id).cloned()
    }
}

impl<S, SK, PT> Inner<S, SK, PT>
where
    S: SessionStorage + Send + 'static,
    SK: SkillLike,
    PT: PromptTemplateLike,
{
    fn emit(&self, event: HarnessEvent<SK, PT>, signal: Option<&AbortSignal>) {
        let listeners: Vec<Listener<SK, PT>> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in listeners {
            listener(&event, signal);
        }
    }

    fn emit_queue_update(&self) {
        let event = HarnessEvent::QueueUpdate {
            steer: self.steer_queue.lock().unwrap().clone(),
            follow_up: self.follow_up_queue.lock().unwrap().clone(),
            next_turn: self.next_turn_queue.lock().unwrap().clone(),
        };
        self.emit(event, None);
    }

    fn emit_hook<E, R>(
        &self,
        pick: impl Fn(&Hooks<SK, PT>) -> &HookList<E, R>,
        event: &E,
    ) -> Option<R> {
        let handlers: Vec<Handler<E, R>> = {
            let hooks = self.hooks.lock().unwrap();
            pick(&hooks)
                .handlers
                .iter()
                .map(|(_, h)| h.clone())
                .collect()
        };
        let mut last = None;
        for handler in handlers {
            if let Some(result) = handler(event) {
                last = Some(result);
            }
        }
        last
    }

    fn resources(&self) -> HarnessResources<SK, PT> {
        self.resources.lock().unwrap().clone()
    }

    fn active_tools(&self) -> Vec<AgentTool> {
        let tools = self.tools.lock().unwrap();
        self.active_tool_names
            .lock()
            .unwrap()
            .iter()
            .filter_map(|name| tools.iter().find(|t| &t.name == name).cloned())
            .collect()
    }

    /// `createTurnState`.
    fn create_turn_state(&self) -> TurnState<SK, PT> {
        let resources = self.resources();
        let active_tools = self.active_tools();
        let model = self.model.lock().unwrap().clone();
        let thinking_level = self.thinking_level.lock().unwrap().clone();
        let session = self.session.lock().unwrap();
        let messages = session.build_context().messages;
        let system_prompt = match &self.system_prompt {
            None => "You are a helpful assistant.".to_string(),
            Some(SystemPrompt::Text(text)) => text.clone(),
            Some(SystemPrompt::Build(build)) => build(SystemPromptContext {
                env: self.env.as_ref(),
                session: &session,
                model: &model,
                thinking_level: &thinking_level,
                active_tools: &active_tools,
                resources: &resources,
            }),
        };
        TurnState {
            messages,
            resources,
            system_prompt,
            model,
            thinking_level,
            active_tools,
        }
    }

    /// `applyTurnState`.
    fn apply_turn_state(&self, turn: &TurnState<SK, PT>) {
        self.agent.set_messages(turn.messages.clone());
        self.agent.set_system_prompt(turn.system_prompt.clone());
        self.agent.set_model(turn.model.clone());
        self.agent.set_thinking_level(turn.thinking_level.clone());
        self.agent.set_tools(turn.active_tools.clone());
    }

    /// `flushPendingSessionWrites`.
    fn flush_pending_session_writes(&self) {
        let writes = std::mem::take(&mut *self.pending_session_writes.lock().unwrap());
        let mut session = self.session.lock().unwrap();
        for write in writes {
            // Storage failures surface on the next direct session call.
            let _ = match write {
                PendingSessionWrite::Message(message) => session.append_message(*message),
                PendingSessionWrite::ModelChange { provider, model_id } => {
                    session.append_model_change(&provider, &model_id)
                }
                PendingSessionWrite::ThinkingLevelChange(level) => {
                    session.append_thinking_level_change(&level)
                }
            };
        }
    }

    /// Remove `message` from `queue`; whether it was there.
    fn take_queued(queue: &Mutex<Vec<AgentMessage>>, message: &AgentMessage) -> bool {
        let mut queue = queue.lock().unwrap();
        match queue.iter().position(|m| m == message) {
            Some(index) => {
                queue.remove(index);
                true
            }
            None => false,
        }
    }

    /// `handleAgentEvent`.
    fn handle_agent_event(&self, event: &AgentEvent, signal: &AbortSignal) {
        self.emit(HarnessEvent::Agent(event.clone()), Some(signal));
        match event {
            AgentEvent::MessageStart {
                message: message @ AgentMessage::User(_),
            } => {
                if Self::take_queued(&self.steer_queue, message)
                    || Self::take_queued(&self.follow_up_queue, message)
                {
                    self.emit_queue_update();
                }
            }
            AgentEvent::MessageEnd { message } => {
                let _ = self.session.lock().unwrap().append_message(message.clone());
            }
            AgentEvent::TurnEnd { .. } => {
                let had_pending_mutations = !self.pending_session_writes.lock().unwrap().is_empty();
                self.flush_pending_session_writes();
                self.emit(
                    HarnessEvent::SavePoint {
                        had_pending_mutations,
                    },
                    None,
                );
            }
            AgentEvent::AgentEnd { .. } => {
                self.flush_pending_session_writes();
                *self.phase.lock().unwrap() = Phase::Idle;
                let next_turn_count = self.next_turn_queue.lock().unwrap().len();
                self.emit(HarnessEvent::Settled { next_turn_count }, Some(signal));
            }
            _ => {}
        }
    }

    fn auth(&self, model: &Model) -> Option<ApiKeyAndHeaders> {
        self.get_api_key_and_headers
            .as_ref()
            .and_then(|get| get(model))
    }
}

impl<S, SK, PT> AgentHarness<S, SK, PT>
where
    S: SessionStorage + Send + 'static,
    SK: SkillLike,
    PT: PromptTemplateLike,
{
    /// Build the harness and wire the agent's hooks through it.
    pub fn new(options: AgentHarnessOptions<S, SK, PT>) -> Self {
        let thinking_level = options.thinking_level.clone().unwrap_or(ThinkingLevel::Off);
        let agent = Agent::with_options(AgentOptions {
            initial_state: Some(AgentState {
                system_prompt: String::new(),
                model: options.model.clone(),
                thinking_level: thinking_level.clone(),
                tools: AgentTools::new(options.tools.clone()),
                messages: Vec::new(),
                is_streaming: false,
                streaming_message: None,
                pending_tool_calls: HashSet::new(),
                error_message: None,
            }),
            steering_mode: options.steering_mode,
            follow_up_mode: options.follow_up_mode,
            // TS's Agent defaults to `streamSimple`.
            stream_fn: Some(Arc::new(Box::new(cortexcode_ai_registry::stream_simple))),
            ..Default::default()
        });
        let active_tool_names = options
            .active_tool_names
            .clone()
            .unwrap_or_else(|| options.tools.iter().map(|t| t.name.clone()).collect());
        let inner = Arc::new(Inner {
            agent,
            env: options.env,
            session: Mutex::new(options.session),
            model: Mutex::new(options.model),
            thinking_level: Mutex::new(thinking_level),
            active_tool_names: Mutex::new(active_tool_names),
            next_turn_queue: Mutex::new(Vec::new()),
            phase: Mutex::new(Phase::Idle),
            steer_queue: Mutex::new(Vec::new()),
            follow_up_queue: Mutex::new(Vec::new()),
            pending_session_writes: Mutex::new(Vec::new()),
            resources: Mutex::new(options.resources),
            system_prompt: options.system_prompt,
            get_api_key_and_headers: options.get_api_key_and_headers,
            tools: Mutex::new(options.tools),
            listeners: Mutex::new(Vec::new()),
            next_listener_id: Mutex::new(0),
            hooks: Mutex::new(Hooks {
                before_agent_start: HookList::default(),
                context: HookList::default(),
                before_provider_request: HookList::default(),
                tool_call: HookList::default(),
                tool_result: HookList::default(),
                session_before_compact: HookList::default(),
                session_before_tree: HookList::default(),
            }),
            subscription: Mutex::new(None),
        });
        Self::wire(&inner);
        Self { inner }
    }

    /// The agent hooks, holding the harness weakly (the agent is inside it).
    fn wire(inner: &Arc<Inner<S, SK, PT>>) {
        let agent = &inner.agent;
        let weak: Weak<Inner<S, SK, PT>> = Arc::downgrade(inner);

        let w = weak.clone();
        agent.set_get_api_key(Some(Arc::new(move |provider: String| {
            let Some(inner) = w.upgrade() else {
                return Ok(None);
            };
            let model = inner.model.lock().unwrap().clone();
            if model.provider != provider {
                return Ok(None);
            }
            Ok(inner.auth(&model).map(|a| a.api_key))
        })));

        let w = weak.clone();
        agent.set_transform_context(Some(Arc::new(
            move |messages: Vec<AgentMessage>, _signal| {
                let Some(inner) = w.upgrade() else {
                    return Ok(messages);
                };
                Ok(inner
                    .emit_hook(|h| &h.context, &messages)
                    .unwrap_or(messages))
            },
        )));

        let w = weak.clone();
        agent.set_before_tool_call(Some(Arc::new(move |ctx, _signal| {
            let Some(inner) = w.upgrade() else {
                return Ok(None);
            };
            let event = ToolCallEvent {
                tool_call_id: ctx.tool_call.id.clone(),
                tool_name: ctx.tool_call.name.clone(),
                input: ctx.args.clone(),
            };
            Ok(inner
                .emit_hook(|h| &h.tool_call, &event)
                .map(|r| BeforeToolCallResult {
                    block: r.block.unwrap_or(false),
                    reason: r.reason,
                }))
        })));

        let w = weak.clone();
        agent.set_after_tool_call(Some(Arc::new(move |ctx, _signal| {
            let Some(inner) = w.upgrade() else {
                return Ok(None);
            };
            let event = ToolResultEvent {
                tool_call_id: ctx.tool_call.id.clone(),
                tool_name: ctx.tool_call.name.clone(),
                input: ctx.args.clone(),
                content: ctx.result.content.clone(),
                details: ctx.result.details.clone(),
                is_error: ctx.is_error,
            };
            Ok(inner
                .emit_hook(|h| &h.tool_result, &event)
                .map(|patch| AfterToolCallResult {
                    content: patch.content,
                    details: patch.details,
                    is_error: patch.is_error,
                    terminate: patch.terminate,
                }))
        })));

        let w = weak.clone();
        agent.set_on_payload(Some(OnPayload::sync(
            move |payload: &Value, _model: &Model| {
                w.upgrade()?
                    .emit_hook(|h| &h.before_provider_request, payload)
            },
        )));

        let w = weak.clone();
        agent.set_on_response(Some(OnResponse::sync(
            move |response: &ProviderResponse, _model: &Model| {
                if let Some(inner) = w.upgrade() {
                    let signal = inner.agent.signal();
                    inner.emit(
                        HarnessEvent::AfterProviderResponse {
                            status: response.status,
                            headers: response.headers.clone(),
                        },
                        signal.as_ref(),
                    );
                }
            },
        )));

        let w = weak.clone();
        agent.set_prepare_next_turn(Some(Arc::new(move |_turn, _signal| {
            let Some(inner) = w.upgrade() else {
                return Ok(None);
            };
            inner.flush_pending_session_writes();
            let turn = inner.create_turn_state();
            inner.apply_turn_state(&turn);
            Ok(Some(AgentLoopTurnUpdate {
                context: Some(AgentContext::new_with_tools(
                    turn.system_prompt,
                    turn.messages,
                    AgentTools::new(turn.active_tools),
                )),
                model: Some(turn.model),
                thinking_level: Some(turn.thinking_level),
            }))
        })));

        let w = weak;
        let subscription = agent.subscribe(move |event, signal| {
            if let Some(inner) = w.upgrade() {
                inner.handle_agent_event(event, signal);
            }
        });
        *inner.subscription.lock().unwrap() = Some(subscription);
    }

    /// The wrapped agent.
    pub fn agent(&self) -> &Agent {
        &self.inner.agent
    }

    /// The execution environment.
    pub fn env(&self) -> &Arc<dyn ExecutionEnv> {
        &self.inner.env
    }

    /// Read the session.
    pub fn with_session<R>(&self, f: impl FnOnce(&Session<S>) -> R) -> R {
        f(&self.inner.session.lock().unwrap())
    }

    pub fn phase(&self) -> Phase {
        *self.inner.phase.lock().unwrap()
    }

    fn begin(&self, phase: Phase, busy: &str) -> Result<(), String> {
        let mut current = self.inner.phase.lock().unwrap();
        if *current != Phase::Idle {
            return Err(busy.to_string());
        }
        *current = phase;
        Ok(())
    }

    fn set_phase(&self, phase: Phase) {
        *self.inner.phase.lock().unwrap() = phase;
    }

    /// `executeTurn`: queued next-turn messages, the prompt, the
    /// `before_agent_start` hook, then the run; the last new assistant
    /// message.
    async fn execute_turn(
        &self,
        turn: TurnState<SK, PT>,
        text: &str,
        images: Option<&[ImageContent]>,
    ) -> Result<AssistantMessage, String> {
        let inner = &self.inner;
        inner.apply_turn_state(&turn);
        let before_length = inner.agent.state().messages.len();
        let mut messages = vec![create_user_message(text, images)];
        let queued = std::mem::take(&mut *inner.next_turn_queue.lock().unwrap());
        if !queued.is_empty() {
            messages = queued.into_iter().chain(messages).collect();
            inner.emit_queue_update();
        }
        let event = BeforeAgentStartEvent {
            prompt: text.to_string(),
            images: images.map(<[ImageContent]>::to_vec),
            system_prompt: turn.system_prompt.clone(),
            resources: turn.resources.clone(),
        };
        if let Some(result) = inner.emit_hook(|h| &h.before_agent_start, &event) {
            if let Some(extra) = result.messages {
                messages = extra.into_iter().chain(messages).collect();
            }
            if let Some(system_prompt) = result.system_prompt {
                inner.agent.set_system_prompt(system_prompt);
            }
        }
        let run = inner.agent.prompt(messages).await;
        inner.flush_pending_session_writes();
        run.map_err(|e| e.to_string())?;
        let state = inner.agent.state();
        state
            .messages
            .get(before_length.min(state.messages.len())..)
            .unwrap_or_default()
            .iter()
            .rev()
            .find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(a.clone()),
                _ => None,
            })
            .ok_or_else(|| "AgentHarness prompt completed without an assistant message".to_string())
    }

    async fn run_turn(
        &self,
        make: impl FnOnce(&TurnState<SK, PT>) -> Result<String, String>,
        images: Option<&[ImageContent]>,
    ) -> Result<AssistantMessage, String> {
        self.begin(Phase::Turn, "AgentHarness is busy")?;
        let turn = self.inner.create_turn_state();
        let result = match make(&turn) {
            Ok(text) => self.execute_turn(turn, &text, images).await,
            Err(e) => Err(e),
        };
        if result.is_err() {
            self.set_phase(Phase::Idle);
        }
        result
    }

    /// `prompt`.
    pub async fn prompt(
        &self,
        text: &str,
        images: Option<&[ImageContent]>,
    ) -> Result<AssistantMessage, String> {
        self.run_turn(|_| Ok(text.to_string()), images).await
    }

    /// `skill`: prompt with the named skill's invocation block.
    pub async fn skill(
        &self,
        name: &str,
        additional_instructions: Option<&str>,
    ) -> Result<AssistantMessage, String> {
        self.run_turn(
            |turn| {
                let skill = turn
                    .resources
                    .skills
                    .iter()
                    .flatten()
                    .find(|s| s.skill().name == name)
                    .ok_or_else(|| format!("Unknown skill: {name}"))?;
                Ok(format_skill_invocation(
                    skill.skill(),
                    additional_instructions,
                ))
            },
            None,
        )
        .await
    }

    /// `promptFromTemplate`.
    pub async fn prompt_from_template(
        &self,
        name: &str,
        args: &[String],
    ) -> Result<AssistantMessage, String> {
        self.run_turn(
            |turn| {
                let template = turn
                    .resources
                    .prompt_templates
                    .iter()
                    .flatten()
                    .find(|t| t.prompt_template().name == name)
                    .ok_or_else(|| format!("Unknown prompt template: {name}"))?;
                Ok(format_prompt_template_invocation(
                    template.prompt_template(),
                    args,
                ))
            },
            None,
        )
        .await
    }

    /// `steer`: interrupt the running turn with a message.
    pub fn steer(&self, text: &str, images: Option<&[ImageContent]>) -> Result<(), String> {
        if self.phase() == Phase::Idle {
            return Err("Cannot steer while idle".to_string());
        }
        let message = create_user_message(text, images);
        self.inner.steer_queue.lock().unwrap().push(message.clone());
        self.inner.agent.steer(message);
        self.inner.emit_queue_update();
        Ok(())
    }

    /// `followUp`: a message for after the run.
    pub fn follow_up(&self, text: &str, images: Option<&[ImageContent]>) -> Result<(), String> {
        if self.phase() == Phase::Idle {
            return Err("Cannot follow up while idle".to_string());
        }
        let message = create_user_message(text, images);
        self.inner
            .follow_up_queue
            .lock()
            .unwrap()
            .push(message.clone());
        self.inner.agent.follow_up(message);
        self.inner.emit_queue_update();
        Ok(())
    }

    /// `nextTurn`: a message sent with the next prompt.
    pub fn next_turn(&self, text: &str, images: Option<&[ImageContent]>) {
        self.inner
            .next_turn_queue
            .lock()
            .unwrap()
            .push(create_user_message(text, images));
        self.inner.emit_queue_update();
    }

    /// `appendMessage`: written now when idle, else at the next save point.
    pub fn append_message(&self, message: AgentMessage) -> Result<(), String> {
        if self.phase() == Phase::Idle {
            self.inner
                .session
                .lock()
                .unwrap()
                .append_message(message)
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            self.inner
                .pending_session_writes
                .lock()
                .unwrap()
                .push(PendingSessionWrite::Message(Box::new(message)));
            Ok(())
        }
    }

    /// `compact`: summarize the current branch into a compaction entry
    /// (unless `session_before_compact` cancels or supplies one).
    pub async fn compact(
        &self,
        custom_instructions: Option<&str>,
    ) -> Result<CompactionResult, String> {
        let inner = &self.inner;
        self.begin(Phase::Compaction, "compact() requires idle harness")?;
        let model = inner.model.lock().unwrap().clone();
        let thinking_level = inner.thinking_level.lock().unwrap().clone();
        let auth = inner
            .auth(&model)
            .ok_or("No auth available for compaction")?;
        let branch_entries = inner.session.lock().unwrap().branch(None);
        let preparation = prepare_compaction(&branch_entries, &DEFAULT_COMPACTION_SETTINGS)
            .ok_or("Nothing to compact")?;
        let event = SessionBeforeCompactEvent {
            preparation: preparation.clone(),
            branch_entries,
            custom_instructions: custom_instructions.map(str::to_string),
            signal: AbortSignal::new(),
        };
        let hook = inner.emit_hook(|h| &h.session_before_compact, &event);
        if hook.as_ref().is_some_and(|h| h.cancel) {
            self.set_phase(Phase::Idle);
            return Err("Compaction cancelled".to_string());
        }
        let provided = hook.and_then(|h| h.compaction);
        let from_hook = provided.is_some();
        let result = match provided {
            Some(result) => result,
            None => {
                let options = SummarizeOptions {
                    api_key: Some(auth.api_key),
                    headers: auth.headers,
                    signal: None,
                    thinking_level: Some(thinking_level),
                };
                run_compaction(&preparation, &model, custom_instructions, &options).await?
            }
        };
        let entry = {
            let mut session = inner.session.lock().unwrap();
            let id = session
                .append_compaction(
                    &result.summary,
                    &result.first_kept_entry_id,
                    result.tokens_before,
                    result.details.clone(),
                    Some(from_hook),
                    result.tokens_after,
                )
                .map_err(|e| e.to_string())?;
            session.entry(&id).cloned()
        };
        if let Some(entry @ FileEntry::Compaction { .. }) = entry {
            inner.emit(
                HarnessEvent::SessionCompact {
                    compaction_entry: entry,
                    from_hook,
                },
                None,
            );
        }
        self.set_phase(Phase::Idle);
        Ok(result)
    }

    /// `navigateTree`: move the leaf to `target_id`, optionally summarizing
    /// the branch being left. A user (or custom) message target moves to its
    /// parent and returns the message text for the editor.
    pub async fn navigate_tree(
        &self,
        target_id: &str,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeResult, String> {
        let inner = &self.inner;
        self.begin(Phase::BranchSummary, "navigateTree() requires idle harness")?;
        let (old_leaf_id, target_entry, collected) = {
            let session = inner.session.lock().unwrap();
            let old_leaf_id = session.leaf_id();
            if old_leaf_id.as_deref() == Some(target_id) {
                drop(session);
                self.set_phase(Phase::Idle);
                return Ok(NavigateTreeResult::default());
            }
            let target_entry = session
                .entry(target_id)
                .cloned()
                .ok_or_else(|| format!("Entry {target_id} not found"))?;
            let collected = collect_entries_for_branch_summary(
                &SessionSource(&session),
                old_leaf_id.as_deref(),
                target_id,
            );
            (old_leaf_id, target_entry, collected)
        };
        let preparation = TreePreparation {
            target_id: target_id.to_string(),
            old_leaf_id: old_leaf_id.clone(),
            common_ancestor_id: collected.common_ancestor_id,
            entries_to_summarize: collected.entries.clone(),
            user_wants_summary: options.summarize,
            custom_instructions: options.custom_instructions.clone(),
            replace_instructions: options.replace_instructions,
            label: options.label.clone(),
        };
        let hook = inner.emit_hook(
            |h| &h.session_before_tree,
            &SessionBeforeTreeEvent {
                preparation,
                signal: AbortSignal::new(),
            },
        );
        if hook.as_ref().is_some_and(|h| h.cancel) {
            self.set_phase(Phase::Idle);
            return Ok(NavigateTreeResult {
                cancelled: true,
                ..Default::default()
            });
        }
        let from_hook = hook.as_ref().is_some_and(|h| h.summary.is_some());
        let mut summary_text = hook
            .as_ref()
            .and_then(|h| h.summary.as_ref())
            .map(|s| s.summary.clone())
            .filter(|s| !s.is_empty());
        let mut summary_details = hook
            .as_ref()
            .and_then(|h| h.summary.as_ref())
            .and_then(|s| s.details.clone());
        if summary_text.is_none() && options.summarize && !collected.entries.is_empty() {
            let model = inner.model.lock().unwrap().clone();
            let auth = inner
                .auth(&model)
                .ok_or("No auth available for branch summary")?;
            let branch_summary = generate_branch_summary(
                &collected.entries,
                &GenerateBranchSummaryOptions {
                    model,
                    api_key: Some(auth.api_key),
                    headers: auth.headers,
                    signal: Some(AbortSignal::new()),
                    custom_instructions: hook
                        .as_ref()
                        .and_then(|h| h.custom_instructions.clone())
                        .or(options.custom_instructions.clone()),
                    replace_instructions: hook
                        .as_ref()
                        .and_then(|h| h.replace_instructions)
                        .or(options.replace_instructions)
                        .unwrap_or(false),
                    reserve_tokens: None,
                },
            )
            .await?;
            if branch_summary.aborted {
                self.set_phase(Phase::Idle);
                return Ok(NavigateTreeResult {
                    cancelled: true,
                    ..Default::default()
                });
            }
            if let Some(error) = branch_summary.error {
                return Err(error);
            }
            summary_text = branch_summary.summary;
            summary_details = Some(json!({
                "readFiles": branch_summary.read_files.unwrap_or_default(),
                "modifiedFiles": branch_summary.modified_files.unwrap_or_default(),
            }));
        }

        let (new_leaf_id, editor_text) = match &target_entry {
            FileEntry::Message {
                parent_id,
                message: AgentMessage::User(user),
                ..
            } => (parent_id.clone(), Some(content_text(&user.content))),
            FileEntry::CustomMessage {
                parent_id, content, ..
            } => (parent_id.clone(), Some(content_text(content))),
            _ => (Some(target_id.to_string()), None),
        };
        let (summary_entry, new_leaf) = {
            let mut session = inner.session.lock().unwrap();
            let summary_id = session
                .move_to(
                    new_leaf_id.as_deref(),
                    summary_text.map(|summary| BranchSummaryInput {
                        summary,
                        details: summary_details,
                        from_hook: Some(from_hook),
                    }),
                )
                .map_err(|e| e.to_string())?;
            let entry = summary_id.and_then(|id| session.entry(&id).cloned());
            (entry, session.leaf_id())
        };
        inner.emit(
            HarnessEvent::SessionTree {
                new_leaf_id: new_leaf,
                old_leaf_id,
                summary_entry: summary_entry.clone(),
                from_hook,
            },
            None,
        );
        self.set_phase(Phase::Idle);
        Ok(NavigateTreeResult {
            cancelled: false,
            editor_text,
            summary_entry,
        })
    }

    /// `setModel`.
    pub fn set_model(&self, model: Model) -> Result<(), String> {
        let inner = &self.inner;
        let previous_model = std::mem::replace(&mut *inner.model.lock().unwrap(), model.clone());
        if self.phase() == Phase::Idle {
            inner.agent.set_model(model.clone());
            inner
                .session
                .lock()
                .unwrap()
                .append_model_change(&model.provider, &model.id)
                .map_err(|e| e.to_string())?;
        } else {
            inner
                .pending_session_writes
                .lock()
                .unwrap()
                .push(PendingSessionWrite::ModelChange {
                    provider: model.provider.clone(),
                    model_id: model.id.clone(),
                });
        }
        inner.emit(
            HarnessEvent::ModelSelect {
                model,
                previous_model: Some(previous_model),
                source: ModelSelectSource::Set,
            },
            None,
        );
        Ok(())
    }

    /// `setThinkingLevel`.
    pub fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), String> {
        let inner = &self.inner;
        let previous_level =
            std::mem::replace(&mut *inner.thinking_level.lock().unwrap(), level.clone());
        if self.phase() == Phase::Idle {
            inner.agent.set_thinking_level(level.clone());
            inner
                .session
                .lock()
                .unwrap()
                .append_thinking_level_change(thinking_level_str(&level))
                .map_err(|e| e.to_string())?;
        } else {
            inner.pending_session_writes.lock().unwrap().push(
                PendingSessionWrite::ThinkingLevelChange(thinking_level_str(&level).to_string()),
            );
        }
        inner.emit(
            HarnessEvent::ThinkingLevelSelect {
                level,
                previous_level,
            },
            None,
        );
        Ok(())
    }

    fn validate_tool_names(&self, names: &[String]) -> Result<(), String> {
        let tools = self.inner.tools.lock().unwrap();
        let missing: Vec<&str> = names
            .iter()
            .filter(|name| !tools.iter().any(|t| &&t.name == name))
            .map(String::as_str)
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("Unknown tool(s): {}", missing.join(", ")))
        }
    }

    /// `setActiveTools`.
    pub fn set_active_tools(&self, tool_names: Vec<String>) -> Result<(), String> {
        self.validate_tool_names(&tool_names)?;
        *self.inner.active_tool_names.lock().unwrap() = tool_names;
        if self.phase() == Phase::Idle {
            self.inner.agent.set_tools(self.inner.active_tools());
        }
        Ok(())
    }

    /// `setTools`: replace the tool set (and optionally the active names).
    pub fn set_tools(
        &self,
        tools: Vec<AgentTool>,
        active_tool_names: Option<Vec<String>>,
    ) -> Result<(), String> {
        *self.inner.tools.lock().unwrap() = tools;
        match active_tool_names {
            Some(names) => {
                self.validate_tool_names(&names)?;
                *self.inner.active_tool_names.lock().unwrap() = names;
            }
            None => {
                let names = self.inner.active_tool_names.lock().unwrap().clone();
                self.validate_tool_names(&names)?;
            }
        }
        if self.phase() == Phase::Idle {
            self.inner.agent.set_tools(self.inner.active_tools());
        }
        Ok(())
    }

    pub fn steering_mode(&self) -> QueueMode {
        self.inner.agent.steering_mode()
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.inner.agent.set_steering_mode(mode);
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.inner.agent.follow_up_mode()
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.inner.agent.set_follow_up_mode(mode);
    }

    /// `getResources`: a copy.
    pub fn get_resources(&self) -> HarnessResources<SK, PT> {
        self.inner.resources()
    }

    /// `setResources`, emitting `resources_update`.
    pub fn set_resources(&self, resources: HarnessResources<SK, PT>) {
        let previous_resources =
            std::mem::replace(&mut *self.inner.resources.lock().unwrap(), resources);
        self.inner.emit(
            HarnessEvent::ResourcesUpdate {
                resources: self.get_resources(),
                previous_resources,
            },
            None,
        );
    }

    /// `abort`: clear the queues, abort the run and wait for it.
    pub async fn abort(&self) -> AbortResult {
        let inner = &self.inner;
        let cleared_steer = std::mem::take(&mut *inner.steer_queue.lock().unwrap());
        let cleared_follow_up = std::mem::take(&mut *inner.follow_up_queue.lock().unwrap());
        inner.agent.clear_all_queues();
        inner.emit_queue_update();
        inner.agent.abort();
        inner.agent.wait_for_idle().await;
        inner.emit(
            HarnessEvent::Abort {
                cleared_steer: cleared_steer.clone(),
                cleared_follow_up: cleared_follow_up.clone(),
            },
            None,
        );
        AbortResult {
            cleared_steer,
            cleared_follow_up,
        }
    }

    pub async fn wait_for_idle(&self) {
        self.inner.agent.wait_for_idle().await;
    }

    /// `subscribe`: returns the unsubscribe function.
    pub fn subscribe(
        &self,
        listener: impl Fn(&HarnessEvent<SK, PT>, Option<&AbortSignal>) + Send + Sync + 'static,
    ) -> impl FnOnce() + Send + 'static {
        let id = {
            let mut next = self.inner.next_listener_id.lock().unwrap();
            *next += 1;
            *next
        };
        self.inner
            .listeners
            .lock()
            .unwrap()
            .push((id, Arc::new(listener)));
        let weak = Arc::downgrade(&self.inner);
        move || {
            if let Some(inner) = weak.upgrade() {
                inner.listeners.lock().unwrap().retain(|(i, _)| *i != id);
            }
        }
    }
}

/// `on(type, handler)` for each hook type; each returns the unregister
/// function.
macro_rules! hook_registration {
    ($(#[$doc:meta] $method:ident => $field:ident : $event:ty => $result:ty;)*) => {
        impl<S, SK, PT> AgentHarness<S, SK, PT>
        where
            S: SessionStorage + Send + 'static,
            SK: SkillLike,
            PT: PromptTemplateLike,
        {
            $(
                #[$doc]
                pub fn $method(
                    &self,
                    handler: impl Fn(&$event) -> Option<$result> + Send + Sync + 'static,
                ) -> impl FnOnce() + Send + 'static {
                    let id = {
                        let mut hooks = self.inner.hooks.lock().unwrap();
                        let list = &mut hooks.$field;
                        list.next_id += 1;
                        let id = list.next_id;
                        list.handlers.push((id, Arc::new(handler)));
                        id
                    };
                    let weak = Arc::downgrade(&self.inner);
                    move || {
                        if let Some(inner) = weak.upgrade() {
                            inner.hooks.lock().unwrap().$field.handlers.retain(|(i, _)| *i != id);
                        }
                    }
                }
            )*
        }
    };
}

hook_registration! {
    /// `on("before_agent_start")`: extra messages or a system prompt for the run.
    on_before_agent_start => before_agent_start: BeforeAgentStartEvent<SK, PT> => BeforeAgentStartResult;
    /// `on("context")`: replace the messages sent to the model.
    on_context => context: Vec<AgentMessage> => Vec<AgentMessage>;
    /// `on("before_provider_request")`: replace the provider payload.
    on_before_provider_request => before_provider_request: Value => Value;
    /// `on("tool_call")`: block a tool call.
    on_tool_call => tool_call: ToolCallEvent => ToolCallResult;
    /// `on("tool_result")`: patch a tool result.
    on_tool_result => tool_result: ToolResultEvent => ToolResultPatch;
    /// `on("session_before_compact")`: cancel or supply the compaction.
    on_session_before_compact => session_before_compact: SessionBeforeCompactEvent => SessionBeforeCompactResult;
    /// `on("session_before_tree")`: cancel or supply the branch summary.
    on_session_before_tree => session_before_tree: SessionBeforeTreeEvent => SessionBeforeTreeResult;
}

#[cfg(test)]
mod tests;

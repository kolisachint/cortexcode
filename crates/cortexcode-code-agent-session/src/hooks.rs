//! What AgentSession needs from the resource loader and the extension runner.
//! The resource loader is [`crate::resources::DefaultResources`]; the extension
//! runner arrives with ledger 12.3 (until then the defaults do nothing).

use std::future::Future;
use std::pin::Pin;

use cortexcode_code_prompts::{ContextFile, PromptSkill};

/// The `type` of a prompt template (`/name args`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// Expanded text replaces the user message.
    User,
    /// Expanded text is appended to the system prompt; the args are the message.
    System,
    /// Expanded text rides along as a hidden custom message; the args are the message.
    Context,
}

/// Input after skill-command and prompt-template expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedInput {
    pub text: String,
    /// The template that matched, if any.
    pub template: Option<TemplateKind>,
    /// The raw argument string after the template name.
    pub args: String,
}

impl ExpandedInput {
    /// Input nothing expanded.
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            template: None,
            args: String::new(),
        }
    }
}

/// `ResourceLoader`: the parts AgentSession reads.
pub trait ResourceLoader: Send + Sync {
    /// `getSystemPrompt()`: a prompt that replaces the built-in one.
    fn system_prompt(&self) -> Option<String> {
        None
    }
    /// `getAppendSystemPrompt()`.
    fn append_system_prompt(&self) -> Vec<String> {
        Vec::new()
    }
    /// `getSkills().skills`, as the system prompt lists them.
    fn skills(&self) -> Vec<PromptSkill> {
        Vec::new()
    }
    /// `getAgentsFiles().agentsFiles`.
    fn context_files(&self) -> Vec<ContextFile> {
        Vec::new()
    }
    /// `_expandSkillCommand` then `tryExpandPromptTemplate`.
    fn expand_input(&self, text: &str) -> ExpandedInput {
        ExpandedInput::plain(text)
    }
    /// `reload()`: re-read resources from disk.
    fn reload(&self) {}
}

/// A loader with no resources, or only a fixed system prompt (and appends).
#[derive(Debug, Clone, Default)]
pub struct StaticResourceLoader {
    pub system_prompt: Option<String>,
    pub append_system_prompt: Vec<String>,
}

impl ResourceLoader for StaticResourceLoader {
    fn system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }
    fn append_system_prompt(&self) -> Vec<String> {
        self.append_system_prompt.clone()
    }
}

/// An error an extension reported (`ExtensionError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionError {
    pub extension_path: String,
    pub event: String,
    pub error: String,
}

/// A running extension command.
pub type CommandFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// The extension runner, as far as AgentSession uses it.
pub trait ExtensionHooks: Send + Sync {
    /// `getCommand(name)` is set.
    fn has_command(&self, _name: &str) -> bool {
        false
    }
    /// Run a registered command's handler.
    fn run_command(&self, _name: &str, _args: &str) -> CommandFuture {
        Box::pin(async { Ok(()) })
    }
    /// `emitError`.
    fn emit_error(&self, _error: ExtensionError) {}
    /// `hasHandlers(type)`.
    fn has_handlers(&self, _event_type: &str) -> bool {
        false
    }
    /// `emit(event)` for session lifecycle events.
    fn emit_session_event(&self, _event: SessionEvent) -> SessionEventFuture {
        Box::pin(async { SessionEventResult::default() })
    }
}

/// No extensions loaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoExtensions;

impl ExtensionHooks for NoExtensions {}

// ----------------------------------------------------------------------
// Session lifecycle events (extensions/types.ts `SessionEvent`)
// ----------------------------------------------------------------------

/// `SessionStartEvent.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartReason {
    Startup,
    Reload,
    New,
    Resume,
    Fork,
}

/// `SessionBeforeSwitchEvent.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSwitchReason {
    New,
    Resume,
}

/// `SessionShutdownEvent.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionShutdownReason {
    Quit,
    Reload,
    New,
    Resume,
    Fork,
}

/// `SessionBeforeForkEvent.position`: fork before a user message (it goes
/// back to the editor) or at an entry (the branch up to it is duplicated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

/// `SessionStartEvent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStartEvent {
    pub reason: SessionStartReason,
    /// Present for `New`, `Resume` and `Fork`.
    pub previous_session_file: Option<String>,
}

impl SessionStartEvent {
    /// `{ type: "session_start", reason: "startup" }`.
    pub fn startup() -> Self {
        Self {
            reason: SessionStartReason::Startup,
            previous_session_file: None,
        }
    }
}

/// `TreePreparation`.
#[derive(Debug, Clone)]
pub struct TreePreparation {
    pub target_id: String,
    pub old_leaf_id: Option<String>,
    pub common_ancestor_id: Option<String>,
    pub entries_to_summarize: Vec<cortexcode_code_session::FileEntry>,
    pub user_wants_summary: bool,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// The session events AgentSession and its runtime emit to extensions.
// Emitted a handful of times per session; boxing the tree preparation would
// only add noise at the construction sites.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Start(SessionStartEvent),
    BeforeSwitch {
        reason: SessionSwitchReason,
        target_session_file: Option<String>,
    },
    BeforeFork {
        entry_id: String,
        position: ForkPosition,
    },
    Shutdown {
        reason: SessionShutdownReason,
        target_session_file: Option<String>,
    },
    BeforeTree {
        preparation: TreePreparation,
        signal: cortexcode_ai_types::AbortSignal,
    },
    Tree {
        new_leaf_id: Option<String>,
        old_leaf_id: Option<String>,
        summary_entry: Option<cortexcode_code_session::FileEntry>,
        from_extension: Option<bool>,
    },
}

impl SessionEvent {
    /// The event's `type` (what `hasHandlers` is asked about).
    pub fn event_type(&self) -> &'static str {
        match self {
            SessionEvent::Start(_) => "session_start",
            SessionEvent::BeforeSwitch { .. } => "session_before_switch",
            SessionEvent::BeforeFork { .. } => "session_before_fork",
            SessionEvent::Shutdown { .. } => "session_shutdown",
            SessionEvent::BeforeTree { .. } => "session_before_tree",
            SessionEvent::Tree { .. } => "session_tree",
        }
    }
}

/// An extension-provided branch summary (`SessionBeforeTreeResult.summary`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionSummary {
    pub summary: String,
    pub details: Option<serde_json::Value>,
}

/// The merged handler results (`SessionBefore{Switch,Fork,Tree}Result`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionEventResult {
    pub cancel: bool,
    pub summary: Option<ExtensionSummary>,
    pub custom_instructions: Option<String>,
    pub replace_instructions: Option<bool>,
    pub label: Option<String>,
}

/// A running session-event emit.
pub type SessionEventFuture = Pin<Box<dyn Future<Output = SessionEventResult> + Send>>;

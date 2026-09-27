//! `AgentSession`: the agent lifecycle shared by the cortex run modes.
//!
//! Ports hoocode's `core/agent-session.ts` (core), `agent-session-stats.ts`
//! and `agent-session-services.ts` with the session-building part of `sdk.ts`,
//! plus `agent-session-{retry,compaction,tree-navigation,runtime}.ts` and
//! `session-cwd.ts`.

pub use cortexcode_code_auth::auth_guidance;
pub mod compaction;
pub mod hooks;
pub mod resources;
pub mod retry;
pub mod runtime;
pub mod services;
pub mod session;
pub mod stats;
pub mod tree;

pub use compaction::{CompactionPlan, CompactionReason};
pub use hooks::{
    CommandFuture, ExpandedInput, ExtensionError, ExtensionHooks, ExtensionSummary, ForkPosition,
    NoExtensions, ResourceLoader, SessionEvent, SessionEventFuture, SessionEventResult,
    SessionShutdownReason, SessionStartEvent, SessionStartReason, SessionSwitchReason,
    SlashCommandInfo, StaticResourceLoader, TemplateKind, TreePreparation,
};
pub use resources::DefaultResources;
pub use runtime::{
    create_agent_session_runtime, AgentSessionRuntime, ChangeDirectoryResult, CreatedRuntime,
    ForkResult, NewSessionRequest, ReplaceResult, RuntimeError, RuntimeFactory, RuntimeRequest,
};
pub use services::{
    create_agent_session, create_agent_session_services, default_base_tools,
    AgentSessionRuntimeDiagnostic, AgentSessionServices, CreateAgentSessionOptions,
    CreatedAgentSession, DiagnosticKind, NoTools,
};
pub use session::{
    compaction_result_json, AgentSession, AgentSessionConfig, AgentSessionError, AgentSessionEvent,
    BaseTools, BaseToolsContext, BaseToolsFactory, CycleDirection, DeliverAs, InputSource,
    ModelCycleResult, PreflightResult, PromptOptions, ScopedModel, SessionSubscription,
    StreamingBehavior, ToolInfo, ToolSource, DEFAULT_ACTIVE_TOOL_NAMES, DEFAULT_THINKING_LEVEL,
};
pub use stats::{
    AssistantUsageTotals, ContextUsage, ForkableMessage, SessionStats, TokenStats,
    TranscriptSelection,
};
pub use tree::{NavigateTreeOptions, NavigateTreeResult};

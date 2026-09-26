//! `AgentSession`: the agent lifecycle shared by the cortex run modes.
//!
//! Ports hoocode's `core/agent-session.ts` (core), `agent-session-stats.ts`
//! and `agent-session-services.ts` with the session-building part of `sdk.ts`.

pub mod auth_guidance;
pub mod hooks;
pub mod services;
pub mod session;
pub mod stats;

pub use hooks::{
    CommandFuture, ExpandedInput, ExtensionError, ExtensionHooks, NoExtensions, ResourceLoader,
    StaticResourceLoader, TemplateKind,
};
pub use services::{
    create_agent_session, create_agent_session_services, default_base_tools,
    AgentSessionRuntimeDiagnostic, AgentSessionServices, CreateAgentSessionOptions,
    CreatedAgentSession, DiagnosticKind, NoTools,
};
pub use session::{
    AgentSession, AgentSessionConfig, AgentSessionError, AgentSessionEvent, BaseTools,
    BaseToolsContext, BaseToolsFactory, CycleDirection, DeliverAs, InputSource, ModelCycleResult,
    PromptOptions, ScopedModel, SessionSubscription, StreamingBehavior, ToolInfo, ToolSource,
    DEFAULT_ACTIVE_TOOL_NAMES, DEFAULT_THINKING_LEVEL,
};
pub use stats::{
    AssistantUsageTotals, ContextUsage, ForkableMessage, SessionStats, TokenStats,
    TranscriptSelection,
};

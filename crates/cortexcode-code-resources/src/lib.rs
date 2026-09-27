//! Resources for the cortex coding agent: skills, prompt templates, slash
//! commands and agent definitions.
//!
//! Ports of hoocode `packages/coding-agent/src/core/{skills,prompt-templates,
//! slash-commands,agent-frontmatter,agent-registry,context-files,source-info,
//! diagnostics}.ts` and
//! `utils/frontmatter.ts` (pinned v0.5.89). Paths are strings, as in hoocode,
//! so they appear in prompts and diagnostics exactly as hoocode prints them.

pub mod agent_frontmatter;
pub mod agent_registry;
pub mod builtin_skills;
pub mod context_files;
pub mod diagnostics;
pub mod frontmatter;
mod js;
pub mod node_path;
pub mod package_discovery;
pub mod package_resolve;
pub mod prompt_templates;
pub mod resource_loader;
pub mod skill_blocks;
pub mod skills;
pub mod slash_commands;
pub mod source_info;

pub use agent_frontmatter::{
    normalize_model, normalize_tools, parse_agent_definition, AgentDefinition, AgentSource,
    CLAUDE_TOOL_ALIASES, HOOCODE_TOOL_NAMES, MODEL_INHERIT, TASK_TOOL_NAME, TODO_WRITE_TOOL_NAME,
};
pub use agent_registry::{
    format_agents_for_prompt, load_agent_registry, summarize_agent_description, AgentRegistry,
    LoadAgentRegistryOptions,
};
pub use context_files::{
    load_project_context_files, resolve_prompt_input, ContextFile, ContextFileSize,
    LoadProjectContextFilesOptions,
};
pub use diagnostics::{DiagnosticType, ResourceCollision, ResourceDiagnostic};
pub use prompt_templates::{
    expand_prompt_template, load_prompt_templates, parse_command_args, substitute_args,
    try_expand_prompt_template, LoadPromptTemplatesOptions, PromptTemplate,
    PromptTemplateExpansion, PromptTemplateType,
};
pub use resource_loader::{
    AgentsFilesResult, DefaultResourceLoader, DefaultResourceLoaderOptions, LoadPromptsResult,
    PathEntry, ResourceExtensionPaths,
};
pub use skills::{
    format_skills_for_prompt, load_skills, load_skills_from_dir, LoadSkillsOptions,
    LoadSkillsResult, Skill,
};
pub use slash_commands::{
    BuiltinSlashCommand, SlashCommandInfo, SlashCommandSource, BUILTIN_SLASH_COMMANDS,
};
pub use source_info::{create_synthetic_source_info, SourceInfo, SourceOrigin, SourceScope};

//! What AgentSession needs from the resource loader and the extension runner.
//! Both are ported later (resources: ledger 10.5; extensions: 12.3); until
//! then they are traits whose defaults load nothing.

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
}

/// No extensions loaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoExtensions;

impl ExtensionHooks for NoExtensions {}

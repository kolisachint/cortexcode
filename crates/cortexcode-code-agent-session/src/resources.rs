//! The real resource loader behind [`ResourceLoader`]: hoocode's
//! `DefaultResourceLoader` (skills, prompt templates, context files, system
//! prompt inputs) from `cortexcode-code-resources`, plus AgentSession's
//! `_expandSkillCommand` + `tryExpandPromptTemplate` input expansion.

use std::sync::{Mutex, MutexGuard};

use cortexcode_code_prompts::{ContextFile, PromptSkill};
use cortexcode_code_resources::skill_blocks::expand_skill_command;
use cortexcode_code_resources::{
    try_expand_prompt_template, DefaultResourceLoader, DefaultResourceLoaderOptions,
    PromptTemplateType,
};

use crate::hooks::{ExpandedInput, ResourceLoader, TemplateKind};

/// A [`DefaultResourceLoader`] shared by the session.
pub struct DefaultResources {
    loader: Mutex<DefaultResourceLoader>,
}

impl DefaultResources {
    /// A loader that has not read anything yet (call [`ResourceLoader::reload`]).
    pub fn new(options: DefaultResourceLoaderOptions) -> Self {
        Self {
            loader: Mutex::new(DefaultResourceLoader::new(options)),
        }
    }

    /// A loader that has already run its first `reload()`.
    pub fn loaded(options: DefaultResourceLoaderOptions) -> Self {
        let this = Self::new(options);
        this.loader().reload();
        this
    }

    /// The underlying loader (skills, prompts, diagnostics, extension paths).
    pub fn loader(&self) -> MutexGuard<'_, DefaultResourceLoader> {
        self.loader.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ResourceLoader for DefaultResources {
    fn system_prompt(&self) -> Option<String> {
        self.loader().system_prompt()
    }

    fn append_system_prompt(&self) -> Vec<String> {
        self.loader().append_system_prompt()
    }

    fn skills(&self) -> Vec<PromptSkill> {
        self.loader()
            .skills()
            .skills
            .into_iter()
            .map(|s| PromptSkill {
                name: s.name,
                description: s.description,
                file_path: s.file_path,
                allowed_tools: s.allowed_tools.unwrap_or_default(),
                disable_model_invocation: s.disable_model_invocation,
            })
            .collect()
    }

    fn context_files(&self) -> Vec<ContextFile> {
        self.loader()
            .agents_files()
            .agents_files
            .into_iter()
            .map(|f| ContextFile {
                path: f.path,
                content: f.content,
            })
            .collect()
    }

    /// `_expandSkillCommand` then `tryExpandPromptTemplate`. A skill file that
    /// cannot be read leaves the text unchanged (hoocode reports it as an
    /// extension error event; the extension runner is ledger 12.3).
    fn expand_input(&self, text: &str) -> ExpandedInput {
        let loader = self.loader();
        let (text, _error) = expand_skill_command(text, &loader.skills().skills);
        let expansion = try_expand_prompt_template(&text, &loader.prompts().prompts);
        ExpandedInput {
            text: expansion.text,
            template: expansion.template.map(|t| match t.kind {
                PromptTemplateType::User => TemplateKind::User,
                PromptTemplateType::System => TemplateKind::System,
                PromptTemplateType::Context => TemplateKind::Context,
            }),
            args: expansion.args_string,
        }
    }

    fn reload(&self) {
        self.loader().reload();
    }
}

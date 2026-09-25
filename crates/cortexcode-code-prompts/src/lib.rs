//! System and mode prompts for the cortex coding agent.
//!
//! Mirrors `core/system-prompt.ts` (with the skills/agents/self-docs sections
//! it appends) from the TypeScript `packages/coding-agent` package. Mode prompts
//! arrive with 10.5b, prompt templates with 10.5.

use std::collections::HashMap;

pub mod system_prompt;

pub use system_prompt::{
    build_system_prompt, format_agents_for_prompt, format_self_docs_for_prompt,
    format_skills_for_prompt, list_self_docs, BuildSystemPromptOptions, ContextFile, PromptAgent,
    PromptSkill, SelfDoc, APP_NAME, LIGHT_SYSTEM_PROMPT,
};

/// Render a prompt template with variables.
pub fn render_template(template: &str, vars: &HashMap<String, String>) -> String {
    cortexcode_agent_harness::render(template, vars)
}

/// Predefined prompt templates.
pub struct Templates;

impl Templates {
    /// Template for summarizing a completed task.
    pub fn summary() -> &'static str {
        "Task completed. Changes made:\n{{changes}}\n\nNext steps:\n{{next_steps}}"
    }

    /// Template for asking the user a clarifying question.
    pub fn clarify() -> &'static str {
        "I need clarification about: {{topic}}\n\nPossible options:\n{{options}}"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_template() {
        let mut vars = HashMap::new();
        vars.insert("changes".to_string(), "added foo".to_string());
        vars.insert("next_steps".to_string(), "test".to_string());
        let out = render_template(Templates::summary(), &vars);
        assert!(out.contains("added foo"));
        assert!(out.contains("test"));
    }
}

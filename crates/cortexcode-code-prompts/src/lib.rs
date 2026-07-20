//! System and mode prompts for the cortex coding agent.
//!
//! Mirrors `core/{system-prompt,mode-prompts,prompt-templates}` from the
//! TypeScript `packages/coding-agent` package.

use cortexcode_agent_harness::SystemPromptBuilder;
use cortexcode_code_config::Config;
use std::collections::HashMap;

/// Available agent modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Default coding assistant mode.
    #[default]
    Code,
    /// Focused review mode.
    Review,
    /// Explain/diagram mode.
    Explain,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "code" => Ok(Mode::Code),
            "review" => Ok(Mode::Review),
            "explain" => Ok(Mode::Explain),
            _ => Err(format!("unknown mode: {}", s)),
        }
    }
}

/// Build the system prompt for a given mode and configuration.
pub fn system_prompt(mode: Mode, config: &Config) -> String {
    let mut builder = SystemPromptBuilder::new()
        .identity("You are Cortex, a helpful coding assistant.")
        .rules(&[
            "Use the provided tools to read, edit, write, and search code.",
            "Prefer small, focused changes and explain your reasoning briefly.",
            "When editing files, use the exact edit tool with old_text/new_text.",
            "Ask for clarification when requirements are ambiguous.",
            "Do not expose secrets or credentials.",
        ]);

    if config.auto_approve_read_only() {
        builder = builder.section(
            "Read-only tools",
            "Read-only tools (read, grep, find, ls) do not require explicit approval.",
        );
    }

    if config.auto_approve_dangerous() {
        builder = builder.section(
            "Dangerous tools",
            "Dangerous tools (bash, write, edit) are auto-approved; use them carefully.",
        );
    } else {
        builder = builder.section(
            "Dangerous tools",
            "Dangerous tools (bash, write, edit) require explicit user approval before running.",
        );
    }

    match mode {
        Mode::Code => builder
            .section(
                "Mode: Code",
                "Implement the user's request. Make minimal changes and verify with tests when possible.",
            )
            .build(),
        Mode::Review => builder
            .section(
                "Mode: Review",
                "Review the provided code. Identify bugs, style issues, and improvements. Do not modify files unless asked.",
            )
            .build(),
        Mode::Explain => builder
            .section(
                "Mode: Explain",
                "Explain the code or concept clearly. Use diagrams in plain text when helpful. Do not modify files.",
            )
            .build(),
    }
}

/// Build the initial user prompt for a given mode from a user request.
pub fn initial_user_prompt(mode: Mode, request: &str) -> String {
    match mode {
        Mode::Code => format!(
            "Please help me with the following coding task:\n{}",
            request
        ),
        Mode::Review => format!("Please review the following code or change:\n{}", request),
        Mode::Explain => format!("Please explain the following:\n{}", request),
    }
}

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
    fn test_system_prompt_contains_identity() {
        let config = Config::new();
        let prompt = system_prompt(Mode::Code, &config);
        assert!(prompt.contains("Cortex"));
        assert!(prompt.contains("read"));
    }

    #[test]
    fn test_mode_review_no_modify() {
        let config = Config::new();
        let prompt = system_prompt(Mode::Review, &config);
        assert!(prompt.contains("Review"));
        assert!(prompt.contains("Do not modify"));
    }

    #[test]
    fn test_initial_user_prompt() {
        assert!(initial_user_prompt(Mode::Code, "fix bug").contains("fix bug"));
    }

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

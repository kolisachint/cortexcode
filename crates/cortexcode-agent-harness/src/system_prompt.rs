//! System prompt construction helpers.

/// Builder for assembling a system prompt from ordered sections.
#[derive(Debug, Default, Clone)]
pub struct SystemPromptBuilder {
    sections: Vec<String>,
}

impl SystemPromptBuilder {
    /// Create a new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an identity section.
    pub fn identity(mut self, text: impl Into<String>) -> Self {
        self.sections.push(text.into());
        self
    }

    /// Add a rules / constraints section.
    pub fn rules(mut self, items: &[impl AsRef<str>]) -> Self {
        if items.is_empty() {
            return self;
        }
        let joined = items
            .iter()
            .enumerate()
            .map(|(i, item)| format!("{}. {}", i + 1, item.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        self.sections
            .push(format!("Follow these rules when working:\n{}", joined));
        self
    }

    /// Add a tools section describing available tools.
    pub fn tools(mut self, descriptions: &[impl AsRef<str>]) -> Self {
        if descriptions.is_empty() {
            return self;
        }
        let joined = descriptions
            .iter()
            .map(|d| format!("- {}", d.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        self.sections.push(format!(
            "You have access to the following tools:\n{}",
            joined
        ));
        self
    }

    /// Add an arbitrary section.
    pub fn section(mut self, heading: impl AsRef<str>, body: impl Into<String>) -> Self {
        self.sections
            .push(format!("{}\n{}", heading.as_ref(), body.into()));
        self
    }

    /// Build the final system prompt string.
    pub fn build(self) -> String {
        self.sections.join("\n\n")
    }
}

/// Build a default coding-agent system prompt.
pub fn default_coding_agent_prompt() -> String {
    SystemPromptBuilder::new()
        .identity("You are Cortex, a helpful coding assistant.")
        .rules(&[
            "Use the provided tools to read, edit, write, and search code.",
            "Always prefer small, focused changes.",
            "Ask for clarification when requirements are ambiguous.",
            "Do not expose secrets or credentials in your responses.",
        ])
        .tools(&[
            "read - read file contents",
            "bash - run shell commands",
            "edit - apply precise text replacements",
            "write - create new files",
            "grep - search file contents",
            "find - list files matching a pattern",
            "ls - list directory contents",
        ])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_sections_present() {
        let prompt = SystemPromptBuilder::new()
            .identity("ID")
            .rules(&["rule one", "rule two"])
            .tools(&["tool-a", "tool-b"])
            .build();
        assert!(prompt.contains("ID"));
        assert!(prompt.contains("1. rule one"));
        assert!(prompt.contains("2. rule two"));
        assert!(prompt.contains("- tool-a"));
    }

    #[test]
    fn test_default_prompt_contains_rules() {
        let prompt = default_coding_agent_prompt();
        assert!(prompt.contains("Cortex"));
        assert!(prompt.contains("read - read file contents"));
    }
}

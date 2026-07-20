//! Simple prompt template substitution.
//!
//! Templates use `{{variable}}` syntax. Missing variables are left unchanged
//! unless configured otherwise.

use std::collections::HashMap;

/// Error returned when template rendering fails.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateError {
    /// A required variable was missing.
    MissingVariable(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::MissingVariable(name) => {
                write!(f, "missing template variable: {}", name)
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// Render a template, replacing `{{key}}` with values from `vars`.
///
/// Variables not present in `vars` are left as-is.
pub fn render(template: &str, vars: &HashMap<String, String>) -> String {
    let mut output = template.to_string();
    for (key, value) in vars {
        output = output.replace(&format!("{{{{{}}}}}", key), value);
    }
    output
}

/// Render a template, returning an error if any `{{key}}` remains unresolved.
pub fn render_strict(
    template: &str,
    vars: &HashMap<String, String>,
) -> Result<String, TemplateError> {
    let rendered = render(template, vars);
    // Find any remaining `{{...}}` placeholders.
    if let Some(open) = rendered.find("{{") {
        if let Some(close) = rendered[open..].find("}}") {
            let var = rendered[open + 2..open + close].trim().to_string();
            return Err(TemplateError::MissingVariable(var));
        }
    }
    Ok(rendered)
}

/// A reusable template with a fixed body.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    body: String,
}

impl PromptTemplate {
    /// Create a new template from a string body.
    pub fn new(body: impl Into<String>) -> Self {
        Self { body: body.into() }
    }

    /// Render the template with the provided variables.
    pub fn render(&self, vars: &HashMap<String, String>) -> String {
        render(&self.body, vars)
    }

    /// Render the template strictly, failing on unresolved placeholders.
    pub fn render_strict(
        &self,
        vars: &HashMap<String, String>,
    ) -> Result<String, TemplateError> {
        render_strict(&self.body, vars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_simple() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Cortex".to_string());
        let out = render("Hello {{name}}!", &vars);
        assert_eq!(out, "Hello Cortex!");
    }

    #[test]
    fn test_render_leaves_unknown() {
        let out = render("Hello {{name}}!", &HashMap::new());
        assert_eq!(out, "Hello {{name}}!");
    }

    #[test]
    fn test_render_strict_ok() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "Cortex".to_string());
        let out = render_strict("Hello {{name}}!", &vars).unwrap();
        assert_eq!(out, "Hello Cortex!");
    }

    #[test]
    fn test_render_strict_missing() {
        let err = render_strict("Hello {{name}}!", &HashMap::new()).unwrap_err();
        assert!(matches!(err, TemplateError::MissingVariable(n) if n == "name"));
    }

    #[test]
    fn test_prompt_template() {
        let tmpl = PromptTemplate::new("{{greeting}}, {{name}}!");
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), "Hi".to_string());
        vars.insert("name".to_string(), "World".to_string());
        assert_eq!(tmpl.render(&vars), "Hi, World!");
    }
}

//! Prompt templates: hoocode `harness/prompt-templates.ts` (v0.5.89)
//! argument handling ([`parse_command_args`], [`substitute_args`],
//! [`format_prompt_template_invocation`]; the loaders are ledger 9.3b), plus
//! the `{{variable}}` [`render`] helpers `cortexcode-code-prompts` uses.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::types::PromptTemplate;

/// `parseCommandArgs`: split on spaces and tabs, honoring single and double
/// quotes (which are dropped).
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for c in args_string.chars() {
        match in_quote {
            Some(quote) => {
                if c == quote {
                    in_quote = None;
                } else {
                    current.push(c);
                }
            }
            None if c == '"' || c == '\'' => in_quote = Some(c),
            None if c == ' ' || c == '\t' => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// `String.prototype.replace` with a string replacement: `$$`, `$&`,
/// `` $` `` and `$'` are expanded (the patterns here have no groups).
fn js_replace_all(text: &str, re: &Regex, replacement: &str) -> String {
    re.replace_all(text, |caps: &Captures<'_>| {
        let m = caps.get(0).expect("whole match");
        let mut out = String::new();
        let mut chars = replacement.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            match chars.peek() {
                Some('$') => out.push('$'),
                Some('&') => out.push_str(m.as_str()),
                Some('`') => out.push_str(&text[..m.start()]),
                Some('\'') => out.push_str(&text[m.end()..]),
                _ => {
                    out.push('$');
                    continue;
                }
            }
            chars.next();
        }
        out
    })
    .into_owned()
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// `substituteArgs`: `$1`.., `${@:N}`, `${@:N:L}`, then `$ARGUMENTS` and `$@`
/// (in that order, so an argument's own text is substituted by later
/// passes, as in TS).
pub fn substitute_args(content: &str, args: &[String]) -> String {
    static POSITIONAL: OnceLock<Regex> = OnceLock::new();
    static SLICE: OnceLock<Regex> = OnceLock::new();
    static ARGUMENTS: OnceLock<Regex> = OnceLock::new();
    static ALL: OnceLock<Regex> = OnceLock::new();

    let arg = |index: Option<usize>| index.and_then(|i| args.get(i)).cloned().unwrap_or_default();
    let result = regex(&POSITIONAL, r"\$(\d+)").replace_all(content, |caps: &Captures<'_>| {
        // `args[parseInt(num) - 1] ?? ""`.
        let n = caps[1].parse::<usize>().ok();
        arg(n.and_then(|n| n.checked_sub(1)))
    });
    let result =
        regex(&SLICE, r"\$\{@:(\d+)(?::(\d+))?\}").replace_all(&result, |caps: &Captures<'_>| {
            let start = caps[1]
                .parse::<usize>()
                .unwrap_or(usize::MAX)
                .saturating_sub(1)
                .min(args.len());
            let end = match caps.get(2) {
                Some(length) => start
                    .saturating_add(length.as_str().parse::<usize>().unwrap_or(usize::MAX))
                    .min(args.len()),
                None => args.len(),
            };
            args[start..end].join(" ")
        });
    let all_args = args.join(" ");
    let result = js_replace_all(&result, regex(&ARGUMENTS, r"\$ARGUMENTS"), &all_args);
    js_replace_all(&result, regex(&ALL, r"\$@"), &all_args)
}

/// `formatPromptTemplateInvocation`.
pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

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

/// A reusable `{{variable}}` template with a fixed body.
#[derive(Debug, Clone)]
pub struct TextTemplate {
    body: String,
}

impl TextTemplate {
    /// Create a new template from a string body.
    pub fn new(body: impl Into<String>) -> Self {
        Self { body: body.into() }
    }

    /// Render the template with the provided variables.
    pub fn render(&self, vars: &HashMap<String, String>) -> String {
        render(&self.body, vars)
    }

    /// Render the template strictly, failing on unresolved placeholders.
    pub fn render_strict(&self, vars: &HashMap<String, String>) -> Result<String, TemplateError> {
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
        let tmpl = TextTemplate::new("{{greeting}}, {{name}}!");
        let mut vars = HashMap::new();
        vars.insert("greeting".to_string(), "Hi".to_string());
        vars.insert("name".to_string(), "World".to_string());
        assert_eq!(tmpl.render(&vars), "Hi, World!");
    }
}

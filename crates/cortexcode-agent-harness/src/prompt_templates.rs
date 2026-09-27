//! Prompt templates: hoocode `harness/prompt-templates.ts` (v0.5.89):
//! loading `.md` templates through an [`ExecutionEnv`] and argument handling
//! ([`parse_command_args`], [`substitute_args`],
//! [`format_prompt_template_invocation`]), plus the `{{variable}}`
//! [`render`] helpers `cortexcode-code-prompts` uses.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::env::{ExecutionEnv, FileKind};
use crate::frontmatter::{locale_compare, parse_frontmatter};
use crate::skills::{basename_env_path, resolve_kind, Sourced};
use crate::types::PromptTemplate;

/// `PromptTemplateDiagnostic` (always a warning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplateDiagnostic {
    pub message: String,
    pub path: String,
}

/// `loadPromptTemplates`' result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedPromptTemplates {
    pub prompt_templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<PromptTemplateDiagnostic>,
}

/// `loadSourcedPromptTemplates`' result.
#[derive(Debug, Clone, PartialEq)]
pub struct SourcedPromptTemplates<S> {
    pub prompt_templates: Vec<Sourced<PromptTemplate, S>>,
    pub diagnostics: Vec<Sourced<PromptTemplateDiagnostic, S>>,
}

/// `str.slice(0, n)` in UTF-16 code units.
fn js_slice(text: &str, n: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(n).collect();
    String::from_utf16_lossy(&units)
}

/// `loadTemplateFromFile`: the name is the file name without `.md`; the
/// description is the frontmatter's, else the first non-blank body line
/// (cut to 60 characters with `...`).
async fn load_template_from_file(
    env: &dyn ExecutionEnv,
    file_path: &str,
) -> Result<PromptTemplate, PromptTemplateDiagnostic> {
    let warn = |message: String| PromptTemplateDiagnostic {
        message,
        path: file_path.to_string(),
    };
    let raw = env
        .read_text_file(file_path)
        .await
        .map_err(|e| warn(e.message))?;
    let (frontmatter, body) = parse_frontmatter(&raw).map_err(warn)?;
    let mut description = frontmatter
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    if description.is_empty() {
        if let Some(first_line) = body.split('\n').find(|l| !l.trim().is_empty()) {
            description = js_slice(first_line, 60);
            if first_line.encode_utf16().count() > 60 {
                description.push_str("...");
            }
        }
    }
    let name = basename_env_path(file_path);
    let name = match name.len().checked_sub(3) {
        Some(cut) if name[cut..].eq_ignore_ascii_case(".md") => name[..cut].to_string(),
        _ => name,
    };
    Ok(PromptTemplate {
        name,
        description: Some(description),
        content: body,
    })
}

async fn push_template(env: &dyn ExecutionEnv, path: &str, result: &mut LoadedPromptTemplates) {
    match load_template_from_file(env, path).await {
        Ok(template) => result.prompt_templates.push(template),
        Err(diagnostic) => result.diagnostics.push(diagnostic),
    }
}

/// `loadPromptTemplates`: a directory loads its direct `.md` children (not
/// recursively), a file input loads that `.md` file; missing paths and other
/// files are skipped.
pub async fn load_prompt_templates(
    env: &dyn ExecutionEnv,
    paths: &[&str],
) -> LoadedPromptTemplates {
    let mut result = LoadedPromptTemplates::default();
    for path in paths {
        let Ok(info) = env.file_info(path).await else {
            continue;
        };
        match resolve_kind(env, &info).await {
            Some(FileKind::Directory) => {
                let mut entries = match env.list_dir(&info.path).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        result.diagnostics.push(PromptTemplateDiagnostic {
                            message: e.message,
                            path: info.path.clone(),
                        });
                        continue;
                    }
                };
                entries.sort_by(|a, b| locale_compare(&a.name, &b.name));
                for entry in entries {
                    if resolve_kind(env, &entry).await == Some(FileKind::File)
                        && entry.name.ends_with(".md")
                    {
                        push_template(env, &entry.path, &mut result).await;
                    }
                }
            }
            Some(FileKind::File) if info.name.ends_with(".md") => {
                push_template(env, &info.path, &mut result).await;
            }
            _ => {}
        }
    }
    result
}

/// `loadSourcedPromptTemplates`: [`load_prompt_templates`] per input,
/// tagged with its source.
pub async fn load_sourced_prompt_templates<S: Clone>(
    env: &dyn ExecutionEnv,
    inputs: &[(String, S)],
) -> SourcedPromptTemplates<S> {
    let mut result = SourcedPromptTemplates {
        prompt_templates: Vec::new(),
        diagnostics: Vec::new(),
    };
    for (path, source) in inputs {
        let loaded = load_prompt_templates(env, &[path.as_str()]).await;
        result
            .prompt_templates
            .extend(loaded.prompt_templates.into_iter().map(|item| Sourced {
                item,
                source: source.clone(),
            }));
        result
            .diagnostics
            .extend(loaded.diagnostics.into_iter().map(|item| Sourced {
                item,
                source: source.clone(),
            }));
    }
    result
}

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

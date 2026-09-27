//! `core/prompt-templates.ts`: prompt templates / slash commands from Markdown
//! files, argument parsing and substitution.

use crate::frontmatter::parse_frontmatter;
use crate::js::{js_string, truthy};
use crate::node_path;
use crate::source_info::{create_synthetic_source_info, SourceInfo, SourceScope};
use serde_json::Value;

/// `PromptTemplateType`: how the expansion enters the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptTemplateType {
    #[default]
    User,
    System,
    Context,
}

/// `PromptTemplate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub kind: PromptTemplateType,
    pub content: String,
    pub source_info: SourceInfo,
    pub file_path: String,
}

/// `parseCommandArgs`: whitespace-separated, `'`/`"` quoting, no escapes; empty
/// quoted strings vanish.
pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;
    for ch in args_string.chars() {
        match in_quote {
            Some(q) if ch == q => in_quote = None,
            Some(_) => current.push(ch),
            None if ch == '"' || ch == '\'' => in_quote = Some(ch),
            None if ch == ' ' || ch == '\t' => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// `String.prototype.replace(/literal/g, replacement)` with a string replacement:
/// `$$`, `$&`, `` $` `` and `$'` are expanded (the regexes have no groups, so
/// `$n` stays literal).
fn js_replace_all(haystack: &str, needle: &str, replacement: &str) -> String {
    if !haystack.contains(needle) {
        return haystack.to_string();
    }
    let mut out = String::new();
    let mut last = 0;
    for (start, _) in haystack.match_indices(needle) {
        out.push_str(&haystack[last..start]);
        let end = start + needle.len();
        let mut chars = replacement.char_indices().peekable();
        while let Some((_, c)) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            match chars.peek().map(|(_, n)| *n) {
                Some('$') => {
                    out.push('$');
                    chars.next();
                }
                Some('&') => {
                    out.push_str(needle);
                    chars.next();
                }
                Some('`') => {
                    out.push_str(&haystack[..start]);
                    chars.next();
                }
                Some('\'') => {
                    out.push_str(&haystack[end..]);
                    chars.next();
                }
                _ => out.push('$'),
            }
        }
        last = end;
    }
    out.push_str(&haystack[last..]);
    out
}

/// Replace every `\$(\d+)` with the positional argument (1-based; missing = "").
fn replace_positional(content: &str, args: &[String]) -> String {
    let bytes = content.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut last = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            out.push_str(&content[last..i]);
            let value = content[i + 1..j]
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|idx| args.get(idx))
                .map(String::as_str)
                .unwrap_or("");
            out.push_str(value);
            i = j;
            last = j;
        } else {
            i += 1;
        }
    }
    out.push_str(&content[last..]);
    out
}

/// Replace every `\$\{@:(\d+)(?::(\d+))?\}` with the bash-style slice.
fn replace_slices(content: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut rest = content;
    while let Some(pos) = rest.find("${@:") {
        let after = &rest[pos + 4..];
        let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
        let start_len = digits(after);
        let mut matched: Option<(usize, usize, Option<usize>)> = None; // (consumed, start, length)
        if start_len > 0 {
            let start_str = &after[..start_len];
            let tail = &after[start_len..];
            if tail.starts_with('}') {
                matched = Some((4 + start_len + 1, parse_or_max(start_str), None));
            } else if let Some(t) = tail.strip_prefix(':') {
                let len_len = digits(t);
                if len_len > 0 && t[len_len..].starts_with('}') {
                    matched = Some((
                        4 + start_len + 1 + len_len + 1,
                        parse_or_max(start_str),
                        Some(parse_or_max(&t[..len_len])),
                    ));
                }
            }
        }
        match matched {
            Some((consumed, start, length)) => {
                out.push_str(&rest[..pos]);
                let start = start.saturating_sub(1).min(args.len());
                let end = match length {
                    Some(l) => start.saturating_add(l).min(args.len()),
                    None => args.len(),
                };
                out.push_str(&args[start..end.max(start)].join(" "));
                rest = &rest[pos + consumed..];
            }
            None => {
                out.push_str(&rest[..pos + 1]);
                rest = &rest[pos + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_or_max(digits: &str) -> usize {
    digits.parse().unwrap_or(usize::MAX)
}

/// `substituteArgs`: `$1`.. first, then `${@:N}` / `${@:N:L}`, then `$ARGUMENTS`
/// and `$@` (all arguments joined by spaces), each pass over the previous result.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let result = replace_positional(content, args);
    let result = replace_slices(&result, args);
    let all = args.join(" ");
    let result = js_replace_all(&result, "$ARGUMENTS", &all);
    js_replace_all(&result, "$@", &all)
}

fn load_template_from_file(file_path: &str, source_info: SourceInfo) -> Option<PromptTemplate> {
    let raw = std::fs::read_to_string(file_path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&raw).ok()?;
    let base = node_path::basename(file_path);
    let name = base
        .strip_suffix(".prompt.md")
        .or_else(|| base.strip_suffix(".md"))
        .unwrap_or(&base)
        .to_string();

    let field = |key: &str| -> Option<String> {
        let v = frontmatter.get(key);
        truthy(v).then(|| js_string(v.unwrap()))
    };
    let description = field("description").unwrap_or_else(|| {
        body.split('\n')
            .find(|line| !line.trim().is_empty())
            .map(|first| {
                // `slice(0, 60)` counts UTF-16 units.
                let units: Vec<u16> = first.encode_utf16().collect();
                let mut d = String::from_utf16_lossy(&units[..units.len().min(60)]);
                if units.len() > 60 {
                    d.push_str("...");
                }
                d
            })
            .unwrap_or_default()
    });
    let kind = match frontmatter.get("type").and_then(Value::as_str) {
        Some("system") => PromptTemplateType::System,
        Some("context") => PromptTemplateType::Context,
        _ => PromptTemplateType::User,
    };
    Some(PromptTemplate {
        name,
        description,
        argument_hint: field("argument-hint"),
        kind,
        content: body,
        source_info,
        file_path: file_path.to_string(),
    })
}

/// `.md` files directly in `dir` (symlinks followed), in name order.
fn load_templates_from_dir(
    dir: &str,
    source_info: &dyn Fn(&str) -> SourceInfo,
) -> Vec<PromptTemplate> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read_dir
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter(|name| name.ends_with(".md"))
        .filter_map(|name| {
            let full = node_path::join(&[dir, &name]);
            let is_file = std::fs::metadata(&full)
                .map(|m| m.is_file())
                .unwrap_or(false);
            if !is_file {
                return None;
            }
            load_template_from_file(&full, source_info(&full))
        })
        .collect()
}

/// `LoadPromptTemplatesOptions`.
#[derive(Debug, Clone, Default)]
pub struct LoadPromptTemplatesOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub prompt_paths: Vec<String>,
    pub slash_command_paths: Vec<String>,
    pub include_defaults: bool,
}

/// `loadPromptTemplates`: `<agentDir>/commands`, `<cwd>/.cortexcode/commands`,
/// `<agentDir>/prompts`, `<cwd>/.cortexcode/prompts` (with defaults), then
/// explicit slash-command paths, then explicit prompt paths. Duplicates are kept;
/// lookups take the first match, so earlier sources win.
pub fn load_prompt_templates(options: &LoadPromptTemplatesOptions) -> Vec<PromptTemplate> {
    let cwd = options.cwd.as_str();
    let config_dir = cortexcode_code_paths::CONFIG_DIR_NAME;
    let global_prompts = node_path::join(&[&options.agent_dir, "prompts"]);
    let project_prompts = node_path::resolve(cwd, &format!("{config_dir}/prompts"));
    let global_commands = node_path::join(&[&options.agent_dir, "commands"]);
    let project_commands = node_path::resolve(cwd, &format!("{config_dir}/commands"));

    let source_info = |resolved: &str| -> SourceInfo {
        let under = node_path::is_under_path;
        if under(resolved, &global_prompts) || under(resolved, &global_commands) {
            let base = if under(resolved, &global_prompts) {
                &global_prompts
            } else {
                &global_commands
            };
            return create_synthetic_source_info(
                resolved,
                "local",
                Some(SourceScope::User),
                None,
                Some(base),
            );
        }
        if under(resolved, &project_prompts) || under(resolved, &project_commands) {
            let base = if under(resolved, &project_prompts) {
                &project_prompts
            } else {
                &project_commands
            };
            return create_synthetic_source_info(
                resolved,
                "local",
                Some(SourceScope::Project),
                None,
                Some(base),
            );
        }
        let base_dir = if std::fs::metadata(resolved).is_ok_and(|m| m.is_dir()) {
            resolved.to_string()
        } else {
            node_path::dirname(resolved)
        };
        create_synthetic_source_info(resolved, "local", None, None, Some(&base_dir))
    };

    let mut templates = Vec::new();
    if options.include_defaults {
        for dir in [
            &global_commands,
            &project_commands,
            &global_prompts,
            &project_prompts,
        ] {
            templates.extend(load_templates_from_dir(dir, &source_info));
        }
    }
    for raw in options
        .slash_command_paths
        .iter()
        .chain(options.prompt_paths.iter())
    {
        let resolved = node_path::resolve_config_path(raw, cwd);
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if meta.is_dir() {
            templates.extend(load_templates_from_dir(&resolved, &source_info));
        } else if meta.is_file() && resolved.ends_with(".md") {
            templates.extend(load_template_from_file(&resolved, source_info(&resolved)));
        }
    }
    templates
}

/// `PromptTemplateExpansion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplateExpansion {
    pub text: String,
    pub template: Option<PromptTemplate>,
    pub args: Vec<String>,
    pub args_string: String,
}

/// `tryExpandPromptTemplate`: `/name args` expands the first template called
/// `name`; anything else comes back unchanged.
pub fn try_expand_prompt_template(
    text: &str,
    templates: &[PromptTemplate],
) -> PromptTemplateExpansion {
    let unchanged = || PromptTemplateExpansion {
        text: text.to_string(),
        template: None,
        args: Vec::new(),
        args_string: String::new(),
    };
    let Some(rest) = text.strip_prefix('/') else {
        return unchanged();
    };
    let (name, args_string) = match rest.find(' ') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (rest, ""),
    };
    let Some(template) = templates.iter().find(|t| t.name == name) else {
        return unchanged();
    };
    let args = parse_command_args(args_string);
    PromptTemplateExpansion {
        text: substitute_args(&template.content, &args),
        template: Some(template.clone()),
        args,
        args_string: args_string.to_string(),
    }
}

/// `expandPromptTemplate`.
pub fn expand_prompt_template(text: &str, templates: &[PromptTemplate]) -> String {
    try_expand_prompt_template(text, templates).text
}

//! `core/agent-frontmatter.ts`: agent definitions (Markdown with YAML
//! frontmatter, Claude Code compatible) and the Claude tool-name shim.

use crate::diagnostics::ResourceDiagnostic;
use crate::frontmatter::parse_frontmatter;
use crate::js::js_string;
use serde_json::Value;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// `AgentSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource {
    Builtin,
    User,
    Project,
    ClaudeUser,
    ClaudeProject,
}

/// `MODEL_INHERIT`.
pub const MODEL_INHERIT: &str = "inherit";

/// `HOOCODE_TOOL_NAMES`: built-in tools an agent's `tools` list is normalized against.
pub const HOOCODE_TOOL_NAMES: &[&str] = &[
    "bash",
    "edit",
    "read",
    "SearchCodebase",
    "webfetch",
    "websearch",
    "write",
];

/// `TASK_TOOL_NAME`.
pub const TASK_TOOL_NAME: &str = "Task";
/// `TODO_WRITE_TOOL_NAME`.
pub const TODO_WRITE_TOOL_NAME: &str = "TodoWrite";

/// `CLAUDE_TOOL_ALIASES`: lower-cased Claude Code tool name -> hoocode tool.
pub const CLAUDE_TOOL_ALIASES: &[(&str, &str)] = &[
    ("read", "read"),
    ("write", "write"),
    ("edit", "edit"),
    ("bash", "bash"),
    ("searchcodebase", "SearchCodebase"),
    ("grep", "SearchCodebase"),
    ("glob", "SearchCodebase"),
    ("find", "SearchCodebase"),
    ("webfetch", "webfetch"),
    ("websearch", "websearch"),
];

const KNOWN_MODEL_ALIASES: &[&str] = &[
    "sonnet", "opus", "haiku", "inherit", "fast", "standard", "capable",
];

/// `AgentDefinition`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    /// `None` = inherit all parent tools.
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub prompt: String,
    pub source: AgentSource,
    pub file_path: Option<String>,
    pub max_turns: Option<u64>,
    pub background: Option<bool>,
    pub delegate: Option<bool>,
    pub delegate_to: Option<Vec<String>>,
    pub fork: Option<bool>,
}

fn is_valid_name_chars(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// JS `string.length` (UTF-16 code units).
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn validate_name(name: &str) -> Vec<String> {
    if name.is_empty() {
        return vec!["name is required".into()];
    }
    let mut errors = Vec::new();
    if js_len(name) > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            js_len(name)
        ));
    }
    if !is_valid_name_chars(name) {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".into(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".into());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".into());
    }
    errors
}

fn validate_description(description: &str) -> Vec<String> {
    if description.trim().is_empty() {
        vec!["description is required".into()]
    } else if js_len(description) > MAX_DESCRIPTION_LENGTH {
        vec![format!(
            "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
            js_len(description)
        )]
    } else {
        Vec::new()
    }
}

fn validate_model(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || KNOWN_MODEL_ALIASES.contains(&trimmed)
        || trimmed.starts_with("claude-")
    {
        return Vec::new();
    }
    vec![format!(
        "model \"{trimmed}\" is not a recognized alias (sonnet | opus | haiku | inherit | fast | standard | capable) or full model ID; the agent may not load correctly"
    )]
}

/// A `tools` / `allowed-tools` value: a comma-separated string or a YAML list.
fn split_tools_value(value: &Value) -> Vec<String> {
    let tokens: Vec<String> = match value {
        Value::Array(items) => items.iter().map(js_string).collect(),
        other => js_string(other).split(',').map(str::to_string).collect(),
    };
    tokens
        .into_iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// `normalizeTools`: map Claude Code tool names (case-insensitive) onto hoocode
/// tools, deduped, with a warning per unmapped name and for YAML lists.
pub fn normalize_tools(
    value: &Value,
    file_path: Option<&str>,
) -> (Vec<String>, Vec<ResourceDiagnostic>) {
    let mut diagnostics = Vec::new();
    if value.is_array() {
        diagnostics.push(ResourceDiagnostic::warning(
            "tools: use a comma-separated string (\"tools: read, bash\") instead of a YAML list for Claude Code compatibility",
            file_path,
        ));
    }
    let mut resolved: Vec<String> = Vec::new();
    for raw in split_tools_value(value) {
        let lower = raw.to_lowercase();
        let Some((_, mapped)) = CLAUDE_TOOL_ALIASES.iter().find(|(k, _)| *k == lower) else {
            diagnostics.push(ResourceDiagnostic::warning(
                format!(
                    "tool \"{raw}\" has no hoocode equivalent and was dropped from the allowlist"
                ),
                file_path,
            ));
            continue;
        };
        if !resolved.iter().any(|t| t == mapped) {
            resolved.push(mapped.to_string());
        }
    }
    (resolved, diagnostics)
}

/// `normalizeModel`: trimmed, `None` when empty or not a string.
pub fn normalize_model(value: Option<&Value>) -> Option<String> {
    let trimmed = value?.as_str()?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `parseAgentDefinition`: `agent` is `None` only when the definition is
/// unusable (bad frontmatter, no description, unusable name).
pub fn parse_agent_definition(
    raw_content: &str,
    source: AgentSource,
    file_path: Option<&str>,
    fallback_name: Option<&str>,
) -> (Option<AgentDefinition>, Vec<ResourceDiagnostic>) {
    let mut diagnostics = Vec::new();
    let (frontmatter, body) = match parse_frontmatter(raw_content) {
        Ok(parsed) => parsed,
        Err(message) => {
            diagnostics.push(ResourceDiagnostic::warning(message, file_path));
            return (None, diagnostics);
        }
    };
    let warn = |diagnostics: &mut Vec<ResourceDiagnostic>, message: String| {
        diagnostics.push(ResourceDiagnostic::warning(message, file_path));
    };

    let fallback = fallback_name.map(str::to_string).unwrap_or_else(|| {
        file_path
            .map(|p| {
                let base = crate::node_path::basename(p);
                base.strip_suffix(".md").map(str::to_string).unwrap_or(base)
            })
            .unwrap_or_default()
    });
    let name = match frontmatter.get("name") {
        None | Some(Value::Null) => fallback,
        Some(v) => js_string(v),
    }
    .trim()
    .to_string();
    for e in validate_name(&name) {
        warn(&mut diagnostics, e);
    }

    let description = frontmatter
        .get("description")
        .and_then(Value::as_str)
        .map(|d| d.trim().to_string())
        .unwrap_or_default();
    for e in validate_description(&description) {
        warn(&mut diagnostics, e);
    }
    if description.is_empty() || !is_valid_name_chars(&name) {
        return (None, diagnostics);
    }

    let tools = frontmatter.get("tools").filter(|v| !v.is_null()).map(|v| {
        let (tools, d) = normalize_tools(v, file_path);
        diagnostics.extend(d);
        tools
    });
    let disallowed_tools = frontmatter
        .get("disallowedTools")
        .filter(|v| !v.is_null())
        .and_then(|v| {
            let (tools, d) = normalize_tools(v, file_path);
            diagnostics.extend(d);
            (!tools.is_empty()).then_some(tools)
        });

    let model = normalize_model(frontmatter.get("model"));
    if let Some(model) = &model {
        for e in validate_model(model) {
            warn(&mut diagnostics, e);
        }
    }

    let max_turns = frontmatter
        .get("maxTurns")
        .and_then(Value::as_f64)
        .filter(|n| n.fract() == 0.0 && *n > 0.0)
        .map(|n| n as u64);

    let flag = |key: &str, diagnostics: &mut Vec<ResourceDiagnostic>| match frontmatter.get(key) {
        None => None,
        Some(Value::Bool(b)) => b.then_some(true),
        Some(other) => {
            diagnostics.push(ResourceDiagnostic::warning(
                format!(
                    "{key} must be a boolean (true or false), got \"{}\" — field ignored",
                    js_string(other)
                ),
                file_path,
            ));
            None
        }
    };
    let background = flag("background", &mut diagnostics);
    let fork = flag("fork", &mut diagnostics);

    let (mut delegate, mut delegate_to) = (None, None);
    match frontmatter.get("delegate") {
        None => {}
        Some(Value::Bool(true)) => delegate = Some(true),
        Some(Value::Bool(false)) => {}
        Some(value @ (Value::String(_) | Value::Array(_))) => {
            let joined = js_string(value);
            let mut names: Vec<String> = Vec::new();
            for n in joined.split(',').map(|s| s.trim().to_lowercase()) {
                if is_valid_name_chars(&n) && !names.contains(&n) {
                    names.push(n);
                }
            }
            if names.is_empty() {
                warn(
                    &mut diagnostics,
                    format!(
                        "delegate list \"{joined}\" contained no valid agent names — field ignored"
                    ),
                );
            } else {
                delegate = Some(true);
                delegate_to = Some(names);
            }
        }
        Some(other) => warn(
            &mut diagnostics,
            format!(
                "delegate must be a boolean or a list of agent names, got \"{}\" — field ignored",
                js_string(other)
            ),
        ),
    }

    let agent = AgentDefinition {
        name,
        description,
        tools,
        disallowed_tools,
        model,
        prompt: body.trim().to_string(),
        source,
        file_path: file_path.map(str::to_string),
        max_turns,
        background,
        delegate,
        delegate_to,
        fork,
    };
    (Some(agent), diagnostics)
}

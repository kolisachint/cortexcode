//! `extensions/core/permission-gate.ts`: the per-mode tool policy and the
//! approval prompt, as a [`PermissionGate`].

use cortexcode_agent_types::{AgentToolCall, PermissionDecision, PermissionGate};
use cortexcode_code_modes::config::{self, HooConfig};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `GATED_TOOLS`: prompted in interactive sessions.
pub const GATED_TOOLS: [&str; 5] = ["bash", "write", "edit", "webfetch", "websearch"];

/// The three prompt choices, in order.
pub const CHOICES: [&str; 3] = [
    "Yes (once)",
    "No (block)",
    "Always (add to auto-allow for this mode)",
];

/// The UI the gate prompts through (`ctx.ui.select` / `ctx.ui.notify`).
pub trait PermissionUi: Send + Sync {
    /// Show `title` with `options`; `None` = cancelled.
    fn select(&self, title: &str, options: &[&str]) -> Option<String>;
    /// An info notification.
    fn notify(&self, message: &str);
}

/// `matchesAllowedPath`: exact or `*`-glob match of the path (and, for an
/// absolute path, its cwd-relative form), all with `/` separators.
pub fn matches_allowed_path(file_path: &str, allowed: &[String], cwd: &Path) -> bool {
    if allowed.is_empty() {
        return true;
    }
    if file_path.is_empty() {
        return false;
    }
    let to_posix = |s: &str| s.replace('\\', "/");
    let mut candidates = vec![to_posix(file_path)];
    let path = Path::new(file_path);
    if path.is_absolute() {
        let rel = relative(cwd, path);
        if !rel.is_empty() && !candidates.contains(&rel) {
            candidates.push(to_posix(&rel));
        }
    }
    for pattern in allowed {
        let pattern = to_posix(pattern);
        if candidates.contains(&pattern) {
            return true;
        }
        if !pattern.contains('*') {
            continue;
        }
        let source = format!(
            "^{}$",
            pattern
                .split('*')
                .map(regex::escape)
                .collect::<Vec<_>>()
                .join(".*")
        );
        if let Ok(re) = regex::Regex::new(&source) {
            if candidates.iter().any(|c| re.is_match(c)) {
                return true;
            }
        }
    }
    false
}

/// Node's `path.relative(from, to)` for absolute paths (lexical).
fn relative(from: &Path, to: &Path) -> String {
    let norm = |p: &Path| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in p.components() {
            match c {
                std::path::Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
                std::path::Component::ParentDir => {
                    out.pop();
                }
                _ => {}
            }
        }
        out
    };
    let (a, b) = (norm(from), norm(to));
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    parts.extend(b[common..].iter().cloned());
    parts.join("/")
}

/// `matchesBashPattern`: an invalid pattern never matches.
fn matches_bash_pattern(pattern: &str, command: &str) -> bool {
    regex::Regex::new(pattern).is_ok_and(|re| re.is_match(command))
}

/// `mutationPath`: `path`, else `file_path` (non-empty strings).
pub fn mutation_path(input: &Value) -> Option<String> {
    ["path", "file_path"].iter().find_map(|k| {
        input
            .get(*k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// `describeTool`: the prompt's subject.
pub fn describe_tool(tool_name: &str, input: &Value) -> String {
    let field = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("(unknown)")
            .to_string()
    };
    match tool_name {
        "bash" => {
            // `command.replace(/\s+/g, " ").slice(0, 100)` (UTF-16 units).
            let command = input.get("command").and_then(Value::as_str).unwrap_or("");
            let mut collapsed = String::new();
            let mut in_space = false;
            for c in command.chars() {
                if c.is_whitespace() || c == '\u{feff}' {
                    if !in_space {
                        collapsed.push(' ');
                    }
                    in_space = true;
                } else {
                    collapsed.push(c);
                    in_space = false;
                }
            }
            let units: Vec<u16> = collapsed.encode_utf16().take(100).collect();
            format!("$ {}", String::from_utf16_lossy(&units))
        }
        "edit" | "write" => format!(
            "{tool_name} {}",
            mutation_path(input).unwrap_or_else(|| "(unknown)".into())
        ),
        "webfetch" => format!("webfetch {}", field("url")),
        "websearch" => format!("websearch \"{}\"", field("query")),
        _ => tool_name.to_string(),
    }
}

/// The gate's verdict before any prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Block(String),
    Allow,
    /// Ask the user.
    Prompt,
}

/// The hoocode `tool_call` handler up to the prompt, over a merged config.
pub fn evaluate(
    config: &HooConfig,
    cwd: &Path,
    tool_name: &str,
    input: &Value,
    has_ui: bool,
) -> Verdict {
    let mode = config::active_mode(config).unwrap_or_else(|| "build".into());
    let list = |key: &str| config::mode_list(config, &mode, key);

    if list("denied_tools").is_some_and(|d| d.iter().any(|t| t == tool_name)) {
        return Verdict::Block(format!(
            "Tool \"{tool_name}\" is denied in mode \"{mode}\"."
        ));
    }
    if let Some(enabled) = list("enabled_tools").filter(|e| !e.is_empty()) {
        if !enabled.iter().any(|t| t == tool_name) {
            return Verdict::Block(format!(
                "Tool \"{tool_name}\" is not enabled in mode \"{mode}\" (enabled: {}).",
                enabled.join(", ")
            ));
        }
    }
    if tool_name == "bash" {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        for pattern in list("denied_bash_commands").unwrap_or_default() {
            if matches_bash_pattern(&pattern, command) {
                return Verdict::Block(format!(
                    "Bash command matches a denied pattern in mode \"{mode}\": {pattern}"
                ));
            }
        }
        if let Some(allowed) = list("allowed_bash_commands").filter(|a| !a.is_empty()) {
            if !allowed.iter().any(|p| matches_bash_pattern(p, command)) {
                return Verdict::Block(format!(
                    "Bash command is not permitted in mode \"{mode}\". Allowed patterns: {}",
                    allowed.join(", ")
                ));
            }
        }
    }

    if !GATED_TOOLS.contains(&tool_name) || !has_ui {
        return Verdict::Allow;
    }
    if tool_name == "write" || tool_name == "edit" {
        if let Some(allowed) = list("allowed_write_paths") {
            let file_path = mutation_path(input).unwrap_or_default();
            if !matches_allowed_path(&file_path, &allowed, cwd) {
                return Verdict::Block(format!(
                    "Mode \"{mode}\" only allows writes to: {}. Attempted to {tool_name}: {file_path}. Switch to \"/mode build\" to modify source files.",
                    allowed.join(", ")
                ));
            }
        }
    }
    if list("auto_allow").is_some_and(|a| a.iter().any(|t| t == tool_name)) {
        return Verdict::Allow;
    }
    Verdict::Prompt
}

/// hoocode's permission gate for one working directory.
pub struct HooPermissionGate {
    cwd: PathBuf,
    ui: Option<Arc<dyn PermissionUi>>,
}

impl HooPermissionGate {
    /// `ui: None` is a headless session: only hard enforcement applies.
    pub fn new(cwd: PathBuf, ui: Option<Arc<dyn PermissionUi>>) -> Self {
        Self { cwd, ui }
    }

    /// The handler: `None` lets the call run, `Some(reason)` blocks it.
    pub fn check(&self, tool_name: &str, input: &Value) -> Option<String> {
        let merged = config::read_merged_config(&self.cwd);
        match evaluate(&merged, &self.cwd, tool_name, input, self.ui.is_some()) {
            Verdict::Allow => None,
            Verdict::Block(reason) => Some(reason),
            Verdict::Prompt => {
                let ui = self.ui.as_ref()?;
                let title = format!("Allow: {}", describe_tool(tool_name, input));
                let choice = ui.select(&title, &CHOICES);
                match choice {
                    None => Some("Denied by permission gate".into()),
                    Some(c) if c.starts_with("No") => Some("Denied by permission gate".into()),
                    Some(c) if c.starts_with("Always") => {
                        // "Always" goes to the global config only.
                        let mut latest = config::read_config();
                        let mode = config::active_mode(&latest).unwrap_or_else(|| "build".into());
                        let modes = latest
                            .entry("modes")
                            .or_insert_with(|| Value::Object(Default::default()));
                        if !modes.is_object() {
                            *modes = Value::Object(Default::default());
                        }
                        let entry = modes.as_object_mut().map(|m| {
                            m.entry(mode.clone())
                                .or_insert_with(|| Value::Object(Default::default()))
                        });
                        if let Some(Value::Object(mode_cfg)) = entry {
                            let mut allow: Vec<Value> = mode_cfg
                                .get("auto_allow")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            if !allow.iter().any(|v| v.as_str() == Some(tool_name)) {
                                allow.push(tool_name.into());
                            }
                            mode_cfg.insert("auto_allow".into(), Value::Array(allow));
                        }
                        let _ = config::write_config(&latest);
                        ui.notify(&format!(
                            "\"{tool_name}\" added to auto-allow for mode \"{mode}\""
                        ));
                        None
                    }
                    Some(_) => None,
                }
            }
        }
    }
}

impl PermissionGate for HooPermissionGate {
    fn request(&self, tool_call: &AgentToolCall) -> PermissionDecision {
        match self.check(&tool_call.name, &tool_call.arguments) {
            None => PermissionDecision::Grant,
            Some(reason) => PermissionDecision::Deny { reason },
        }
    }
}

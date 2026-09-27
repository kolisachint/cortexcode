//! `core/skills.ts`: skill discovery, validation and the system-prompt block.

use crate::agent_frontmatter::normalize_tools;
use crate::diagnostics::{DiagnosticType, ResourceCollision, ResourceDiagnostic};
use crate::frontmatter::parse_frontmatter;
use crate::js::{js_string, truthy};
use crate::node_path;
use crate::source_info::{create_synthetic_source_info, SourceInfo, SourceScope};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];

/// `Skill`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: String,
    pub base_dir: String,
    pub source_info: SourceInfo,
    pub disable_model_invocation: bool,
    /// Normalized tool names from `allowed-tools`.
    pub allowed_tools: Option<Vec<String>>,
}

/// `LoadSkillsResult`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// The `ignore` package matcher: gitignore rules from every visited directory,
/// prefixed with that directory's path relative to the scan root.
#[derive(Clone)]
struct IgnoreMatcher {
    lines: Vec<String>,
    matcher: ignore::gitignore::Gitignore,
}

impl IgnoreMatcher {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            matcher: ignore::gitignore::Gitignore::empty(),
        }
    }

    fn add(&mut self, patterns: Vec<String>) {
        self.lines.extend(patterns);
        let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
        for line in &self.lines {
            let _ = builder.add_line(None, line);
        }
        if let Ok(matcher) = builder.build() {
            self.matcher = matcher;
        }
    }

    /// `ignores(relPath)`; a trailing `/` marks a directory.
    fn ignores(&self, rel_path: &str) -> bool {
        if self.lines.is_empty() || rel_path.is_empty() {
            return false;
        }
        let is_dir = rel_path.ends_with('/');
        let path = format!("/{}", rel_path.trim_end_matches('/'));
        self.matcher
            .matched_path_or_any_parents(&path, is_dir)
            .is_ignore()
    }
}

fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || (trimmed.starts_with('#') && !trimmed.starts_with("\\#")) {
        return None;
    }
    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if pattern.starts_with("\\!") {
        pattern = &pattern[1..];
    }
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
    let prefixed = format!("{prefix}{pattern}");
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

fn add_ignore_rules(matcher: &mut IgnoreMatcher, dir: &str, root: &str) {
    let relative = node_path::relative(root, dir);
    let prefix = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };
    for name in IGNORE_FILE_NAMES {
        let Ok(content) = std::fs::read_to_string(node_path::join(&[dir, name])) else {
            continue;
        };
        let patterns: Vec<String> = content
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter_map(|line| prefix_ignore_pattern(line, &prefix))
            .collect();
        if !patterns.is_empty() {
            matcher.add(patterns);
        }
    }
}

/// JS `string.length`.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn validate_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            "name \"{name}\" does not match parent directory \"{parent_dir_name}\""
        ));
    }
    if js_len(name) > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({})",
            js_len(name)
        ));
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
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

fn validate_description(description: Option<&str>) -> Vec<String> {
    match description {
        None => vec!["description is required".into()],
        Some(d) if d.trim().is_empty() => vec!["description is required".into()],
        Some(d) if js_len(d) > MAX_DESCRIPTION_LENGTH => vec![format!(
            "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})",
            js_len(d)
        )],
        Some(_) => Vec::new(),
    }
}

fn create_skill_source_info(file_path: &str, base_dir: &str, source: &str) -> SourceInfo {
    let (source, scope) = match source {
        "user" | "claude-user" => ("local", Some(SourceScope::User)),
        "project" | "claude-project" => ("local", Some(SourceScope::Project)),
        "path" => ("local", None),
        other => (other, None),
    };
    create_synthetic_source_info(file_path, source, scope, None, Some(base_dir))
}

/// `isPluginRoot` (plugins/formats): a directory carrying a plugin manifest in
/// any supported format (native, Claude, Copilot). Only its presence as valid
/// JSON matters.
fn is_plugin_root(root: &str) -> bool {
    const MANIFESTS: [&str; 6] = [
        ".agents-plugin/plugin.json",
        ".claude-plugin/plugin.json",
        ".plugin/plugin.json",
        "plugin.json",
        ".github/plugin/plugin.json",
        ".github/copilot-plugin.json",
    ];
    MANIFESTS.iter().any(|rel| {
        std::fs::read_to_string(node_path::join(&[root, rel]))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .is_some_and(|v| !v.is_null())
    })
}

/// `loadSkillFromFile`.
fn load_skill_from_file(file_path: &str, source: &str) -> (Option<Skill>, Vec<ResourceDiagnostic>) {
    let mut diagnostics = Vec::new();
    let warn = |message: String| ResourceDiagnostic::warning(message, Some(file_path));
    let raw = match std::fs::read_to_string(file_path) {
        Ok(raw) => raw,
        Err(e) => return (None, vec![warn(e.to_string())]),
    };
    let (frontmatter, _body) = match parse_frontmatter(&raw) {
        Ok(parsed) => parsed,
        Err(message) => return (None, vec![warn(message)]),
    };
    let skill_dir = node_path::dirname(file_path);
    let parent_dir_name = node_path::basename(&skill_dir);

    // A non-string description makes hoocode's `.trim()` throw; the catch turns
    // the TypeError into the only diagnostic.
    let description = match frontmatter.get("description") {
        Some(Value::String(s)) => Some(s.as_str()),
        other if truthy(other) => {
            return (
                None,
                vec![warn(
                    "frontmatter.description.trim is not a function".into(),
                )],
            )
        }
        _ => None,
    };
    diagnostics.extend(validate_description(description).into_iter().map(warn));

    let name = match frontmatter.get("name") {
        v if truthy(v) => js_string(v.unwrap()),
        _ => parent_dir_name.clone(),
    };
    diagnostics.extend(validate_name(&name, &parent_dir_name).into_iter().map(warn));

    let Some(description) = description.filter(|d| !d.trim().is_empty()) else {
        return (None, diagnostics);
    };

    let allowed_tools = match frontmatter.get("allowed-tools") {
        None => None,
        Some(value) => {
            let (tools, d) = normalize_tools(value, Some(file_path));
            diagnostics.extend(d);
            (!tools.is_empty()).then_some(tools)
        }
    };

    let skill = Skill {
        name,
        description: description.to_string(),
        file_path: file_path.to_string(),
        base_dir: skill_dir.clone(),
        source_info: create_skill_source_info(file_path, &skill_dir, source),
        disable_model_invocation: frontmatter.get("disable-model-invocation")
            == Some(&Value::Bool(true)),
        allowed_tools,
    };
    (Some(skill), diagnostics)
}

/// Whether a directory entry is a file / directory, following symlinks
/// (`None` for a broken link).
fn entry_kind(entry: &std::fs::DirEntry, full_path: &str) -> Option<(bool, bool)> {
    let file_type = entry.file_type().ok()?;
    if file_type.is_symlink() {
        let meta = std::fs::metadata(full_path).ok()?;
        return Some((meta.is_file(), meta.is_dir()));
    }
    Some((file_type.is_file(), file_type.is_dir()))
}

/// `loadSkillsFromDirInternal`: a `SKILL.md` makes the directory one skill and
/// stops the descent; otherwise subdirectories are walked (skipping dot entries,
/// `node_modules`, ignored paths and plugin roots) and, at the root only, direct
/// `.md` files are skills.
fn load_skills_from_dir_internal(
    dir: &str,
    source: &str,
    include_root_files: bool,
    matcher: Option<&mut IgnoreMatcher>,
    root_dir: Option<&str>,
) -> LoadSkillsResult {
    let mut result = LoadSkillsResult::default();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return result;
    };
    let root = root_dir.unwrap_or(dir).to_string();
    let mut own_matcher;
    let matcher = match matcher {
        Some(m) => m,
        None => {
            own_matcher = IgnoreMatcher::new();
            &mut own_matcher
        }
    };
    add_ignore_rules(matcher, dir, &root);

    // `readdirSync` order: the directory's own order, which Node returns sorted
    // on common filesystems; sort for determinism.
    let mut entries: Vec<std::fs::DirEntry> = read_dir.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in &entries {
        if entry.file_name() != "SKILL.md" {
            continue;
        }
        let full_path = node_path::join(&[dir, "SKILL.md"]);
        let Some((is_file, _)) = entry_kind(entry, &full_path) else {
            continue;
        };
        let rel_path = node_path::relative(&root, &full_path);
        if !is_file || matcher.ignores(&rel_path) {
            continue;
        }
        let (skill, diagnostics) = load_skill_from_file(&full_path, source);
        result.skills.extend(skill);
        result.diagnostics.extend(diagnostics);
        return result;
    }

    for entry in &entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let full_path = node_path::join(&[dir, &name]);
        let Some((is_file, is_dir)) = entry_kind(entry, &full_path) else {
            continue;
        };
        let rel_path = node_path::relative(&root, &full_path);
        let ignore_path = if is_dir {
            format!("{rel_path}/")
        } else {
            rel_path
        };
        if matcher.ignores(&ignore_path) {
            continue;
        }
        if is_dir {
            if is_plugin_root(&full_path) {
                continue;
            }
            let sub = load_skills_from_dir_internal(
                &full_path,
                source,
                false,
                Some(&mut *matcher),
                Some(&root),
            );
            result.skills.extend(sub.skills);
            result.diagnostics.extend(sub.diagnostics);
            continue;
        }
        if !is_file || !include_root_files || !name.ends_with(".md") {
            continue;
        }
        let (skill, diagnostics) = load_skill_from_file(&full_path, source);
        result.skills.extend(skill);
        result.diagnostics.extend(diagnostics);
    }
    result
}

/// `loadSkillsFromDir`.
pub fn load_skills_from_dir(dir: &str, source: &str) -> LoadSkillsResult {
    load_skills_from_dir_internal(dir, source, true, None, None)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `formatSkillsForPrompt`: the `<available_skills>` block (model-invocable
/// skills only), or `""`.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".into(),
        "Use the read tool to load a skill's file when the task matches its description.".into(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".into(),
        String::new(),
        "<available_skills>".into(),
    ];
    for skill in visible {
        lines.push("  <skill>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        if let Some(tools) = skill.allowed_tools.as_ref().filter(|t| !t.is_empty()) {
            lines.push(format!(
                "    <tools>{}</tools>",
                escape_xml(&tools.join(", "))
            ));
        }
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".into());
    }
    lines.push("</available_skills>".into());
    lines.join("\n")
}

/// `LoadSkillsOptions`.
#[derive(Debug, Clone, Default)]
pub struct LoadSkillsOptions {
    pub cwd: String,
    pub agent_dir: String,
    pub skill_paths: Vec<String>,
    pub include_defaults: bool,
    /// Discover `.claude/skills` (default true in hoocode).
    pub include_claude: bool,
    /// Contributed skill directory -> owning plugin id (`<plugin>:<skill>` names).
    pub namespaces: HashMap<String, String>,
}

impl LoadSkillsOptions {
    /// hoocode's defaults: defaults and `.claude/skills` included.
    pub fn new(cwd: impl Into<String>, agent_dir: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            agent_dir: agent_dir.into(),
            skill_paths: Vec::new(),
            include_defaults: true,
            include_claude: true,
            namespaces: HashMap::new(),
        }
    }
}

/// `loadSkills`: `~/.claude/skills`, `<cwd>/.claude/skills`, `<agentDir>/skills`,
/// `<cwd>/.cortexcode/skills`, then explicit paths. The first skill with a name
/// wins; later ones become collision diagnostics (listed after the others).
/// The same file reached twice (symlinks) is skipped silently.
pub fn load_skills(options: &LoadSkillsOptions) -> LoadSkillsResult {
    let cwd = options.cwd.as_str();
    let agent_dir = options.agent_dir.as_str();
    let mut skill_map: Vec<Skill> = Vec::new();
    let mut real_paths: HashSet<String> = HashSet::new();
    let mut all_diagnostics = Vec::new();
    let mut collision_diagnostics = Vec::new();

    let mut add_skills = |result: LoadSkillsResult,
                          skill_map: &mut Vec<Skill>,
                          all_diagnostics: &mut Vec<ResourceDiagnostic>| {
        all_diagnostics.extend(result.diagnostics);
        for skill in result.skills {
            let real_path = node_path::canonicalize(&skill.file_path);
            if real_paths.contains(&real_path) {
                continue;
            }
            if let Some(existing) = skill_map.iter().find(|s| s.name == skill.name) {
                collision_diagnostics.push(ResourceDiagnostic {
                    kind: DiagnosticType::Collision,
                    message: format!("name \"{}\" collision", skill.name),
                    path: Some(skill.file_path.clone()),
                    collision: Some(ResourceCollision {
                        resource_type: "skill".into(),
                        name: skill.name.clone(),
                        winner_path: existing.file_path.clone(),
                        loser_path: skill.file_path.clone(),
                        winner_source: None,
                        loser_source: None,
                    }),
                });
            } else {
                real_paths.insert(real_path);
                skill_map.push(skill);
            }
        }
    };

    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    let user_skills_dir = node_path::join(&[agent_dir, "skills"]);
    let project_skills_dir = node_path::resolve(
        cwd,
        &format!("{}/skills", cortexcode_code_paths::CONFIG_DIR_NAME),
    );

    if options.include_defaults {
        if options.include_claude {
            add_skills(
                load_skills_from_dir_internal(
                    &node_path::join(&[&home, ".claude", "skills"]),
                    "claude-user",
                    true,
                    None,
                    None,
                ),
                &mut skill_map,
                &mut all_diagnostics,
            );
            add_skills(
                load_skills_from_dir_internal(
                    &node_path::resolve(cwd, ".claude/skills"),
                    "claude-project",
                    true,
                    None,
                    None,
                ),
                &mut skill_map,
                &mut all_diagnostics,
            );
        }
        add_skills(
            load_skills_from_dir_internal(&user_skills_dir, "user", true, None, None),
            &mut skill_map,
            &mut all_diagnostics,
        );
        add_skills(
            load_skills_from_dir_internal(&project_skills_dir, "project", true, None, None),
            &mut skill_map,
            &mut all_diagnostics,
        );
    }

    let get_source = |resolved: &str| -> &'static str {
        if !options.include_defaults {
            if node_path::is_under_path(resolved, &user_skills_dir) {
                return "user";
            }
            if node_path::is_under_path(resolved, &project_skills_dir) {
                return "project";
            }
        }
        "path"
    };

    for raw_path in &options.skill_paths {
        let resolved = node_path::resolve_config_path(raw_path, cwd);
        let namespace = options
            .namespaces
            .get(raw_path)
            .or_else(|| options.namespaces.get(&resolved))
            .cloned();
        let namespaced = |mut result: LoadSkillsResult| {
            if let Some(ns) = &namespace {
                for skill in &mut result.skills {
                    skill.name = format!("{ns}:{}", skill.name);
                }
            }
            result
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path does not exist",
                Some(&resolved),
            ));
            continue;
        };
        let source = get_source(&resolved);
        if meta.is_dir() {
            add_skills(
                namespaced(load_skills_from_dir_internal(
                    &resolved, source, true, None, None,
                )),
                &mut skill_map,
                &mut all_diagnostics,
            );
        } else if meta.is_file() && resolved.ends_with(".md") {
            let (skill, diagnostics) = load_skill_from_file(&resolved, source);
            match skill {
                Some(skill) => add_skills(
                    namespaced(LoadSkillsResult {
                        skills: vec![skill],
                        diagnostics,
                    }),
                    &mut skill_map,
                    &mut all_diagnostics,
                ),
                None => all_diagnostics.extend(diagnostics),
            }
        } else {
            all_diagnostics.push(ResourceDiagnostic::warning(
                "skill path is not a markdown file",
                Some(&resolved),
            ));
        }
    }

    all_diagnostics.extend(collision_diagnostics);
    LoadSkillsResult {
        skills: skill_map,
        diagnostics: all_diagnostics,
    }
}

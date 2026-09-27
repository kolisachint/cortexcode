//! `core/agent-registry.ts` + `agent-manifest-paths.ts`: subagent definitions
//! from built-ins, the user and project directories (Claude Code compatible),
//! explicit paths and `--agent`, later sources overriding earlier ones by name.

use crate::agent_frontmatter::{parse_agent_definition, AgentDefinition, AgentSource};
use crate::diagnostics::{DiagnosticType, ResourceCollision, ResourceDiagnostic};
use crate::node_path;
use std::sync::RwLock;

/// `EMBEDDED_AGENT_PROMPTS` (templates/agents/*.md, in file-name order).
pub const EMBEDDED_AGENT_PROMPTS: &[(&str, &str)] = &[
    (
        "code-review",
        include_str!("../templates/agents/code-review.md"),
    ),
    ("explore", include_str!("../templates/agents/explore.md")),
    (
        "general-purpose",
        include_str!("../templates/agents/general-purpose.md"),
    ),
    ("plan", include_str!("../templates/agents/plan.md")),
    (
        "security-review",
        include_str!("../templates/agents/security-review.md"),
    ),
];

static MANIFEST_PATHS: RwLock<Vec<String>> = RwLock::new(Vec::new());
static CLI_PATHS: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// `setAgentManifestPaths` (package `agents` manifests, refreshed on reload).
pub fn set_agent_manifest_paths(paths: Vec<String>) {
    *MANIFEST_PATHS.write().unwrap_or_else(|e| e.into_inner()) = paths;
}

/// `getAgentManifestPaths`.
pub fn get_agent_manifest_paths() -> Vec<String> {
    MANIFEST_PATHS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// `setAgentCliPaths` (`--agent <path>`, set once at startup).
pub fn set_agent_cli_paths(paths: Vec<String>) {
    *CLI_PATHS.write().unwrap_or_else(|e| e.into_inner()) = paths;
}

/// `getAgentCliPaths`.
pub fn get_agent_cli_paths() -> Vec<String> {
    CLI_PATHS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

fn source_name(source: AgentSource) -> &'static str {
    match source {
        AgentSource::Builtin => "builtin",
        AgentSource::User => "user",
        AgentSource::Project => "project",
        AgentSource::ClaudeUser => "claude-user",
        AgentSource::ClaudeProject => "claude-project",
    }
}

/// `AgentRegistry`: definitions by name, in first-registration order.
#[derive(Debug, Clone, Default)]
pub struct AgentRegistry {
    agents: Vec<AgentDefinition>,
    diagnostics: Vec<ResourceDiagnostic>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `register`: later registrations win; an override records a collision.
    pub fn register(&mut self, def: AgentDefinition) {
        if let Some(i) = self.agents.iter().position(|a| a.name == def.name) {
            let existing = &self.agents[i];
            let label = |a: &AgentDefinition| {
                a.file_path
                    .clone()
                    .unwrap_or_else(|| format!("<{}>", source_name(a.source)))
            };
            self.diagnostics.push(ResourceDiagnostic {
                kind: DiagnosticType::Collision,
                message: format!(
                    "agent \"{}\" from {} overrides {}",
                    def.name,
                    source_name(def.source),
                    source_name(existing.source)
                ),
                path: def.file_path.clone(),
                collision: Some(ResourceCollision {
                    resource_type: "skill".into(),
                    name: def.name.clone(),
                    winner_path: label(&def),
                    loser_path: label(existing),
                    winner_source: Some(source_name(def.source).into()),
                    loser_source: Some(source_name(existing.source).into()),
                }),
            });
            // A JS Map keeps the original insertion position on `set`.
            self.agents[i] = def;
        } else {
            self.agents.push(def);
        }
    }

    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.iter().find(|a| a.name == name)
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn list(&self) -> &[AgentDefinition] {
        &self.agents
    }

    pub fn diagnostics(&self) -> &[ResourceDiagnostic] {
        &self.diagnostics
    }

    pub fn add_diagnostics(&mut self, diagnostics: impl IntoIterator<Item = ResourceDiagnostic>) {
        self.diagnostics.extend(diagnostics);
    }

    fn register_parsed(
        &mut self,
        raw: &str,
        source: AgentSource,
        file_path: Option<&str>,
        fallback: Option<&str>,
    ) {
        let (agent, diagnostics) = parse_agent_definition(raw, source, file_path, fallback);
        self.add_diagnostics(diagnostics);
        if let Some(agent) = agent {
            self.register(agent);
        }
    }

    /// `registerFile`.
    fn register_file(&mut self, file_path: &str, source: AgentSource) {
        match std::fs::metadata(file_path) {
            Ok(m) if m.is_file() => match std::fs::read_to_string(file_path) {
                Ok(raw) => self.register_parsed(&raw, source, Some(file_path), None),
                Err(e) => self
                    .add_diagnostics([ResourceDiagnostic::warning(e.to_string(), Some(file_path))]),
            },
            _ => {}
        }
    }

    /// `registerDir`: flat `*.md` files (no dot files, no subdirectories).
    fn register_dir(&mut self, dir: &str, source: AgentSource) {
        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<String> = read_dir
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for name in names {
            if name.starts_with('.') || !name.ends_with(".md") {
                continue;
            }
            self.register_file(&node_path::join(&[dir, &name]), source);
        }
    }

    fn register_path(&mut self, path: &str, dir_source: AgentSource, file_source: AgentSource) {
        match std::fs::metadata(path) {
            Err(_) => self.add_diagnostics([ResourceDiagnostic::warning(
                format!("Agent path does not exist: {path}"),
                Some(path),
            )]),
            Ok(m) if m.is_dir() => self.register_dir(path, dir_source),
            Ok(_) => self.register_file(path, file_source),
        }
    }
}

fn find_git_repo_root(start: &str) -> Option<String> {
    let mut dir = node_path::resolve("/", start);
    loop {
        if std::path::Path::new(&node_path::join(&[&dir, ".git"])).exists() {
            return Some(dir);
        }
        let parent = node_path::dirname(&dir);
        if parent == dir {
            return None;
        }
        dir = parent;
    }
}

/// `.agents/agents` directories from `start` up to the git root (cwd first).
fn collect_ancestor_agents_dirs(start: &str) -> Vec<String> {
    let start = node_path::resolve("/", start);
    let git_root = find_git_repo_root(&start);
    let mut dirs = Vec::new();
    let mut dir = start;
    loop {
        dirs.push(node_path::join(&[&dir, ".agents", "agents"]));
        if git_root.as_deref() == Some(dir.as_str()) {
            break;
        }
        let parent = node_path::dirname(&dir);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    dirs
}

/// `LoadAgentRegistryOptions`.
#[derive(Debug, Clone)]
pub struct LoadAgentRegistryOptions {
    pub cwd: String,
    pub agent_dir: Option<String>,
    pub include_builtins: bool,
    pub include_claude: bool,
    pub agent_paths: Vec<String>,
}

impl LoadAgentRegistryOptions {
    /// hoocode's defaults: built-ins and `.claude/agents` included.
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            agent_dir: None,
            include_builtins: true,
            include_claude: true,
            agent_paths: Vec::new(),
        }
    }
}

/// `loadAgentRegistry`: built-ins, package manifests, `~/.claude/agents`,
/// `<agentDir>/agents`, ancestor `.agents/agents` (git root first),
/// `<cwd>/.claude/agents`, `<cwd>/.cortexcode/agents`, explicit paths, `--agent`.
pub fn load_agent_registry(options: &LoadAgentRegistryOptions) -> AgentRegistry {
    let cwd = options.cwd.as_str();
    let agent_dir = options.agent_dir.clone().unwrap_or_else(|| {
        cortexcode_code_paths::agent_dir()
            .to_string_lossy()
            .into_owned()
    });
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    let mut registry = AgentRegistry::new();

    if options.include_builtins {
        for (key, raw) in EMBEDDED_AGENT_PROMPTS {
            registry.register_parsed(raw, AgentSource::Builtin, None, Some(key));
        }
    }
    for path in get_agent_manifest_paths() {
        registry.register_file(&path, AgentSource::User);
    }
    if options.include_claude {
        registry.register_dir(
            &node_path::join(&[&home, ".claude", "agents"]),
            AgentSource::ClaudeUser,
        );
    }
    registry.register_dir(&node_path::join(&[&agent_dir, "agents"]), AgentSource::User);
    for dir in collect_ancestor_agents_dirs(cwd).into_iter().rev() {
        registry.register_dir(&dir, AgentSource::Project);
    }
    if options.include_claude {
        registry.register_dir(
            &node_path::resolve(cwd, ".claude/agents"),
            AgentSource::ClaudeProject,
        );
    }
    registry.register_dir(
        &node_path::resolve(
            cwd,
            &format!("{}/agents", cortexcode_code_paths::CONFIG_DIR_NAME),
        ),
        AgentSource::Project,
    );
    for raw in &options.agent_paths {
        let path = node_path::resolve(cwd, &node_path::expand_home(raw));
        registry.register_path(&path, AgentSource::Project, AgentSource::Project);
    }
    for path in get_agent_cli_paths() {
        registry.register_path(&path, AgentSource::User, AgentSource::User);
    }
    registry
}

/// `summarizeAgentDescription`: the first "when to use" bullets (up to three),
/// else the first prose line, capped at 200 characters.
pub fn summarize_agent_description(description: &str) -> String {
    let lines: Vec<&str> = description
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let is_stop = |line: &str| {
        let lower = line.to_lowercase();
        let starts_word = |prefix: &str| {
            lower.strip_prefix(prefix).is_some_and(|rest| {
                !rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        };
        // /^(do\s*not|don'?t|avoid)\b/i
        let do_not = lower.strip_prefix("do").is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.strip_prefix("not").is_some_and(|r| {
                !r.chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        });
        do_not || starts_word("don't") || starts_word("dont") || starts_word("avoid")
    };
    let region: &[&str] = match lines.iter().position(|l| is_stop(l)) {
        Some(stop) => &lines[..stop],
        None => &lines,
    };
    let body: &[&str] = if region.len() > 1 && region[0].ends_with(':') {
        &region[1..]
    } else {
        region
    };
    let bullet = |line: &str| -> Option<String> {
        let rest = line.strip_prefix(['-', '*', '•'])?;
        let trimmed = rest.trim_start();
        (trimmed.len() < rest.len()).then(|| trimmed.trim().to_string())
    };
    let bullets: Vec<String> = body
        .iter()
        .filter_map(|l| bullet(l))
        .filter(|l| !l.is_empty())
        .collect();
    let summary = if bullets.is_empty() {
        let first = body.first().or(lines.first()).copied().unwrap_or("");
        first.strip_suffix(':').unwrap_or(first).to_string()
    } else {
        bullets[..bullets.len().min(3)].join("; ")
    };
    const MAX: usize = 200;
    let units: Vec<u16> = summary.encode_utf16().collect();
    if units.len() > MAX {
        let cut = String::from_utf16_lossy(&units[..MAX - 1]);
        format!("{}…", cut.trim_end())
    } else {
        summary
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `formatAgentsForPrompt`: the `<available_agents>` block, or `""`.
pub fn format_agents_for_prompt(agents: &[AgentDefinition]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = vec![
        "\n\nThe following specialized agents are available for delegation via the Task tool."
            .into(),
        "Choose the agent whose description best matches the task and pass it as `subagent_type`."
            .into(),
        String::new(),
        "<available_agents>".into(),
    ];
    for agent in agents {
        lines.push("  <agent>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&agent.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&summarize_agent_description(&agent.description))
        ));
        if let Some(tools) = agent.tools.as_ref().filter(|t| !t.is_empty()) {
            lines.push(format!(
                "    <tools>{}</tools>",
                escape_xml(&tools.join(", "))
            ));
        }
        if let Some(model) = &agent.model {
            lines.push(format!("    <model>{}</model>", escape_xml(model)));
        }
        lines.push("  </agent>".into());
    }
    lines.push("</available_agents>".into());
    lines.join("\n")
}

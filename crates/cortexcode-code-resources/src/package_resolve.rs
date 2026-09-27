//! The local half of `DefaultPackageManager.resolve()` (core/package-manager.ts):
//! resource entries from the global and project settings, the auto-discovered
//! directories (`<agentDir>/…`, `.cortexcode/…`, `.claude/skills`,
//! `.agents/skills`), override patterns, then precedence order with symlink
//! duplicates removed. Package sources (`packages`: npm / git) are ledger 12.2.

use crate::node_path;
use crate::package_discovery::{
    apply_patterns, collect_ancestor_agents_skill_dirs, collect_auto_extension_entries,
    collect_auto_prompt_entries, collect_auto_skill_entries, collect_auto_theme_entries,
    collect_resource_files, is_enabled_by_overrides, split_patterns, ResourceType,
    SkillDiscoveryMode, SETTINGS_RESOURCE_TYPES,
};
use crate::source_info::{SourceInfo, SourceOrigin, SourceScope};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// `PathMetadata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMetadata {
    /// `"local"` (settings entry), `"auto"` (discovered), `"cli"`, or a package source.
    pub source: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    pub base_dir: Option<String>,
    /// Plugin id for `<plugin>:<skill>` names.
    pub namespace: Option<String>,
}

impl PathMetadata {
    pub fn new(
        source: &str,
        scope: SourceScope,
        origin: SourceOrigin,
        base_dir: Option<&str>,
    ) -> Self {
        Self {
            source: source.into(),
            scope,
            origin,
            base_dir: base_dir.map(str::to_string),
            namespace: None,
        }
    }

    /// `createSourceInfo(path, metadata)`.
    pub fn source_info(&self, path: &str) -> SourceInfo {
        SourceInfo {
            path: path.to_string(),
            source: self.source.clone(),
            scope: self.scope,
            origin: self.origin,
            base_dir: self.base_dir.clone(),
        }
    }
}

/// `ResolvedResource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedResource {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
}

/// `ResolvedPaths`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPaths {
    pub extensions: Vec<ResolvedResource>,
    pub skills: Vec<ResolvedResource>,
    pub prompts: Vec<ResolvedResource>,
    pub themes: Vec<ResolvedResource>,
    pub agents: Vec<ResolvedResource>,
}

impl ResolvedPaths {
    pub fn get(&self, resource_type: ResourceType) -> &[ResolvedResource] {
        match resource_type {
            ResourceType::Extensions => &self.extensions,
            ResourceType::Skills => &self.skills,
            ResourceType::Prompts => &self.prompts,
            ResourceType::Themes => &self.themes,
            ResourceType::Agents => &self.agents,
        }
    }
}

/// First-added wins per path (a JS `Map` keyed by path, insertion ordered).
#[derive(Default)]
struct Target(Vec<ResolvedResource>);

impl Target {
    fn add(&mut self, path: String, metadata: &PathMetadata, enabled: bool) {
        if path.is_empty() || self.0.iter().any(|r| r.path == path) {
            return;
        }
        self.0.push(ResolvedResource {
            path,
            enabled,
            metadata: metadata.clone(),
        });
    }
}

#[derive(Default)]
struct Accumulator {
    extensions: Target,
    skills: Target,
    prompts: Target,
    themes: Target,
    agents: Target,
}

impl Accumulator {
    fn target(&mut self, resource_type: ResourceType) -> &mut Target {
        match resource_type {
            ResourceType::Extensions => &mut self.extensions,
            ResourceType::Skills => &mut self.skills,
            ResourceType::Prompts => &mut self.prompts,
            ResourceType::Themes => &mut self.themes,
            ResourceType::Agents => &mut self.agents,
        }
    }
}

/// `resourcePrecedenceRank`: project settings, project auto, project
/// `.claude`, then the same for user, then packages.
fn precedence_rank(m: &PathMetadata) -> u8 {
    if m.origin == SourceOrigin::Package {
        return 6;
    }
    let scope_base = if m.scope == SourceScope::Project {
        0
    } else {
        3
    };
    let source_rank = if m.source == "local" {
        0
    } else if m.origin == SourceOrigin::ClaudeCode {
        2
    } else {
        1
    };
    scope_base + source_rank
}

/// Inputs of [`resolve_local_resources`].
#[derive(Debug, Clone)]
pub struct ResolveOptions<'a> {
    pub cwd: &'a str,
    pub agent_dir: &'a str,
    /// `getHomeDir()`: `$HOME`, else the OS home.
    pub home: String,
    pub global_settings: &'a Map<String, Value>,
    pub project_settings: &'a Map<String, Value>,
}

/// `getHomeDir`.
pub fn home_dir() -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into())
        })
}

fn string_list(settings: &Map<String, Value>, key: &str) -> Vec<String> {
    settings
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `resolvePathFromBase`: `~` against home, else relative to `base_dir`.
fn resolve_from_base(input: &str, base_dir: &str, home: &str) -> String {
    let trimmed = input.trim();
    if trimmed == "~" {
        return home.to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return node_path::join(&[home, rest]);
    }
    if let Some(rest) = trimmed.strip_prefix('~') {
        return node_path::join(&[home, rest]);
    }
    node_path::resolve(base_dir, trimmed)
}

/// `collectFilesFromPaths`.
fn collect_files_from_paths(paths: &[String], resource_type: ResourceType) -> Vec<String> {
    let mut files = Vec::new();
    for p in paths {
        match std::fs::metadata(p) {
            Ok(m) if m.is_file() => files.push(p.clone()),
            Ok(m) if m.is_dir() => files.extend(collect_resource_files(p, resource_type)),
            _ => {}
        }
    }
    files
}

/// `resolveLocalEntries`: plain entries collect files; patterns decide which
/// are enabled.
fn resolve_local_entries(
    entries: &[String],
    resource_type: ResourceType,
    target: &mut Target,
    metadata: &PathMetadata,
    base_dir: &str,
    home: &str,
) {
    if entries.is_empty() {
        return;
    }
    let (plain, patterns) = split_patterns(entries);
    let resolved: Vec<String> = plain
        .iter()
        .map(|p| resolve_from_base(p, base_dir, home))
        .collect();
    let all_files = collect_files_from_paths(&resolved, resource_type);
    let enabled = apply_patterns(&all_files, &patterns, base_dir);
    for f in all_files {
        let on = enabled.contains(&f);
        target.add(f, metadata, on);
    }
}

/// `DefaultPackageManager.resolve()` without package sources.
pub fn resolve_local_resources(options: &ResolveOptions<'_>) -> ResolvedPaths {
    let mut acc = Accumulator::default();
    let home = options.home.as_str();
    let global_base = options.agent_dir.to_string();
    let project_base = node_path::join(&[options.cwd, cortexcode_code_paths::CONFIG_DIR_NAME]);

    for resource_type in SETTINGS_RESOURCE_TYPES {
        let key = resource_type.key();
        let project_meta =
            PathMetadata::new("local", SourceScope::Project, SourceOrigin::TopLevel, None);
        let user_meta = PathMetadata::new("local", SourceScope::User, SourceOrigin::TopLevel, None);
        resolve_local_entries(
            &string_list(options.project_settings, key),
            resource_type,
            acc.target(resource_type),
            &project_meta,
            &project_base,
            home,
        );
        resolve_local_entries(
            &string_list(options.global_settings, key),
            resource_type,
            acc.target(resource_type),
            &user_meta,
            &global_base,
            home,
        );
    }

    add_auto_discovered(&mut acc, options, &global_base, &project_base);
    to_resolved_paths(acc)
}

fn add_auto_discovered(
    acc: &mut Accumulator,
    options: &ResolveOptions<'_>,
    global_base: &str,
    project_base: &str,
) {
    let home = options.home.as_str();
    let user_meta = PathMetadata::new(
        "auto",
        SourceScope::User,
        SourceOrigin::TopLevel,
        Some(global_base),
    );
    let project_meta = PathMetadata::new(
        "auto",
        SourceScope::Project,
        SourceOrigin::TopLevel,
        Some(project_base),
    );
    let overrides = |settings: &Map<String, Value>, t: ResourceType| string_list(settings, t.key());
    let user_agents_skills = node_path::join(&[home, ".agents", "skills"]);
    let project_agents_skill_dirs: Vec<String> = collect_ancestor_agents_skill_dirs(options.cwd)
        .into_iter()
        .filter(|d| node_path::resolve("/", d) != node_path::resolve("/", &user_agents_skills))
        .collect();

    let add = |acc: &mut Accumulator,
               t: ResourceType,
               paths: Vec<String>,
               meta: &PathMetadata,
               overrides: &[String],
               base: &str| {
        for path in paths {
            let enabled = is_enabled_by_overrides(&path, overrides, base);
            acc.target(t).add(path, meta, enabled);
        }
    };

    let project = options.project_settings;
    let global = options.global_settings;
    let dir = |base: &str, sub: &str| node_path::join(&[base, sub]);

    add(
        acc,
        ResourceType::Extensions,
        collect_auto_extension_entries(&dir(project_base, "extensions")),
        &project_meta,
        &overrides(project, ResourceType::Extensions),
        project_base,
    );
    add(
        acc,
        ResourceType::Skills,
        collect_auto_skill_entries(&dir(project_base, "skills"), SkillDiscoveryMode::Hoocode),
        &project_meta,
        &overrides(project, ResourceType::Skills),
        project_base,
    );
    let claude_project_base = node_path::resolve(options.cwd, ".claude");
    let claude_project_meta = PathMetadata::new(
        "auto",
        SourceScope::Project,
        SourceOrigin::ClaudeCode,
        Some(&claude_project_base),
    );
    add(
        acc,
        ResourceType::Skills,
        collect_auto_skill_entries(
            &dir(&claude_project_base, "skills"),
            SkillDiscoveryMode::Hoocode,
        ),
        &claude_project_meta,
        &overrides(project, ResourceType::Skills),
        &claude_project_base,
    );
    for agents_skills in &project_agents_skill_dirs {
        let agents_base = node_path::dirname(agents_skills);
        let meta = PathMetadata {
            base_dir: Some(agents_base.clone()),
            ..project_meta.clone()
        };
        add(
            acc,
            ResourceType::Skills,
            collect_auto_skill_entries(agents_skills, SkillDiscoveryMode::Agents),
            &meta,
            &overrides(project, ResourceType::Skills),
            &agents_base,
        );
    }
    add(
        acc,
        ResourceType::Prompts,
        collect_auto_prompt_entries(&dir(project_base, "prompts")),
        &project_meta,
        &overrides(project, ResourceType::Prompts),
        project_base,
    );
    add(
        acc,
        ResourceType::Themes,
        collect_auto_theme_entries(&dir(project_base, "themes")),
        &project_meta,
        &overrides(project, ResourceType::Themes),
        project_base,
    );

    add(
        acc,
        ResourceType::Extensions,
        collect_auto_extension_entries(&dir(global_base, "extensions")),
        &user_meta,
        &overrides(global, ResourceType::Extensions),
        global_base,
    );
    add(
        acc,
        ResourceType::Skills,
        collect_auto_skill_entries(&dir(global_base, "skills"), SkillDiscoveryMode::Hoocode),
        &user_meta,
        &overrides(global, ResourceType::Skills),
        global_base,
    );
    let claude_user_base = node_path::join(&[home, ".claude"]);
    let claude_user_meta = PathMetadata::new(
        "auto",
        SourceScope::User,
        SourceOrigin::ClaudeCode,
        Some(&claude_user_base),
    );
    add(
        acc,
        ResourceType::Skills,
        collect_auto_skill_entries(
            &dir(&claude_user_base, "skills"),
            SkillDiscoveryMode::Hoocode,
        ),
        &claude_user_meta,
        &overrides(global, ResourceType::Skills),
        &claude_user_base,
    );
    let user_agents_base = node_path::dirname(&user_agents_skills);
    let user_agents_meta = PathMetadata {
        base_dir: Some(user_agents_base.clone()),
        ..user_meta.clone()
    };
    add(
        acc,
        ResourceType::Skills,
        collect_auto_skill_entries(&user_agents_skills, SkillDiscoveryMode::Agents),
        &user_agents_meta,
        &overrides(global, ResourceType::Skills),
        &user_agents_base,
    );
    add(
        acc,
        ResourceType::Prompts,
        collect_auto_prompt_entries(&dir(global_base, "prompts")),
        &user_meta,
        &overrides(global, ResourceType::Prompts),
        global_base,
    );
    add(
        acc,
        ResourceType::Themes,
        collect_auto_theme_entries(&dir(global_base, "themes")),
        &user_meta,
        &overrides(global, ResourceType::Themes),
        global_base,
    );
}

/// `toResolvedPaths`: stable sort by precedence, then drop later entries that
/// resolve (through symlinks) to an already listed path.
fn to_resolved_paths(acc: Accumulator) -> ResolvedPaths {
    let finish = |target: Target| {
        let mut resolved = target.0;
        resolved.sort_by_key(|r| precedence_rank(&r.metadata));
        let mut seen = HashSet::new();
        resolved.retain(|r| seen.insert(node_path::canonicalize(&r.path)));
        resolved
    };
    ResolvedPaths {
        extensions: finish(acc.extensions),
        skills: finish(acc.skills),
        prompts: finish(acc.prompts),
        themes: finish(acc.themes),
        agents: finish(acc.agents),
    }
}

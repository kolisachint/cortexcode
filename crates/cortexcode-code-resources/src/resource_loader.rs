//! `core/resource-loader.ts`: `DefaultResourceLoader`.
//!
//! Loads skills, prompt templates / slash commands, context files and the
//! system-prompt inputs from the settings, the auto-discovered directories and
//! explicit paths. Not ported yet: extensions (ledger 12.3), themes (11.1) and
//! package sources (12.2); their getters are absent and their paths ignored.

use crate::context_files::{
    load_project_context_files, resolve_prompt_input, ContextFile, LoadProjectContextFilesOptions,
};
use crate::diagnostics::{DiagnosticType, ResourceCollision, ResourceDiagnostic};
use crate::node_path;
use crate::package_resolve::{home_dir, resolve_local_resources, PathMetadata, ResolveOptions};
use crate::prompt_templates::{load_prompt_templates, LoadPromptTemplatesOptions, PromptTemplate};
use crate::skills::{load_skills, LoadSkillsOptions, LoadSkillsResult, Skill};
use crate::source_info::{SourceInfo, SourceOrigin, SourceScope};
use cortexcode_code_settings::SettingsManager;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Prompts plus their diagnostics (`getPrompts()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadPromptsResult {
    pub prompts: Vec<PromptTemplate>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// Context files plus warnings (`getAgentsFiles()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentsFilesResult {
    pub agents_files: Vec<ContextFile>,
    pub warnings: Vec<String>,
}

/// A contributed path with its metadata (`{ path, metadata }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntry {
    pub path: String,
    pub metadata: PathMetadata,
}

/// `ResourceExtensionPaths` (themes are not ported yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceExtensionPaths {
    pub skill_paths: Vec<PathEntry>,
    pub prompt_paths: Vec<PathEntry>,
    pub slash_command_paths: Vec<PathEntry>,
    pub agent_paths: Vec<PathEntry>,
}

type Override<T> = Box<dyn Fn(T) -> T + Send + Sync>;

/// `DefaultResourceLoaderOptions`.
#[derive(Default)]
pub struct DefaultResourceLoaderOptions {
    pub cwd: String,
    pub agent_dir: String,
    /// Shared with the session services; created for `cwd` when absent.
    pub settings: Option<Arc<Mutex<SettingsManager>>>,
    pub additional_skill_paths: Vec<String>,
    pub additional_prompt_template_paths: Vec<String>,
    pub additional_slash_command_paths: Vec<String>,
    pub no_skills: bool,
    pub no_prompt_templates: bool,
    pub no_slash_commands: bool,
    pub no_context_files: bool,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<Vec<String>>,
    pub skills_override: Option<Override<LoadSkillsResult>>,
    pub prompts_override: Option<Override<LoadPromptsResult>>,
    pub agents_files_override: Option<Override<AgentsFilesResult>>,
    pub system_prompt_override: Option<Override<Option<String>>>,
    pub append_system_prompt_override: Option<Override<Vec<String>>>,
    /// `getHomeDir()` for user-scope discovery; `$HOME` when absent.
    pub home: Option<String>,
    /// The `~/.agents` context-file scope; `getUserAgentsDir()` when absent.
    pub user_agents_dir: Option<String>,
}

/// `DefaultResourceLoader`.
pub struct DefaultResourceLoader {
    options: DefaultResourceLoaderOptions,
    settings: Arc<Mutex<SettingsManager>>,
    skills: Vec<Skill>,
    skill_diagnostics: Vec<ResourceDiagnostic>,
    prompts: Vec<PromptTemplate>,
    prompt_diagnostics: Vec<ResourceDiagnostic>,
    agents_files: Vec<ContextFile>,
    agents_file_warnings: Vec<String>,
    system_prompt: Option<String>,
    append_system_prompt: Vec<String>,
    last_skill_paths: Vec<String>,
    last_agent_paths: Vec<String>,
    last_prompt_paths: Vec<String>,
    last_slash_command_paths: Vec<String>,
    extension_skill_source_infos: Vec<(String, SourceInfo)>,
    extension_prompt_source_infos: Vec<(String, SourceInfo)>,
    skill_namespaces: HashMap<String, String>,
}

/// Resolved path -> metadata, first registration wins (a JS `Map`).
type MetadataByPath = Vec<(String, PathMetadata)>;

fn metadata_for<'a>(map: &'a MetadataByPath, path: &str) -> Option<&'a PathMetadata> {
    map.iter().find(|(p, _)| p == path).map(|(_, m)| m)
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

impl DefaultResourceLoader {
    pub fn new(options: DefaultResourceLoaderOptions) -> Self {
        let settings = options.settings.clone().unwrap_or_else(|| {
            Arc::new(Mutex::new(SettingsManager::create(
                &options.cwd,
                &options.agent_dir,
            )))
        });
        Self {
            options,
            settings,
            skills: Vec::new(),
            skill_diagnostics: Vec::new(),
            prompts: Vec::new(),
            prompt_diagnostics: Vec::new(),
            agents_files: Vec::new(),
            agents_file_warnings: Vec::new(),
            system_prompt: None,
            append_system_prompt: Vec::new(),
            last_skill_paths: Vec::new(),
            last_agent_paths: Vec::new(),
            last_prompt_paths: Vec::new(),
            last_slash_command_paths: Vec::new(),
            extension_skill_source_infos: Vec::new(),
            extension_prompt_source_infos: Vec::new(),
            skill_namespaces: HashMap::new(),
        }
    }

    /// `getSkills()`.
    pub fn skills(&self) -> LoadSkillsResult {
        LoadSkillsResult {
            skills: self.skills.clone(),
            diagnostics: self.skill_diagnostics.clone(),
        }
    }

    /// `getSkillPaths()`.
    pub fn skill_paths(&self) -> Vec<String> {
        self.last_skill_paths.clone()
    }

    /// `getAgentPaths()`.
    pub fn agent_paths(&self) -> Vec<String> {
        self.last_agent_paths.clone()
    }

    /// `getPrompts()`.
    pub fn prompts(&self) -> LoadPromptsResult {
        LoadPromptsResult {
            prompts: self.prompts.clone(),
            diagnostics: self.prompt_diagnostics.clone(),
        }
    }

    /// `getAgentsFiles()`.
    pub fn agents_files(&self) -> AgentsFilesResult {
        AgentsFilesResult {
            agents_files: self.agents_files.clone(),
            warnings: self.agents_file_warnings.clone(),
        }
    }

    /// `getSystemPrompt()`.
    pub fn system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }

    /// `getAppendSystemPrompt()`.
    pub fn append_system_prompt(&self) -> Vec<String> {
        self.append_system_prompt.clone()
    }

    /// `addAppendSystemPrompt`.
    pub fn add_append_system_prompt(&mut self, text: impl Into<String>) {
        self.append_system_prompt.push(text.into());
    }

    fn home(&self) -> String {
        self.options.home.clone().unwrap_or_else(home_dir)
    }

    /// `resolveResourcePath`: `~` expansion, then against the cwd.
    fn resolve_resource_path(&self, p: &str) -> String {
        let trimmed = p.trim();
        let home = self.home();
        let expanded = if trimmed == "~" {
            home
        } else if let Some(rest) = trimmed.strip_prefix("~/") {
            node_path::join(&[&home, rest])
        } else if let Some(rest) = trimmed.strip_prefix('~') {
            node_path::join(&[&home, rest])
        } else {
            trimmed.to_string()
        };
        node_path::resolve(&self.options.cwd, &expanded)
    }

    /// `mergePaths`: resolved, deduped by real path, first wins.
    fn merge_paths(&self, primary: &[String], additional: &[String]) -> Vec<String> {
        let mut merged = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for p in primary.iter().chain(additional) {
            let resolved = self.resolve_resource_path(p);
            if seen.insert(node_path::canonicalize(&resolved)) {
                merged.push(resolved);
            }
        }
        merged
    }

    /// `extendResources`: paths contributed at runtime (extensions, plugins).
    pub fn extend_resources(&mut self, paths: ResourceExtensionPaths) {
        let normalize = |this: &Self, entries: Vec<PathEntry>| -> Vec<PathEntry> {
            entries
                .into_iter()
                .map(|e| PathEntry {
                    path: this.resolve_resource_path(&e.path),
                    metadata: e.metadata,
                })
                .collect()
        };
        let skill_paths = normalize(self, paths.skill_paths);
        for entry in &skill_paths {
            if let Some(ns) = &entry.metadata.namespace {
                self.skill_namespaces.insert(entry.path.clone(), ns.clone());
            }
            self.extension_skill_source_infos
                .push((entry.path.clone(), entry.metadata.source_info(&entry.path)));
        }
        let prompt_paths = normalize(self, paths.prompt_paths);
        for entry in &prompt_paths {
            self.extension_prompt_source_infos
                .push((entry.path.clone(), entry.metadata.source_info(&entry.path)));
        }
        if !skill_paths.is_empty() {
            let new: Vec<String> = skill_paths.iter().map(|e| e.path.clone()).collect();
            self.last_skill_paths = self.merge_paths(&self.last_skill_paths.clone(), &new);
            let paths = self.last_skill_paths.clone();
            self.update_skills(&paths, None);
        }
        if !prompt_paths.is_empty() {
            let new: Vec<String> = prompt_paths.iter().map(|e| e.path.clone()).collect();
            self.last_prompt_paths = self.merge_paths(&self.last_prompt_paths.clone(), &new);
            let (p, s) = (
                self.last_prompt_paths.clone(),
                self.last_slash_command_paths.clone(),
            );
            self.update_prompts(&p, &s, None);
        }
        let slash_paths = normalize(self, paths.slash_command_paths);
        for entry in &slash_paths {
            self.extension_prompt_source_infos
                .push((entry.path.clone(), entry.metadata.source_info(&entry.path)));
        }
        if !slash_paths.is_empty() {
            let new: Vec<String> = slash_paths.iter().map(|e| e.path.clone()).collect();
            self.last_slash_command_paths =
                self.merge_paths(&self.last_slash_command_paths.clone(), &new);
            let (p, s) = (
                self.last_prompt_paths.clone(),
                self.last_slash_command_paths.clone(),
            );
            self.update_prompts(&p, &s, None);
        }
        let agent_paths = normalize(self, paths.agent_paths);
        if !agent_paths.is_empty() {
            let files = expand_agent_files(agent_paths.iter().map(|e| e.path.as_str()));
            self.last_agent_paths = self.merge_paths(&self.last_agent_paths.clone(), &files);
            crate::agent_registry::set_agent_manifest_paths(self.last_agent_paths.clone());
        }
    }

    /// `reload()`: settings, resolved resource paths, then every resource kind.
    pub fn reload(&mut self) {
        let (global, project, settings_slash_paths) = {
            let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            settings.reload();
            (
                settings.global_settings(),
                settings.project_settings(),
                settings.slash_command_paths(),
            )
        };
        let home = self.home();
        let resolved = resolve_local_resources(&ResolveOptions {
            cwd: &self.options.cwd,
            agent_dir: &self.options.agent_dir,
            home: home.clone(),
            global_settings: &global,
            project_settings: &project,
        });

        self.extension_skill_source_infos.clear();
        self.extension_prompt_source_infos.clear();
        self.skill_namespaces.clear();

        let mut metadata_by_path: MetadataByPath = Vec::new();
        let enabled = |list: &[crate::package_resolve::ResolvedResource],
                       metadata_by_path: &mut MetadataByPath| {
            for r in list {
                if metadata_for(metadata_by_path, &r.path).is_none() {
                    metadata_by_path.push((r.path.clone(), r.metadata.clone()));
                }
            }
            list.iter()
                .filter(|r| r.enabled)
                .cloned()
                .collect::<Vec<_>>()
        };
        let enabled_skill_resources = enabled(&resolved.skills, &mut metadata_by_path);
        let enabled_prompts: Vec<String> = enabled(&resolved.prompts, &mut metadata_by_path)
            .into_iter()
            .map(|r| r.path)
            .collect();
        let _enabled_themes = enabled(&resolved.themes, &mut metadata_by_path);
        let _enabled_extensions = enabled(&resolved.extensions, &mut metadata_by_path);
        let enabled_agents: Vec<String> = enabled(&resolved.agents, &mut metadata_by_path)
            .into_iter()
            .map(|r| r.path)
            .collect();

        // `mapSkillPath`: an auto/package skill directory holding SKILL.md is its file.
        let enabled_skills: Vec<String> = enabled_skill_resources
            .iter()
            .map(|r| {
                if r.metadata.source != "auto" && r.metadata.origin != SourceOrigin::Package {
                    return r.path.clone();
                }
                if !std::fs::metadata(&r.path).is_ok_and(|m| m.is_dir()) {
                    return r.path.clone();
                }
                let skill_file = node_path::join(&[&r.path, "SKILL.md"]);
                if exists(&skill_file) {
                    if metadata_for(&metadata_by_path, &skill_file).is_none() {
                        metadata_by_path.push((skill_file.clone(), r.metadata.clone()));
                    }
                    return skill_file;
                }
                r.path.clone()
            })
            .collect();

        let additional_skills = self.options.additional_skill_paths.clone();
        let skill_paths = if self.options.no_skills {
            self.merge_paths(&[], &additional_skills)
        } else {
            self.merge_paths(&enabled_skills, &additional_skills)
        };
        self.last_skill_paths = skill_paths.clone();
        self.update_skills(&skill_paths, Some(&metadata_by_path));
        for p in &additional_skills {
            if cortexcode_code_paths::is_local_path(p)
                && !exists(p)
                && !self
                    .skill_diagnostics
                    .iter()
                    .any(|d| d.path.as_deref() == Some(p))
            {
                self.skill_diagnostics.push(ResourceDiagnostic {
                    kind: DiagnosticType::Error,
                    message: "Skill path does not exist".into(),
                    path: Some(p.clone()),
                    collision: None,
                });
            }
        }

        // Prompt templates and slash commands share one `/name` namespace; either
        // disable flag turns both off (explicit paths still load).
        let feature_disabled = self.options.no_prompt_templates || self.options.no_slash_commands;
        let additional_prompts = self.options.additional_prompt_template_paths.clone();
        let prompt_paths = if feature_disabled {
            self.merge_paths(&[], &additional_prompts)
        } else {
            self.merge_paths(&enabled_prompts, &additional_prompts)
        };
        self.last_prompt_paths = prompt_paths.clone();
        let cwd = self.options.cwd.clone();
        let config_dir = cortexcode_code_paths::CONFIG_DIR_NAME;
        let mut default_slash_dirs = vec![
            node_path::join(&[&cwd, config_dir, "commands"]),
            node_path::join(&[&cwd, ".claude", "commands"]),
        ];
        default_slash_dirs.extend(
            cortexcode_code_paths::collect_agents_ancestor_dirs(
                std::path::Path::new(&cwd),
                "commands",
            )
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned()),
        );
        default_slash_dirs.push(node_path::join(&[&self.options.agent_dir, "commands"]));
        default_slash_dirs.push(node_path::join(&[&home, ".agents", "commands"]));
        default_slash_dirs.push(node_path::join(&[&home, ".claude", "commands"]));
        default_slash_dirs.retain(|d| exists(d));
        let additional_slash = self.options.additional_slash_command_paths.clone();
        let slash_paths = if feature_disabled {
            self.merge_paths(&[], &additional_slash)
        } else {
            let mut primary = default_slash_dirs;
            primary.extend(settings_slash_paths);
            self.merge_paths(&primary, &additional_slash)
        };
        self.last_slash_command_paths = slash_paths.clone();
        self.update_prompts(&prompt_paths, &slash_paths, Some(&metadata_by_path));
        for (paths, message) in [
            (&additional_prompts, "Prompt template path does not exist"),
            (&additional_slash, "Slash command path does not exist"),
        ] {
            for p in paths {
                if cortexcode_code_paths::is_local_path(p)
                    && !exists(p)
                    && !self
                        .prompt_diagnostics
                        .iter()
                        .any(|d| d.path.as_deref() == Some(p))
                {
                    self.prompt_diagnostics.push(ResourceDiagnostic {
                        kind: DiagnosticType::Error,
                        message: message.into(),
                        path: Some(p.clone()),
                        collision: None,
                    });
                }
            }
        }

        self.last_agent_paths = enabled_agents.clone();
        crate::agent_registry::set_agent_manifest_paths(enabled_agents);

        let context = if self.options.no_context_files {
            AgentsFilesResult::default()
        } else {
            let (agents_files, warnings) =
                load_project_context_files(&LoadProjectContextFilesOptions {
                    cwd: cwd.clone(),
                    agent_dir: self.options.agent_dir.clone(),
                    user_agents_dir: self.options.user_agents_dir.clone(),
                });
            AgentsFilesResult {
                agents_files,
                warnings,
            }
        };
        let context = match &self.options.agents_files_override {
            Some(f) => f(context),
            None => context,
        };
        self.agents_files = context.agents_files;
        self.agents_file_warnings = context.warnings;

        let (base, warning) =
            resolve_prompt_input(self.options.system_prompt.as_deref(), "system prompt");
        if let Some(w) = warning {
            eprintln!("\x1b[33m{w}\x1b[39m");
        }
        self.system_prompt = match &self.options.system_prompt_override {
            Some(f) => f(base),
            None => base,
        };
        let base_append: Vec<String> = self
            .options
            .append_system_prompt
            .clone()
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                let (value, warning) = resolve_prompt_input(Some(s), "append system prompt");
                if let Some(w) = warning {
                    eprintln!("\x1b[33m{w}\x1b[39m");
                }
                value
            })
            .collect();
        self.append_system_prompt = match &self.options.append_system_prompt_override {
            Some(f) => f(base_append),
            None => base_append,
        };
    }

    /// `updateSkillsFromPaths`.
    fn update_skills(&mut self, skill_paths: &[String], metadata_by_path: Option<&MetadataByPath>) {
        let result = if self.options.no_skills && skill_paths.is_empty() {
            LoadSkillsResult::default()
        } else {
            load_skills(&LoadSkillsOptions {
                cwd: self.options.cwd.clone(),
                agent_dir: self.options.agent_dir.clone(),
                skill_paths: skill_paths.to_vec(),
                include_defaults: false,
                include_claude: true,
                namespaces: self.skill_namespaces.clone(),
            })
        };
        if let Some(map) = metadata_by_path {
            for (dir, metadata) in map {
                if let Some(ns) = &metadata.namespace {
                    self.skill_namespaces.insert(dir.clone(), ns.clone());
                }
            }
        }
        let result = match &self.options.skills_override {
            Some(f) => f(result),
            None => result,
        };
        self.skills = result
            .skills
            .into_iter()
            .map(|mut skill| {
                if let Some(info) = self.find_source_info_for_path(
                    &skill.file_path,
                    &self.extension_skill_source_infos,
                    metadata_by_path,
                ) {
                    skill.source_info = info;
                }
                skill
            })
            .collect();
        self.skill_diagnostics = result.diagnostics;
    }

    /// `updatePromptsFromPaths` (with `dedupePrompts`).
    fn update_prompts(
        &mut self,
        prompt_paths: &[String],
        slash_command_paths: &[String],
        metadata_by_path: Option<&MetadataByPath>,
    ) {
        let disabled = self.options.no_prompt_templates || self.options.no_slash_commands;
        let result = if disabled && prompt_paths.is_empty() && slash_command_paths.is_empty() {
            LoadPromptsResult::default()
        } else {
            dedupe_prompts(load_prompt_templates(&LoadPromptTemplatesOptions {
                cwd: self.options.cwd.clone(),
                agent_dir: self.options.agent_dir.clone(),
                prompt_paths: prompt_paths.to_vec(),
                slash_command_paths: slash_command_paths.to_vec(),
                include_defaults: false,
            }))
        };
        let result = match &self.options.prompts_override {
            Some(f) => f(result),
            None => result,
        };
        self.prompts = result
            .prompts
            .into_iter()
            .map(|mut prompt| {
                if let Some(info) = self.find_source_info_for_path(
                    &prompt.file_path,
                    &self.extension_prompt_source_infos,
                    metadata_by_path,
                ) {
                    prompt.source_info = info;
                }
                prompt
            })
            .collect();
        self.prompt_diagnostics = result.diagnostics;
    }

    /// `findSourceInfoForPath`: a contributed path's info, else the resolved
    /// metadata of the path or a directory containing it.
    fn find_source_info_for_path(
        &self,
        resource_path: &str,
        extra: &[(String, SourceInfo)],
        metadata_by_path: Option<&MetadataByPath>,
    ) -> Option<SourceInfo> {
        if resource_path.is_empty() {
            return None;
        }
        if resource_path.starts_with('<') {
            return Some(default_source_info_for_synthetic(resource_path));
        }
        let normalized = node_path::resolve("/", resource_path);
        let within = |source: &str| {
            let root = node_path::resolve("/", source);
            normalized == root || normalized.starts_with(&format!("{root}/"))
        };
        for (source_path, info) in extra {
            if within(source_path) {
                return Some(SourceInfo {
                    path: resource_path.to_string(),
                    ..info.clone()
                });
            }
        }
        let map = metadata_by_path?;
        if let Some(m) = metadata_for(map, &normalized).or_else(|| metadata_for(map, resource_path))
        {
            return Some(m.source_info(resource_path));
        }
        map.iter()
            .find(|(source_path, _)| within(source_path))
            .map(|(_, m)| m.source_info(resource_path))
    }
}

/// `getDefaultSourceInfoForPath` for `<source:...>` synthetic paths.
fn default_source_info_for_synthetic(path: &str) -> SourceInfo {
    let inner = path.trim_start_matches('<').trim_end_matches('>');
    let source = inner
        .split(':')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("temporary");
    SourceInfo {
        path: path.to_string(),
        source: source.to_string(),
        scope: SourceScope::Temporary,
        origin: SourceOrigin::TopLevel,
        base_dir: None,
    }
}

/// `dedupePrompts`: the first template with a name wins; later ones are
/// collision diagnostics.
fn dedupe_prompts(prompts: Vec<PromptTemplate>) -> LoadPromptsResult {
    let mut kept: Vec<PromptTemplate> = Vec::new();
    let mut diagnostics = Vec::new();
    for prompt in prompts {
        if let Some(existing) = kept.iter().find(|p| p.name == prompt.name) {
            diagnostics.push(ResourceDiagnostic {
                kind: DiagnosticType::Collision,
                message: format!("name \"/{}\" collision", prompt.name),
                path: Some(prompt.file_path.clone()),
                collision: Some(ResourceCollision {
                    resource_type: "prompt".into(),
                    name: prompt.name.clone(),
                    winner_path: existing.file_path.clone(),
                    loser_path: prompt.file_path.clone(),
                    winner_source: None,
                    loser_source: None,
                }),
            });
        } else {
            kept.push(prompt);
        }
    }
    LoadPromptsResult {
        prompts: kept,
        diagnostics,
    }
}

/// `expandAgentFiles`: directories become their `.md` files.
fn expand_agent_files<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut files = Vec::new();
    for p in paths {
        match std::fs::metadata(p) {
            Ok(m) if m.is_dir() => {
                let Ok(read_dir) = std::fs::read_dir(p) else {
                    continue;
                };
                let mut names: Vec<String> = read_dir
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".md"))
                    .collect();
                names.sort();
                files.extend(names.iter().map(|n| node_path::join(&[p, n])));
            }
            Ok(_) => files.push(p.to_string()),
            Err(_) => {}
        }
    }
    files
}

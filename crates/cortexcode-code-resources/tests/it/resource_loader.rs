//! Port of hoocode `packages/coding-agent/test/resource-loader.test.ts` (v0.5.89),
//! minus the cases for extensions (ledger 12.3) and themes (11.1). `.hoocode`
//! paths are cortex's `.cortexcode` (CONFIG_DIR_NAME); home-scoped discovery
//! uses an isolated home.

use cortexcode_code_resources::context_files::ContextFileSize;
use cortexcode_code_resources::package_resolve::PathMetadata;
use cortexcode_code_resources::source_info::{
    create_synthetic_source_info, SourceOrigin, SourceScope,
};
use cortexcode_code_resources::{
    DefaultResourceLoader, DefaultResourceLoaderOptions, LoadSkillsResult, PathEntry,
    ResourceExtensionPaths, Skill,
};
use cortexcode_code_settings::SettingsManager;
use std::sync::{Arc, Mutex};

const CONFIG: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

struct Env {
    _tmp: tempfile::TempDir,
    temp: String,
    agent_dir: String,
    cwd: String,
    home: String,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let temp = std::fs::canonicalize(tmp.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let e = Env {
        agent_dir: format!("{temp}/agent"),
        cwd: format!("{temp}/project"),
        home: format!("{temp}/home"),
        temp,
        _tmp: tmp,
    };
    for d in [&e.agent_dir, &e.cwd, &e.home] {
        std::fs::create_dir_all(d).unwrap();
    }
    e
}

impl Env {
    fn options(&self) -> DefaultResourceLoaderOptions {
        DefaultResourceLoaderOptions {
            cwd: self.cwd.clone(),
            agent_dir: self.agent_dir.clone(),
            settings: Some(Arc::new(Mutex::new(SettingsManager::in_memory(
                Default::default(),
            )))),
            home: Some(self.home.clone()),
            user_agents_dir: Some(format!("{}/.agents", self.home)),
            ..Default::default()
        }
    }
    fn loaded(&self, options: DefaultResourceLoaderOptions) -> DefaultResourceLoader {
        let mut loader = DefaultResourceLoader::new(options);
        loader.reload();
        loader
    }
}

fn write(path: &str, content: &str) {
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn skill(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
}

fn prompt_names(loader: &DefaultResourceLoader) -> Vec<String> {
    loader
        .prompts()
        .prompts
        .into_iter()
        .map(|p| p.name)
        .collect()
}

// reload

#[test]
fn should_initialize_with_empty_results_before_reload() {
    let e = env();
    let loader = DefaultResourceLoader::new(e.options());
    assert!(loader.skills().skills.is_empty());
    assert!(loader.prompts().prompts.is_empty());
}

#[test]
fn should_discover_skills_from_agent_dir() {
    let e = env();
    write(
        &format!("{}/skills/test-skill.md", e.agent_dir),
        &skill("test-skill", "A test skill", "Skill content here."),
    );
    let loader = e.loaded(e.options());
    assert!(loader
        .skills()
        .skills
        .iter()
        .any(|s| s.name == "test-skill"));
}

#[test]
fn should_ignore_extra_markdown_files_in_auto_discovered_skill_dirs() {
    let e = env();
    let dir = format!("{}/skills/pi-skills/browser-tools", e.agent_dir);
    write(
        &format!("{dir}/SKILL.md"),
        &skill("browser-tools", "Browser tools", "Skill content here."),
    );
    write(&format!("{dir}/EFFICIENCY.md"), "No frontmatter here");
    let loader = e.loaded(e.options());
    let result = loader.skills();
    assert!(result.skills.iter().any(|s| s.name == "browser-tools"));
    assert!(!result.diagnostics.iter().any(|d| d
        .path
        .as_deref()
        .is_some_and(|p| p.ends_with("EFFICIENCY.md"))));
}

#[test]
fn should_discover_prompts_from_agent_dir() {
    let e = env();
    write(
        &format!("{}/prompts/test-prompt.md", e.agent_dir),
        "---\ndescription: A test prompt\n---\nPrompt content.",
    );
    let loader = e.loaded(e.options());
    assert!(prompt_names(&loader).contains(&"test-prompt".to_string()));
}

#[test]
fn should_discover_slash_commands_from_agent_dir_and_project_commands_dirs() {
    let e = env();
    write(
        &format!("{}/commands/user-cmd.md", e.agent_dir),
        "User command body.",
    );
    write(
        &format!("{}/{CONFIG}/commands/project-cmd.md", e.cwd),
        "Project command body.",
    );
    let names = prompt_names(&e.loaded(e.options()));
    assert!(names.contains(&"user-cmd".to_string()));
    assert!(names.contains(&"project-cmd".to_string()));
}

#[test]
fn should_not_discover_slash_commands_when_no_slash_commands_is_set() {
    let e = env();
    write(
        &format!("{}/commands/hidden-cmd.md", e.agent_dir),
        "Should not load.",
    );
    let mut o = e.options();
    o.no_slash_commands = true;
    assert!(!prompt_names(&e.loaded(o)).contains(&"hidden-cmd".to_string()));
}

#[test]
fn should_import_claude_code_slash_commands_from_project_claude_commands() {
    let e = env();
    write(
        &format!("{}/.claude/commands/cc-cmd.md", e.cwd),
        "Claude Code command body.",
    );
    assert!(prompt_names(&e.loaded(e.options())).contains(&"cc-cmd".to_string()));
}

#[test]
fn should_not_import_claude_code_slash_commands_when_no_slash_commands_is_set() {
    let e = env();
    write(
        &format!("{}/.claude/commands/cc-hidden.md", e.cwd),
        "Should not load.",
    );
    let mut o = e.options();
    o.no_slash_commands = true;
    assert!(!prompt_names(&e.loaded(o)).contains(&"cc-hidden".to_string()));
}

#[test]
fn should_prefer_config_commands_over_claude_commands_on_name_collision() {
    let e = env();
    let native = format!("{}/{CONFIG}/commands/deploy.md", e.cwd);
    write(&native, "hoocode deploy");
    write(
        &format!("{}/.claude/commands/deploy.md", e.cwd),
        "claude deploy",
    );
    let loader = e.loaded(e.options());
    let deploy = loader
        .prompts()
        .prompts
        .into_iter()
        .find(|p| p.name == "deploy")
        .unwrap();
    assert_eq!(deploy.file_path, native);
}

#[test]
fn should_discover_slash_commands_from_project_agents_commands() {
    let e = env();
    write(
        &format!("{}/.agents/commands/agents-cmd.md", e.cwd),
        "Dot-agents command body.",
    );
    assert!(prompt_names(&e.loaded(e.options())).contains(&"agents-cmd".to_string()));
}

#[test]
fn should_prefer_config_commands_over_agents_commands_on_name_collision() {
    let e = env();
    let native = format!("{}/{CONFIG}/commands/deploy.md", e.cwd);
    write(&native, "hoocode deploy");
    write(
        &format!("{}/.agents/commands/deploy.md", e.cwd),
        "dot-agents deploy",
    );
    let loader = e.loaded(e.options());
    let deploy = loader
        .prompts()
        .prompts
        .into_iter()
        .find(|p| p.name == "deploy")
        .unwrap();
    assert_eq!(deploy.file_path, native);
}

#[test]
fn disables_both_prompts_and_commands_when_no_prompt_templates_is_set() {
    let e = env();
    write(&format!("{}/{CONFIG}/prompts/bar.md", e.cwd), "bar body");
    write(&format!("{}/{CONFIG}/commands/foo.md", e.cwd), "foo body");
    let mut o = e.options();
    o.no_prompt_templates = true;
    let names = prompt_names(&e.loaded(o));
    assert!(!names.contains(&"bar".to_string()));
    assert!(!names.contains(&"foo".to_string()));
}

#[test]
fn disables_both_prompts_and_commands_when_no_slash_commands_is_set() {
    let e = env();
    write(&format!("{}/{CONFIG}/prompts/baz.md", e.cwd), "baz body");
    write(&format!("{}/{CONFIG}/commands/qux.md", e.cwd), "qux body");
    let mut o = e.options();
    o.no_slash_commands = true;
    let names = prompt_names(&e.loaded(o));
    assert!(!names.contains(&"baz".to_string()));
    assert!(!names.contains(&"qux".to_string()));
}

#[test]
fn should_prefer_project_resources_over_user_on_name_collisions() {
    let e = env();
    write(&format!("{}/prompts/commit.md", e.agent_dir), "User prompt");
    let project_prompt = format!("{}/{CONFIG}/prompts/commit.md", e.cwd);
    write(&project_prompt, "Project prompt");
    write(
        &format!("{}/skills/collision-skill/SKILL.md", e.agent_dir),
        &skill("collision-skill", "user", "User skill"),
    );
    let project_skill = format!("{}/{CONFIG}/skills/collision-skill/SKILL.md", e.cwd);
    write(
        &project_skill,
        &skill("collision-skill", "project", "Project skill"),
    );
    let loader = e.loaded(e.options());
    let prompt = loader
        .prompts()
        .prompts
        .into_iter()
        .find(|p| p.name == "commit")
        .unwrap();
    assert_eq!(prompt.file_path, project_prompt);
    let skill = loader
        .skills()
        .skills
        .into_iter()
        .find(|s| s.name == "collision-skill")
        .unwrap();
    assert_eq!(skill.file_path, project_skill);
}

#[test]
fn should_honor_overrides_for_auto_discovered_resources() {
    let e = env();
    let mut settings = SettingsManager::in_memory(Default::default());
    settings.set_skill_paths(&["-skills/skip-skill".to_string()]);
    settings.set_prompt_template_paths(&["-prompts/skip.md".to_string()]);
    write(
        &format!("{}/skills/skip-skill/SKILL.md", e.agent_dir),
        &skill("skip-skill", "Skip me", "Content"),
    );
    write(&format!("{}/prompts/skip.md", e.agent_dir), "Skip prompt");
    let mut o = e.options();
    o.settings = Some(Arc::new(Mutex::new(settings)));
    let loader = e.loaded(o);
    assert!(!loader
        .skills()
        .skills
        .iter()
        .any(|s| s.name == "skip-skill"));
    assert!(!prompt_names(&loader).contains(&"skip".to_string()));
}

#[test]
fn should_discover_agents_md_context_files() {
    let e = env();
    write(
        &format!("{}/AGENTS.md", e.cwd),
        "# Project Guidelines\n\nBe helpful.",
    );
    let loader = e.loaded(e.options());
    assert!(loader
        .agents_files()
        .agents_files
        .iter()
        .any(|f| f.path.contains("AGENTS.md")));
}

#[test]
fn should_skip_context_file_discovery_when_no_context_files_is_true() {
    let e = env();
    write(
        &format!("{}/AGENTS.md", e.cwd),
        "# Project Guidelines\n\nBe helpful.",
    );
    write(
        &format!("{}/CLAUDE.md", e.cwd),
        "# Claude Guidelines\n\nBe helpful.",
    );
    let mut o = e.options();
    o.no_context_files = true;
    assert!(e.loaded(o).agents_files().agents_files.is_empty());
}

#[test]
fn should_flag_agents_md_that_exceeds_warn_size_as_large() {
    let e = env();
    write(&format!("{}/AGENTS.md", e.cwd), &"x".repeat(9 * 1024));
    let result = e.loaded(e.options()).agents_files();
    assert_eq!(result.agents_files.len(), 1);
    assert_eq!(result.agents_files[0].size, Some(ContextFileSize::Large));
    assert_eq!(result.agents_files[0].tokens, Some(9 * 1024 / 4));
    assert!(result.warnings.is_empty());
}

#[test]
fn should_flag_and_truncate_agents_md_that_exceeds_max_size() {
    let e = env();
    write(&format!("{}/AGENTS.md", e.cwd), &"x".repeat(41 * 1024));
    let result = e.loaded(e.options()).agents_files();
    assert_eq!(result.agents_files.len(), 1);
    assert!(result.agents_files[0].content.contains("[truncated:"));
    assert_eq!(
        result.agents_files[0].size,
        Some(ContextFileSize::Truncated)
    );
    assert!(result.warnings.is_empty());
}

// extendResources

#[test]
fn should_load_skills_and_prompts_with_extension_metadata() {
    let e = env();
    let skill_dir = format!("{}/extra-skills/extra-skill", e.temp);
    let skill_path = format!("{skill_dir}/SKILL.md");
    write(
        &skill_path,
        &skill("extra-skill", "Extra skill", "Extra content"),
    );
    let prompt_dir = format!("{}/extra-prompts", e.temp);
    let prompt_path = format!("{prompt_dir}/extra.md");
    write(
        &prompt_path,
        "---\ndescription: Extra prompt\n---\nExtra prompt content",
    );
    let mut loader = e.loaded(e.options());
    let meta = |base: &str| PathMetadata {
        source: "extension:extra".into(),
        scope: SourceScope::Temporary,
        origin: SourceOrigin::TopLevel,
        base_dir: Some(base.to_string()),
        namespace: None,
    };
    loader.extend_resources(ResourceExtensionPaths {
        skill_paths: vec![PathEntry {
            path: skill_dir.clone(),
            metadata: meta(&skill_dir),
        }],
        prompt_paths: vec![PathEntry {
            path: prompt_path.clone(),
            metadata: meta(&prompt_dir),
        }],
        ..Default::default()
    });
    let s = loader
        .skills()
        .skills
        .into_iter()
        .find(|s| s.name == "extra-skill")
        .unwrap();
    assert_eq!(s.source_info.source, "extension:extra");
    assert_eq!(s.source_info.path, skill_path);
    let p = loader
        .prompts()
        .prompts
        .into_iter()
        .find(|p| p.name == "extra")
        .unwrap();
    assert_eq!(p.source_info.source, "extension:extra");
    assert_eq!(p.source_info.path, prompt_path);
}

// noSkills option

#[test]
fn should_skip_skill_discovery_when_no_skills_is_true() {
    let e = env();
    write(
        &format!("{}/skills/test-skill.md", e.agent_dir),
        &skill("test-skill", "A test skill", "Content"),
    );
    let mut o = e.options();
    o.no_skills = true;
    assert!(e.loaded(o).skills().skills.is_empty());
}

#[test]
fn should_still_load_additional_skill_paths_when_no_skills_is_true() {
    let e = env();
    let dir = format!("{}/custom-skills", e.temp);
    write(
        &format!("{dir}/custom.md"),
        &skill("custom", "Custom skill", "Content"),
    );
    let mut o = e.options();
    o.no_skills = true;
    o.additional_skill_paths = vec![dir];
    assert!(e
        .loaded(o)
        .skills()
        .skills
        .iter()
        .any(|s| s.name == "custom"));
}

// override functions

#[test]
fn should_apply_skills_override() {
    let e = env();
    let injected = Skill {
        name: "injected".into(),
        description: "Injected skill".into(),
        file_path: "/fake/path".into(),
        base_dir: "/fake".into(),
        source_info: create_synthetic_source_info("/fake/path", "custom", None, None, None),
        disable_model_invocation: false,
        allowed_tools: None,
    };
    let mut o = e.options();
    let skill = injected.clone();
    o.skills_override = Some(Box::new(move |_| LoadSkillsResult {
        skills: vec![skill.clone()],
        diagnostics: vec![],
    }));
    let skills = e.loaded(o).skills().skills;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "injected");
}

#[test]
fn should_apply_system_prompt_override() {
    let e = env();
    let mut o = e.options();
    o.system_prompt_override = Some(Box::new(|_| Some("Custom system prompt".into())));
    assert_eq!(
        e.loaded(o).system_prompt().as_deref(),
        Some("Custom system prompt")
    );
}

// Beyond the TS file: missing explicit paths are errors, prompt collisions
// are reported, and a system prompt file is read.

#[test]
fn missing_explicit_paths_and_prompt_collisions_are_diagnostics() {
    let e = env();
    write(&format!("{}/commands/dup.md", e.agent_dir), "first");
    write(&format!("{}/prompts/dup.md", e.agent_dir), "second");
    let prompt_file = format!("{}/system.md", e.temp);
    write(&prompt_file, "From a file.");
    let mut o = e.options();
    o.additional_skill_paths = vec![format!("{}/nope", e.temp)];
    o.additional_prompt_template_paths = vec![format!("{}/nope.md", e.temp)];
    o.system_prompt = Some(prompt_file);
    let loader = e.loaded(o);
    assert!(loader
        .skills()
        .diagnostics
        .iter()
        .any(|d| d.message == "Skill path does not exist"
            || d.message == "skill path does not exist"));
    let diagnostics = loader.prompts().diagnostics;
    assert!(diagnostics
        .iter()
        .any(|d| d.message == "Prompt template path does not exist"));
    assert!(diagnostics
        .iter()
        .any(|d| d.message == "name \"/dup\" collision"));
    assert_eq!(loader.system_prompt().as_deref(), Some("From a file."));
}

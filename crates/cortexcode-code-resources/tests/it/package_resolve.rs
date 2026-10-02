//! Port of the local/auto `resolve()` cases of hoocode
//! `packages/coding-agent/test/package-manager.test.ts` (v0.5.89). Package
//! sources (npm/git) are ledger 12.2. `.hoocode` in the TS paths is cortex's
//! `.cortexcode` (CONFIG_DIR_NAME); `$HOME` is passed explicitly instead of
//! mutating the process environment.

use cortexcode_code_resources::package_discovery::collect_auto_extension_entries;
use cortexcode_code_resources::package_resolve::{
    resolve_local_resources, ResolveOptions, ResolvedPaths, ResolvedResource,
};
use cortexcode_code_resources::source_info::{SourceOrigin, SourceScope};
use serde_json::{json, Map, Value};
use std::path::Path;

const CONFIG: &str = cortexcode_code_paths::CONFIG_DIR_NAME;

struct Env {
    _tmp: tempfile::TempDir,
    temp: String,
    agent_dir: String,
    home: String,
    cwd: String,
    global: Map<String, Value>,
    project: Map<String, Value>,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    // Canonical so symlink dedupe compares like-for-like on macOS /var -> /private/var.
    let temp = std::fs::canonicalize(tmp.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let agent_dir = format!("{temp}/agent");
    let home = format!("{temp}/home");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env {
        _tmp: tmp,
        cwd: temp.clone(),
        temp,
        agent_dir,
        home,
        global: Map::new(),
        project: Map::new(),
    }
}

impl Env {
    fn resolve(&self) -> ResolvedPaths {
        resolve_local_resources(&ResolveOptions {
            cwd: &self.cwd,
            agent_dir: &self.agent_dir,
            home: self.home.clone(),
            global_settings: &self.global,
            project_settings: &self.project,
        })
    }
    fn set(&mut self, key: &str, values: &[&str]) {
        self.global.insert(key.into(), json!(values));
    }
    fn set_project(&mut self, key: &str, values: &[&str]) {
        self.project.insert(key.into(), json!(values));
    }
}

fn write(path: &str, content: &str) {
    std::fs::create_dir_all(Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn skill(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name}\n---\nContent")
}

fn has(list: &[ResolvedResource], path: &str, enabled: bool) -> bool {
    list.iter().any(|r| r.path == path && r.enabled == enabled)
}

fn enabled_ending(list: &[ResolvedResource], suffix: &str) -> bool {
    list.iter().any(|r| r.path.ends_with(suffix) && r.enabled)
}

fn disabled_ending(list: &[ResolvedResource], suffix: &str) -> bool {
    list.iter().any(|r| r.path.ends_with(suffix) && !r.enabled)
}

// resolve

#[test]
fn should_return_no_package_sourced_paths_when_no_sources_configured() {
    let r = env().resolve();
    assert!(r.extensions.is_empty());
    assert!(r.prompts.is_empty());
    assert!(r.themes.is_empty());
    assert!(r
        .skills
        .iter()
        .all(|s| s.metadata.source == "auto" && s.metadata.origin == SourceOrigin::TopLevel));
}

#[test]
fn should_resolve_local_extension_paths_from_settings() {
    let mut e = env();
    let ext = format!("{}/extensions/my-extension.ts", e.agent_dir);
    write(&ext, "export default function() {}");
    e.set("extensions", &["extensions/my-extension.ts"]);
    assert!(has(&e.resolve().extensions, &ext, true));
}

#[test]
fn should_resolve_skill_paths_from_settings() {
    let mut e = env();
    let file = format!("{}/skills/my-skill/SKILL.md", e.agent_dir);
    write(&file, &skill("test-skill"));
    e.set("skills", &["skills"]);
    assert!(has(&e.resolve().skills, &file, true));
}

#[test]
fn should_auto_discover_root_markdown_skills_from_skill_dirs() {
    let e = env();
    let file = format!("{}/skills/single-file.md", e.agent_dir);
    write(&file, &skill("single-file"));
    assert!(has(&e.resolve().skills, &file, true));
}

#[test]
fn should_resolve_project_paths_relative_to_the_config_dir() {
    let mut e = env();
    let ext = format!("{}/{CONFIG}/extensions/project-ext.ts", e.temp);
    write(&ext, "export default function() {}");
    e.set_project("extensions", &["extensions/project-ext.ts"]);
    assert!(has(&e.resolve().extensions, &ext, true));
}

#[test]
fn should_auto_discover_user_prompts_with_overrides() {
    let mut e = env();
    let prompt = format!("{}/prompts/auto.md", e.agent_dir);
    write(&prompt, "Auto prompt");
    e.set("prompts", &["!prompts/auto.md"]);
    assert!(has(&e.resolve().prompts, &prompt, false));
}

#[test]
fn should_resolve_symlinked_user_and_project_resources_once() {
    let mut e = env();
    e.home = e.temp.clone();
    let shared = format!("{}/shared-resources", e.temp);
    write(
        &format!("{shared}/extensions/shared.ts"),
        "export default function() {}",
    );
    write(
        &format!("{shared}/skills/shared-skill/SKILL.md"),
        &skill("shared-skill"),
    );
    write(&format!("{shared}/prompts/shared.md"), "Shared prompt");
    write(
        &format!("{shared}/themes/shared.json"),
        "{\"name\": \"shared-theme\"}",
    );
    std::fs::create_dir_all(format!("{}/{CONFIG}", e.temp)).unwrap();
    for kind in ["extensions", "skills", "prompts", "themes"] {
        std::os::unix::fs::symlink(
            format!("{shared}/{kind}"),
            format!("{}/{kind}", e.agent_dir),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            format!("{shared}/{kind}"),
            format!("{}/{CONFIG}/{kind}", e.temp),
        )
        .unwrap();
    }
    let r = e.resolve();
    assert_eq!(
        (
            r.extensions.len(),
            r.skills.len(),
            r.prompts.len(),
            r.themes.len()
        ),
        (1, 1, 1, 1)
    );
    for list in [&r.extensions, &r.skills, &r.prompts, &r.themes] {
        assert_eq!(list[0].metadata.scope, SourceScope::Project);
    }
}

#[test]
fn should_auto_discover_project_prompts_with_overrides() {
    let mut e = env();
    let prompt = format!("{}/{CONFIG}/prompts/is.md", e.temp);
    write(&prompt, "Is prompt");
    e.set_project("prompts", &["!prompts/is.md"]);
    assert!(has(&e.resolve().prompts, &prompt, false));
}

#[test]
fn should_resolve_directory_with_package_json_extensions_in_extensions_setting() {
    let mut e = env();
    let pkg = format!("{}/my-extensions-pkg", e.temp);
    write(
        &format!("{pkg}/package.json"),
        &json!({"name": "my-extensions-pkg", "hoocode": {"extensions": ["./extensions/clip.ts", "./extensions/cost.ts"]}}).to_string(),
    );
    for f in ["clip", "cost", "helper"] {
        write(
            &format!("{pkg}/extensions/{f}.ts"),
            "export default function() {}",
        );
    }
    e.set("extensions", &[&pkg]);
    let r = e.resolve();
    assert!(has(
        &r.extensions,
        &format!("{pkg}/extensions/clip.ts"),
        true
    ));
    assert!(has(
        &r.extensions,
        &format!("{pkg}/extensions/cost.ts"),
        true
    ));
    assert!(!r.extensions.iter().any(|x| x.path.ends_with("helper.ts")));
}

// auto-discovered skill metadata

#[test]
fn should_use_the_agent_dir_as_base_dir_for_user_skills() {
    let e = env();
    let path = format!("{}/skills/user-pi/SKILL.md", e.agent_dir);
    write(&path, "---\nname: user-pi\ndescription: user hoo\n---\n");
    let r = e.resolve();
    let s = r.skills.iter().find(|s| s.path == path).unwrap();
    assert_eq!(s.metadata.source, "auto");
    assert_eq!(s.metadata.scope, SourceScope::User);
    assert_eq!(s.metadata.base_dir.as_deref(), Some(e.agent_dir.as_str()));
}

#[test]
fn should_use_the_project_config_dir_as_base_dir_for_project_skills() {
    let e = env();
    let base = format!("{}/{CONFIG}", e.temp);
    let path = format!("{base}/skills/project-pi/SKILL.md");
    write(
        &path,
        "---\nname: project-pi\ndescription: project hoo\n---\n",
    );
    let r = e.resolve();
    let s = r.skills.iter().find(|s| s.path == path).unwrap();
    assert_eq!(s.metadata.source, "auto");
    assert_eq!(s.metadata.scope, SourceScope::Project);
    assert_eq!(s.metadata.base_dir.as_deref(), Some(base.as_str()));
}

#[test]
fn should_use_home_agents_as_base_dir_for_user_agents_skills() {
    let mut e = env();
    e.home = e.temp.clone();
    let base = format!("{}/.agents", e.temp);
    let path = format!("{base}/skills/user-agents/SKILL.md");
    write(
        &path,
        "---\nname: user-agents\ndescription: user agents\n---\n",
    );
    let r = e.resolve();
    let s = r.skills.iter().find(|s| s.path == path).unwrap();
    assert_eq!(s.metadata.source, "auto");
    assert_eq!(s.metadata.scope, SourceScope::User);
    assert_eq!(s.metadata.base_dir.as_deref(), Some(base.as_str()));
}

#[test]
fn should_use_each_project_agents_dir_as_base_dir_for_project_agents_skills() {
    let mut e = env();
    let repo = format!("{}/repo", e.temp);
    e.cwd = format!("{repo}/packages/feature");
    std::fs::create_dir_all(&e.cwd).unwrap();
    std::fs::create_dir_all(format!("{repo}/.git")).unwrap();
    let repo_base = format!("{repo}/.agents");
    let repo_skill = format!("{repo_base}/skills/repo/SKILL.md");
    write(&repo_skill, "---\nname: repo\ndescription: repo\n---\n");
    let pkg_base = format!("{repo}/packages/.agents");
    let pkg_skill = format!("{pkg_base}/skills/package/SKILL.md");
    write(
        &pkg_skill,
        "---\nname: package\ndescription: package\n---\n",
    );
    let r = e.resolve();
    for (path, base) in [(&repo_skill, &repo_base), (&pkg_skill, &pkg_base)] {
        let s = r.skills.iter().find(|s| &s.path == path).unwrap();
        assert_eq!(s.metadata.source, "auto");
        assert_eq!(s.metadata.scope, SourceScope::Project);
        assert_eq!(s.metadata.base_dir.as_deref(), Some(base.as_str()));
    }
}

// .agents/skills auto-discovery

#[test]
fn should_scan_agents_skills_from_cwd_up_to_git_repo_root() {
    let mut e = env();
    let repo = format!("{}/repo", e.temp);
    e.cwd = format!("{repo}/packages/feature");
    std::fs::create_dir_all(&e.cwd).unwrap();
    std::fs::create_dir_all(format!("{repo}/.git")).unwrap();
    let above = format!("{}/.agents/skills/above-repo/SKILL.md", e.temp);
    write(&above, "---\nname: above-repo\ndescription: above\n---\n");
    let root = format!("{repo}/.agents/skills/repo-root/SKILL.md");
    write(&root, "---\nname: repo-root\ndescription: repo\n---\n");
    let nested = format!("{repo}/packages/.agents/skills/nested/SKILL.md");
    write(&nested, "---\nname: nested\ndescription: nested\n---\n");
    let r = e.resolve();
    assert!(has(&r.skills, &root, true));
    assert!(has(&r.skills, &nested, true));
    assert!(!r.skills.iter().any(|s| s.path == above));
}

#[test]
fn should_scan_agents_skills_up_to_filesystem_root_when_not_in_a_git_repo() {
    let mut e = env();
    let non_repo = format!("{}/non-repo", e.temp);
    e.cwd = format!("{non_repo}/a/b");
    std::fs::create_dir_all(&e.cwd).unwrap();
    let root = format!("{non_repo}/.agents/skills/root/SKILL.md");
    write(&root, "---\nname: root\ndescription: root\n---\n");
    let middle = format!("{non_repo}/a/.agents/skills/middle/SKILL.md");
    write(&middle, "---\nname: middle\ndescription: middle\n---\n");
    let r = e.resolve();
    assert!(has(&r.skills, &root, true));
    assert!(has(&r.skills, &middle, true));
}

#[test]
fn should_ignore_root_markdown_files_in_agents_skills() {
    let mut e = env();
    let dir = format!("{}/.agents/skills", e.temp);
    let root_file = format!("{dir}/root-file.md");
    let nested = format!("{dir}/nested-skill/SKILL.md");
    write(
        &root_file,
        "---\nname: root-file\ndescription: Root markdown file\n---\n",
    );
    write(
        &nested,
        "---\nname: nested-skill\ndescription: Nested skill\n---\n",
    );
    e.cwd = format!("{}/work", e.temp);
    std::fs::create_dir_all(&e.cwd).unwrap();
    let r = e.resolve();
    assert!(!r.skills.iter().any(|s| s.path == root_file));
    assert!(has(&r.skills, &nested, true));
}

#[test]
fn should_keep_home_agents_skills_user_scoped_when_cwd_is_under_home() {
    let mut e = env();
    e.home = e.temp.clone();
    e.cwd = format!("{}/scratch/nested", e.temp);
    e.agent_dir = format!("{}/{CONFIG}/agent", e.temp);
    std::fs::create_dir_all(&e.cwd).unwrap();
    std::fs::create_dir_all(&e.agent_dir).unwrap();
    let home_skill = format!("{}/.agents/skills/home-skill/SKILL.md", e.temp);
    write(
        &home_skill,
        "---\nname: home-skill\ndescription: home\n---\n",
    );
    let r = e.resolve();
    let matching: Vec<_> = r.skills.iter().filter(|s| s.path == home_skill).collect();
    assert_eq!(matching.len(), 1);
    assert!(matching[0].enabled);
    assert_eq!(matching[0].metadata.scope, SourceScope::User);
    assert_eq!(matching[0].metadata.source, "auto");
}

#[test]
fn should_dedupe_user_skills_when_agent_skills_is_a_symlink_to_agents_skills() {
    let mut e = env();
    e.home = e.temp.clone();
    let agents_skills = format!("{}/.agents/skills", e.temp);
    std::fs::create_dir_all(&agents_skills).unwrap();
    std::os::unix::fs::symlink(&agents_skills, format!("{}/skills", e.agent_dir)).unwrap();
    write(
        &format!("{agents_skills}/foo/SKILL.md"),
        "---\nname: foo\ndescription: foo\n---\n",
    );
    let r = e.resolve();
    assert_eq!(
        r.skills
            .iter()
            .filter(|s| s.path.ends_with("foo/SKILL.md"))
            .count(),
        1
    );
}

// ignore files

#[test]
fn should_respect_gitignore_in_skill_directories() {
    let mut e = env();
    let dir = format!("{}/skills", e.agent_dir);
    write(&format!("{dir}/.gitignore"), "venv\n__pycache__\n");
    write(&format!("{dir}/good-skill/SKILL.md"), &skill("good-skill"));
    write(
        &format!("{dir}/venv/bad-skill/SKILL.md"),
        &skill("bad-skill"),
    );
    e.set("skills", &["skills"]);
    let r = e.resolve();
    assert!(r
        .skills
        .iter()
        .any(|s| s.path.contains("good-skill") && s.enabled));
    assert!(!r
        .skills
        .iter()
        .any(|s| s.path.contains("venv") && s.enabled));
}

#[test]
fn should_not_apply_parent_gitignore_to_auto_discovery() {
    let e = env();
    write(&format!("{}/.gitignore", e.temp), &format!("{CONFIG}\n"));
    let path = format!("{}/{CONFIG}/skills/auto-skill/SKILL.md", e.temp);
    write(&path, &skill("auto-skill"));
    assert!(has(&e.resolve().skills, &path, true));
}

// pattern filtering in top-level arrays

#[test]
fn should_exclude_extensions_with_bang_pattern() {
    let mut e = env();
    for f in ["keep", "remove"] {
        write(
            &format!("{}/extensions/{f}.ts", e.agent_dir),
            "export default function() {}",
        );
    }
    e.set("extensions", &["extensions", "!**/remove.ts"]);
    let r = e.resolve();
    assert!(enabled_ending(&r.extensions, "keep.ts"));
    assert!(disabled_ending(&r.extensions, "remove.ts"));
}

#[test]
fn should_filter_themes_with_glob_patterns() {
    let mut e = env();
    for f in ["dark", "light", "funky"] {
        write(&format!("{}/themes/{f}.json", e.agent_dir), "{}");
    }
    e.set("themes", &["themes", "!funky.json"]);
    let r = e.resolve();
    assert!(enabled_ending(&r.themes, "dark.json"));
    assert!(enabled_ending(&r.themes, "light.json"));
    assert!(disabled_ending(&r.themes, "funky.json"));
}

#[test]
fn should_filter_prompts_with_exclusion_pattern() {
    let mut e = env();
    write(&format!("{}/prompts/review.md", e.agent_dir), "Review code");
    write(
        &format!("{}/prompts/explain.md", e.agent_dir),
        "Explain code",
    );
    e.set("prompts", &["prompts", "!explain.md"]);
    let r = e.resolve();
    assert!(enabled_ending(&r.prompts, "review.md"));
    assert!(disabled_ending(&r.prompts, "explain.md"));
}

#[test]
fn should_filter_skills_with_exclusion_pattern() {
    let mut e = env();
    write(
        &format!("{}/skills/good-skill/SKILL.md", e.agent_dir),
        &skill("good-skill"),
    );
    write(
        &format!("{}/skills/bad-skill/SKILL.md", e.agent_dir),
        &skill("bad-skill"),
    );
    e.set("skills", &["skills", "!**/bad-skill"]);
    let r = e.resolve();
    assert!(r
        .skills
        .iter()
        .any(|s| s.path.contains("good-skill") && s.enabled));
    assert!(r
        .skills
        .iter()
        .any(|s| s.path.contains("bad-skill") && !s.enabled));
}

#[test]
fn should_work_without_patterns() {
    let mut e = env();
    let ext = format!("{}/extensions/my-ext.ts", e.agent_dir);
    write(&ext, "export default function() {}");
    e.set("extensions", &["extensions/my-ext.ts"]);
    assert!(has(&e.resolve().extensions, &ext, true));
}

// force-include / force-exclude patterns (top level)

#[test]
fn should_force_include_extensions_with_plus_pattern_after_exclusion() {
    let mut e = env();
    for f in ["keep", "excluded", "force-back"] {
        write(
            &format!("{}/extensions/{f}.ts", e.agent_dir),
            "export default function() {}",
        );
    }
    e.set(
        "extensions",
        &[
            "extensions",
            "!extensions/*.ts",
            "+extensions/force-back.ts",
        ],
    );
    let r = e.resolve();
    assert!(disabled_ending(&r.extensions, "keep.ts"));
    assert!(disabled_ending(&r.extensions, "excluded.ts"));
    assert!(enabled_ending(&r.extensions, "force-back.ts"));
}

#[test]
fn should_force_include_after_specific_exclusion() {
    let mut e = env();
    for f in ["a", "b"] {
        write(
            &format!("{}/extensions/{f}.ts", e.agent_dir),
            "export default function() {}",
        );
    }
    e.set(
        "extensions",
        &["extensions", "!extensions/b.ts", "+extensions/b.ts"],
    );
    let r = e.resolve();
    assert!(enabled_ending(&r.extensions, "/a.ts"));
    assert!(enabled_ending(&r.extensions, "/b.ts"));
}

#[test]
fn should_force_include_themes() {
    let mut e = env();
    for f in ["dark", "light", "special"] {
        write(&format!("{}/themes/{f}.json", e.agent_dir), "{}");
    }
    e.set(
        "themes",
        &["themes", "!themes/*.json", "+themes/special.json"],
    );
    let r = e.resolve();
    assert!(disabled_ending(&r.themes, "dark.json"));
    assert!(disabled_ending(&r.themes, "light.json"));
    assert!(enabled_ending(&r.themes, "special.json"));
}

#[test]
fn should_force_include_prompts() {
    let mut e = env();
    for f in ["review", "explain", "debug"] {
        write(&format!("{}/prompts/{f}.md", e.agent_dir), f);
    }
    e.set(
        "prompts",
        &["prompts", "!prompts/*.md", "+prompts/debug.md"],
    );
    let r = e.resolve();
    assert!(disabled_ending(&r.prompts, "review.md"));
    assert!(disabled_ending(&r.prompts, "explain.md"));
    assert!(enabled_ending(&r.prompts, "debug.md"));
}

#[test]
fn should_force_exclude_top_level_resources() {
    let mut e = env();
    for f in ["alpha", "beta"] {
        write(
            &format!("{}/extensions/{f}.ts", e.agent_dir),
            "export default function() {}",
        );
    }
    e.set(
        "extensions",
        &["extensions", "+extensions/alpha.ts", "-extensions/alpha.ts"],
    );
    let r = e.resolve();
    assert!(disabled_ending(&r.extensions, "alpha.ts"));
    assert!(enabled_ending(&r.extensions, "beta.ts"));
}

// multi-file extension discovery (issue #1102), through the auto-discovery
// function the package resolver uses for an `extensions/` directory.

#[test]
fn should_only_load_index_ts_from_subdirectories_not_helper_modules() {
    let e = env();
    let ext = format!("{}/multifile-pkg/extensions", e.temp);
    write(
        &format!("{ext}/subagent/index.ts"),
        "export default function(api) {}",
    );
    write(
        &format!("{ext}/subagent/agents.ts"),
        "export function helper() {}",
    );
    write(
        &format!("{ext}/standalone.ts"),
        "export default function(api) {}",
    );
    let found = collect_auto_extension_entries(&ext);
    assert!(found.iter().any(|p| p.ends_with("subagent/index.ts")));
    assert!(found.iter().any(|p| p.ends_with("standalone.ts")));
    assert!(!found.iter().any(|p| p.ends_with("agents.ts")));
}

#[test]
fn should_respect_package_json_extensions_manifest_in_subdirectories() {
    let e = env();
    let ext = format!("{}/manifest-subdir-pkg/extensions", e.temp);
    write(
        &format!("{ext}/custom/package.json"),
        &json!({"hoocode": {"extensions": ["./main.ts"]}}).to_string(),
    );
    write(
        &format!("{ext}/custom/main.ts"),
        "export default function(api) {}",
    );
    write(&format!("{ext}/custom/utils.ts"), "export const util = 1;");
    let found = collect_auto_extension_entries(&ext);
    assert!(found.iter().any(|p| p.ends_with("custom/main.ts")));
    assert!(!found.iter().any(|p| p.ends_with("utils.ts")));
}

#[test]
fn should_handle_mixed_top_level_files_and_subdirectories() {
    let e = env();
    let ext = format!("{}/mixed-pkg/extensions", e.temp);
    write(
        &format!("{ext}/simple.ts"),
        "export default function(api) {}",
    );
    write(
        &format!("{ext}/complex/index.ts"),
        "export default function(api) {}",
    );
    write(&format!("{ext}/complex/a.ts"), "export const a = 1;");
    write(&format!("{ext}/complex/b.ts"), "export const b = 2;");
    let found = collect_auto_extension_entries(&ext);
    assert_eq!(found.len(), 2);
    assert!(found.iter().any(|p| p.ends_with("simple.ts")));
    assert!(found.iter().any(|p| p.ends_with("complex/index.ts")));
}

#[test]
fn should_skip_subdirectories_without_index_ts_or_manifest() {
    let e = env();
    let ext = format!("{}/no-entry-pkg/extensions", e.temp);
    write(&format!("{ext}/broken/helper.ts"), "export const x = 1;");
    write(&format!("{ext}/broken/another.ts"), "export const y = 2;");
    write(
        &format!("{ext}/valid.ts"),
        "export default function(api) {}",
    );
    let found = collect_auto_extension_entries(&ext);
    assert_eq!(found, [format!("{ext}/valid.ts")]);
}

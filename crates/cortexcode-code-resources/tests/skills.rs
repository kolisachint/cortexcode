//! Port of hoocode `packages/coding-agent/test/skills.test.ts` (v0.5.89).

use cortexcode_code_resources::source_info::{create_synthetic_source_info, SourceScope};
use cortexcode_code_resources::{
    format_skills_for_prompt, load_skills, load_skills_from_dir, LoadSkillsOptions,
    ResourceDiagnostic, Skill,
};
use std::collections::HashMap;

fn fixtures() -> String {
    format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"))
}

fn skills_dir(name: &str) -> String {
    format!("{}/skills/{name}", fixtures())
}

fn any_contains(diagnostics: &[ResourceDiagnostic], needle: &str) -> bool {
    diagnostics.iter().any(|d| d.message.contains(needle))
}

fn test_skill(name: &str, description: &str, file_path: &str, disable: bool) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: file_path.into(),
        base_dir: cortexcode_code_resources::node_path::dirname(file_path),
        source_info: create_synthetic_source_info(file_path, "test", None, None, None),
        disable_model_invocation: disable,
        allowed_tools: None,
    }
}

// loadSkillsFromDir

#[test]
fn should_load_a_valid_skill() {
    let r = load_skills_from_dir(&skills_dir("valid-skill"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "valid-skill");
    assert_eq!(
        r.skills[0].description,
        "A valid skill for testing purposes."
    );
    assert_eq!(r.skills[0].source_info.source, "test");
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_warn_when_name_doesnt_match_parent_directory() {
    let r = load_skills_from_dir(&skills_dir("name-mismatch"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "different-name");
    assert!(any_contains(
        &r.diagnostics,
        "does not match parent directory"
    ));
}

#[test]
fn should_warn_when_name_contains_invalid_characters() {
    let r = load_skills_from_dir(&skills_dir("invalid-name-chars"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(any_contains(&r.diagnostics, "invalid characters"));
}

#[test]
fn should_warn_when_name_exceeds_64_characters() {
    let r = load_skills_from_dir(&skills_dir("long-name"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(any_contains(&r.diagnostics, "exceeds 64 characters"));
}

#[test]
fn should_warn_and_skip_skill_when_description_is_missing() {
    let r = load_skills_from_dir(&skills_dir("missing-description"), "test");
    assert!(r.skills.is_empty());
    assert!(any_contains(&r.diagnostics, "description is required"));
}

#[test]
fn should_ignore_unknown_frontmatter_fields() {
    let r = load_skills_from_dir(&skills_dir("unknown-field"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_load_nested_skills_recursively() {
    let r = load_skills_from_dir(&skills_dir("nested"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "child-skill");
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_prefer_a_directorys_root_skill_md_over_nested_ones() {
    let r = load_skills_from_dir(&skills_dir("root-skill-preferred"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "root-skill-preferred");
    assert_eq!(r.skills[0].description, "Root skill should win.");
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_skip_files_without_frontmatter() {
    let r = load_skills_from_dir(&skills_dir("no-frontmatter"), "test");
    assert!(r.skills.is_empty());
    assert!(any_contains(&r.diagnostics, "description is required"));
}

#[test]
fn should_warn_and_skip_skill_when_yaml_frontmatter_is_invalid() {
    let r = load_skills_from_dir(&skills_dir("invalid-yaml"), "test");
    assert!(r.skills.is_empty());
    assert!(any_contains(&r.diagnostics, "at line"));
}

#[test]
fn should_preserve_multiline_descriptions_from_yaml() {
    let r = load_skills_from_dir(&skills_dir("multiline-description"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(r.skills[0].description.contains('\n'));
    assert!(r.skills[0]
        .description
        .contains("This is a multiline description."));
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_warn_when_name_contains_consecutive_hyphens() {
    let r = load_skills_from_dir(&skills_dir("consecutive-hyphens"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(any_contains(&r.diagnostics, "consecutive hyphens"));
}

#[test]
fn should_load_all_skills_from_fixture_directory() {
    let r = load_skills_from_dir(&format!("{}/skills", fixtures()), "test");
    assert!(r.skills.len() >= 6);
}

#[test]
fn should_return_empty_for_non_existent_directory() {
    let r = load_skills_from_dir("/non/existent/path", "test");
    assert!(r.skills.is_empty());
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_use_parent_directory_name_when_name_not_in_frontmatter() {
    let r = load_skills_from_dir(&skills_dir("valid-skill"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "valid-skill");
}

#[test]
fn should_parse_disable_model_invocation_frontmatter_field() {
    let r = load_skills_from_dir(&skills_dir("disable-model-invocation"), "test");
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].name, "disable-model-invocation");
    assert!(r.skills[0].disable_model_invocation);
    assert!(!any_contains(&r.diagnostics, "unknown frontmatter field"));
}

#[test]
fn should_default_disable_model_invocation_to_false() {
    let r = load_skills_from_dir(&skills_dir("valid-skill"), "test");
    assert_eq!(r.skills.len(), 1);
    assert!(!r.skills[0].disable_model_invocation);
}

// formatSkillsForPrompt

#[test]
fn should_return_empty_string_for_no_skills() {
    assert_eq!(format_skills_for_prompt(&[]), "");
}

#[test]
fn should_format_skills_as_xml() {
    let result = format_skills_for_prompt(&[test_skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        false,
    )]);
    for needle in [
        "<available_skills>",
        "</available_skills>",
        "<skill>",
        "<name>test-skill</name>",
        "<description>A test skill.</description>",
        "<location>/path/to/skill/SKILL.md</location>",
    ] {
        assert!(result.contains(needle), "{needle}");
    }
}

#[test]
fn should_include_intro_text_before_xml() {
    let result = format_skills_for_prompt(&[test_skill(
        "test-skill",
        "A test skill.",
        "/path/to/skill/SKILL.md",
        false,
    )]);
    let intro = &result[..result.find("<available_skills>").unwrap()];
    assert!(intro.contains("The following skills provide specialized instructions"));
    assert!(intro.contains("Use the read tool to load a skill's file"));
}

#[test]
fn should_escape_xml_special_characters() {
    let result = format_skills_for_prompt(&[test_skill(
        "test-skill",
        "A skill with <special> & \"characters\".",
        "/path/to/skill/SKILL.md",
        false,
    )]);
    assert!(result.contains("&lt;special&gt;"));
    assert!(result.contains("&amp;"));
    assert!(result.contains("&quot;characters&quot;"));
}

#[test]
fn should_format_multiple_skills() {
    let result = format_skills_for_prompt(&[
        test_skill("skill-one", "First skill.", "/path/one/SKILL.md", false),
        test_skill("skill-two", "Second skill.", "/path/two/SKILL.md", false),
    ]);
    assert!(result.contains("<name>skill-one</name>"));
    assert!(result.contains("<name>skill-two</name>"));
    assert_eq!(result.matches("<skill>").count(), 2);
}

#[test]
fn should_exclude_skills_with_disable_model_invocation_from_prompt() {
    let result = format_skills_for_prompt(&[
        test_skill(
            "visible-skill",
            "A visible skill.",
            "/path/visible/SKILL.md",
            false,
        ),
        test_skill(
            "hidden-skill",
            "A hidden skill.",
            "/path/hidden/SKILL.md",
            true,
        ),
    ]);
    assert!(result.contains("<name>visible-skill</name>"));
    assert!(!result.contains("<name>hidden-skill</name>"));
    assert_eq!(result.matches("<skill>").count(), 1);
}

#[test]
fn should_return_empty_string_when_all_skills_are_hidden() {
    let result = format_skills_for_prompt(&[test_skill(
        "hidden-skill",
        "A hidden skill.",
        "/path/hidden/SKILL.md",
        true,
    )]);
    assert_eq!(result, "");
}

// loadSkills with options

fn options(skill_paths: Vec<String>, include_claude: bool) -> LoadSkillsOptions {
    LoadSkillsOptions {
        cwd: format!("{}/empty-cwd", fixtures()),
        agent_dir: format!("{}/empty-agent", fixtures()),
        skill_paths,
        include_defaults: true,
        include_claude,
        namespaces: HashMap::new(),
    }
}

#[test]
fn should_load_from_explicit_skill_paths() {
    let r = load_skills(&options(vec![skills_dir("valid-skill")], false));
    assert_eq!(r.skills.len(), 1);
    assert_eq!(r.skills[0].source_info.scope, SourceScope::Temporary);
    assert!(r.diagnostics.is_empty());
}

#[test]
fn should_warn_when_skill_path_does_not_exist() {
    let r = load_skills(&options(vec!["/non/existent/path".into()], false));
    assert!(r.skills.is_empty());
    assert!(any_contains(&r.diagnostics, "does not exist"));
}

#[test]
fn should_expand_tilde_in_skill_paths() {
    let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
    let with_tilde = load_skills(&options(vec!["~/.hoocode/agent/skills".into()], true));
    let without = load_skills(&options(
        vec![format!("{home}/.hoocode/agent/skills")],
        true,
    ));
    assert_eq!(with_tilde.skills.len(), without.skills.len());
}

// collision handling

#[test]
fn should_detect_name_collisions_and_keep_first_skill() {
    let first = load_skills_from_dir(&format!("{}/skills-collision/first", fixtures()), "first");
    let second = load_skills_from_dir(&format!("{}/skills-collision/second", fixtures()), "second");
    let mut map: HashMap<String, Skill> = HashMap::new();
    let mut warnings = Vec::new();
    for skill in first.skills {
        map.insert(skill.name.clone(), skill);
    }
    for skill in second.skills {
        if let Some(existing) = map.get(&skill.name) {
            warnings.push(format!(
                "name collision: \"{}\" already loaded from {}",
                skill.name, existing.file_path
            ));
        } else {
            map.insert(skill.name.clone(), skill);
        }
    }
    assert_eq!(map.len(), 1);
    assert_eq!(map["calendar"].source_info.source, "first");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("name collision"));
}

// Beyond the TS file: load_skills' own collision diagnostics, ignore files,
// plugin roots, allowed-tools and namespaces.

#[test]
fn load_skills_reports_collisions_after_other_diagnostics() {
    let r = load_skills(&options(
        vec![
            format!("{}/skills-collision/first", fixtures()),
            format!("{}/skills-collision/second", fixtures()),
        ],
        false,
    ));
    assert_eq!(r.skills.len(), 1);
    assert!(r.skills[0].file_path.contains("/first/"));
    let last = r.diagnostics.last().unwrap();
    assert_eq!(last.message, "name \"calendar\" collision");
    let collision = last.collision.as_ref().unwrap();
    assert!(collision.winner_path.contains("/first/"));
    assert!(collision.loser_path.contains("/second/"));
}

#[test]
fn ignore_files_plugin_roots_allowed_tools_and_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().into_owned();
    let write = |rel: &str, content: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    };
    let skill = |name: &str, extra: &str| {
        format!("---\nname: {name}\ndescription: The {name} skill.\n{extra}---\nBody\n")
    };
    write(
        "keep/SKILL.md",
        &skill("keep", "allowed-tools: Read, Grep, NotebookEdit\n"),
    );
    write("hidden/SKILL.md", &skill("hidden", ""));
    write(".gitignore", "hidden/\n");
    write("plug/.claude-plugin/plugin.json", "{\"name\": \"plug\"}");
    write("plug/skills/inner/SKILL.md", &skill("inner", ""));
    write("loose.md", &skill("loose", ""));

    let r = load_skills_from_dir(&root, "test");
    let names: Vec<&str> = r.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["keep", "loose"]);
    assert_eq!(
        r.skills[0].allowed_tools,
        Some(vec!["read".to_string(), "SearchCodebase".to_string()])
    );
    assert!(any_contains(
        &r.diagnostics,
        "tool \"NotebookEdit\" has no hoocode equivalent"
    ));
    let prompt = format_skills_for_prompt(&r.skills);
    assert!(prompt.contains("    <tools>read, SearchCodebase</tools>\n"));

    let mut opts = options(vec![format!("{root}/keep")], false);
    opts.namespaces
        .insert(format!("{root}/keep"), "myplug".into());
    let r = load_skills(&opts);
    assert_eq!(r.skills[0].name, "myplug:keep");
}

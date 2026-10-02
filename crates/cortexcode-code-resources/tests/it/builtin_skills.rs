//! Port of hoocode `packages/coding-agent/test/builtin-skills.test.ts` (v0.5.89).
//! The `/new-canvas` brief case (canvasBuildBrief) belongs to the canvas port
//! (ledger 12.7); only the guide path it names is checked here.

use cortexcode_code_resources::builtin_skills::{
    builtin_skill_paths, builtin_skills_cache_dir, canvas_design_guide_path,
    materialize_builtin_skills, BuiltinSkillGate, BUILTIN_SKILLS, EMBEDDED_SKILLS,
};
use cortexcode_code_resources::{format_skills_for_prompt, load_skills_from_dir, DiagnosticType};

const ON: BuiltinSkillGate = BuiltinSkillGate {
    enable_plugin_tools: true,
};
const OFF: BuiltinSkillGate = BuiltinSkillGate {
    enable_plugin_tools: false,
};

fn agent_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

fn embedded(path: &str) -> &'static str {
    EMBEDDED_SKILLS
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, c)| *c)
        .unwrap_or("")
}

// the built-in skill catalog

#[test]
fn has_an_embedded_skill_md_for_every_catalog_entry() {
    for skill in BUILTIN_SKILLS {
        assert!(
            !embedded(&format!("{}/SKILL.md", skill.name)).is_empty(),
            "{}",
            skill.name
        );
    }
    let mut embedded_names: Vec<&str> = EMBEDDED_SKILLS
        .iter()
        .map(|(p, _)| p.split('/').next().unwrap())
        .collect();
    embedded_names.sort();
    embedded_names.dedup();
    let mut catalog: Vec<&str> = BUILTIN_SKILLS.iter().map(|s| s.name).collect();
    catalog.sort();
    assert_eq!(embedded_names, catalog);
}

#[test]
fn parses_every_shipped_skill_with_the_frontmatter_name_matching_its_directory() {
    let (_d, agent) = agent_dir();
    let root = materialize_builtin_skills(&agent).unwrap();
    for skill in BUILTIN_SKILLS {
        let result = load_skills_from_dir(&format!("{root}/{}", skill.name), "user");
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.kind != DiagnosticType::Error),
            "{}",
            skill.name
        );
        assert!(
            result.skills.iter().any(|s| s.name == skill.name),
            "{}",
            skill.name
        );
    }
}

// the plugin-authoring skill

#[test]
fn says_when_to_use_it_not_just_what_it_is() {
    let (_d, agent) = agent_dir();
    let root = materialize_builtin_skills(&agent).unwrap();
    let skill = &load_skills_from_dir(&format!("{root}/plugin-authoring"), "user").skills[0];
    assert!(skill.description.contains("ProposePlugin"));
}

#[test]
fn carries_the_guidance_the_plugin_tools_stopped_shipping() {
    let body = embedded("plugin-authoring/SKILL.md").to_lowercase();
    for needle in [
        "portab",
        "capability, not the",
        "second hook",
        "never grant a subagent",
        "publish",
    ] {
        assert!(body.contains(needle), "{needle}");
    }
}

// materializing

#[test]
fn writes_the_embedded_content_to_a_content_addressed_cache_dir() {
    let (_d, agent) = agent_dir();
    let root = materialize_builtin_skills(&agent).unwrap();
    assert_eq!(root, builtin_skills_cache_dir(&agent));
    for (relative, content) in EMBEDDED_SKILLS {
        assert_eq!(
            std::fs::read_to_string(format!("{root}/{relative}")).unwrap(),
            *content
        );
    }
    // Same content as hoocode's templates, so the same cache directory name.
    assert!(root.ends_with("/cache/builtin-skills/965fb5cbad49"));
}

#[test]
fn repairs_a_file_that_was_corrupted_in_the_cache() {
    let (_d, agent) = agent_dir();
    let root = materialize_builtin_skills(&agent).unwrap();
    let (first, content) = EMBEDDED_SKILLS[0];
    std::fs::write(format!("{root}/{first}"), "corrupted").unwrap();
    materialize_builtin_skills(&agent);
    assert_eq!(
        std::fs::read_to_string(format!("{root}/{first}")).unwrap(),
        content
    );
}

#[test]
fn is_idempotent() {
    let (_d, agent) = agent_dir();
    assert_eq!(
        materialize_builtin_skills(&agent),
        materialize_builtin_skills(&agent)
    );
}

// gating

#[test]
fn withholds_a_gated_skill_when_its_feature_is_off() {
    let (_d, agent) = agent_dir();
    let cache = builtin_skills_cache_dir(&agent);
    assert_eq!(
        builtin_skill_paths(OFF, &agent),
        [
            format!("{cache}/artifact-design"),
            format!("{cache}/canvas-design")
        ]
    );
}

#[test]
fn keeps_canvas_design_out_of_the_per_turn_skill_list() {
    let (_d, agent) = agent_dir();
    let root = materialize_builtin_skills(&agent).unwrap();
    let skill = load_skills_from_dir(&format!("{root}/canvas-design"), "user")
        .skills
        .remove(0);
    assert!(skill.disable_model_invocation);
    assert_eq!(format_skills_for_prompt(&[skill]), "");
}

#[test]
fn points_new_canvas_at_the_guide() {
    let (_d, agent) = agent_dir();
    assert_eq!(
        canvas_design_guide_path(&agent),
        Some(format!(
            "{}/canvas-design/SKILL.md",
            builtin_skills_cache_dir(&agent)
        ))
    );
}

#[test]
fn contributes_the_skill_directory_when_the_feature_is_on() {
    let (_d, agent) = agent_dir();
    assert!(builtin_skill_paths(ON, &agent).contains(&format!(
        "{}/plugin-authoring",
        builtin_skills_cache_dir(&agent)
    )));
}

#[test]
fn materializes_the_whole_tree_but_contributes_only_enabled_skills() {
    let (_d, agent) = agent_dir();
    let contributed = builtin_skill_paths(OFF, &agent);
    let cache = builtin_skills_cache_dir(&agent);
    assert!(
        std::fs::read_to_string(format!("{cache}/plugin-authoring/SKILL.md"))
            .unwrap()
            .contains("plugin-authoring")
    );
    assert!(!contributed.contains(&format!("{cache}/plugin-authoring")));
}

// degrading

fn unwritable_agent_dir(agent: &str) -> String {
    let blocked = format!("{agent}/not-a-directory");
    std::fs::write(&blocked, "").unwrap();
    blocked
}

#[test]
fn returns_no_paths_rather_than_throwing_when_the_cache_cannot_be_written() {
    let (_d, agent) = agent_dir();
    assert!(builtin_skill_paths(ON, &unwritable_agent_dir(&agent)).is_empty());
}

#[test]
fn reports_the_failure_as_none_rather_than_a_half_written_root() {
    let (_d, agent) = agent_dir();
    assert_eq!(
        materialize_builtin_skills(&unwritable_agent_dir(&agent)),
        None
    );
}

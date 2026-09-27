//! Port of hoocode `packages/coding-agent/test/sdk-skills.test.ts` (v0.5.89):
//! a session exposes its resource loader's skills.

mod common;

use std::sync::Arc;

use common::{Harness, HarnessOptions};
use cortexcode_code_agent_session::{DefaultResources, ResourceLoader, StaticResourceLoader};
use cortexcode_code_prompts::PromptSkill;
use cortexcode_code_resources::DefaultResourceLoaderOptions;

fn temp_with_skill() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("skills").join("test-skill");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test skill for SDK tests.\n---\n\n# Test Skill\n\nThis is a test skill.\n",
    )
    .unwrap();
    temp
}

#[test]
fn should_discover_skills_by_default_and_expose_them_on_the_session() {
    let temp = temp_with_skill();
    let base = temp.path().to_string_lossy().into_owned();
    // `createAgentSession({ cwd: tempDir, agentDir: tempDir })` builds a
    // DefaultResourceLoader over the agent dir.
    let loader = DefaultResources::loaded(DefaultResourceLoaderOptions {
        cwd: base.clone(),
        agent_dir: base.clone(),
        home: Some(format!("{base}/home")),
        user_agents_dir: Some(format!("{base}/home/.agents")),
        ..Default::default()
    });
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(loader)),
        ..Default::default()
    });
    let skills = h.session.resource_loader().skills();
    assert!(!skills.is_empty());
    assert!(skills.iter().any(|s| s.name == "test-skill"));
}

#[test]
fn should_have_empty_skills_when_the_resource_loader_returns_none() {
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(StaticResourceLoader::default())),
        ..Default::default()
    });
    assert!(h.session.resource_loader().skills().is_empty());
}

struct CustomSkills;

impl ResourceLoader for CustomSkills {
    fn skills(&self) -> Vec<PromptSkill> {
        vec![PromptSkill {
            name: "custom-skill".into(),
            description: "A custom skill".into(),
            file_path: "/fake/path/SKILL.md".into(),
            ..Default::default()
        }]
    }
}

#[test]
fn should_use_provided_skills_when_the_resource_loader_supplies_them() {
    let h = Harness::new(HarnessOptions {
        resource_loader: Some(Arc::new(CustomSkills)),
        ..Default::default()
    });
    let skills = h.session.resource_loader().skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "custom-skill");
}

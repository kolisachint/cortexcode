//! subagent-skills.test.ts: a spawned subagent (agent body as
//! `--system-prompt`, no `--no-skills`) runs the same skill discovery as the
//! root, and the prompt lists the skills when it has the `read` tool.

use cortexcode_code_prompts::{build_system_prompt, BuildSystemPromptOptions, PromptSkill};
use cortexcode_code_resources::package_discovery::collect_ancestor_agents_skill_dirs;
use cortexcode_code_resources::skills::{load_skills, LoadSkillsOptions};

struct Dirs {
    project: tempfile::TempDir,
    agent: tempfile::TempDir,
}

fn setup() -> Dirs {
    let project = tempfile::tempdir().unwrap();
    let agent = tempfile::tempdir().unwrap();
    let skill = project.path().join(".agents/skills/greeting");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: greeting\ndescription: Say hello in a friendly, on-brand way. Use when greeting a user.\n---\n\n# Greeting\n\nAlways greet warmly.\n",
    )
    .unwrap();
    Dirs { project, agent }
}

fn skills(d: &Dirs) -> Vec<PromptSkill> {
    let cwd = d.project.path().to_string_lossy().into_owned();
    let mut options = LoadSkillsOptions::new(cwd.clone(), d.agent.path().to_string_lossy());
    options.skill_paths = collect_ancestor_agents_skill_dirs(&cwd);
    options.include_claude = false;
    load_skills(&options)
        .skills
        .into_iter()
        .map(|s| PromptSkill {
            name: s.name,
            description: s.description,
            file_path: s.file_path,
            allowed_tools: s.allowed_tools.unwrap_or_default(),
            disable_model_invocation: s.disable_model_invocation,
        })
        .collect()
}

fn subagent_prompt(d: &Dirs, tools: &[&str]) -> String {
    build_system_prompt(&BuildSystemPromptOptions {
        custom_prompt: Some(
            "You are an explore-only agent running inside hoocode. You read code and produce summaries. You NEVER edit files.".into(),
        ),
        selected_tools: Some(tools.iter().map(|t| t.to_string()).collect()),
        skills: skills(d),
        cwd: d.project.path().to_string_lossy().into_owned(),
        ..Default::default()
    })
}

#[test]
fn discovers_the_project_skill() {
    let d = setup();
    assert!(skills(&d).iter().any(|s| s.name == "greeting"));
}

#[test]
fn injects_the_skill_card_for_a_read_capable_subagent() {
    let d = setup();
    let prompt = subagent_prompt(&d, &["read", "SearchCodebase"]);
    assert!(prompt.contains("<available_skills>"));
    assert!(prompt.contains("<name>greeting</name>"));
    assert!(prompt.contains("Say hello in a friendly, on-brand way"));
    assert!(prompt.contains(".agents/skills/greeting"));
}

#[test]
fn omits_skills_without_the_read_tool() {
    let d = setup();
    let prompt = subagent_prompt(&d, &["bash"]);
    assert!(!prompt.contains("<available_skills>"));
    assert!(!prompt.contains("greeting"));
}

//! Ports of hoocode `packages/agent/test/harness/skills.test.ts` and
//! `prompt-templates.test.ts` (v0.5.89) over [`LocalExecutionEnv`], plus
//! ignore files and `executeShellWithCapture`. Unix only (symlinks, `sh`).
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use cortexcode_agent_harness::utils::shell_output::execute_shell_with_capture;
use cortexcode_agent_harness::{
    format_prompt_template_invocation, load_prompt_templates, load_skills,
    load_sourced_prompt_templates, load_sourced_skills, ExecOptions, ExecutionEnv,
    LocalExecutionEnv, PromptTemplate, Skill, SkillDiagnostic, Sourced,
};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = format!(
            "harness-loaders-{:x}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(id);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
    fn join(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
    fn symlink(&self, target: &str, link: &str) {
        std::os::unix::fs::symlink(self.join(target), self.join(link)).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Source {
    User,
    Project,
}

// --- skills.test.ts ---

#[tokio::test]
async fn loads_skill_md_files_through_the_execution_environment() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir(".agents/skills/example", true)
        .await
        .unwrap();
    env.write_file(
        ".agents/skills/example/SKILL.md",
        b"---\nname: example\ndescription: Example skill\ndisable-model-invocation: true\n---\nUse this skill.\n",
    )
    .await
    .unwrap();
    let loaded = load_skills(&env, &[".agents/skills"]).await;
    assert_eq!(loaded.diagnostics, []);
    assert_eq!(
        loaded.skills,
        [Skill {
            name: "example".into(),
            description: "Example skill".into(),
            content: "Use this skill.".into(),
            file_path: root.join(".agents/skills/example/SKILL.md"),
            disable_model_invocation: Some(true),
        }]
    );
}

#[tokio::test]
async fn loads_skills_through_symlinked_directories() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("actual/example", true).await.unwrap();
    env.write_file(
        "actual/example/SKILL.md",
        b"---\nname: example\ndescription: Example skill\n---\nUse this skill.",
    )
    .await
    .unwrap();
    root.symlink("actual", "skills-link");
    let loaded = load_skills(&env, &["skills-link"]).await;
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["example"]);
    assert_eq!(
        loaded.skills[0].file_path,
        root.join("skills-link/example/SKILL.md")
    );
}

#[tokio::test]
async fn preserves_source_info_for_sourced_skills() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("user/example", true).await.unwrap();
    env.write_file(
        "user/example/SKILL.md",
        b"---\nname: example\ndescription: Example skill\n---\nUse this skill.",
    )
    .await
    .unwrap();
    let loaded = load_sourced_skills(&env, &[("user".to_string(), Source::User)]).await;
    assert_eq!(loaded.diagnostics, []);
    assert_eq!(
        loaded.skills,
        [Sourced {
            item: Skill {
                name: "example".into(),
                description: "Example skill".into(),
                content: "Use this skill.".into(),
                file_path: root.join("user/example/SKILL.md"),
                disable_model_invocation: Some(false),
            },
            source: Source::User,
        }]
    );
}

#[tokio::test]
async fn attaches_source_info_to_skill_diagnostics() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("user/broken", true).await.unwrap();
    env.write_file(
        "user/broken/SKILL.md",
        b"---\nname: broken\n---\nMissing description.",
    )
    .await
    .unwrap();
    let loaded = load_sourced_skills(&env, &[("user".to_string(), Source::User)]).await;
    assert_eq!(loaded.skills, []);
    assert_eq!(
        loaded.diagnostics,
        [Sourced {
            item: SkillDiagnostic {
                message: "description is required".into(),
                path: root.join("user/broken/SKILL.md"),
            },
            source: Source::User,
        }]
    );
}

#[tokio::test]
async fn loads_direct_markdown_children_only_from_the_root_directory() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("skills/nested", true).await.unwrap();
    env.write_file(
        "skills/root.md",
        b"---\ndescription: Root skill\n---\nRoot content",
    )
    .await
    .unwrap();
    env.write_file(
        "skills/nested/ignored.md",
        b"---\ndescription: Ignored\n---\nIgnored content",
    )
    .await
    .unwrap();
    let loaded = load_skills(&env, &["skills"]).await;
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["skills"]);
    assert_eq!(loaded.skills[0].content, "Root content");
}

#[tokio::test]
async fn honors_ignore_files_and_skips_dot_entries() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    for name in ["keep", "skip", "nested/deep", ".hidden", "node_modules/pkg"] {
        let skill_name = name.rsplit('/').next().unwrap();
        env.write_file(
            &format!("skills/{name}/SKILL.md"),
            format!("---\nname: {skill_name}\ndescription: d\n---\nbody").as_bytes(),
        )
        .await
        .unwrap();
    }
    env.write_file("skills/.gitignore", b"# comment\n/skip\n")
        .await
        .unwrap();
    env.write_file("skills/nested/.ignore", b"deep/\n")
        .await
        .unwrap();
    let loaded = load_skills(&env, &["skills", "missing-dir"]).await;
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["keep"]);
}

#[tokio::test]
async fn validates_skill_names_against_their_directory() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.write_file(
        "skills/My_Skill/SKILL.md",
        b"---\nname: other\ndescription: d\n---\nbody",
    )
    .await
    .unwrap();
    let loaded = load_skills(&env, &["skills"]).await;
    // Name warnings do not drop the skill.
    assert_eq!(loaded.skills.len(), 1);
    let messages: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["name \"other\" does not match parent directory \"My_Skill\""]
    );
}

// --- prompt-templates.test.ts ---

fn template(name: &str, description: &str, content: &str) -> PromptTemplate {
    PromptTemplate {
        name: name.into(),
        description: Some(description.into()),
        content: content.into(),
    }
}

#[tokio::test]
async fn loads_markdown_templates_non_recursively_from_one_or_more_dirs() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("a/nested", true).await.unwrap();
    env.create_dir("b", true).await.unwrap();
    env.write_file("a/one.md", b"---\ndescription: One template\n---\nHello $1")
        .await
        .unwrap();
    env.write_file("a/nested/ignored.md", b"Ignored")
        .await
        .unwrap();
    env.write_file("b/two.md", b"First line description\nBody")
        .await
        .unwrap();
    let loaded = load_prompt_templates(&env, &["a", "b"]).await;
    assert_eq!(loaded.diagnostics, []);
    assert_eq!(
        loaded.prompt_templates,
        [
            template("one", "One template", "Hello $1"),
            template(
                "two",
                "First line description",
                "First line description\nBody"
            ),
        ]
    );
}

#[tokio::test]
async fn preserves_source_info_for_sourced_prompt_templates() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("prompts", true).await.unwrap();
    env.write_file(
        "prompts/example.md",
        b"---\ndescription: Example\n---\nExample body",
    )
    .await
    .unwrap();
    let loaded =
        load_sourced_prompt_templates(&env, &[("prompts".to_string(), Source::Project)]).await;
    assert!(loaded.diagnostics.is_empty());
    assert_eq!(
        loaded.prompt_templates,
        [Sourced {
            item: template("example", "Example", "Example body"),
            source: Source::Project,
        }]
    );
}

#[tokio::test]
async fn attaches_source_info_to_prompt_template_diagnostics() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.write_file("broken.md", b"---\ndescription: [unterminated\n---\nBody")
        .await
        .unwrap();
    let loaded =
        load_sourced_prompt_templates(&env, &[("broken.md".to_string(), Source::User)]).await;
    assert!(loaded.prompt_templates.is_empty());
    assert_eq!(loaded.diagnostics.len(), 1);
    assert_eq!(loaded.diagnostics[0].item.path, root.join("broken.md"));
    assert_eq!(loaded.diagnostics[0].source, Source::User);
}

#[tokio::test]
async fn loads_explicit_markdown_files_and_symlinked_files() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.write_file("target.md", b"---\ndescription: Target\n---\nTarget body")
        .await
        .unwrap();
    root.symlink("target.md", "link.md");
    let loaded = load_prompt_templates(&env, &["target.md", "link.md"]).await;
    assert_eq!(
        loaded.prompt_templates,
        [
            template("target", "Target", "Target body"),
            template("link", "Target", "Target body"),
        ]
    );
}

#[test]
fn format_prompt_template_invocation_substitutes_command_arguments() {
    let content = "$1 ${@:2} $ARGUMENTS";
    let args = ["hello world".to_string(), "test".to_string()];
    assert_eq!(
        format_prompt_template_invocation(&template("one", "", content), &args),
        "hello world test hello world test"
    );
}

#[tokio::test]
async fn long_first_lines_become_truncated_descriptions() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let line = "x".repeat(70);
    env.write_file("long.MD", format!("\n\n{line}\nmore").as_bytes())
        .await
        .unwrap();
    // `.MD` is not loaded (the extension check is case-sensitive)...
    assert!(load_prompt_templates(&env, &["long.MD"])
        .await
        .prompt_templates
        .is_empty());
    env.write_file("long.md", format!("\n\n{line}\nmore").as_bytes())
        .await
        .unwrap();
    let loaded = load_prompt_templates(&env, &["long.md"]).await;
    assert_eq!(
        loaded.prompt_templates[0].description.as_deref(),
        Some(format!("{}...", "x".repeat(60)).as_str())
    );
}

// --- shell-output.ts executeShellWithCapture ---

#[tokio::test]
async fn captures_shell_output_through_the_env() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = seen.clone();
    let result = execute_shell_with_capture(
        &env,
        "printf 'a\\r\\nb'; printf 'c' >&2; exit 2",
        ExecOptions::default(),
        Some(Arc::new(move |chunk: &str| {
            sink.lock().unwrap().push_str(chunk)
        })),
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(2));
    assert!(!result.cancelled && !result.truncated);
    let mut chars: Vec<char> = result.output.chars().collect();
    chars.sort();
    // stdout and stderr interleave nondeterministically; `\r` is dropped.
    assert_eq!(chars, ['\n', 'a', 'b', 'c']);
    assert_eq!(seen.lock().unwrap().len(), 4);
}

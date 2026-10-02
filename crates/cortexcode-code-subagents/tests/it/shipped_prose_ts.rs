//! The Task half of the pin's `coding-agent/test/shipped-prose.test.ts`
//! ("the Task delegation prompt has one source"), against the template files,
//! plus a byte check of the crate's copies against the pin when it is present.

use std::path::{Path, PathBuf};

use cortexcode_code_subagents::tools::build_task_main_prompt;

fn templates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/prompts")
}

fn template(name: &str) -> String {
    std::fs::read_to_string(templates().join(name)).unwrap()
}

/// A cwd with no project agents: the built-in explore/plan agents are
/// background agents, so this takes the with-background branch.
fn cwd() -> PathBuf {
    std::env::temp_dir().join(format!("shipped-prose-{}", std::process::id()))
}

#[test]
fn is_assembled_from_templates_prompts_and_nothing_else() {
    let background = template("task-background-agents.md");
    let expected = template("task-main.md")
        .replacen("{{BACKGROUND_GUIDANCE}}", background.trim(), 1)
        .trim()
        .to_string();
    assert_eq!(build_task_main_prompt(&cwd()), expected);
}

#[test]
fn keeps_the_substitution_token_the_background_variants_fill() {
    assert!(template("task-main.md").contains("{{BACKGROUND_GUIDANCE}}"));
    assert!(!template("task-background-agents.md").is_empty());
    assert!(!template("task-background-none.md").is_empty());
}

#[test]
fn leaves_no_substitution_token_in_the_rendered_prompt() {
    assert!(!build_task_main_prompt(&cwd()).contains("{{"));
}

#[test]
fn template_copies_match_the_pin() {
    let pin = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/hoocode-pin/packages/coding-agent/templates/prompts");
    if !pin.is_dir() {
        eprintln!("pinned hoocode not built: skipping the copy check");
        return;
    }
    for name in [
        "task-main.md",
        "task-background-agents.md",
        "task-background-none.md",
    ] {
        assert_eq!(
            template(name),
            std::fs::read_to_string(pin.join(name)).unwrap(),
            "{name} differs from the pin"
        );
    }
}

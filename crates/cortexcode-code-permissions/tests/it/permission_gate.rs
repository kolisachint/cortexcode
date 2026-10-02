//! Port of hoocode `test/permission-gate-mutation-path.test.ts` (v0.5.89), plus
//! the hard-enforcement rules of `permission-gate.ts`.
//!
//! The project config is `.cortexcode/hoo-config.json`; patterns keep hoocode's
//! `.hoocode/...` text since they are plain strings here.

use cortexcode_code_permissions::{describe_tool, evaluate, Verdict};
use serde_json::{json, Value};
use std::path::Path;

/// `ctx.ui.select` that records the prompt and answers "No (block)".
fn run(config: Value, cwd: &Path, tool: &str, input: Value) -> (Verdict, Option<String>) {
    let config = config.as_object().unwrap().clone();
    let verdict = evaluate(&config, cwd, tool, &input, true);
    let prompt =
        (verdict == Verdict::Prompt).then(|| format!("Allow: {}", describe_tool(tool, &input)));
    (verdict, prompt)
}

fn cwd() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn names_the_file_in_the_approval_prompt_instead_of_unknown() {
    let d = cwd();
    let config = json!({"active_mode": "build", "modes": {"build": {}}});
    let prompts: Vec<String> = ["edit", "write"]
        .iter()
        .map(|t| {
            run(config.clone(), d.path(), t, json!({"path": "src/app.ts"}))
                .1
                .unwrap()
        })
        .collect();
    assert_eq!(
        prompts,
        ["Allow: edit src/app.ts", "Allow: write src/app.ts"]
    );
}

#[test]
fn still_names_the_file_when_a_model_sends_file_path() {
    let d = cwd();
    let (_, prompt) = run(
        json!({"active_mode": "build", "modes": {"build": {}}}),
        d.path(),
        "edit",
        json!({"file_path": "src/app.ts"}),
    );
    assert_eq!(prompt.as_deref(), Some("Allow: edit src/app.ts"));
}

fn docs() -> Value {
    json!({"active_mode": "docs", "modes": {"docs": {"allowed_write_paths": ["docs/*"], "auto_allow": ["edit", "write"]}}})
}

fn plan() -> Value {
    json!({"active_mode": "plan", "modes": {"plan": {"allowed_write_paths": [".hoocode/plans/*"], "auto_allow": ["write"]}}})
}

#[test]
fn lets_allowed_write_paths_permit_a_matching_write() {
    let d = cwd();
    assert_eq!(
        run(docs(), d.path(), "edit", json!({"path": "docs/guide.md"})).0,
        Verdict::Allow
    );
}

#[test]
fn still_blocks_a_write_outside_allowed_write_paths_and_says_which_file() {
    let d = cwd();
    match run(docs(), d.path(), "edit", json!({"path": "src/app.ts"})).0 {
        Verdict::Block(reason) => assert!(reason.contains("src/app.ts")),
        v => panic!("{v:?}"),
    }
}

#[test]
fn accepts_a_windows_style_relative_path_against_a_forward_slash_pattern() {
    let d = cwd();
    assert_eq!(
        run(
            plan(),
            d.path(),
            "write",
            json!({"path": ".hoocode\\plans\\s1.md"})
        )
        .0,
        Verdict::Allow
    );
}

#[test]
fn accepts_an_absolute_path_inside_an_allowed_relative_pattern() {
    let d = cwd();
    let path = d.path().join(".hoocode").join("plans").join("s1.md");
    assert_eq!(
        run(
            plan(),
            d.path(),
            "write",
            json!({"path": path.to_string_lossy()})
        )
        .0,
        Verdict::Allow
    );
}

#[test]
fn still_blocks_an_absolute_path_that_escapes_the_project_root() {
    let d = cwd();
    assert!(matches!(
        run(plan(), d.path(), "write", json!({"path": "/etc/passwd"})).0,
        Verdict::Block(_)
    ));
}

#[test]
fn does_not_let_a_patterns_dots_match_arbitrary_characters() {
    let d = cwd();
    assert!(matches!(
        run(
            plan(),
            d.path(),
            "write",
            json!({"path": "Xhoocode/plans/evil.md"})
        )
        .0,
        Verdict::Block(_)
    ));
}

#[test]
fn blocks_a_mutation_whose_path_cannot_be_identified() {
    let d = cwd();
    assert!(matches!(
        run(docs(), d.path(), "edit", json!({})).0,
        Verdict::Block(_)
    ));
}

// Hard enforcement (always, with or without a UI).

#[test]
fn hard_rules_apply_without_a_ui_and_everything_else_runs() {
    let d = cwd();
    let config = json!({"modes": {"build": {
        "denied_tools": ["write"],
        "denied_bash_commands": ["\\brm\\b"],
        "allowed_bash_commands": ["^git\\s", "^ls"]
    }}});
    let c = config.as_object().unwrap().clone();
    let headless = |tool: &str, input: Value| evaluate(&c, d.path(), tool, &input, false);
    assert_eq!(
        headless("write", json!({"path": "a"})),
        Verdict::Block("Tool \"write\" is denied in mode \"build\".".into())
    );
    assert_eq!(
        headless("bash", json!({"command": "rm -rf x"})),
        Verdict::Block("Bash command matches a denied pattern in mode \"build\": \\brm\\b".into())
    );
    assert_eq!(
        headless("bash", json!({"command": "cat x"})),
        Verdict::Block(
            "Bash command is not permitted in mode \"build\". Allowed patterns: ^git\\s, ^ls"
                .into()
        )
    );
    assert_eq!(
        headless("bash", json!({"command": "git status"})),
        Verdict::Allow
    );
    // No UI: gated tools run without a prompt.
    assert_eq!(headless("edit", json!({"path": "a"})), Verdict::Allow);
    assert_eq!(
        headless("webfetch", json!({"url": "https://x"})),
        Verdict::Allow
    );

    let allow = json!({"modes": {"plan": {"enabled_tools": ["read"]}}, "active_mode": "plan"});
    let c = allow.as_object().unwrap().clone();
    assert_eq!(
        evaluate(&c, d.path(), "bash", &json!({"command": "ls"}), false),
        Verdict::Block("Tool \"bash\" is not enabled in mode \"plan\" (enabled: read).".into())
    );
}

#[test]
fn ungated_tools_are_never_prompted_and_bash_is_described_collapsed() {
    let d = cwd();
    assert_eq!(
        run(json!({}), d.path(), "read", json!({"path": "a"})).0,
        Verdict::Allow
    );
    let (_, prompt) = run(
        json!({}),
        d.path(),
        "bash",
        json!({"command": "  ls\n  -la  "}),
    );
    assert_eq!(prompt.as_deref(), Some("Allow: $  ls -la "));
    assert_eq!(
        describe_tool("websearch", &json!({"query": "rust"})),
        "websearch \"rust\""
    );
    assert_eq!(describe_tool("webfetch", &json!({})), "webfetch (unknown)");
}

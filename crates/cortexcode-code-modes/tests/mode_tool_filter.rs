//! Port of hoocode `packages/coding-agent/test/mode-tool-filter.test.ts` (v0.5.89).

use cortexcode_code_modes::config::{merge_configs, HooConfig};
use cortexcode_code_modes::{
    build_approve_message, build_mode_system_prompt, parse_plan_sections, PlanSections,
    DEFAULT_MODE_PROMPTS,
};
use serde_json::{json, Value};

fn cfg(v: Value) -> HooConfig {
    v.as_object().unwrap().clone()
}

fn field(merged: &HooConfig, mode: &str, key: &str) -> Value {
    merged["modes"][mode]
        .get(key)
        .cloned()
        .unwrap_or(Value::Null)
}

// mergeConfigs: mode enabled_tools

#[test]
fn uses_global_enabled_tools_when_project_has_none() {
    let m = merge_configs(
        &cfg(json!({"modes": {"plan": {"enabled_tools": ["read", "bash", "SearchCodebase"]}}})),
        &cfg(json!({})),
    );
    assert_eq!(
        field(&m, "plan", "enabled_tools"),
        json!(["read", "bash", "SearchCodebase"])
    );
}

#[test]
fn project_enabled_tools_overrides_global() {
    let m = merge_configs(
        &cfg(json!({"modes": {"plan": {"enabled_tools": ["read", "bash"]}}})),
        &cfg(json!({"modes": {"plan": {"enabled_tools": ["read", "SearchCodebase"]}}})),
    );
    assert_eq!(
        field(&m, "plan", "enabled_tools"),
        json!(["read", "SearchCodebase"])
    );
}

#[test]
fn project_enabled_tools_is_used_even_when_global_has_different_mode() {
    let m = merge_configs(
        &cfg(json!({"modes": {"build": {"enabled_tools": ["read", "write", "edit"]}}})),
        &cfg(json!({"modes": {"plan": {"enabled_tools": ["read"]}}})),
    );
    assert_eq!(field(&m, "plan", "enabled_tools"), json!(["read"]));
    assert_eq!(
        field(&m, "build", "enabled_tools"),
        json!(["read", "write", "edit"])
    );
}

// allowed_write_paths

#[test]
fn uses_global_allowed_write_paths_when_project_has_none() {
    let m = merge_configs(
        &cfg(json!({"modes": {"plan": {"allowed_write_paths": [".hoocode/plan.md"]}}})),
        &cfg(json!({})),
    );
    assert_eq!(
        field(&m, "plan", "allowed_write_paths"),
        json!([".hoocode/plan.md"])
    );
}

#[test]
fn unions_global_and_project_allowed_write_paths() {
    let m = merge_configs(
        &cfg(json!({"modes": {"plan": {"allowed_write_paths": [".hoocode/plan.md"]}}})),
        &cfg(json!({"modes": {"plan": {"allowed_write_paths": [".hoocode/notes.md"]}}})),
    );
    assert_eq!(
        field(&m, "plan", "allowed_write_paths"),
        json!([".hoocode/plan.md", ".hoocode/notes.md"])
    );
}

#[test]
fn removes_duplicates_in_allowed_write_paths_union() {
    let m = merge_configs(
        &cfg(json!({"modes": {"plan": {"allowed_write_paths": [".hoocode/plan.md"]}}})),
        &cfg(
            json!({"modes": {"plan": {"allowed_write_paths": [".hoocode/plan.md", ".hoocode/notes.md"]}}}),
        ),
    );
    assert_eq!(
        field(&m, "plan", "allowed_write_paths"),
        json!([".hoocode/plan.md", ".hoocode/notes.md"])
    );
}

#[test]
fn unions_auto_allow_arrays() {
    let m = merge_configs(
        &cfg(json!({"modes": {"build": {"auto_allow": ["bash"]}}})),
        &cfg(json!({"modes": {"build": {"auto_allow": ["write"]}}})),
    );
    assert_eq!(field(&m, "build", "auto_allow"), json!(["bash", "write"]));
}

// denied_tools

#[test]
fn denied_tools_merge_rules() {
    let g = cfg(json!({"modes": {"build": {"denied_tools": ["write"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &cfg(json!({}))), "build", "denied_tools"),
        json!(["write"])
    );
    let p = cfg(json!({"modes": {"build": {"denied_tools": ["edit"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "denied_tools"),
        json!(["write", "edit"])
    );
    let p = cfg(json!({"modes": {"build": {"denied_tools": ["write", "edit"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "denied_tools"),
        json!(["write", "edit"])
    );
    let m = merge_configs(
        &cfg(json!({"modes": {"build": {}}})),
        &cfg(json!({"modes": {"build": {}}})),
    );
    assert_eq!(field(&m, "build", "denied_tools"), json!([]));
}

// allowed_bash_commands / denied_bash_commands

#[test]
fn bash_command_merge_rules() {
    let g = cfg(json!({"modes": {"build": {"allowed_bash_commands": ["^git\\s"]}}}));
    assert_eq!(
        field(
            &merge_configs(&g, &cfg(json!({}))),
            "build",
            "allowed_bash_commands"
        ),
        json!(["^git\\s"])
    );
    let p = cfg(json!({"modes": {"build": {"allowed_bash_commands": ["^npm\\s"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "allowed_bash_commands"),
        json!(["^npm\\s"])
    );
    let g = cfg(json!({"modes": {"build": {"allowed_bash_commands": ["^ls"]}}}));
    let p = cfg(json!({"modes": {"build": {}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "allowed_bash_commands"),
        json!(["^ls"])
    );

    let g = cfg(json!({"modes": {"build": {"denied_bash_commands": ["\\brm\\b"]}}}));
    assert_eq!(
        field(
            &merge_configs(&g, &cfg(json!({}))),
            "build",
            "denied_bash_commands"
        ),
        json!(["\\brm\\b"])
    );
    let p = cfg(json!({"modes": {"build": {"denied_bash_commands": ["\\bsudo\\b"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "denied_bash_commands"),
        json!(["\\brm\\b", "\\bsudo\\b"])
    );
    let p = cfg(json!({"modes": {"build": {"denied_bash_commands": ["\\brm\\b", "\\bsudo\\b"]}}}));
    assert_eq!(
        field(&merge_configs(&g, &p), "build", "denied_bash_commands"),
        json!(["\\brm\\b", "\\bsudo\\b"])
    );
}

// mode_paths

#[test]
fn mode_paths_merge_rules() {
    let m = merge_configs(
        &cfg(json!({"mode_paths": ["/global/a", "/shared"]})),
        &cfg(json!({"mode_paths": ["/project/a", "/shared"]})),
    );
    assert_eq!(
        m["mode_paths"],
        json!(["/project/a", "/shared", "/global/a"])
    );
    assert!(merge_configs(&cfg(json!({})), &cfg(json!({})))
        .get("mode_paths")
        .is_none());
    assert_eq!(
        merge_configs(&cfg(json!({"mode_paths": ["/g"]})), &cfg(json!({})))["mode_paths"],
        json!(["/g"])
    );
}

// parsePlanSections

#[test]
fn parses_all_standard_sections() {
    let s = parse_plan_sections(
        "\n## Goal\nImplement feature X\n\n## Files to modify\n- src/foo.ts\n\n## New files\n- src/bar.ts\n\n## Tests\n- test/foo.test.ts\n\n## Verification\nRun tests\n",
    );
    assert_eq!(s.goal.as_deref(), Some("Implement feature X"));
    assert_eq!(s.files_to_modify.as_deref(), Some("- src/foo.ts"));
    assert_eq!(s.new_files.as_deref(), Some("- src/bar.ts"));
    assert_eq!(s.tests.as_deref(), Some("- test/foo.test.ts"));
    assert_eq!(s.verification.as_deref(), Some("Run tests"));
}

#[test]
fn returns_raw_content_when_no_sections_found() {
    let s = parse_plan_sections("Just some text");
    assert_eq!(s.raw, "Just some text");
    assert_eq!(s.goal, None);
}

#[test]
fn handles_bold_section_headers() {
    let s = parse_plan_sections(
        "\n**Goal**\nImplement feature X\n\n**Files to modify**\n- src/foo.ts\n",
    );
    assert_eq!(s.goal.as_deref(), Some("Implement feature X"));
    assert_eq!(s.files_to_modify.as_deref(), Some("- src/foo.ts"));
}

#[test]
fn matches_hoocodes_multiline_regex_edge_cases() {
    // Recorded from hoocode's parsePlanSections: a section keeps its first line,
    // a heading may sit on the line after `##`, blank lines after a heading are
    // skipped, and an inline `**bold**` ending a line cuts the content.
    let s = parse_plan_sections(
        "## Goal\nline one\nline two\n\n## Files to modify\n- a.ts\n- b.ts\n**Tests**\nx **y**\nz\n",
    );
    assert_eq!(s.goal.as_deref(), Some("line one"));
    assert_eq!(s.files_to_modify.as_deref(), Some("- a.ts"));
    assert_eq!(s.tests.as_deref(), Some("x"));
    assert_eq!(
        parse_plan_sections("##\nGoal\nbody\n").goal.as_deref(),
        Some("body")
    );
    let s = parse_plan_sections("# Goal   \n\n  body after blank\n## Verification\nnpm test");
    assert_eq!(s.goal.as_deref(), Some("body after blank"));
    assert_eq!(s.verification.as_deref(), Some("npm test"));
}

// buildApproveMessage

#[test]
fn builds_message_with_all_sections() {
    let message = build_approve_message(&PlanSections {
        goal: Some("Implement feature".into()),
        files_to_modify: Some("- src/foo.ts".into()),
        new_files: Some("- src/bar.ts".into()),
        tests: Some("- Add tests".into()),
        verification: Some("Run tests".into()),
        raw: String::new(),
    });
    for needle in [
        "**Goal:** Implement feature",
        "**Step 1 — Modify existing files:**",
        "**Step 2 — Create new files:**",
        "**Step 3 — Update tests:**",
        "**Step 4 — Verify:**",
    ] {
        assert!(message.contains(needle), "{needle}");
    }
}

#[test]
fn uses_raw_content_when_no_sections_parsed() {
    let message = build_approve_message(&PlanSections {
        raw: "Just do something".into(),
        ..Default::default()
    });
    assert_eq!(message, "Execute the following plan:\n\nJust do something");
}

// buildSystemPrompt search-path precedence

fn write_mode(root: &std::path::Path, name: &str, body: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("system.md"), body).unwrap();
}

#[test]
fn project_mode_wins_over_external_dirs() {
    let (cwd, a) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_mode(
        &cwd.path()
            .join(cortexcode_code_paths::CONFIG_DIR_NAME)
            .join("modes"),
        "ask",
        "PROJECT MODE",
    );
    write_mode(a.path(), "ask", "EXTERNAL A");
    let prompt = build_mode_system_prompt(
        "ask",
        cwd.path(),
        &[a.path().to_string_lossy().into_owned()],
    )
    .unwrap();
    assert!(prompt.contains("PROJECT MODE"));
    assert!(!prompt.contains("EXTERNAL A"));
}

#[test]
fn external_dirs_are_searched_in_declared_order() {
    let (cwd, a, b) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    write_mode(a.path(), "custom", "FIRST EXTERNAL");
    write_mode(b.path(), "custom", "SECOND EXTERNAL");
    let dirs = [&a, &b].map(|d| d.path().to_string_lossy().into_owned());
    assert_eq!(
        build_mode_system_prompt("custom", cwd.path(), &dirs).as_deref(),
        Some("FIRST EXTERNAL")
    );
}

#[test]
fn falls_through_to_second_external_when_first_does_not_have_the_mode() {
    let (cwd, a, b) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    write_mode(b.path(), "only-in-b", "FROM B");
    let dirs = [a.path(), b.path()].map(|d| d.to_string_lossy().into_owned());
    assert_eq!(
        build_mode_system_prompt("only-in-b", cwd.path(), &dirs).as_deref(),
        Some("FROM B")
    );
}

#[test]
fn falls_back_to_mode_defaults_when_nothing_matches() {
    let cwd = tempfile::tempdir().unwrap();
    let prompt = build_mode_system_prompt("ask", cwd.path(), &[]).unwrap();
    let default = DEFAULT_MODE_PROMPTS
        .iter()
        .find(|(n, _)| *n == "ask")
        .unwrap()
        .1;
    assert_eq!(prompt, default);
    assert!(prompt.contains("ask mode"));
}

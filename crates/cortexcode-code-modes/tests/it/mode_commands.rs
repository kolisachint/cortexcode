//! Port of hoocode `packages/coding-agent/test/mode-commands.test.ts` (v0.5.89):
//! the `/grill` and `/goal` handlers, observed through their [`ModeAction`]s.

use cortexcode_code_modes::config::HooConfig;
use cortexcode_code_modes::{ModeAction, ModeSession, ModesExtension};

const SESSION_ID: &str = "test-session";
const PLAN: &str = "## Goal\nShip the /goal command.\n\n## Files to modify\n- packages/coding-agent/src/extensions/core/modes.ts\n\n## Verification\nnpm run check && npm test\n";

fn extension(cwd: &std::path::Path) -> ModesExtension {
    ModesExtension::with_config(
        ModeSession {
            cwd: cwd.to_path_buf(),
            session_id: SESSION_ID.into(),
            light: false,
            mode_search_paths: vec![],
        },
        &HooConfig::new(),
    )
}

fn write_plan(cwd: &std::path::Path) {
    let path = cwd
        .join(cortexcode_code_paths::CONFIG_DIR_NAME)
        .join("plans")
        .join(format!("{SESSION_ID}.md"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, PLAN).unwrap();
}

fn sent(actions: &[ModeAction]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            ModeAction::SendFollowUp(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn notes(actions: &[ModeAction]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            ModeAction::Notify(m, _) => Some(m.as_str()),
            _ => None,
        })
        .collect()
}

fn auto_starts(actions: &[ModeAction]) -> Vec<(&str, Option<u64>, &str)> {
    actions
        .iter()
        .filter_map(|a| match a {
            ModeAction::StartAutoLoop {
                task,
                max_turns,
                continue_prompt,
            } => Some((task.as_str(), *max_turns, continue_prompt.as_str())),
            _ => None,
        })
        .collect()
}

// /grill

#[test]
fn runs_both_phases_against_the_plan_by_default() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("grill", "");
    let sent = sent(&actions);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("ask_options"));
    assert!(sent[0].contains("attacking the plan"));
    assert!(sent[0].contains("Ship the /goal command."));
}

#[test]
fn restricts_itself_to_the_requested_phase() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("grill", "plan");
    assert!(!sent(&actions)[0].contains("ask_options"));
}

#[test]
fn declines_with_a_pointer_to_plan_when_there_is_no_plan() {
    let dir = tempfile::tempdir().unwrap();
    let actions = extension(dir.path()).command("grill", "");
    assert!(sent(&actions).is_empty());
    assert!(notes(&actions)[0].contains("No plan to grill"));
    assert!(notes(&actions)[0].contains("Run /plan first"));
}

#[test]
fn shows_usage_for_an_unknown_subcommand_instead_of_guessing() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("grill", "everything");
    assert!(sent(&actions).is_empty());
    assert!(notes(&actions)[0].contains("Usage: /grill"));
}

#[test]
fn drops_the_interrogation_phase_while_an_autonomous_loop_is_running() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let ext = extension(dir.path());
    ext.set_auto_loop_active(true);
    let actions = ext.command("grill", "me");
    assert!(!sent(&actions)[0].contains("ask_options"));
    assert!(sent(&actions)[0].contains("attacking the plan"));
    assert!(notes(&actions)
        .iter()
        .any(|n| n.contains("skipping questions")));
}

#[test]
fn asks_again_once_the_loop_stops() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let ext = extension(dir.path());
    ext.set_auto_loop_active(true);
    ext.set_auto_loop_active(false);
    assert!(sent(&ext.command("grill", "me"))[0].contains("ask_options"));
}

// /goal

#[test]
fn takes_the_objective_and_completion_condition_from_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("goal", "");
    let starts = auto_starts(&actions);
    assert_eq!(starts.len(), 1);
    assert!(starts[0].0.contains("Ship the /goal command."));
    assert!(starts[0].0.contains("npm run check && npm test"));
    assert!(starts[0].2.contains("Ship the /goal command."));
}

#[test]
fn prefers_an_explicit_objective_but_keeps_the_plans_verification() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("goal", "make the parser tests pass");
    let task = auto_starts(&actions)[0].0;
    assert!(task.contains("make the parser tests pass"));
    assert!(!task.contains("Ship the /goal command."));
    assert!(task.contains("npm run check && npm test"));
}

#[test]
fn runs_free_standing_with_no_plan_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let actions = extension(dir.path()).command("goal", "make the parser tests pass");
    let starts = auto_starts(&actions);
    assert_eq!(starts.len(), 1);
    assert!(starts[0].0.contains("make the parser tests pass"));
}

#[test]
fn passes_the_turn_budget_through_to_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    let actions = extension(dir.path()).command("goal", "--max-turns 25 ship it");
    assert_eq!(auto_starts(&actions)[0].1, Some(25));
}

#[test]
fn declines_when_there_is_neither_an_objective_nor_a_plan_goal() {
    let dir = tempfile::tempdir().unwrap();
    let actions = extension(dir.path()).command("goal", "");
    assert!(auto_starts(&actions).is_empty());
    assert!(notes(&actions)[0].contains("No objective"));
}

#[test]
fn shows_usage_for_a_malformed_budget() {
    let dir = tempfile::tempdir().unwrap();
    write_plan(dir.path());
    let actions = extension(dir.path()).command("goal", "--max-turns lots ship it");
    assert!(auto_starts(&actions).is_empty());
    assert!(notes(&actions)[0].contains("Usage: /goal"));
}

// Beyond the TS file: the before_agent_start appendix and plan-mode prompt.

#[test]
fn appends_the_mode_block_and_substitutes_the_plan_path() {
    use cortexcode_code_agent_session::ExtensionHooks;
    let dir = tempfile::tempdir().unwrap();
    let build = extension(dir.path());
    let prompt = build.before_agent_start("hi", "BASE").unwrap();
    assert!(prompt.starts_with("BASE\n\n<!-- hoo-core: mode=build -->\nYou are in **build mode**"));

    let config: HooConfig = serde_json::from_str(
        r#"{"active_mode": "plan", "modes": {"plan": {"enabled_tools": ["read", "write"]}}}"#,
    )
    .unwrap();
    let plan = ModesExtension::with_config(
        ModeSession {
            cwd: dir.path().to_path_buf(),
            session_id: SESSION_ID.into(),
            light: false,
            mode_search_paths: vec![],
        },
        &config,
    );
    let prompt = plan.before_agent_start("hi", "BASE").unwrap();
    assert!(prompt.contains("<!-- hoo-core: mode=plan -->"));
    assert!(prompt.contains(&format!(
        "{}/plans/{SESSION_ID}.md",
        cortexcode_code_paths::CONFIG_DIR_NAME
    )));
    assert!(!prompt.contains("{{PLAN_PATH}}"));
    assert_eq!(
        plan.active().enabled_tools,
        Some(vec!["read".to_string(), "write".to_string()])
    );
    // /approve outside plan mode declines.
    assert!(matches!(
        &build.command("approve", "")[0],
        ModeAction::Notify(m, _) if m.contains("only available in plan mode")
    ));
}

#[test]
fn light_mode_adds_no_appendix() {
    use cortexcode_code_agent_session::ExtensionHooks;
    let dir = tempfile::tempdir().unwrap();
    let ext = ModesExtension::with_config(
        ModeSession {
            cwd: dir.path().to_path_buf(),
            session_id: SESSION_ID.into(),
            light: true,
            mode_search_paths: vec![],
        },
        &HooConfig::new(),
    );
    assert_eq!(ext.before_agent_start("hi", "BASE"), None);
}

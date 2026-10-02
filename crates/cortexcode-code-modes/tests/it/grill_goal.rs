//! Ports of hoocode `test/grill-command.test.ts` and `test/goal-command.test.ts` (v0.5.89).

use cortexcode_code_modes::{
    build_goal_messages, build_grill_message, parse_goal_args, parse_grill_target,
    parse_plan_sections, GoalInvocation, GrillTarget, AUTO_LOOP_DONE_TOKEN,
};

const SAMPLE_PLAN: &str = "## Goal\nAdd a /grill command that stress-tests the current plan.\n\n## Files to modify\n- packages/coding-agent/src/extensions/core/modes.ts\n\n## New files\n- packages/coding-agent/test/grill-command.test.ts\n\n## Tests\nCover target parsing and message construction.\n\n## Verification\nnpm run check && npm test\n";

// parseGrillTarget

#[test]
fn defaults_to_both_phases_for_a_bare_grill() {
    assert_eq!(parse_grill_target(""), Some(GrillTarget::Both));
    assert_eq!(parse_grill_target("   "), Some(GrillTarget::Both));
}

#[test]
fn parses_the_explicit_subcommands() {
    assert_eq!(parse_grill_target("me"), Some(GrillTarget::Me));
    assert_eq!(parse_grill_target("plan"), Some(GrillTarget::Plan));
}

#[test]
fn tolerates_surrounding_whitespace_and_casing() {
    assert_eq!(parse_grill_target("  ME  "), Some(GrillTarget::Me));
    assert_eq!(parse_grill_target("Plan"), Some(GrillTarget::Plan));
}

#[test]
fn returns_none_for_unknown_arguments() {
    for arg in ["everything", "me plan", "--hard"] {
        assert_eq!(parse_grill_target(arg), None);
    }
}

// buildGrillMessage

#[test]
fn asks_the_user_via_ask_options_in_the_me_phase_without_critiquing() {
    let m = build_grill_message(&parse_plan_sections(SAMPLE_PLAN), GrillTarget::Me);
    assert!(m.contains("ask_options"));
    assert!(m.contains("interrogating the *request*"));
    assert!(!m.contains("attacking the plan"));
}

#[test]
fn critiques_the_plan_in_the_plan_phase_without_asking_questions() {
    let m = build_grill_message(&parse_plan_sections(SAMPLE_PLAN), GrillTarget::Plan);
    assert!(m.contains("attacking the plan"));
    assert!(!m.contains("ask_options"));
}

#[test]
fn runs_questions_before_critique_when_both_phases_are_requested() {
    let m = build_grill_message(&parse_plan_sections(SAMPLE_PLAN), GrillTarget::Both);
    let ask = m.find("ask_options").unwrap();
    let critique = m.find("attacking the plan").unwrap();
    assert!(ask < critique);
}

#[test]
fn forbids_implementation_in_every_phase() {
    for target in [GrillTarget::Both, GrillTarget::Me, GrillTarget::Plan] {
        let m = build_grill_message(&parse_plan_sections(SAMPLE_PLAN), target).to_lowercase();
        assert!(m.contains("do not edit files") || m.contains("do not implement"));
    }
}

#[test]
fn embeds_the_parsed_plan_sections() {
    let m = build_grill_message(&parse_plan_sections(SAMPLE_PLAN), GrillTarget::Plan);
    assert!(m.contains("Add a /grill command that stress-tests the current plan."));
    assert!(m.contains("packages/coding-agent/src/extensions/core/modes.ts"));
    assert!(m.contains("npm run check && npm test"));
}

#[test]
fn falls_back_to_the_raw_plan_when_no_sections_are_recognised() {
    let s = parse_plan_sections("just do the thing, no headings anywhere");
    assert!(build_grill_message(&s, GrillTarget::Plan)
        .contains("just do the thing, no headings anywhere"));
}

// parseGoalArgs

fn goal(objective: Option<&str>, max_turns: Option<u64>) -> Option<GoalInvocation> {
    Some(GoalInvocation {
        objective: objective.map(str::to_string),
        max_turns,
    })
}

#[test]
fn treats_a_bare_goal_as_read_the_objective_from_the_plan() {
    assert_eq!(parse_goal_args(""), goal(None, None));
    assert_eq!(parse_goal_args("   "), goal(None, None));
}

#[test]
fn takes_the_whole_argument_as_the_objective() {
    assert_eq!(
        parse_goal_args("make the parser tests pass"),
        goal(Some("make the parser tests pass"), None)
    );
}

#[test]
fn parses_max_turns_with_and_without_a_trailing_objective() {
    assert_eq!(
        parse_goal_args("--max-turns 25 ship the migration"),
        goal(Some("ship the migration"), Some(25))
    );
    assert_eq!(parse_goal_args("--max-turns 3"), goal(None, Some(3)));
}

#[test]
fn honours_an_explicit_budget_of_zero() {
    assert_eq!(
        parse_goal_args("--max-turns 0 do the thing"),
        goal(Some("do the thing"), Some(0))
    );
}

#[test]
fn rejects_a_malformed_budget() {
    for args in [
        "--max-turns",
        "--max-turns lots",
        "--max-turns -4",
        "--max-turns 2.5",
    ] {
        assert_eq!(parse_goal_args(args), None, "{args}");
    }
}

#[test]
fn only_treats_max_turns_as_a_flag_at_the_start() {
    assert_eq!(
        parse_goal_args("explain --max-turns to me"),
        goal(Some("explain --max-turns to me"), None)
    );
}

// buildGoalMessages

#[test]
fn states_the_objective_in_both_messages() {
    let m = build_goal_messages("make the parser tests pass", None);
    assert!(m.task.contains("make the parser tests pass"));
    assert!(m.continue_prompt.contains("make the parser tests pass"));
}

#[test]
fn asks_for_the_done_token_only_in_the_continuation() {
    let m = build_goal_messages("make the parser tests pass", None);
    assert!(m.continue_prompt.contains(AUTO_LOOP_DONE_TOKEN));
}

#[test]
fn promotes_the_plans_verification_section_into_the_completion_condition() {
    let s =
        parse_plan_sections("## Goal\nShip /goal.\n\n## Verification\nnpm run check && npm test\n");
    let m = build_goal_messages(s.goal.as_deref().unwrap_or(""), s.verification.as_deref());
    assert!(m.task.contains("npm run check && npm test"));
    assert!(m.continue_prompt.contains("npm run check && npm test"));
    assert!(m
        .task
        .to_lowercase()
        .contains("complete only once this verification passes"));
}

#[test]
fn omits_the_verification_clause_entirely_when_the_plan_has_none() {
    let m = build_goal_messages("make the parser tests pass", None);
    assert!(!m.task.to_lowercase().contains("verification"));
    assert!(!m.continue_prompt.to_lowercase().contains("verification"));
}

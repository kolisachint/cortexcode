//! dispatch-evaluator.test.ts: the depth guard and the complexity estimate.
//! (TS mutates process.env; here the env is explicit.)

use std::collections::HashMap;

use cortexcode_code_subagents::dispatch::{Complexity, DispatchEvaluator};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn delegates_by_default() {
    let analysis =
        DispatchEvaluator.evaluate_with_env("Add console.log to src/index.ts line 10", &env(&[]));
    assert!(analysis.should_delegate);
}

#[test]
fn estimates_low_complexity_for_a_single_file_change() {
    let analysis =
        DispatchEvaluator.evaluate_with_env("Add console.log to src/index.ts line 10", &env(&[]));
    assert_eq!(analysis.estimated_complexity, Complexity::Low);
}

#[test]
fn estimates_high_complexity_for_a_multi_file_refactor() {
    let analysis = DispatchEvaluator.evaluate_with_env(
        "Refactor database layer across 10 files to use Prisma",
        &env(&[]),
    );
    assert_eq!(analysis.estimated_complexity, Complexity::High);
}

#[test]
fn estimates_medium_complexity_from_file_and_line_mentions() {
    let e = env(&[]);
    let medium = |task: &str| {
        DispatchEvaluator
            .evaluate_with_env(task, &e)
            .estimated_complexity
    };
    assert_eq!(medium("Update a.ts and b.rs"), Complexity::Medium);
    assert_eq!(medium("Add 80 lines of tests"), Complexity::Medium);
    assert_eq!(medium("Touch 3 files"), Complexity::Medium);
    assert_eq!(medium("Rewrite 300 LOC"), Complexity::High);
    assert_eq!(medium("Edit a.ts b.ts c.ts d.ts"), Complexity::High);
}

#[test]
fn prevents_subagents_from_spawning_subagents_at_the_default_cap() {
    let analysis = DispatchEvaluator.evaluate_with_env(
        "Implement login endpoint, write tests, do security review",
        &env(&[("HOOCODE_SUBAGENT_DEPTH", "1")]),
    );
    assert!(!analysis.should_delegate);
    assert_eq!(analysis.reason, "Subagents cannot spawn subagents");
}

#[test]
fn allows_one_level_of_nesting_when_the_cap_is_raised_to_2() {
    let child = env(&[
        ("HOOCODE_SUBAGENT_MAX_DEPTH", "2"),
        ("HOOCODE_SUBAGENT_DEPTH", "1"),
    ]);
    assert!(
        DispatchEvaluator
            .evaluate_with_env("Refactor module", &child)
            .should_delegate
    );
    let grandchild = env(&[
        ("HOOCODE_SUBAGENT_MAX_DEPTH", "2"),
        ("HOOCODE_SUBAGENT_DEPTH", "2"),
    ]);
    let blocked = DispatchEvaluator.evaluate_with_env("Refactor module", &grandchild);
    assert!(!blocked.should_delegate);
    assert_eq!(blocked.reason, "Maximum subagent depth (2) reached");
}

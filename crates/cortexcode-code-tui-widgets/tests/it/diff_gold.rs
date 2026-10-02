//! jsdiff `diffWords` and the pin's `renderDiff` against the real ones
//! (`fixtures/diff-gold.json`, from `migration/tools/goldens/diff.mjs`).

use crate::support::lock;
use cortexcode_code_tui_widgets::diff::render_diff;
use cortexcode_code_tui_widgets::jsdiff::diff_words;
use serde_json::Value;

fn gold() -> Value {
    serde_json::from_str(include_str!("../fixtures/diff-gold.json")).unwrap()
}

#[test]
fn diff_words_matches_jsdiff() {
    let gold = gold();
    let mut failures = Vec::new();
    for case in gold["words"].as_array().unwrap() {
        let (a, b) = (case["a"].as_str().unwrap(), case["b"].as_str().unwrap());
        let got: Vec<(String, bool, bool)> = diff_words(a, b)
            .into_iter()
            .map(|c| (c.value, c.added, c.removed))
            .collect();
        let want: Vec<(String, bool, bool)> = case["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["value"].as_str().unwrap().to_string(),
                    c["added"].as_bool().unwrap(),
                    c["removed"].as_bool().unwrap(),
                )
            })
            .collect();
        if got != want {
            failures.push(format!("{a:?} -> {b:?}\n  want {want:?}\n  got  {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn render_diff_matches_the_pin() {
    let _g = lock();
    let gold = gold();
    let mut failures = Vec::new();
    for case in gold["rendered"].as_array().unwrap() {
        let diff = case["diff"].as_str().unwrap();
        let got = render_diff(diff);
        if got != case["out"].as_str().unwrap() {
            failures.push(format!(
                "{diff:?}\n  want {:?}\n  got  {got:?}",
                case["out"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

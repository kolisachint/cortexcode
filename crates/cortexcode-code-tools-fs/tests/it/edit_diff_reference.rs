//! The edit matcher and diff against hoocode's own `edit-diff.js`: every case
//! in `fixtures/edit_diff_cases.json` was produced by the pinned
//! implementation (`fixtures/gen_edit_diff_cases.mjs`), including the error
//! messages and jsdiff's line diff.

use cortexcode_code_tools_fs::{apply_edits_to_normalized_content, generate_diff_string, Edit};
use serde_json::Value;

fn cases() -> Value {
    let text = include_str!("../fixtures/edit_diff_cases.json");
    serde_json::from_str(text).unwrap()
}

#[test]
fn apply_edits_matches_hoocode() {
    let cases = cases();
    let mut failures = Vec::new();
    for (i, case) in cases["apply"].as_array().unwrap().iter().enumerate() {
        let content = case["content"].as_str().unwrap();
        let edits: Vec<Edit> = case["edits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| Edit {
                old_text: e["oldText"].as_str().unwrap().to_owned(),
                new_text: e["newText"].as_str().unwrap().to_owned(),
                replace_all: e["replaceAll"].as_bool() == Some(true),
            })
            .collect();
        let got = apply_edits_to_normalized_content(content, &edits, "f.txt");
        let ok = match (&got, case.get("ok"), case.get("err")) {
            (Ok(applied), Some(expected), _) => {
                let diff = generate_diff_string(&applied.base_content, &applied.new_content, 4);
                applied.new_content == expected.as_str().unwrap()
                    && diff.diff == case["diff"]["diff"].as_str().unwrap()
                    && diff.first_changed_line.map(|n| n as u64)
                        == case["diff"]["firstChangedLine"].as_u64()
            }
            (Err(message), _, Some(expected)) => message == expected.as_str().unwrap(),
            _ => false,
        };
        if !ok {
            failures.push(format!("case {i}: {case}\n  got: {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases["apply"].as_array().unwrap().len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn diff_string_matches_hoocode() {
    let cases = cases();
    let mut failures = Vec::new();
    for (i, case) in cases["diff"].as_array().unwrap().iter().enumerate() {
        let got = generate_diff_string(
            case["old"].as_str().unwrap(),
            case["new"].as_str().unwrap(),
            case["context"].as_u64().unwrap() as usize,
        );
        if got.diff != case["diff"].as_str().unwrap()
            || got.first_changed_line.map(|n| n as u64) != case["firstChangedLine"].as_u64()
        {
            failures.push(format!(
                "case {i}:\n--- expected\n{}\n--- got\n{}",
                case["diff"].as_str().unwrap(),
                got.diff
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        cases["diff"].as_array().unwrap().len(),
        failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

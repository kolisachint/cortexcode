//! output-verifier.test.ts.

use std::path::Path;

use cortexcode_code_subagents::output_verifier::OutputVerifier;
use serde_json::{json, Value};

fn write_result(cwd: &Path, task_id: &str, content: &str) {
    let dir = cortexcode_code_paths::dispatch_task_dir(cwd, task_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("result.json"), content).unwrap();
}

fn verify(task_id: &str, content: Value) -> (bool, Option<String>) {
    let cwd = tempfile::tempdir().unwrap();
    write_result(
        cwd.path(),
        task_id,
        &serde_json::to_string_pretty(&content).unwrap(),
    );
    let result = OutputVerifier::new(cwd.path()).verify(task_id, None);
    (result.valid, result.reason)
}

fn reason(task_id: &str, content: Value) -> String {
    let (valid, reason) = verify(task_id, content);
    assert!(!valid);
    reason.unwrap()
}

#[test]
fn valid_for_a_correct_result_json() {
    let (valid, reason) = verify(
        "task-1",
        json!({"summary": "All files updated successfully", "files_changed": ["src/foo.ts", "src/bar.ts"], "confidence": 0.95, "status": "complete"}),
    );
    assert!(valid);
    assert_eq!(reason, None);
}

#[test]
fn fails_when_result_json_does_not_exist() {
    let cwd = tempfile::tempdir().unwrap();
    let result = OutputVerifier::new(cwd.path()).verify("missing-task", None);
    assert!(!result.valid);
    assert_eq!(
        result.reason.as_deref(),
        Some("result.json not found for task missing-task")
    );
}

#[test]
fn fails_on_invalid_json() {
    let cwd = tempfile::tempdir().unwrap();
    write_result(cwd.path(), "bad-json", "not json");
    let result = OutputVerifier::new(cwd.path()).verify("bad-json", None);
    assert_eq!(
        result.reason.as_deref(),
        Some("Invalid JSON in result.json for task bad-json")
    );
}

#[test]
fn fails_when_not_an_object() {
    assert_eq!(
        reason("not-obj", json!("string")),
        "result.json is not an object for task not-obj"
    );
}

#[test]
fn fails_when_summary_is_missing_or_empty() {
    assert_eq!(
        reason(
            "no-summary",
            json!({"files_changed": [], "confidence": 0.9, "status": "complete"})
        ),
        "Missing or invalid 'summary' in result.json for task no-summary"
    );
    assert_eq!(
        reason(
            "empty-summary",
            json!({"summary": "   ", "files_changed": [], "confidence": 0.9, "status": "complete"})
        ),
        "Empty 'summary' in result.json for task empty-summary"
    );
}

#[test]
fn fails_on_bad_files_changed() {
    assert_eq!(
        reason(
            "no-files",
            json!({"summary": "Done", "confidence": 0.9, "status": "complete"})
        ),
        "Missing or invalid 'files_changed' in result.json for task no-files"
    );
    assert_eq!(
        reason(
            "bad-files",
            json!({"summary": "Done", "files_changed": ["ok", 123, true], "confidence": 0.9, "status": "complete"})
        ),
        "Non-string entries in 'files_changed' for task bad-files"
    );
}

#[test]
fn fails_on_missing_or_low_confidence() {
    assert_eq!(
        reason(
            "no-confidence",
            json!({"summary": "Done", "files_changed": [], "status": "complete"})
        ),
        "Missing or invalid 'confidence' in result.json for task no-confidence"
    );
    assert_eq!(
        reason(
            "low-confidence",
            json!({"summary": "Done", "files_changed": [], "confidence": 0.3, "status": "complete"})
        ),
        "Confidence 0.3 below threshold (0.5) for task low-confidence"
    );
}

#[test]
fn passes_when_confidence_is_exactly_0_5() {
    assert!(
        verify(
            "exact-confidence",
            json!({"summary": "Done", "files_changed": [], "confidence": 0.5, "status": "complete"})
        )
        .0
    );
}

#[test]
fn fails_on_missing_or_invalid_status() {
    assert_eq!(
        reason(
            "no-status",
            json!({"summary": "Done", "files_changed": [], "confidence": 0.9})
        ),
        "Missing or invalid 'status' in result.json for task no-status"
    );
    assert_eq!(
        reason(
            "bad-status",
            json!({"summary": "Done", "files_changed": [], "confidence": 0.9, "status": "done"})
        ),
        "Invalid status 'done' in result.json for task bad-status"
    );
}

#[test]
fn passes_for_partial_and_failed() {
    assert!(verify("partial", json!({"summary": "Some changes applied", "files_changed": ["a.ts"], "confidence": 0.75, "status": "partial"})).0);
    assert!(verify("failed", json!({"summary": "Could not apply changes", "files_changed": [], "confidence": 0.6, "status": "failed"})).0);
}

#[test]
fn accepts_a_cwd_override() {
    let cwd = tempfile::tempdir().unwrap();
    write_result(
        cwd.path(),
        "override",
        r#"{"summary":"Done","files_changed":[],"confidence":0.9,"status":"complete"}"#,
    );
    let verifier = OutputVerifier::new("/nonexistent");
    assert!(verifier.verify("override", Some(cwd.path())).valid);
}

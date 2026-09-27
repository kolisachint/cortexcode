//! `core/output-verifier.ts`: checks a subagent's `result.json` exists,
//! matches the schema, and meets the quality bar (non-empty summary,
//! confidence >= 0.5).

use std::path::{Path, PathBuf};

use serde_json::Value;

/// `VerificationResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationResult {
    pub valid: bool,
    pub reason: Option<String>,
}

impl VerificationResult {
    fn valid() -> Self {
        Self {
            valid: true,
            reason: None,
        }
    }

    fn invalid(reason: String) -> Self {
        Self {
            valid: false,
            reason: Some(reason),
        }
    }
}

const VALID_STATUSES: &[&str] = &["complete", "partial", "failed"];

/// `String(n)` for the confidence in a reason.
fn js_number_string(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

/// `OutputVerifier`.
#[derive(Debug, Clone)]
pub struct OutputVerifier {
    default_cwd: PathBuf,
}

impl Default for OutputVerifier {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl OutputVerifier {
    pub fn new(default_cwd: impl Into<PathBuf>) -> Self {
        Self {
            default_cwd: default_cwd.into(),
        }
    }

    /// Verify the output for a task (`cwd` overrides the constructor's).
    pub fn verify(&self, task_id: &str, cwd: Option<&Path>) -> VerificationResult {
        let base = cwd.unwrap_or(&self.default_cwd);
        let path = cortexcode_code_paths::dispatch_task_dir(base, task_id).join("result.json");
        let invalid = |reason: String| VerificationResult::invalid(reason);

        if !path.exists() {
            return invalid(format!("result.json not found for task {task_id}"));
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return invalid(format!("Cannot read result.json for task {task_id}"));
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            return invalid(format!("Invalid JSON in result.json for task {task_id}"));
        };
        // `typeof parsed !== "object"` lets arrays through (they fail on
        // `summary` below), as in hoocode.
        let result = match &parsed {
            Value::Object(map) => Some(map),
            Value::Array(_) => None,
            _ => return invalid(format!("result.json is not an object for task {task_id}")),
        };
        let get = |key: &str| result.and_then(|m| m.get(key));

        let Some(summary) = get("summary").and_then(Value::as_str) else {
            return invalid(format!(
                "Missing or invalid 'summary' in result.json for task {task_id}"
            ));
        };
        if summary.trim().is_empty() {
            return invalid(format!("Empty 'summary' in result.json for task {task_id}"));
        }

        let Some(files) = get("files_changed").and_then(Value::as_array) else {
            return invalid(format!(
                "Missing or invalid 'files_changed' in result.json for task {task_id}"
            ));
        };
        if !files.iter().all(Value::is_string) {
            return invalid(format!(
                "Non-string entries in 'files_changed' for task {task_id}"
            ));
        }

        let Some(confidence) = get("confidence").and_then(Value::as_f64) else {
            return invalid(format!(
                "Missing or invalid 'confidence' in result.json for task {task_id}"
            ));
        };
        if confidence < 0.5 {
            return invalid(format!(
                "Confidence {} below threshold (0.5) for task {task_id}",
                js_number_string(confidence)
            ));
        }

        let Some(status) = get("status").and_then(Value::as_str) else {
            return invalid(format!(
                "Missing or invalid 'status' in result.json for task {task_id}"
            ));
        };
        if !VALID_STATUSES.contains(&status) {
            return invalid(format!(
                "Invalid status '{status}' in result.json for task {task_id}"
            ));
        }

        VerificationResult::valid()
    }
}

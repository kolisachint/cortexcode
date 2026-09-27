//! `JSON.stringify(value, null, 2)` for tool arguments.

use serde_json::Value;

/// `JSON.stringify(value, null, 2)`; `""` for `undefined`/`null` arguments
/// that the pin would not print.
pub fn stringify_pretty(value: &Value) -> String {
    if value.is_null() {
        return String::new();
    }
    serde_json::to_string_pretty(value).unwrap_or_default()
}

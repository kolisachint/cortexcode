//! Port of hoocode `utils/diagnostics.ts` (v0.5.89): redacted diagnostics
//! appended to `AssistantMessage.diagnostics`. Rust errors carry no stack, so
//! `error.stack` is never set.

use cortexcode_ai_types::AssistantMessage;
use serde_json::{json, Map, Value};

/// `DiagnosticErrorInfo` for an error: its `name` (`Error`, or the error
/// class such as `WebSocketCloseError`), `message` and optional `code`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticError {
    pub name: String,
    pub message: String,
    pub code: Option<Value>,
}

impl DiagnosticError {
    /// A plain `Error` with `message`.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            name: "Error".to_string(),
            message: message.into(),
            code: None,
        }
    }

    /// `extractDiagnosticError`: `message || name`.
    fn to_json(&self) -> Value {
        let mut info = Map::new();
        if !self.name.is_empty() {
            info.insert("name".into(), json!(self.name));
        }
        let message = if self.message.is_empty() {
            &self.name
        } else {
            &self.message
        };
        info.insert("message".into(), json!(message));
        if let Some(code) = self
            .code
            .as_ref()
            .filter(|c| c.is_string() || c.is_number())
        {
            info.insert("code".into(), code.clone());
        }
        Value::Object(info)
    }
}

/// `createAssistantMessageDiagnostic(type, error, details)`.
pub fn create_assistant_message_diagnostic(
    kind: &str,
    error: &DiagnosticError,
    details: Option<Map<String, Value>>,
) -> Value {
    let mut diagnostic = Map::new();
    diagnostic.insert("type".into(), json!(kind));
    diagnostic.insert("timestamp".into(), json!(cortexcode_ai_types::now_ms()));
    diagnostic.insert("error".into(), error.to_json());
    if let Some(details) = details {
        diagnostic.insert("details".into(), Value::Object(details));
    }
    Value::Object(diagnostic)
}

/// `appendAssistantMessageDiagnostic`.
pub fn append_assistant_message_diagnostic(message: &mut AssistantMessage, diagnostic: Value) {
    message
        .diagnostics
        .get_or_insert_with(Vec::new)
        .push(diagnostic);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_shape_matches_hoocode() {
        let error = DiagnosticError {
            name: "WebSocketCloseError".into(),
            message: "WebSocket closed 1009 message too big".into(),
            code: Some(json!(1009)),
        };
        let mut details = Map::new();
        details.insert("phase".into(), json!("before_message_stream_start"));
        let d = create_assistant_message_diagnostic(
            "provider_transport_failure",
            &error,
            Some(details),
        );
        assert_eq!(d["type"], "provider_transport_failure");
        assert!(d["timestamp"].as_i64().unwrap() > 0);
        assert_eq!(
            d["error"],
            json!({"name": "WebSocketCloseError", "message": "WebSocket closed 1009 message too big", "code": 1009})
        );
        assert_eq!(d["details"]["phase"], "before_message_stream_start");

        let plain = create_assistant_message_diagnostic("x", &DiagnosticError::error(""), None);
        assert_eq!(plain["error"], json!({"name": "Error", "message": "Error"}));
        assert!(plain.get("details").is_none());
    }
}

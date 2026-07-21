//! Error types for MCP transport and tool discovery.

use std::fmt;

/// Errors that can occur when connecting to or calling an MCP server.
#[derive(Debug)]
pub enum McpError {
    /// An I/O operation failed.
    Io(std::io::Error),
    /// JSON serialization or deserialization failed.
    Json(serde_json::Error),
    /// An HTTP request failed before a response was received.
    Request(String),
    /// The server returned an error message.
    Server { message: String },
    /// A JSON-RPC request timed out.
    Timeout { method: String, timeout_ms: u64 },
    /// The connection was terminated.
    Terminated { name: String },
    /// A required configuration field was missing.
    MissingField { field: String },
    /// A configuration value was invalid.
    Config(String),
    /// An HTTP response returned an unexpected status code.
    HttpStatus { status: u16, body: String },
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpError::Io(e) => write!(f, "io error: {}", e),
            McpError::Json(e) => write!(f, "json error: {}", e),
            McpError::Request(msg) => write!(f, "request error: {}", msg),
            McpError::Server { message } => write!(f, "mcp server error: {}", message),
            McpError::Timeout { method, timeout_ms } => {
                write!(
                    f,
                    "mcp server timed out after {}ms on {}",
                    timeout_ms, method
                )
            }
            McpError::Terminated { name } => {
                write!(f, "mcp server \"{}\" connection terminated", name)
            }
            McpError::MissingField { field } => write!(f, "missing field: {}", field),
            McpError::Config(msg) => write!(f, "config error: {}", msg),
            McpError::HttpStatus { status, body } => {
                write!(f, "http status {}: {}", status, body)
            }
        }
    }
}

impl std::error::Error for McpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            McpError::Io(e) => Some(e),
            McpError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for McpError {
    fn from(e: std::io::Error) -> Self {
        McpError::Io(e)
    }
}

impl From<serde_json::Error> for McpError {
    fn from(e: serde_json::Error) -> Self {
        McpError::Json(e)
    }
}

impl From<reqwest::Error> for McpError {
    fn from(e: reqwest::Error) -> Self {
        McpError::Request(e.to_string())
    }
}

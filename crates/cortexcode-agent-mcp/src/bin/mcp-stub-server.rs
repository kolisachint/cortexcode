//! Minimal MCP server used by the `cortexcode-agent-mcp` integration tests.
//!
//! Reads JSON-RPC requests from stdin (one per line) and writes JSON-RPC
//! responses to stdout. Implements the `initialize`, `notifications/initialized`,
//! `tools/list`, and `tools/call` methods used by the test suite.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};

#[derive(Debug, Deserialize)]
struct Request {
    id: Option<u64>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct Response<T> {
    jsonrpc: String,
    id: Option<u64>,
    result: T,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    jsonrpc: String,
    id: Option<u64>,
    error: JsonRpcError,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Serialize)]
struct InitializeResult {
    protocol_version: String,
    capabilities: serde_json::Value,
    server_info: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolsListResult {
    tools: Vec<ToolDef>,
}

#[derive(Debug, Serialize)]
struct ToolDef {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ToolCallResult {
    content: Vec<serde_json::Value>,
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(_) => continue,
        };

        match req.method.as_str() {
            "initialize" => {
                let result = InitializeResult {
                    protocol_version: "2024-11-05".to_string(),
                    capabilities: serde_json::json!({"tools": {}}),
                    server_info: serde_json::json!({"name": "stub", "version": "1.0.0"}),
                };
                let resp = Response {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result,
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
            }
            "notifications/initialized" => {
                // No response expected for notifications.
            }
            "tools/list" => {
                let result = ToolsListResult {
                    tools: vec![ToolDef {
                        name: "echo".to_string(),
                        description: "Echo the given text back".to_string(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            },
                            "required": ["text"]
                        }),
                    }],
                };
                let resp = Response {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result,
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
            }
            "tools/call" => {
                let text = req
                    .params
                    .get("arguments")
                    .and_then(|a| a.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let result = ToolCallResult {
                    content: vec![serde_json::json!({
                        "type": "text",
                        "text": format!("echo: {}", text)
                    })],
                };
                let resp = Response {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result,
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
            }
            _ => {
                let resp = ErrorResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    error: JsonRpcError {
                        code: -32601,
                        message: format!("Method not found: {}", req.method),
                    },
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
            }
        }
        let _ = stdout.flush();
    }
}

//! JSON-RPC server for the cortex coding agent.
//!
//! Mirrors the `modes/rpc-mode.ts` surface from the TypeScript
//! `packages/coding-agent` package. The server runs over a generic `Read` /
//! `Write` transport and dispatches JSON-RPC 2.0 requests to a method
//! registry. Built-in methods cover the server lifecycle (`initialize`,
//! `initialized`, `shutdown`, `exit`) and the coding tool surface
//! (`tools/list`, `tools/call`).
//!
//! The transport is line-delimited JSON (`\n` separated), which is the same
//! wire format used by the subagent pool (`--mode subagent`) and the
//! stdio-based MCP client.

use cortexcode_agent_tools::ToolRegistry;
use cortexcode_code_tools::permissions::PermissionPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 message received from the wire.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RpcMessage {
    /// A request that expects a response.
    Request(Request),
    /// A notification that does not expect a response.
    Notification(Notification),
}

impl RpcMessage {
    /// Parse a JSON-RPC message, distinguishing requests from notifications
    /// by the presence of an `id` field.
    fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        if value.get("id").is_some() {
            serde_json::from_value(value).map(RpcMessage::Request)
        } else {
            serde_json::from_value(value).map(RpcMessage::Notification)
        }
    }
}

/// A JSON-RPC 2.0 request object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Request {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request identifier. May be a string, number, or null.
    pub id: Option<Value>,
    /// Method name.
    pub method: String,
    /// Optional parameters.
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 notification object (no `id`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC 2.0 error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    ServerError = -32000,
}

impl ErrorCode {
    /// Return the numeric code value.
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl RpcError {
    /// Create an error from a standard error code.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_i32(),
            message: message.into(),
            data: None,
        }
    }

    /// Create a `MethodNotFound` error.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            ErrorCode::MethodNotFound,
            format!("method not found: {}", method),
        )
    }

    /// Create an `InvalidParams` error.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParams, message)
    }

    /// Create an `InternalError` error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

/// Result of handling a single message.
#[derive(Debug, Clone)]
pub enum HandleResult {
    /// A response should be sent back.
    Response(Response),
    /// A notification was handled; no response.
    Acknowledged,
    /// The server should stop after this message.
    Stop,
}

// ---------------------------------------------------------------------------
// Method handler registry
// ---------------------------------------------------------------------------

/// A method handler takes a JSON params value and returns a JSON result or
/// JSON-RPC error.
pub type MethodHandler = Box<dyn Fn(Option<Value>) -> Result<Value, RpcError> + Send + Sync>;

/// JSON-RPC server state.
#[derive(Default)]
pub struct Server {
    handlers: HashMap<String, MethodHandler>,
    initialized: bool,
}

impl Server {
    /// Create an empty server.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler for a method.
    pub fn register(
        &mut self,
        method: impl Into<String>,
        handler: impl Fn(Option<Value>) -> Result<Value, RpcError> + Send + Sync + 'static,
    ) -> &mut Self {
        self.handlers.insert(method.into(), Box::new(handler));
        self
    }

    /// Return true if the server has a handler for `method`.
    pub fn has(&self, method: &str) -> bool {
        self.handlers.contains_key(method)
    }

    /// Handle a parsed JSON-RPC message.
    pub fn handle(&mut self, message: RpcMessage) -> HandleResult {
        match message {
            RpcMessage::Request(req) => {
                let id = req.id.clone();
                let response = self.dispatch(req);
                let (result, error) = match response {
                    Ok(value) => (Some(value), None),
                    Err(e) => (None, Some(e)),
                };
                HandleResult::Response(Response {
                    jsonrpc: "2.0".to_string(),
                    id,
                    result,
                    error,
                })
            }
            RpcMessage::Notification(notif) => {
                if notif.method == "exit" {
                    HandleResult::Stop
                } else if notif.method == "initialized" {
                    self.initialized = true;
                    HandleResult::Acknowledged
                } else {
                    // Notifications are not expected to return errors in this
                    // minimal server; just acknowledge.
                    HandleResult::Acknowledged
                }
            }
        }
    }

    fn dispatch(&mut self, request: Request) -> Result<Value, RpcError> {
        if request.jsonrpc != "2.0" {
            return Err(RpcError::new(
                ErrorCode::InvalidRequest,
                "jsonrpc must be 2.0",
            ));
        }

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request.params),
            "shutdown" => self.handle_shutdown(request.params),
            "tools/list" => self.handle_tools_list(request.params),
            "tools/call" => self.handle_tools_call(request.params),
            _ => {
                if let Some(handler) = self.handlers.get(&request.method) {
                    handler(request.params)
                } else {
                    Err(RpcError::method_not_found(&request.method))
                }
            }
        }
    }

    fn handle_initialize(&mut self, _params: Option<Value>) -> Result<Value, RpcError> {
        Ok(serde_json::json!({
            "jsonrpc": "2.0",
            "name": "cortex",
            "version": env!("CARGO_PKG_VERSION"),
            "capabilities": {
                "tools": true,
                "streaming": false,
            }
        }))
    }

    fn handle_shutdown(&mut self, _params: Option<Value>) -> Result<Value, RpcError> {
        Ok(Value::Object(serde_json::Map::new()))
    }

    fn handle_tools_list(&self, _params: Option<Value>) -> Result<Value, RpcError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let tools = cortexcode_code_tools::default_tools(cwd, PermissionPolicy::default());
        let registry = ToolRegistry::from_iter(tools);
        let list: Vec<Value> = registry
            .list()
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        Ok(Value::Array(list))
    }

    fn handle_tools_call(&self, params: Option<Value>) -> Result<Value, RpcError> {
        let params = params.ok_or_else(|| RpcError::invalid_params("missing params"))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::invalid_params("missing name"))?
            .to_string();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let tools = cortexcode_code_tools::default_tools(cwd, PermissionPolicy::default());
        let registry = ToolRegistry::from_iter(tools);
        let tool = registry
            .get(&name)
            .ok_or_else(|| RpcError::method_not_found(&format!("tool: {}", name)))?;
        let result = (tool.execute)(String::new(), arguments, None, None)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(serde_json::json!({
            "content": result.content,
            "details": result.details,
            "terminate": result.terminate,
        }))
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// Run a server over the provided reader and writer until `exit` is received
/// or the input stream closes.
///
/// Errors are written to `output` as JSON-RPC error responses when a request id
/// is available; otherwise they are silently dropped.
pub fn serve_stdio<R, W>(server: &mut Server, input: R, output: &mut W) -> Result<(), RpcError>
where
    R: std::io::Read,
    W: std::io::Write,
{
    let reader = BufReader::new(input);
    for line in reader.lines() {
        let line = line.map_err(|e| RpcError::internal(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let response = Response {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(RpcError::new(ErrorCode::ParseError, e.to_string())),
                };
                send_response(output, &response)?;
                continue;
            }
        };
        let message: RpcMessage = match RpcMessage::from_value(value) {
            Ok(m) => m,
            Err(e) => {
                let response = Response {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(RpcError::new(ErrorCode::ParseError, e.to_string())),
                };
                send_response(output, &response)?;
                continue;
            }
        };
        match server.handle(message) {
            HandleResult::Response(response) => send_response(output, &response)?,
            HandleResult::Acknowledged => {}
            HandleResult::Stop => break,
        }
    }
    Ok(())
}

fn send_response<W: Write>(output: &mut W, response: &Response) -> Result<(), RpcError> {
    let line = serde_json::to_string(response).map_err(|e| RpcError::internal(e.to_string()))?;
    writeln!(output, "{}", line).map_err(|e| RpcError::internal(e.to_string()))?;
    output
        .flush()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience entry point
// ---------------------------------------------------------------------------

/// Create a server with the default built-in methods and serve it over stdin/stdout.
///
/// This is the entry point used by the `cortex --mode rpc` and `cortex --mode
/// subagent` command lines.
pub fn start_stdio_server() -> Result<(), RpcError> {
    let mut server = Server::new();
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_stdio(&mut server, stdin.lock(), &mut stdout.lock())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request(id: impl Serialize, method: &str, params: Option<Value>) -> Request {
        Request {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::to_value(id).unwrap()),
            method: method.to_string(),
            params,
        }
    }

    #[test]
    fn test_initialize() {
        let mut server = Server::new();
        let req = make_request(1, "initialize", None);
        let result = server.dispatch(req).unwrap();
        assert_eq!(result["name"], "cortex");
        assert_eq!(result["jsonrpc"], "2.0");
    }

    #[test]
    fn test_unknown_method() {
        let mut server = Server::new();
        let req = make_request(1, "foo/bar", None);
        let err = server.dispatch(req).unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound.as_i32());
    }

    #[test]
    fn test_custom_handler() {
        let mut server = Server::new();
        server.register("ping", |_| Ok(Value::String("pong".to_string())));
        let req = make_request(1, "ping", None);
        let result = server.dispatch(req).unwrap();
        assert_eq!(result, "pong");
    }

    #[test]
    fn test_serve_stdio_initialize() {
        let mut server = Server::new();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
        let mut output = Vec::new();
        serve_stdio(&mut server, &input[..], &mut output).unwrap();
        let line = String::from_utf8(output).unwrap();
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["id"], 1);
        assert!(parsed["result"].is_object());
    }

    #[test]
    fn test_serve_stdio_exit() {
        let mut server = Server::new();
        let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"exit\"}\n";
        let mut output = Vec::new();
        serve_stdio(&mut server, &input[..], &mut output).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn test_invalid_json() {
        let mut server = Server::new();
        let input = b"not json\n";
        let mut output = Vec::new();
        serve_stdio(&mut server, &input[..], &mut output).unwrap();
        let parsed: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(parsed["error"]["code"], ErrorCode::ParseError.as_i32());
    }

    #[test]
    fn test_tools_list() {
        let mut server = Server::new();
        let req = make_request(1, "tools/list", None);
        let result = server.dispatch(req).unwrap();
        let tools = result.as_array().unwrap();
        assert!(!tools.is_empty());
        assert!(tools.iter().any(|t| t["name"] == "read"));
    }

    #[test]
    fn test_tools_call_unknown() {
        let mut server = Server::new();
        let req = make_request(1, "tools/call", Some(serde_json::json!({"name": "nope"})));
        let err = server.dispatch(req).unwrap_err();
        assert_eq!(err.code, ErrorCode::MethodNotFound.as_i32());
    }
}

//! MCP transport implementations for stdio and HTTP/SSE servers.

use crate::error::McpError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// JSON-RPC request sent to an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC error object.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC response received from an MCP server.
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A connected MCP transport that can issue JSON-RPC calls and notifications.
pub trait McpConnection: Send + Sync {
    /// Issue a JSON-RPC request and wait for the result.
    fn rpc(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_ms: Option<u64>,
    ) -> Result<Value, McpError>;

    /// Send a JSON-RPC notification (no id, no response expected).
    fn notify(&self, method: &str, params: Option<Value>);

    /// Terminate the connection and reject any pending requests.
    fn terminate(&self);
}

pub type McpConnectionRef = Arc<dyn McpConnection + Send + Sync + 'static>;

type PendingMap = Arc<Mutex<HashMap<u64, Sender<Result<Value, McpError>>>>>;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn route_jsonrpc_response(resp: JsonRpcResponse, pending: &PendingMap) {
    let id = match resp.id {
        Some(id) => id,
        None => return,
    };
    let tx = pending.lock().unwrap().remove(&id);
    if let Some(tx) = tx {
        let result = if let Some(err) = resp.error {
            Err(McpError::Server {
                message: err.message,
            })
        } else {
            Ok(resp.result.unwrap_or(Value::Null))
        };
        let _ = tx.send(result);
    }
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Configuration for a local stdio MCP server.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Unique server identifier used as prefix for tool names.
    pub name: String,
    /// Command to spawn.
    pub command: String,
    /// Arguments passed to the command.
    pub args: Option<Vec<String>>,
    /// Extra environment variables merged into the current environment.
    pub env: Option<HashMap<String, String>>,
}

pub struct StdioMcpConnection {
    name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    next_id: AtomicU64,
    pending: PendingMap,
    terminated: Arc<AtomicBool>,
    _reader: Option<JoinHandle<()>>,
}

impl StdioMcpConnection {
    fn send_message(&self, request: &JsonRpcRequest) -> Result<(), McpError> {
        let line = serde_json::to_string(request)? + "\n";
        let mut stdin = self.stdin.lock().unwrap();
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }
}

impl McpConnection for StdioMcpConnection {
    fn rpc(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_ms: Option<u64>,
    ) -> Result<Value, McpError> {
        if self.terminated.load(Ordering::SeqCst) {
            return Err(McpError::Terminated {
                name: self.name.clone(),
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);

        if let Err(e) = self.send_message(&request) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }

        if let Some(timeout) = timeout_ms.filter(|t| *t > 0) {
            match rx.recv_timeout(Duration::from_millis(timeout)) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => {
                    self.pending.lock().unwrap().remove(&id);
                    Err(McpError::Timeout {
                        method: method.to_string(),
                        timeout_ms: timeout,
                    })
                }
                Err(RecvTimeoutError::Disconnected) => Err(McpError::Terminated {
                    name: self.name.clone(),
                }),
            }
        } else {
            match rx.recv() {
                Ok(result) => result,
                Err(_) => Err(McpError::Terminated {
                    name: self.name.clone(),
                }),
            }
        }
    }

    fn notify(&self, method: &str, params: Option<Value>) {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params,
        };
        let _ = self.send_message(&request);
    }

    fn terminate(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut pending = self.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(McpError::Terminated {
                name: self.name.clone(),
            }));
        }
        drop(pending);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

fn stdio_reader_loop(
    name: String,
    stdout: ChildStdout,
    pending: PendingMap,
    terminated: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                    route_jsonrpc_response(resp, &pending);
                }
            }
            Err(_) => break,
        }
    }

    terminated.store(true, Ordering::SeqCst);
    let mut pending = pending.lock().unwrap();
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(McpError::Terminated { name: name.clone() }));
    }
}

/// Spawn a local stdio MCP server and return a connection to it.
pub fn connect_stdio_mcp_server(config: McpServerConfig) -> Result<McpConnectionRef, McpError> {
    let mut cmd = Command::new(&config.command);
    if let Some(args) = &config.args {
        cmd.args(args);
    }
    let mut envs: HashMap<String, String> = std::env::vars().collect();
    if let Some(extra) = &config.env {
        envs.extend(extra.clone());
    }
    cmd.envs(&envs);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| McpError::Config("failed to open child stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| McpError::Config("failed to open child stdout".into()))?;

    let pending = Arc::new(Mutex::new(HashMap::new()));
    let terminated = Arc::new(AtomicBool::new(false));
    let reader_pending = Arc::clone(&pending);
    let reader_term = Arc::clone(&terminated);
    let name = config.name.clone();
    let reader_handle = std::thread::spawn(move || {
        stdio_reader_loop(name, stdout, reader_pending, reader_term);
    });

    Ok(Arc::new(StdioMcpConnection {
        name: config.name,
        stdin: Arc::new(Mutex::new(stdin)),
        child: Arc::new(Mutex::new(child)),
        next_id: AtomicU64::new(1),
        pending,
        terminated,
        _reader: Some(reader_handle),
    }))
}

// ---------------------------------------------------------------------------
// HTTP / SSE transport
// ---------------------------------------------------------------------------

/// Configuration for a remote HTTP/SSE MCP server.
#[derive(Debug, Clone)]
pub struct McpHttpServerConfig {
    /// Unique server identifier used as prefix for tool names.
    pub name: String,
    /// Remote server URL.
    pub url: String,
    /// Extra HTTP headers sent on every request.
    pub headers: Option<HashMap<String, String>>,
    /// `"http"` = Streamable HTTP with SSE fallback (default); `"sse"` = legacy HTTP+SSE.
    pub server_type: Option<String>,
}

/// Options that customize remote-server behavior.
///
/// OAuth support is currently a placeholder; the fields are retained for API
/// compatibility with the TypeScript implementation.
#[derive(Clone, Default)]
pub struct McpRemoteOptions {
    /// Directory for persisted OAuth state.
    pub auth_storage_dir: Option<String>,
    /// Open the authorization URL.
    pub open_browser: Option<Arc<dyn Fn(String) + Send + Sync>>,
    /// Max time to wait for the browser redirect.
    pub auth_timeout_ms: Option<u64>,
}

impl std::fmt::Debug for McpRemoteOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRemoteOptions")
            .field("auth_storage_dir", &self.auth_storage_dir)
            .field("auth_timeout_ms", &self.auth_timeout_ms)
            .field("open_browser", &self.open_browser.is_some())
            .finish()
    }
}

enum HttpMode {
    Http,
    Sse { post_url: String },
}

pub struct HttpMcpConnection {
    config: McpHttpServerConfig,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    next_id: AtomicU64,
    pending: PendingMap,
    session_id: Arc<Mutex<Option<String>>>,
    mode: Mutex<HttpMode>,
    got_response: AtomicBool,
    fallback: AtomicBool,
    terminated: Arc<AtomicBool>,
    _reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl HttpMcpConnection {
    fn build_post_request(&self, url: &str) -> reqwest::RequestBuilder {
        let mut builder = self.client.post(url);
        if let Some(headers) = &self.config.headers {
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
        }
        if let Some(sid) = self.session_id.lock().unwrap().as_ref() {
            builder = builder.header("mcp-session-id", sid);
        }
        builder
    }

    fn dispatch_request(&self, request: &JsonRpcRequest) -> Result<(), McpError> {
        if self.terminated.load(Ordering::SeqCst) {
            return Err(McpError::Terminated {
                name: self.config.name.clone(),
            });
        }

        let request = request.clone();
        self.runtime.block_on(async {
            loop {
                // If the user explicitly configured an SSE server, open the SSE
                // stream before issuing the first request.
                if matches!(&*self.mode.lock().unwrap(), HttpMode::Http)
                    && self.config.server_type.as_deref() == Some("sse")
                {
                    let post_url = self.start_sse_transport().await?;
                    *self.mode.lock().unwrap() = HttpMode::Sse { post_url };
                    continue;
                }

                let url: String = match &*self.mode.lock().unwrap() {
                    HttpMode::Http => self.config.url.clone(),
                    HttpMode::Sse { post_url } => post_url.clone(),
                };
                let body = serde_json::to_vec(&request)?;

                let response = self.build_post_request(&url).body(body).send().await?;
                let status = response.status();

                if let Some(sid) = response
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                {
                    *self.session_id.lock().unwrap() = Some(sid.to_string());
                }

                if status.is_success() {
                    // Notifications and SSE POST acknowledgements (202/204) have no body.
                    if request.id.is_none() || status.as_u16() == 202 || status.as_u16() == 204 {
                        let _ = response.text().await;
                        return Ok(());
                    }

                    let content_type = response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if content_type.contains("text/event-stream") {
                        read_sse_response(response, &self.pending).await?;
                    } else {
                        let resp: JsonRpcResponse = response.json().await?;
                        route_jsonrpc_response(resp, &self.pending);
                    }
                    self.got_response.store(true, Ordering::SeqCst);
                    return Ok(());
                }

                let code = status.as_u16();
                let body = response.text().await.unwrap_or_default();
                let should_fallback = (400..500).contains(&code)
                    && code != 401
                    && code != 403
                    && !self.got_response.load(Ordering::SeqCst)
                    && !self.fallback.load(Ordering::SeqCst)
                    && self.config.server_type.as_deref() != Some("sse");

                if should_fallback {
                    self.fallback.store(true, Ordering::SeqCst);
                    if let Some(handle) = self._reader.lock().unwrap().take() {
                        handle.abort();
                    }
                    let post_url = self.start_sse_transport().await?;
                    *self.mode.lock().unwrap() = HttpMode::Sse { post_url };
                    continue;
                }

                return Err(McpError::HttpStatus { status: code, body });
            }
        })
    }

    async fn start_sse_transport(&self) -> Result<String, McpError> {
        let config = self.config.clone();
        let session_id = Arc::clone(&self.session_id);
        let pending = Arc::clone(&self.pending);
        let terminated = Arc::clone(&self.terminated);

        let mut builder = self.client.get(&config.url);
        if let Some(headers) = &config.headers {
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
        }
        if let Some(sid) = session_id.lock().unwrap().as_ref() {
            builder = builder.header("mcp-session-id", sid);
        }
        builder = builder.header("accept", "text/event-stream");

        let mut response = builder.send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::HttpStatus {
                status: status.as_u16(),
                body,
            });
        }
        if let Some(sid) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *session_id.lock().unwrap() = Some(sid.to_string());
        }

        let mut parser = SseParser::new();
        let mut post_url: Option<String> = None;

        loop {
            if terminated.load(Ordering::SeqCst) {
                return Err(McpError::Terminated {
                    name: config.name.clone(),
                });
            }
            let chunk = response.chunk().await?.unwrap_or_default();
            if chunk.is_empty() {
                break;
            }
            let text = String::from_utf8_lossy(&chunk);
            for event in parser.push(&text) {
                match event.name.as_str() {
                    "endpoint" => {
                        post_url = Some(resolve_url(&config.url, &event.data)?);
                    }
                    "message" => {
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&event.data) {
                            route_jsonrpc_response(resp, &pending);
                        }
                    }
                    _ => {}
                }
            }
            if post_url.is_some() {
                break;
            }
        }

        let post_url =
            post_url.ok_or_else(|| McpError::Request("SSE endpoint event not received".into()))?;

        let handle = tokio::spawn(async move {
            sse_reader_task(response, parser, pending, terminated).await;
        });
        *self._reader.lock().unwrap() = Some(handle);

        Ok(post_url)
    }
}

impl McpConnection for HttpMcpConnection {
    fn rpc(
        &self,
        method: &str,
        params: Option<Value>,
        timeout_ms: Option<u64>,
    ) -> Result<Value, McpError> {
        if self.terminated.load(Ordering::SeqCst) {
            return Err(McpError::Terminated {
                name: self.config.name.clone(),
            });
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let (tx, rx) = channel();
        self.pending.lock().unwrap().insert(id, tx);

        if let Err(e) = self.dispatch_request(&request) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }

        if let Some(timeout) = timeout_ms.filter(|t| *t > 0) {
            match rx.recv_timeout(Duration::from_millis(timeout)) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => {
                    self.pending.lock().unwrap().remove(&id);
                    Err(McpError::Timeout {
                        method: method.to_string(),
                        timeout_ms: timeout,
                    })
                }
                Err(RecvTimeoutError::Disconnected) => Err(McpError::Terminated {
                    name: self.config.name.clone(),
                }),
            }
        } else {
            match rx.recv() {
                Ok(result) => result,
                Err(_) => Err(McpError::Terminated {
                    name: self.config.name.clone(),
                }),
            }
        }
    }

    fn notify(&self, method: &str, params: Option<Value>) {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params,
        };
        let _ = self.dispatch_request(&request);
    }

    fn terminate(&self) {
        if self.terminated.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut pending = self.pending.lock().unwrap();
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(McpError::Terminated {
                name: self.config.name.clone(),
            }));
        }
        drop(pending);
        if let Some(handle) = self._reader.lock().unwrap().take() {
            handle.abort();
        }
    }
}

async fn read_sse_response(
    mut response: reqwest::Response,
    pending: &PendingMap,
) -> Result<(), McpError> {
    let mut parser = SseParser::new();
    loop {
        let chunk = response.chunk().await?.unwrap_or_default();
        if chunk.is_empty() {
            return Err(McpError::Request(
                "SSE response stream closed without message".into(),
            ));
        }
        let text = String::from_utf8_lossy(&chunk);
        for event in parser.push(&text) {
            if event.name == "message" {
                if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&event.data) {
                    route_jsonrpc_response(resp, pending);
                    return Ok(());
                }
            }
        }
    }
}

async fn sse_reader_task(
    mut response: reqwest::Response,
    mut parser: SseParser,
    pending: PendingMap,
    terminated: Arc<AtomicBool>,
) {
    loop {
        if terminated.load(Ordering::SeqCst) {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let text = String::from_utf8_lossy(&chunk);
                for event in parser.push(&text) {
                    if event.name == "message" {
                        if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&event.data) {
                            route_jsonrpc_response(resp, &pending);
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
}

/// Open a connection to a remote MCP server.
pub fn connect_http_mcp_server(
    config: McpHttpServerConfig,
    _options: McpRemoteOptions,
) -> Result<McpConnectionRef, McpError> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| McpError::Config(format!("failed to create tokio runtime: {}", e)))?;
    Ok(Arc::new(HttpMcpConnection {
        config,
        client: reqwest::Client::new(),
        runtime,
        next_id: AtomicU64::new(1),
        pending: Arc::new(Mutex::new(HashMap::new())),
        session_id: Arc::new(Mutex::new(None)),
        mode: Mutex::new(HttpMode::Http),
        got_response: AtomicBool::new(false),
        fallback: AtomicBool::new(false),
        terminated: Arc::new(AtomicBool::new(false)),
        _reader: Mutex::new(None),
    }))
}

// ---------------------------------------------------------------------------
// SSE parser
// ---------------------------------------------------------------------------

struct SseEvent {
    name: String,
    data: String,
}

struct SseParser {
    buffer: String,
    current_event: String,
    current_data: Vec<String>,
}

impl SseParser {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            current_event: String::new(),
            current_data: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=pos).collect();
            let line = line.trim_end_matches('\n').trim_end_matches('\r');
            if line.is_empty() {
                if !self.current_event.is_empty() || !self.current_data.is_empty() {
                    events.push(SseEvent {
                        name: std::mem::take(&mut self.current_event),
                        data: self.current_data.join("\n"),
                    });
                    self.current_data.clear();
                }
            } else if let Some(rest) = line.strip_prefix("event:") {
                self.current_event = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.current_data.push(rest.trim_start().to_string());
            }
        }
        events
    }
}

fn resolve_url(base: &str, path: &str) -> Result<String, McpError> {
    let base =
        reqwest::Url::parse(base).map_err(|e| McpError::Config(format!("invalid url: {}", e)))?;
    let joined = base
        .join(path)
        .map_err(|e| McpError::Config(format!("invalid endpoint: {}", e)))?;
    Ok(joined.to_string())
}

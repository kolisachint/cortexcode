//! Integration tests for `cortexcode-agent-mcp`.
//!
//! These tests mirror the TypeScript `@kolisachint/hoocode-agent-core` MCP
//! loader tests: empty configs, stdio stub servers, and remote streamable HTTP
//! / legacy SSE servers.

use cortexcode_agent_mcp::{close_mcp_tools, load_mcp_tools, McpRemoteOptions};
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Empty / missing config
// ---------------------------------------------------------------------------

#[test]
fn test_empty_config() {
    let dir = temp_dir("empty");

    let empty = dir.join("empty.json");
    std::fs::write(&empty, "{}").unwrap();
    assert!(load_mcp_tools(&empty, McpRemoteOptions::default())
        .unwrap()
        .is_empty());

    let no_servers = dir.join("no-servers.json");
    std::fs::write(&no_servers, json!({"mcpServers": {}}).to_string()).unwrap();
    assert!(load_mcp_tools(&no_servers, McpRemoteOptions::default())
        .unwrap()
        .is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_missing_config_file() {
    let path = std::env::temp_dir().join("cortex-mcp-does-not-exist.json");
    let _ = std::fs::remove_file(&path);
    assert!(load_mcp_tools(&path, McpRemoteOptions::default()).is_err());
}

// ---------------------------------------------------------------------------
// Stdio stub server
// ---------------------------------------------------------------------------

#[test]
fn test_stdio_stub_server() {
    let dir = temp_dir("stdio");
    let bin = std::env::var("CARGO_BIN_EXE_mcp-stub-server")
        .expect("tests must be run by cargo so CARGO_BIN_EXE_mcp-stub-server is set");

    let config = dir.join("mcp.json");
    std::fs::write(
        &config,
        json!({
            "mcpServers": {
                "stub": { "command": bin }
            }
        })
        .to_string(),
    )
    .unwrap();

    let tools = load_mcp_tools(&config, McpRemoteOptions::default()).unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mcp_stub_echo");

    let result = (tools[0].execute)("call-1".into(), json!({"text": "hi there"}), None, None);
    let result = result.unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            cortexcode_ai_types::Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(text.contains("echo: hi there"), "got: {}", text);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// HTTP test servers
// ---------------------------------------------------------------------------

fn parse_request(
    reader: &mut BufReader<&TcpStream>,
) -> Option<(String, String, HashMap<String, String>, Vec<u8>)> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first()?.to_string();
    let path = parts.get(1)?.to_string();

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some((method, path, headers, body))
}

fn make_jsonrpc_response(id: serde_json::Value, result: serde_json::Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn handle_mcp_method(
    method: &str,
    id: serde_json::Value,
    msg: &serde_json::Value,
) -> (u16, String, String, String) {
    match method {
        "initialize" => {
            let result = json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "stub", "version": "1"}
            });
            (
                200,
                "application/json".to_string(),
                make_jsonrpc_response(id, result),
                "mcp-session-id: sess-123\r\n".to_string(),
            )
        }
        "tools/list" => {
            let result = json!({
                "tools": [{
                    "name": "echo",
                    "description": "Echo the given text back",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"]
                    }
                }]
            });
            (
                200,
                "application/json".to_string(),
                make_jsonrpc_response(id, result),
                "mcp-session-id: sess-123\r\n".to_string(),
            )
        }
        "tools/call" => {
            let text = msg
                .get("params")
                .and_then(|p| p.get("arguments"))
                .and_then(|a| a.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let result = json!({"content": [{"type": "text", "text": format!("echo: {}", text)}]});
            (
                200,
                "application/json".to_string(),
                make_jsonrpc_response(id, result),
                "mcp-session-id: sess-123\r\n".to_string(),
            )
        }
        _ => (
            202,
            "application/json".to_string(),
            "".to_string(),
            "".to_string(),
        ),
    }
}

struct TestServer {
    url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    seen_headers: Arc<Mutex<Vec<(String, HashMap<String, String>)>>>,
}

impl TestServer {
    fn stop(self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle {
            let _ = handle.join();
        }
    }
}

fn start_streamable_server(sse_responses: bool) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_c = Arc::clone(&shutdown);
    let seen_headers: Arc<Mutex<Vec<(String, HashMap<String, String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let seen_c = Arc::clone(&seen_headers);

    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        loop {
            if shutdown_c.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let seen = Arc::clone(&seen_c);
                    std::thread::spawn(move || {
                        let mut stream = stream;
                        {
                            let mut reader = BufReader::new(&stream);
                            if let Some((_, _, headers, body)) = parse_request(&mut reader) {
                                let msg: serde_json::Value =
                                    serde_json::from_slice(&body).unwrap_or(json!(null));
                                let method = msg
                                    .get("method")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let id = msg.get("id").cloned().unwrap_or(json!(null));
                                seen.lock().unwrap().push((method.clone(), headers));

                                let (status, content_type, body, extra) = if sse_responses {
                                    match method.as_str() {
                                        "initialize" => {
                                            let result = json!({
                                                "protocolVersion": "2025-03-26",
                                                "capabilities": {"tools": {}},
                                                "serverInfo": {"name": "stub", "version": "1"}
                                            });
                                            let payload = make_jsonrpc_response(id, result);
                                            let sse_body =
                                                format!("event: message\ndata: {}\n\n", payload);
                                            (
                                                200,
                                                "text/event-stream".to_string(),
                                                sse_body,
                                                "mcp-session-id: sess-123\r\n".to_string(),
                                            )
                                        }
                                        "tools/list" => {
                                            let result = json!({"tools": [{"name": "echo", "description": "Echo the given text back", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}]});
                                            let payload = make_jsonrpc_response(id, result);
                                            let sse_body =
                                                format!("event: message\ndata: {}\n\n", payload);
                                            (
                                                200,
                                                "text/event-stream".to_string(),
                                                sse_body,
                                                "mcp-session-id: sess-123\r\n".to_string(),
                                            )
                                        }
                                        "tools/call" => {
                                            let text = msg
                                                .get("params")
                                                .and_then(|p| p.get("arguments"))
                                                .and_then(|a| a.get("text"))
                                                .and_then(|t| t.as_str())
                                                .unwrap_or("");
                                            let result = json!({"content": [{"type": "text", "text": format!("echo: {}", text)}]});
                                            let payload = make_jsonrpc_response(id, result);
                                            let sse_body =
                                                format!("event: message\ndata: {}\n\n", payload);
                                            (
                                                200,
                                                "text/event-stream".to_string(),
                                                sse_body,
                                                "mcp-session-id: sess-123\r\n".to_string(),
                                            )
                                        }
                                        _ => (
                                            202,
                                            "application/json".to_string(),
                                            "".to_string(),
                                            "".to_string(),
                                        ),
                                    }
                                } else {
                                    handle_mcp_method(&method, id, &msg)
                                };

                                let response = format!(
                                    "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n{}\r\n{}",
                                    status,
                                    content_type,
                                    body.len(),
                                    extra,
                                    body
                                );
                                let _ = stream.write_all(response.as_bytes());
                                let _ = stream.flush();
                            }
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    TestServer {
        url: format!("http://127.0.0.1:{}/mcp", port),
        shutdown,
        handle: Some(handle),
        seen_headers,
    }
}

fn start_legacy_sse_server(reject_405_on_mcp_post: bool) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_c = Arc::clone(&shutdown);
    let seen_headers: Arc<Mutex<Vec<(String, HashMap<String, String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let seen_c = Arc::clone(&seen_headers);
    let sse_stream: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
    let sse_stream_c = Arc::clone(&sse_stream);

    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        loop {
            if shutdown_c.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let seen = Arc::clone(&seen_c);
                    let sse_stream = Arc::clone(&sse_stream_c);
                    let shutdown = Arc::clone(&shutdown_c);
                    std::thread::spawn(move || {
                        let mut stream = stream;
                        let (method, path, headers, body) = {
                            let mut reader = BufReader::new(&stream);
                            parse_request(&mut reader).unwrap_or_else(|| {
                                ("".to_string(), "".to_string(), HashMap::new(), Vec::new())
                            })
                        };

                        seen.lock().unwrap().push((method.clone(), headers));

                        if method == "GET" && path == "/mcp" {
                            // Hold the SSE stream open.
                            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: keep-alive\r\n\r\n");
                            let _ = stream.write_all(b"event: endpoint\ndata: /messages\n\n");
                            let _ = stream.flush();
                            if let Ok(clone) = stream.try_clone() {
                                *sse_stream.lock().unwrap() = Some(clone);
                            }
                            while !shutdown.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            return;
                        }

                        if method == "POST" && path == "/mcp" && reject_405_on_mcp_post {
                            let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                            let _ = stream.flush();
                            return;
                        }

                        if method == "POST" && path == "/messages" {
                            let msg: serde_json::Value =
                                serde_json::from_slice(&body).unwrap_or(json!(null));
                            let id = msg.get("id").cloned().unwrap_or(json!(null));
                            let mcp_method =
                                msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
                            let (_status, _ct, payload, _extra) =
                                handle_mcp_method(mcp_method, id.clone(), &msg);
                            let _ = stream.write_all(b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                            let _ = stream.flush();

                            let event = format!("event: message\ndata: {}\n\n", payload);
                            if let Some(sse) = sse_stream.lock().unwrap().as_mut() {
                                let _ = sse.write_all(event.as_bytes());
                                let _ = sse.flush();
                            }
                            return;
                        }

                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                        let _ = stream.flush();
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    TestServer {
        url: format!("http://127.0.0.1:{}/mcp", port),
        shutdown,
        handle: Some(handle),
        seen_headers,
    }
}

fn load_from_config(
    dir: &std::path::Path,
    file: &str,
    server: serde_json::Value,
) -> Vec<cortexcode_agent_types::AgentTool> {
    let path = dir.join(file);
    std::fs::write(&path, json!({"mcpServers": {"remote": server}}).to_string()).unwrap();
    load_mcp_tools(&path, McpRemoteOptions::default()).unwrap()
}

fn run_echo_tool(tool: &cortexcode_agent_types::AgentTool, text: &str) -> String {
    let result = (tool.execute)("call-1".into(), json!({"text": text}), None, None).unwrap();
    result
        .content
        .iter()
        .filter_map(|c| match c {
            cortexcode_ai_types::Content::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// Remote HTTP tests
// ---------------------------------------------------------------------------

#[test]
fn test_http_streamable() {
    let dir = temp_dir("http");
    let server = start_streamable_server(false);

    let tools = load_from_config(
        &dir,
        "http.json",
        json!({
            "type": "http",
            "url": server.url,
            "headers": {"x-api-key": "test-token"}
        }),
    );
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mcp_remote_echo");
    assert!(run_echo_tool(&tools[0], "hi remote").contains("echo: hi remote"));

    let seen = server.seen_headers.lock().unwrap();
    for (_, headers) in seen.iter() {
        assert_eq!(headers.get("x-api-key"), Some(&"test-token".to_string()));
    }
    let tools_call = seen.iter().find(|(m, _)| m == "tools/call");
    assert_eq!(
        tools_call.and_then(|(_, h)| h.get("mcp-session-id")),
        Some(&"sess-123".to_string())
    );
    drop(seen);

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_http_sse_responses() {
    let dir = temp_dir("http-sse");
    let server = start_streamable_server(true);

    let tools = load_from_config(
        &dir,
        "http-sse.json",
        json!({"type": "http", "url": server.url}),
    );
    assert_eq!(tools.len(), 1);
    assert!(run_echo_tool(&tools[0], "streamed").contains("echo: streamed"));

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_http_url_only() {
    let dir = temp_dir("url-only");
    let server = start_streamable_server(false);

    let tools = load_from_config(&dir, "url-only.json", json!({"url": server.url}));
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mcp_remote_echo");

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_legacy_sse() {
    let dir = temp_dir("sse");
    let server = start_legacy_sse_server(false);

    let tools = load_from_config(&dir, "sse.json", json!({"type": "sse", "url": server.url}));
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "mcp_remote_echo");
    assert!(run_echo_tool(&tools[0], "over sse").contains("echo: over sse"));

    server.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_http_falls_back_to_sse_on_405() {
    let dir = temp_dir("fallback");
    let server = start_legacy_sse_server(true);

    // type "http" attempts Streamable HTTP first; the 405 flips it to legacy SSE.
    let tools = load_from_config(
        &dir,
        "fallback.json",
        json!({"type": "http", "url": server.url}),
    );
    assert_eq!(tools.len(), 1);
    assert!(run_echo_tool(&tools[0], "fell back").contains("echo: fell back"));

    server.stop();
    close_mcp_tools();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_remote_entry_without_url() {
    let dir = temp_dir("bad-url");
    let path = dir.join("bad.json");
    std::fs::write(
        &path,
        json!({"mcpServers": {"remote": {"type": "http"}}}).to_string(),
    )
    .unwrap();
    let err = load_mcp_tools(&path, McpRemoteOptions::default()).unwrap_err();
    assert!(err.to_string().contains("no \"url\""));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cortex-mcp-test-{}-{}-{}",
        name,
        std::process::id(),
        rand::random::<u32>()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

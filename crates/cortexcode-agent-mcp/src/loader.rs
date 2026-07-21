//! MCP tool loader: parse `mcp.json`, connect declared servers, and expose their
//! tools as `AgentTool` instances.

use crate::error::McpError;
use crate::transport::{
    connect_http_mcp_server, connect_stdio_mcp_server, McpConnectionRef, McpHttpServerConfig,
    McpRemoteOptions, McpServerConfig,
};
use cortexcode_agent_types::AgentTool;
use cortexcode_ai_types::{Content, TextContent};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Definition of a single tool advertised by an MCP server.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct McpToolsList {
    tools: Vec<McpToolDef>,
}

#[derive(Debug, Deserialize)]
struct McpConfig {
    #[serde(rename = "mcpServers")]
    mcp_servers: Option<HashMap<String, McpServerEntry>>,
}

#[derive(Debug, Deserialize)]
struct McpServerEntry {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    #[serde(rename = "type")]
    server_type: Option<String>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
}

static LIVE_CONNECTIONS: Mutex<Vec<McpConnectionRef>> = Mutex::new(Vec::new());

const HANDSHAKE_TIMEOUT_MS: u64 = 15_000;

fn handshake(conn: &McpConnectionRef) -> Result<Vec<McpToolDef>, McpError> {
    conn.rpc(
        "initialize",
        Some(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "clientInfo": { "name": "cortexcode-agent-mcp", "version": "0.0.1" }
        })),
        Some(HANDSHAKE_TIMEOUT_MS),
    )?;

    // Per the MCP spec the client must acknowledge initialize before further
    // requests; strict servers gate tools/list on this notification.
    conn.notify("notifications/initialized", None);

    let result = conn.rpc("tools/list", Some(json!({})), Some(HANDSHAKE_TIMEOUT_MS))?;
    let list: McpToolsList = serde_json::from_value(result)?;
    Ok(list.tools)
}

fn build_schema(input_schema: Option<serde_json::Value>) -> serde_json::Value {
    let mut schema = input_schema.unwrap_or_else(|| json!({ "type": "object" }));
    if schema.get("type").is_none() {
        schema["type"] = json!("object");
    }
    if schema.get("properties").is_none() {
        schema["properties"] = json!({});
    }
    if schema.get("required").is_none() {
        schema["required"] = json!([]);
    }
    schema
}

fn create_agent_tool(server_name: &str, conn: &McpConnectionRef, tool: &McpToolDef) -> AgentTool {
    let name = format!("mcp_{}_{}", server_name, tool.name);
    let description = tool
        .description
        .clone()
        .unwrap_or_else(|| format!("MCP tool {} from server {}", tool.name, server_name));
    let parameters = build_schema(tool.input_schema.clone());
    let tool_name = tool.name.clone();
    let conn = Arc::clone(conn);

    let mut agent_tool = AgentTool::new(
        name,
        description,
        parameters,
        Box::new(move |_id, args, signal, _update| {
            if let Some(signal) = signal {
                if signal.aborted() {
                    return Err("Aborted".into());
                }
            }
            let params = json!({ "name": tool_name, "arguments": args });
            let result = conn.rpc("tools/call", Some(params), None)?;
            let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
            Ok(cortexcode_agent_types::AgentToolResult {
                content: vec![Content::Text(TextContent {
                    text,
                    cache_control: None,
                })],
                details: result,
                terminate: false,
            })
        }),
    );
    agent_tool.label = format!("[MCP] {} › {}", server_name, tool.name);
    agent_tool
}

/// Parse a standard `mcp.json` file, connect every declared server, and return
/// their tools as `AgentTool` instances.
///
/// Tool names are prefixed with `mcp_<server>_` so they can be registered
/// alongside built-in agent tools without collisions.
pub fn load_mcp_tools<P: AsRef<Path>>(
    path: P,
    remote_options: McpRemoteOptions,
) -> Result<Vec<AgentTool>, McpError> {
    let raw = std::fs::read_to_string(&path)?;
    let config: McpConfig = serde_json::from_str(&raw)?;
    let servers = config.mcp_servers.unwrap_or_default();
    if servers.is_empty() {
        return Ok(Vec::new());
    }

    let mut tools = Vec::new();
    for (name, entry) in servers {
        let is_remote = entry.server_type.as_deref() == Some("http")
            || entry.server_type.as_deref() == Some("sse")
            || (entry.command.is_none() && entry.url.is_some());

        let conn: McpConnectionRef = if is_remote {
            let url = entry.url.ok_or_else(|| {
                McpError::Config(format!(
                    "{}: mcpServers[\"{}\"] has remote type but no \"url\"",
                    path.as_ref().display(),
                    name
                ))
            })?;
            connect_http_mcp_server(
                McpHttpServerConfig {
                    name: name.clone(),
                    url,
                    headers: entry.headers,
                    server_type: entry.server_type,
                },
                remote_options.clone(),
            )?
        } else {
            let command = entry.command.ok_or_else(|| {
                McpError::Config(format!(
                    "{}: mcpServers[\"{}\"] is missing a \"command\"",
                    path.as_ref().display(),
                    name
                ))
            })?;
            connect_stdio_mcp_server(McpServerConfig {
                name: name.clone(),
                command,
                args: entry.args,
                env: entry.env,
            })?
        };

        let tool_defs = handshake(&conn)?;
        LIVE_CONNECTIONS.lock().unwrap().push(Arc::clone(&conn));
        for def in tool_defs {
            tools.push(create_agent_tool(&name, &conn, &def));
        }
    }

    Ok(tools)
}

/// Terminate every MCP server connection opened by `load_mcp_tools` in this
/// process.
pub fn close_mcp_tools() {
    let mut conns = LIVE_CONNECTIONS.lock().unwrap();
    for conn in conns.drain(..) {
        conn.terminate();
    }
}

//! MCP tool integration for cortex agents.
//!
//! This crate ports the TypeScript `@kolisachint/hoocode-agent-core` MCP loader
//! and transports to Rust. It can:
//!
//! * parse a standard `mcp.json` file (`{ "mcpServers": { ... } }`),
//! * connect stdio (`command`) and remote HTTP/SSE (`url`) MCP servers,
//! * run the `initialize` / `tools/list` handshake,
//! * expose discovered tools as [`AgentTool`] values ready for the agent loop.
//!
//! ```no_run
//! use cortexcode_agent_mcp::load_mcp_tools;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let tools = load_mcp_tools("mcp.json", Default::default())?;
//! for tool in &tools {
//!     println!("discovered {}", tool.name);
//! }
//! # Ok(())
//! # }
//! ```

pub mod error;
pub mod loader;
pub mod transport;

pub use error::McpError;
pub use loader::{close_mcp_tools, load_mcp_tools, McpToolDef};
pub use transport::{
    connect_http_mcp_server, connect_stdio_mcp_server, McpConnection, McpConnectionRef,
    McpHttpServerConfig, McpRemoteOptions, McpServerConfig,
};

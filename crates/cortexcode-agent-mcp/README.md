# cortexcode-agent-mcp

MCP tool integration for cortex agents

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

This crate implements MCP tool integration for cortex agents:

- Parses a standard `mcp.json` file (`{ "mcpServers": { ... } }`).
- Connects stdio (`command`) and remote HTTP/SSE (`url`) MCP servers.
- Runs the MCP `initialize` / `tools/list` handshake.
- Exposes discovered tools as `AgentTool` values prefixed with `mcp_<server>_`.
- Supports Streamable HTTP with automatic fallback to legacy HTTP+SSE when the
  endpoint returns a 4xx error.

OAuth support is currently a placeholder; the API surface is retained for
future compatibility with the TypeScript implementation.

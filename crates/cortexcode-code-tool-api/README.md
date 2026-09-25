# cortexcode-code-tool-api

Shared plumbing for the built-in tools of the cortex coding agent.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.
Ports these hoocode (`packages/coding-agent/src/core/tools/`) modules:

- `truncate.ts`: `truncate_head`, `truncate_tail`, `truncate_line`, `format_size`, and the
  800-line / 32KB defaults. The same algorithm in `packages/agent/src/harness/utils/truncate.ts`
  uses 2000 lines / 50KB (`HARNESS_DEFAULT_MAX_*`).
- `path-utils.ts`: `expand_path`, `resolve_to_cwd`, `resolve_read_path` (with the macOS
  screenshot variants: AM/PM narrow no-break space, NFD, curly apostrophe).
- `tool-definition-wrapper.ts`: `ToolDefinition` (a tool plus its prompt metadata and a
  context-aware `execute`) and `wrap_tool_definition` into an `AgentTool`.
- `fs_error`: Node-style fs error messages (`ENOENT: no such file or directory, access '/x'`),
  so tool errors read exactly like hoocode's.

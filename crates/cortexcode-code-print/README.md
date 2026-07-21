# cortexcode-code-print

Output formatting for the cortex coding agent

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

This crate formats agent output for the non-interactive `cortex -p` / `cortex --mode json` print mode.

- `PrintMode::Text` extracts and prints the final assistant response as plain text.
- `PrintMode::Json` serializes every `AgentEvent` as a newline-delimited JSON line.
- `PrintFormatter` collects events during a run and renders the chosen output format.

It mirrors the output-formatting side of `modes/print-mode.ts` from the TypeScript
`packages/coding-agent` package.

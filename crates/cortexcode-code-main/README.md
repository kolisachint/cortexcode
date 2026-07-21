# cortexcode-code-main

Main entry point for the cortex coding agent

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

This crate is the `cortex` binary entry point.

It provides the CLI argument parser and command dispatch for the coding agent:

- Parses flags like `-p`, `--mode`, `--provider`, `--model`, `--session`, etc.
- Collects unknown flags as diagnostics for future extension support.
- Routes to help, version, print mode, and interactive mode.

Runtime-backed commands are currently placeholders; the full wiring will happen
once the session runtime and interactive/RPC modes are ported.

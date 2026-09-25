# cortexcode-code-main

Main entry point for the cortex coding agent

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

This crate is the `cortex` binary. It forwards the process arguments to
`cortexcode-code-cli`, which owns argument parsing and mode dispatch.

# cortexcode

Rust migration of the [HooCode](https://github.com/kolisachint/hoocode) TypeScript coding-agent framework.

This is a multi-crate workspace that mirrors the structure of the [pycortex](https://github.com/kolisachint/pycortex) Python migration. Each namespace (`ai`, `agent`, `code`, `tui`) is split into focused, version-locked crates published to crates.io.

## Workspace structure

```
crates/
  cortexcode/              # Top-level umbrella crate
  cortexcode-ai/           # AI namespace umbrella
  cortexcode-ai-types/
  cortexcode-ai-models/
  ...
  cortexcode-agent/        # Agent namespace umbrella
  cortexcode-agent-core/
  ...
  cortexcode-code/         # Code namespace umbrella
  ...
  cortexcode-tui/          # TUI namespace umbrella
  ...
```

All crates share a single lockstep version defined in the workspace `Cargo.toml`.

## Installation

### From source

```bash
git clone https://github.com/kolisachint/cortexcode
cd cortexcode
cargo install --path crates/cortexcode-code-main --bin cortex
```

### Pre-built binaries

Download a pre-built binary for your platform from the [GitHub Releases](https://github.com/kolisachint/cortexcode/releases)
page. Extract it and place the `cortex` executable on your `PATH`.

## Usage

```bash
# Single-shot print mode (text or JSON)
cortex -p "Explain this codebase"
cortex -p --mode json "Explain this codebase"

# Interactive TUI mode
cortex

# JSON-RPC server mode
cortex --mode rpc

# Subagent mode (used internally by the Task tool)
cortex --mode subagent --task-id <id>
```

## Development

```bash
# Build the entire workspace
cargo build

# Run checks for all crates
cargo check --workspace

# Run all tests
cargo test --workspace
```

## Migration status

The workspace ports the TypeScript HooCode project, pinned to **hoocode v0.5.89**
(commit `a6cd96e7`). A 2026-09-24 audit re-baselined progress to roughly 25–30% of
pinned hoocode behavior. The crate skeleton and TUI library are largely ported, but
hoocode-compatible wire formats, the async core, AgentSession, the full tool set and
the interactive TUI are still open. See
[`docs/design/hoocode-to-cortexcode-migration.md`](docs/design/hoocode-to-cortexcode-migration.md)
(§0 audit, §9 Phases 7–13), which is the single source of truth for status.

## Publishing

Publishing is driven from GitHub Actions:

- `Reserve crates.io names` — one-off workflow that publishes `0.0.1` placeholder crates.
- `Release` — bump, build, publish, and create a GitHub release.
- `Merge Release` — auto-releases PRs labeled `rust:patch`, `rust:minor`, or `rust:major`.
- `Build binaries` — cross-compiles the `cortex` binary for Linux, macOS (Intel/Apple Silicon), and Windows.

Crates marked with `[package.metadata.cortex] publish = true` are included in automated releases.

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

The workspace is being ported from the TypeScript HooCode project.
See [`docs/design/hoocode-to-cortexcode-migration.md`](docs/design/hoocode-to-cortexcode-migration.md)
for the detailed phase checklist. Phase 5 (code namespace advanced features) and
Phase 6 (release, parity, and documentation) are complete; the `cortex` CLI
now runs the agent loop in print and interactive modes.

## Publishing

Publishing is driven from GitHub Actions:

- `Reserve crates.io names` — one-off workflow that publishes `0.0.1` placeholder crates.
- `Release` — bump, build, publish, and create a GitHub release.
- `Merge Release` — auto-releases PRs labeled `rust:patch`, `rust:minor`, or `rust:major`.
- `Build binaries` — cross-compiles the `cortex` binary for Linux, macOS (Intel/Apple Silicon), and Windows.

Crates marked with `[package.metadata.cortex] publish = true` are included in automated releases.

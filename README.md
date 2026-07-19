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

## Development

```bash
# Build the entire workspace
cargo build

# Run checks for all crates
cargo check --workspace
```

## Publishing

Publishing is driven from GitHub Actions:

- `Reserve crates.io names` — one-off workflow that publishes `0.0.1` placeholder crates.
- `Release` — bump, build, publish, and create a GitHub release.
- `Merge Release` — auto-releases PRs labeled `rust:patch`, `rust:minor`, or `rust:major`.

Crates marked with `[package.metadata.cortex] publish = true` are included in automated releases.

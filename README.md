# cortexcode

Rust migration of the [HooCode](https://github.com/kolisachint/hoocode-ts) TypeScript coding-agent framework.

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

### Command names: `hoocode` and `hoocode-ts`

This Rust build is installed as **`hoocode`**. The TypeScript original is reached as
**`hoocode-ts`**, a small shim (`scripts/shims/hoocode-ts`) that runs the npm package
`@kolisachint/hoocode-agent` (a global npm install if present, otherwise `npx`; set
`HOOCODE_TS_BIN` to point it anywhere else). Only the installed names changed: the crates,
the cargo binary (`cortex`), the config directory and the in-app name are unchanged. If you
also have the TS one installed globally from npm, put the Rust `hoocode` earlier on `PATH`
(npm's `hoo` alias still runs the TS one).

```bash
git clone https://github.com/kolisachint/cortexcode
cd cortexcode
scripts/install.sh                     # → ~/.local/bin/hoocode and hoocode-ts
scripts/install.sh --prefix /usr/local/bin --also-cortex   # also keep a `cortex` link
```

Plain cargo still works and installs the binary as `cortex`:
`cargo install --path crates/cortexcode-code-main --bin cortex`.

### Pre-built binaries

Download `hoocode-<target>.tar.gz` (or `.zip` on Windows) from the
[GitHub Releases](https://github.com/kolisachint/cortexcode/releases) page. It holds the
`hoocode` binary and the `hoocode-ts` shim; put both on your `PATH`.

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

# Run all tests (CI uses nextest; `cargo test --workspace` works too)
scripts/ci/fetch_hoocode_fixtures.sh   # pinned hoocode fixtures a few tests read
cargo nextest run --workspace
```

### Releases

Label a PR `rust:patch`, `rust:minor` or `rust:major`; merging it runs the release:
CI gates (parallel) → version bump, tag and GitHub release → binaries for 4 targets in
parallel, each uploaded as soon as it is built. Optional labels: `release:skip-gates`
(skip the gates, the PR's CI already passed) and `release:crates` (also publish to
crates.io, off by default). Manual runs: Actions → Release.

## Migration status (paused 2026-10-01, ready to use)

The port of hoocode **v0.5.89** (commit `a6cd96e7`) is paused, and `cortex` is usable as a
daily coding agent. Phases 7, 8 and 11 are complete. Phase 10 is complete apart from the
deferred items listed below. Every remaining task is **deferred** by user decision; none is
in progress. Status per task: `python3 migration/ledger.py status`.

What works (the pinned hoocode behavior, checked against hoocode itself):

- **Modes:** interactive TUI, print mode (`-p`, text and `--mode json`), and `--mode rpc`
  (with a Rust `RpcClient`).
- **Providers:** every catalog provider, plus OAuth logins (`/login`) and `models.json`.
- **Sessions:** AgentSession with compaction, `/new`, `/resume`, `/fork`, `/tree`,
  `--continue`, `--session`, and `/export <file>.jsonl`.
- **Tools:** read, bash, edit, write, lexical SearchCodebase, TodoWrite, ask_options, and the
  Yes/No/Always permission prompt.
- **Agent modes:** ask/plan/build/debug (`/mode`, `/plan`, `/grill`, `/approve`, alt+a).
- **Subagents:** the Task/TaskOutput tools, the task panel, and the subagent roster.
- **Resources:** skills, prompt templates and slash commands, AGENTS.md/CLAUDE.md context
  files, themes, settings (`/settings`) and keybindings.

Deferred (not available yet, even though `--help` still lists some of them):

| Area | Ledger task |
|---|---|
| MCP servers (`mcp.json`, stdio/HTTP/SSE, OAuth) | 9.1, 10.11 |
| `webfetch` / `websearch` (`--enable-webtools`) | 10.2e |
| Plugins and marketplace (`--enable-plugintools`) | 12.1 |
| `cortex install/remove/update/list` package manager | 12.2 |
| Code extensions (`-e`, extension flags) | 12.3 |
| Semantic and hybrid search (`embsearch`). Search is lexical-only. | 12.4 |
| `/loop`, `/goal` autonomous loop, cron/scheduler | 12.5 |
| Warm subagent pool (`--warm-subagents`) | 12.6 |
| HTML export and `/share`, `/learn`, canvas, `--team`, voice, telemetry, version check | 12.7 |

Known parity gaps: tasks 10.2a/b/c/d/f/g, 10.4c and 10.5 pass Level 1 but not Level 2. The
default-bundle system prompt in hoocode also advertises the SearchHooCode self-knowledge tool
from 12.4, so model requests differ in that one block.

The plan and its rationale are in
[`docs/design/hoocode-to-cortexcode-migration.md`](docs/design/hoocode-to-cortexcode-migration.md)
(§0 status, §9 phases), and the handoff log is in `migration/PROGRESS.md`. To resume, take a
deferred task out of `deferred` and say "continue migration" (see `CLAUDE.md`).

## Quick start

```bash
cargo install --path crates/cortexcode-code-main --bin cortex   # puts `cortex` on PATH
export ANTHROPIC_API_KEY=...        # or any provider key from `cortex --help`, or /login
cortex                              # interactive TUI
cortex -p "Summarize this repo"     # one-shot
```

- Config, auth, sessions and settings live in `~/.cortexcode/agent` (override with
  `CORTEX_CODING_AGENT_DIR`). Project overrides go in `./.cortexcode/`.
- The settings, `models.json`, `auth.json`, session JSONL and `hoo-config.json` formats match
  hoocode's, so an existing hoocode setup can be copied over.

## Publishing

Publishing is driven from GitHub Actions:

- `Reserve crates.io names` — one-off workflow that publishes `0.0.1` placeholder crates.
- `Release` — bump, build, publish, and create a GitHub release.
- `Merge Release` — auto-releases PRs labeled `rust:patch`, `rust:minor`, or `rust:major`.
- `Build binaries` — cross-compiles the `cortex` binary for Linux, macOS (Intel/Apple Silicon), and Windows.

Crates marked with `[package.metadata.cortex] publish = true` are included in automated releases.

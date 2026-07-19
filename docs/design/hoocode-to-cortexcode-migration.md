# HooCode (TypeScript) → CortexCode (Rust) Migration Design Document

> **Status:** REVISED — aligned with existing cortexcode repo structure  
> **Author:** Sachin Koli  
> **Source Repo:** https://github.com/kolisachint/hoocode (TypeScript monorepo, ~828 commits)  
> **Target Repo:** https://github.com/kolisachint/cortexcode (Rust workspace, this repo)  
> **Migration Reference:** https://github.com/kolisachint/pycortex (Python ultramodular migration)  

---

## Table of Contents

1. [Scope and Goals](#1-scope-and-goals)
2. [Principles](#2-principles)
3. [Source Analysis — HooCode (TypeScript)](#3-source-analysis--hoocode-typescript)
4. [Target Architecture — Multi-Level Modular Crate Structure](#4-target-architecture--multi-level-modular-crate-structure)
5. [Ultramodular Leaf Crate Map](#5-ultramodular-leaf-crate-map)
    - [5.1 AI Namespace](#51-ai-namespace)
    - [5.2 Agent Namespace](#52-agent-namespace)
    - [5.3 Code Namespace](#53-code-namespace)
    - [5.4 TUI Namespace](#54-tui-namespace)
6. [Dependency Graph](#6-dependency-graph)
7. [Key Migration Decisions: TypeScript → Rust](#7-key-migration-decisions-typescript--rust)
8. [CI / CD Pipeline](#8-ci--cd-pipeline)
    - [8.1 CI Workflow](#81-ci-workflow)
    - [8.2 PR Merge-Release Workflow](#82-pr-merge-release-workflow)
    - [8.3 Manual Release Workflow](#83-manual-release-workflow)
    - [8.4 Cargo Publish](#84-cargo-publish)
9. [Migration Plan](#9-migration-plan)
10. [Edge Cases and Risk Mitigation](#10-edge-cases-and-risk-mitigation)

---

## 1. Scope and Goals

### What

Rewrite [HooCode](https://github.com/kolisachint/hoocode) — a deterministic terminal coding agent written as a TypeScript npm monorepo (4 packages, ~120K LOC) — into an **ultramodular Rust workspace** of ~47 small, independently versioned crates published on [crates.io](https://crates.io).

### Why

| Concern | TypeScript | Rust |
|---|---|---|
| Startup time | Node.js/bun runtime overhead (100–300 ms) | Near-instant (~1 ms) |
| Distribution | Requires Node.js; binary bundling fragile (`pkg`, `bun build --compile`) | Single static binary per platform via `cargo build` |
| Performance | GC pressure, large heap | Zero-cost abstractions, deterministic memory |
| Cross-platform | 3 targets via fragile tooling | Every platform natively via `rustc` target triples |
| Native tooling | Downloads fd/rg + JS fallback | Pure-Rust fallback via `grep`/`ignore`/`walkdir` crates |

### Non-Goals

- Full feature parity in initial release (tracked incrementally per Phase).
- Backward-compatible npm publishing (Rust crates replace npm packages).
- TypeScript extension/plugin system in v1 (WASM plugin system deferred).

---

## 2. Principles

Adapted from the [pycortex migration](https://github.com/kolisachint/pycortex) which follows the same architecture:

1. **Ultra-modular.** Each crate has one responsibility, minimal deps, its own tests, its own `Cargo.toml`. Stable leaves publish first.
2. **Stability-ordered.** Port in order of *lowest churn first* (measured from hoocode git history). Stable code lands on crates.io early; volatile code stays in-repo until settled.
3. **Never broken.** Every migration step ends with `cargo test --workspace` green. Steps are small, atomic, and independently revertable.
4. **Testable by construction.** Every ported module ships with tests ported from the TS originals. The `faux` provider makes the whole stack testable offline.
5. **Executable plan.** The migration plan (Phase sections below) is a machine-readable checklist tracked in this document.
6. **Automated releases.** GitHub Actions + crates.io token publish tagged crates. No manual uploads.
7. **Lockstep versioning.** All crates share one workspace-level version (semver). Version bumps happen atomically via `scripts/bump_versions.py`.

---

## 3. Source Analysis — HooCode (TypeScript)

### 3.1 Package inventory

Four npm packages, lockstep-versioned at **0.4.146**:

| Package | npm name | LOC (src) | Responsibility |
|---|---|---|---|
| `packages/tui` | `@kolisachint/hoocode-tui` | ~11,400 | Terminal UI library: differential renderer, components, keybindings |
| `packages/ai` | `@kolisachint/hoocode-ai` | ~27,700 | Unified LLM API: providers, streaming, model discovery, OAuth, images |
| `packages/agent` | `@kolisachint/hoocode-agent-core` | ~9,500 | Provider-agnostic agent loop, tool execution, state, sessions, MCP |
| `packages/coding-agent` | `@kolisachint/hoocode-agent` | ~71,500 | The `hoocode` CLI: tools, permission gates, modes, extensions, subagents |

### 3.2 Dependency graph (build order = leaves first)

```
tui   (no internal deps)
ai    (no internal deps)
agent          → ai
coding-agent   → agent, ai, tui
```

### 3.3 Churn analysis → stability tiers

| Tier | Contents | crates.io posture |
|---|---|---|
| **T0 — frozen** | tui core (renderer, components, keys), ai types/stream, agent loop + types | Publish early, semver from day one |
| **T1 — settling** | ai providers (anthropic, openai, google), agent harness, MCP | Publish after tests ported |
| **T2 — volatile** | coding-agent `core/` (tools, session, settings, extensions) | In-repo only until stable |
| **T3 — hot / UI** | interactive mode, subagent pool, task panel | Port last |

**Migration order follows stability tiers AND dependency order:**  
`tui → ai → agent → code`

### 3.4 External dependency mapping (TypeScript → Rust)

| TS dep | Rust equivalent | Used by |
|---|---|---|
| `typebox` | `serde` + derive macros | ai, agent |
| `chalk` | `colored` / `termcolor` | tui, code |
| `marked` | `pulldown-cmark` or `comrak` | tui |
| `@anthropic-ai/sdk` / `openai` / `@google/genai` | `reqwest` raw HTTP clients | ai |
| `@modelcontextprotocol/sdk` | `mcp` (Rust SDK) + custom client | agent |
| `ignore` (gitignore) | `ignore` crate | agent, code |
| `glob` / `minimatch` | `glob` / `globset` crate | code |
| `diff` | `similar` crate | code |
| `uuid`, `yaml` | `uuid` crate, `serde_yaml` | agent |
| `jiti` (dynamic import) | `libloading` / `wasmtime` (deferred) | code extensions |
| `undici` (HTTP) | `reqwest` | ai, code |
| `proper-lockfile` | `fs2` or advisory `tokio::sync::Mutex` | code |
| Biome + tsgo | `cargo fmt` + `cargo clippy` | root |
| bun test / vitest | `cargo test` | all |

---

## 4. Target Architecture — Multi-Level Modular Crate Structure

### 4.1 Naming convention

Following the **pycortex** model (which itself follows the hooocde model), each TypeScript package explodes into many small crates grouped by *co-change*. Crates share a **single lockstep version** defined at the workspace level.

**Naming pattern:** `cortexcode-{namespace}-{leaf}`

| Namespace | Crate name pattern | Import namespace | PyPI equivalent (pycortex) |
|---|---|---|---|
| **Top umbrella** | `cortexcode` | `cortexcode` | `cortexcode` |
| **AI** | `cortexcode-ai-*` | `cortexcode::ai::*` | `cortexcode-ai-*` |
| **Agent** | `cortexcode-agent-*` | `cortexcode::agent::*` | `cortexcode-agent-*` |
| **Code** | `cortexcode-code-*` | `cortexcode::code::*` | `cortexcode-cli-*` |
| **TUI** | `cortexcode-tui-*` | `cortexcode::tui::*` | `cortexcode-tui-*` |

### 4.2 Crate types

| Type | Example | Contains code? | `publish` metadata | Purpose |
|---|---|---|---|---|
| **Top umbrella** | `cortexcode` | No (re-exports only) | `true` | Single `cortexcode = "X.Y.Z"` dependency for users |
| **Namespace umbrella** | `cortexcode-ai` | No (re-exports sub-crates) | `true` | Allows `cortexcode-ai = "X.Y.Z"` to install full namespace |
| **Leaf** | `cortexcode-ai-types` | Yes | `true` (when stable) | Single responsibility, independently testable |
| **Leaf (draft)** | `cortexcode-code-tools` | Yes | `false` | In development, not yet ready for publication |

Umbrella crates carry `[package.metadata.cortex] publish = true` but have **zero code** — they depend on all their namespace's leaf crates and re-export them. This means `cargo add cortexcode-ai` installs every AI leaf.

### 4.3 Repository layout

```
cortexcode/
├── Cargo.toml                 # workspace root -- single version, all members listed
├── Cargo.lock
├── scripts/
│   ├── bump_versions.py       # lockstep version bump across all Cargo.toml files
│   └── publish_packages.py    # publish publishable crates in dependency order
├── .github/workflows/
│   ├── ci.yml                 # fmt + clippy + check + test + doc
│   ├── release.yml            # manual dispatch + workflow_call
│   ├── merge-release.yml      # auto-release on rust:patch/minor/major label
│   └── reserve-names.yml      # one-off crates.io name reservation
├── docs/
│   └── design/
│       └── hoocode-to-cortexcode-migration.md    # this document
└── crates/
    ├── cortexcode/                    # [top umbrella] re-exports all namespaces
    ├── cortexcode-ai/                 # [AI umbrella] re-exports all ai leaves
    ├── cortexcode-ai-types/           # [leaf] core types
    ├── cortexcode-ai-models/          # [leaf] model registry
    ├── cortexcode-ai-stream/          # [leaf] streaming abstraction
    ├── cortexcode-ai-env/             # [leaf] API key detection
    ├── cortexcode-ai-util/            # [leaf] JSON repair, validation, etc.
    ├── cortexcode-ai-oauth/           # [leaf] OAuth flows
    ├── cortexcode-ai-images/          # [leaf] image generation
    ├── cortexcode-ai-provider-anthropic/  # [leaf] Anthropic provider
    ├── cortexcode-ai-provider-openai/     # [leaf] OpenAI provider
    ├── cortexcode-ai-provider-google/     # [leaf] Google Gemini provider
    ├── cortexcode-ai-provider-azure/      # [leaf] Azure OpenAI provider
    ├── cortexcode-ai-provider-faux/       # [leaf] test provider
    ├── cortexcode-agent/              # [agent umbrella] re-exports all agent leaves
    ├── cortexcode-agent-types/        # [leaf] shared agent types
    ├── cortexcode-agent-core/         # [leaf] Agent struct + orchestration
    ├── cortexcode-agent-loop/         # [leaf] agent turn loop
    ├── cortexcode-agent-harness/      # [leaf] messages, system prompt, templates
    ├── cortexcode-agent-session/      # [leaf] session persistence
    ├── cortexcode-agent-compaction/   # [leaf] context compaction
    ├── cortexcode-agent-tools/        # [leaf] tool registry
    ├── cortexcode-agent-mcp/          # [leaf] MCP transport
    ├── cortexcode-code/               # [code umbrella] re-exports all code leaves
    ├── cortexcode-code-config/        # [leaf] settings, config paths
    ├── cortexcode-code-main/          # [leaf] CLI entry point
    ├── cortexcode-code-tools/         # [leaf] built-in tools (read, bash, edit, ...)
    ├── cortexcode-code-session/       # [leaf] session management
    ├── cortexcode-code-prompts/       # [leaf] system prompt, mode prompts
    ├── cortexcode-code-print/         # [leaf] print mode
    ├── cortexcode-code-rpc/           # [leaf] RPC mode
    ├── cortexcode-code-resources/     # [leaf] resource loading
    ├── cortexcode-code-subagents/     # [leaf] subagent pool
    ├── cortexcode-code-extensions/    # [leaf] extension system
    ├── cortexcode-tui/                # [TUI umbrella] re-exports all tui leaves
    ├── cortexcode-tui-components/     # [leaf] UI widgets
    ├── cortexcode-tui-editing/        # [leaf] text editor
    ├── cortexcode-tui-fuzzy/          # [leaf] fuzzy matching
    ├── cortexcode-tui-images/         # [leaf] terminal image rendering
    ├── cortexcode-tui-keys/           # [leaf] keybindings
    ├── cortexcode-tui-render/         # [leaf] differential renderer
    ├── cortexcode-tui-terminal/       # [leaf] terminal abstraction
    └── cortexcode-tui-util/           # [leaf] ANSI width, grapheme handling
```

### 4.4 Workspace `Cargo.toml` (root)

```toml
[workspace]
resolver = "2"
members = [
    # Top-level umbrella
    "crates/cortexcode",

    # AI namespace
    "crates/cortexcode-ai",
    "crates/cortexcode-ai-env",
    "crates/cortexcode-ai-images",
    "crates/cortexcode-ai-models",
    "crates/cortexcode-ai-oauth",
    "crates/cortexcode-ai-provider-anthropic",
    "crates/cortexcode-ai-provider-azure",
    "crates/cortexcode-ai-provider-faux",
    "crates/cortexcode-ai-provider-google",
    "crates/cortexcode-ai-provider-openai",
    "crates/cortexcode-ai-stream",
    "crates/cortexcode-ai-types",
    "crates/cortexcode-ai-util",

    # Agent namespace
    "crates/cortexcode-agent",
    "crates/cortexcode-agent-core",
    "crates/cortexcode-agent-compaction",
    "crates/cortexcode-agent-harness",
    "crates/cortexcode-agent-loop",
    "crates/cortexcode-agent-mcp",
    "crates/cortexcode-agent-session",
    "crates/cortexcode-agent-tools",
    "crates/cortexcode-agent-types",

    # Code namespace
    "crates/cortexcode-code",
    "crates/cortexcode-code-config",
    "crates/cortexcode-code-extensions",
    "crates/cortexcode-code-main",
    "crates/cortexcode-code-print",
    "crates/cortexcode-code-prompts",
    "crates/cortexcode-code-resources",
    "crates/cortexcode-code-rpc",
    "crates/cortexcode-code-session",
    "crates/cortexcode-code-subagents",
    "crates/cortexcode-code-tools",

    # TUI namespace
    "crates/cortexcode-tui",
    "crates/cortexcode-tui-components",
    "crates/cortexcode-tui-editing",
    "crates/cortexcode-tui-fuzzy",
    "crates/cortexcode-tui-images",
    "crates/cortexcode-tui-keys",
    "crates/cortexcode-tui-render",
    "crates/cortexcode-tui-terminal",
    "crates/cortexcode-tui-util",
]

[workspace.package]
version = "0.0.1"
authors = ["Mario Zechner (original author)", "Sachin Koli (HooCode fork)"]
edition = "2021"
license = "MIT"
repository = "https://github.com/kolisachint/cortexcode"
rust-version = "1.78"

[workspace.dependencies]
# All workspace crates declared here with path = "crates/..."
# (full listing in the existing Cargo.toml)
```

### 4.5 Leaf `Cargo.toml` pattern

Every leaf follows a standard template:

```toml
[package]
name = "cortexcode-ai-types"
version.workspace = true
authors.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true
description = "Core types for the cortex AI namespace"
readme = "README.md"

[package.metadata.cortex]
publish = true

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

The `[package.metadata.cortex] publish = true|false` flag controls whether `publish_packages.py` includes this crate in automated releases. Umbrella crates and stable leaves are `true`; volatile leaves are `false` until they reach stability.

### 4.6 Umbrella crate pattern

```toml
[package]
name = "cortexcode-ai"
version.workspace = true
# ... standard fields ...
description = "Umbrella crate for the cortex AI namespace"

[package.metadata.cortex]
publish = true

[dependencies]
cortexcode-ai-types = { workspace = true }
cortexcode-ai-stream = { workspace = true }
cortexcode-ai-models = { workspace = true }
cortexcode-ai-env = { workspace = true }
cortexcode-ai-util = { workspace = true }
cortexcode-ai-oauth = { workspace = true }
cortexcode-ai-images = { workspace = true }
cortexcode-ai-provider-anthropic = { workspace = true }
cortexcode-ai-provider-openai = { workspace = true }
cortexcode-ai-provider-google = { workspace = true }
cortexcode-ai-provider-azure = { workspace = true }
cortexcode-ai-provider-faux = { workspace = true }
```

The umbrella `lib.rs` re-exports all leaf crates:

```rust
//! Umbrella crate for the cortex AI namespace.
pub use cortexcode_ai_types as types;
pub use cortexcode_ai_stream as stream;
pub use cortexcode_ai_models as models;
// ...
```

This allows `use cortexcode_ai::types::*` without specifying the leaf crate directly.

---

## 5. Ultramodular Leaf Crate Map

### 5.1 AI Namespace

| Crate | Import path | Owns (from TS) | Stability | Published |
|---|---|---|---|---|
| `cortexcode-ai-types` | `cortexcode::ai::types` | `types.ts` (LlmEvent, Message, Tool, etc.) | T0 | ✅ |
| `cortexcode-ai-stream` | `cortexcode::ai::stream` | `stream.rs` (channel-backed event stream) | T0 | ✅ |
| `cortexcode-ai-models` | `cortexcode::ai::models` | model registry, generated model lists | T0/T1 | ✅ |
| `cortexcode-ai-env` | `cortexcode::ai::env` | `env-api-keys.ts` (credential detection) | T0 | ✅ |
| `cortexcode-ai-util` | `cortexcode::ai::util` | JSON repair, validation, headers, hash | T0 | ✅ |
| `cortexcode-ai-oauth` | `cortexcode::ai::oauth` | `oauth.ts` | T2 | ✅ |
| `cortexcode-ai-images` | `cortexcode::ai::images` | image generation, image model registry | T2 | ✅ |
| `cortexcode-ai-provider-faux` | `cortexcode::ai::providers::faux` | `providers/faux.ts` (test provider) | T0 | ✅ |
| `cortexcode-ai-provider-anthropic` | `cortexcode::ai::providers::anthropic` | `providers/anthropic.ts` | T1 | ✅ |
| `cortexcode-ai-provider-openai` | `cortexcode::ai::providers::openai` | `providers/openai-*.ts` | T1 | ✅ |
| `cortexcode-ai-provider-google` | `cortexcode::ai::providers::google` | `providers/google*.ts`, `google-vertex.ts` | T1 | ✅ |
| `cortexcode-ai-provider-azure` | `cortexcode::ai::providers::azure` | `providers/azure-openai-responses.ts` | T2 | ✅ |
| `cortexcode-ai` *(umbrella)* | `cortexcode::ai` | re-exports all AI leaves | T0 | ✅ |

**Leaf dependency chain:**  
`types` (no internal deps) ← `models`, `util`, `env` ← `stream` ← `provider-*` ← `oauth`, `images`

### 5.2 Agent Namespace

| Crate | Import path | Owns (from TS) | Stability | Published |
|---|---|---|---|---|
| `cortexcode-agent-types` | `cortexcode::agent::types` | `types.ts` (AgentTool, AgentState, AgentLoopConfig) | T0 | ✅ |
| `cortexcode-agent-core` | `cortexcode::agent::core` | `agent.ts` (Agent struct), `agent-loop.ts` (loop) | T0 | ✅ |
| `cortexcode-agent-loop` | `cortexcode::agent::loop` | `agent-loop.ts` (standalone loop impl) | T0 | ✅ |
| `cortexcode-agent-harness` | `cortexcode::agent::harness` | `harness/{messages,system-prompt,prompt-templates,skills}` | T1 | ✅ |
| `cortexcode-agent-session` | `cortexcode::agent::session` | `harness/session/*`, execution environment | T1 | ✅ |
| `cortexcode-agent-compaction` | `cortexcode::agent::compaction` | `harness/compaction/*` | T1 | ✅ |
| `cortexcode-agent-tools` | `cortexcode::agent::tools` | `tools/default-tools.ts` | T1 | ✅ |
| `cortexcode-agent-mcp` | `cortexcode::agent::mcp` | `tools/mcp-*.ts` | T2 | ✅ |
| `cortexcode-agent` *(umbrella)* | `cortexcode::agent` | re-exports all agent leaves | T0 | ✅ |

**Leaf dependency chain:**  
`types` ← `loop`, `core` ← `harness` ← `session` ← `compaction` ← `tools` ← `mcp`

### 5.3 Code Namespace

Coarser leaves — matches the higher churn rate of the TypeScript `packages/coding-agent/src/`.

| Crate | Import path | Owns (from TS) | Stability | Published |
|---|---|---|---|---|
| `cortexcode-code-config` | `cortexcode::code::config` | `config.ts`, `core/settings-*` | T2 | ✅ |
| `cortexcode-code-main` | `cortexcode::code::main` | `main.ts`, `cli/args.ts` | T2 | ✅ |
| `cortexcode-code-tools` | `cortexcode::code::tools` | `core/tools/{read,bash,edit,write,grep,find,ls}` | T2 | ✅ |
| `cortexcode-code-session` | `cortexcode::code::session` | `core/agent-session*.ts`, `session-manager.ts` | T2 | ✅ |
| `cortexcode-code-prompts` | `cortexcode::code::prompts` | `core/{system-prompt,mode-prompts,prompt-templates}` | T2 | ✅ |
| `cortexcode-code-print` | `cortexcode::code::print` | `modes/print-mode.ts` | T2 | ✅ |
| `cortexcode-code-rpc` | `cortexcode::code::rpc` | `modes/rpc-mode.ts` | T3 | ✅ |
| `cortexcode-code-resources` | `cortexcode::code::resources` | `core/{skills,resource-loader}` | T3 | ✅ |
| `cortexcode-code-subagents` | `cortexcode::code::subagents` | `core/subagent*.ts`, `core/tools/subagent.ts` | T3 | ✅ |
| `cortexcode-code-extensions` | `cortexcode::code::extensions` | `core/extensions/**` (semantics only, WASM redesign) | T3 | ✅ |
| `cortexcode-code` *(umbrella)* | `cortexcode::code` | re-exports all code leaves | T2 | ✅ |

### 5.4 TUI Namespace

| Crate | Import path | Owns (from TS) | Stability | Published |
|---|---|---|---|---|
| `cortexcode-tui-util` | `cortexcode::tui::util` | `utils.ts` (text width, ANSI wrap/truncate) | T0 | ✅ |
| `cortexcode-tui-fuzzy` | `cortexcode::tui::fuzzy` | `fuzzy.ts` | T0 | ✅ |
| `cortexcode-tui-keys` | `cortexcode::tui::keys` | `keys.ts`, `keybindings.ts` | T0 | ✅ |
| `cortexcode-tui-terminal` | `cortexcode::tui::terminal` | `terminal.ts` | T0 | ✅ |
| `cortexcode-tui-render` | `cortexcode::tui::render` | `tui.ts` (differential renderer) | T0 | ✅ |
| `cortexcode-tui-editing` | `cortexcode::tui::editing` | `editor-component.ts`, `kill-ring.ts`, `undo-stack.ts` | T0 | ✅ |
| `cortexcode-tui-components` | `cortexcode::tui::components` | `components/*.ts` (widgets) | T0 | ✅ |
| `cortexcode-tui-images` | `cortexcode::tui::images` | terminal image rendering | T1 | ✅ |
| `cortexcode-tui` *(umbrella)* | `cortexcode::tui` | re-exports all TUI leaves | T0 | ✅ |

**Leaf dependency chain:**  
`util` ← `fuzzy`, `keys`, `terminal` ← `render` ← `editing` ← `components` ← `images`

---

## 6. Dependency Graph

```
                    ┌─────────────────────┐
                    │    cortexcode-tui    │
                    │  (umbrella, T0)      │
                    └─────────┬───────────┘
                              │ depends on
                              ▼
┌──────────────────────────────────────────────────────┐
│                   cortexcode-code                     │
│  (umbrella, T2)    config  main  tools  session       │
│                    prompts  print  rpc  resources      │
│                    subagents  extensions                │
└──────────────────────────────────────────────────────┘
         ▲                                    ▲
         │ depends on                        │ depends on
         │                                    │
┌────────┴──────────┐          ┌──────────────┴───────────┐
│   cortexcode-ai   │          │     cortexcode-agent      │
│  (umbrella, T0)   │          │   (umbrella, T0)          │
│  types  stream     │          │   types  core  loop       │
│  models  env      │◄─────────│   harness  session         │
│  util  provider-* │ depends  │   compaction  tools  mcp   │
│  oauth  images    │          │                             │
└───────────────────┘          └─────────────────────────────┘
```

Build order (leaves-first, matching `cargo build` topological sort):

1. `cortexcode-ai-types`, `cortexcode-tui-util` (no internal deps)
2. `cortexcode-ai-*` leaves, `cortexcode-tui-*` leaves
3. `cortexcode-ai` umbrella, `cortexcode-tui` umbrella
4. `cortexcode-agent-types`, `cortexcode-agent-core`, `cortexcode-agent-loop`
5. `cortexcode-agent-*` leaves
6. `cortexcode-agent` umbrella
7. `cortexcode-code-*` leaves
8. `cortexcode-code` umbrella
9. `cortexcode` top umbrella

This is exactly the order `cargo build --workspace` resolves automatically.

---

## 7. Key Migration Decisions: TypeScript → Rust

### 7.1 Language feature mapping

| TypeScript Feature | Rust Equivalent |
|---|---|
| Dynamic imports (lazy provider registration) | `once_cell::sync::OnceCell<HashMap<&'static str, Box<dyn ProviderFactory>>>` |
| Async/await | `tokio::async` — same semantics, compiler-enforced |
| `npm publish` | `cargo publish` |
| `package.json` `exports` map | Cargo features + `lib.rs` module visibility |
| `biome` (lint + format) | `cargo clippy` + `cargo fmt` |
| `vitest` | `cargo test` (built-in harness) |
| `undici` (HTTP) | `reqwest` + `tower` |
| `chalk` (colors) | `colored` or `termcolor` or `ratatui` styling |
| `yaml` | `serde_yaml` |
| `ignore` (gitignore) | `ignore` crate (Rust-native port) |
| `uuid` | `uuid` crate |
| `marked` | `pulldown-cmark` or `comrak` |
| `diff` | `similar` crate |
| `typebox` (runtime types) | `serde` + derive (compile-time only) |
| Dynamic TypeScript extensions | WASM plugins via `wasmtime` (deferred) |

### 7.2 LLM Provider implementation pattern

TypeScript exports a `stream()` function returning an async generator of events. Rust uses a trait-based approach:

```rust
// cortexcode-ai-types defines the stream interface
pub trait AssistantMessageEventStream: Send {
    fn next_event(&mut self) -> Option<AssistantMessageEvent>;
    fn result(&mut self) -> AssistantMessage;
}

// cortexcode-ai-stream provides a channel-backed implementation
pub struct AiMessageEventStream { /* mpsc channel */ }

// Each provider crate exports a stream() function
// cortexcode-ai-provider-anthropic:
pub async fn stream(
    model: &Model,
    context: &Context,
    options: &SimpleStreamOptions,
) -> Result<Box<dyn AssistantMessageEventStream>>;
```

Provider registration is lazy (via `OnceCell`), mirroring `register-builtins.ts`:

```rust
static PROVIDER_REGISTRY: Lazy<Mutex<HashMap<String, Box<dyn ProviderFactory>>>> = Lazy::new(|| {
    let mut m = HashMap::new();
    m.insert("anthropic".into(), Box::new(AnthropicProvider));
    // ...
    Mutex::new(m)
});
```

### 7.3 Dynamic vs static dispatch

| Area | TypeScript | Rust |
|---|---|---|
| Provider registry | `register-builtins.ts` — lazy static imports | `HashMap<&'static str, Box<dyn ProviderFactory>>` via `OnceCell` |
| Tool registry | `TOOL_FACTORIES` table in `tools/index.ts` | `HashMap<&'static str, Box<dyn ToolFactory>>` |
| MCP tools | Dynamic shape, `Record<string, unknown>` | `dyn Tool` trait, `Box<dyn Any>` for unknown shapes |
| Agent loop callbacks | Callback functions in `AgentOptions` | `Box<dyn Fn(...)>` in `AgentLoopConfig` |

### 7.4 Key implementation decisions

| Decision | Choice | Rationale |
|---|---|---|
| TUI framework | **`ratatui`** + `crossterm` | Mature, maintained; bespoke renderer unnecessary |
| Native search (fd/rg) | **Pure-Rust fallback** (`grep` + `ignore` + `walkdir`) | No binary downloads; cross-platform by default |
| Async runtime | **`tokio`** (multi-threaded) | Industry standard; matches reqwest, async I/O |
| Serialization | **`serde`** + `serde_json` + `serde_yaml` | De facto Rust standard |
| HTTP client | **`reqwest`** | TLS, streaming, proxy support built in |
| Embedded templates | **`include_str!`** at compile time | No `build.rs` needed for static content |
| Generated models | **`build.rs`** (gated behind `CORTEX_UPDATE_MODELS=1`) | Mirrors TS `scripts/generate-models.ts` |
| Plugin system | **Deferred** (WASM in post-v1) | High complexity, TS extensions not portable |
| Binary name | **`cortex`** | Short, memorable, available |
| Cross-compilation | **GitHub Actions matrix** — 4 targets | Native `rustc` cross-compilation |

---

## 8. CI / CD Pipeline

### 8.1 CI Workflow

**GitHub Actions:** `.github/workflows/ci.yml`

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: cargo fmt
        run: cargo fmt --all -- --check
      - name: cargo clippy
        run: cargo clippy --workspace --all-targets -- -D warnings
      - name: cargo check
        run: cargo check --workspace
      - name: cargo test
        run: cargo test --workspace
      - name: cargo doc
        run: cargo doc --workspace --no-deps
```

**Triggered on:** push/PR to `main`.  
**Steps:** `fmt` → `clippy` → `check` → `test` → `doc`.  
**Concurrency:** automatic (no explicit concurrency needed since there's no per-package matrix yet — all crates build together via workspace).

**Comparison with pycortex CI:**

| Step | cortexcode (Rust) | pycortex (Python) |
|---|---|---|
| Toolchain setup | `dtolnay/rust-toolchain@stable` | `astral-sh/setup-uv@v5` (3.11) |
| Dep install | implicit via `cargo check` | `uv sync --all-packages` |
| Lint | `cargo clippy` | `ruff check` + `ruff format --check` |
| Type check | `cargo check` (compiler) | `pyright` |
| Test | `cargo test --workspace` | `pytest` per-package matrix |
| Doc build | `cargo doc --workspace --no-deps` | (not available) |

**Future enhancement:** Add a per-namespace test matrix (similar to pycortex's `matrix: [tui, ai, agent, code]`) once the workspace has substantial code in all four namespaces.

### 8.2 PR Merge-Release Workflow

**GitHub Actions:** `.github/workflows/merge-release.yml`

```yaml
name: Merge Release

on:
  pull_request:
    types: [closed]
    branches: [main]

jobs:
  level:
    if: github.event.pull_request.merged == true
    runs-on: ubuntu-latest
    outputs:
      level: ${{ steps.pick.outputs.level }}
    steps:
      - id: pick
        run: |
          labels='${{ toJson(github.event.pull_request.labels.*.name) }}'
          level=""
          for l in major minor patch; do
            echo "$labels" | grep -q "\"rust:$l\"" && { level="$l"; break; }
          done
          echo "level=$level" >> "$GITHUB_OUTPUT"

  release:
    needs: level
    if: needs.level.outputs.level != ''
    uses: ./.github/workflows/release.yml
    with:
      level: ${{ needs.level.outputs.level }}
    secrets: inherit
```

**Labels:** `rust:patch` · `rust:minor` · `rust:major`

**Comparison with pycortex labels:** `rust:*` in cortexcode ↔ `pypi:*` in pycortex ↔ `npm:*` in hoocode.

### 8.3 Manual Release Workflow

**GitHub Actions:** `.github/workflows/release.yml`

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      level:
        description: "Version bump level"
        required: true
        type: choice
        options: [patch, minor, major]
  workflow_call:
    inputs:
      level:
        required: true
        type: string

jobs:
  release:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          token: ${{ secrets.GITHUB_TOKEN }}
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install cargo-edit
        run: cargo install cargo-edit --locked || true
      - name: Gates
        run: |
          cargo fmt --all -- --check
          cargo clippy --workspace --all-targets -- -D warnings
          cargo test --workspace
      - name: Bump versions
        id: bump
        run: |
          version=$(python3 scripts/bump_versions.py "${{ inputs.level }}" | tail -1)
          echo "version=$version" >> "$GITHUB_OUTPUT"
      - name: Commit, tag, push
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add -A
          git commit -m "Release v${{ steps.bump.outputs.version }}"
          git tag "v${{ steps.bump.outputs.version }}"
          git push origin HEAD:main "v${{ steps.bump.outputs.version }}"
      - name: Log in to crates.io
        run: cargo login "$CRATES_IO_TOKEN"
        env:
          CRATES_IO_TOKEN: ${{ secrets.CRATES_IO_TOKEN }}
      - name: Publish to crates.io
        run: python3 scripts/publish_packages.py
        env:
          CRATES_IO_TOKEN: ${{ secrets.CRATES_IO_TOKEN }}
      - name: GitHub release
        run: gh release create "v${{ steps.bump.outputs.version }}" --generate-notes
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

**Pipeline steps:**

1. **Gates** — run `cargo fmt`, `cargo clippy`, `cargo test`; fail on any warning/error.
2. **Bump** — `scripts/bump_versions.py` rewrites workspace `version = "X.Y.Z"` in root `Cargo.toml`. All crates inherit via `version.workspace = true`.
3. **Commit + tag + push** — `git commit -m "Release vX.Y.Z"`, `git tag vX.Y.Z`, `git push`.
4. **Publish** — `scripts/publish_packages.py` publishes every crate with `[package.metadata.cortex] publish = true` in dependency order (leaves first). Skips versions already on crates.io (idempotent).
5. **GitHub Release** — `gh release create` with auto-generated release notes.

### 8.4 Cargo Publish

#### 8.4.1 Publishing Order

Crates are published in topological dependency order (leaves first, umbrellas last). This is enforced by `scripts/publish_packages.py`:

```
1. cortexcode-ai-types         (no workspace deps)
2. cortexcode-ai-stream        (depends on ai-types)
3. cortexcode-ai-models        (depends on ai-types)
4. cortexcode-ai-env           (no workspace deps)
5. cortexcode-ai-util          (no workspace deps)
6. cortexcode-ai-provider-faux (depends on stream, types, models)
   ... (other providers)
7. cortexcode-ai-oauth         (depends on types)
8. cortexcode-ai-images        (depends on types)
9. cortexcode-ai               (umbrella — depends on all AI leaves)
10. cortexcode-tui-util        (no workspace deps)
11. cortexcode-tui-fuzzy       (no workspace deps)
12. cortexcode-tui-keys        (no workspace deps)
13. cortexcode-tui-terminal    (no workspace deps)
14. cortexcode-tui-render      (depends on util, terminal)
15. cortexcode-tui-editing     (depends on render, keys)
16. cortexcode-tui-components  (depends on editing, fuzzy, keys)
17. cortexcode-tui-images      (depends on components)
18. cortexcode-tui             (umbrella — depends on all TUI leaves)
19. cortexcode-agent-types     (depends on ai-types)
20. cortexcode-agent-core      (depends on agent-types, ai-types)
21. cortexcode-agent-loop      (depends on agent-types)
    ... (remaining agent leaves)
22. cortexcode-agent           (umbrella)
    ... (code leaves — published later, once stable)
23. cortexcode-code            (umbrella)
24. cortexcode                 (top umbrella — depends on all namespace umbrellas)
```

#### 8.4.2 Lockstep Versioning

All crates share the **same version** defined in the workspace `Cargo.toml`:

```toml
[workspace.package]
version = "0.0.1"
```

Each leaf inherits it via `version.workspace = true`. Bumping is a single operation:

```bash
python3 scripts/bump_versions.py patch  # 0.0.1 → 0.0.2
python3 scripts/bump_versions.py minor  # 0.0.1 → 0.1.0
python3 scripts/bump_versions.py major  # 0.0.1 → 1.0.0
```

#### 8.4.3 `scripts/bump_versions.py`

Scans the workspace `Cargo.toml`, finds the single `version = "..."` field in the `[workspace.package]` section, and replaces it with the bumped version. All member crates that use `version.workspace = true` automatically pick up the new value. No per-crate edits needed.

**Key difference from pycortex:** The Rust version is simpler because member crates inherit the workspace version rather than each carrying its own `version = "..."` line. pycortex's `bump_versions.py` must edit every `pyproject.toml` individually and re-pin sibling dependency ranges.

#### 8.4.4 `scripts/publish_packages.py`

Follows the same pattern as pycortex's `publish_packages.py`:

1. Parse all `crates/*/Cargo.toml`.
2. Resolve the dependency graph to determine topological order.
3. Check crates.io for each crate's name+version (skip if already published — idempotent).
4. Run `cargo publish -p <crate>` in topological order.
5. Apply rate-limiting delays between publishes (crates.io throttles new project creation).
6. Retry on 429 errors with `Retry-After` backoff.

#### 8.4.5 Crate publishability gate

A crate is only published to crates.io when:

1. All migration steps for its namespace phase are checked off.
2. Test coverage of ported modules ≥ the TS originals' test surface.
3. `cargo clippy` passes with `-D warnings`.
4. Its public API is documented.

**Control flag:** `[package.metadata.cortex] publish = true|false` in each crate's `Cargo.toml`. Volatile crates are `false` until they reach publishable status.

#### 8.4.6 Crate name reservation

The one-off `.github/workflows/reserve-names.yml` workflow publishes `0.0.1` placeholder crates for all 47 workspace members to reserve names on crates.io. Safe to re-run: already-published versions are skipped.

---

## 9. Migration Plan

### Phase 0 — Workspace bootstrap (✓ Complete)

- [x] **0.1 Root workspace** — `Cargo.toml` with all 47+ members, shared `[workspace.package]`
- [x] **0.2 CI** — `.github/workflows/ci.yml` — fmt + clippy + check + test + doc
- [x] **0.3 Reserve crates.io names** — `.github/workflows/reserve-names.yml` publishes 0.0.1 placeholders
- [x] **0.4 Release plumbing** — `scripts/bump_versions.py`, `scripts/publish_packages.py`, `release.yml`, `merge-release.yml`
- [x] **0.5 Crate scaffolding** — All 47 crate directories with `Cargo.toml` and placeholder `lib.rs`
- [x] **0.6 Core types** — `cortexcode-ai-types` (full types implementation), `cortexcode-agent-types` (full types + Agent struct)

### Phase 1 — AI Namespace (T0/T1)

- [x] **1.1 cortexcode-ai-stream** — Channel-backed `AssistantMessageEventStream` — **DONE**
- [x] **1.2 cortexcode-ai-env** — API key detection from environment variables — **DONE**
- [x] **1.3 cortexcode-ai-models** — Model registry + generated model lists — **DONE**
- [x] **1.4 cortexcode-ai-util** — JSON repair, validation, hash, header utilities — **DONE**
- [x] **1.5 cortexcode-ai-provider-faux** — Test provider (port `faux.ts`) — **DONE**
- [x] **1.6 cortexcode-ai-provider-anthropic** — Anthropic streaming provider — **DONE**
- [x] **1.7 cortexcode-ai-provider-openai** — OpenAI Chat Completions provider — **DONE** (Responses/Codex APIs not yet ported)
- [x] **1.8 cortexcode-ai-provider-google** — Google Gemini + Vertex providers — **DONE** (Vertex ADC/service-account auth deferred; API-key/access-token auth only)
- [x] **1.9 cortexcode-ai-provider-azure** — Azure OpenAI Responses provider — **DONE** (cross-provider reasoning-item ID pairing not ported)
- [x] **1.10 cortexcode-ai-oauth** — OAuth flow support — **DONE** (Anthropic PKCE + GitHub Copilot device flow; interactive browser/callback-server wiring deferred to CLI layer)
- [x] **1.11 cortexcode-ai-images** — Image generation support — **DONE** (OpenRouter provider)
- [x] **1.12 cortexcode-ai umbrella publishable** — Flip all T0/T1 leaves to `publish = true` — **DONE** (all AI leaves already had `publish = true` from scaffolding; wired the umbrella's `lib.rs` to actually re-export every leaf)

### Phase 2 — TUI Namespace (T0)

- [x] **2.1 cortexcode-tui-util** — ANSI width, grapheme handling, truncate/wrap — **DONE**
- [x] **2.2 cortexcode-tui-fuzzy** — Fuzzy matching — **DONE**
- [x] **2.3 cortexcode-tui-keys** — Key parsing + keybindings — **DONE** (global keybindings singleton not ported)
- [x] **2.4 cortexcode-tui-terminal** — Terminal abstraction (raw mode, stdin buffer) — **DONE** (Windows `ENABLE_VIRTUAL_TERMINAL_INPUT` koffi tweak deferred)
- [x] **2.5 cortexcode-tui-render** — Differential renderer — **DONE** (flatten memoization and 16ms render-coalescing are performance-only optimizations, not ported; differential terminal output is behaviorally equivalent)
- [ ] **2.6 cortexcode-tui-editing** — Text editor, kill ring, undo stack
- [ ] **2.7 cortexcode-tui-components** — Box, text, markdown, select-list, autocomplete, etc.
- [x] **2.8 cortexcode-tui-images** — Terminal image rendering — **DONE** (ported ahead of 2.5/2.6/2.7 since the renderer depends on it)
- [ ] **2.9 cortexcode-tui umbrella publishable** — All T0 leaves `publish = true`

### Phase 3 — Agent Namespace (T0/T1)

- [ ] **3.1 cortexcode-agent-core** — Agent struct, orchestration, state management
- [ ] **3.2 cortexcode-agent-loop** — Turn loop, tool dispatch, background tools
- [ ] **3.3 cortexcode-agent-harness** — Message conversion, system prompt, prompt templates
- [ ] **3.4 cortexcode-agent-session** — Session persistence, file management
- [ ] **3.5 cortexcode-agent-compaction** — Context window compaction, summarization
- [ ] **3.6 cortexcode-agent-tools** — Tool registry / factory pattern
- [ ] **3.7 cortexcode-agent-mcp** — MCP transport, tool discovery
- [ ] **3.8 cortexcode-agent umbrella publishable** — T0/T1 leaves `publish = true`

### Phase 4 — Code Namespace Core (T2)

- [ ] **4.1 cortexcode-code-config** — Settings load/merge/persist, config paths
- [ ] **4.2 cortexcode-code-tools** — `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`
- [ ] **4.3 cortexcode-code-session** — Session CRUD, directory layout, lifecycle
- [ ] **4.4 cortexcode-code-prompts** — System prompt assembly, mode prompts
- [ ] **4.5 cortexcode-code-print** — Non-interactive print mode
- [ ] **4.6 cortexcode-code-main** — CLI entry point (`cortex` binary), arg parsing

### Phase 5 — Code Namespace Full (T3)

- [ ] **5.1 cortexcode-code-rpc** — JSON-RPC mode
- [ ] **5.2 cortexcode-code-subagents** — Subagent pool, Task tool, IPC
- [ ] **5.3 cortexcode-code-resources** — Resource loading, skills, context files
- [ ] **5.4 cortexcode-code-extensions** — WASM plugin API (design + initial implementation)
- [ ] **5.5 Interactive mode** — TUI-based interactive mode wiring
- [ ] **5.6 cortexcode-code umbrella publishable** — All leaves `publish = true`

### Phase 6 — Integrate and Release

- [ ] **6.1 cortexcode top umbrella** — Re-export all namespace umbrellas
- [ ] **6.2 Parity checklist** — Run hoocode and cortex side-by-side on scripted scenarios
- [ ] **6.3 Binary distribution** — Cross-platform CI builds (4 targets)
- [ ] **6.4 First public release** — `cargo publish` train, GitHub Release with binaries
- [ ] **6.5 Documentation** — README, install guide, migration guide from hoocode

---

## 10. Edge Cases and Risk Mitigation

### 10.1 Async tool execution (background tools)

TypeScript runs long tools (bash, webfetch) as background tasks while the agent continues. Rust uses `tokio::spawn` with `oneshot` channels for results:

```rust
let (tx, rx) = tokio::sync::oneshot::channel();
tokio::spawn(async move {
    let result = tool.execute(request).await;
    let _ = tx.send(result);
});
// Store rx; collect when agent loop needs the result
```

### 10.2 Process management (bash tool)

- Use `tokio::process::Command` with `kill_on_drop(true)`.
- Process group management prevents orphan children on agent abort.
- Configurable timeout matches TypeScript's `bashTimeout` setting.

### 10.3 Permission gate (Yes/No/Always)

- **Interactive mode:** `ratatui` dialog widget. Agent awaits user input.
- **Print mode:** Auto-grant (same as `-p` flag).
- **RPC mode:** Part of JSON-RPC protocol.
- Persistent decisions stored in a `HashMap<ToolCallHash, Permission>` for the session.

### 10.4 Subagent pool (Task tool)

- Spawn child processes running `cortex` binary with `--mode subagent`.
- Communicate via stdin/stdout JSON-RPC.
- Concurrency limits and timeout controls matching TypeScript's `subagent-pool.ts`.

### 10.5 File locking (settings, sessions)

- Use `fs2` crate for cross-process safety (file locks via `flock` / `LockFile`).
- Fall back to `tokio::sync::RwLock` for single-process scenarios.

### 10.6 Data migration (from hoocode)

- Read `~/.hoocode/settings.json` for backward compatibility during transition.
- Write to `~/.cortexcode/` going forward.
- Auto-migrate settings on first run.

### 10.7 Plugin / extension system

Porting TypeScript's dynamic extension system (`jiti`-based) to Rust is the highest-risk item:

- **v1:** No plugin system — all built-in tools compiled into the binary.
- **v2:** WASM plugins via `wasmtime` runtime — portable, sandboxed, no ABI stability issues.
- **v3:** Full plugin SDK with the same capabilities as the TypeScript version.

---

## Appendix: Design Document Structure (aligned with pycortex)

This document follows the same structure as the pycortex migration design docs:

| Doc | pycortex | cortexcode |
|---|---|---|
| Overview | `00-migration-overview.md` | §1–2 (this document) |
| Source Analysis | `01-source-analysis.md` | §3 |
| Target Architecture | `02-target-architecture.md` | §4–6 |
| Release Pipeline | `03-release-pipeline.md` | §8 |
| Migration Plan | `04-migration-plan.md` | §9 |
| Skills & Commands | `05-skills-and-commands.md` | (future — `scripts/migrate_next.py` TBD) |

---

## References

- **HooCode (TypeScript):** https://github.com/kolisachint/hoocode — the source being ported
- **pycortex (Python):** https://github.com/kolisachint/pycortex — ultramodular Python migration (same architecture)
- **cortexcode (Rust):** https://github.com/kolisachint/cortexcode — this repo, the target
- **Upstream pi-mono:** Mario Zechner (@badlogicgames, @earendil-works) — original project

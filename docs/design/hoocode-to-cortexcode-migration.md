# HooCode (TypeScript) → CortexCode (Rust) Migration Design Document

> **Status:** RE-BASELINED 2026-09-24 — audited against the pinned source; Phases 7–13 added (see §0 and §9)  
> **Author:** Sachin Koli  
> **Source Repo:** https://github.com/kolisachint/hoocode (TypeScript monorepo)  
> **Source Pin:** hoocode **v0.5.89**, commit [`a6cd96e73c23c897ced685b6bb97b4a0dd61b65a`](https://github.com/kolisachint/hoocode/tree/a6cd96e73c23c897ced685b6bb97b4a0dd61b65a) (2026-09-24) — see §0  
> **Target Repo:** https://github.com/kolisachint/cortexcode (Rust workspace, this repo)  
> **Migration Reference:** https://github.com/kolisachint/pycortex (Python ultramodular migration)  

---

## Table of Contents

0. [Source pin and audit summary](#0-source-pin-and-audit-summary)
1. [Scope and Goals](#1-scope-and-goals)
2. [Principles](#2-principles)
3. [Source Analysis — HooCode (TypeScript)](#3-source-analysis--hoocode-typescript)
4. [Target Architecture — Multi-Level Modular Crate Structure](#4-target-architecture--multi-level-modular-crate-structure)
5. [Ultramodular Leaf Crate Map](#5-ultramodular-leaf-crate-map)
    - [5.1 AI Namespace](#51-ai-namespace)
    - [5.2 Agent Namespace](#52-agent-namespace)
    - [5.3 Code Namespace](#53-code-namespace)
    - [5.4 TUI Namespace](#54-tui-namespace)
    - [5.5 Revised split: volatility tiers, dependency firewall](#55-revised-split-2026-09-24-churn-driven-crates-volatility-tiers-dependency-firewall)
    - [5.6 Migration operations: ledger, two-level done, resuming](#56-migration-operations-ledger-two-level-done-resuming)
6. [Dependency Graph](#6-dependency-graph)
7. [Key Migration Decisions: TypeScript → Rust](#7-key-migration-decisions-typescript--rust)
8. [CI / CD Pipeline](#8-ci--cd-pipeline)
    - [8.1 CI Workflow](#81-ci-workflow)
    - [8.2 PR Merge-Release Workflow](#82-pr-merge-release-workflow)
    - [8.3 Manual Release Workflow](#83-manual-release-workflow)
    - [8.4 Cargo Publish](#84-cargo-publish)
9. [Migration Plan](#9-migration-plan)
10. [Edge Cases and Risk Mitigation](#10-edge-cases-and-risk-mitigation)
11. [Parity checklist](#11-parity-checklist)

---

## 0. Source pin and audit summary

### 0.1 Pinned source version

All porting work, parity fixtures and "done" criteria in this document refer to one
immutable hoocode snapshot:

| Field | Value |
|---|---|
| hoocode version | `0.5.89` (all four npm packages, lockstep) |
| hoocode commit | `a6cd96e73c23c897ced685b6bb97b4a0dd61b65a` |
| Commit date | 2026-09-24 |
| Machine-readable copy | `[workspace.metadata.cortex.source]` (`hoocode-version`, `hoocode-commit`) in the root `Cargo.toml` |

**Pin policy.**

1. hoocode keeps shipping, so we don't chase `main`. Port from the pinned commit only:
   `git -C ../hoocode checkout a6cd96e7`.
2. Moving the pin is a deliberate PR. It updates `Cargo.toml` metadata and this header,
   and adds a "delta" checklist built from
   `git diff <old-pin>..<new-pin> -- packages/` (the files that changed, mapped to crates).
3. Golden fixtures (Phase 13) are recorded from the pinned commit and carry the pin in
   their file header. Fixtures from a different pin fail the check.
4. The previous revision of this plan targeted **0.4.146**. Everything marked done in
   Phases 0–6 was ported against 0.4.x, so it needs re-verification against 0.5.89
   (see §0.2).

### 0.2 Audit of the Phase 0–6 "complete" claim (2026-09-24)

`cargo test --workspace` is green (645 tests, 0 failures) and the crate skeleton is
sound. However, the earlier status documents (`MIGRATION_STATUS.md`,
`MIGRATION_COMPLETE*.md`) claim "100% / feature parity", and that does not hold. What
the audit found:

| # | Finding | Evidence | Severity |
|---|---|---|---|
| A1 | **Wire formats don't match hoocode.** `ai-types`/`agent-types` have no serde attributes, so messages serialize as `{"inner":{"Standard":{"User":{…}}}}` with snake_case fields. hoocode uses `{"role":"user","content":[{"type":"text",…}]}`, camelCase. Session entries use `#[serde(rename_all)]` on the enum, which renames variants only, so fields come out as `parent_id` instead of `parentId`. | A probe that parses a real hoocode `message` session line fails with `missing field 'inner'`. | **Blocker.** Hoocode sessions can't be loaded, and the RPC and JSON print formats diverge. |
| A2 | **The app doesn't use most crates.** `cortexcode-code-main` doesn't depend on `tui-*`, `code-session`, `agent-mcp`, `agent-compaction`, `code-subagents` or `code-extensions`. Interactive mode is a 90-line crossterm `readline` loop. | `crates/cortexcode-code-main/Cargo.toml`, `runtime.rs::run_interactive_mode` | **Blocker** |
| A3 | **The whole stack is synchronous.** It uses `reqwest::blocking` and `std::sync::mpsc`, and only one crate depends on tokio. §7.4 said tokio. Streaming can't be cancelled mid-read, and steering, follow-ups, parallel tools and async-only libraries like `rmcp` all need an async core. | `grep reqwest crates/*/Cargo.toml` | High |
| A4 | **Tool set follows 0.4.x.** hoocode 0.5.89's default bundle is `read, bash, edit, write, SearchCodebase` plus opt-in `webfetch`, `websearch`, `todo`, `subagent`, `canvas` and plugin tools. It no longer ships `grep`/`find`/`ls`. The Rust tools are thin wrappers: no truncation, image reads, read dedup, process-group kill, edit-diff/fuzzy matching or mutation queue, and no `.gitignore` handling. | `packages/coding-agent/src/core/tools/index.ts` vs `crates/cortexcode-code-tools/src/lib.rs` (940 LOC Rust vs ~8K LOC TS) | High |
| A5 | **Provider/API coverage is partial.** hoocode dispatches on 8 `Api`s and 31 known providers. Rust dispatches on a hard-coded provider name (`anthropic/openai/opencode/google`). `openai-responses`, `openai-codex-responses` and `google-gemini-cli` are missing, as is compat-driven routing for the ~25 OpenAI-compatible providers. `AssistantMessage` lacks `api/provider/model/responseId`, and `Model` lacks `compat`. Rust bundles 907 models against hoocode's 1224. | `crates/cortexcode-code-main/src/runtime.rs::make_stream_fn`, `packages/ai/src/types.ts` | High |
| A6 | **Agent loop parity is untested.** `agent-core` and `agent-loop` have 0 tests. hoocode's `agent.test.ts`/`agent-loop.test.ts` are not ported. | `grep -c '#\[test\]'` | High |
| A7 | **The "parity test" only checks `--help`.** `scripts/parity_test.sh` checks that flags appear in the help text. It never runs hoocode and cortex side by side. | `scripts/parity_test.sh` | Medium (process) |
| A8 | **Two parallel session stacks.** `agent-session` (`FileSessionStore`) and `code-session` (JSONL tree) overlap, and neither is wired into the CLI. | crate sources | Medium |
| A9 | **Four copies of an SSE parser, one per provider, plus a 1.8K-LOC hand-written MCP transport.** Maintained ecosystem crates (`eventsource-stream`, `rmcp`) already cover these. | `crates/*/src/sse.rs`, `agent-mcp/src/transport.rs` | Medium (maintenance) |

**Size check at the pin** (non-generated TypeScript `src/` LOC vs. Rust LOC today):

| Namespace | hoocode 0.5.89 | cortexcode | Rough coverage |
|---|---|---|---|
| tui | 13.9K | 14.6K (tui-*) | ~90%. Faithful port, but **not wired into the app** |
| ai | 14.1K (+22.2K generated models) | 9.2K | ~55% |
| agent | 9.2K | 5.0K | ~45% |
| coding-agent | 95.7K | 7.3K (code-*) | **~10%** |

**Conclusion.** The TUI namespace is the most complete. AI is roughly half done.
Agent and coding-agent are early. The honest overall figure is **about 25–30% of
pinned hoocode behavior**, not 100%. Phases 7–13 (§9) are the gap-closure plan.
Phase 7 (wire formats plus the async core) must land first, because every later
phase builds on those types.

> The following status files predated this audit and overstated completion:
> `MIGRATION_STATUS.md`, `MIGRATION_COMPLETE.md`, `MIGRATION_COMPLETE_FINAL.md`,
> `MIGRATION_FINAL_SESSION_SUMMARY.md`, `MIGRATION_SESSION_SUMMARY.md`,
> `MIGRATION_VIM_JUMP_COMPLETE.md`. **This document is the single source of truth.**
> Those files were removed in task 7.0, and their history is summarized in `CHANGELOG.md`.

---

## 1. Scope and Goals

### What

Rewrite [HooCode](https://github.com/kolisachint/hoocode) — a deterministic terminal coding agent written as a TypeScript npm monorepo (4 packages, ~133K hand-written LOC plus ~22K generated at the v0.5.89 pin) — into an **ultramodular Rust workspace** of ~47 small, independently versioned crates published on [crates.io](https://crates.io).

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
8. **Volatility isolation.** *(Added 2026-09-24.)* Code that churns upstream, or that tracks a fast-moving vendor API or third-party crate, lives in its own small crate behind cortexcode-owned types (§5.5). A dependency firewall enforces this.
9. **Two-level done.** *(Added 2026-09-24.)* Programmatic tests **and** a rendered-TUI comparison against the pinned hoocode (§5.6).

---

## 3. Source Analysis — HooCode (TypeScript)

### 3.1 Package inventory

Four npm packages, lockstep-versioned at **0.5.89** (pinned, §0.1). The previous plan revision measured 0.4.146.

| Package | npm name | LOC (src) | TS test files | Responsibility |
|---|---|---|---|---|
| `packages/tui` | `@kolisachint/hoocode-tui` | ~13,900 | 36 | Terminal UI library: differential renderer, components, keybindings, mouse, frame |
| `packages/ai` | `@kolisachint/hoocode-ai` | ~36,300 (22.2K generated models) | 68 | Unified LLM API: 8 APIs, 31 known providers, streaming, OAuth (5 flows), images |
| `packages/agent` | `@kolisachint/hoocode-agent-core` | ~9,200 | 17 | Agent loop, agent harness, compaction + branch summarization, session repo, MCP (+OAuth), proxy |
| `packages/coding-agent` | `@kolisachint/hoocode-agent` | ~95,700 | 279 | The `hoocode` CLI: AgentSession, tools, settings, modes, extensions/plugins, subagents, interactive TUI |

#### 3.1.1 coding-agent feature inventory at the pin (for triage)

| Area | Main TS files (LOC) | Target phase |
|---|---|---|
| AgentSession orchestrator | `core/agent-session*.ts` (~4.3K) | 10.3 |
| Settings / config / auth | `core/settings-*.ts`, `config.ts`, `core/auth-storage.ts` (~2.9K) | 10.1, 10.4 |
| Model registry / resolver | `core/model-registry.ts`, `core/model-resolver.ts` (~1.5K) | 10.4 |
| Session manager (JSONL tree) | `core/session-manager.ts` (1.4K) | 7.2, 10.3 |
| Built-in tools | `core/tools/*` (~8K) | 10.2 |
| Resources: skills, prompt templates, context files, slash commands, modes | `core/{resource-loader,skills,prompt-templates,context-files,slash-commands,mode-prompts}.ts`, `extensions/core/modes.ts` (~3.7K) | 10.5 |
| Built-in core extension (permission gate, MCP loader, modes, cost, thinking escalation, ask-options) | `extensions/core/*` (~5K) | 10.6, 9.1, 10.5 |
| Print / RPC modes | `modes/print-mode.ts`, `modes/rpc/*` (~1.8K) | 10.8 |
| CLI args / main | `cli/*`, `main.ts` (~2.5K) | 10.7 |
| Subagents (pool, tool, lifeguard, depth, inbox, warm pool) | `core/subagent*.ts`, `core/lifeguard.ts`, `core/tools/subagent.ts` (~3.7K) | 10.9 (warm pool → 12) |
| Interactive TUI mode | `modes/interactive/**` (~20K incl. 45 components, theme 1.9K) | 11 |
| Extension runtime (TS, jiti) + plugin marketplace/packaging | `core/extensions/**`, `core/package-manager.ts` (~9K) | 12 |
| Semantic index / hybrid search / capability retrieval | `core/embsearch`, `core/search`, `core/capabilities` (~3K) | 10.2 (lexical), 12 (dense) |
| Scheduler/`/loop`, learn, canvas, teams, voice, export-html, telemetry | various (~10K) | 12 |

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

Dependency lists come from the `package.json` files at the pin. **Status** says what the
workspace uses today. "Adopt" means switch to the named crate in the listed phase
rather than hand-writing or keeping a hand-written version.

| TS dep (pinned) | Rust crate (latest checked 2026-09-24) | Status today | Action |
|---|---|---|---|
| `typebox` (schemas + validation) | `serde` derive + **`jsonschema` 0.57** for tool-arg validation | serde only; no validation | Adopt `jsonschema` (8.5) |
| `partial-json` | **`partial-json-fixer` 0.5** | hand-rolled in `ai-util` | Adopt if it passes the ported `validation`/partial tests; keep ours otherwise (8.5) |
| `@anthropic-ai/sdk`, `openai`, `@google/genai` | `reqwest` 0.12 (async, rustls) + **`eventsource-stream` 0.2** | `reqwest::blocking` + 4 hand-written `sse.rs` | Switch to async and share one SSE decoder (7.3) |
| `undici` | `reqwest` | ✓ | — |
| `@modelcontextprotocol/sdk` (stdio, SSE, streamable HTTP, OAuth) | **`rmcp` 3.4** (official SDK; `client`, `transport-child-process`, `transport-streamable-http-client`, `auth`) | 1.8K-LOC hand-written transport, no OAuth | Replace the transport with `rmcp`; keep our `mcp.json` loader and tool adapter (9.1) |
| `ignore` | **`ignore` 0.4** (ripgrep) | not used (`walkdir`, so `.gitignore` is ignored) | Adopt (10.2, 10.5) |
| `glob`, `minimatch` | **`globset` 0.4** (+ `ignore::overrides`) | `glob` | Adopt `globset` (10.2) |
| downloaded `rg`/`fd` + `native-search.ts` fallback | **`grep-searcher` + `grep-regex` + `ignore`** (ripgrep as a library) | naive regex over `walkdir` | Adopt: no binary downloads (10.2) |
| `diff` | **`similar` 3.x** (`TextDiff`, unified diff) | not used | Adopt for edit-diff and the diff component (10.2, 11.2) |
| `cli-highlight` | **`syntect` 5.3** + **`two-face`** (bat's syntax/theme set) | none | Adopt (11.2) |
| `marked` | `pulldown-cmark` | ✓ | — |
| `chalk`, `strip-ansi`, `get-east-asian-width` | own `tui-util` (ANSI-aware width/wrap) + `unicode-width` | ✓ (ported) | Keep. The semantics are tied to the renderer |
| `@silvia-odwyer/photon-node`, `file-type`, EXIF | **`image` 0.25** (resize, formats, EXIF orientation via `ImageDecoder::orientation`) + `infer` | none | Adopt (10.2 read-image, 11.4) |
| `@mariozechner/clipboard` | **`arboard` 3.6** (text + image) | none | Adopt (11.4) |
| `yaml` (frontmatter, agents, skills) | **`serde_yaml_ng` 0.10** (`serde_yaml` is unmaintained) | none | Adopt (10.5) |
| `uuid` | `uuid` (v7 for session ids, matching TS) | ✓ | — |
| `proper-lockfile` | **`fs4` 1.x** (advisory file locks) | none | Adopt for settings/auth/session writes (10.1) |
| atomic writes (`utils/atomic-file.ts`) | **`tempfile`** `NamedTempFile::persist` | none | Adopt (10.1) |
| `extract-zip`, `hosted-git-info` | `zip`, own parser | none | Phase 12 (package manager) |
| `jiti` (TS extensions) | none. Needs a redesign (§10.7) | `wasmtime` prototype | Phase 12 |
| `@anthropic-ai/sandbox-runtime` (example extension) | none. Out of scope | — | — |
| CLI parsing (`cli/args.ts`) | `clap` 4 derive (rejected in 10.7a) | exact port of the hand-written parser | Keep own. The pinned grammar has multi-letter short aliases (`-nt`, `-nsc`), captures unknown `--flags` with a greedy value as extension flags, lets `-p` take the next arg as the prompt, and silently drops invalid values. `clap` would change observable behavior (10.7a) |
| OAuth PKCE + loopback server | keep own (`ai-oauth`) + **`open` 5** for browser launch | own + ad-hoc `open`/`xdg-open` | Adopt `open`; `oauth2` isn't worth it (provider quirks) |
| cron (`core/scheduler.ts`) | **`croner` 4** | none | Phase 12 |
| BM25 (capability retrieval) | **`bm25` 2.x** | none | Phase 12 |
| fs watch (`utils/fs-watch.ts`) | `notify` 8 (9 is still RC) | none | Phase 11/12 |
| process groups / kill tree (bash tool) | **`process-wrap` 10** (`ProcessGroup`/`JobObject`, tokio feature) | `std::process` with no group | Adopt (10.2) |
| HTML → text for `webfetch` | **`htmd`** (turndown port) + **`dom_smoothie`** (readability) | raw body | Adopt (10.2) |
| Biome + tsgo / vitest | `cargo fmt`, `cargo clippy`, `cargo test` (+ `insta` snapshots for fixtures) | ✓ | Add `insta` (13) |

#### 3.4.1 Reference implementations worth reading (not dependencies)

| Project | License | What to borrow |
|---|---|---|
| `openai/codex` → `codex-rs` | Apache-2.0 | tokio + crossterm TUI event-loop structure, inserting history above an inline viewport, `exec` process handling and sandboxing (Seatbelt/Landlock), `rmcp` client usage, apply-patch parser |
| `block/goose` | Apache-2.0 | `rmcp` integration and MCP OAuth wiring, provider abstraction over many OpenAI-compatible backends |
| ripgrep (`grep-*`, `ignore`) examples | MIT/Unlicense | `SearchCodebase` lexical retriever |

Borrow patterns, not wholesale code, and keep license headers for anything copied.

#### 3.4.2 Internal reuse (already in this workspace)

| Existing code | Reuse for |
|---|---|
| `tui-render`, `tui-components` (Editor, Markdown, SelectList, SettingsList, Loader, Image), `tui-keys`, `tui-terminal` (340 tests) | Phase 11 interactive mode. **Wire them in; don't rewrite on ratatui** (§7.4) |
| Responses-API request/stream logic inside `ai-provider-azure` | Extract into a shared `openai-responses` module for `openai-responses` and `openai-codex-responses` (8.3) |
| `code-session` JSONL tree (branching, labels, fork) | The single session implementation. Fold `agent-session` into it or reduce it to a trait (7.4) |
| `ai-oauth` (Anthropic PKCE, Copilot device flow) + `code-main::auth` callback server | Base for the Codex, Gemini CLI and Antigravity OAuth flows (8.4) |
| `agent-mcp::loader` (`mcp.json` parsing, tool naming) | Keep on top of `rmcp` (9.1) |
| `ai-provider-faux` | Deterministic driver for every parity fixture (13) |

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
rust-version = "1.88"

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

> **Superseded by §5.5** (finer, churn-driven split). Kept for history.

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

### 5.5 Revised split (2026-09-24): churn-driven crates, volatility tiers, dependency firewall

> Supersedes the §5.3 code-namespace table and extends §5.1/§5.2. The TS package
> `coding-agent` (95.7K LOC) is too big and changes too unevenly for ~10 crates.
> Crates are now cut so that **code which changes often, or which follows a
> fast-moving upstream (vendor APIs, OAuth flows, model lists, volatile third-party
> crates), lives in its own small crate**. Tracking an upstream change then touches
> one crate.

**Churn data** (commits touching the file, full hoocode history to the pin, 1,481
commits). Recompute when moving the pin:
`git log --name-only --format= -- packages/*/src | sort | uniq -c | sort -rn`.

| Area (TS) | Commits | Tier |
|---|---|---|
| `modes/interactive/interactive-mode.ts` | 118 | H |
| `core/settings-{manager,types,defaults}.ts` | 139 | V |
| `main.ts` + `cli/args.ts` | 117 | V |
| `core/tools/subagent.ts` + `core/subagent-pool.ts` | 100 | V |
| `core/agent-session.ts` (+ `-services`) | 56 | V |
| `extensions/core/hoo-core.ts` + `core/extensions/**` | 190 | D |
| `core/search/**`, `core/embsearch/**` | 92 | V / D |
| `modes/interactive/{components,theme}` | ~390 | H |
| `core/tools/{read,edit,write,bash}` | ~40 | S |
| `core/session-manager.ts`, `auth-storage.ts`, `config.ts` paths | low | S |
| `packages/tui` | 114 across 27 files (~4 each) | S |
| `packages/ai/src/providers/*` | 69 | V, driven by vendor APIs |

**Tiers.** **S**table: port once, rarely touched. **V**olatile: isolated leaf crates
with a narrow API. **H**ot UI: the thinnest possible orchestration crates over stable
widget crates. **D**eferred: Phase 12, created only on a go decision.

#### Code namespace target map

| Crate | Tier | Owns (TS at pin) | Ledger |
|---|---|---|---|
| `code-paths` | S | `config.ts` dirs, env overrides, `.hoocode` fallback | 10.1 |
| `code-settings` | V | `settings-{types,defaults,manager,storage}.ts`. Typed schema plus unknown-key passthrough, so new upstream keys don't break us | 10.1 |
| `code-session` *(exists)* | S | `session-manager.ts` JSONL tree | 7.2 |
| `code-auth` | S | `auth-storage.ts`, `auth-guidance.ts` | 10.4b |
| `code-models` | V | `model-registry.ts` (`models.json`), `model-resolver.ts` | 10.4a/b |
| `code-tool-api` | S | tool definition wrapper, truncation, path utils, output accumulator, TodoWrite, ask_options | 10.2a/f |
| `code-tools-fs` | S | `read`, `write`, `edit` + `edit-diff`, read-dedup, mutation queue | 10.2a/c |
| `code-tool-bash` | S | `bash`, bash-executor, shell resolution | 10.2b |
| `code-tool-search` | V | `SearchCodebase` (lexical; ripgrep libs) | 10.2d |
| `code-tool-web` | V | `webfetch`, `websearch` (they follow external sites and services) | 10.2e |
| `code-prompts` *(exists)* | V | `system-prompt.ts` | 10.4c |
| `code-modes` | V | ask/plan/build/debug, mode prompts | 10.5b |
| `code-resources` *(exists)* | V | resource-loader, skills, prompt templates, context files, slash commands, agents | 10.5 |
| `code-permissions` | V | permission gate policy | 10.6 |
| `code-mcp` | V | MCP server discovery, deferred MCP, status (over `agent-mcp`) | 10.11 |
| `code-agent-session` | V | AgentSession orchestrator (+ runtime, services, retry, stats, tree nav) | 10.3 |
| `code-subagents` *(exists)* | V | pool, Task/TaskOutput, lifeguard, depth | 10.9 |
| `code-print` *(exists)* | S | print + json mode | 10.8a/b |
| `code-rpc` *(exists, rewrite)* | S | hoocode RPC protocol + client | 10.8c |
| `code-cli` | V | `cli/args.ts`, `main.ts` composition | 10.7a/b |
| `code-media` | V | image resize and convert, clipboard images (`image` crate) | 10.2a (sniff + resize for `read`), 11.4 |
| `code-tui-theme` | H | `theme.ts` + JSON themes | 11.1 |
| `code-tui-keybindings` | H | app keybindings | 11.1 |
| `code-tui-widgets` | H | message, tool, diff, footer, summary and task-panel components | 11.2, 11.5 |
| `code-tui-selectors` | H | model, session, tree, settings, theme, login selectors | 11.3 |
| `code-tui-app` | H | `interactive-mode.ts`, command executor (orchestration only) | 11.1–11.4 |
| `code-main` *(exists)* | S | the `cortex` bin: calls `code-cli` and nothing else | 10.7a |
| `code-config` *(exists)* | — | retired by 10.1 (re-exports during transition) | 10.1 |
| `code-tools` *(exists)* | — | retired by 10.2 (re-exports during transition) | 10.2 |
| `code-plugins`, `code-packages`, `code-extensions`, `code-capabilities`, `code-scheduler`, `code-extras` | D | Phase 12 | 12.x |

#### AI / agent / TUI additions (volatility isolation)

| Crate | Why separate | Ledger |
|---|---|---|
| `ai-models-catalog` | Generated model data changes every pin bump. `ai-models` keeps the logic | 8.1 |
| `ai-sse` | One SSE decoder (`eventsource-stream`) shared by all providers | 7.3 |
| `ai-provider-openai-responses` | Responses API shared by openai, azure and codex | 8.3 |
| `ai-provider-openai-codex`, `ai-provider-google-gemini-cli` | Subscription backends change independently | 8.4a/c |
| `ai-oauth` → core + `ai-oauth-{anthropic,github-copilot,openai-codex,google}` | Each vendor's OAuth flow changes on its own schedule | 8.7, 8.4 |
| `tui-highlight` | `syntect`/`two-face` behind our own API | 11.2 |

#### Dependency firewall

Each volatile third-party crate (fast-moving major versions, or a wrapper around a
vendor service) may be a dependency of **only its adapter crate(s)**, which expose
cortexcode-owned types. The list is in `migration/dep-firewall.json`, and
`migration/check_dep_firewall.py` enforces it in CI and in `ledger.py verify`. Current
exceptions are listed under `pending` with the ledger task that removes them. Examples:
`rmcp` → `agent-mcp`, `wasmtime` → `code-extensions`,
`crossterm` → `tui-terminal`, `syntect` → `tui-highlight`, `grep-*` → `code-tool-search`.

### 5.6 Migration operations: ledger, two-level done, resuming

- **`migration/ledger.json`** is the machine-readable task list and the single source
  of truth for status. Phase 9 of this document is the narrative, and its checkboxes
  follow the ledger. `python3 migration/ledger.py status|next|show|start|verify`.
- **Level 1 (programmatic):** `cargo fmt`, `clippy -D warnings` and tests for the
  task's crates (with ported TS tests), plus the dependency firewall and task-specific
  commands.
- **Level 2 (rendered TUI):** `migration/tui-parity/harness.py` runs the *real* pinned
  hoocode (built by `setup_hoocode.sh` into `target/hoocode-pin`) and the *real*
  `cortex` in identical fixed-size tmux terminals. It uses throwaway HOME and workspace
  directories and one scripted mock LLM (`mockllm.py`, OpenAI-compatible SSE, reached via
  a `models.json` custom provider). It sends identical keystrokes and captures the
  rendered screen as a (char, style) cell grid. After minimal normalization (temp
  paths, random session names, durations, branding) it requires identical **text**,
  identical **style** (colors and attributes), and optionally identical **model
  requests**, meaning the system prompt, tool schemas and tool results the model saw.
  Reports: `target/tui-parity/<scenario>/report.{md,html,png}`.
- A scenario is only trusted after `harness.py selfcheck` shows that hoocode renders it
  identically twice.
- **Done** = `ledger.py verify` passes both levels. L1 alone gives `l1_done`, and the
  task is re-verified once the tasks its scenarios need have landed.
- **Resuming:** "continue migration" follows `.claude/skills/continue-migration/SKILL.md`:
  orient (`ledger.py next`, `migration/PROGRESS.md`), port one task, gate, commit, write
  the handoff note, repeat.

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
// cortexcode-ai-stream ports utils/event-stream.ts: EventStream<T, R> is a
// futures::Stream plus a final-result future; clones share one queue.
pub type AssistantMessageEventStream = EventStream<AssistantMessageEvent, AssistantMessage>;

// Each provider crate exports a stream() function that returns at once and
// runs the HTTP work on tokio (spawn_producer):
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions, // options.signal: AbortSignal (CancellationToken)
) -> Result<AssistantMessageEventStream, BoxError>;
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
| TUI framework | **Ported hoocode renderer** (`tui-render` + `tui-components`) on `crossterm`. *Revised 2026-09-24: was `ratatui`* | hoocode's renderer draws inline into scrollback with differential line updates, which is not ratatui's full-frame buffer model. The port already exists with 340 tests, so switching to ratatui would mean a rewrite plus behavior drift. Use codex-rs as the reference for tokio/crossterm event-loop integration only |
| Native search (fd/rg) | **ripgrep as a library** (`grep-searcher`, `grep-regex`, `ignore`, `globset`) | No binary downloads, `.gitignore`-aware, same engine as `rg` |
| Async runtime | **`tokio`** (multi-threaded) end to end: providers, agent loop, tools, MCP. *Status: providers, agent loop/core and OAuth are async since 7.3 (2026-09-25); tools still run their sync bodies on the blocking pool (7.5, 10.2)* | Needed for abort (`CancellationToken`), steering/follow-up queues, parallel tool execution, streaming TUI and `rmcp` (tokio-only). Library crates stay runtime-agnostic where practical (futures `Stream`) |
| Wire formats | **Byte-compatible with hoocode JSON** (messages, session JSONL v3, RPC protocol, `--mode json` events, `settings.json`, `auth.json`, `models.json`). *Added 2026-09-24* | Users can resume hoocode sessions, RPC clients and IDE integrations keep working, and golden fixtures from hoocode can be replayed |
| Config directory | Read `~/.hoocode/` and `.hoocode/` (project) as a fallback source. Write `~/.cortexcode/` and `.cortexcode/` | Matches §10.6. Project-level `.hoocode/` (modes, skills, prompts) must be discovered too, not only the global `settings.json` |
| MSRV | **1.88** (was 1.78) | Needed by `rmcp` 3.x. `similar` 3.x needs 1.85 |
| Serialization | **`serde`** + `serde_json` (`preserve_order`, so re-serialized JSON such as tool-call arguments keeps hoocode's key order) + `serde_yaml` | De facto Rust standard |
| HTTP client | **`reqwest`** | TLS, streaming, proxy support built in |
| Embedded templates | **`include_str!`** at compile time | No `build.rs` needed for static content |
| Generated models | **`build.rs`** (gated behind `CORTEX_UPDATE_MODELS=1`) | Mirrors TS `scripts/generate-models.ts` |
| Plugin system | **Split** (revised). Declarative plugins (`.agents-plugin` manifests: skills, commands, prompts, MCP servers) are ported in Phase 12. Code extensions run as an out-of-process JSON-RPC protocol, with the `wasmtime` prototype optional | hoocode's marketplace plugins are mostly markdown and JSON, so they port without executing TS. Only `ExtensionAPI` code hooks need a runtime |
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

> **Reading Phases 0–6 after the 2026-09-24 audit.** Their checkboxes mean "a crate with
> this responsibility exists and has unit tests against hoocode **0.4.x**". They do
> **not** mean parity with the pinned 0.5.89 source, and they don't mean the crate is
> wired into the `cortex` binary. Each ⚠ note says which later task reopens the item.
> New work is tracked in Phases 7–13.
>
> **Definition of done (from Phase 7 on).** A task is checked only when all four hold:
> (a) it is ported from the pinned commit, (b) the relevant TS tests are ported or have a
> documented equivalent, (c) it is reachable from the `cortex` binary where it applies,
> and (d) a golden fixture covers its wire format when it has one (Phase 13).

### Phase 0 — Workspace bootstrap (✓ Complete)

- [x] **0.1 Root workspace** — `Cargo.toml` with all 47+ members, shared `[workspace.package]`
- [x] **0.2 CI** — `.github/workflows/ci.yml` — fmt + clippy + check + test + doc
- [x] **0.3 Reserve crates.io names** — `.github/workflows/reserve-names.yml` publishes 0.0.1 placeholders
- [x] **0.4 Release plumbing** — `scripts/bump_versions.py`, `scripts/publish_packages.py`, `release.yml`, `merge-release.yml`
- [x] **0.5 Crate scaffolding** — All 47 crate directories with `Cargo.toml` and placeholder `lib.rs`
- [x] **0.6 Core types** — `cortexcode-ai-types` (full types implementation), `cortexcode-agent-types` (full types + Agent struct)

### Phase 1 — AI Namespace (T0/T1)

- [x] **1.1 cortexcode-ai-stream** — Channel-backed `AssistantMessageEventStream` — **DONE** (async `EventStream` port since 7.3a)
- [x] **1.2 cortexcode-ai-env** — API key detection from environment variables — **DONE**
- [x] **1.3 cortexcode-ai-models** — Model registry + generated model lists — **DONE** ⚠ 907 of 1224 models, no `compat` → 8.1
- [x] **1.4 cortexcode-ai-util** — JSON repair, validation, hash, header utilities — **DONE**
- [x] **1.5 cortexcode-ai-provider-faux** — Test provider (port `faux.ts`) — **DONE**
- [x] **1.6 cortexcode-ai-provider-anthropic** — Anthropic streaming provider — **DONE**
- [x] **1.7 cortexcode-ai-provider-openai** — OpenAI Chat Completions provider — **DONE** (Responses/Codex APIs not yet ported) ⚠ Responses/Codex/compat routing → 8.2–8.4
- [x] **1.8 cortexcode-ai-provider-google** — Google Gemini + Vertex providers — **DONE** (Vertex ADC/service-account auth deferred; API-key/access-token auth only)
- [x] **1.9 cortexcode-ai-provider-azure** — Azure OpenAI Responses provider — **DONE** (reasoning-item ID pairing ported: streamed function calls carry their Responses item id encoded as `call_id|item_id` on `ToolCallContent::id`, reasoning items are stored verbatim in `ThinkingContent::signature`, and both are split/replayed on the next turn so Azure's `rs_...`/`fc_...` pairing validation passes; the cross-provider / different-model foreign-id remapping is not ported since the Rust `AssistantMessage` carries no originating provider/model)
- [x] **1.10 cortexcode-ai-oauth** — OAuth flow support — **DONE** (Anthropic PKCE + GitHub Copilot device flow; interactive browser/callback-server wiring deferred to CLI layer)
- [x] **1.11 cortexcode-ai-images** — Image generation support — **DONE** (OpenRouter provider)
- [x] **1.12 cortexcode-ai umbrella publishable** — Flip all T0/T1 leaves to `publish = true` — **DONE** (all AI leaves already had `publish = true` from scaffolding; wired the umbrella's `lib.rs` to actually re-export every leaf)

### Phase 2 — TUI Namespace (T0)

- [x] **2.1 cortexcode-tui-util** — ANSI width, grapheme handling, truncate/wrap — **DONE**
- [x] **2.2 cortexcode-tui-fuzzy** — Fuzzy matching — **DONE**
- [x] **2.3 cortexcode-tui-keys** — Key parsing + keybindings — **DONE** (global keybindings singleton not ported)
- [x] **2.4 cortexcode-tui-terminal** — Terminal abstraction (raw mode, stdin buffer) — **DONE** (Windows `ENABLE_VIRTUAL_TERMINAL_INPUT` koffi tweak deferred)
- [x] **2.5 cortexcode-tui-render** — Differential renderer — **DONE** (flatten memoization and 16ms render-coalescing are performance-only optimizations, not ported; differential terminal output is behaviorally equivalent)
- [x] **2.6 cortexcode-tui-editing** — Text editor, kill ring, undo stack — **DONE** (`KillRing`, `UndoStack`; `editor-component.ts`'s trait is defined in 2.7 alongside `AutocompleteProvider`/`Editor`)
- [x] **2.7 cortexcode-tui-components** — Box, text, markdown, select-list, autocomplete, etc. — **DONE**: Spacer, Text, TruncatedText, BoxComponent, Image, Loader, CancellableLoader, SelectList, SettingsList, Input, AutocompleteProvider, Markdown, Editor (109 tests). `Editor` is reduced-scope (see `crates/cortexcode-tui-components/src/editor/editor.rs`): no paste-marker compression, no vim-style char-jump mode, no internal viewport scrolling.
- [x] **2.8 cortexcode-tui-images** — Terminal image rendering — **DONE** (ported ahead of 2.5/2.6/2.7 since the renderer depends on it)
- [x] **2.9 cortexcode-tui umbrella publishable** — All T0 leaves `publish = true` — **DONE** (all TUI leaves already had `publish = true` from scaffolding; wired the umbrella's `lib.rs` to re-export every leaf, matching the AI umbrella pattern). Phase 2 is now complete. ⚠ not wired into `cortex` → Phase 11

### Phase 3 — Agent Namespace (T0/T1)

- [x] **3.1 cortexcode-agent-core** — Agent struct, orchestration, state management — **DONE** (`Agent`, `build_loop_config`, orchestration)
- [x] **3.2 cortexcode-agent-loop** — Turn loop, tool dispatch, background tools — **DONE** (loop moved from agent-core into standalone crate; sequential/parallel dispatch, background tasks, hooks) ⚠ 0 tests, no steering/follow-up → 7.5
- [x] **3.3 cortexcode-agent-harness** — Message conversion, system prompt, prompt templates — **DONE** (message helpers, system-prompt builder, prompt templates)
- [x] **3.4 cortexcode-agent-session** — Session persistence, file management — **DONE**; since 7.4 the port of `harness/session` (entry format, `buildSessionContext`, storage trait + memory/JSONL backends, `Session`, repos); the old `FileSessionStore` is gone
- [x] **3.5 cortexcode-agent-compaction** — Context window compaction, summarization — **DONE** (token estimation, KeepRecentStrategy, SummaryStrategy) ⚠ 171 LOC vs 1.4K TS (no branch summarization) → 9.2
- [x] **3.6 cortexcode-agent-tools** — Tool registry / factory pattern — **DONE** (ToolRegistry, factory helpers, result constructors)
- [x] **3.7 cortexcode-agent-mcp** — MCP transport, tool discovery — **DONE** (stdio and HTTP/SSE transports, `mcp.json` loader, tool discovery, Streamable HTTP with SSE fallback; OAuth deferred) ⚠ replace transport with `rmcp`, add OAuth → 9.1
- [x] **3.8 cortexcode-agent umbrella publishable** — T0/T1 leaves `publish = true` — **DONE** (umbrella re-exports all agent leaves; all T0/T1 leaves publish = true). Phase 3 T0/T1 is complete.

### Phase 4 — Code Namespace Core (T2)

- [x] **4.1 cortexcode-code-config** — Settings load/merge/persist, config paths — **DONE** (JSON config, merge, default paths)
- [x] **4.2 cortexcode-code-tools** — `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls` — **DONE** (tool functions, schemas, default_tools factory, permission policy) ⚠ 0.4.x tool set, thin wrappers → 10.2
- [x] **4.3 cortexcode-code-session** — Session CRUD, directory layout, lifecycle — **DONE** (JSONL session tree with append-only entries, branching, labels, compaction/context building, CRUD/list/fork) ⚠ JSON not hoocode-compatible, not wired → 7.2, 10.3
- [x] **4.4 cortexcode-code-prompts** — System prompt assembly, mode prompts — **DONE** (Mode, system_prompt, initial_user_prompt, templates)
- [x] **4.5 cortexcode-code-print** — Non-interactive print mode — **DONE** (text/JSON output formatting, `PrintFormatter`)
- [x] **4.6 cortexcode-code-main** — CLI entry point (`cortex` binary), arg parsing — **DONE** (`cortex` binary, `Args`, `parse_args`, dispatch; runtime modes deferred) ⚠ hand-written args, subset of flags → 10.7

### Phase 5 — Code Namespace Full (T3)

- [x] **5.1 cortexcode-code-rpc** — JSON-RPC mode — **DONE** (line-delimited JSON-RPC 2.0 server over stdin/stdout, lifecycle methods, tools/list, tools/call; wired to `cortex --mode rpc`) ⚠ generic JSON-RPC 2.0, not hoocode's RPC protocol → 10.8
- [x] **5.2 cortexcode-code-subagents** — Subagent pool, Task tool, IPC — **DONE** (`SubagentPool`, `SubagentHandle`, `task_tool`, concurrency permits, timeout, wired `cortex --mode subagent`) ⚠ not wired into the tool set → 10.9
- [x] **5.3 cortexcode-code-resources** — Resource loading, skills, context files — **DONE** (`Resource`, `Skill`, `load_resource`, `load_skills_dir`, `load_context_files`, `assemble_context`)
- [x] **5.4 cortexcode-code-extensions** — WASM plugin API (design + initial implementation) — **DONE** (`Plugin`, `PluginRegistry`, `wasmtime` runtime, `alloc`/`dealloc`/`run` ABI, `log` host import) ⚠ prototype only → Phase 12
- [x] **5.5 Interactive mode** — TUI-based interactive mode wiring — **DONE** (crossterm-based raw-mode chat loop wired to `cortex` default path; agent response placeholder until runtime integration) ⚠ readline loop, TUI crates unused → Phase 11
- [x] **5.6 cortexcode-code umbrella publishable** — All leaves `publish = true` — **DONE** (umbrella re-exports all code leaves; all T0/T1 leaves already `publish = true`)

### Phase 6 — Integrate and Release

- [x] **6.1 cortexcode top umbrella** — Re-export all namespace umbrellas — **DONE**
- [x] **6.2 Parity checklist** — Run hoocode and cortex side-by-side on scripted scenarios — **DONE** (structured parity checklist in Section 11; scripted runtime validation deferred until CLI agent wiring is complete) ⚠ script only checks `--help` → Phase 13
- [x] **6.3 Binary distribution** — Cross-platform CI builds (4 targets) — **DONE** (`.github/workflows/binaries.yml` builds for `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`)
- [x] **6.4 First public release** — `cargo publish` train, GitHub Release with binaries — **READY** (all code changes complete, CI workflows in place, clippy clean, tests passing; trigger via `Release` workflow to publish and create release) ⚠ hold the code-namespace publish until Phase 10
- [x] **6.5 Documentation** — README, install guide, migration guide from hoocode — **DONE** (README updated with install/source instructions, usage modes, and migration status link)

### Phase 7 — Foundations reset (blocking; do first)

Everything later serializes these types or runs on this runtime, so this phase is ordered strictly.

- [ ] **7.0 Pin + docs hygiene.** Record the pin in root `Cargo.toml` `[workspace.metadata.cortex.source]` (done with this revision). Fold the stale `MIGRATION_*.md` files into `CHANGELOG.md` or delete them, and point the README status at this document.
- [ ] **7.1 MSRV 1.88.** Bump `rust-version`, set the CI toolchain, and add `rust-toolchain.toml` so contributors build the same way.
- [ ] **7.2 Hoocode-compatible wire types.** In `ai-types` and `agent-types`:
  - Internally tagged `role` on messages and `type` on content blocks, all camelCase.
  - `UserMessage.content` accepts `string | blocks`.
  - `AssistantMessage` gains `api`, `provider`, `model`, `responseModel?`, `responseId?`, `diagnostics?`, a non-optional `usage` (with `cost`), and a non-optional `timestamp`.
  - `ToolResultMessage.details`.
  - Content blocks match TS field for field: `ToolCall` becomes `{type:"toolCall", id, name, arguments, thoughtSignature?}`, `ThinkingContent` becomes `{thinking, thinkingSignature?, redacted?}`, `TextContent` becomes `{text, textSignature?}`, and `ImageContent` becomes `{data, mimeType}`. Rust's per-block `cache_control` moves out of the content model into request-building options, as in TS.
  - `AgentMessage` becomes a flat `#[serde(untagged)]`/`tag="role"` enum covering the custom roles (`bashExecution`, `custom`, `branchSummary`, `compactionSummary`) instead of `{inner:{Standard:…}}`.
  - `code-session::FileEntry` uses `rename_all_fields = "camelCase"`.
  - Acceptance: round-trip real JSONL files recorded with hoocode at the pin (`tests/fixtures/hoocode-0.5.89/sessions/*.jsonl`), including v1→v2→v3 migration.
- [x] **7.3 Async core.** Move providers to async `reqwest` plus one shared SSE decoder (`eventsource-stream`), and delete the four `sse.rs` copies. `AssistantMessageEventStream` becomes a `futures::Stream`. `AbortSignal` becomes `tokio_util::sync::CancellationToken`. Drop the `blocking` feature everywhere. Keep a thin `block_on` helper for tests. *Split 2026-09-25:* 7.3a (ai-sse, EventStream, abort, the four providers), 7.3b (async agent loop/core and consumers), 7.3c (remaining `reqwest::blocking`, `cache_control` into request building).
- [x] **7.4 One session stack.** Keep `code-session` (the JSONL tree) as the implementation. Reduce `agent-session` to the storage trait hoocode has in `agent/src/harness/session/{repo,storage}` (memory + JSONL). Delete the duplicate `FileSessionStore`.
- [x] **7.5 Agent loop parity.** Port `agent.ts` and `agent-loop.ts` from the pin: steering and follow-up queues, `prepareNextTurn`, `transformContext`, `convertToLlm`, parallel/sequential tool execution, abort mid-stream and mid-tool, and the full event sequence (`agent_start … turn_end … agent_end`). Port `agent.test.ts`, `agent-loop.test.ts` and `prepare-next-turn-refresh.test.ts` onto the faux provider (these crates have 0 tests today).

### Phase 8 — AI namespace parity

- [x] **8.1 Model registry at the pin.** Regenerate `models.json` from `packages/ai/src/models.generated.ts` at the pin (1224 models) using `scripts/convert_models_to_json.py`, and record the pin in the file. Add the `compat` structs (`OpenAICompletionsCompat`, `OpenAIResponsesCompat`, `AnthropicMessagesCompat`, OpenRouter/Vercel routing). Regenerate `image-models` too.
- [ ] **8.2 API registry.** Dispatch on `model.api` (8 APIs), not on the provider name, mirroring `api-registry.ts` and `register-builtins.ts`. That routes all 31 known providers (groq, xai, openrouter, deepseek, cerebras, zai, …) through `openai-completions` with compat, and removes the hard-coded match in `code-main::runtime`. Extend the `env-api-keys.ts` parity test to cover every provider.
- [ ] **8.3 `openai-responses`.** Extract the Responses request/stream code already in `ai-provider-azure` into a shared module, then add the `openai-responses` API (reasoning replay, foreign tool-call ids, partial-JSON cleanup, image tool results).
- [ ] **8.4 Subscription providers.** `openai-codex-responses` (+ ChatGPT OAuth, SSE and WebSocket transport), `github-copilot` routing (Anthropic and OpenAI backends), `google-gemini-cli` and `google-antigravity` (+ OAuth). Lower priority: behind the core, but needed by users who log in with subscriptions.
- [ ] **8.5 Provider utilities.** Port with their tests: `transform-messages` (cross-provider handoff), `overflow`, `retry-delay`, `param-fallback`, `tool-constraints`, `simple-options`, the Anthropic tool-name normalization and eager-tool-input handling, cache retention, and `xhigh`. Adopt `jsonschema` for tool-argument validation (`validation.ts`).
- [ ] **8.6 Port the provider test suite.** The 68 TS test files map to Rust unit and fixture tests. Tests that need live keys (`*-e2e.test.ts`) become `#[ignore]` integration tests gated on env vars.

### Phase 9 — Agent harness parity

- [ ] **9.1 MCP on `rmcp`.** Replace `agent-mcp/src/transport.rs` with `rmcp` clients (child-process stdio, streamable HTTP, legacy SSE). Add MCP OAuth (`mcp-oauth.ts`) via `rmcp`'s `auth` feature. Keep the `mcp.json` loader, tool naming (`mcp_<server>_<tool>`), deferred MCP loading (`mcp-deferred.ts`) and status reporting (`mcp-status.ts`). Port the `extensions/core/mcp-loader.ts` discovery order.
- [ ] **9.2 Compaction.** Port `harness/compaction/{compaction,branch-summarization,utils}.ts` (~1.4K LOC), the cut-point selection, and `agent-session-compaction.ts` (auto-compaction thresholds and overflow recovery).
- [ ] **9.3 Harness utilities.** Port `harness/{skills,prompt-templates,messages,system-prompt}.ts` deltas since 0.4.x, plus `utils/{truncate,shell-output,output-compression}.ts` (tool output limits shared by bash and read).
- [ ] **9.4 Agent harness.** Port `harness/agent-harness.ts`: the execution environment abstraction (`env/nodejs.ts` → std/tokio implementation).
- [ ] **9.5 Proxy stream.** Port `proxy.ts` (app-server proxy streaming) only if an RPC or web consumer needs it. Otherwise it goes to Phase 12.

### Phase 10 — Coding-agent core (headless parity)

Goal: `cortex -p` and `cortex --mode rpc` behave like `hoocode` at the pin with the same settings, sessions and tools.

- [ ] **10.1 Settings.** Port `settings-{manager,types,defaults,storage}.ts`: global + project scopes, merge rules, file locking (`fs4`) and atomic writes (`tempfile`). Read `~/.hoocode/settings.json` and `.hoocode/settings.json` as fallback sources, and keep the `migrate.rs` one-shot copy.
- [ ] **10.2 Built-in tools at the pin.** Default bundle `read, bash, edit, write, SearchCodebase`. Opt-in via flags: `webfetch`, `websearch` (`--enable-webtools`), `todo` (`--enable-todowrite`), `subagent` (`--enable-subagents`). Keep `grep/find/ls` only as non-default compatibility tools. Per tool:
  - `read`: truncation, line ranges, image reads via `image`, and `read-dedup`.
  - `bash`: `process-wrap` process groups, timeout, abort, `output-accumulator`, and shell resolution (`utils/shell.ts`).
  - `edit`: `edit-diff.ts` fuzzy matching with `similar`, `file-mutation-queue`, and diff details for the UI.
  - `write`
  - `SearchCodebase`: lexical mode on `grep-searcher` + `ignore`. Semantic and hybrid modes go to Phase 12.
  - `webfetch`/`websearch`: `htmd` + `dom_smoothie`, and the `webtools-shared.ts` limits.
  - Port the tool tests.
- [ ] **10.3 AgentSession.** Port `agent-session.ts` plus its `-runtime`, `-services`, `-retry`, `-stats`, `-skills` and `-tree-navigation` modules. This is the orchestrator all three modes share: persistence into the `code-session` tree, retries, auto-compaction, model and thinking switching, and cost/usage stats. `session-manager.ts` deltas and `session-cwd`/`session-identity` go here too.
- [ ] **10.4 Models and auth.** Port `model-registry.ts` (built-ins + user `models.json` custom providers), `model-resolver.ts` (`provider/model` patterns, `--models` scoping, fuzzy match), `auth-storage.ts` (`auth.json` format-compatible; OAuth refresh with a lock), and `auth-guidance.ts`.
- [ ] **10.5 Resources.** Port `resource-loader.ts`, `skills.ts`, `builtin-skills.ts`, `prompt-templates.ts`, `context-files.ts` (AGENTS.md/CLAUDE.md walk-up), `slash-commands.ts`, `mode-prompts.ts` and the ask/plan/build/debug mode system (`extensions/core/modes.ts`), plus `agent-frontmatter`/`agent-registry` (`--agent`). Parse frontmatter with `serde_yaml_ng`. Discovery honors `.hoocode/` and `.cortexcode/`.
- [ ] **10.6 Permission gate.** Port the `extensions/core/permission-gate.ts` policy (hard tool/command policy, `--disallowed-tools`, per-session approvals). Keep the trait from `agent-types`.
- [ ] **10.7 CLI.** Move `code-main` args into `code-cli` as an exact port of the hand-written `args.ts` parser (not `clap`; see §3.4) with **exactly** the pinned flag set (`cli/args.ts`, 50+ flags incl. `--continue/--resume/--session/--fork/--no-session`, `--models`, `--thinking`, `--tools/--no-tools`, `--list-models`, `--export`, `--offline`, `--print-token-surface`). Also port `initial-message.ts`, `file-processor.ts` (`@file` args) and `list-models.ts`.
- [ ] **10.8 Print and RPC protocol parity.** `--mode json` must emit the same event objects as `print-mode.ts`. Replace the generic JSON-RPC server with hoocode's RPC protocol (`modes/rpc/{rpc-types,rpc-mode,jsonl}.ts`: commands, events, extension UI requests). Port `rpc-client.ts` as a Rust client so subagents and tests can use it.
- [ ] **10.9 Subagents.** Port `subagent-pool.ts`, `tools/subagent.ts`, `subagent-{depth,events,inbox,result}.ts`, `lifeguard.ts` (heartbeat/timeout) and `dispatch-evaluator.ts` (depth guard) on the 10.8 RPC protocol. The warm pool goes to Phase 12.
- [ ] **10.10 Small core modules.** `bash-executor`, `exec`, `event-bus`, `git-branch`, `format-*`, `token-budget`, `timings`, `diagnostics`, `output-guard`/`output-verifier`, `resolve-config-value` (`!cmd` / env interpolation), `utils/{paths,git,mime,tls-ca}`. `--ca-cert`/`--use-system-ca` map to `reqwest` rustls roots.

### Phase 11 — Interactive TUI mode

- [ ] **11.1 Wire the ported TUI.** Run `tui-render`, `tui-terminal` and `tui-keys` on a tokio task, fed by AgentSession events over channels. Replace `run_interactive_mode` and the ad-hoc `permission_dialog`. Port `core/keybindings.ts` (app keybindings on top of `tui-keys`) and `theme/theme.ts` with its JSON themes.
- [ ] **11.2 Chat view.** Port `interactive-mode.ts` (4.6K) incrementally: user and assistant messages (markdown + `syntect` highlighting), tool execution and tool-chain summaries, the `diff` component (`similar`), bash execution, compaction and branch summaries, footer, loaders, and notifications.
- [ ] **11.3 Selectors and dialogs.** Model, scoped-models, session (resume), tree, settings, thinking, theme, login/oauth, config, user-message and ask-options.
- [ ] **11.4 Input features.** The slash-command executor (`command-executor.ts`), `@file` autocomplete (`tui-components` autocomplete + `ignore`), `!` bash, clipboard text and image paste (`arboard` + `image`), and image display (`tui-images`).
- [ ] **11.5 Task panel.** `task-panel.ts`, `task-store.ts`, and subagent progress display.

### Phase 12 — Extended features (triage; each needs a go/no-go before starting)

| Item | hoocode source | Suggested approach |
|---|---|---|
| Declarative plugins and marketplace (skills, commands, MCP from `.agents-plugin`, claude and copilot formats) | `core/extensions/plugins/**`, `extensions/core/marketplace.ts` | Port. It is data only, so no code runtime is needed |
| Package manager (`hoocode install`, git/npm sources) | `core/package-manager.ts`, `package-manager-cli.ts` | Port the git/local sources. npm sources are optional |
| Code extensions (`ExtensionAPI`) | `core/extensions/{types,runner,loader}.ts` | Out-of-process protocol (reuse the 10.8 RPC). `wasmtime` stays experimental |
| Semantic index, hybrid search, capability retrieval | `core/embsearch`, `core/search`, `core/capabilities` | Spawn the same external `embsearch` binary. `bm25` crate for capabilities |
| Scheduler, `/loop`, Cron tools | `core/scheduler.ts`, `extensions/core/loop.ts` | `croner` |
| Warm subagent pool | `core/warm-subagent-pool*.ts` | After 10.9 |
| `/learn`, canvas, hooteams (`--team`), voice, export-html/share, telemetry, version check, context-gc, thinking escalation, self-docs | various | Decide case by case. Default is not ported |

### Phase 13 — Parity verification (runs alongside Phases 7–11)

- [x] **13.1 Fixture recorder.** *(Level-2 harness built: `migration/tui-parity/`; recording of session/json/rpc fixtures for 13.2 uses `harness.py run --keep`.)* A script that checks out hoocode at the pin, runs scripted scenarios with the **faux provider**, and records the session JSONL, `--mode json` event streams and RPC transcripts into `tests/fixtures/hoocode-0.5.89/`, stamped with the pin.
- [ ] **13.2 Replay harness.** Rust integration tests drive `cortex` with the same faux script and compare normalized output (ids, timestamps and paths masked) using `insta`.
- [ ] **13.3 Replace `scripts/parity_test.sh`.** Its `--help` grep checks become a smoke test only. CI runs 13.2.
- [ ] **13.4 TS test port ledger.** A table in this doc listing each of the 400 TS test files as ported, equivalent or not applicable, updated per PR.

### Recommended execution order

`migration/ledger.json` holds the authoritative order, and `ledger.py next` picks the
first ready task. In summary:

1. **7.0–7.2**: docs hygiene, MSRV/lockfile/CI gates, hoocode-compatible wire types.
2. **Milestone M1 (first Level-2 green):** a thin vertical slice.
   - 10.4a (`models.json`), 8.2a (API dispatch), 10.7a (`code-cli`) and 10.8a (print mode)
     make `print-basic` pass.
   - 10.2a (`read`) and 10.4c (system prompt) make `print-tool-read` pass, including
     identical model requests.
3. **7.3–7.5**: the async core refactor, guarded by the M1 scenarios, then one session
   stack and agent-loop parity.
4. **Phase 10 remainder + 8.1/8.2 + 9.x**: headless parity (settings, tools,
   AgentSession, resources, modes, CLI, json/RPC, subagents, MCP).
5. **Phase 11**: interactive TUI. The `startup`, `chat-basic` and `tool-read`
   scenarios already exist.
6. **8.3–8.7**: remaining providers and OAuth, any time after 7.3.
7. **Phase 12**: only after a go/no-go from the user.

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
- *Added 2026-09-24:* The following must also be readable without conversion, which is
  why the wire types in 7.2 must match hoocode:
  - `~/.hoocode/auth.json` (OAuth + API keys) and `~/.hoocode/models.json` (custom providers)
  - hoocode session JSONL files (v1–v3, with in-place migration as in `session-manager.ts`)
  - `~/.hoocode/sessions/`, where the session list and `--resume` should show hoocode sessions
  - project `.hoocode/{settings.json,modes,skills,prompts,commands,agents}` and user `~/.agents/`
  - Env overrides map as follows: `HOOCODE_CODING_AGENT_DIR` → `CORTEXCODE_CODING_AGENT_DIR`,
    and the same for `*_CODING_AGENT_SESSION_DIR` and `*_USER_AGENTS_DIR`. Both names are honored.

### 10.7 Plugin / extension system

Porting TypeScript's dynamic extension system (`jiti`-based) to Rust is the highest-risk item:

- **v1:** No plugin system — all built-in tools compiled into the binary.
- **v2:** WASM plugins via `wasmtime` runtime — portable, sandboxed, no ABI stability issues.
- **v3:** Full plugin SDK with the same capabilities as the TypeScript version.

---

## 11. Parity checklist

Tracks behavioral parity with hoocode **0.5.89** (§0.1) as seen from the `cortex`
binary. Legend: ✅ parity (fixture-verified) · 🟡 crate exists, partial or not wired ·
⬜ not started. The previous revision's checklist (all ✅) measured "crate exists" and
has been replaced. Its details are preserved in git history and in the Phase 1–6 notes.

### AI namespace

| Capability | Status | Task |
|---|---|---|
| hoocode-compatible message/content JSON | ⬜ | 7.2 |
| Async streaming + abort | ✅ | 7.3 |
| Model registry (1224 models, compat) | 🟡 907 models, no compat | 8.1 |
| API-based dispatch, 31 providers | 🟡 4 hard-coded providers | 8.2 |
| anthropic-messages | 🟡 | 7.3, 8.5 |
| openai-completions (+compat providers) | 🟡 | 8.2 |
| openai-responses | ⬜ (logic lives in the azure crate) | 8.3 |
| azure-openai-responses | 🟡 | 8.3 |
| google-generative-ai / google-vertex (ADC) | 🟡 | 7.3 |
| openai-codex-responses, google-gemini-cli, antigravity, copilot | ⬜ | 8.4 |
| OAuth: Anthropic PKCE, Copilot device | 🟡 | 8.4 |
| Cross-provider handoff, overflow, retry | ⬜ | 8.5 |
| Images (OpenRouter) | 🟡 | 8.1 |

### Agent namespace

| Capability | Status | Task |
|---|---|---|
| Agent + loop (steering, follow-up, parallel tools, events) | 🟡 untested | 7.5 |
| Session storage (memory + JSONL) | 🟡 duplicated | 7.4 |
| Compaction + branch summarization | 🟡 minimal | 9.2 |
| MCP stdio / HTTP / SSE | 🟡 hand-rolled | 9.1 |
| MCP OAuth, deferred MCP | ⬜ | 9.1 |
| Skills / prompt templates / truncation utils | 🟡 | 9.3 |

### Code namespace

| Capability | Status | Task |
|---|---|---|
| Settings (global + project, locking) | 🟡 | 10.1 |
| Default tools `read/bash/edit/write/SearchCodebase` at pin semantics | 🟡 thin | 10.2 |
| Opt-in tools `webfetch/websearch/todo/subagent` | 🟡 / ⬜ | 10.2, 10.9 |
| AgentSession (persist, retry, auto-compact, stats) | ⬜ | 10.3 |
| Session resume/continue/fork of hoocode sessions | ⬜ | 7.2, 10.3 |
| Model registry/resolver, `models.json`, `auth.json` | 🟡 | 10.4 |
| Skills, prompt templates, context files, slash commands, modes | 🟡 | 10.5 |
| Permission gate | 🟡 | 10.6 |
| CLI flag set (pinned `cli/args.ts`) | 🟡 subset | 10.7 |
| `-p` text / `--mode json` events | 🟡 format differs | 10.8 |
| RPC protocol | ⬜ (generic JSON-RPC) | 10.8 |
| Subagents | 🟡 not wired | 10.9 |
| Settings migration from `~/.hoocode` | 🟡 global only | 10.1 |

### TUI / interactive

| Capability | Status | Task |
|---|---|---|
| Renderer, terminal, keys, editor, markdown, select list, images (library) | 🟡 ported, unused by app | 11.1 |
| Interactive chat mode | ⬜ (readline loop) | 11.1–11.2 |
| Selectors, slash commands, autocomplete, clipboard | ⬜ | 11.3–11.4 |
| Themes, app keybindings | ⬜ | 11.1 |
| Task panel | ⬜ | 11.5 |

### Extended (Phase 12, triaged)

Plugins/marketplace ⬜ · package manager ⬜ · code extensions 🟡 (wasm prototype) ·
semantic/hybrid search ⬜ · scheduler ⬜ · warm pool ⬜ · learn/canvas/teams/voice/export ⬜

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

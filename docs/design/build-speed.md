# Build speed: shortest path from idea to binary

Status: agreed design (2026-09-29). Implemented 2026-10-01: D1, D9 (CI and release; §4.2,
§4.6) and D2 in CI. Implemented 2026-10-02: D3 + D4 (nextest lists 97 test binaries, down
from 270 in CI; the `--doc` CI step is scoped to cortexcode-agent-mcp, the one crate with a
doctest). Not yet: D2 locally (`ledger.py verify`), D5–D8, D10–D12.
Scope: cortexcode only; hoocode is
never touched. Implementation lands as small steps in the order of §5, each re-measured
against §2.

## 1. Goal

cortexcode is written mostly by a coding agent, so optimise the agent loop:

```
edit → cargo check -p <crate> → fix → nextest → clippy → commit → CI green → labeled release
```

Target: every step after the edit is either instant, or runs where nobody waits on it.

Environments:

- **Cloud sessions** (Claude Code on the web): Linux x86_64, 4 vCPU, 15 GB RAM, ephemeral
  container, fixed per-session disk allowance. The repo is cloned fresh each session.
- **Local**: macOS on Apple Silicon (aarch64), Ghostty terminal.
- **CI**: GitHub-hosted runners. The repo is public, so runner minutes (macOS included) are free.

## 2. Baseline (2026-09-29, cloud session, rustc 1.94.1, 74 crates, 735 build units)

| Measurement | Result |
|---|---|
| Clean `cargo check --workspace --all-targets` | 1m23s (243s CPU) |
| Clean `cargo test --workspace --no-run`, current full debuginfo | **Exhausted the session disk allowance (ENOSPC)** |
| Same with `CARGO_PROFILE_DEV_DEBUG=line-tables-only` | 3m09s, **219 test binaries, 13 GB** `target/` |
| Edit a top crate (`code-tui-app`) → `cargo check --workspace --all-targets` | 1.3s |
| Edit a base crate (`ai-types`) → `cargo check --workspace --all-targets` | 7.4s |
| Edit a base crate → `cargo test --workspace --no-run` | **48s** (relinks ~200 test binaries) |
| Edit a top crate → `cargo test -p code-tui-app --no-run` | 5s |
| `cargo test --workspace` execution (already built) | **45s** wall; 38s is test bodies run one binary at a time |
| Slowest test binaries | `tui-highlight/highlight_gold` 10.8s, `core_utils` 5.2s, `replay` 2.0s |
| Doctests | 73 `Doc-tests` invocations for 1 doctest in the whole workspace |
| Slowest compile units | the `wasmtime` stack (cranelift-codegen, wasmparser, wasmtime, zstd-sys, wast, wit-parser, wasmtime-environ) ≈ 55s of the 243s CPU |
| Linker | Already **LLD 21**: the Rust default on `x86_64-unknown-linux-gnu` since 1.90 |
| Alternating `-p <crate>` and `--workspace` builds | 0 recompiles: Cargo keeps both feature variants (costs disk, not time) |
| Fresh session `cargo test --workspace` | 3 failures (`agent-compaction`): `target/hoocode-pin` fixtures are missing until `setup_hoocode.sh` runs |

Test binaries come from 142 `crates/*/tests/*.rs` files (each its own binary) plus ~77
lib/bin unit-test targets. **The bottleneck is linking and running test binaries, not compiling.**

Reproduce with the commands in §7.

## 3. Decisions

### 3.1 Adopt

| # | Change | Where | Why |
|---|---|---|---|
| D1 | `[profile.dev] debug = "line-tables-only"` | root `Cargo.toml` | Full debuginfo × 219 binaries does not fit a session's disk. Backtraces keep file:line. |
| D2 | `cargo nextest run` for the test loop; doctests only via `cargo test --doc` in CI | CI, `CLAUDE.md`, `ledger.py verify` | Runs binaries in parallel. Estimated 45s → ~12–15s (bounded by `highlight_gold`). |
| D3 | **One integration-test binary per crate**: `crates/X/tests/*.rs` → `crates/X/tests/it/<name>.rs` + `tests/it/main.rs` (`mod <name>;` per file). Cargo auto-discovers `tests/it/main.rs` as test target `it`. | all crates with `tests/` | ~219 → ~77 binaries. Estimated relink after a base-crate edit 48s → ~20s, test build ~13 GB → ~5 GB. |
| D4 | `doctest = false` in `[lib]` of crates with no doctests | crate `Cargo.toml`s | Stops rustdoc running 73 times for 1 doctest. |
| D5 | `wasmtime` becomes an **optional dependency behind a `wasm` feature, off by default**, on `cortexcode-code-extensions`. Plugin code is `#[cfg(feature = "wasm")]`. The umbrella `cortexcode` forwards `wasm = ["cortexcode-code-extensions/wasm"]`. When enabled, use `default-features = false` plus only the features the code uses. | `cortexcode-code-extensions`, `cortexcode` | The shipped `cortex` binary (`cortexcode-code-main`) does not depend on wasmtime at all. It only costs ~55s CPU on every clean workspace build. One CI job builds and tests with `--features wasm` so it doesn't rot. The crate is experimental (a cortex-only WASM plugin host, not a hoocode port). Consumers of the published crate opt in with `features = ["wasm"]`. |
| D6 | Targeted `opt-level` only where measured: investigate `cortexcode-tui-highlight` (`highlight_gold` 10.8s in debug) with `[profile.dev.package.cortexcode-tui-highlight] opt-level = 1`. Keep only if the test gain outweighs the slower rebuild of that crate. No blanket `[profile.dev.package."*"]`. | root `Cargo.toml` | A blanket setting slows clean builds, and the crate rarely changes. Same pattern as the existing regex overrides. |
| D7 | `[workspace.lints]` (small): `rust.unused_must_use = "deny"`, `clippy.all = { level = "warn", priority = -1 }`; every crate gets `[lints] workspace = true`. No pedantic groups. | root + crate `Cargo.toml`s | Rules live in the repo, not in CLI flags. CI keeps `-D warnings`. |
| D8 | Claude Code hooks in `.claude/settings.json` (§4.1) | `.claude/` | Removes whole agent turns: warm build, fmt and clippy failures caught before hand-off. |
| D9 | CI rework (§4.2) | `.github/workflows/ci.yml`, `release.yml` | Parallel jobs, toolchain from `rust-toolchain.toml`, no redundant `cargo check`. |
| D10 | Agent rules in `CLAUDE.md` (§4.3), rationale stays here | `CLAUDE.md` | `CLAUDE.md` is what Claude Code loads each session. Keep it short. |
| D11 | macOS local setup (§4.4), documentation only | this doc | Manual OS settings; cannot live in the repo. |
| D12 | Hygiene: `cargo machete` and `cargo deny check` as a CI job | CI | Keeps unused deps and advisories out without slowing the edit loop. |

### 3.2 Unchanged by decision

- **Binaries are built only for labeled releases.** A merged PR with a `rust:patch|minor|major`
  label → `merge-release.yml` → `release.yml` → published release → `binaries.yml` (4 targets).
  No per-merge binaries or nightly builds. To get a binary, label the PR.
- **Crate split**: already 74 crates. No further splitting for build speed; more crates means
  more test binaries. Splits follow the migration plan (§5.5 of the migration doc) only.
- **Error handling**: the workspace uses neither `anyhow` nor `thiserror`; errors are
  hand-written enums implementing `std::error::Error`. Keep that convention. Don't
  introduce either crate for build-speed reasons.

### 3.3 Rejected or deferred, with the trigger to revisit

| Item | Decision | Revisit when |
|---|---|---|
| mold linker | Deferred. Rust already links with LLD. mold might save 10–30% of link time but adds install time to every session. | After D3, if linking is still >30% of an incremental `nextest --no-run`: measure mold via `RUSTFLAGS`, adopt only on a measured win. Never on macOS (unsupported; ld-prime is already fast). |
| sccache | Rejected. With no remote backend it only adds overhead in an ephemeral session and requires `CARGO_INCREMENTAL=0`. | A remote bucket (S3/GCS) exists and its credentials are configured as environment secrets. |
| cargo-hakari | Rejected. No rebuild thrash measured (§2); its `workspace-hack` crate complicates crates.io publishing. | Measured thrash, or disk pressure from duplicate feature variants. |
| bacon | Local human use only. For the agent it contends for the build lock. | — |
| `rust-analyzer` in `rust-toolchain.toml` | Rejected. Every cloud session would download it unused. Install locally with `rustup component add rust-analyzer`. | — |
| `CARGO_BUILD_JOBS` cap | Not needed at 15 GB RAM. | OOM kills during linking. |
| Cranelift codegen backend | Rejected: nightly-only and unsupported on aarch64 macOS. | Stable on both platforms. |
| Nightly parallel front end (`-Z threads`) | Rejected: nightly-only. | Stabilised. |

## 4. Design details

### 4.1 Claude Code hooks (`.claude/settings.json`, scripts in `.claude/hooks/`)

| Hook | Does | Notes |
|---|---|---|
| `SessionStart` | 1. In cloud sessions (`CLAUDE_CODE_REMOTE=true`): install `cargo-binstall`, then `cargo binstall -y cargo-nextest` (prebuilt, no compiling). 2. Run `migration/tui-parity/setup_hoocode.sh` (fixes the 3 fresh-session failures, needed for L2). 3. Start `cargo nextest run --workspace --no-run` **in the background**, logging to `target/warmup.log`, so the first real build is warm. | Idempotent; must return quickly (backgrounds long work). Installs only in cloud sessions; local machines are left alone. |
| `PostToolUse` (`Edit\|Write` on `*.rs`) | `rustfmt --edition 2021 <file>` on the edited file only | Milliseconds. Removes fmt failures from the loop. No `cargo check` here: it would run once per file in a multi-file change. |
| `Stop` | Map `git diff --name-only HEAD` (plus untracked files) to `crates/<name>/` → `cargo clippy -p <each> --all-targets -- -D warnings`. On failure, exit 2 with the short errors so the agent keeps working instead of handing off red code. | Skipped when no `.rs` changed. Bounded timeout. |

### 4.2 CI (`.github/workflows/ci.yml`)

- Toolchain read from `rust-toolchain.toml` (today `dtolnay/rust-toolchain@stable` installs
  stable and rustup then installs 1.94.1 as well).
- `concurrency: { group: ci-${{ github.ref }}, cancel-in-progress: true }`.
- Env: `CARGO_INCREMENTAL=0`, `CARGO_TERM_COLOR=always`; all cargo commands `--locked`.
- Parallel jobs, sharing `Swatinem/rust-cache`:
  - `fmt`: `cargo fmt --all -- --check` (no build, fails fast).
  - `clippy`: `cargo clippy --workspace --all-targets -- -D warnings`.
  - `test`: `cargo nextest run --workspace` (via `taiki-e/install-action`), then `cargo test --doc --workspace`.
  - `wasm`: `cargo clippy -p cortexcode-code-extensions --features wasm --all-targets -- -D warnings` + `cargo nextest run -p cortexcode-code-extensions --features wasm`.
  - `doc`: `cargo doc --workspace --no-deps`.
  - `hygiene`: `cargo machete`, `cargo deny check` (adds a `deny.toml`).
  - `test-layout` guard: fail if any `crates/*/tests/*.rs` exists outside `tests/it/` (keeps D3).
- The standalone `cargo check` step is removed (clippy already type-checks everything).
- `release.yml` pre-publish verification uses nextest too.

### 4.3 Agent rules (to add to `CLAUDE.md`)

- After an edit: `cargo check -p <crate> --message-format=short`. Build only to run tests.
- Tests: `cargo nextest run -p <crate>`; `--workspace` before committing.
- New integration tests go in `crates/<crate>/tests/it/<name>.rs`, registered in `tests/it/main.rs`.
- Lints: run `cargo clippy --fix --allow-dirty -p <crate>` first, then fix the rest by hand.
- Prefer owned types, `Arc` and `.clone()` over lifetimes in structs. Plain `async fn`
  (including in traits); avoid hand-written futures and deep combinator chains.
- Errors: follow the existing hand-written error enums; don't add `anyhow`/`thiserror`.
- New dependency: check the resolved version in `Cargo.lock`, and read the API from
  `cargo doc` / the source for that exact version, not from memory. Volatile crates obey the
  dependency firewall (`migration/dep-firewall.json`).
- `wasmtime` is behind the `wasm` feature; don't enable it in default builds.

### 4.4 macOS local setup (manual, once)

1. `sudo spctl developer-mode enable-terminal`, then System Settings → Privacy & Security →
   Developer Tools → add Ghostty and enable it. Restart Ghostty. This stops Gatekeeper scanning
   every new build script and test binary.
2. System Settings → Spotlight → Search Privacy → add the repo's `target/`.
3. `rustup component add rust-analyzer`; in the editor set `rust-analyzer.cargo.targetDir = true`
   so rust-analyzer uses its own target dir and never holds the agent's build lock.
4. Keep the default linker (ld-prime) and `CARGO_INCREMENTAL` on (the default).

### 4.5 Cloud environment

The repo-side `SessionStart` hook (§4.1) is the source of truth, so a fresh session works with
no environment configuration. Optionally, the environment's setup script (environment settings
→ Setup script) can pre-install `cargo-binstall` and `cargo-nextest`. The hook skips anything
already installed.

### 4.6 Release pipeline (implemented 2026-10-01)

Before: `merge-release.yml` failed at startup on every labeled merge (the caller granted the
reusable `release.yml` less than its `contents: write`), so no release ever ran. Behind it,
the release ran fmt, clippy and test serially, `cargo install cargo-edit` (unused), then
crates.io publishing with `cargo publish` verify builds and a 25s sleep per crate, and
relied on `release: published` to start `binaries.yml`, which a `GITHUB_TOKEN`-created
release never fires.

Now `release.yml` calls `ci.yml` (parallel gates; skippable) → bump, `cargo update
--workspace`, tag, `gh release create` → calls `binaries.yml` (4 targets in parallel, each
uploads on completion). crates.io is opt-in (`publish_crates` / `release:crates` label) and
runs beside the binaries: `--no-verify`, sleeping only before brand-new crate names.
crates.io publishing is still blocked by the workspace itself: publishable crates depend on
crates marked `publish = false`, and internal workspace deps carry no `version`.

Binaries ship as `hoocode-<target>` archives holding `hoocode` (the `cortex` binary,
renamed at packaging) and the `hoocode-ts` shim.

## 5. Implementation order

Each step is its own PR, verified with Level 1 (`cargo fmt`, clippy `-D warnings`, tests,
`migration/check_dep_firewall.py`), then re-measured with §7 and the §2 table updated.

1. **D1 + D2 + D8**: line-tables-only, nextest (`ledger.py verify` switches to
   `cargo nextest run -p …`), hooks.
2. **D3 + D4**: test consolidation. **Done 2026-10-02.** As landed: scripted `git mv` (keeps
   blame), generated `tests/it/main.rs` per crate, helpers moved to `tests/it/{common,support}/`
   and declared once in `main.rs` (per-file `#[path]` mods lost to `clippy::duplicate_mod`),
   test files rewritten to `crate::common::`/`crate::support::`. tui-components shares
   tui-render's support as `mod render_support` via one cross-crate `#[path]` in its main.rs.
   `migration/ts-tests.json` needed no scripted rewrite — `ts_tests.py generate` re-derives
   paths from the tree. `--test replay` → `--test it replay` in `migration/ledger.json` and
   `scripts/parity_test.sh`. replay.rs pins insta's old `replay__<name>` snapshot names with
   `prepend_module_to_snapshot => false`. Test names gain a module prefix (`replay::foo`).
3. **D9 + D12**: CI rework.
4. **D5**: wasmtime behind the `wasm` feature.
5. **D6 + D7 + D10**: highlight opt-level experiment, workspace lints, `CLAUDE.md` rules.
6. Re-measure; decide mold per §3.3.

## 6. Risks

- **D3 failure isolation**: one test file that doesn't compile blocks all tests of that crate.
  Accepted: CI gates on `-D warnings`, and the agent checks per crate.
- **D3 merge conflicts** with in-flight branches: schedule the move between migration tasks.
- **D5 published API**: `cortexcode-code-extensions` is marked publishable. Without the feature
  it exports no plugin host. Accepted while the plugin system is experimental; document it in
  the crate README.
- **Hooks slowing sessions**: `SessionStart` must background long work; `Stop` clippy is
  per touched crate only.

## 7. How to measure

Run in a fresh cloud session. Delete `target/` between clean runs, and watch the disk allowance.

```bash
cargo check --workspace --all-targets --timings        # clean check + slowest units (target/cargo-timings/)
cargo nextest run --workspace --no-run                  # clean test build; count binaries, du -sh target
echo '// x' >> crates/cortexcode-ai-types/src/lib.rs    # base-crate edit
cargo check --workspace --all-targets                   # incremental check
cargo nextest run --workspace --no-run                  # incremental relink
git checkout -- crates/
cargo nextest run --workspace                           # execution time
```

Record the results in the §2 table, with the date and the step that changed them.

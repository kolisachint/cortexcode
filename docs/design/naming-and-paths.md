# Naming and paths: hoocode (Rust) vs hoocode-ts

Status: **designed 2026-10-01, not started.** Architecture only; no code.
Companion to `subagents.md` (an as-built record of the same investigation, which
is where the naming problem surfaced) and `distribution.md` (which owns what
ships).

## Problem

The Rust project was renamed from **cortexcode** to **hoocode**, and the
rename is only partly done: the command is `hoocode`, but the code, the data
directory and the environment still say cortexcode.

The dangerous part is not the leftovers. It is that the leftovers **already
collide with the TypeScript tool**, in the one place that must not collide.

`cortexcode-code-paths/src/lib.rs`:

```rust
pub const CONFIG_DIR_NAME: &str = ".cortexcode";          // line 19 — we WRITE this
pub const LEGACY_CONFIG_DIR_NAME: &str = ".hoocode";      // line 21 — we READ this
pub const ENV_PREFIXES: [&str; 3] = ["CORTEXCODE_", "CORTEX_", "HOOCODE_"]; // line 25
```

The TypeScript hoocode owns `~/.hoocode` (`hoocode-ts` `config.ts:442`:
`CONFIG_DIR_NAME = appConfig.configDir || ".hoocode"`). So today:

- The Rust tool **writes** `~/.cortexcode`.
- The Rust tool **reads** the TS tool's `~/.hoocode` as a fallback
  (`resolve_agent_file`, line 81) and accepts the TS `HOOCODE_*` env
  variables, third in priority.
- The curl installer already installs the Rust binary into **`~/.hoocode/bin`**
  and deletes `~/.hoocode/lib/hoocode` (`scripts/install.sh:15,36-42`).

The two layouts are inverted: the installer treats `~/.hoocode` as the
product root while the code treats it as someone else's legacy directory. A
naive rename of `CONFIG_DIR_NAME` to `.hoocode` would put `settings.json`,
`auth.json`, `sessions/`, `cache/` and `dispatch/` in the same directory the
TS agent reads and writes — each tool silently adopting the other's settings
and credentials.

Scale of the leftovers: **676 files, 5,304 occurrences** of `cortexcode`
(636 in `crates/`, 25 in `migration/`, 5 in `scripts/`, 4 in `docs/`), 94
references to `.cortexcode`, and 23 `CORTEX*` environment variables.

## Decisions (2026-10-01, with the owner)

1. **One neutral shared root, namespaced per implementation:**
   `~/.hoocode/rust/` and `~/.hoocode/ts/`. `~/.hoocode` is the product;
   each tool owns a subdirectory and never writes the other's files.
2. **User-facing surfaces become hoocode. Internal identity stays.**
   Commands, env vars, docs, paths, TUI titles and log messages move to
   `hoocode`/`HOOCODE_`. Crate names, the `cortex` cargo binary and internal
   types stay `cortexcode`/`cortex`.
3. **Automatic one-time migration with a backup**, and the old locations stay
   readable until the user removes them.
4. **`CORTEXCODE_`/`CORTEX_` remain accepted as deprecated aliases** for one
   release, warning once, so existing scripts and docs do not break.

## Target layout

```
~/.hoocode/                    # the product root (already exists: bin/)
  bin/                         # install.sh territory: hoocode, hoo, hoocode-ts
  lib/hoocode/                 # TS install (hoocode-ts)
  rust/                        # ← Rust agent data (was ~/.cortexcode)
    settings.json
    auth.json
    hoo-config.json
    sessions/
    cache/
    embsearch/
    bin/                       # managed binaries: fd, rg
    themes/
    dispatch/                  # project dispatch dirs, when cwd == home
  ts/                          # ← reserved for the TS agent's data
```

Project-local, in any repository:

```
<cwd>/.hoocode/rust/           # settings.json, agents/, skills/, modes/, dispatch/
<cwd>/.hoocode/ts/             # the TS agent's equivalent
```

Dispatch directories move with this: `dispatch_root` (paths `lib.rs:112`)
goes from `<cwd>/.cortexcode/dispatch/` to `<cwd>/.hoocode/rust/dispatch/`.

Note that the Rust project's own dispatch dirs are **not gitignored today** —
`hoobot/.cortexcode/dispatch/` shows up as untracked noise. The rename ships a
`.gitignore` snippet for consumers, since the path changes in every downstream
repo.

## What changes in code

### `cortexcode-code-paths`

| Item | From | To |
|---|---|---|
| `CONFIG_DIR_NAME` | `.cortexcode` | `.hoocode/rust` (global and project) |
| `LEGACY_CONFIG_DIR_NAME` | `.hoocode` | `.cortexcode` — the roles swap |
| `ENV_PREFIXES` | `CORTEXCODE_`, `CORTEX_`, `HOOCODE_` | `HOOCODE_`, `CORTEXCODE_`, `CORTEX_` |
| `APP_TITLE` | `Cortex` | `HooCode` |
| `APP_NAME` | `cortex` | **unchanged** (binary, debug log name) |
| `debug_log_path` | `~/.cortexcode/cortex-debug.log` | `~/.hoocode/rust/cortex-debug.log` |

`CONFIG_DIR_NAME` becomes a path, not a single segment, so the call sites that
do `cwd.join(CONFIG_DIR_NAME).join("settings.json")` keep working unchanged.
`resolve_agent_file` keeps its shape with the swapped fallback: prefer
`~/.hoocode/rust/<name>`, fall back to `~/.cortexcode/<name>`, and stop
reading `~/.hoocode/<name>` once migration has run — that file now belongs to
the TS agent.

`bin_dir()` becomes `~/.hoocode/rust/bin`, separate from the installer's
`~/.hoocode/bin`. The alternative — sharing `~/.hoocode/bin` for fd/rg — is
rejected: install.sh rewrites that directory and would not know about
downloaded binaries.

### Environment variables

`HOOCODE_*` becomes canonical for everything that has a user-facing meaning:

| Variable | Notes |
|---|---|
| `HOOCODE_CODING_AGENT_DIR` | points at the Rust agent dir |
| `HOOCODE_CODING_AGENT_SESSION_DIR` | |
| `HOOCODE_USER_AGENTS_DIR` | |
| `HOOCODE_SUBAGENT_DEPTH`, `HOOCODE_SUBAGENT_MAX_DEPTH` | |
| `HOOCODE_NESTED_SUBAGENT_CONCURRENCY` | |
| `HOOCODE_CACHE_RETENTION`, `HOOCODE_SHARE_VIEWER_URL`, `HOOCODE_TUI_WRITE_LOG` | |
| `HOOCODE_OAUTH_CALLBACK_HOST` | |
| `HOOCODE_WEBTOOLS_TIMEOUT` | |
| `HOOCODE_GEMINI_CLI_CLIENT_ID` / `_SECRET`, `HOOCODE_ANTIGRAVITY_*` | provider client credentials |

`CORTEXCODE_*` and `CORTEX_*` are read as deprecated aliases: still honoured,
lowest priority, with a single warning per process naming the replacement.
`CORTEX_BIN` stays (it names the binary, which keeps its name).

Precedence becomes `HOOCODE_` → `CORTEXCODE_` → `CORTEX_`. Since `HOOCODE_`
was already accepted — it is the TS tool's prefix — the risk of a *shared*
variable meaning different things is real and is why the Rust data dir moves
under `rust/`: setting `HOOCODE_CODING_AGENT_DIR` now unambiguously targets
the Rust agent.

### What deliberately stays `cortexcode`

Per `CLAUDE.md` and the owner's decision: crate names (`cortexcode-*`, 76 of
them), the `cortex` cargo binary and its `scripts/install.sh` rename to
`hoocode` at install time, `APP_NAME`, internal type names, the `cortexcode`
Rust identifier in code, `[package.metadata.cortex]`, and the harness's own
`cortex` tooling names. Renaming the crates would touch all 676 files and the
entire dependency graph for no user-visible gain.

### What becomes `hoocode`

TUI window/tab titles (`APP_TITLE`), `--version` and help output, the npm
package and binary names (already `hoocode` via `scripts/install.sh` and
`scripts/shims/`), the debug-log and crash-report text, docs under `docs/`,
the migration scripts under `migration/` (including this repo's own doc
names, which reference the old project name), and every user-facing log line
that says "cortexcode".

### External dependency

`hoobot/src/config.ts:72` hardcodes `<workspace>/.cortexcode`, with a comment
at line 63 explaining that the Rust hoocode reads it. It must move to
`<workspace>/.hoocode/rust` in the same change, or hoobot's Discord workspace
stops finding its modes and `hoo-config.json`. This is the one item outside
this repo that the rename blocks.

## Migration

Automatic, one-time, on first run after the upgrade, guarded by a marker file
at `~/.hoocode/rust/.migrated-from-cortexcode`:

1. If `~/.cortexcode` exists and the marker does not, **copy** (never move)
   `sessions/`, `auth.json`, `settings.json`, `hoo-config.json`, `cache/`,
   `embsearch/`, `themes/` and `bin/` into `~/.hoocode/rust/`.
2. Leave the originals in place as the backup, and print the source, the
   destination and the file count.
3. Write the marker. Any later run skips straight to normal operation.

Copy, not move: a half-finished migration that renames the source directory
is unrecoverable, and the backup costs a few hundred MB at worst while
sessions accumulate. Deleting `~/.cortexcode` is the user's call, once they
have seen the new directory work.

Project-local `.cortexcode/` directories are **not** migrated automatically —
they sit in git working trees. The tool reads the old path as a fallback and
warns once per project, suggesting the rename, which the user commits
alongside any `.gitignore` update.

Reading both locations continues until the marker exists, so a rollback is
just deleting `~/.hoocode/rust/`.

## Plan

1. `cortexcode-code-paths`: the layout above, with `LEGACY` swapped and
   `ENV_PREFIXES` reordered. Every other crate keeps compiling.
2. The migration routine plus its marker, with `--no-migrate` and
   `--migrate-only` flags for testing.
3. User-facing strings: `APP_TITLE`, help/version, log lines, docs.
4. `HOOCODE_*` variables documented; deprecation warnings for `CORTEX*`.
5. `hoobot/src/config.ts` updated in step 1's release, not before.
6. Drop the `~/.hoocode/<file>` fallback once migration is confirmed.

Steps 1–2 are the risky part and want tests around the fallback matrix: new
location only, old only, both, neither.

## Tests

- Path resolution: each of new / old / both / neither, for
  `agent_dir`, `resolve_agent_file`, `sessions_dir`, `auth_path`,
  `dispatch_root`, `bin_dir`.
- Migration: copy is idempotent, the marker prevents a second run, a partial
  source directory still yields a working agent dir.
- Env precedence: `HOOCODE_` beats `CORTEXCODE_` beats `CORTEX_`, and a
  deprecated prefix warns exactly once.
- A golden test asserting the TUI title and `--version` say hoocode.

## Open

- **When does the TS tool adopt `~/.hoocode/ts/`?** That change lives in
  hoocode-ts, which this repo must not modify. Until it happens, `ts/` stays
  reserved and empty, and the TS agent keeps its flat `~/.hoocode/*` layout.
  The reservation is what keeps the door open without breaking anything.
- `hoo-config.json` has no `HOOCODE_` equivalent name; it is hoobot's, and it
  moves by path, not by rename.
- The migration doc's own name (`hoocode-to-cortexcode-migration.md`) refers to
  the old project name. Renaming a design doc breaks every link to it, so it
  keeps its name and gets a line at the top saying the project is now hoocode.
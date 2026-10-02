# Distribution: how the Rust hoocode reaches users

Status: **partly done** (2026-10-01). This page records what ships today and
the work deliberately left for later, so the scope of each step stays small.

## Names

| Command | Build | Repo | npm |
|---|---|---|---|
| `hoocode`, `hoo` | Rust (this repo, cargo binary `cortex`) | kolisachint/hoocode | `@kolisachint/hoocode` |
| `hoocode-ts`, `hoo-ts` | TypeScript | kolisachint/hoocode-ts | `@kolisachint/hoocode-agent` |

## Shipped

Every release made by `release.yml` (a merged PR with a `rust:*` label):

1. **Archives**, built by `binaries.yml`: `hoocode-<rust-target>.tar.gz` for
   `x86_64`/`aarch64` × `apple-darwin`/`unknown-linux-musl`. Linux builds are
   static, so one binary runs on every distro.
2. **`SHA256SUMS`** covering the archives.
3. **npm**: `@kolisachint/hoocode` plus `@kolisachint/hoocode-{darwin,linux}-{arm64,x64}`.
   The main package is a small Node launcher (`npm/hoocode/bin/hoocode.js`);
   the platform packages are `optionalDependencies` gated by `os`/`cpu`, so
   npm and bun fetch only the one for the machine. No postinstall script.
   Built by `scripts/npm/build_packages.py`, published with provenance by
   `scripts/npm/publish_packages.py` using the `NPM_TOKEN` secret. Re-runs skip
   versions that already exist. After publishing, the script checks every
   package against the registry packument and fails the job if any has not
   turned up: a fresh publish is readable only after registry propagation
   (`v0.1.5`'s `darwin-x64` took ~11 minutes), and `npm publish` reporting
   success does not mean `npm install` can resolve it yet. Verify a published
   release with
   `python3 scripts/npm/publish_packages.py <dist-dir> --verify-only`.
4. **curl installer**: `install/install.sh`, served at
   `https://kolisachint.github.io/hoocode/install.sh`. The site repo
   (kolisachint.github.io) keeps a verbatim copy in `public/hoocode/`.
   It verifies `SHA256SUMS`, test-runs the binary before replacing anything,
   installs into `~/.hoocode/bin`, and removes links left by an old
   TypeScript install (`~/.hoocode/lib/hoocode`).

## Deferred

| Item | Why deferred | What it needs |
|---|---|---|
| **Windows** | The Windows leg was the slowest part of every release. Windows users run hoocode-ts. | Add `x86_64-pc-windows-msvc` back to `binaries.yml`, a `.zip`, an `install.ps1`, a `win32-x64` npm package. |
| **crates.io** | 34 of 42 crates are unpublished; a first publish hits crates.io rate limits (run 29692981384). `release:crates` only leaves a notice. | A paced, resumable `scripts/publish_packages.py` run, then re-enable the `crates` job in `release.yml`. |
| **Site docs** | The site at `/hoocode/` still renders the TypeScript docs (synced from hoocode-ts). Only `install.sh` is Rust today. | Decide where Rust docs live (this repo's `docs/` or a `docs/` mirror of hoocode-ts), move the TS docs to `/hoocode-ts/`, and point `scripts/sync-docs.mjs` at both. |
| **Auto-sync of `install.sh`** | The site copies it by hand for now. | Have the site's sync script also fetch `install/install.sh` from kolisachint/hoocode into `public/hoocode/`. |
| **macOS signing** | Binaries carry only the linker's ad-hoc signature. curl, npm and `gh` downloads are not quarantined, so they run; a browser download triggers "Apple could not verify". | An Apple Developer ID, `codesign` + `notarytool` in `binaries.yml`. |
| **Self-update** | `hoocode update` does not know which of curl or npm installed it. | Detect the install method (path under `~/.hoocode/bin` vs `node_modules`) and print the matching command. |
| **PyPI** | Out of scope for now. | — |

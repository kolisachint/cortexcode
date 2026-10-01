# cortexcode

Rust port of the TypeScript coding agent hoocode (https://github.com/kolisachint/hoocode),
pinned to hoocode v0.5.89 (`[workspace.metadata.cortex.source]` in `Cargo.toml`).
Never modify hoocode; it is the reference.

- "continue migration" / migration status → follow `.claude/skills/continue-migration/SKILL.md`.
- **Paused 2026-10-01:** every remaining task is `deferred` by user decision (plan §0.3).
  Resume only a task the user names, by moving it back to `todo` first.
- Plan and rationale: `docs/design/hoocode-to-cortexcode-migration.md`.
- Task status (source of truth): `migration/ledger.json` via `python3 migration/ledger.py`.
- Handoff log: `migration/PROGRESS.md`.
- Build speed / agent loop design (profiles, nextest, test layout, hooks, CI):
  `docs/design/build-speed.md`. Its §4.3 rules join this file as they are implemented.
- Done = Level 1 (cargo fmt/clippy/test + `migration/check_dep_firewall.py`) + Level 2
  (`migration/tui-parity/harness.py`: real hoocode vs real cortex rendered in tmux against
  one mock LLM). `ledger.py verify <id>` runs both.

- Command names: the Rust build installs as `hoocode` and the TS one is `hoocode-ts`, by
  shims only (`scripts/install.sh`, `scripts/shims/`, release packaging). Code, crates and
  the `cortex` cargo binary keep their names; don't rename them.

Commands:

```bash
cargo nextest run --workspace                      # or cargo test --workspace
scripts/ci/fetch_hoocode_fixtures.sh               # fixtures some tests need (no build)
cargo clippy --workspace --all-targets -- -D warnings
python3 migration/check_dep_firewall.py
migration/tui-parity/setup_hoocode.sh              # build pinned hoocode into target/hoocode-pin
python3 migration/tui-parity/harness.py run all     # L2 parity; reports in target/tui-parity/
```

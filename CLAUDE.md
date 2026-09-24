# cortexcode

Rust port of the TypeScript coding agent hoocode (https://github.com/kolisachint/hoocode),
pinned to hoocode v0.5.89 (`[workspace.metadata.cortex.source]` in `Cargo.toml`).
Never modify hoocode; it is the reference.

- "continue migration" / migration status → follow `.claude/skills/continue-migration/SKILL.md`.
- Plan and rationale: `docs/design/hoocode-to-cortexcode-migration.md`.
- Task status (source of truth): `migration/ledger.json` via `python3 migration/ledger.py`.
- Handoff log: `migration/PROGRESS.md`.
- Done = Level 1 (cargo fmt/clippy/test + `migration/check_dep_firewall.py`) + Level 2
  (`migration/tui-parity/harness.py`: real hoocode vs real cortex rendered in tmux against
  one mock LLM). `ledger.py verify <id>` runs both.

Commands:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python3 migration/check_dep_firewall.py
migration/tui-parity/setup_hoocode.sh              # build pinned hoocode into target/hoocode-pin
python3 migration/tui-parity/harness.py run all     # L2 parity; reports in target/tui-parity/
```

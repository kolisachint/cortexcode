# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Next task: run `python3 migration/ledger.py next` (7.0 at the time of writing).
- Milestone M1 (first Level-2 green): 10.4a + 8.2a + 10.7a → 10.8a makes `print-basic`
  pass; 10.2a + 10.4c make `print-tool-read` pass, including identical model requests.

## Log

### 2026-09-24: 7.0 done (docs hygiene)
- Removed 10 stale status/report docs; CHANGELOG has a re-baseline entry.
- **Security:** an OpenCode API key was committed in 5 files (since 8f5693e). It has
  been removed from the tree, but it is still in git history, so the user must rotate it.
- The orphan `tests/opencode_e2e_test.rs` (never compiled) moved to
  `crates/cortexcode-ai-provider-openai/tests/opencode_live.rs`: fixed to compile, all
  `#[ignore]`d. The live shell scripts moved to `scripts/live/` (key from env).
- `cargo fmt` was already failing on main; fixed.
- Next: 7.1.

### 2026-09-24: migration infrastructure
- Pinned hoocode v0.5.89 (a6cd96e7). Audit and re-baselined plan in `docs/design/…` §0.
- Added the churn-driven crate split, volatility tiers and dependency firewall (§5.5,
  `migration/dep-firewall.json`, `migration/check_dep_firewall.py`).
- Added the Level-2 harness in `migration/tui-parity/`:
  - `setup_hoocode.sh` builds the pinned reference.
  - `mockllm.py` is the scripted OpenAI-compatible LLM.
  - `harness.py` drives tmux, normalizes cell grids, compares text, style and model
    requests, and writes md/html/png reports.
  - 5 scenarios (startup, chat-basic, tool-read, print-basic, print-tool-read), all
    `stable` on hoocode. cortex currently fails all of them (`unknown model mock:mock-model`,
    no models.json support).
- Added the ledger (`migration/ledger.json`, 58 tasks) and `ledger.py` (next/verify gate),
  plus the `continue-migration` skill.

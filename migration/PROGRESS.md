# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Next task: run `python3 migration/ledger.py next` (10.4a at the time of writing).
- Milestone M1 (first Level-2 green): 10.4a + 8.2a + 10.7a → 10.8a makes `print-basic`
  pass; 10.2a + 10.4c make `print-tool-read` pass, including identical model requests.

## Log

### 2026-09-24: 7.2 done (hoocode-compatible wire types)
- `ai-types`: every type now serializes exactly as TS:
  - `type`/`role` tags and camelCase fields;
  - TS `StopReason` values;
  - non-optional `usage`/`stopReason`/`timestamp`;
  - `AssistantMessage.api/provider/model/responseModel/responseId/diagnostics`;
  - `textSignature`, `thinkingSignature`/`redacted`, `thoughtSignature`, `ToolResult.details`;
  - string-or-blocks `content`.
  The Anthropic stop-reason mapping is ported exactly (unknown values → error).
- `agent-types`: `AgentMessage` is a flat enum tagged by `role` (user, assistant,
  toolResult, bashExecution, custom, branchSummary, compactionSummary).
- `agent-harness`: `convert_to_llm` / `bash_execution_to_text` ported.
- `code-session`:
  - camelCase entry fields; `branch` and `color` added;
  - `buildSessionContext` ported exactly (typed summary/custom messages, model taken from
    assistant messages);
  - v1→v3 migration on raw JSON; migrated files are rewritten only when every line parsed.
- Providers stamp api/provider/model (`AssistantMessage::for_model`).
- Bugs fixed on the way:
  - `PromptInput::Text` was sent as a custom message and then dropped before reaching
    the LLM.
  - Opening a v1 session silently dropped its entries on rewrite.
  - `is_context_overflow` case 3 differed from TS.
- Fixtures: 3 sessions recorded from the pinned hoocode plus hand-written
  all-entry-types, v1 and v2 files. They round-trip exactly
  (`crates/cortexcode-code-session/tests/hoocode_fixtures.rs`).
- New scenario `session-mixed` (thinking, bash permission prompt, failing read, 2 prompts).
- Harness: `wait_stable` now compares normalized screens, and the blinking "Working..."
  indicator is masked.
- Deferred, with notes in the ledger: event shape → 10.8b; the `cache_control` field → 7.3;
  faux.ts parity → 8.6; `Header.branch` → 10.3.
- Next: 10.4a (models.json), the first step of milestone M1.

### 2026-09-24: 7.1 done (MSRV and toolchain)
- MSRV 1.88; the workspace builds with `cargo +1.88 check`. `rust-toolchain.toml` pins
  1.94.1 for dev, fmt and clippy. `Cargo.lock` is now committed.
- CI changes are staged in `migration/ci/` (ledger/firewall checks, MSRV job, parity
  workflow) because workflows can't be pushed from here. The user needs to apply them.
- Next: 7.2 (wire types).

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

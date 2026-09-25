# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Next task: run `python3 migration/ledger.py next`.
- Milestone M1 (first Level-2 green): 10.4a + 8.2a + 10.7a → 10.8a makes `print-basic`
  pass; 10.2a + 10.4c make `print-tool-read` pass, including identical model requests.

## Log

### 2026-09-25: 10.8a done (print-mode text parity; M1 part 1 green)
- `code-cli::runtime::run_print_mode` ports `runPrintMode` (text) plus `prepareInitialMessage`:
  - initial message = piped stdin (trimmed) + `@file` text + first message;
  - each remaining message is its own prompt on the same transcript;
  - stdout gets each text block of the final assistant message, followed by `\n`;
  - `error`/`aborted` writes `errorMessage || "Request <reason>"` to stderr and exits 1;
  - exceptions go to stderr and exit 1.
- `code-cli::initial_message` ports `initial-message.ts` and `file-processor.ts` (text files):
  - `<file name="abs">` wrapping; empty files skipped;
  - `Error: File not found: <abs>`;
  - `expandPath`/`resolveReadPath`, minus the NFD variant (10.2).
  Image `@file`s fail as not yet supported (resize needs code-media, 11.4).
- `code-print::text_result` holds the text-mode tail as a pure function, with tests.
- Bug fixes:
  - `agent-core`: `Agent::prompt` replaced the transcript with only the new run's messages,
    so a second prompt, and every interactive turn, lost history. It now appends (regression
    test with faux).
  - User prompts are no longer wrapped in "Please help me with the following coding
    task:" (hoocode sends the raw text).
  - openai provider: HTTP errors use the SDK's `APIError.makeMessage` (`400 <error.message>`).
    User block content is always a parts array, and empty user messages are skipped
    (`convertMessages`).
- New L2 scenarios (all `selfcheck` stable):
  - `print-error`: 400 from the provider gives stderr + exit 1. 10.8a gate, passes.
  - `print-multi`: `-p q1 q2` checks the transcript. Its stdout passes; its requests differ
    only in the system prompt, so it is listed as a 10.4c gate.
- Next: `ledger.py next`. M1 part 2 is 10.2a (`read`; print-tool-read shows "Cloned tool:
  execute not available") + 10.4c (system prompt).

### 2026-09-25: 10.7a done (`code-cli`: exact port of the pinned CLI)
- New crate `cortexcode-code-cli`:
  - `args.rs` is an exact port of `cli/args.ts` `parseArgs`: every pinned flag, `-nt`/`-nbt`/`-nsc`
    shorts, unknown `--flag [value]` captured as extension flags, `-p <prompt>` (incl. `---`
    frontmatter), `parseInt` semantics. All of `args.test.ts` is ported with the same titles,
    plus extra edge cases.
  - `help_text.rs` is generated from `printHelp()` by `migration/tools/gen_help_text.py`
    (branding hoocode → cortex). Re-run it after a pin bump.
  - `lib.rs` ports the arg half of `main.ts`:
    - `Error:`/`Warning:` diagnostics (chalk colors), exit 1 on errors;
    - `--version` prints the bare version, and `--help` wins over later checks;
    - `resolveAppMode` (rpc > json > print or non-TTY stdin > interactive);
    - rpc rejects `@file`;
    - unknown long flags get `Unknown option(s): --x` (no extensions registered yet).
  - Flags that parse but aren't implemented fail with `Error: --flag is not yet supported by
    cortex` (`unsupported_flags`); so do the `install|remove|update|list|config|resources`
    subcommands.
  - `runtime`/`auth`/`permission_dialog` moved here from code-main. The crossterm firewall
    exception moved with them (still 11.1).
- `code-main` is now a thin bin (`cortexcode_code_cli::main`). The umbrella re-exports `code::cli`.
- Decision: **no `clap`**. It can't express the pinned grammar without behavior changes.
  Design doc §3.4 and §10.7 are updated.
- Removed cortex-only flags that hoocode doesn't have:
  - `--login` (the OAuth driver `code_cli::auth::login` is kept for `/login` in 11.3);
  - `--config`;
  - `--mode subagent` (the subagent pool now spawns `--mode rpc --task-id`).
- L2 fix (environment, not cortex): print-basic failed here for the pre-change binary too.
  tmux 3.4 scrolls one row when it writes "Pane is dead", and hoocode's `embsearch` stderr
  warning depends on PATH. `print-basic` and `print-tool-read` now set
  `enableSemanticIndex: false` and snapshot with history. Both are `selfcheck` stable.
  `print-tool-read` stdout matches; its requests still differ (system prompt, 10.4c).
- Known flake (pre-existing, not fixed): `cortexcode-tui-keys`
  `test_parse_key_alt_letter_legacy` sometimes fails under parallel tests. Other tests toggle
  the global kitty-protocol flag. Fix with a test mutex when tui-keys is next touched.
- Next: `ledger.py next` (10.8a print-mode parity closes M1 part 1; then 10.2a + 10.4c).

### 2026-09-24: 8.2a done; 10.4a done (first Level-2 green: `print-basic`)
- New crate `cortexcode-ai-registry`, a port of `api-registry.ts` + `stream.ts`:
  - dispatch on `model.api`;
  - `register_api_provider` / `unregister_api_providers(source_id)`;
  - TS error texts ("No API provider registered for api: X", "Mismatched api: …").
  Built-ins: anthropic-messages, openai-completions, azure-openai-responses,
  google-generative-ai, google-vertex. The hard-coded provider match in code-main is gone.
- Faux bug fixed: the stream never called `end()`, so `result()` always failed.
- `print-basic`: hoocode and cortex are byte-identical (text and style) against the mock LLM.
- Next: 10.7a (`code-cli` on clap with the pinned flag set). Note that `--offline` is
  currently accepted only because the old parser ignores unknown flags.

### 2026-09-24: 10.4a l1_done (models.json custom providers)
- New crate `cortexcode-code-models`, a port of `model-registry.ts`:
  - built-ins plus `models.json` (with `//` comments and trailing commas);
  - provider baseUrl/compat overrides, per-model overrides and custom models (custom wins);
  - `validateConfig` errors;
  - request auth via `resolve-config-value` (`!cmd`, env var, literal) and `authHeader`.
  31 tests, ported from `model-registry.test.ts` with the same titles.
- `Model.compat` added as untyped JSON (typed in 8.1).
- `cortex` looks models up through the registry and uses models.json keys/headers.
- L2 `print-basic`: cortex now finds `mock/mock-model` and fails with "No stream function
  configured". That is 8.2a (dispatch on `model.api`).
- Next: 8.2a.

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

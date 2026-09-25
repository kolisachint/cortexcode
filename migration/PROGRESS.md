# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Next task: run `python3 migration/ledger.py next` (expected: **7.3b**, async agent loop/core).
- Milestone M1 (first Level-2 green with identical model requests) is **reached** through
  light mode: `print-tool-read-light` (10.4d) passes on messages + tools. By user decision
  (2026-09-25) the default-bundle scenarios (`print-tool-read`, `-paging`, `print-multi`) stay
  as later gates for 10.4c/10.2a/10.2g; see the 10.4c ledger notes for what they wait on.

## Log

### 2026-09-25: 7.3 split; 7.3a done (async provider streams)
- Ledger: 7.3 split into 7.3a (streams + providers), 7.3b (async agent loop/core + consumers,
  remove the blocking bridge), 7.3c (remaining `reqwest::blocking` in ai-oauth/ai-images/
  code-tools, and `cache_control` into request building). Dependents re-pointed. Also fixed a
  hand-written 7.3 log entry that was a string (broke `ledger.py next`).
- New crate `ai-sse` (only owner of `eventsource-stream`): `sse_events(bytes_stream)`. Keeps
  hoocode's flush of a trailing event without a blank line. The four `sse.rs` copies are gone.
- `ai-stream` ports `event-stream.ts`: `EventStream<T, R>` (futures `Stream` + `final_result()`
  future, clones share the queue), `AssistantMessageEventStream` alias,
  `create_assistant_message_event_stream()`. `spawn_producer` runs a provider on the current
  tokio runtime, or a shared 2-thread runtime for sync callers, and ends the stream if the
  producer panics. `next_blocking`/`result_blocking` bridge the still-sync agent loop (7.3b
  removes their non-test uses). `testing` feature: one-shot mock HTTP server.
- ai-types: the sync `AssistantMessageEventStream` trait is removed; `AbortSignal` wraps a
  `CancellationToken` (clones share it; before, each clone had its own bool, so abort never
  reached anything).
- anthropic/openai/azure/google: async reqwest (`stream` feature, no `blocking`), abort via
  `select!` on `signal.cancelled()`. Errors and aborts now carry the partial content (open blocks
  included) and usage, with `stopReason: aborted` + "Request was aborted" when the signal fired,
  as in hoocode. Vertex credentials resolve on the producer task (async token exchange /
  metadata server), so missing creds are an `error` event, as in TS.
- faux: honors an already-aborted signal (TS `streamWithDeltas`); no `tokensPerSecond` yet.
- registry: `complete_simple()`; `tests/live_e2e.rs` ports abort.test.ts + the basic
  stream.test.ts cases (text, streaming, tool call) as `#[ignore]` live tests for anthropic,
  openai-completions and google. The rest of stream.test.ts is 8.6.
- Harness fix: `wait_exit` now also waits for tmux's "Pane is dead" line. tmux sets
  `pane_dead` before drawing it, so `print-tool-read-light` failed once on a missing
  `<exited status=0>` (a sync race, not an app difference; 15/15 passes after).
- Next: **7.3b**. Make `agent-loop`/`agent-core` async (tokio), consume `EventStream` with
  `.next().await`, pass `AbortSignal` from `Agent::abort` into `SimpleStreamOptions`, make
  code-cli main a tokio runtime, then drop `next_blocking`/`result_blocking` from non-test code.
  Keep `print-basic`, `print-error`, `print-tool-read-light` green.

### 2026-09-25: 10.4d done (light mode); M1 reached
- User decision: reach M1 through hoocode's `--light` preset, which has a portable prompt.
- `code-prompts::LIGHT_SYSTEM_PROMPT`. `code-tools::light` has `create_light_tools` (read,
  write, edit, bash with hoocode's short descriptions and stripped schemas, in TypeBox key order)
  and `measure_prompt_surface`. The light read gets default options and no context, as
  `baseToolsOverride` does in hoocode. Light edit maps `oldText/newText` onto the placeholder
  edit until 10.2c.
- Runtime: `--light` picks the light tools and `--system-prompt ?? LIGHT_SYSTEM_PROMPT` (custom
  prompt path: date + cwd only). `--light` is no longer an unsupported flag. The `light`
  setting waits for 10.1.
- openai provider: tools carry `"strict": false` (`convertTools`), omitted for
  Moonshot/Together or `compat.supportsStrictMode: false`.
- New L2 scenario `print-tool-read-light` (selfcheck stable, compares messages + tools):
  **pass**. The done tasks' `print-basic`/`print-error` still pass.
- 7.3's L2 guard `print-tool-read` → `print-tool-read-light` (M1 is light mode; bookkeeping,
  noted in the task log).
- Next: **7.3** (async core). It's large (~7.6K lines across ai-types stream trait, 4
  providers, oauth, agent-loop/core). Consider splitting it in the ledger first: e.g. 7.3a ai-sse
  + async provider streams behind the existing trait, 7.3b async agent loop + CancellationToken
  abort. Keep `print-basic`, `print-error`, `print-tool-read-light` green throughout.

### 2026-09-25: 10.4c l1_done (buildSystemPrompt port); M1 needs a decision
- `code-prompts::system_prompt` ports `buildSystemPrompt` exactly (app name `cortex`, which
  the harness normalizes), plus `formatSkillsForPrompt`, `formatAgentsForPrompt` (with
  `summarizeAgentDescription`) and `listSelfDocs`/`formatSelfDocsForPrompt`.
  `system-prompt.test.ts` is ported, plus exact-layout tests.
- The cortex-only `Mode`/`system_prompt`/`initial_user_prompt` inventions are removed from
  code-prompts. hoocode's modes arrive with 10.5b.
- `code-tools::default_tool_definitions` returns `ToolDefinition`s. The runtime builds the
  prompt from their snippets/guidelines (`_rebuildSystemPrompt`), then wraps them.
  `--system-prompt` goes through `resolvePromptInput` (a file path means its contents).
  Placeholder tools have no snippet, so they are unlisted, as in hoocode.
- L2 is still red: the prompt's layout matches, but the default bundle's content comes from
  other tasks (see the 10.4c notes in the ledger). One part, SearchHooCode (hoo-core
  self-knowledge), only fits deferred Phase 12. The `# About hoocode itself` section lists
  hoocode's installed docs, which cortex doesn't ship.
- **Decision needed (asked the user):** M1 (`print-tool-read` green) is blocked behind
  Phase 12 as scoped. Options:
  (a) port hoocode's `--light` mode (`core/light.ts`: fixed terse prompt + date/cwd,
      four core tools) as a small task and add light-mode print scenarios as the M1 gate;
  (b) pull SearchHooCode + shipped docs forward from Phase 12;
  (c) keep waiting for the full bundle.
- Next: act on the decision; otherwise `ledger.py next`.

### 2026-09-25: 10.2a l1_done (read tool at pin semantics)
- New crates:
  - `code-tool-api`: `truncate` (800 lines / 32KB, JS `toFixed` rounding in `format_size`),
    `path_utils` (`resolveReadPath` with the AM/PM, NFD and curly-quote variants; `code-cli`'s
    `@file` handling now uses it), `ToolDefinition` + `wrap_tool_definition` with a
    `ToolContext` (model, session branch), and Node-style fs errors (`ENOENT: ..., access '/x'`).
  - `code-media`, created early because `read` needs it: `file-type`-style sniffing (APNG
    rejected, BOM skipped) and `resize_image`/`format_dimension_note` on the `image` crate.
    11.4 adds clipboard.
  - `code-tools-fs`: `read` and `read_dedup`. JS number/slice semantics are ported for odd
    offset/limit values, checked against the pinned hoocode. All read cases of `tools.test.ts`
    and the `findCoveringRead` half of `read-dedup.test.ts` are ported.
- `code-tools::default_tools_with` takes read options + a context factory. The runtime passes
  the model, plus a `LiveTranscript` fed by `message_end` events (stands in for the session
  branch until 10.3). It uses hoocode's default settings: dedup on, since `contextGc.enabled`
  defaults to true (settings are 10.1).
- Agent-loop parity fixes:
  - tool errors are the bare message with `details: {}` (no `Error: ` prefix);
  - unknown or blocked tools are error results (`Tool x not found`, `Tool execution was blocked`);
  - tool `details` reach the tool result message;
  - tool results emit `message_start`/`message_end`: in parallel mode after the whole batch,
    so sibling reads don't dedup against each other (tools still run one at a time);
  - the abort signal reaches tools.
- `serde_json` `preserve_order` (enabled via ai-types): cortex re-serialized tool-call
  arguments with sorted keys; hoocode keeps insertion order.
- L2: new scenario `print-tool-read-paging` (selfcheck stable): truncation, paging, ENOENT,
  EISDIR, PDF note, dedup pointer. Every read result is byte-identical. Both scenarios still fail,
  but only on:
  - the system prompt (10.4c);
  - hoocode's context GC stubbing superseded reads, which no ledger task covered. Added
    **10.2g** (context-gc.ts + transformContext wiring).
- Next: `ledger.py next` (10.4c turns print-tool-read green; 10.2g turns paging green once
  10.4c lands).

### 2026-09-25: fix — tools never executed (agent-loop used a closure-less clone)
- `prepare_tool_call` handed the loop `tool.clone_via_fields()`. That clone's `execute` always
  returned "Cloned tool: execute not available", so every foreground tool call failed
  (print-tool-read showed it as the tool result).
- `AgentTool.execute` and `prepare_arguments` are now `Arc<dyn Fn + Send + Sync>`
  (`ToolExecuteFn`, `PrepareArgumentsFn`), and `AgentTool: Clone` keeps the closure.
  `clone_via_fields` is removed. `AgentTool::new` still takes a `Box` (it now needs `Sync`);
  no caller had to change.
- print-tool-read: stdout ✓, and the model requests now differ only in the system prompt
  (10.4c). The simple read result is already byte-identical, but 10.2a (the full read port:
  offset/limit, truncation, images, dedup) is still todo.

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

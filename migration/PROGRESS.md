# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Next task: run `python3 migration/ledger.py next`. 8.6 was split (too big: 41 of 68 ai
  test files unported) into 8.6a (faux, done), 8.6b (UserMessage string content, done),
  8.6c (anthropic tests incl. claude-5 request format; next), 8.6d (google tests), 8.6e
  (cross-provider suites, mostly live `#[ignore]`). Codex/Copilot/gemini-cli/OAuth test
  files stay with 8.4a/8.4b/8.4c/8.7; transform-messages-copilot-openai-to-anthropic was
  added to 8.4b.
- Still missing in phase 8: openai-codex (8.4a), Copilot (8.4b), gemini-cli/antigravity
  (8.4c), the OAuth split (8.7).
- Milestone M1 (first Level-2 green with identical model requests) is **reached** through
  light mode. By user decision (2026-09-25) the default-bundle scenarios (`print-tool-read`,
  `-paging`, `print-multi`) stay as later gates for 10.4c/10.2a/10.2g; see the 10.4c ledger
  notes for what they wait on.

## Log

### 2026-09-25: 8.6b done (UserMessage string content)
- Decision: `UserMessage.content` (and `CustomMessage.content`) is now
  `ai_types::UserContent` = untagged `Text(String) | Blocks(Vec<Content>)`, the TS
  `string | (TextContent | ImageContent)[]`. A string stays a string on the wire (sessions,
  RPC/json output). `blocks()` (Cow), `into_blocks()`, `as_str()`, and `From` for Vec/String/&str.
  `deserialize_string_or_blocks` is gone; agent-session's `CustomMessageContent` is an alias.
- Providers follow TS for string content: openai-completions sends `content: "…"` (cache
  marker / prompt suffix already handled strings); openai-responses sends one input_text part;
  google one text part (even empty); transform-messages leaves strings alone; custom messages
  become one text block in convertToLlm. anthropic: string -> one text block (skipped when
  blank); its convertMessages still isn't faithful (note on 8.6c).
- Tests: cache-control-format "cacheRetention none" now asserts the string like TS; the
  completions request tests use string user content as the TS tests do; the session fixture
  test now expects the custom message's string content to round-trip as a string (it pinned
  the old divergence). New tests for the responses/google string path.
- Next: 8.6c via `ledger.py next`.

### 2026-09-25: 8.6 split; 8.6a done (faux provider = faux.ts)
- Ledger: 8.6 -> 8.6a..8.6e (see Resume here). No task depended on 8.6.
- ai-provider-faux rewritten as a port of faux.ts: `register_faux_provider(options)` registers
  on ai-registry under a random `faux:<ms>:<id>` api (or `options.api`) and returns a
  registration (derefs to `FauxProvider`; `unregister()`); `FauxProvider::stream_fn()` for
  direct injection. Options: models, provider, `tokens_per_second`, `token_size`. Behaviour:
  empty queue -> `error` event "No more faux responses queued"; factory `Err` -> `error`
  event; sync + async factories get `(context, options, state, model)`; messages stamped with
  the registration's api/provider + requested model id; usage estimated from the TS
  `serializeContext` text (UTF-16 lengths) with prompt-cache simulation per `sessionId`;
  paced deltas and abort before/mid thinking/text/toolcall.
- Helpers now follow TS: `faux_text`, `faux_thinking`, `faux_tool_call` (random `tool:` id),
  `faux_assistant_message(content, FauxMessageOptions)`. Old `faux_text_message`/
  `faux_message`/`faux_error`/`faux_aborted` removed; agent-core tests updated.
- ai-registry no longer dev-depends on faux (faux now depends on the registry).
- Tests: faux-provider.test.ts ported (`tests/faux_provider.rs`, 22 tests).
- Known gap: `onResponse` isn't modelled in `SimpleStreamOptions` (it's an `Option<String>`
  placeholder), so faux doesn't call it.
- Next: 8.6b via `ledger.py next`.

### 2026-09-25: 8.5b done (validateToolArguments)
- ai-util `validation`: `validate_tool_arguments(name, schema, args, SchemaOrigin)` =
  validation.ts. `typebox_convert` ports TypeBox 1.1 `Value.Convert` (TryNumber/Boolean/
  String/Null/Array, unions, literals, enums; StringEnum/Unsafe untouched) for hoocode's
  TypeBox tools; `coerce_with_json_schema` for plain JSON schemas; a validator reporting
  TypeBox's errors (keyword order, instance paths, en_US messages) and the TS error text.
- `AgentTool.plain_json_schema` (default false; hoocode builds MCP schemas with TypeBox
  too, noted on 10.11). agent-loop's `prepareToolCall` now validates for real.
- Tests: validation.test.ts ported; `validation_fixture.json` recorded from the pinned
  hoocode with node (TypeBox and plain paths); agent-loop conversion/error test.
- New L2 scenario `print-tool-invalid-light` (read called without path): the tool-result
  error text in the next request matches hoocode byte-for-byte.
- Next: `ledger.py next`.

### 2026-09-25: 8.5 split; 8.5a done (retry-delay + SDK retries, overflow, partial JSON)
- Ledger: 8.5 split into 8.5a (retry-delay, SDK retry policy, overflow, json-parse,
  cross-provider handoff) and 8.5b (validation.ts + agent-loop wiring). The claude-5-models
  request-format note moved to 8.6 (anthropic provider); diagnostics.ts noted on 8.4a.
- ai-util `retry_delay`: parseRetryAfterMs, formatDelay, describeProviderError,
  isLongRetryDelayError, the cap-fetch rule (`exceeds_retry_delay_cap`), and the SDK retry
  loop (`send_with_sdk_retries` / `post_json_with_sdk_retries`: 2 retries on 408/409/429/5xx
  and transport errors, x-should-retry, retry-after(-ms), 0.5..8s backoff with jitter, JS
  timer clamp). Wired into openai-completions (inside each param-fallback pass), the
  Responses driver (openai-responses + azure) and anthropic; final errors go through
  describeProviderError; transport errors read "Connection error." / "Request timed out.".
  `SimpleStreamOptions.max_retries` now reaches the providers.
- anthropic: `api_error_message` follows @anthropic-ai/sdk (whole parsed body).
- ai-util `partial_json`: port of the partial-json package (0.1.7, Allow.ALL);
  `parse_streaming_json` now follows json-parse.ts (the old tolerant parse returned
  `{"path":"README}"}` for `{"path":"README`).
- overflow: Together AI pattern fixed (`model'?s`); overflow.test.ts ported.
- cross-provider-handoff.test.ts ported as an `#[ignore]` live test in cortexcode-ai.
- New L2 scenario `print-retry` (503 then an answer): passes, identical requests.
- Next: 8.5b (validation.ts), via `ledger.py next`.

### 2026-09-25: 8.3 done (openai-responses crate; azure on top of it)
- New crate `ai-provider-openai-responses`: `shared` = openai-responses-shared.ts
  (`convert_responses_messages` incl. foreign `fc_<hash>` item ids, different-model fc id
  drop, TextSignatureV1 ids/phase, reasoning-item replay, image tool outputs;
  `convert_responses_tools`; `ResponsesStreamState` = processResponsesStream with summary /
  content-part tracking, refusals, arguments.done deltas, usage + cost, service-tier hook,
  error/failed events; `run_responses_stream` HTTP driver). lib = openai-responses.ts
  (`stream`, `stream_responses(ResponsesOptions)`, cache-affinity headers + compat,
  reasoning off/none defaults, Copilot exception, service-tier pricing).
- ai-provider-azure rewritten on the shared crate (request.rs gone): option/env/model base
  URL resolution + normalization via `reqwest::Url`, deployment-name map, `api-key` header,
  `{base}/responses?api-version=` with the query replaced (as the SDK's buildURL), config
  errors surface as stream `error` events.
- `openai-responses` registered in ai-registry; routing test no longer lists it as pending.
  `openai_api_error_message` moved to ai-util (openai crate re-exports it).
- Tests: copilot-provider, foreign-toolcall-id, partial-json-cleanup, tool-result-images
  (conversion), azure-openai-base-url ported; the two live e2e files are `#[ignore]`d.
- Next: `ledger.py next`.

### 2026-09-25: 8.2 done (openai-completions parity; env-api-keys parity; routing)
- provider-openai is a port of openai-completions.ts: `getCompat`/`detectCompat`
  (`request::ResolvedCompat`), `buildParams` (prompt_cache_key/retention, store,
  stream_options, max_tokens field, tools/`tools: []` on tool history, tool_choice,
  tool_stream, every thinking format, OpenRouter/Vercel routing), Anthropic-style cache
  markers, promptSuffix, `convertMessages` (transformMessages, developer role, thinking
  replay incl. signature field / thinking-as-text, reasoning_details, bridging assistant,
  tool result names, batched tool-result images), strict tools via `to_strict_json_schema`,
  client headers (model, Copilot dynamic, session affinity, caller overrides).
  Streaming keeps content live in `partial` (as TS), coalesces tool calls by index then id,
  responseId/responseModel, choice-usage fallback, cost via ai-models, param-fallback retry
  loop, OpenRouter `metadata.raw` suffix. `stream()` resolves the key via ai-env and fails
  with `No API key for provider: X`.
- ai-util gains transform_messages, to_strict_json_schema, param_fallback, copilot headers
  (8.5 still owns their TS tests). `SimpleStreamOptions` gains temperature, max_tokens,
  headers, timeout_ms, metadata, constrain_tool_calls, tool_choice.
- ai-env: dropped `mistral` (not in hoocode), empty values count as unset, GAC path does not
  fall back to the default ADC file, OnceLock cache.
- ai-stream testing: `serve_script` (scripted multi-response server that records requests).
- Tests: all 9 openai-completions-*.test.ts files + env-api-keys.test.ts + fireworks/together
  env halves (59 + 9 tests); `cortexcode-ai/tests/routing.rs` checks every catalog
  (provider, api) pair dispatches through the registry, with openai-responses (8.3),
  openai-codex-responses (8.4a), google-gemini-cli (8.4c) listed as pending.
- Harness requests now match hoocode on every non-message field except `prompt_cache_key`
  (`prompt_cache_retention: "24h"` and `store: false` were missing before). The key needs the
  agent's session id, which code-cli doesn't set yet (noted on 10.3). Scenario results are
  unchanged: everything owned by a done task passes.
- Left: SDK client retries + retry-after suffix (noted on 8.5); UserMessage string content
  (noted on 8.6).
- Next: `ledger.py next`.

### 2026-09-25: 8.1 done (model catalog at the pin)
- New data crate `ai-models-catalog`: `data/models.json` (1224 models), `data/image-models.json`
  (57), `data/pin.json`, exposed as `MODELS_JSON` / `IMAGE_MODELS_JSON` / `PIN_JSON`. A test
  fails when `pin.json` differs from the workspace pin (regenerate on every pin bump).
- `scripts/convert_models_to_json.py` now loads the pin's own built
  `dist/models.generated.js` / `image-models.generated.js` with node (the old TS parser broke
  on escaped quotes) and keeps hoocode's order. Run after `setup_hoocode.sh`.
- ai-types: `Model`/`ModelCost` serde in hoocode's JSON shape; typed compat views
  `OpenAICompletionsCompat`, `OpenAIResponsesCompat`, `AnthropicMessagesCompat` via
  `Model::compat_as()` (compat stays JSON on `Model` so models.json overrides deep-merge).
  anthropic/openai request builders read compat through them.
- ai-models: order-preserving registry (`get_providers`/`get_models` in catalog order, id
  index); code-models dropped its sort-by-id stopgap (built-in defaults = first catalog model).
- ai-images: `ImagesModel` gets `name`/`input` + serde; `get_image_model`/`get_image_models`/
  `get_image_providers` (image-models.ts).
- Ported the catalog cases of claude-5-models / fireworks-models / together-models tests
  (8 tests). Env-key halves noted on 8.2, request-format cases on 8.5.
- Next: `ledger.py next`.

### 2026-09-25: 7.5b done (agent.ts parity); phase 7 complete except 7.1 bookkeeping
- `Agent` follows agent.ts: state is reduced from loop events (`message_end` appends,
  streaming message, pending tool calls, `turn_end` error message); one run at a time with
  hoocode's busy errors for `prompt`/`continue`; a fresh `AbortSignal` per run passed to
  listeners (`subscribe(|event, signal|)`) and exposed as `signal()`; `wait_for_idle()`;
  thrown run failures become the error assistant message + `message_start/end`, `turn_end`,
  `agent_end` (`handleRunFailure`); `continue()` from an assistant tail drains steering
  (skipping the initial steering poll) then follow-ups; queue modes settable; settings
  (`session_id`, tool execution, budgets, retry cap) and hooks settable after construction,
  with `prepareNextTurn` always wired so a late assignment reaches the running prompt.
- API: `prompt(impl Into<PromptInput>) -> Result<(), AgentError>` (text + images or messages);
  run output is read from `state()`. State setters replace property assignment. code-cli
  adapted (listeners take `(event, signal)`; interactive mode reads the new messages from state).
- Ported agent.test.ts (16) and prepare-next-turn-refresh.test.ts; async-subscriber tests
  become blocking listeners (a run can't finish before they return). 20 agent-core tests.
- agent-loop tests: the parallel gate waits up to 10s (it opens as soon as the second tool
  runs), fixing a flake under full-workspace load.
- Next: `ledger.py next`.

### 2026-09-25: 7.5 split; 7.5a done (agent-loop.ts parity)
- Ledger: 7.5 split into 7.5a (agent-loop.ts + agent-loop.test.ts) and 7.5b (agent.ts +
  agent.test.ts + prepare-next-turn-refresh.test.ts).
- `agent-loop` is a rewrite following `agent-loop.ts`: `agent_loop` / `agent_loop_continue`
  return an `EventStream<AgentEvent, Vec<AgentMessage>>` (run spawned on tokio);
  `run_agent_loop(prompts, context, &config, emit)`, `run_agent_loop_continue(&mut context, ..)`.
  Turn order, pending/steering/follow-up handling, `prepareNextTurn` (context, model, thinking
  level; `off` clears reasoning) before `shouldStopAfterTurn`, error/aborted early exit, no
  turn-start abort check (as in TS). Hook errors propagate like TS throws.
- Tools: `prepareToolCall` (not found, `prepareArguments`, validation hook, cortex permission
  gate, `beforeToolCall` which may rewrite `args` in place, block reason), parallel batches run
  concurrently with `tool_execution_end` in completion order and result messages in source
  order, sequential when the config or any tool says so, `tool_execution_update` from tool
  `onUpdate` via a channel, terminate only when every result terminates, `afterToolCall`
  overrides incl. `details`. Background tools: placeholder result now, detached run, follow-up
  message later (default or `createBackgroundResultMessage`), loop stays alive while in flight.
- agent-types: `MessageUpdate` carries the provider `AssistantMessageEvent` (boxed);
  `AssistantMessagePartialEvent` removed; new `ToolExecutionUpdate`; `ToolExecutionEnd` has no
  `args` (as TS); `TurnEnd`/stop-context `tool_results: Vec<ToolResultMessage>`; config hooks are
  `Send + Sync`; `AgentLoopConfig::new(model)`. code-print json mapping updated (10.8b owns parity).
- Known deviations (documented in the crate): hooks are sync; tools keep sync `execute` on
  `spawn_blocking`; a background tool's `afterToolCall` runs at collection time;
  `validateToolArguments` is a pass-through until 8.5 (noted on 8.5).
- 26 tests (all 22 of agent-loop.test.ts plus model/thinking switch, tool updates, blocked /
  unknown tools, assistant-tail continue). M1 scenarios still pass.
- Next: **7.5b** (agent.ts). Use `AgentLoopConfig::new` in agent-core's `build_loop_config`.

### 2026-09-25: 7.4 done (one session stack)
- `agent-session` is now the port of hoocode `packages/agent/src/harness/session/`:
  - `entry` + `context` moved here from code-session (hoocode keeps `SessionTreeEntry` and
    `buildSessionContext` in the agent harness; coding-agent's session-manager imports them).
    code-session re-exports both modules, so its API is unchanged.
  - `storage`: `SessionStorage` trait (sync; the TS promises wrap in-process state and local
    appends), `InMemorySessionStorage`, `JsonlSessionStorage` (v3 header in hoocode key order,
    malformed entry lines skipped, leaf = last line), `load_jsonl_session_metadata`.
  - `session::Session<S>` (all `append*`, `moveTo` with branch summary, `getSessionName`,
    `buildContext`); `repo`: `InMemorySessionRepo` (sessions shared as `Arc<Mutex<Session>>`,
    so `open` returns the same one), `JsonlSessionRepo`, `get_entries_to_fork`.
  - Shared helpers: `create_session_id` (v7), `generate_entry_id`, `create_timestamp`,
    `encode_cwd`. code-session's own copies now call these. Behavior fix: `encode_cwd` drops
    only one leading separator, as hoocode's `/^[/\\]/` does (it used to trim all).
  - The old `SessionData` / `FileSessionStore` / `MemorySessionStore` are deleted (no users).
- Ported `storage.test.ts`, `session.test.ts` (both backends) and `repo.test.ts`: 29 tests.
- Next: `ledger.py next`.

### 2026-09-25: 7.3c done (no reqwest::blocking left; cache_control in request building)
- ai-oauth (anthropic token exchange/refresh, GitHub Copilot device flow + refresh), ai-images
  (`generate_images`) and code-tools (`webfetch`/`websearch` placeholders) are async; nothing in
  the workspace enables reqwest's `blocking` feature. code-cli runs the OAuth calls through its
  `async_runtime()`. Placeholder tools bridge with a `block_on` helper (block_in_place on the
  runtime, or a throwaway runtime in unit tests) until 10.2e replaces them. code-tools' reqwest
  now uses rustls like the rest.
- ai-types: `TextContent`/`ImageContent.cache_control` and `CacheControl` are gone, as are the
  cortex-invented `cache_control_format` / `supports_long_cache_retention` stream options (in
  TS those are openai-completions `compat` fields: 8.2/8.5). New `CacheRetention`
  (none/short/long) + `cache_retention` on the stream options and `AgentLoopConfig`, forwarded
  by the loop.
- ai-util `resolve_cache_retention` (cache-retention.ts): explicit, else
  `CORTEXCODE_CACHE_RETENTION` / `HOOCODE_CACHE_RETENTION`, else long.
- anthropic request building follows `buildParams`/`convertTools`/`convertMessages`: the
  system prompt is always a text-block array; `cache_control` (`{"type":"ephemeral","ttl":"1h"}`
  for long unless `compat.supportsLongCacheRetention` is false; no ttl for short; none for
  none) goes on the system block, the last tool and the last block of a final user turn. Other
  anthropic request gaps (OAuth identity block, tool schema shape, adaptive thinking) are 8.5.
- `fix_struct_fields.py` learned the removed fields (59 edits).
- Next: `ledger.py next`.

### 2026-09-25: 7.3b done (async agent loop + core)
- agent-loop: `run_agent_loop`/`run_agent_loop_continue` and the turn/tool helpers are async.
  The provider stream is consumed with `.next().await`. Tools (still sync `execute`) run on
  `spawn_blocking` with the run's signal. Background-task waits use `tokio::sync::Notify`
  instead of a Condvar.
- agent-core: `prompt`/`continue` are async. Each run gets a fresh `AbortSignal`
  (agent.ts `abortController`) in `AgentLoopConfig.signal`, so it reaches the provider stream
  and the tools. `abort()` aborts it; the old `stop_requested` flag (which nothing read) is
  gone. New test: `abort()` mid-stream against a stalling mock server ends the run with the
  partial assistant message, `stopReason: aborted`.
- code-cli drives the agent from one multi-thread tokio runtime (`async_runtime().block_on`);
  providers spawn onto it. `next_blocking`/`result_blocking` are now test-only.
- Harness: `cortex_cmd` always runs `cargo build` for the binary (a no-op when fresh).
  Before, it only built when the binary was missing, so `verify` could compare a stale binary.
- Next: **7.3c**: async `ai-oauth`/`ai-images`/`code-tools` HTTP (drop the last
  `reqwest::blocking`), then move `cache_control` hints into anthropic request building
  (TS `cache-retention.ts` + anthropic.ts) and delete the field.

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

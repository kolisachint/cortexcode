# Migration progress / handoff log

Newest entry first. Each entry says where to resume. Status numbers come from
`python3 migration/ledger.py status`; don't duplicate them here.

## Resume here

- Disk: if builds fail with ENOSPC / "Bus error" in ld, `rm -rf target/debug` (keep
  target/hoocode-pin) and build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
  CARGO_PROFILE_TEST_DEBUG=0`. Without debuginfo a full verify leaves target/debug at about
  1 GB instead of about 28 GB.
- Phase 12 is deferred by user decision (2026-09-29). Phase 11 is complete (2026-09-29).
  What's left outside phase 12: the phase-10 `l1_done` tasks whose L2 is the default-bundle
  prompt (10.2a/b/c/d/f/g, 10.4c, 10.5, 10.5b). That prompt includes SearchHooCode from the
  self-knowledge extension (12.4, deferred), so they can't reach L2 until 12.4 or a user
  decision. Also 9.1/10.2e (blocked on decisions), 10.11 (needs 9.1), and phase 13.
- Next task: run `python3 migration/ledger.py next`. 8.6 is finished (8.6a..8.6e done): every
  ai test file is ported or owned by a task (codex/Copilot/gemini-cli/OAuth files by
  8.4a/8.4b/8.4c/8.7; openrouter-cache-write-repro by the new 8.8 onPayload/onResponse task;
  lazy-module-load has no Rust counterpart).
- Phase 8 is complete (16/16): every catalog API is registered and every provider honors the
  typed onPayload/onResponse hooks.
- Phase 9: 9.2a/9.3a/9.3b/9.4a/9.4b done; 9.1 blocked on the rmcp decision (see its ledger
  block); 9.2b is l1_done (L2 compact-command waits on the interactive app).
- 10.3a done: print mode (and the stopgap interactive loop) run on `AgentSession`
  (`crates/cortexcode-code-agent-session`). 10.3b/10.3c done.
- 2026-09-27 session: 10.5, 10.5b (code-modes), 10.6 (code-permissions) are l1_done (their L2
  scenarios `print-context-files` / `mode-plan` / `permission-prompt` wait on the phase-11 TUI
  and 10.9 agents roster); 10.7b done; 10.8b done (`--mode json` in hoocode's wire shape);
  10.8c done (`--mode rpc` core); 10.8e done (Rust `RpcClient`); 10.8d done (rpc mode runs an
  `AgentSessionRuntime`). 10.9 split into 10.9a..e; 10.9a done (subagent foundations), 10.9b done (cold pool +
  lifeguard), 10.9e done (child protocol), 10.9c done (warm pool + inbox), 10.9d done (Task/TaskOutput tools),
  10.9f done (Task tool in sessions). Phase-10 subagents are complete. 10.10 split into 10.10a/b/c,
  all done (core utils, parseGitUrl, TLS CA trust); check `ledger.py next`.
- Deferred leftovers recorded in the ledger: `--resume` (11.3), `--export` (phase 12),
  `.webtoolsignore` host rule in the permission gate (10.2e), TUI consumers of
  `ModesExtension::take_actions` / `PermissionUi` (11.2/11.3).
- Phase 10: 10.1 split into 10.1a/b/c all done: code-paths + code-settings, and the CLI
  reads settings.json (the invented `Config` / `config.json` crate is gone).
- Milestone M1 (first Level-2 green with identical model requests) is **reached** through
  light mode. By user decision (2026-09-25) the default-bundle scenarios (`print-tool-read`,
  `-paging`, `print-multi`) stay as later gates for 10.4c/10.2a/10.2g; see the 10.4c ledger
  notes for what they wait on.

## Log

### 2026-09-29 · 11.5b task panel in the app; 11.5 closed; phase 11 complete
- `TaskPanelComponent` mounted in the tasks slot (chrome density → full/summary), task-store
  subscription → re-render, 1s run-clock tick, alt+l / shift+alt+l lens cycle with dial steps,
  `task_store().reset()` on each user message, `settle_dangling_main_tasks` when a request settles
  (done after a clean stop, else cancelled; skipped while messages are queued).
- Team focus (alt+n, nudge/attach) is not wired: role agents come from hooteams `--team` (12.7).
- L2 `todo-write` passes (stable). 10.2f still needs `print-todo-write` (default-bundle prompt).
- Full L2 after 11.5b: 53 pass; the known 9 default-bundle/mode scenarios fail, plus `subagent-task`
  once (the parent timed out waiting on the child at 60 s). It passed on the next two reruns and
  selfchecks stable. If it recurs, look at child start-up time under load.

### 2026-09-29 · 11.5 split; 11.5a TaskPanelComponent
- 11.5 split into 11.5a (component + tests) and 11.5b (app wiring; L2 todo-write, which is
  10.2f's gate and waits on the panel).
- `code-tui-widgets::task_panel`: lenses (flat plan with linked runs nested / subagents forest /
  teams roster), tab strip with per-lens counts and key hints, rail colour, rows with tags,
  usage, live activity + run clock, ⚠ notes, summary density, team focus (↑/↓, n, a, q/esc as
  `TaskPanelEvent`s). The pin's per-version row memo is an optimization and isn't reproduced;
  its 1s run clock is `ticking()` for the app to poll.
- Ported task-panel.test.ts + task-panel-team-focus.test.ts (45 tests; a static lock serializes
  the global store). Not ported: the two console.warn store tests (Rust store is silent).
- Next: 11.5b (mount in the tasks slot, alt+l cycle, store subscription → re-render, chrome
  summary density and mid-turn collapse, team focus).

### 2026-09-29 · 11.4g message queue; 11.4 closed
- Enter while streaming steers (`prompt` with `StreamingBehavior::Steer`, no turn of its own to
  settle); alt+Enter queues a follow-up; alt+Up (`app.message.dequeue`) restores every queued message
  into the prompt; Escape while streaming restores then aborts; the pending-messages area lists
  "Steering:/Follow-up:" rows plus the dequeue hint (on `QueueUpdate` and each user message).
- Compaction queue: text typed during a compaction is held ("Queued message for after
  compaction") and flushed on `compaction_end` (first as the prompt, the rest steer/follow-up;
  all queued when a retry is pending; failures put them back).
- L2 (new, stable, pass): message-queue (with request comparison), message-queue-escape,
  compact-queue. 11.4 container verified.
- Next: `ledger.py next` (11.5 task panel, L2 subagent-task).

### 2026-09-29 · 11.4f compaction UI; 9.2b done; 11.4g added
- `compaction_start`: spinner with "(Esc to cancel)" in the status row, terminal progress; Escape
  aborts the compaction. `compaction_end`: transcript rebuilt from the session file plus the
  appended `CompactionSummaryMessageComponent` (new widget), or the cancel/error line.
  `/reload` now refuses during a compaction.
- Session semantics fixed to the pin: a manual compaction awaits the summary request instead of
  racing the abort signal (`tokio::select!`). An abort before any text fails as "Compaction failed:
  Summarization produced an empty summary", which is what the pinned TUI shows. The adapted Rust test
  now asserts that. The pin's own test cancels via a `session_before_compact` extension (ledger
  note on 12.3).
- mockllm: per-response `delay_s` (README). New L2 `compact-empty` (the pin compacts an empty
  session and draws the block twice; matched), `compact-cancel`; `compact-command` passes, so
  9.2b is done. Known failures are now 10.
- New task 11.4g: message-queue-controller.ts had no owner (steer/follow-up while streaming,
  pending display, dequeue, compaction queue). 11.4 now depends on it. Its code is drafted in
  `/tmp` only; start from the pin source.
- cd-reload: waits for the first warning to clear (it lingered under load).
- Next: `ledger.py next` (11.4g).

### 2026-09-29 · 11.4e6 /subagent; 11.4e closed (+ /model completions)
- `/subagent <mode> <task>`: usage / unknown-type status, "Spawning …", `get_subagent_pool`
  (made inside the runtime task: its lifeguard needs the Tokio context) + `dispatch` with
  `force_agent`; the summary is appended to the session file as a displayed `subagent` custom
  message.
- New widgets `custom_message.rs`: `CustomMessageComponent`, `BranchSummaryMessageComponent`
  (had no owner); the transcript draws `custom` and `branchSummary` messages.
- Fix: the app now calls `set_terminal_owned_by_tui(true/false)` around the TUI (agent-log.ts), so
  `[DISPATCH]` lines no longer write over the screen.
- `/reload` replays from `SessionManager::build_context()` like the pin.
- `/model <prefix>` argument completions (fuzzy over scoped else available models); L2
  `model-completions` gates the 11.4e container.
- L2 `subagent-command`, `model-completions` (new, stable, pass). Full L2: only the known 11.
- Next: `ledger.py next` (11.4f compaction UI; also add the /reload is_compacting guard).

### 2026-09-29 · 11.4e5 /export (jsonl), /import
- `/export <file.jsonl>` via `AgentSession::export_to_jsonl` (record line); any other target
  reports that the HTML export isn't available yet (export-html is 12.7, deferred).
- `/import <path>`: Yes/No confirm in the prompt slot, `AgentSessionRuntime::import_from_jsonl`,
  the missing-cwd confirm on a second pass, file-not-found error; `getPathArgument` quoting.
- L2 `export-import` (new, stable, pass). Next: `ledger.py next` (11.4e6 /subagent).

### 2026-09-29 · 11.4e4 /cd, /reload
- `/cd [path|~|-]` through `AgentSessionRuntime::change_directory` (new session there, "✓ Working
  directory" note), `previous_cwd` for `/cd -`, `/cd` argument completions
  (getChangeDirectoryCompletions; names sorted like libuv's scandir), alt+… `app.session.
  changeDirectory` prefills `/cd `, `app.session.fork` opens /fork.
- `/reload`: reload box in the prompt slot, `session.reload()`, keybindings/theme re-read, the
  transcript replayed with the listing below, "Reloaded …" status. The compaction guard waits on
  11.4f (ledger note).
- Leftover (noted on 11.4e): `/model <prefix>` argument completions aren't wired.
- L2 `cd-reload` (new, stable, pass). Next: `ledger.py next` (11.4e5 /export jsonl, /import).

### 2026-09-29 · 11.4e3 /fork, /clone
- `/fork` opens `UserMessageSelectorComponent` (polled like the tree selector) on the latest
  user message; selecting forks before it through `AgentSessionRuntime::fork(.., Before)`,
  rebinds, redraws, and puts the message text back in the prompt. `/clone` forks `At` the leaf.
- L2 `fork-clone` (in memory) and `fork-clone-persisted` (new, stable, pass).
- Next: `ledger.py next` (11.4e4 /cd /reload).

### 2026-09-29 · 11.4e2 /color, /chrome, colour dial
- `/color <slot|name>` (chip line in the chat, usage warning), bare `/color` opens
  `session_color_selector` in the prompt's slot with live chip preview (Esc restores the real
  colour), alt+c / shift+alt+c step the colour dial, `/chrome [full|compact|bare]`.
- L2 `color-chrome` (new, stable, pass). Next: `ledger.py next` (11.4e3 /fork /clone).

### 2026-09-29 · 11.4e split; 11.4e1 /hotkeys /changelog /debug + startup What's New
- 11.4e split into 11.4e1..e6 (info; /chrome /color; /fork /clone; /cd /reload; /export jsonl +
  /import; /subagent). /share and the HTML half of /export stay with 12.7 (deferred).
- `code-tui-app`: `hotkeys.rs` (the pin's page, generated from command-executor.ts with every
  key looked up live), `changelog.rs` (parseChangelog, getNewEntries, getChangelogPath,
  getChangelogForDisplay), `/hotkeys` + alt+k, `/changelog`, `/debug` (`Tui::render` is now pub),
  startup "What's New" / collapsed "Updated to v…" after the resource listing.
- Harness: env values expand `{WORK}`/`{HOME}`/`{TMP}`; new `symlinks` (with `{HOOCODE_PKG}`)
  so HOOCODE_PACKAGE_DIR can point at a seeded CHANGELOG.md while hoocode keeps its package.json
  and themes. README updated.
- Perf: first markdown render took ~1 s in dev builds (markdown rules' regexes compiled
  unoptimized), which delayed the first frame. Root `Cargo.toml` now builds regex-automata,
  regex-syntax and fancy-regex at opt-level 3 in dev: ~0.13 s.
- L2 (new, stable, pass): info-commands (whole /hotkeys page via scrollback), changelog-command,
  changelog-startup, changelog-startup-collapsed. Full L2: only the known 11 fail.
- Next: `ledger.py next` (11.4e2 /chrome /color).

### 2026-09-29 · 11.4d2 image paste; 11.4d closed
- `cortexcode-code-media::clipboard_image` (readClipboardImage behind `ClipboardImageHost`:
  wl-paste → xclip on Wayland/WSL, PowerShell on WSL, native on X11/macOS/Windows; non-model
  formats such as BMP re-encoded to PNG via `image`, bmp feature on). Ported
  clipboard-image.test.ts + clipboard-image-bmp-conversion.test.ts.
- App: ctrl+v (`app.clipboard.pasteImage`) reads off the UI thread, writes
  `$TMPDIR/cortexcode-clipboard-<uuid>.<ext>` and inserts the path at the cursor. arboard
  (image-data) is now a plain dep of code-tui-app; RGBA → PNG via `rgba_to_png`. Shares the one
  `image` 0.25 in the tree.
- L2 `paste-image-empty` (no clipboard in tmux: ctrl+v leaves the prompt alone): stable, pass.
  A real image paste can't be exercised in the harness.
- Next: `ledger.py next` (11.4e remaining slash commands).

### 2026-09-29 · 11.4d split; 11.4d1 /copy
- 11.4d split into 11.4d1 (text copy) and 11.4d2 (image paste: clipboard-image*.ts + tests).
- `cortexcode-code-media`: `clipboard` (copyToClipboard behind a `ClipboardHost` trait:
  native off Linux, pbcopy/clip/termux/wl-copy/xclip/xsel, OSC 52 when remote or nothing else
  worked), `rich_clipboard` (JXA / PowerShell CF_HTML, `wrap_cf_html`), `markdown_to_html`
  (fancy-regex, pin's JS patterns). Ported clipboard.test.ts + copy-structure.test.ts, plus
  golden outputs recorded from the pin's dist `markdownToHtml`.
- App: `/copy [all|n]` and `app.clipboard.copyMessage` (write off the UI thread, flavour on the
  band). arboard (default-features off) is a non-Linux target dep of code-tui-app, like the pin
  which skips the native addon on Linux; its snippet was `cargo check`ed for x86_64-apple-darwin.
- Parity fix: `show_error` is now hoocode's filled "Error: ..." block (`show_block`), with the
  long-retry-delay hint. Full L2: only the known 11 fail.
- L2 `copy-command` (new): stable, passes. Next: `ledger.py next` (11.4d2 image paste).

### 2026-09-29 · 11.4c ! and !! bash
- `interactive_mode.rs`: bash mode (`onChange` → `!` prompt prefix + bash-mode border; Escape
  aborts a running command, else clears a bash-mode prompt), `handle_bash_command` runs
  `AgentSession::execute_bash` on a thread and streams `AppEvent::BashChunk`/`BashDone` into a
  `BashExecutionComponent`; rows started while streaming wait in the pending container and move
  to the chat on the next idle submit; history renders `bashExecution` messages; the ctrl+o
  sweep expands `!` rows; `show_warning` (notification band).
- Not ported: the `user_bash` extension hook (waits on 12.3, deferred).
- New L2 `bash-command` (added as 11.4c's gate): bash mode, `!echo`, `!!printf`, Escape, and a
  follow-up prompt whose request carries only the `!` output. Stable, passes.
- One `cargo test --workspace -q` run exited 101 without a visible failing test; three reruns
  were clean. If it recurs, capture the full log.
- Next: `python3 migration/ledger.py next` (11.4d clipboard).

### 2026-09-29 · 11.3f, 11.3 closed; 11.4b @file autocomplete
- User decision: phase 12 stays deferred (noted on 12.1..12.7); skip it and keep going.
- 11.3f and 11.3 were containers with all subtasks done: `start` + `verify` only.
- 11.4b: `setup_autocomplete_provider` passes `get_tool_path("fd")` (override, managed copy,
  `fd`/`fdfind` on PATH) to `CombinedAutocompleteProvider`. Downloading fd is not ported.
- L2 `file-autocomplete` ('@rs' menu, Down, Tab): needs fd on the host (`apt-get install
  fd-find`; `setup_hoocode.sh` warns when missing). Stable, passes.
- Next: `python3 migration/ledger.py next` (11.4c `!` bash).

### 2026-09-28 · 11.3f2 /login and /logout
- `crates/cortexcode-code-tui-app/src/login_controller.rs`: provider option lists, post-login
  default-model pick, `OAuthBridge` (OAuth callbacks → `AppEvent::Login` updates answered through
  oneshots; a dropped answer = "Login cancelled"), `open_url`/`is_openable_url` (open-url.ts).
- The mode drives a `LoginStep` machine: auth-type pane → provider pane (Esc goes back) → API-key
  dialog or an OAuth login spawned on the runtime (prompt / pasted redirect URL / onSelect pane).
- `provider_display_name` / `provider_auth_status` (registry lookups) live in
  `cortexcode-code-auth::provider_display_names`; `ModelRegistry::provider_api_key_config` added.
- The CLI now shares one `AuthStorage` between the session runtime and the app
  (`InteractiveOptions::auth_storage`), so a saved key is visible to model availability.
- Adaptations: no `modelRegistry.refresh()` after login (Arc registry; availability reads the store
  live); extension-registered provider names wait on `registerProvider`.
- L2 `login-api-key` written from hoocode, stable, passes. Next: `python3 migration/ledger.py next`.

### 2026-09-28 · 11.3d settings selector (parent)
- No code: 11.3d1/11.3d2/11.3d3 are done and 11.3d has no scenario of its own; `ledger.py verify 11.3d` passed L1.
- Next: `python3 migration/ledger.py next`.

### 2026-09-28: 11.3d2 done (/settings in the app)
- `/settings` and alt+s (`app.settings.open`) open the pane built from the live session
  (`show_settings_selector`); every `SettingsChange` is applied (`apply_settings_change`):
  settings writes plus the live effects (active tools, footer, editor border/padding/
  autocomplete, hardware cursor, clear-on-shrink, theme + preview, thinking level, transport
  via new `Agent::set_transport`, platforms, images on tool blocks, hidden thinking rebuild).
- Changes are queued by the pane's callback and applied in the poll loop, so the pane's own
  re-price ran too early: new `SettingsSelectorComponent::refresh_token_surface()`, called
  after a batch is applied.
- `code-tools::external_tools::describe_external_tools` (+ `get_tool_path`/`get_tool_status`,
  the never-downloads half of tools-manager.ts; PATH lookup instead of spawning `--version`).
  Ported the skipped "resolves live status" test. `--offline` now sets `CORTEX_OFFLINE=1`
  process-wide (main.ts), read by `cortexcode_code_paths::is_offline_mode()`.
- Not live yet: extension flags (12.3: the pane lists none), voice silence (voice not ported).
- L2 `settings-pane` (open, category, turn bash off → surface re-priced, back, close): stable, pass.
  Full L2: the known 11, plus `print-error` once under load (snapshot taken before the exit;
  passes on rerun, scenario untouched).
- Next: `ledger.py next`.

### 2026-09-28: 11.3b done (model pickers, /model, cycling)
- `code-tui-selectors`: `model_selector` (`ModelSelectorComponent`, all/scoped tabs, events
  instead of callbacks; the owner hands in the loaded models) and `scoped_models_selector`
  (`ScopedModelsSelectorComponent` + the enabled-set helpers `toggle`/`enable_all`/`clear_all`/
  `move_id`). Tests: `tests/model_selector.rs` (port of regression 3217 + picker behaviour).
- App (model-controller.ts): `/model [ref]` (exact match switches, else the picker searching
  for it), `/scoped-models` (session-only scope; alt+s persists `enabledModels`), alt+m /
  shift+alt+m cycling with the dial note, `app.model.select`, the footer's available-provider
  count (startup, rebind, scope change), and the once-per-session Anthropic extra-usage notice
  (`show_notice`, a warningBg block; `AgentSession::uses_anthropic_subscription_auth`).
- Adaptation: the registry is `Arc` without interior mutability, so the pickers don't
  `refresh()` models.json first; the pick's default-model save happens in `set_model`.
- L2 `model-selector` written from hoocode, selfcheck stable, pass. Full L2: only the known 11 fail.
- Next: `ledger.py next` → 11.3d2 (/settings in the app).

### 2026-09-28: 11.4 split; 11.4a done (slash command dispatch)
- 11.4 split into:
  - 11.4a: dispatch, slash autocomplete, and the commands whose UI exists.
  - 11.4b: `@file`.
  - 11.4c: `!` bash.
  - 11.4d: clipboard, `/copy`, paste.
  - 11.4e: the remaining commands.
  - 11.4f: the compaction UI (it had no owner). `compact-command` moved there.
- 11.3b, 11.3d2 and 11.3f2 now depend on 11.4a.
- Submit routes built-in `/commands` the way `createBuiltInSlashCommands` does. The autocomplete
  provider lists the built-ins, then prompt templates, then skill commands (with u/p/t source tags).
  It is reinstalled on a session swap. There is no fd yet (11.4b), and templates carry no
  argument hints.
- Wired commands: `/quit`, `/resume`, `/tree`, `/name`, `/session`, `/new`, `/compact` (it
  dispatches, but nothing draws it until 11.4f). Every other built-in shows "/x is not available
  yet" until its task lands.
- L2 `slash-commands` passes and selfcheck is stable. It uses `/sess` narrowing plus `/session`.
  The unfiltered menu is 39 entries vs 25: hoocode's core extensions add commands (12.3).
- Seen once: `footer_data_provider` debounce test failed under full `cargo test --workspace`
  load, then passed 4 of 4 alone. It is a file-watch timing test and was not touched here.
- Next: `ledger.py next`.

### 2026-09-28: 11.3f split; 11.3f1 done (login components)
- 11.3f split into 11.3f1 (components) and 11.3f2 (`/login` and `/logout` LoginController flows,
  L2 `login-api-key`, after 11.4).
- `code-tui-selectors::oauth_selector`: `OAuthSelectorComponent` (searchable provider list with
  each provider's auth state; `take_events`) and `is_api_key_login_provider`.
  `login_dialog::LoginDialogComponent`: prompts arm the input and the answer comes out as an event.
  It never opens the browser itself; the owner calls the URL opener (11.3f2). Escape aborts its
  `AbortSignal`.
- `code-auth::provider_display_names`. `SelectedRowList` moved to code-tui-widgets.
- Tests: oauth-selector.test.ts (6) plus a login-dialog input case.
- 11.3f2 needs `ModelRegistry` `getProviderDisplayName` and `getProviderAuthStatus` (registered
  providers, models.json apiKey), plus `utils/open-url.ts`.
- Next: `ledger.py next`.

### 2026-09-28: 11.3e2 done (session tree in the app); 11.3e closed
- A double escape on an empty, idle prompt opens the tree (`doubleEscapeAction` = tree). A label
  edit appends a label entry. Select asks "Summarize branch?" through the extension selector:
  - No summary: `navigate_tree` runs in place.
  - Summarize, or custom: it runs on the runtime behind a "Summarizing branch..." loader that
    escape aborts. Custom instructions come from the new `extension_editor.rs` dialog
    (no `$EDITOR` hand-off).
  - Escape on the question returns to the tree on the same entry.
- After navigating, the transcript is rebuilt from the session, and the message text goes back in
  the prompt.
- L2 `session-tree` passes and selfcheck is stable, with requests compared.
- Left: `doubleEscapeAction` = fork and `/tree` both wait on 11.4.
- Next: `ledger.py next`.

### 2026-09-28: 11.3e split; 11.3e1 done (session tree component)
- `code-tui-selectors::tree_selector`: `TreeSelectorComponent::new(tree, leaf, rows, initial,
  filter)`. Entries are read as their JSON wire form so the flatten, filter, search, fold and
  display rules match tree-selector.ts line for line. Keys come out as `TreeEvent`s (`Select`,
  `Cancel`, `LabelChange`) through `poll(now)`, which also fires the empty tree's 100ms
  auto-cancel. The label editor replaces the tree inside the frame.
- `DynamicBorder` moved to code-tui-widgets (re-exported from `extension_selector`).
- Tests: tree-selector.test.ts (17) plus the tree case of picker-widths.
- 11.3e2 (wiring) is next or later. It covers double-escape now, `/tree` after 11.4,
  "Summarize branch?", navigate_tree, the custom-prompt editor (extension-editor.ts, unported)
  and the L2 `session-tree` scenario.

### 2026-09-28: 11.3d3 done (`cortex config`)
- `code-tui-selectors::config_selector`: `build_groups` and `ConfigSelectorComponent`
  (header, groups by origin/scope/source, type subheads, `[x]` rows, filter, pageUp/pageDown).
  A toggle writes `+pattern`/`-pattern` to the scope's resource array, or to the package entry's
  filter. Group sort uses a V8-style binary insertion sort because the original comparator is not
  a total order.
- `code-tui-app::session_picker::select_config` runs it on its own TUI. code-cli dispatches
  `config` (off the not-supported list). It lists local resources only; packages wait on 12.2.
- Fix in `cortexcode-tui-terminal`: a lone Escape was never delivered because nothing called
  `StdinBuffer::poll_timeout`. `ProcessTerminal` now flushes a pending partial sequence after the
  buffer timeout. No earlier scenario pressed Escape.
- L2 `config-selector` passes and selfcheck is stable (it also compares the project settings.json
  the toggle writes). Rust tests: `tests/config_selector.rs` (hoocode has none).
- Next: `ledger.py next`.

### 2026-09-28: 11.3d split; 11.3d1 done (the /settings pane component)
- 11.3d split into 11.3d1 (pane component + tests), 11.3d2 (/settings in the app, L2
  `settings-pane`, depends on 11.4 because nothing dispatches slash commands yet) and 11.3d3
  (`config-selector.ts`, the `config` subcommand's resource TUI).
- `code-tui-selectors::settings_selector`: `SettingsSelectorComponent::new(SettingsConfig,
  FnMut(SettingsChange))`. The TS callbacks are one `SettingsChange` enum. Leaves live in a shared
  table so a category rebuilds its rows from current values. `ListSubmenu`/`submenu_list` let a
  caller reach the list a submenu factory built.
- New: `code-tools::external_tools` (catalog, `status_label`, `build_row_gates`; no
  `describeExternalTools` until tools-manager is ported) and `code-settings::platform_targets`
  (+ `MarketplacePlatform`).
- `SettingsList` gained `items()`, `filtered_items()`, `emit_change()`, `emit_cancel()` and a
  public `apply_filter`. A submenu opened from a filtered list now writes back to the right row.
  `Component` gained `as_any()`.
- Tests: settings-token-surface, learn-settings-pane, platform-settings-pane,
  plugin-settings-keyboard, external-tools-pane (minus its live-status case), and the
  platform-targets part of platform.test.ts.
- Pane text says `cortex` / `.cortexcode` where hoocode says `hoocode` / `.hoocode`; the static
  external-tools prose is verbatim. The 11.3d2 scenario will need a branding rule for that.
- Next: `ledger.py next`.

### 2026-09-28: 11.3c done (small selectors, ask_options pane)
- `code-tui-selectors`: `small_selectors` (thinking, theme, show-images, session colour, each a
  `FramedSelectList`: a `SelectList` in the prompt's `InputFrame`), `user_message_selector`
  (fork picker; with no messages it auto-cancels after 100ms via `poll`), and `ask_options`
  (the options pane: steps, breadcrumb, quick-pick, custom row with arrow hand-off).
- Tests: ask-options.test.ts (10) and picker-widths.test.ts, except the tree case (11.3e).
  The extension picker's width case is in code-tui-app `tests/extension_selector.rs`.
  ask-options-loop.test.ts stays with 10.2f/12.5 (the /loop extension).
- `session_chip` moved to code-tui-widgets (re-exported from code-tui-app).
  `AskQuestion` gained `short`; the tool leaves it unset, as the pin does.
- The ask_options tool uses `dialog_bridge::TuiAskOptionsHost` in interactive mode
  (`DialogRequest::AskOptions` / `HideAskOptions` on abort). The pane takes the editor's slot.
  The new L2 scenario `ask-options` passes and selfcheck is stable (it compares screens only,
  since the default-bundle system prompt still differs, see 10.4c).
- Not wired yet: the thinking, theme, images, colour and fork pickers are opened by slash
  commands and /settings (11.4/11.3d). The chime on a blocked ask is not ported.
- Next: `ledger.py next`.

### 2026-09-28: 11.3a2 done (in-app resume, missing-cwd prompts); 11.3a closed
- The interactive mode now owns an `AgentSessionRuntime` (`InteractiveOptions::session_runtime`).
  The CLI's `build_session_runtime` is shared with rpc mode. `alt+h` (`app.session.resume`)
  opens the session selector in the editor slot (rename works through `append_session_info`).
  Enter calls `switch_session`, then `rebind_current_session` (new subscription, footer
  source, footer cwd, chip, title), then `render_current_session_state` (transcript reset,
  resource listing, initial messages).
- Missing cwd: at startup (interactive), the Continue/Cancel selector runs on its own TUI
  (`session_picker::prompt_for_missing_session_cwd`, which replaces the stdin `[y/N]`
  placeholder). In the app, the Yes/No confirm goes through `show_selector`, and
  `poll_cwd_prompt` resumes with the fallback cwd or shows "Resume cancelled".
- `show_status` is now hoocode's `showStatus`: the notification band, which fades after 3s.
  The old chat-line version is `show_record` (`showRecord`). "Session compacted N times" and
  "Current model does not support thinking" moved to the band, as they are in the pin.
- The `listing` callback takes the session (`Fn(&AgentSession)`), so it follows a swap.
- New L2 scenarios `session-resume-inapp` and `session-missing-cwd` pass, and selfcheck is
  stable. The in-app missing-cwd confirm has no L2: a scenario cannot seed a session under
  the sessions root.
- `run all` shows the same failure set as before, all belonging to l1_done tasks.
- Next: `ledger.py next` (11.3c, the small selectors).

### 2026-09-28: 11.3a split; 11.3a1 done (session selector, `--resume`, initial transcript)
- 11.3a is now a container: 11.3a1 (done) and 11.3a2 (todo: in-app `alt+h` resume via
  `switch_session` + `renderCurrentSessionState`, the missing-session-cwd Continue/Cancel
  selector, L2 `session-resume-inapp`, not written yet).
- The four session-selector TS test files are ported (`code-tui-selectors/tests`, 24 tests).
  The pin's `flushPromises` is `SessionSelectorComponent::poll`, and the list's callbacks are
  the `ListEvent`s that `handle_key` returns.
- `code-tui-app::session_picker` is `cli/session-picker.ts`: the loaders run on a thread and
  report through `LoadSink`. The CLI's `--resume` opens it (it is no longer in
  `unsupported_flags`), and "No session selected" exits 0.
- The interactive mode now renders the loaded session after the resource listing
  (`render_session_context` / `render_initial_messages`: tool calls with results, chain
  boundaries, "Session compacted N times", and editor history).
- `tui-terminal`: stdin is read by one process-wide reader (`stdin_hub`) that hands chunks to
  whichever terminal is started. The old per-terminal reader outlived `stop()` and swallowed
  the next terminal's first keys, which broke the picker → interactive-mode handoff.
- L2 `session-resume` passes and selfcheck is stable (local normalization covers the
  random slug and swatch). A full `harness.py run all` shows no regressions: every failure
  belongs to an l1_done task already waiting on L2.
- Next: 11.3a2 (see its ledger card). `AgentSessionRuntime::switch_session` exists in
  code-agent-session. The mode holds a bare `AgentSession`, so the swap needs a resubscribe
  and a transcript reset (`resetTranscriptView`).

### 2026-09-27: 11.2d2 done (code highlighting: highlight.js 10.7.3 port)
- New crate `cortexcode-tui-highlight`: highlight.js 10.7.3's engine ported over its own
  grammars, which `migration/tools/goldens/hljs-grammars.mjs` dumps as an object graph
  (identity, frozen flags, named callbacks) to `data/hljs-grammars.json`. The engine mutates
  them as hljs does, so each thread keeps its own copy.
- `highlight` colors like cli-highlight (caller theme, then its `DEFAULT_THEME` with chalk 4
  nesting). `code-tui-theme` installs it as the default `CodeHighlighter` (`CliHighlight`).
- Golden: `highlight.mjs` -> `tests/fixtures/highlight-gold.json`, 435 cases (pin and
  cortexcode sources, snippets, a polyglot snippet in all 191 languages), byte-identical.
  The debug-mode test takes about 40s; release highlights about 1,900 lines of TS in about 100ms.
- `js_regex` moved to `cortexcode-tui-util`. Multiline `^`/`$` now also treat `\r`, U+2028
  and U+2029 as line ends, as JS does (CRLF files).
- `needs_highlighter` has been removed from `tool_renderers_gold.rs`.
- 11.2d (container) done. 11.2 done: L2 chat-basic, tool-read, tool-bash and the new
  `tool-edit` pass (edit diff and write preview, both allowed through the permission prompt;
  selfcheck stable).
- Next: `ledger.py next` (11.3 selectors).

### 2026-09-28: 11.3 split into 11.3a..f; 11.3a in progress (session selector)
- 11.3 is now a container: 11.3a session selector (L2 `session-resume`), 11.3b model selector
  (L2 `model-selector`; needs `/model` from 11.4), 11.3c small selectors, 11.3d settings,
  11.3e tree, 11.3f login.
- 11.3a WIP: new crate `cortexcode-code-tui-selectors` with the session search and picker
  components. It builds and passes clippy, but has no tests or app wiring yet. See the 11.3a
  ledger note for the exact next steps.

### 2026-09-27: 11.2d split; 11.2d1 done (bash/diff/edit; L2 `tool-bash`); 10.6 done
- 11.2d is split (bookkeeping): 11.2d1 covers bash, diff, edit and bash-execution;
  11.2d2 covers code highlighting (highlightCode); 11.2d closes after both.
- `jsdiff`: a literal port of jsdiff 8's Myers core and `diffWords` (tokenizer and
  whitespace dedupe). `diff::render_diff` is diff.ts. Goldens from the real
  jsdiff/renderDiff (`migration/tools/goldens/diff.mjs`: 1210 word pairs, 66 diffs)
  match exactly.
- `tools::bash`: `$ cmd` with the timeout suffix; a peek of the last visual lines;
  truncation and full-output notes; `Took`/`Elapsed`. Bash sets a `ticking` state flag
  while partial, and the mode invalidates ticking blocks once a second, as the pin's
  `setInterval` does.
- `tools::edit` is a framed call whose header band is tinted by the preview (pending,
  success or error), plus a diff or error body; the result slot shows only what the
  preview did not. `ToolRenderContext.objects` gives both slots a shared Rust object
  (the pin keeps the call component in `state`). The preview is computed synchronously
  (the pin computes it in the background and re-renders), so only the settled state
  matches: the golden generator waits for the preview.
- `bash_execution`: the `!` command block (it gets wired in with `!` input in 11.4).
- Goldens: `tool-renderers.mjs` now covers bash and edit; the Took/Elapsed durations are
  compared as a placeholder. The edit case of tool-execution-component.test.ts is ported.
- With the bash renderer in place, 10.6's permission-prompt L2 passes, so 10.6 is done.
- Next: 11.2d2 (highlighting).

### 2026-09-27: 11.2e added and done (selector dialog; permission prompt on the TUI)
- 11.2e was added (bookkeeping) because tool-bash (11.2d) and permission-prompt (10.6)
  both need the gate's "Allow: …" prompt on the TUI.
- `code-tui-app::extension_selector`: `ExtensionSelectorComponent` (an InputFrame with the
  title in the border, select keys plus `j`/`k`, and hints), `SelectedRowList`,
  `DynamicBorder` and `CountdownTimer` (driven by `poll`, not a timer thread).
- `dialog_bridge`: `TuiPermissionUi` sends `DialogRequest`s to the running mode through
  a global sink and blocks on the reply. The mode swaps the selector into the editor
  slot (`showSelector`/`hideSelector`/`restoreEditor`); `notify` is `showStatus`.
- code-cli drops the crossterm `TerminalPermissionUi` and its crossterm dependency. The
  dep firewall's last `pending` entry is gone.
- permission-prompt now differs from hoocode only in the bash block (`$ ls` call line,
  `Took` line), which 11.2d ports; 10.6's L2 should pass after that.
- Next: 11.2d.

### 2026-09-27: 11.2c2 done; 11.2c done (remaining tool renderers)
- New `tools::{web, subagent, plugins}` renderers: webfetch/websearch (with the token,
  truncation and outline/match notes), Task (`Agent [type]` in the agent's colour),
  TaskOutput (status card with inbox elapsed time and task-store tokens, or the coloured
  roster), and the four plugin tools that render their results.
- Registered tools get renderers from `tools::registered_tool_definition`; the mode uses
  it for any tool the session knows. Canvas has no renderers of its own in the pin.
- `tests/tool_renderers_gold.rs` compares against the pin's real renderCall/renderResult
  output, rendered at 120 columns: 30 calls, each at both expand settings (generator
  `migration/tools/goldens/tool-renderers.mjs`). Two expanded read results on `.md`
  paths are exempt until the highlighter lands; 11.2d's ledger notes say to remove that
  exemption.
- Next: 11.2d (bash/diff/highlight; L2 tool-bash).

### 2026-09-27: 11.2c split; 11.2c1 done (tool blocks; L2 `tool-read` passes)
- 11.2c is split (bookkeeping). 11.2c1 is the tool-block framework plus the read, write
  and SearchCodebase renderers. 11.2c2 is the remaining built-in renderers (webfetch,
  websearch, subagent, canvas, plugins). 11.2c closes when both are done.
- `cortexcode-code-tui-widgets` gains:
  - `tool_execution`: renderer slots with built-in/registered inheritance, the
    status-dot prefix, peek-budget fallbacks, the radar row, images and freeze.
  - `tool_signal`, `tool_chain`, `tool_chain_summary`, `tool_output_view`,
    `read_output`, `visual_truncate` and `render_utils`.
  - `tools::{read, write, search}`.
- Renderers are `ToolRenderDefinition` closures (`Result<ComponentHandle, _>`), and
  renderer state is a JSON map shared by the call and result slots. Renderers build a
  fresh `Text` instead of reusing `lastComponent`: the output is the same.
- The bash, edit, webfetch and websearch definitions are registered with empty slots
  (edit is `self`-shelled). They draw the fallbacks until 11.2d and 11.2c2.
- Interactive mode:
  - Tool events build blocks inside chains.
  - A chain closes when the agent speaks, and at settle (done or interrupted).
  - It marks the latest block and chain, and freezes all but the last 50 blocks.
  - The view dial (`app.view.cycle*`, `app.tools.expand` jump) is live.
  - `showDialStep` is now used for the view, chrome and thinking dials.
  - `showStatus` updates the previous status line in place.
- Tests ported: tool-execution-component (all cases except the edit-renderer one, which
  11.2d owns), tool-chain, tool-chain-summary and tool-output-view. The bash
  "initial empty partial update" case went into tool-bash's own tests.
- Next: 11.2c2 or 11.2d (bash/diff/highlight; L2 tool-bash).

### 2026-09-27: 11.2b done (turn transcript; L2 `chat-basic` passes)
- New crate `cortexcode-code-tui-widgets` with `UserMessageComponent`,
  `AssistantMessageComponent` (thinking display full/label/omit, Markdown reuse cache,
  segmented streaming above 2048 UTF-16 units, abort/error line, OSC 133 zones) and
  `segment_streaming_markdown`. Ports assistant-message, user-message and
  streaming-segmentation tests; the ledger now lists them on 11.2b.
- Interactive mode: user messages come from `message_start` as in the pin.
  - The assistant message streams with a 100 ms throttle.
  - The working loader runs in the status container (ticked by the loop).
  - The turn cost line (`showTurnCost`) prints when the prompt future resolves. That
    is the Rust stand-in for `settleRequestOnIdle`, and it runs after retries too.
  - The cost anchor is sampled inside the session subscriber at `agent_start`,
    because the UI thread sees events only after the session has already recorded
    usage.
- Fixed: submitting from the editor dropped the prompt, because the editor clears
  itself before `on_submit` and the mode re-read the empty editor. The text now
  travels in `Action::Submit(text)`.
- Tool blocks (`tool_execution_*`), chains and summaries are 11.2c/11.2d.
- Next: 11.2c (tool blocks; L2 tool-read).

### 2026-09-27: 11.2a done (markdown on a marked lexer port)
- `cortexcode-tui-components/src/markdown/` now parses with a literal port of the pinned
  marked 15 lexer (`lexer.rs`) instead of pulldown-cmark. The rules are marked's own
  compiled GFM regex sources (`rules_gen.rs`, regenerate with
  `migration/tools/goldens/marked-rules.mjs`), run on fancy-regex through a JS-to-Rust
  regex translator (`js_regex.rs`: ASCII `\d \w \b`, the JS `\s` set and `.`, literal
  `[`/`{` where JS allows them). The inline queue, token merges and markdown.ts's
  strict-strikethrough `del` are kept.
- One known divergence: masks in the emphasis scan are byte-aligned, where marked keeps
  UTF-16 lengths. They only differ for an escaped astral symbol (`\😀`), where marked's
  own mask misaligns.
- `render.rs` is a re-port of markdown.ts: style-prefix restore after inline resets,
  table sizing, per-width line cache, and a token cache.
- Tests: all 59 cases of markdown.test.ts (`tests/markdown.rs`, with TUI cell checks on the
  shared vt100 support). `tests/markdown_gold.rs` checks token streams against real marked
  (175 corpus docs plus 1500 fuzzed docs, byte-exact JSON) and renders against real
  markdown.ts (4 variants per doc). Generators: `migration/tools/goldens/markdown.mjs`
  and `markdown-fuzz.mjs`.
- Next: 11.2b (turn transcript; L2 chat-basic).

### 2026-09-27: 11.1c2 done; 11.1c done (interactive mode on the TUI, L2 `startup` passes)
- `cortexcode-code-tui-app::interactive_mode` replaces code-cli's crossterm REPL: the banner,
  the flex fill, the resource listing, notification band, prompt editor with the app's key
  dispatch (`CustomEditor`), session chip, thinking-coloured border, footer, chrome dial,
  theme watcher, startup-progress/branch rerenders. The loop drives `Tui::process_event`,
  drains session events from a channel, and polls the band's and editor's deadlines.
- code-cli: `run_interactive_mode` builds the listing from the concrete loader (kept as
  `last_resources()`) + the agent registry, and reports the semantic index (binary lookup
  only; indexing is 12.4). Headless end-to-end test with a scripted terminal.
- L2 `startup`: pass, text and style, first run. normalize.json gained `\bcortex_` ->
  `<WORDMARK>` (branding is the only banner difference).
- Placeholders until 11.2: a turn shows the user's text and the agent's final text, no
  working loader. The permission prompt still draws with crossterm (firewall pending now
  names 10.6).
- `harness.py run all`: every failing scenario belongs to a task that is `l1_done`/`todo`
  (10.2a-g, 10.4c, 10.5, 10.5b, 10.6, 9.2b, 11.2) — no done task regressed.
- Next: 11.2 (chat widgets + tui-highlight).

### 2026-09-27: 11.1c split; 11.1c1 done (interactive chrome components)
- 11.1c split into 11.1c1 (chrome components) and 11.1c2 (the mode + L2 `startup`); 11.1c
  is a container closed when both are done (bookkeeping).
- New crate `cortexcode-code-tui-app`: brand glyphs, compact wordmark, `ExpandableText`,
  the startup-progress store and embsearch progress mapping, the shared progress bar,
  session chip, footer (+ `FooterSource` trait the session will implement) and
  `FooterDataProvider` (polling git-branch watcher), chrome density (`resolve_chrome`,
  controller over `Slot`s), notification band (polled: `deadline()`/`poll()`),
  `InputFrame`, and the resource listing (`ResourceListing` in, components out).
- Goldens from the pin via `migration/tools/goldens/` (`run.sh` copies a script into the
  pin's dist and runs it with a TTY-like chalk level): footer (13 states), resource listing,
  wordmark. All matched byte for byte.
- tui-util: `truncate_to_width` now walks ANSI codes and tabs like the pin (it used to
  count escape bytes as visible and could cut a sequence in half).
- Next: 11.1c2 (the mode itself, replacing `run_interactive_mode`; L2 `startup`).

### 2026-09-27: 11.1b done (keybindings)
- New crate `cortexcode-code-tui-keybindings`: the full keyboard map (TUI + app bindings in
  hoocode's declaration order, which is also the order `keybindings.json` is written in),
  legacy-name migration, `AppKeybindingsManager` (`create` from the agent dir, `reload`,
  `install` as the global manager), the keybindings step of `migrations.ts`, and the hint
  helpers from `keybinding-hints.ts`.
- tui-keys: the TUI table is now the ordered `TUI_KEYBINDINGS` const; `KeybindingsManager` is
  `Clone`.
- Tests: keybinding-layout (every scope/dial/family rule), keybindings-migration, and a
  golden of the table, key text and migration output from the pin.
- Next: 11.1c (tui-app idle screen + L2 `startup`).

### 2026-09-27: 11.1a done (theme)
- New crate `cortexcode-code-tui-theme` (port of `modes/interactive/theme/theme.ts` + the 8
  bundled JSON themes, embedded): color-mode detection, hex/256/HSL math with JS rounding,
  WCAG contrast, chip fills (lift / magenta deepen), schema validation with typebox's exact
  messages, `Theme` (fg/bg/fill/has/…), loader (built-in, custom `themes/`, registered,
  retired names), the process-wide current theme (`theme()`, `init_theme`, `set_theme`,
  `set_theme_instance`, `on_theme_change`), custom-theme watcher, export colors, and the TUI
  hooks (`get_select_list_theme`, `get_settings_list_theme`, `get_editor_theme`,
  `get_markdown_theme`, `apply_paper_sheet`, `apply_block_fill`, `message_label`, ...).
  Hooks resolve the current theme when called, so a theme switch reaches existing components.
- Tests: goldens from the pin for every token's ANSI in both modes; theme-contrast (51),
  theme-cutout-tokens (radar-row cases wait on 11.2's tool-signal), theme-export + loader.
- Deviations (ledger note): pluggable `CodeHighlighter` until tui-highlight (11.2); polling
  watcher; built-ins have no file path.
- Next: 11.1b (keybindings).

### 2026-09-27: 11.0c done (Editor re-port at the pin)
- `components/editor.ts` re-ported from the pin instead of patching the old divergent Editor:
  `Editor::new(EditorHost{rows, request_render}, EditorTheme, EditorOptions)`; scrollOffset over
  layout lines capped at 30% of terminal rows; frame border box/rule/none via `Frame` with a
  top-border label; redo (alt+u); autocomplete visibility callback; 20ms debounce for `@`/`#`
  contexts (`autocomplete_deadline` / `poll_autocomplete`). Cursor columns are UTF-16, as in JS.
- `editor/word_wrap.rs`: UTF-16 helpers (`len16`, `slice16`), marker-aware segmentation, and
  `word_wrap_segments` with the oversized-atomic-segment and wide-grapheme (no infinite
  recursion) fixes.
- `tests/editor.rs`: editor.test.ts, 199 tests. N/A: the async "aborts active autocomplete"
  test (the Rust provider is synchronous); the visibility-announcement test is adapted to it.
- tui-keys: removed stray plain `f`/`F` jump bindings that the pin doesn't have (typing "f"
  entered jump mode and swallowed the next key).
- Next: 11.1a (theme).

### 2026-09-27: 11.0b done (renderer catch-up to the pin)
- tui-render: `FlexSpacer` (moved here from components, re-exported there) + `Tui::set_flex_spacer`
  (buffer never shorter than the screen; prompt on the floor); window-moved-back repaint; hardware
  cursor move folded into every frame's synchronized block (`\r` + hide when unfocused); the pinned
  viewport on the alternate screen (`scroll_by_lines/pages`, `scroll_to_top/live/row`, search with
  match highlight, `ScrollStatus` + `default_scroll_status`, `can_pin_scroll`, kitty image copies
  retagged per pinned frame); mouse reports consumed before listeners (wheel scrolls 3 lines, click
  opens `on_hyperlink`); `Slot`; `child_row_offsets`; Termux height exemption.
- tui-terminal: `mouse` module (SGR + X10), `mouse_reporting()` / `set_alternate_screen()` on the
  trait; ProcessTerminal enables `?1000h?1006h` unless `CORTEX_MOUSE=0`/dumb/not a tty.
- tui-images: Sixel (`encode_sixel`, host rasterizer hook, WT_SESSION detection,
  `CORTEX_IMAGE_PROTOCOL` override); `Image` saves/restores the cursor around a sixel.
- Tests: a vt100-backed `VirtualTerminal` (`crates/cortexcode-tui-render/tests/support`) stands in
  for the pin's xterm-headless one; ported screen-fill, scroll-viewport, cursor-parking,
  hyperlink-click, scroll-images, sixel, mouse.
- Next: 11.0c (Editor re-port), then 11.1a/b/c.

### 2026-09-27: 11.1 split; TUI catch-up tasks 11.0a/b/c added; 11.0a done
- 11.1 split into 11.1a (theme), 11.1b (app keybindings), 11.1c (tui-app idle screen + L2 `startup`).
- Found: the tui-* crates were ported 2026-07-20 from hoocode ~713e5dd; `packages/tui` grew +2.6K lines
  in 35 commits before the pin. New tasks: 11.0a (utils/components delta), 11.0b (tui.ts renderer
  +1.1K, terminal, mouse, terminal-image/Sixel, image), 11.0c (re-port Editor: the Rust one diverged
  from hoocode even before the delta). Diff with `git -C target/hoocode-pin diff 713e5dd HEAD -- packages/tui`.
- 11.0a done: tui-util `hyperlink_at` / `bare_url_at`, band repair in `apply_background_to_line`;
  Box paper sheets (`set_paper`, `PaperSheet`); `Frame` + `render_frame_edge` (new frame.rs);
  SelectList/SettingsList cursor + selected-row band, no 2-col right margin, `""` ellipsis fixes
  (the old port used "..."); SettingsList value_suffix/keywords; Loader ○/● pulse, single line;
  Input `❯` prompt_prefix/prompt_color; FlexSpacer; UndoStack redo; keybindings redo + unbound
  editor pageUp/Down; markdown heading level + heading_block; thread-local
  `get_keybindings()/set_keybindings()` and `Component::handle_input` on Input/SelectList/
  SettingsList/CancellableLoader. Tests ported from the pin's test files.
- Noted for 11.2: the Rust markdown is a custom AST renderer, not a port of hoocode's.
- Next: 11.0b (renderer), then 11.0c (editor), then 11.1a/b/c.

### 2026-09-27: 10.10c done (tls-ca); 10.10 complete
- `cortexcode_ai_util::tls`: `TlsSources` (argv pre-scan for `--ca-cert` / `--use-system-ca` +
  `CORTEX_CA_CERT` / `NODE_EXTRA_CA_CERTS` / `CORTEX_USE_SYSTEM_CA`), `resolve_trusted_cas`
  (first explicit source only, OS store via rustls-native-certs only when opted in, dedupe,
  warn-once `[tls] ...` and skip on failure), `configure_global_tls`, `http_client_builder()` /
  `http_client()`. Bundled roots = reqwest's webpki-roots, always kept (add_root_certificate is additive).
- Every `reqwest::Client::new()/builder()` in the workspace (11 crates) now starts from
  `http_client_builder()`; new client code must too. The CLI installs the set first thing in `main`,
  and `--ca-cert` / `--use-system-ca` are no longer "unsupported".
- Checked end to end against a local `openssl s_server` with a throwaway CA: UnknownIssuer without
  the flag, 200 with `--ca-cert`.
- L2 run-all: only scenarios of not-yet-done tasks fail (interactive TUI = phase 11, default bundle).

### 2026-09-27: 10.10b done (utils/git parseGitUrl)
- `cortexcode_code_paths::git`: `parse_git_url` / `GitSource` plus a port of hosted-git-info 9.0.3
  `fromUrl` (`hosted_git_info_from_url`: shorthand detection, correctProtocol/correctUrl, the five
  host extractors, decodeURIComponent failure = no match). The `url` crate is the same WHATWG parser
  as Node's `URL`. Quirks kept: `/tree` with no ref gives ref "undefined", a user-less gist gives
  path "null/<id>".
- Tests: git-ssh-url.test.ts + a 70-input golden corpus from the pinned build
  (`tests/fixtures/git-url-gold.json`).
- Next: 10.10c (tls-ca).

### 2026-09-27: 10.10 split; 10.10a done (exec, event-bus, format-*, output-guard)
- 10.10 split into 10.10a/b/c (parent closed as a container, noted in the ledger). Already ported
  before the split: token-budget, format-duration, mime, resolve-config-value, utils/paths, git-branch.
- code-agent-session: `format` (format_tokens, format_duration_secs moved here from code-subagents
  and re-exported there, plural/pad_cell/truncate_visible/wrap_indented/render_list/
  render_compact_rows, `js_to_fixed` with toFixed's exact-tie rounding), `exec` (execCommand:
  SIGTERM then SIGKILL after 5s, `code ?? 0`, spawn error -> code 1, 100ms stdio grace after exit),
  `event_bus` (EventBus on/emit/clear, handler errors reported, re-entrant), `output_guard`
  (explicit take_over/write_stdout/write_raw_stdout; Rust can't patch stdout, so nothing wires it yet).
- tui-util fix: `visible_width` counted every U+2600..27BF symbol (e.g. `✓`) as 2 columns; hoocode
  only widens RGI emoji. Goldens from the pinned build are in the tests.
- Next: 10.10b (utils/git parseGitUrl with hosted-git-info semantics), then 10.10c (tls-ca, threading
  the CA set into every reqwest client builder).

### 2026-09-27: 10.9f done (Task tool in sessions)
- CLI (`subagent_tools` in runtime.rs, main.ts's buildSessionOptions block): seeds
  `CORTEXCODE_SUBAGENT_MAX_DEPTH` / `CORTEXCODE_NESTED_SUBAGENT_CONCURRENCY` when unset, sets or clears
  `--delegate-allow`, registers Task + TaskOutput when not light, below the depth cap and
  `--enable-subagents` / `enableSubagent` (default on), and sets `WARM_SUBAGENTS` for a root with
  `--warm-subagents` / `warmSubagents`. TodoWrite is no longer registered inside a spawned child.
  The Task appendix (`build_task_main_prompt`) goes into the loader's append-system-prompt; skill
  paths are forwarded to both pools; the shared pool is disposed when a run ends. The five subagent
  flags are no longer "unsupported".
- AgentSession lists `<available_agents>` (the registry) when the Task tool is active.
- L2 `subagent-task`: a real foreground dispatch in print mode (child = the same binary). The child's
  model request matches hoocode byte for byte; Task/TaskOutput schemas, the roster and the appendix
  do too. Pinned hoocode never exits after such a run (lifeguard setInterval not unref'd); the
  scenario drops only a clean exit marker, commented and ledgered.
- Tests: suite/subagent-execution (remaining cases), subagent-skills, mode-subagent-appendix.

### 2026-09-27: 10.9d done (Task/TaskOutput tools); 10.9d split off 10.9f
- 10.9d was split: the tools here; session wiring (flags/settings, task-main appendix, agents roster,
  skill forwarding, exit-time pool dispose) and L2 `subagent-task` moved to the new 10.9f.
- code-subagents `tools`: `create_task_tool_definition` (cold/warm/background/resume/fork paths,
  provider-exhaustion skip, delegate scoping, unknown-agent error, task store + roster bookkeeping,
  child task-tree merge, abort -> `pool.cancel`) and `create_task_output_tool_definition` (roster,
  collect-once, running/failed status, `wait` / barrier); `build_task_main_prompt` with hoocode's
  task-*.md templates embedded; `resolve_fork_session_file`; `format_duration_secs`.
- `AgentTool` / `ToolDefinition` gain `background_when` (hoocode's `background: (toolCall) =>
  boolean`), used by the agent loop's partition; `ToolContext` gains cwd, available models and
  session file (filled by AgentSession). Struct literals updated with fix_struct_fields.py.
- Tools block on their async work via `block_in_place` + `Handle::block_on` (they run in
  spawn_blocking threads).
- hoocode quirk kept: the "partial result, resume with ..." hint keys off the pool result's status,
  which the pool only ever sets to complete/failed.
- Next: 10.9f. main.ts buildSessionOptions (enableSubagent default true, --enable-subagents /
  --no-subagents, --delegate-allow env, --warm-subagents env, --max-subagent-depth seeding),
  `buildTaskMainPrompt` as an append-system-prompt, `<available_agents>` in
  AgentSession::rebuild_system_prompt when Task is active, then L2 `subagent-task`.

### 2026-09-27: 10.9c done (warm subagent pool + inbox)
- code-subagents `warm`: `WarmSubagentWorker` (a `--mode rpc` child through `RpcClient`: prompt and
  wait, last assistant text, session-stats usage, `new_session` reset) and `WarmSubagentPool`
  (keyed per agent/model/provider, up to 2 idle per key, 30s idle reclaim, infra failures are
  `WarmWorkerError` for cold fallback), plus the shared instance and the `WARM_SUBAGENTS` env gate.
  hoocode's warm-child env "delete DEFER_MCP_SCHEMAS" is a no-op (its RpcClient spreads
  process.env back), so the Rust worker only adds env vars, like hoocode.
- code-subagents `inbox`: the notify-and-pull records for background Task/TaskOutput (labels,
  lifecycle, collect-once body, 50 settled kept, `wait_for`/`wait_for_all` on a Notify), observed
  per pool via the new `SubagentPool::id`.
- Tests: the fake RPC child is a shell port of `fixtures/fake-rpc-child.mjs` (crash knob via an arg
  instead of process env).
- Next: 10.9d: tools/subagent.ts (Task/TaskOutput on the pool, warm fallback, inbox), the agents
  roster in the system prompt, shared-pool dispose at exit, and L2 `subagent-task`.

### 2026-09-27: 10.9e done (subagent child protocol in print mode)
- `--mode json --task-id <id>`: an immediate `{"ping":true}` then one every 30s, only
  SUBAGENT_STDOUT_EVENT_TYPES events after the header, the `--max-turns` cap (wrap-up steer at
  90%, abort at the cap), and result.json (usage from session stats, task tree from the task store)
  written to the session cwd's dispatch dir; a failed result exits 1. `--max-turns` is no longer an
  "unsupported flag".
- Listener timing: hoocode's session listeners run asynchronously, one loop step behind; the
  turn-limit hook applies its steer/abort at the next `turn_start` to match (ledger note on 10.9e).
- openai-completions: an abort before the response arrives now reads "Request was aborted." (SDK
  text); mid-stream stays "Request was aborted".
- Harness: `work_files` compares files a run leaves in the workspace (`{config}` = the app's
  config dir), masked like `stdout_jsonl`; selfcheck covers them. New L2 `json-subagent-child`.
- Next: 10.9c (warm-subagent-pool.ts on RpcClient, subagent-inbox.ts).

### 2026-09-27: 10.9b done (subagent pool + lifeguard)
- code-subagents: `pool::SubagentPool` (tokio; priority FIFO, `spawn`/`wait_for`/`dispatch`/
  `dispatch_detached`/`collect`/`resume`/`cancel`/`dispose`, verified-result.json settlement,
  inherited-model retry, output.json/dispatch-log.json, `PoolEvent { name, data }` in hoocode's
  event names/payloads), `lifeguard::SubagentLifeguard` (load-scaled heartbeat + hard timeouts,
  process-group kills, 24h sweep), `instance` (shared pool + task-panel activity wiring).
  Children get `CORTEXCODE_SUBAGENT_DEPTH` (read back through any prefix).
- The invented JSON-RPC pool (`jsonrpc.rs`, `task_tool`) is gone; nothing outside the crate used it.
- Tests use shell-script mock children (hoocode's are Node). One TS test is vacuous (the priority
  test records its own await order); the Rust test checks real completion order
  (blocker, t2, t1, t3: doc and edit tie in `priorityOf`).
- Deviations, ledgered: the lifeguard does not hook SIGINT/SIGTERM itself
  (`graceful_shutdown` for the host); the shared pool's exit-time dispose lands with 10.9d.
- New task 10.9e: the child side (print-mode under `--task-id`: pings, event filter, max turns,
  result.json). 10.9d now depends on it.
- Next: 10.9e (print-mode.ts `isSubagent` branch), then 10.9c (warm pool + inbox), 10.9d.

### 2026-09-27: 10.9 split; 10.9a done (subagent foundations)
- 10.9 was ~5.3k TS lines with ~20 test files: split into 10.9a (foundations), 10.9b (cold pool +
  lifeguard), 10.9c (warm pool + inbox), 10.9d (Task/TaskOutput tools, agents roster, L2
  subagent-task). Correction to the old card: hoocode's cold pool spawns `--mode json --task-id`
  children (progress events + `{"ping":true}` heartbeats on stdout, `result.json` settles them);
  only the warm pool drives `--mode rpc` children through `RpcClient`.
- code-subagents gains `depth` (env contract; any CORTEXCODE_/CORTEX_/HOOCODE_ prefix, `SubagentEnv`
  so tests pass explicit envs), `dispatch` (DispatchEvaluator), `events` (+ `classify_subagent_line`),
  `result` (result.json build/write, task forest), `output_verifier`, `token_budget`,
  `model_categories`, `agent_log`. The invented JSON-RPC pool in lib.rs is untouched until 10.9b.
- code-agent-session gains `provider_health`; AgentSession clears exhaustion on a good response and
  marks it when a quota error outlives retries (the 10.3b ledger note).
- Next: 10.9b. Read subagent-pool.ts (1174 lines) + lifeguard.ts; tests subagent-pool*.test.ts,
  lifeguard.test.ts. The pool needs the agent registry/frontmatter (code-resources has
  agent_registry.rs) and `--mode json`'s SUBAGENT_STDOUT_EVENT_TYPES filter under `--task-id`.

### 2026-09-27: 10.8d done (RPC session commands on AgentSessionRuntime)
- `cortexcode_code_rpc::RuntimeHost`: an `RpcHost` over `AgentSessionRuntime`; new_session,
  switch_session, fork and clone replace the session and RPC mode rebinds to it.
- CLI: `build_session` split into `initial_session_manager` + `create_runtime` (main.ts's
  `createRuntime` factory; `AuthStorage` shared across runtimes, settings/models per cwd,
  `session_start_event` passed through). `--mode rpc` builds the runtime with that factory.
- `SessionError::NotFound` now reads hoocode's `Entry <id> not found` (clone on a fresh session
  fails with it in both apps: the leaf isn't on disk yet).
- Runtime callback boxes are `Send + Sync` so runtime futures can run behind a tokio mutex.
- New L2 `rpc-session` (clone/fork errors, clone, new_session, prompt after). L1
  `code-rpc/tests/runtime_host.rs` also covers switch_session and fork-before-message.
- Next: 10.9.

### 2026-09-27: 10.8e done (Rust RpcClient)
- `cortexcode_code_rpc::client::RpcClient` (rpc-client.ts): spawns `<exe> --mode rpc`, `req_<n>`
  ids, every typed command, `on_event`/`on_exit`, `wait_for_idle`, `collect_events`,
  `prompt_and_wait`, fail-fast on child exit, SIGTERM then kill on `stop`. `RpcClient::attach`
  drives any stream pair (tests run it against in-process `run_rpc_mode` on the faux provider).
- Tests: rpc-client-clone.test.ts, rpc.test.ts ported off the live Anthropic key (9).
- Next: 10.8d, then 10.9 (subagents move onto this client and drop code-subagents' jsonrpc.rs).

### 2026-09-27: 10.8c done (--mode rpc core); split off 10.8d / 10.8e
- `cortexcode-code-rpc` rewritten on hoocode's protocol: `jsonl` (LF-only framing, `\r`
  stripped, maxBuffer) and `mode` (`RpcMode::handle_line`, `run_rpc_mode`, `RpcHost`). Every
  command in rpc-types.ts is handled; long ones (prompt, compact, bash, abort, session
  switches) run in the background like hoocode's un-awaited handlers, the rest in order.
- The invented JSON-RPC 2.0 server is gone; its types live on privately in code-subagents
  (`jsonrpc.rs`) until 10.9 moves subagents onto the real protocol.
- code-agent-session: `PromptOptions::preflight_result` (hoocode's `preflightResult`),
  `get_available_models`, `ResourceLoader::slash_commands` (get_commands),
  `compaction_result_json`; stats `cost`/`percent` and `ModelCost` print as JS numbers.
- Gaps (ledgered): new_session/switch_session/fork/clone answer an error until the CLI runs
  an `AgentSessionRuntime` (10.8d); the Rust RpcClient is 10.8e; export_html (phase 12) and
  extension UI dialogs (12.3) are not there; parse-error text is serde's, not V8's.
- Harness: `wait_stdout` step. L2 `rpc-basic` (new, stable) passes.
- Tests: rpc-jsonl.test.ts, rpc-prompt-response-semantics.test.ts (+ id/unknown/parse cases).
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.8b done (--mode json event stream)
- Wire serializers: `AssistantMessageEvent::to_json` (ai-types; contentIndex, `content` on
  *_end, `toolCall` on toolcall_end, `reason` on done/error, all read from the partial),
  `AgentEvent::to_json` / `AgentToolResult::to_json` (agent-types), `AgentSessionEvent::to_json`
  (code-agent-session; queue/session-info/thinking/compaction/retry). RPC (10.8c) can reuse them.
- ai-types: `Cost` fields serialize like JS numbers (`0`, not `0.0`) and `AssistantMessage`
  fields are in the providers' wire order (`…usage, stopReason, timestamp, responseId, …`).
  Both also change what cortex writes to session files (now closer to hoocode's bytes).
- code-print: the invented `JsonEvent`/`PrintFormatter` format is gone; `json_line`.
  code-cli streams the session header + every event live (tokio select over the prompt).
- Harness: `"stdout_jsonl"` scenarios send stdout to a file and compare it line by line
  (key order kept; `mask_keys` / per-event `mask_fields`). json-basic masks message_update's
  live partial (hoocode mutates it while the event is queued: timing-dependent).
- L2 `json-basic`, `json-error` (new, stable) pass.
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.7b done (CLI session flags)
- code-cli `session_flags.rs`: `--fork`, `--session` (path / id prefix / other project with a y/N
  fork prompt), `--continue`, `--no-session`, session dir from flag / env / setting; the runtime
  runs in the session's cwd. `--tools`, `--no-tools`, `--no-builtin-tools`, `--thinking`,
  `--session-dir` are no longer rejected.
- Harness: `pre_runs` (arg lists run to completion before the recorded run).
- L2: `print-continue` (new) and `list-models` pass.
- Left: `--resume` (session picker, 11.3) and `--export` (export-html, phase 12) still error.
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.6 l1_done (code-permissions)
- New crate `cortexcode-code-permissions`: `HooPermissionGate` ports permission-gate.ts over the
  merged hoo-config.json (hard rules always; prompt / allowed_write_paths / auto_allow with a UI;
  "Always" writes the global auto_allow). Replaces the CLI's invented PolicyPermissionGate, so
  print mode no longer denies unknown tools.
- CLI: `--disallowed-tools` (+ `disabledTools` setting) is supported.
- Tests: permission-gate-mutation-path.test.ts + the hard-enforcement rules (11).
- L2 `permission-prompt` (new, stable) waits on the TUI selector (11.3).
- Left: the `.webtoolsignore` webfetch rule (10.2e).
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.5b l1_done (code-modes)
- New crate `cortexcode-code-modes` (extensions/core/{modes,config}.ts, core/mode-prompts.ts):
  hoo-config.json read/merge/write, the four mode prompts and grill prompts embedded verbatim,
  plan-file parsing (matches hoocode's multiline regex, which keeps only a section's first line),
  `/mode /plan /grill /goal /approve` returning `ModeAction`s, and `ModesExtension`.
- `ExtensionHooks::before_agent_start` added; `AgentSession::prompt` applies it (hoocode's
  emitBeforeAgentStart system-prompt override).
- code-cli installs `ModesExtension` per session (`--mode-path`, mode `enabled_tools`).
- Tests: mode-tool-filter, mode-commands, grill-command, goal-command (54).
- print-context-files: the build-mode appendix matches hoocode now; the remaining request diff
  is the agents roster (10.9) and the "About <app> itself" docs section.
- L2 `mode-plan` (new, stable) waits on the interactive TUI (11.x); nothing consumes
  `ModeAction`s yet (see the 11.2 note).
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.5 l1_done (DefaultResourceLoader, builtin skills, wiring)
- `code-resources::resource_loader` (resource-loader.ts): `DefaultResourceLoader` over the local
  resolve (10.5d): skills (+ namespaces, source info), prompt templates + slash commands (default
  dirs incl. `.claude/commands`, `.agents/commands` ancestors, dedupe collisions), context files,
  system prompt / append inputs, overrides, `extend_resources`. Extensions (12.3), themes (11.1)
  and package sources (12.2) are not ported.
- `code-resources::builtin_skills` (builtin-skills.ts): hoocode's `templates/skills` embedded
  verbatim, so the cache dir hash (965fb5cbad49) and the `<location>` match hoocode.
- `code-resources::skill_blocks` (agent-session-skills.ts): `parse_skill_block`,
  `expand_skill_command`.
- `code-agent-session::DefaultResources` implements `ResourceLoader` (skills, context files,
  `/skill:` + template expansion); `AgentSession::resource_loader()`.
- code-cli: the loader replaces `StaticResourceLoader`; `--skill`, `--no-skills`,
  `--prompt-template`, `--slash-command`, `--no-prompt-templates`, `--no-slash-commands`,
  `--no-context-files` are supported; light mode drops skills and context files.
- Tests: resource-loader.test.ts (minus extensions/themes), builtin-skills.test.ts,
  sdk-skills.test.ts, and the skill-expansion case of agent-session-prompt.test.ts.
- L2 `print-context-files` (new, stable): the context and skills sections match hoocode; the
  request still differs by the default-bundle remainder (agents roster 10.9, docs section,
  build-mode appendix 10.5b), like print-tool-read.
- Disk: the session disk allowance filled up (target/debug grew to 20G). `rm -rf target/debug`
  (never `cargo clean`: it deletes target/hoocode-pin) and prefer `CARGO_INCREMENTAL=0`.
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.5d done (package resource discovery, local resolve)
- `code-resources::package_discovery` (package-resource-discovery.ts): recursive collection with
  ignore files, skill layouts (hoocode vs `.agents`), prompt/theme/extension auto-discovery
  (package.json `hoocode`/`pi`/`cortexcode` manifests, index.ts), include/exclude/force patterns.
- `code-resources::package_resolve`: the settings-entries + auto-discovery half of
  `DefaultPackageManager.resolve()` with precedence ranks and symlink dedupe (`home` passed in
  instead of reading `$HOME` at each call). Package sources (npm/git) remain 12.2.
- Tests: 34 cases from package-manager.test.ts (resolve, skill metadata, `.agents/skills`, ignore
  files, top-level patterns, force include/exclude, multi-file extension discovery).
- Next: 10.5 (DefaultResourceLoader, builtin skills, session/CLI wiring, print-context-files).

### 2026-09-27: 10.5c done (context files, agent registry)
- `code-resources::context_files` (context-files.ts): AGENTS.md/CLAUDE.md from `~/.agents`, the
  agent dir and the cwd ancestors (root first), per-file 8K/40K and total 24K/64K budgets
  (trims least specific first), `resolve_prompt_input`.
- `code-resources::agent_registry` (agent-registry.ts + agent-manifest-paths.ts): built-ins
  embedded from hoocode `templates/agents/*.md` (branding line "running inside cortex"),
  precedence chain incl. `.claude/agents`, ancestor `.agents/agents`, explicit paths, `--agent`;
  `summarize_agent_description`, `format_agents_for_prompt` (checked byte-for-byte against the
  `<available_agents>` block in hoocode's recorded model request).
- Tests: context-files-user-scope.test.ts and agent-registry.test.ts ported.
- Next: 10.5 (DefaultResourceLoader + wiring + print-context-files).

### 2026-09-27: 10.5 split; 10.5a done (skills, prompt templates, agent frontmatter)
- Ledger: 10.5 split into 10.5a (this), 10.5c (context files + agent registry) and 10.5
  (resource-loader.ts, session/CLI wiring, print-context-files). Bookkeeping fix: the new tasks'
  `l2` was first written as a list, which `verify` reads as an unwritten scenario; it is the
  plain "n/a: ..." string like other library tasks.
- `cortexcode-code-resources` rewritten as a port (the old crate was invented): `frontmatter`
  (shares the harness parser), `source_info`, `diagnostics`, `agent_frontmatter`
  (parseAgentDefinition, normalizeTools Claude shim), `skills` (discovery with ignore files,
  plugin-root skip via the six manifest paths, `.claude/skills`, collisions, namespaces,
  `format_skills_for_prompt`), `prompt_templates` (load, parseCommandArgs, substituteArgs with
  JS `String.replace` `$$`/`$&` semantics, tryExpand), `slash_commands` (built-in list),
  `node_path` (Node path semantics over strings).
- Tests: skills.test.ts, agent-frontmatter.test.ts, prompt-templates.test.ts ported (fixtures
  copied to `crates/cortexcode-code-resources/tests/fixtures`).
- Next: 10.5c (context-files.ts, agent-registry.ts), then 10.5.

### 2026-09-27: 10.4b done (model resolver, auth storage, --list-models, --models)
- `code-models::resolver` (model-resolver.ts): `parse_model_pattern`, `resolve_model_scope`
  (globs via `glob` with minimatch options, thinking suffixes, warnings returned),
  `resolve_cli_model`, `find_initial_model` over a `ModelSource` trait, `DEFAULT_MODEL_PER_PROVIDER`,
  `locale_compare`. `ModelRegistry::set_model_modifier` runs the OAuth `modifyModels` pass;
  `resolve_config_value_cached` / `clear_config_value_cache` port the command cache.
- New `cortexcode-code-auth` (auth-storage.ts + auth-guidance.ts): `AuthStorage` over file or
  in-memory backends, hoocode-compatible `auth.json` (type-tagged entries, pretty JSON, 0600,
  `auth.json.lock` dir lock like proper-lockfile), runtime keys, fallback resolver, locked OAuth
  refresh; implements `AuthLookup`.
- code-cli: `CliAuth`/`CredentialStore` replaced by `AuthStorage` (login stores `type: oauth`);
  `--model` via `resolve_cli_model`, `--models`/`enabledModels` scope, `--api-key` as a runtime
  key for the chosen provider, main.ts diagnostics; `--list-models`. services.rs uses
  `find_initial_model`.
- Tests: `code-models/tests/model_resolver.rs` (all of model-resolver.test.ts),
  `code-auth/tests/auth_storage.rs` (all of auth-storage.test.ts), list-models unit tests.
- L2 `list-models` (mock + GROQ_API_KEY env auth) passes. Harness fix: tmux options from a
  config file, and apps run under `sh -c '"$@"; exit $?'` because tmux 3.4 sometimes never draws
  "Pane is dead" when node exits right away (~1 in 8 runs).
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.3c done (tree navigation, fork, runtime replacement)
- `code-agent-session::tree` (agent-session-tree-navigation.ts): `navigate_tree` (leaf moves,
  editor text for user/custom messages, optional branch summary at the new position, labels,
  `session_before_tree`/`session_tree` hooks), `abort_branch_summary`; branch summarization
  counts as `is_compacting` and the abort races the summary request.
- `code-agent-session::runtime` (agent-session-runtime.ts + session-cwd.ts):
  `AgentSessionRuntime` over a `RuntimeFactory` closure with `new_session`, `switch_session`,
  `fork` (before/at, persisted and in-memory), `change_directory` (pinned session dir travels),
  `import_from_jsonl`, `dispose`, rebind / before-invalidate callbacks;
  `assert_session_cwd_exists`.
- `ExtensionHooks` gained `has_handlers` + `emit_session_event(SessionEvent)`; sessions carry
  a `session_start_event` emitted by `bind_extensions()`; `AgentSession::reload()`.
- Tests: `tests/runtime.rs` (suite runtime + runtime-events + branching, faux models reach
  the registry via models.json; agent dir isolated by env), `tests/tree_navigation.rs`.
  Extension-only cases (message_end replacement, stale ctx, withSession) noted on 12.3.
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 9.2b l1_done (/compact, compaction e2e tests)
- `tests/compaction_e2e.rs` ports `test/agent-session-compaction.test.ts` (live-model e2e) onto
  the faux provider: manual compact, usable after compaction, persisted to the session file
  (reopened from disk), in-memory `--no-session`, compaction_start/end events. With
  `keepRecentTokens: 1` the cut splits the last turn, so each compact makes two summary
  requests (history + turn prefix). The suite file was ported in 10.3b. Test harness gained
  `HarnessOptions::session_manager`.
- L2 `compact-command` scenario written; `selfcheck` stable. hoocode shows the `/compact`
  slash menu, then the chat rebuilt with the `[compaction]` block (rendered twice at the pin:
  once from the rebuilt messages, once appended by `compaction_end`; port it as-is). It waits
  on the interactive app (11.1+), so 9.2b is `l1_done`.
- Stopgap interactive loop accepts `/compact [instructions]` until 11.1 replaces it.
- Next: `python3 migration/ledger.py next`.

### 2026-09-27: 10.3b done (AgentSession retry + auto-compaction)
- `code-agent-session::retry` (agent-session-retry.ts): retry is armed synchronously on
  `agent_end` (a pending flag + `Notify`, waited on by `prompt()`), exponential backoff with an
  abortable sleep (the abort signal is installed before `auto_retry_start` so a listener can
  cancel), the error dropped from the context, then `continue()` once the agent is idle.
- `code-agent-session::compaction` (agent-session-compaction.ts): manual `compact()` (abort,
  disconnect, summarize through the API registry, persist, reload context), `plan_compaction`
  (the decision half of `checkCompaction`: one-shot overflow recovery, threshold with the
  error-message usage estimate, stale pre-compaction guards) and `run_auto_compaction` (retry
  the turn after overflow, or kick queued messages, after 100ms).
- The `agent_end` tail runs as a spawned task (TS's async event queue); the pre-prompt check
  runs in `prompt()`. New events: CompactionStart/End, AutoRetryStart/End.
- Tests: 13 retry/event-order + 12 compaction (TS spies on `_runAutoCompaction` become
  `plan_compaction` assertions; extension-hook cases noted on 12.3). L2 print-retry passes.

### 2026-09-26: 10.3a done (AgentSession core)
- New crate `cortexcode-code-agent-session`: `session.rs` (agent-session.ts core: event
  processing with session persistence on `message_end`, queue bookkeeping removed before
  listeners see `message_start`, prompt/steer/followUp/sendCustomMessage/sendUserMessage,
  abort, setModel/cycleModel (scoped + available), thinking level set/cycle/clamp, tool
  registry with allow/deny lists and SDK tools, `_rebuildSystemPrompt`, `prepareNextTurn`
  refresh, executeBash/recordBashResult with deferred flush, session name/colour, stats,
  JSONL export, dispose -> `cleanup_session_resources`), `stats.rs` (agent-session-stats.ts),
  `services.rs` (agent-session-services.ts + sdk.ts `createAgentSession`: model/thinking
  restore, harness `convert_to_llm` + blockImages, context GC transform, per-request auth and
  headers from the model registry, OpenRouter attribution, session id on the agent),
  `hooks.rs` (`ResourceLoader` and `ExtensionHooks` traits until 10.5 / 12.3),
  `auth_guidance.rs`.
- Tests: 49 ported (suite/agent-session-prompt, -queue, -bash-persistence; the model/thinking
  cases of suite/agent-session-model-extension; agent-session-stats; the SDK-tool case of
  agent-session-dynamic-tools; the non-extension agent-session-concurrent cases) plus registry,
  session-info, export and tool-context checks. Extension-handler cases are noted on 12.3,
  runtime-events/branching on 10.3c, skill expansion on 10.5.
- code-cli: `build_session` replaces `build_agent_with_gate`/`LiveTranscript`; tools see the
  real session branch. Light mode is hoocode's allowlist `[read, write, edit, bash]` over the
  light override (active order now matches). `--no-session`, `--session-dir`, `--thinking`
  wired; sessions persist under `sessions_dir()` otherwise, as in hoocode.
- The 8.2 note is resolved: both sides now send `prompt_cache_key`.
- Also: `code-session` gained session identity (session-identity.ts: slug, colour slot, colour
  names; `append_session_info(name, color)`; `session_name` skips colour-only entries) and
  records `Header.branch` via the new `code-paths::git_branch` (git-branch.ts);
  `code-paths::{package_dir, docs_path}`; `Agent::with_state`; `AuthLookup::is_oauth`.
- Default-bundle scenarios (print-tool-search, print-todo-write, print-tool-read-paging) are
  unchanged: identical tools and messages, only the system prompt differs (10.4c).

### 2026-09-26: 10.3 split into 10.3a/b/c
- 10.3 (AgentSession: ~4.5K source + ~2.8K test lines) is now 10.3a core (agent-session.ts
  core, services, stats; print mode on AgentSession; L2 guards print-basic and
  print-tool-read-light), 10.3b retry + auto-compaction (L2 print-retry; 9.2b now depends on
  it), and 10.3c tree navigation, fork and runtime rebuild (L2 n/a). Tasks that depended on
  10.3 now depend on 10.3a (10.7b on 10.3a + 10.3c).

### 2026-09-26: 10.2g l1_done (context GC)
- `code-tools-fs::context_gc` (context-gc.ts): superseded-read stubs (later successful
  edit/write, or a later overlapping read; disjoint ranges coexist; dedup pointers neither
  supersede nor get stubbed; failed edits never evict) and pressure-gated bash eviction (>= 0.6
  for outputs over 2000 UTF-16 units, >= 0.8 always; side-effecting commands never), returning
  `None` when nothing changed. `BudgetPressureLatch` is sdk.ts's high-water latch.
- CLI: `transform_context` runs it when `contextGc.enabled`, with pressure from
  `estimate_context_tokens` over the model's context window (the extension `context` hook part
  of sdk.ts's transformContext waits for the extension runtime).
- Tests: context-gc.test.ts, read-dedup.test.ts's GC x pointer cases, the full 258-sequence
  read/edit/GC matrix, and the recovery-loop GC case from 10.2c (8 tests). The tests give
  every tool call a process-unique id: read-dedup's content stamps are keyed by call id in a
  process-wide map, and Rust runs tests in parallel (vitest runs a file sequentially), which
  made two tests that both used `call-1` flaky. edit_write.rs's harness got the same fix.
- L2: `print-tool-read-paging` now matches hoocode in every tool result, superseded stubs
  included; only the default-bundle system prompt differs → 10.4c.

### 2026-09-26: 10.2f l1_done (TodoWrite, ask_options, task store)
- New crate `cortexcode-code-task-store` (task-store.ts): tasks + owning agents, batched change
  notifications (listeners run outside the lock), version counter, create/update (clearable
  note)/remove/arrange/reset/clear, and the process-wide `task_store()`. Ported here because
  TodoWrite writes it; 11.5 renders it (ledger notes updated; task-store.ts added to 10.2f's
  sources).
- New crate `cortexcode-code-tools-optin`: TodoWrite (reconcile by item content, then leftover
  slots, drop the tail, keep list order; glyph lines; counts in details) with
  `settle_dangling_main_tasks`; ask_options behind an `AskOptionsHost` (has_ui, the pane,
  `/loop` state + halt); `NoUi` gives hoocode's print-mode text. Exact schemas and texts taken
  from hoocode's dist.
- CLI: registers ask_options always and TodoWrite when `--enable-todowrite` / `enableTodoWrite`
  (default true), after the five base tools, as hoocode orders extension and custom tools;
  the permission gate treats both as read-only. The default prompt's tool list now differs
  from hoocode's only by SearchHooCode, Task and TaskOutput.
- Tests: todo-tool.test.ts and the loop cases of ask-options-loop.test.ts ported, plus
  task-store/identity-reorder/settle cases (8 tests). ask-options.test.ts is the pane (11.3);
  the `/loop auto` case is noted on 12.5 (the loop extension). Bookkeeping: my first note on
  10.2f named 7.5a as the loop's owner; corrected to 12.5 before committing.
- L2: new `print-todo-write` (stable; tool results identical incl. ask_options' no-UI text;
  only the system prompt differs → 10.4c) and `todo-write` (interactive, phase 11).

### 2026-09-26: 10.2e blocked on a design question (webtools)
- hoocode's webfetch/websearch do no HTML work themselves: they run the external `webtools`
  binary (kolisachint/webtools, Rust, downloaded by tools-manager) and format its `--json`
  output. The design doc planned in-process `htmd` + `dom_smoothie`, which can only
  approximate that binary; its source was not readable here (add_repo was denied). Asked the
  user to pick: (A) port the tool layer exactly and run the same binary, (B) in-process as
  planned, (C) depend on webtools as a library. Continuing with 10.2f meanwhile.

### 2026-09-26: 10.2d l1_done (SearchCodebase); default bundle is now hoocode's five tools
- New crate `cortexcode-code-tool-search` (tools/search.ts + the runtime half of core/search/):
  query plan, mode resolution, lexical retriever, grep→chunk adapter, RRF, deterministic
  reranker (IDF, path affinity, declaration bonus, prose gate), stale hoist, span merge,
  token-budgeted assembler, jsonl trace (`<agentDir>/embsearch/<sha256[..16]>`), cross-encoder
  rerank, and an `EmbsearchService` trait for 12.4's daemon client (without one every mode is
  lexical, with hoocode's degradation texts).
- The lexical leg reproduces hoocode's `rg --json --hidden --no-require-git --ignore-case
  --sort path --glob '!**/.git/**'` in-process with ripgrep's crates (`ignore` walker sorted by
  file name, `.rgignore`, overrides; `grep-searcher` with NUL binary detection). A test
  compares it with the real `rg` binary on a tree with .gitignore/.ignore/hidden/.git/binary
  files and globs (skips when rg is absent).
- code-tools: the default bundle is exactly read, bash, edit, write, SearchCodebase
  (CODING_TOOL_NAMES); the invented grep/find/ls/webfetch/websearch/todo placeholder tools are
  gone from it (their free functions remain until 10.2e/10.2f). The permission gate treats
  SearchCodebase as read-only. The default system prompt's tool list now matches hoocode's
  up to SearchCodebase; the rest (Task/TodoWrite/ask_options/SearchHooCode, skills, agents,
  docs, build mode) is 10.4c and friends.
- Tests: search.test.ts and the non-eval cases of hybrid-search.test.ts ported (14 tests).
  Eval tooling and native-search.ts go to 12.4 (noted there).
- L2: new `print-tool-search` (stable): all three SearchCodebase results byte-identical to
  hoocode (auto→lexical, glob+limit, hybrid degraded with reason); only the system prompt
  differs → passes with 10.4c. 10.2d stays l1_done like 10.2a-c.

### 2026-09-26: 10.2c l1_done (edit + write tools); print-tool-edit-light green
- code-tools-fs gains `edit_diff` (edit-diff.ts), `edit`, `write` and `mutation_queue`
  (file-mutation-queue.ts). The matcher works in UTF-16 units like JS (spans, error columns),
  with the three tiers (exact, fuzzy-normalized with a source map, indentation-tolerant
  blocks), the anchored-indent rule, the fuzzy no-op error that names the offending code
  points, and all of hoocode's error texts. NFKC via `unicode-normalization`, `\p{Mn}` via
  `regex`.
- The diff: a port of jsdiff 8.0.4's Myers (tokenizer, diagonal pruning, tie-breaking), not
  `similar` as the design doc planned, because similar aligns ambiguous lines differently.
  `tests/fixtures/edit_diff_cases.json` holds 1000 apply cases + 300 diff cases generated
  from hoocode's own edit-diff.js (`gen_edit_diff_cases.mjs`, deterministic seed); the Rust
  port reproduces all of them byte for byte, errors included.
- Mutation queue: a per-real-path FIFO ticket lock (tools run on blocking threads), so
  edit/write to one file (or a symlink to it) serialize in arrival order.
- Wiring: code-tools' default bundle and light preset use the real edit/write (the light edit
  converts the flat oldText/newText to edits[] at execute time, as light.ts does); the
  placeholder write/edit are gone.
- Tests: tools.test.ts write/edit/fuzzy/CRLF sections, edit-tool-legacy-input,
  edit-tool-preserves-untouched-lines, edit-encoding-recovery-loop (except the context-GC
  case, noted on 10.2g) and file-mutation-queue ported (16 tests) + the reference fixture.
  edit-tool-no-full-redraw is TUI (phase 11).
- L2: new `print-tool-edit-light` (pass, stable: exact, fuzzy with tab + smart quotes, not
  found, nested write, then a read) and `print-tool-edit` (default bundle; differs only in
  the system prompt → 10.4c). 10.2c stays l1_done until then, like 10.2a/10.2b.

### 2026-09-26: 10.2b l1_done (bash tool); print-tool-bash-light green
- New crate `cortexcode-code-tool-bash` (bash.ts, bash-executor.ts, output-accumulator.ts,
  utils/shell.ts): shell resolution (`shellPath`, /bin/bash, bash on PATH, sh; Git Bash on
  Windows), `get_shell_env` (bin dir first on PATH), sanitize + strip-ansi (ansi-regex 6.2
  pattern), process-tree kill and detached-child tracking. `LocalBashOperations` spawns
  through process-wrap (process group on Unix, job object on Windows), kills the group on
  abort/timeout (`aborted` / `timeout:<s>` errors), and waits at most 100 ms for pipes held
  by backgrounded children after the shell exits (waitForChildProcess). `OutputAccumulator`:
  streaming UTF-8 decode, bounded tail, temp-file spill, final compression. The tool keeps
  hoocode's texts (truncation notices, exit/timeout/abort statuses, allowed/denied command
  patterns, command prefix, spawn hook) and 100 ms update throttling (a flusher thread
  plays the timer). `execute_bash_with_operations` is the user-`!` executor.
- Wiring: code-tools' default bundle and light preset use the real bash tool (placeholder
  and `bash()`/`format_output` helpers removed); the CLI passes `shellCommandPrefix`,
  `shellPath` and the `toolOutput` caps (agent-session.ts `_buildRuntime`).
- Deviations: temp files are `cortex-bash-<16 random alnum>.log` (hoocode: `hoocode-bash-<hex>`);
  numeric exit only (a signal-killed shell is a success, as in hoocode); after the grace
  period a reader thread may linger until the orphan closes the pipe (Node destroys the
  stream).
- Tests: tools.test.ts bash section + bash-prompt-snippet.test.ts ported (19 integration +
  7 unit tests, including tree kill on abort and the pipe-holding background child).
- L2: new `print-tool-bash-light` (pass, selfcheck stable; tool results identical incl. exit 3
  and timeout texts). New `print-tool-bash` (differs only in the default-bundle system
  prompt → passes with 10.4c) and `tool-bash` (interactive; hoocode's build mode asks to
  allow each command, the scenario answers it; needs phase 11 rendering). Both stable on
  hoocode. 10.2b stays l1_done until they pass, as 10.2a does.

### 2026-09-26: 10.1c done (CLI on code-paths + code-settings; code-config deleted)
- `cortexcode-code-config` is deleted: its invented `~/.cortexcode/config.json` schema
  (provider/model/api_key/providers/auto_approve_*) and the `migrate.rs` one-shot copy from
  `~/.hoocode/settings.json` are replaced by code-settings, which reads the real
  `settings.json` with the `.hoocode` fallback. The design doc's "keep the migrate.rs
  one-shot copy" is superseded; the doc's crate tables are updated.
- code-cli: default provider/model come from `defaultProvider` / `defaultModel`; `--light`
  falls back to the `light` setting; the read tool takes `toolOutput.maxBytes/maxLines`,
  `images.autoResize` and `contextGc.enabled`. API keys: CLI flag, env, stored OAuth (the
  config-file keys, which hoocode never had, are gone; auth.json precedence is 10.4b).
  Read-only tools stay auto-approved (was the `auto_approve_read_only` config default).
- Auth store path is `code_paths::auth_path()`; session dirs come from
  `code_paths::sessions_dir()` (both now honor `*_CODING_AGENT_DIR`). The `cortexcode-code`
  umbrella re-exports `paths` and `settings` instead of `config`.
- Ledger bookkeeping: 10.1c's crate list now names `cortexcode-code` instead of the deleted
  `cortexcode-code-config` (noted on the task).
- Checks: workspace tests + clippy clean; L2 `print-basic` pass. Full harness: print-basic,
  print-error, print-retry, print-tool-read-light, print-tool-invalid-light pass; the other
  failures belong to later tasks (10.2/10.2a/10.3/10.8a/11.x), as before.

### 2026-09-26: 10.1b done (code-settings)
- New crate `cortexcode-code-settings` (settings-{types,defaults,storage,manager}.ts). Settings
  stay the raw JSON object (`Settings = serde_json::Map`, preserve_order), so unknown and
  future keys pass through; the typing is in the getters (enums for the string settings,
  structs for compaction/retry/branch-summary/learn/warnings, `PackageSource`), which apply
  the TS defaults, clamps and legacy reads (toolOutputDisplay, migrateSettings).
- Storage: `FileSettingsStorage` writes `<agentDir>/settings.json` and
  `<cwd>/.cortexcode/settings.json`. When a cortex file is missing, its `.hoocode` twin is
  read instead and the first write creates the cortex file (the hoocode file is never written
  or locked). Lock: `fs4` on a `settings.json.lock` sidecar with TS's 10 x 20 ms retry
  (hoocode's proper-lockfile uses a `.lock` directory); writes go to a temp file, then rename.
- Deviations: writes are synchronous (hoocode queues them on a promise chain), so `flush()`
  is a no-op; the file state after each call is the same. Getters return the default for
  wrong-typed values where TS would pass the raw value through (e.g. a non-enum
  `steeringMode`). Numeric setters take integers.
- Tests: settings-manager.test.ts + settings-manager-bug.test.ts ported (35 tests including
  the hoocode fallback, migrations, merge, byte-exact output). settings-token-surface.test.ts
  is the /settings pane; noted on 11.3.
- Next: 10.1c wires code-cli onto code-paths/code-settings and retires `cortexcode-code-config`.

### 2026-09-26: 10.1a done (code-paths); 10.1 split into 10.1a/b/c
- Ledger: 10.1 was too big (config.ts + settings-manager.ts + CLI wiring), so it is now 10.1a
  code-paths, 10.1b code-settings (after 10.1a), and 10.1c wiring (keeps the print-basic L2
  scenario). 10.3, 10.4b, 10.5 and 11.1 now depend on 10.1c. config.ts install-method /
  self-update (config.test.ts) is noted on 12.7.
- New crate `cortexcode-code-paths`: app identity (`cortex`, `~/.cortexcode`, legacy
  `~/.hoocode`), env overrides with `CORTEXCODE_` / `CORTEX_` / `HOOCODE_` prefixes (the help
  text says `CORTEX_*`, so all three are read), agent/auth/sessions/bin/themes/debug-log dirs,
  dispatch dirs, `resolve_agent_file` (falls back to `~/.hoocode/<file>` when only that one
  exists and there is no env override), and utils/paths.ts (canonicalize, isPathInside,
  isLocalPath, cwd-relative formatting, `.agents` ancestor walk).
- Tests: paths.test.ts ported (tests/paths.rs), plus env-order, fallback and path edge cases.

### 2026-09-26: 9.4b done (AgentHarness); phase 9 done except 9.1 (blocked) and 9.2b (needs 10.3)
- New crate `cortexcode-agent-orchestrator` (re-exported as `cortexcode_agent::orchestrator`):
  AgentHarness from harness/agent-harness.ts. It cannot live in agent-harness because
  agent-compaction depends on agent-harness; ledger 9.4b and the design doc crate table say so.
  Drives `Agent` over `Session<S>`: prepareNextTurn rebuilds the context from the session
  (system prompt text or callback), message_end appends to the session, mid-turn writes wait
  for the save point (turn_end), agent_end settles. prompt / skill / prompt_from_template /
  steer / follow_up / next_turn / append_message / compact / navigate_tree / set_model /
  set_thinking_level / set_active_tools / set_tools / resources / abort / wait_for_idle /
  subscribe, and typed `on_*` hooks (before_agent_start, context, before_provider_request,
  tool_call, tool_result, session_before_compact, session_before_tree; last result wins).
  App resource types via `SkillLike` / `PromptTemplateLike`.
- Agent changes: `AgentTool` doc fixed (it is Clone), Agent `set_get_api_key`,
  `set_on_payload` / `set_on_response` + `AgentOptions.on_payload/on_response`, and
  AgentLoopConfig's `Box<dyn Fn(String)>` placeholders are now the typed hooks, passed into the
  loop's SimpleStreamOptions. The harness sets the agent's stream fn to
  `cortexcode_ai_registry::stream_simple` (TS's default).
- Deviations: hooks, system-prompt and auth callbacks are synchronous (the agent's hooks are);
  steer/follow-up queue matching is by equality, not object identity; session append failures
  inside agent events are dropped. Kept from TS: compact()/navigateTree() errors after the phase
  is set (no auth, nothing to compact, unknown entry) leave the phase non-idle.
- Tests: agent-harness.test.ts (both cases) + turns (session writes, events, next-turn and
  before_agent_start messages, system prompt callback), errors, model/thinking writes,
  compaction (hook cancel, session_compact), navigate_tree (editor text, leaf move), tool_call
  block + context hook, all against the faux provider.
- Next: `ledger.py next` (phase 10). Open question for the user: 9.1 rmcp decision.

### 2026-09-26: 9.3b done (skill + prompt-template loaders, executeShellWithCapture)
- cortexcode-agent-harness: `frontmatter::parse_frontmatter` (YAML via serde_yaml_ng -> JSON
  object) and `locale_compare`; `load_skills` / `load_sourced_skills` (SKILL.md makes a
  directory one skill, root `.md` files are skills, ignore files via the `ignore` crate with
  rules rebased on the root as in TS, dot entries / node_modules skipped, symlinks resolved,
  name/description diagnostics); `load_prompt_templates` / `load_sourced_prompt_templates`
  (non-recursive, 60-char first-line description); `utils::shell_output::
  execute_shell_with_capture` over ExecutionEnv. Sourced results use `Sourced { item, source }`;
  TS's optional map callbacks are left to the caller.
- dep-firewall: `serde_yaml_ng` owners now `cortexcode-agent-harness` + `code-resources`
  (the TS harness parses frontmatter with `yaml` too). 10.5 can reuse
  `cortexcode_agent_harness::parse_frontmatter` instead of a second YAML adapter.
- Deviations: a non-string `description`/`name` counts as missing (TS would throw a TypeError
  diagnostic); YAML error texts are serde_yaml_ng's; localeCompare is the case-insensitive
  approximation code-prompts already uses.
- Tests: skills.test.ts and prompt-templates.test.ts (all cases, unix) + ignore files, name
  validation, long descriptions, shell capture.
- Disk: stale test executables in target/debug/deps grew to 21G and filled the session disk;
  `find target/debug/deps -maxdepth 1 -type f -perm -u+x ! -name '*.so' ! -name '*.rlib'
  ! -name '*.rmeta' ! -name '*.d' -delete` frees it (they are relinked on demand).
- Next: 9.4b (agent-harness.ts).

### 2026-09-26: 9.4a done (ExecutionEnv + the local tokio env)
- cortexcode-agent-harness `env`: `ExecutionEnv` trait (BoxFuture methods: exec, read/write,
  file_info/list_dir without following symlinks, real_path, exists, create_dir, remove,
  temp dir/file, cleanup), `FileInfo`, `FileKind`, `FileError` + `FileErrorCode` (io
  ErrorKind -> the TS codes), `ExecOptions` (cwd, env, timeout seconds, AbortSignal,
  on_stdout/on_stderr), and `LocalExecutionEnv` (alias `NodeExecutionEnv`): shell from
  getShellConfig (/bin/bash, which bash, sh; Git Bash on Windows), own process group killed
  with SIGKILL on abort/timeout (`aborted` / `timeout:<s>` errors), UTF-8 chunk streaming that
  holds back split characters, mkdtemp-style temp dirs.
- Deviations: FileError messages are the OS text (Node's read `ENOENT: ..., lstat '...'`);
  rm on a directory without recursive gives Node's ERR_FS_EISDIR text with code `unknown`,
  as toFileError maps it.
- Tests: nodejs-env.test.ts (all cases, unix) + timeout / exit code / rm / custom shell.
- Next: 9.3b (skill + prompt-template loaders, executeShellWithCapture over ExecutionEnv).

### 2026-09-26: 9.2 split; 9.2a done (harness compaction + branch summarization)
- Ledger: 9.2 -> 9.2a (harness/compaction/*, both compaction test files) + 9.2b
  (agent-session-compaction.ts: auto-compaction thresholds, /compact, the `compact-command` L2
  scenario; depends on 10.3). 9.4b now depends on 9.2a. Remaining phase-9 order: 9.4a -> 9.3b
  -> 9.4b (9.1 blocked on the rmcp decision).
- cortexcode-agent-compaction rewritten as the port (the old KeepRecent/Summary strategies were
  unused placeholders): `utils` (file ops, serializeConversation with the 2000-char tool-result
  cap, SUMMARIZATION_SYSTEM_PROMPT), `compaction` (token estimates in UTF-16 chars/4,
  shouldCompact with maxContextRatio, findCutPoint/findTurnStartIndex, prepareCompaction,
  generateSummary / turn-prefix summary via `cortexcode_ai_registry::complete_simple`, compact
  with parallel split-turn summaries and tokensAfter), `branch_summarization`
  (collectEntriesForBranchSummary over a `BranchEntrySource` trait, prepareBranchEntries,
  generateBranchSummary). Works on `cortexcode_agent_session::FileEntry`.
- API shape: TS positional (apiKey, headers, signal, thinkingLevel) -> `SummarizeOptions`;
  `turn_start_index: Option<usize>` for TS -1. Deviation: tool-call args serialize with serde
  (an integral float prints `1.0`, JS `1`).
- Tests: agent/test/harness/compaction.test.ts and coding-agent/test/compaction.test.ts (incl.
  the v1 large-session.jsonl fixture read from target/hoocode-pin, migrated by code-session;
  the live LLM case is `#[ignore]`, ANTHROPIC_OAUTH_TOKEN), plus split-turn and branch cases.
- Disk: target/debug/incremental hit the session's disk allowance (11G); deleted it.
- Next: 9.4a.

### 2026-09-26: 9.3/9.4 split; 9.3a done (harness utils without an ExecutionEnv)
- Ledger: 9.3 -> 9.3a (env-free utils) + 9.3b (skill/prompt-template loaders and
  executeShellWithCapture over ExecutionEnv); 9.4 -> 9.4a (ExecutionEnv trait + tokio env,
  nodejs-env.test.ts) + 9.4b (agent-harness.ts). The loaders' tests use NodeExecutionEnv, which
  is 9.4's port. 9.2 (compaction) now depends on 9.3a (it needs messages.ts + compressGeneral);
  its earlier `start` was undone as a bookkeeping fix (noted on the task). Order from here:
  9.4a -> 9.3b -> 9.2 -> 9.4b.
- cortexcode-agent-harness: `types` (Skill, PromptTemplate), `messages` (create*SummaryMessage,
  createCustomMessage with ISO timestamps, summarizeArgs, describeBackgroundTool,
  createBackgroundPlaceholderText, createBackgroundTaskMessage), `prompt_templates`
  (parseCommandArgs, substituteArgs with String.replace `$&`/`$$` semantics,
  formatPromptTemplateInvocation), `skills` (formatSkillInvocation, name/description rules, env
  path helpers), `system_prompt::format_skills_for_system_prompt`, `utils::{truncate,
  output_compression, shell_output}` (ShellCapture = executeShellWithCapture's accumulator).
  JS details kept: UTF-16 lengths, JS trim whitespace, toFixed half-up, ASCII `\b`.
- The old invented `{{var}}` template struct is now `TextTemplate` (code-prompts still uses
  `render`); `SystemPromptBuilder` is untouched.
- Tests: resource-formatting.test.ts and system-prompt.test.ts, plus unit tests for each util.
- Next: 9.4a.

### 2026-09-26: 9.1 blocked on a design decision (rmcp)
- Found while starting 9.1: rmcp 3.4.1 parses every result into typed structs (tools/call ->
  `CallToolResult`), while hoocode feeds the model `JSON.stringify(rawResult, null, 2)`, so tool
  output would be re-serialized from rmcp's structs (unknown fields dropped, rmcp key order;
  identical for plain `{content:[{type:"text",text}]}`). And rmcp 3.x has no legacy HTTP+SSE
  client, which hoocode still uses (`type: "sse"` and the 4xx fallback).
- Options (in the ledger block): (1) rmcp for stdio + streamable HTTP + OAuth, hand-written
  legacy SSE, document the re-serialization deviation (recommended); (2) keep the hand-rolled
  transport, make it async with raw JSON, add streamable-HTTP sessions + OAuth by hand.
- Also: the `mcp-tool-call` L2 scenario needs app-side MCP loading, which is 10.11, so 9.1 can
  reach only `l1_done` on its own.
- Waiting on the user; continuing with 9.2.

### 2026-09-26: 8.8 done (typed onPayload / onResponse); phase 8 complete
- `cortexcode_ai_types`: `OnPayload<M = Model>` / `OnResponse<M = Model>` (Arc'd async closures
  with `new` / `sync` constructors, `apply` / `notify` helpers), `ProviderResponse {status,
  headers}` (`from_pairs` = `headersToRecord`: lower-case, sorted, repeats joined with ", "),
  `HookFuture`. The `Option<String>` placeholders in SimpleStreamOptions / StreamOptions /
  ProviderStreamOptions are gone. `cortexcode_ai_util::provider_response(&reqwest::Response)`.
- Wired as in TS: openai-completions, openai-responses, azure (via `ResponsesRequest`),
  anthropic (`stream: true` re-forced after the hook), codex (onPayload before the transport
  choice; onResponse for every SSE response, failures included), google / vertex / gemini-cli
  (onPayload only; google's hook sees the SDK params), faux (onResponse with a synthetic 200),
  images/openrouter (`OnPayload<ImagesModel>`). The SDK-backed providers call onResponse only for
  a successful response (the SDK throws first), as in TS.
- Tests: `crates/cortexcode-ai/tests/stream_hooks.rs` (onPayload replacement reaches the wire for
  every registered API; onResponse status/headers; failures skip it; codex reports a 400),
  faux and images hook tests, and openrouter-cache-write-repro.test.ts as an `#[ignore]` live
  test (OPENROUTER_API_KEY) plus an offline check of its cache-marker transform.
- Hook closures need their parameter types written (`|payload: &Value, model: &Model|`): the
  default model type parameter is not used for inference.
- Ledger note on 9.4: agent-types' `Box<dyn Fn(String)>` hook placeholders still need mapping
  onto these.
- Next: `ledger.py next` (phase 9/10).

### 2026-09-26: 8.4c done (Cloud Code Assist: gemini-cli + antigravity)
- New `cortexcode-ai-provider-google-gemini-cli` (google-gemini-cli.ts), registered for the
  `google-gemini-cli` API (both providers): envelope `buildRequest` in TS key order (Antigravity
  system instruction, `requestType: agent`, Claude tools as `parameters`), Gemini CLI /
  Antigravity headers (`CORTEXCODE_`/`HOOCODE_ANTIGRAVITY_VERSION`), Antigravity endpoint
  fallbacks, the TS retry loop verbatim (403/404 cascade, 429/5xx backoff with
  `extractRetryDelay`, and -- as in TS -- every error raised inside the loop, a plain 400
  included, is caught and retried 1/2/4 s), empty-stream refetch (0.5/1 s), lazy `start` event,
  `streamSimple` thinking levels/budgets. Message/tool conversion reused from ai-provider-google.
- New `cortexcode-ai-oauth-google` (google-gemini-cli.ts, google-antigravity.ts,
  google-oauth-client.ts): both PKCE logins (verifier = state; new core
  `CallbackValidation::CodeAndStateDeferred`), pasted-redirect race, token exchange/refresh with
  the TS error texts, strict (gemini-cli) vs fall-through (antigravity) project discovery with
  onboarding + operation polling, `get_api_key` = `{token, projectId}` JSON. Client from
  `CORTEXCODE_{GEMINI_CLI,ANTIGRAVITY}_CLIENT_{ID,SECRET}` (HOOCODE_ twins honored).
- `cortexcode_ai::builtin_oauth_providers()` / `install_builtin_oauth_providers()` =
  `BUILT_IN_OAUTH_PROVIDERS` (nothing installed the built-ins before). code-cli: `login
  google-gemini-cli|gemini-cli|google-antigravity|antigravity`; runtime returns the JSON API
  key for the Google providers and refreshes them.
- Tests: google-gemini-cli.test.ts (all cases; registry half in cortexcode-ai
  `tests/oauth_providers.rs`), mock-server stream/retry/empty-stream cases, OAuth flows with a
  fake fetch (discovery, refresh, pasted-redirect login). Routing test: PENDING_APIS is empty;
  gemini-cli gets a successful stream (a 400 would sit through its 7 s of retries).
- Noticed, not fixed: help_text.rs lists `CORTEX_*` env names (e.g. `CORTEX_GEMINI_CLI_CLIENT_ID`,
  `CORTEX_CODING_AGENT_DIR`) while the code reads `CORTEXCODE_*`/`HOOCODE_*`; pre-existing.
- Deviations: onPayload (8.8); Retry-After dates parse RFC 2822/3339 only.
- Next: `ledger.py next`.

### 2026-09-26: 8.4b done (GitHub Copilot routing)
- No production code change: Copilot routing was already in place (catalog models on
  `anthropic-messages` / `openai-responses` / `openai-completions` with the static Copilot
  headers; Bearer auth + `buildCopilotDynamicHeaders` in all three providers; env key
  `COPILOT_GITHUB_TOKEN` only; OAuth device flow + token refresh from 8.7). This task adds the
  missing test coverage in `crates/cortexcode-ai/tests/github_copilot.rs`:
  github-copilot-anthropic.test.ts (Bearer + static/dynamic headers, no fine-grained beta,
  Opus 4.8 adaptive thinking, interleaved beta), transform-messages-copilot-openai-to-anthropic
  .test.ts (all 4 cases), and a routing check that each Copilot backend sends Bearer auth, the
  catalog headers, `X-Initiator` user/agent and `Copilot-Vision-Request` for image input.
- Already ported elsewhere: github-copilot-oauth.test.ts (ai-oauth-github-copilot, 8.7),
  openai-responses-copilot-provider.test.ts (ai-provider-openai-responses, 8.3).
- Not in this task: applying `modifyModels` (token `proxy-ep` -> model baseUrl for
  business/enterprise accounts) when models are listed; that is model-registry work (10.4b).
- Next: `ledger.py next` (8.4c).

### 2026-09-26: 8.4a done (openai-codex provider + ChatGPT OAuth)
- New `cortexcode-ai-provider-openai-codex` (openai-codex-responses.ts): request body in TS key
  order (`instructions`, `text.verbosity`, `strict: null` tools, reasoning via thinkingLevelMap),
  SSE/WebSocket headers (`originator: pi` + `pi (<platform> <release>; <arch>)` UA, account id
  from the JWT), SSE path with the TS retry loop (every failure but a usage limit, 1/2/4 s) and
  friendly usage-limit errors, `mapCodexEvents` (stops at the terminal event, so a body that
  stays open still completes), and the WebSocket transport: per-session connection cache with
  5 min idle expiry, `previous_response_id` delta continuation, per-session SSE fallback with a
  `provider_transport_failure` diagnostic, debug stats. WebSocket = `tokio-tungstenite` (owned by
  this crate in dep-firewall.json; rustls/ring, same as reqwest).
- New `cortexcode-ai-oauth-openai-codex` (openai-codex.ts): PKCE + state, callback server on 1455
  (new `CallbackValidation::StateThenCode` in the core server), manual-paste race, prompt
  fallback, token exchange/refresh with the TS error texts, `accountId` stored flat in auth.json.
  Wired into code-cli `login` ("openai-codex"/"codex"/"chatgpt") and token refresh.
- `Transport` enum is now hoocode's (`sse`/`websocket`/`websocket-cached`/`auto`, serde kebab);
  the unused Stdio/StreamableHttp variants are gone. `ResponsesStreamOptions` gained
  `resolve_service_tier`. `utils/diagnostics.ts` ported into ai-util.
- `session-resources.ts` ported as `cortexcode_ai_registry::session_resources` (codex cleanup
  built in); ledger note on 10.3: `AgentSession.dispose()` must call it.
- Tests: openai-codex-stream.test.ts (all cases; local HTTP + WebSocket mock servers instead of
  stubbed globals), openai-codex-oauth.test.ts, cache-affinity e2e (`#[ignore]`, needs
  `OPENAI_CODEX_OAUTH_TOKEN`), plus fallback/error/retry cases. Routing test covers codex.
- Deviations: onPayload/onResponse (8.8); JSON parse errors use serde's text; diagnostics have no
  stack; `os.release()` is empty on non-unix.
- Next: `ledger.py next`.

### 2026-09-25: 8.7 done (OAuth split: core + anthropic + github-copilot)
- `cortexcode-ai-oauth` is the core: types (`OAuthCredentials` now serializes flat like
  hoocode's auth.json — extra fields such as `enterpriseUrl`/`type` sit beside
  refresh/access/expires), `OAuthLoginCallbacks` / `OAuthProvider` traits, PKCE, the OAuth
  page HTML (oauth-page.ts), a tokio loopback `CallbackServer` (404/400/error pages, state
  check, `cancelWait`), a `Fetch` seam (TS tests stub global fetch) with `ReqwestFetch`, and
  the provider registry of index.ts (built-ins are installed by the composer with
  `install_builtin_oauth_providers`, since they live in their own crates).
- New `cortexcode-ai-oauth-anthropic` (anthropic.ts: authorize URL, callback server on 53692
  + manual-paste race, prompt fallback, state checks, token exchange/refresh with the TS error
  texts) and `cortexcode-ai-oauth-github-copilot` (github-copilot.ts: device flow with the
  1.2x / 1.4x poll timing and slow_down handling, Copilot token refresh, model policy enabling,
  base URL from the token, `modify_models`).
- code-cli: `/login` plumbing now runs the provider flows through terminal callbacks; its own
  callback server is gone; token refresh uses the new crates and reads `enterpriseUrl` (the
  old code wrote `enterprise_url`, which hoocode never uses).
- Tests: anthropic-oauth.test.ts and github-copilot-oauth.test.ts ported (the Copilot poll
  times 6000/12000/26000 and 6000/20000/25000 reproduce under tokio's paused clock).
- Also fixed a flaky agent-loop test (`emits_tool_execution_end_in_completion_order…`, ~50%
  failures at the session's starting commit): the released tool now finishes 50 ms after
  the releasing one, which is the order JS guarantees.
- Note for future sessions: never share `CARGO_TARGET_DIR` with a second worktree of this
  repo; cargo hashes path crates relative to the workspace root and the builds clobber
  each other (fix: touch the sources and rebuild).
- Next: `ledger.py next`.

### 2026-09-25: 8.6e done (cross-provider suites); 8.8 added
- Non-live ports: constrain-tool-calls (ai-util strict schema, completions + responses
  tools), supports-xhigh (ai-models catalog), openrouter-images (ai-images), cache-retention
  (`cortexcode-ai/tests/cache_retention.rs`, payload builders instead of onPayload; the
  key-gated env-default cases only inspect the payload, so they run unconditionally).
- ai-images brought to openrouter.ts: `response_id`, `signal`/`timeout_ms`/`max_retries`
  options, SDK client retries, getEnvApiKey, TS error texts (`No API key available for
  provider: …`, openai APIError message), stricter data-URI match, and a `generate_images`
  dispatcher (`No API provider registered for api: …`).
- Live (`#[ignore]`, key-gated) in `cortexcode-ai/tests/live_matrix.rs`: context-overflow,
  empty, image-tool-result (fixture copied to `tests/data/red-circle.png`), responseid,
  tokens (abort usage), total-tokens, tool-call-without-result, unicode-surrogate (the lone
  surrogate case sends the sanitized text; Rust strings cannot hold one),
  tool-call-id-normalization (prefilled OpenRouter case), xhigh, zen. Copilot/Codex cases
  and local Ollama/LM Studio/llama.cpp cases are not included (see the file header).
- New ledger task 8.8: typed onPayload/onResponse hooks for every provider (the
  SimpleStreamOptions fields are String placeholders) + openrouter-cache-write-repro; 9.4
  depends on it.
- Next: `ledger.py next`.

### 2026-09-25: 8.6d done (google.ts / google-vertex.ts / google-shared.ts re-port)
- ai-provider-google rebuilt: shared.rs = google-shared.ts (convertMessages with
  transformMessages + id normalization for claude-/gpt-oss-, base64 thought-signature
  validation for same provider/model only, merged function responses, Gemini 3 nested vs
  Gemini <3 separate image turn; convertTools with sanitizeForOpenApi; isThinkingPart,
  retainThoughtSignature, mapToolChoice, mapStopReason(+String)). request.rs =
  `GoogleOptions`/`GoogleThinking`, both streamSimple mappings (levels for Gemini 3 / Gemma 4,
  budgets for 2.5, disabled configs), buildParams (`{model, contents, config}`), and the
  @google/genai 1.52 pieces: `sdk_body` (REST mapping incl. part key order),
  `ClientConfig` (= `new GoogleGenAI({...})` options; vertex API-key vs ADC choice, custom
  base URL with COLLECTION scope, api version) and `ClientConfig::endpoint` (= ApiClient URL:
  regional / global / multi-region hosts, project path, `x-goog-api-key` or ADC Bearer).
  adc.rs: GOOGLE_APPLICATION_CREDENTIALS (service_account, authorized_user), gcloud
  well-known file, metadata server (external_account not supported). lib.rs: the stream loop
  (SDK chunk decoder with its three delimiters and error-chunk check, ApiError messages,
  block switching, retained signatures, `<name>_<ms>_<n>` ids for missing/duplicate ids,
  usage + cost, finishReason errors, `Request aborted` when pre-aborted).
- Behaviour changes vs the old crate: Vertex no longer reads GOOGLE_VERTEX_ACCESS_TOKEN /
  GOOGLE_ACCESS_TOKEN or defaults the location to us-central1 (hoocode does neither).
  The SDK's `gl-node/<version>` user-agent part is `gl-rust/cortexcode`.
- Tests: vertex-api-key-resolution (as ClientConfig), thinking-signature, convert-tools,
  gemini3-unsigned-tool-call, image-tool-result-routing, thinking payloads, stream/SDK
  tests; live thinking-disable E2E in `tests/live_e2e.rs` (`#[ignore]`).
- Next: 8.6e via `ledger.py next`.

### 2026-09-25: 8.6c done (anthropic.ts re-port + its tests)
- ai-provider-anthropic is now a port of anthropic.ts: `stream` = streamSimpleAnthropic
  (`No API key for provider: …` via getEnvApiKey; buildBaseOptions; adaptive thinking +
  effort via thinkingLevelMap, or budget thinking via adjustMaxTokensForThinking);
  `stream_anthropic(model, context, AnthropicOptions)` = streamAnthropic. request.rs:
  createClient headers (x-api-key / Bearer for Copilot and OAuth `sk-ant-oat`, Claude Code
  identity + betas, fine-grained tool streaming and interleaved-thinking betas, Copilot
  dynamic headers), buildParams (OAuth system identity, temperature rule, thinking
  adaptive/enabled/disabled, always-on models -> `output_config: {effort: low}`, metadata
  user_id, tool_choice), convertMessages (transformMessages + id normalization, string user
  content, redacted/unsigned thinking, merged consecutive tool results, cache marker on the
  last user turn), convertTools (eager_input_streaming, defer_loading + BM25 tool-search tool,
  breakpoint on the last tool), Claude Code tool-name mapping. sse.rs: iterateSseMessages'
  line decoder + iterateAnthropicEvents (error events, unknown events skipped,
  parseJsonWithRepair, "ended before message_stop"). lib.rs: the event loop (contentIndex =
  position in content, signatures, redacted_thinking, responseId, usage + calculateCost,
  unknown stop reason / refusal errors).
- ai-types: `Tool.defer_loading` (TS `deferLoading`); fix_struct_fields knows it; faux
  serializes it.
- Tests: claude-5-models request format, thinking-disable payloads, tool-search, sse-parsing,
  eager-tool-input compat (mock server); live e2e files in `tests/live_e2e.rs` (`#[ignore]`;
  Copilot cases left to 8.4b). The opus-4.7 smoke TS test expects `thinking: {type:
  "adaptive"}` without `display`, which the pinned code no longer sends; not asserted.
- Next: 8.6d via `ledger.py next`.

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

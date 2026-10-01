# `hoocode app-server`

Status: **built 2026-10-01** (L3 stdio + Unix socket daemon). Design for the first build.
Outside the migration (no TypeScript reference). Supersedes
[rpc-approvals.md](rpc-approvals.md) for hoobot: hoobot moves to this server
instead of `--mode rpc`, so the RPC approval fix is no longer its blocker.

Background and the larger plan: hoobot `docs/design/` 12, 13, 16, 17, 18
("B+"). This doc is the part we build now.

## Goal

Clients are swappable. One wire protocol, the Codex app-server protocol, on
both sides:

- hoobot can talk to `hoocode app-server` **or** the real `codex app-server`;
- the stock Codex TUI (`codex --remote unix://…`) can talk to
  `hoocode app-server`.

We implement only what those clients need, not the full protocol
(171 client methods upstream).

## Decisions (2026-10-01, with the owner)

1. **Codex-compatible wire format both ways.** Same method names, field
   names and shapes as Codex for everything we implement.
2. **First build:** L3 (stdio, one client) **and** a single-process daemon on
   a Unix socket with fan-out to many clients. One workspace (the server's
   cwd) per server. No worker processes, no WebSocket-on-TCP, no profiles
   yet.
3. **Unimplemented methods:** `-32601 method not found`, except the few that
   stock Codex clients call at startup, which get schema-valid neutral
   results. That list comes from recording a real `codex --remote` session,
   not from guessing.
4. **MIT only.** Our code is MIT. Nothing is copied from `../codex`
   (Apache-2.0): no code, no schemas, no generated types. Types are written
   by us in `cortexcode-app-server-protocol`. Codex's JSON schemas are
   generated at test time (`codex app-server generate-json-schema`) into
   `target/`, never checked in. Dependencies must be permissive and
   MIT-compatible (MIT, MIT OR Apache-2.0, BSD, ISC, Zlib, …).
5. **hoobot moves to the app-server** right after this lands (Discord port).

## Crates

| Crate | What |
|---|---|
| `cortexcode-app-server-protocol` | Wire types: envelope, method params/results, notifications, server requests. serde only. No I/O. |
| `cortexcode-app-server` | Server: connections, thread manager, event → item mapping, approvals, transports (stdio, Unix socket). |
| `cortexcode-code-cli` | `hoocode app-server [--listen URL]` subcommand; builds real sessions for the server through a factory. |

`cortexcode-app-server` does not depend on `cortexcode-code-cli`. The CLI
hands it a `SessionFactory` (trait object) that builds an `AgentSession`
for a session file. Tests pass a faux-provider factory.

New third-party deps: `tokio-tungstenite` (MIT) for the WebSocket framing
on the Unix socket. Add `cortexcode-app-server` as an owner in
`migration/dep-firewall.json`. `uuid` for turn/item ids. Dev-only:
`jsonschema` for the schema check.

## Wire format

- JSON objects; no `"jsonrpc": "2.0"` field (Codex omits it; accept it if sent).
- Request `{id, method, params?}`, response `{id, result}` or
  `{id, error: {code, message, data?}}`, notification `{method, params?}`.
  `id` is a string or an integer.
- Server → client requests use integer ids from one counter per server.
- **stdio:** one JSON object per line (LF), like `--mode rpc`.
- **Unix socket:** WebSocket framing over the socket (one JSON object per
  text frame). Codex's TUI and its `app-server proxy` connect this way, and
  Bun can too (`new WebSocket("ws+unix://PATH")`, checked 2026-10-01).

### `--listen`

Same values as Codex: `stdio://` (default), `unix://` (default path),
`unix://PATH`. Default socket path:
`<agent dir>/app-server-control/app-server-control.sock`
(`~/.cortexcode/…`). The directory is created 0700, the socket 0600. If the
path exists: connect to it; if something answers, refuse to start
("already running"); otherwise remove the stale file and bind.

## Methods

### Client → server requests

| Method | Notes |
|---|---|
| `initialize` | Before anything else, once per connection. Result: `userAgent` (`hoocode/<version>`), `codexHome` (agent dir), `platformFamily`, `platformOs`. Records `capabilities.experimentalApi`. |
| `thread/start` | New session in the server's cwd. Optional `model` (hoocode model pattern). Optional `config["hoocode.profile"]` reserved for profiles (ignored for now). Caller is subscribed. Emits `thread/started`. |
| `thread/resume` | Load a saved session by `threadId` (or `path`). Caller is subscribed. Result has `thread.turns` rebuilt from the session file. Pending approvals for the thread are replayed to the caller. |
| `thread/list` | Sessions for the server's cwd, newest first; `limit` + `cursor` (opaque offset). `archived: true` → empty. |
| `thread/read` | One thread; `includeTurns` rebuilds turns from the file. Doesn't subscribe. |
| `thread/unsubscribe` | Stop events for this connection. Result `status`: `unsubscribed` / `notSubscribed` / `notLoaded`. |
| `turn/start` | Starts a run with `input` (text, data-URL image or local image). Error if a turn is already running (clients steer instead). Optional `model` switches this turn and later ones (must be an available model, else `-32602`). Result `{turn}` with `status: inProgress`. |
| `turn/steer` | `expectedTurnId` must equal the running turn. Queued as a hoocode steer. Result `{turnId}`. |
| `turn/interrupt` | Denies and resolves pending approvals, starts the abort and answers `{}` at once; `turn/completed` (`interrupted`) follows. No further approvals are asked in that turn. |
| neutral stubs | What the recorded Codex TUI session needs (see "Codex TUI" below): `account/read`, `model/list` (hoocode's real models), `configRequirements/read`, `skills/list` (empty), `thread/loaded/list`. |

Client notification: `initialized` (accepted; nothing else required).

### Server → client notifications

`thread/started`, `thread/status/changed`, `turn/started`, `turn/completed`,
`item/started`, `item/completed`, `item/agentMessage/delta`,
`serverRequest/resolved`, `error`.

### Server → client requests (approvals)

| hoocode tool | Request | Item type |
|---|---|---|
| `bash` | `item/commandExecution/requestApproval` with `command`, `cwd` | `commandExecution` |
| `edit`, `write` | `item/fileChange/requestApproval` | `fileChange` |
| `webfetch`, `websearch` | `item/commandExecution/requestApproval`, `command` = hoocode's one-line description, `reason` names the tool | `dynamicToolCall` |

Decisions back: `accept` → run once; `acceptForSession` → run, and don't ask
again for that tool on this thread while it is loaded (in memory only);
`decline` → deny; `cancel` → deny and interrupt the turn. Anything else
(amendments) → treated as `accept` for that one call. hoocode's "Always"
(writing the global config) is never offered over the server.

Which calls ask is hoocode's existing policy (`cortexcode-code-permissions`
`evaluate`, per-mode `auto_allow`, `denied_tools`, bash patterns, …) with
"has UI" = true. So a workspace in hoobot's `discord` mode asks for
bash/edit/write exactly as the terminal UI would.

## Mapping hoocode → Codex

| hoocode | Codex |
|---|---|
| session (JSONL file, id) | thread (`id` = `sessionId` = session id, `path` = file) |
| one `prompt` run (agent_start … agent_end) | turn (our own uuid; hoocode's internal LLM turns are not exposed) |
| user prompt | `userMessage` item (started + completed at turn start) |
| assistant text | `agentMessage` item: `item/started` on first text, `item/agentMessage/delta` per text delta, `item/completed` with the full text at message end |
| assistant thinking | `reasoning` item, completed with `content` at message end (no deltas) |
| `bash` tool | `commandExecution` (`source: agent`, `commandActions: []`, `aggregatedOutput`, `status`) |
| `edit` / `write` | `fileChange` (`changes: [{path, kind, diff}]`; `diff` from the tool result when present) |
| any other tool | `dynamicToolCall` (`tool`, `arguments`, `status`, `success`, `contentItems` as text) |
| denied tool | item completes with `status: declined` (`failed` for dynamic tools) |
| agent_end, normal | `turn/completed` status `completed` |
| aborted | `turn/completed` status `interrupted` |
| error stop | `error` notification, then `turn/completed` status `failed` with `error.message` |

Thread fields we can't fill honestly get neutral values: `source: "appServer"`,
`modelProvider` from the session model, `ephemeral: false`, `projectId: null`,
`cliVersion` = hoocode version. Start/resume results: `approvalPolicy:
"on-request"`, `approvalsReviewer: "user"`, `sandbox: {type:
"dangerFullAccess"}` (hoocode has no sandbox; that is the honest value).

Thread status: `idle` when loaded and not running; `active` (with
`waitingOnApproval` while an approval is open) during a turn; `notLoaded` in
list/read results for threads not in memory.

## Server internals

- **Connections:** each has an outgoing channel; transports only move JSON
  values. A connection must `initialize` first (else error `-32600`).
- **Thread manager:** `threadId → LoadedThread {session, subscribers,
  active turn, pending approvals, session grants}`. Subscribe on
  start/resume; unsubscribe on request or disconnect. A thread with no
  subscribers unloads when idle (after its turn ends, if running).
- **Fan-out:** every notification for a thread goes to all its subscribers.
  An approval request goes to all of them with one id; the **first answer
  wins**; the rest get `serverRequest/resolved`. Late answers are ignored.
- **Approvals and threads:** `PermissionGate::request` is synchronous and
  runs on a tokio worker inside the agent loop. The server's gate waits with
  `tokio::task::block_in_place` (needs the multi-thread runtime, which the
  CLI uses). Interrupt, unload and shutdown deny every pending approval so
  nothing blocks forever. A connection dropping does **not** deny: the
  request stays pending and is replayed to the next subscriber
  (reconnect / hoobot restart).
- **Session files:** hoocode's default session dir for the cwd (same as the
  terminal UI, so `hoocode --resume` sees server threads), or `--session-dir`.

## Codex TUI, recorded (codex-cli 0.159.3, 2026-10-01)

`codex --remote unix://PATH` against `hoocode app-server`, logged by a
WebSocket proxy. Startup sends, in order: `initialize`, `initialized`,
`account/read`, `model/list` + `configRequirements/read` (fatal if they
fail), then `collaborationMode/list`, `hooks/list`, `config/read`,
`thread/start`, `thread/loaded/list`, `thread/list` ×2, `skills/list`,
`plugin/list`.

- `collaborationMode/list`, `hooks/list`, `config/read`, `plugin/list`:
  the TUI accepts `method not found` silently. Left unimplemented.
- `skills/list`: `method not found` shows an error banner, so it returns
  an empty list per folder.
- The TUI's `thread/start` names its own default model (e.g. `gpt-6-luna`)
  and `sandbox: read-only`. The model is used only if hoocode has auth for
  a model with exactly that id (`provider/id` or bare id, default provider
  first); otherwise hoocode's default model runs. Sandbox and approval
  fields are ignored: hoocode's mode policy decides.
- Checked by hand: a turn streams; `bash` in an asking mode shows the TUI's
  "Would you like to run the following command?"; accepting runs it and
  the TUI shows the output.

Note on modes: a project `hoo-config.json` can only add to a mode's
`auto_allow` that the global config also defines (lists are unioned). A
workspace that must ask needs a mode name the global config doesn't use
(hoobot uses `discord`).

## Wire behaviour worth knowing

- A connection's requests are handled **one at a time, in order** (clients
  pipeline `initialize` and what follows). Different connections run in
  parallel. Turns run in the background, so `turn/start` answers at once.
- After a `thread/resume` response, the server sends that connection the
  thread's open approval requests (same ids; first answer still wins).

## Not in this build

WebSocket on TCP + token, worker processes per (profile, workspace),
profiles and policy (L2), `item/tool/requestUserInput` dialogs, idle
timeouts, thread fork/archive/name, daemon start/stop commands and launchd.

## Tests

1. Protocol crate: serde round trips for every type; field names checked
   against hand-written JSON fixtures we wrote ourselves.
2. Server crate, faux provider: initialize gate; start → turn → deltas →
   completed; steer; interrupt; approval accept / decline / cancel /
   acceptForSession; two connections on one thread (fan-out, first answer
   wins, `serverRequest/resolved`); resume replays a pending approval;
   list/read rebuild turns.
3. Schema check: `scripts/codex-schema.sh` generates Codex's schemas into
   `target/codex-schema/`; `cargo test -p cortexcode-app-server --test
   server -- --ignored schema_conformance` drives a scenario and validates
   every message (76 messages, 23 types) against them.
4. Manual: `codex --remote unix://PATH` against `hoocode app-server`; record
   its calls, add neutral stubs for what it needs, repeat until a turn with
   an approval works.

Done = `cargo fmt --check`, clippy `-D warnings`, nextest for the new and
touched crates, `migration/check_dep_firewall.py`, plus steps 3 and 4.

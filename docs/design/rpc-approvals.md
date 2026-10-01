# RPC approval dialogs

Status: **planned 2026-10-01, not started.** Design and plan only.
This is not a migration-ledger task: the user took it out of the migration
(which is paused) on 2026-10-01. It is the one blocker for running hoobot on
the Rust `hoocode`.

## Problem

In `--mode rpc`, tools that should ask first (bash, write, edit, webfetch,
websearch) run without asking.

- `cortexcode-code-cli/src/runtime.rs` `build_permission_gate` attaches a
  `PermissionUi` only in interactive mode. RPC mode gets `None`.
- `cortexcode-code-permissions/src/lib.rs` `evaluate` returns `Allow` for a
  gated tool when there is no UI (`!has_ui`). Hard rules (`denied_tools`,
  `enabled_tools`, bash patterns) still apply; `auto_allow` and the prompt
  don't.
- `cortexcode-code-rpc/src/mode.rs` drops every `extension_ui_response`.

hoobot runs `hoocode --mode rpc` per Discord thread with a `discord` mode that
auto-allows only `read`, and shows the prompts as Allow / Deny buttons. On the
Rust build every allowlisted Discord user can run bash and edit files with no
approval.

## Wire format (what hoobot already speaks)

Request, on stdout:

```json
{"type":"extension_ui_request","id":"<uuid>","method":"select","title":"Allow: bash ls","options":["Yes (once)","No (block)","Always (add to auto-allow for this mode)"]}
{"type":"extension_ui_request","id":"<uuid>","method":"notify","message":"…","notifyType":"info"}
```

Response, on stdin:

```json
{"type":"extension_ui_response","id":"<uuid>","value":"Yes (once)"}
{"type":"extension_ui_response","id":"<uuid>","cancelled":true}
```

`notify` needs no answer. hoobot recognises the permission prompt by
`method: "select"` with `"Yes (once)"` in `options`, and handles its own
approval timeout by answering `cancelled`.

## Design

1. **`RpcPermissionUi`** in `cortexcode-code-rpc`, implementing
   `cortexcode_code_permissions::PermissionUi`:
   - `select`: new id, register a pending answer channel, write the request
     through the mode's existing `RpcOutput`, block until answered.
     `None` (cancelled) means deny, as in the terminal UI.
   - `notify`: write the `notify` request; nothing pending.
2. **`handle_line`** delivers `extension_ui_response` to the pending entry
   by id: `value` → `Some(value)`, `cancelled` → `None`. Unknown ids are
   ignored.
3. **Cancel paths answer `None`** (deny) for every pending dialog:
   `abort`, stdin EOF, and session replacement (`new_session`, switch, fork).
   A dialog must never be left blocking a tool.
4. **Wiring:** `build_permission_gate` takes the CLI mode instead of a bool:
   interactive → `TuiPermissionUi`, rpc → `RpcPermissionUi`,
   print/json → `None` (unchanged).
5. **Threading:** the gate is synchronous and blocks. It must not block the
   task that reads stdin, or the answer can never arrive. Check where
   `PermissionGate::request` runs in RPC mode; if it's on a runtime worker,
   wrap the wait in `tokio::task::block_in_place` or run the stdin reader on
   its own thread.
6. **Warm subagents:** `cortexcode-code-subagents/src/warm.rs` drives
   `--mode rpc` children through `RpcClient`, which doesn't answer UI
   requests. Those children would block. Keep today's behaviour for them by
   having the pool spawn workers headless (an internal env var read by
   `run_rpc_mode`, like the existing `CORTEXCODE_*` subagent vars).
   Cold subagents use `--mode json` and are unaffected.
7. **"Always"** keeps writing the global config. hoobot never offers it.

Not included: `confirm`, `input` and `editor` dialogs, and an RPC host for the
`ask_options` tool. Nothing needs them yet.

## Plan

1. `RpcPermissionUi` + response routing + cancel paths, with unit tests in
   `cortexcode-code-rpc` using the faux provider:
   - gated tool emits a `select` request; `Yes (once)` runs it; `No (block)`
     and `cancelled` deny it;
   - `auto_allow` tool runs with no request;
   - `abort` and stdin EOF while a dialog is open deny and don't hang;
   - unknown response id is ignored.
2. Wire it in `build_permission_gate`; headless env for warm workers, with a
   test that a warm worker still runs bash without a request.
3. Manual check with hoobot on Discord: approve, deny, approval timeout,
   abort during a prompt, bot restart with `--continue`.

Done when `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo nextest run` for the touched crates and `migration/check_dep_firewall.py`
pass, and step 3 has been run. No TS parity run: TypeScript hoocode is no
longer a reference for new work.

## Open

- Should the headless opt-out for warm workers be an env var (internal) or a
  CLI flag?

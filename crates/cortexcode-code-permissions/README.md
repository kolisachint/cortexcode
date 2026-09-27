# cortexcode-code-permissions

hoocode's permission gate (`packages/coding-agent/src/extensions/core/permission-gate.ts`,
pinned v0.5.89) as a [`PermissionGate`](../cortexcode-agent-types):

- Hard enforcement from the active mode in the merged `hoo-config.json`, with or without a UI:
  `denied_tools`, the `enabled_tools` allowlist, `denied_bash_commands` / `allowed_bash_commands`.
- With a UI: `allowed_write_paths` for edit/write, `auto_allow`, else an
  `Allow: …` prompt (Yes once / No / Always, which appends to the mode's global `auto_allow`).

The `.webtoolsignore` host rule for webfetch arrives with the web tools (ledger 10.2e).
Bash patterns are JavaScript regexes in hoocode; here they compile with the `regex` crate
(no look-around or backreferences; a pattern that does not compile never matches, as in hoocode).

Volatility tier V (see the migration plan §5.5).

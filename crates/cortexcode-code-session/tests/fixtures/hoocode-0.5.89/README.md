# hoocode 0.5.89 session fixtures

Recorded from the pinned hoocode (`a6cd96e7`) by the Level-2 harness:
`python3 migration/tui-parity/harness.py run <scenario> --app hoocode --keep`, then
copying `~/.hoocode/sessions/**/*.jsonl` from the kept temp HOME.

| File | Source |
|---|---|
| `chat-basic.jsonl` | scenario `chat-basic` |
| `tool-read.jsonl` | scenario `tool-read` |
| `session-mixed.jsonl` | scenario `session-mixed` (thinking, bash + permission, failing read, 2 prompts) |
| `all-entry-types.jsonl` | hand-written from `packages/coding-agent/docs/session-format.md` and `packages/agent/src/harness/types.ts` |
| `legacy-v1.jsonl`, `legacy-v2-hook.jsonl` | hand-written from `test/session-manager/migration.test.ts` |

Re-record only when moving the pin, and never edit recorded files by hand.

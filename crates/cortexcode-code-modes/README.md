# cortexcode-code-modes

The mode system of hoocode's built-in `hoo-core` extension, ported natively
(`packages/coding-agent/src/extensions/core/{modes,config}.ts`, `core/mode-prompts.ts`, pinned
v0.5.89):

- `config` — `hoo-config.json` (agent dir, then `.cortexcode/hoo-config.json` in the project):
  read, merge rules, write.
- `prompts` — the shipped ask/plan/build/debug prompts and `/grill` phases (hoocode's
  `templates/modes` and `templates/prompts/grill-*.md`, verbatim).
- `plan` — plan-file sections, `/approve`, `/grill` and `/goal` messages.
- `extension` — `ModesExtension`: resolves the active mode for a session, appends the mode prompt
  (`<!-- hoo-core: mode=… -->`) through the `before_agent_start` hook, and runs the commands as
  [`ModeAction`]s for the UI to carry out (notify, follow-up message, reload, new session,
  autonomous loop).

Volatility tier V (see the migration plan §5.5).

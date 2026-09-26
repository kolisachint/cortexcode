# cortexcode-code-tools-optin

Port of hoocode `core/tools/todo.ts` (TodoWrite, over the task store) and the tool half of
`extensions/core/ask-options.ts` (v0.5.89). The options pane itself is TUI work (11.3); the
host that shows it, and the `/loop` state, reach the tool through `AskOptionsHost`.

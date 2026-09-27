# cortexcode-code-subagents

Subagent orchestration for the cortex coding agent

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Ports hoocode's subagent machinery: the child-process pool (`pool`: children run
`cortex --mode json --task-id <id>`, a verified `result.json` settles them), the
lifeguard that reaps silent or overdue children, the depth guard and dispatch
evaluator, token budgets, `result.json` building and verification, and model
categories (`fast` / `standard` / `capable`).

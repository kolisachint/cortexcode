# Level-2 parity scenarios

Each `*.json` file is one scenario, run against the real hoocode (pinned build in
`target/hoocode-pin`) and the real `cortex` binary in identical tmux terminals.

```jsonc
{
  "description": "what is exercised",
  "phase": "ledger task id that owns this scenario",
  "terminal": {"cols": 100, "rows": 30},          // fixed size; optional term/colorterm
  "args": ["--offline", "--provider", "mock", "--model", "mock-model"],  // default
  "files": {"notes.txt": "..."},                  // seeded into the workspace (cwd)
  "git": false,                                   // git init the workspace
  "settings": {},                                 // written to ~/.hoocode and ~/.cortexcode settings.json
  "env": {},                                      // extra env (API keys are never inherited)
  "llm": [ {"text": "...", "thinking": "...", "tool_calls": [{"id": "...", "name": "read", "arguments": {}}]},
           {"error": "boom", "status": 500} ],     // one entry per model request, see mockllm.py
  "compare": "style",                             // "style" (default: text + colors/attrs) or "text"
  "compare_requests": false,                      // also require identical model requests
  "request_fields": ["messages", "tools"],        // subset compared when compare_requests
  "normalize": [ {"pattern": "regex", "replace": "x", "style": "optional forced style"} ],
  "steps": [
    {"wait_for": "regex", "timeout": 15},
    {"wait_gone": "regex"},
    {"wait_stable": 1.0},                         // screen unchanged for N seconds
    {"wait_exit": true},                          // app process exited (print mode)
    {"type": "literal text"},
    {"keys": ["Enter", "C-c", "Escape", "Up", "Tab"]},   // tmux key names
    {"sleep": 0.5},
    {"snapshot": "name", "contains": ["..."], "not_contains": ["..."], "history": false}
  ]
}
```

Rules:

1. A scenario must pass `harness.py selfcheck <name>` (hoocode renders identically twice)
   before it can be used to mark a task done.
2. Prefer `wait_for` / `wait_stable` over `sleep`.
3. Global normalization lives in `../normalize.json`. Add scenario-local rules only for
   scenario-specific randomness, and document why.
4. A scenario is owned by exactly one ledger task (`phase`), but may be listed as an
   L2 gate by several tasks.

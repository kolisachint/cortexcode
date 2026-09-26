# cortexcode-code-paths

Where the coding agent keeps its files: hoocode `packages/coding-agent/src/config.ts` (the
app identity, directory and env-override half; v0.5.89) and `src/utils/paths.ts`.

- Writes go to `~/.cortexcode/` (project: `.cortexcode/`); `resolve_agent_file` reads a
  file from `~/.hoocode/` when only that one exists (plan §10.6).
- Env overrides accept `CORTEXCODE_*`, `CORTEX_*` (the names the help text shows) and
  hoocode's `HOOCODE_*`: `*_CODING_AGENT_DIR`, `*_CODING_AGENT_SESSION_DIR`,
  `*_USER_AGENTS_DIR`, `*_SHARE_VIEWER_URL`.

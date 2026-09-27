# cortexcode-code-resources

Resources for the `cortex` coding agent, ported from hoocode `packages/coding-agent`
(pinned v0.5.89):

- `frontmatter` — `utils/frontmatter.ts` (YAML via `cortexcode-agent-harness`).
- `source_info`, `diagnostics` — `core/source-info.ts`, `core/diagnostics.ts`.
- `agent_frontmatter` — `core/agent-frontmatter.ts`: agent definitions and the Claude Code
  tool-name shim (`normalize_tools`).
- `skills` — `core/skills.ts`: discovery (`SKILL.md` roots, ignore files, `.claude/skills`,
  plugin roots skipped), validation, and the `<available_skills>` prompt block.
- `prompt_templates` — `core/prompt-templates.ts`: prompt/slash-command templates, argument
  parsing and `$1` / `$@` / `$ARGUMENTS` / `${@:N:L}` substitution.
- `slash_commands` — `core/slash-commands.ts`: the built-in slash command list.

Volatility tier V (see the migration plan §5.5): these formats follow upstream.

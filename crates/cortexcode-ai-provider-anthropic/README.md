# cortexcode-ai-provider-anthropic

Anthropic provider for cortex AI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Streams completions from the Anthropic Messages API (`POST /v1/messages`,
`stream: true`), translating Anthropic's SSE event stream into the shared
`cortexcode_ai_types::AssistantMessageEvent` sequence used by the agent loop.

Supports text, extended thinking, tool use, image input, prompt caching
(`cache_control`), and both API-key (`ANTHROPIC_API_KEY`) and OAuth
(`ANTHROPIC_OAUTH_TOKEN`) credentials.

Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/anthropic.ts`.

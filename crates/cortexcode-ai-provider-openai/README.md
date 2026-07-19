# cortexcode-ai-provider-openai

OpenAI provider for cortex AI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Streams completions from the OpenAI Chat Completions API (`POST
/chat/completions`, `stream: true`), translating the SSE event stream into
the shared `cortexcode_ai_types::AssistantMessageEvent` sequence used by the
agent loop.

Supports text, tool calls, image input, `reasoning_effort` for reasoning
models, and `OPENAI_API_KEY` credentials. Provider-specific `compat` quirks
(zai/together/moonshot/openrouter/deepseek-specific request shaping) are not
yet ported — see the migration design doc's stated non-goal of full parity
in the initial pass.

Ported from TypeScript `@kolisachint/hoocode-ai` →
`providers/openai-completions.ts`.

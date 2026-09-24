# cortexcode-ai-registry

Port of hoocode `packages/ai/src/api-registry.ts`, `stream.ts` and
`providers/register-builtins.ts` (pinned v0.5.89): streams are dispatched on
`model.api`, not on the provider name, so every provider whose models use an API
(e.g. the ~25 OpenAI-compatible ones on `openai-completions`) works without code
changes. Extensions can register or replace an API (`register_api_provider`).

Built-in APIs registered today: `anthropic-messages`, `openai-completions`,
`azure-openai-responses`, `google-generative-ai`, `google-vertex`. Not yet ported:
`openai-responses` (ledger 8.3), `openai-codex-responses` (8.4a), `google-gemini-cli` (8.4c).

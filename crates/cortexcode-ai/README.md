# cortexcode-ai

Umbrella crate for the cortex AI namespace.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Depend on this single crate to pull in every `cortexcode-ai-*` leaf:

- `cortexcode_ai::types` — shared types (`Model`, `Context`, `AssistantMessage`, ...)
- `cortexcode_ai::stream` — channel-backed event stream
- `cortexcode_ai::models` — model registry
- `cortexcode_ai::env` — API-key environment-variable detection
- `cortexcode_ai::util` — JSON repair, hashing, header/overflow utilities
- `cortexcode_ai::oauth` — Anthropic and GitHub Copilot OAuth flows
- `cortexcode_ai::images` — OpenRouter image generation
- `cortexcode_ai::provider_anthropic` / `provider_openai` / `provider_google` /
  `provider_azure` / `provider_faux` — streaming LLM providers

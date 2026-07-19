# cortexcode-ai-images

Image generation for cortex AI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Generates images via OpenRouter's Chat Completions API (`modalities:
["image", ...]`) — a single non-streaming request/response, unlike the text
providers' SSE streams. Supports text + reference-image input, text +
generated-image output, usage/cost accounting, and `OPENROUTER_API_KEY`
credentials.

Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/images/*`.

# cortexcode-ai-provider-google

Google Gemini / Vertex AI provider for cortex AI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Streams completions from the `:streamGenerateContent?alt=sse` REST endpoint
shared by Google Generative AI (`stream`) and Vertex AI (`stream_vertex`),
translating the SSE event stream into the shared
`cortexcode_ai_types::AssistantMessageEvent` sequence used by the agent loop.

Supports text, thinking (including Gemini 3 `thinkingLevel` and budget-based
models), tool calls, tool-result images (inlined for Gemini 3+, separate turn
for older models), and `GEMINI_API_KEY` credentials.

Vertex AI credential support in this pass is limited to an already-minted
OAuth2 access token (`GOOGLE_VERTEX_ACCESS_TOKEN` or an explicit `api_key`).
Full Application Default Credentials (service-account JSON key parsing, RS256
JWT signing, and token exchange) is not yet ported.

Ported from TypeScript `@kolisachint/hoocode-ai` → `providers/google.ts`,
`providers/google-vertex.ts`, `providers/google-shared.ts`.

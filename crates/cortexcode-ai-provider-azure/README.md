# cortexcode-ai-provider-azure

Azure OpenAI provider for cortex AI.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Streams completions from Azure's OpenAI Responses API (`POST
{base_url}/responses?api-version={version}`, `stream: true`), translating
the SSE event stream into the shared
`cortexcode_ai_types::AssistantMessageEvent` sequence used by the agent loop.

Supports text, reasoning summaries, tool calls, tool-result images,
deployment-name resolution (`AZURE_OPENAI_DEPLOYMENT_NAME_MAP`), Azure host
base-URL normalization, and `AZURE_OPENAI_API_KEY` credentials.

Simplification vs. the TypeScript source: cross-provider reasoning-item ID
pairing (replaying `reasoning`/`function_call` items produced by a
*different* provider) is not ported — each tool call's wire `call_id` is used
directly. See the module docs in `src/request.rs` for details.

Ported from TypeScript `@kolisachint/hoocode-ai` →
`providers/azure-openai-responses.ts`, `providers/openai-responses-shared.ts`.

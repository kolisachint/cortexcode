# cortexcode-ai-provider-google-gemini-cli

The `google-gemini-cli` API: Google's Cloud Code Assist endpoint
(`v1internal:streamGenerateContent`), used by the `google-gemini-cli` (Gemini CLI) and
`google-antigravity` providers. Port of hoocode `providers/google-gemini-cli.ts` (v0.5.89).

The API key is the JSON `{"token": …, "projectId": …}` that the Google OAuth providers
(`cortexcode-ai-oauth-google`) return from `get_api_key`. Message and tool conversion is shared
with `cortexcode-ai-provider-google`.

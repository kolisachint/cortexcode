# cortexcode-ai-oauth-google

The two Google Cloud Code Assist logins: `google-gemini-cli` (Gemini CLI) and
`google-antigravity` (Antigravity). Port of hoocode `utils/oauth/google-gemini-cli.ts`,
`google-antigravity.ts` and `google-oauth-client.ts` (v0.5.89).

Both are authorization-code + PKCE flows with a local callback server (ports 8085 and 51121)
racing a pasted redirect URL, followed by Cloud project discovery or provisioning
(`loadCodeAssist` / `onboardUser`). Google's token endpoint needs a client secret, and no
client ships with cortexcode: set `CORTEXCODE_GEMINI_CLI_CLIENT_ID` /
`CORTEXCODE_GEMINI_CLI_CLIENT_SECRET` or `CORTEXCODE_ANTIGRAVITY_CLIENT_ID` /
`CORTEXCODE_ANTIGRAVITY_CLIENT_SECRET` (the `HOOCODE_*` names are honored too).

The credentials carry `projectId` (and `email`) as extra fields; `get_api_key` returns the
JSON `{"token", "projectId"}` the `google-gemini-cli` provider expects.

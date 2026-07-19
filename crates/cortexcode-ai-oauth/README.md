# cortexcode-ai-oauth

OAuth flows for cortex AI providers.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Provides the pure request/response logic for provider OAuth flows:

- **`anthropic`** — authorize-URL building, pasted-code/redirect-URL
  parsing, authorization-code exchange, and token refresh (PKCE, Claude
  Pro/Max).
- **`github_copilot`** — the full device authorization grant (start, poll,
  Copilot token exchange/refresh) and API base-URL derivation from a token's
  `proxy-ep`.
- **`pkce`** — PKCE verifier/challenge generation (SHA-256 + base64url).

Interactive concerns — running a local HTTP callback server, opening a
browser, prompting the user — are left to the caller (an interactive CLI/TUI
layer); this crate only implements the network calls and pure parsing those
flows need.

Ported from TypeScript `@kolisachint/hoocode-ai` → `utils/oauth/*`.

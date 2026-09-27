# cortexcode-code-auth

Credential storage for the `cortex` CLI. Port of hoocode
`packages/coding-agent/src/core/auth-storage.ts` (pinned v0.5.89).

- `auth.json` in the agent dir, byte-compatible with hoocode: a JSON object from provider id
  to `{"type": "api_key", "key": ...}` or `{"type": "oauth", "refresh", "access", "expires", ...}`,
  written pretty-printed (2 spaces) with mode `0600`.
- Writes and OAuth refreshes hold the same lock hoocode uses (`proper-lockfile`: a
  `auth.json.lock` directory, stale after 10 s / 30 s), so cortex and hoocode can share one file.
- API key priority: runtime override (`--api-key`), stored API key (resolved like models.json
  values: `!command`, env var name, literal), stored OAuth token (refreshed under the lock),
  environment variable, then the fallback resolver.
- `AuthStorage` implements `cortexcode_code_models::AuthLookup`, and
  `AuthStorage::model_modifier` gives the registry the OAuth providers' `modifyModels` pass.

Volatility tier V (see the migration plan §5.5): the file format follows upstream.

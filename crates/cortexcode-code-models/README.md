# cortexcode-code-models

Model registry for the `cortex` CLI. Port of hoocode
`packages/coding-agent/src/core/model-registry.ts` (pinned v0.5.89): built-in models
from `cortexcode-ai-models`, plus custom providers, provider overrides and per-model
overrides from `models.json`, and request auth (`apiKey` / `headers` / `authHeader`)
resolved like `resolve-config-value.ts` (`!command`, environment variable, or literal).

Volatility tier V (see the migration plan §5.5): the models.json schema follows upstream.

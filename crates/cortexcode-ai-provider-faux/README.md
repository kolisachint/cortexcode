# cortexcode-ai-provider-faux

Faux / test provider for cortex AI

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

Port of hoocode `providers/faux.ts`: `register_faux_provider()` registers a
provider on the API registry that streams queued assistant messages (or
factory results) with estimated usage, simulated prompt caching per
`sessionId`, optional `tokens_per_second` pacing and abort handling.
`FauxProvider::stream_fn()` gives the same stream function without registering.

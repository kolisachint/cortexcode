# cortexcode-code-rpc

RPC mode for the cortex coding agent (`cortex --mode rpc`): JSON commands on
stdin, responses and session events on stdout, one JSON object per line. The
protocol is hoocode's (`packages/coding-agent/docs/rpc.md`).

- `run_rpc_mode` over an `RpcHost`: `RuntimeHost` (an `AgentSessionRuntime`, so
  `new_session`, `switch_session`, `fork` and `clone` work) or `SingleSessionHost`.
- `client::RpcClient`: drives a `--mode rpc` process (hoocode's `rpc-client.ts`).

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

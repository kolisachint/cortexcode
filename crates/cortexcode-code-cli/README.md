# cortexcode-code-cli

CLI argument parsing and mode dispatch for the `cortex` coding agent.

Part of the [cortexcode](https://github.com/kolisachint/cortexcode) Rust workspace.

- `args`: exact port of hoocode's `cli/args.ts` (`parseArgs`, `printHelp`), with the
  pinned flag set and its quirks (`-nt`-style short aliases, unknown `--flags` captured
  as extension flags, `-p <prompt>`).
- `run`: the `main.ts` composition: diagnostics, `--version`/`--help`, app-mode
  resolution, then print / json / rpc / interactive dispatch. Flags that parse but whose
  behavior is not ported yet fail with `Error: <flag> is not yet supported by cortex`.

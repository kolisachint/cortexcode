# hoocode 0.5.89 replay fixtures (Level 1, task 13.2)

Recorded from the pinned hoocode (`a6cd96e7`) by
`python3 migration/tui-parity/harness.py record <scenario|all>` for the scenarios listed in
`migration/tui-parity/replay.json`. Each scenario runs headless (stdin a pipe or /dev/null,
stdout/stderr to files) against `mockllm.py`, twice; the recording is kept only when both runs
render identically.

Per scenario directory:

| File | Content |
|---|---|
| `meta.json` | pin commit, the run's temp `HOME`/`WORK`/`TMP` (for normalization), exit status, work-file map |
| `stdout`, `stderr` | raw process output |
| `requests.jsonl` | every model request (`{"body", "path"}`) the mock received |
| `sessions/N.jsonl` | session files left under `~/.hoocode/sessions` |
| `file-N` | `work_files` of the scenario |

The normalized rendering is the insta snapshot `tests/snapshots/replay__<scenario>.snap`.
`cargo test -p cortexcode-code-main --test replay` checks that the Rust normalizer renders
these raw files to exactly that snapshot, then runs `cortex` the same way and asserts the same
snapshot.

Re-record only when moving the pin. Never edit these files or the snapshots by hand, and never
`cargo insta accept` cortex output into them.

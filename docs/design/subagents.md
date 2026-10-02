# Subagents: the process model, and what was fixed

Status: **as-built, 2026-10-03.** The original design in this file (in-process sessions,
delta token accounting, enforced budgets, a provider-error classifier enum) was reviewed
against the code and the recorded failures in October 2026 and **rejected in three places**.
What shipped is the child-process model with seven targeted fixes. The in-process migration
is still open; see §6.

Not a migration-ledger task: the migration is paused and this was taken out of it.

## 1. The problem, and what the evidence actually said

Subagents were failing on every recorded run. In `hoobot/.cortexcode/dispatch/`, a clean
success deletes its dispatch dir (`pool.rs`, clean-success branch), so every dir left on disk
is a failure. **Ten dirs, zero successes.**

| Outcome | Count | What the parent was told |
|---|---|---|
| `timeout` | 4 | `task_timeout`, no partial work |
| `cancelled` | 3 | `subagent cancelled` |
| `stalled` | 1 | a 10 KB stdout dump |
| `failed` | 1 | `Provider finish_reason: error` |
| *(no `output.json`)* | 1 | nothing |

Six causes were written up. Three of the six were wrong on inspection, and two of the three
surviving causes were mis-diagnosed. The corrections are the most useful part of this
record, so they come first.

### Corrections to the original analysis

**The load multiplier does not make subagents *more* likely to be killed — the opposite.**
`lifeguard.rs` re-arms the timer rather than firing when `mult > 1.0`, and scales the stall
threshold by it too. More concurrency means a genuinely stuck task is reaped *later*, up to a
4x ceiling. And it never engaged in any recorded failure: all four timeouts fired at exactly
300s = 5 min x 1.0 multiplier, because concurrency was 1.

**`output.json` was not an unbounded transcript.** Capture is a 256 KB tail
(`MAX_CAPTURED_STREAM_CHARS`, `append_tail`). The recorded 267-275 KB files were a *saturated*
buffer — the head was already discarded. "Redundant" was right; "grows without bound" was not.

**The timeout table matched no shipped agent, and that was the proximate cause of the
recurring timeouts.** `base_timeout_ms` keyed on `edit | test | review`; we ship
`code-review`, `explore`, `general-purpose`, `plan`, `security-review`. **No arm matched**,
so every agent silently got the 5-minute default. hoocode has the same dead table against the
same agent names — this is a faithful port of an upstream bug, and it is why three
`code-review` runs died holding 220-297s of finished work.

**The region errors are not in the artifacts.** `400 Upstream request failed: This Go model
requires Global regions` appears in **zero** of the ten dispatch dirs. It may well have been
seen live in the parent session, but the evidence base does not carry it, and the three most
recent failures ran on `anthropic/claude-opus-5-5` and still timed out at 300s.

**`capable` is not the priciest model.** The configured `defaultProvider`/`defaultModel` wins;
priciest is only the fallback. `fast` is the cheapest *among models priced at or below*
`capable`. The original write-up simplified this wrongly; the code comment beside it was right.

**`dispatch.rs` was never the stdout framing.** It is `dispatch-evaluator.ts`: the depth guard
and complexity heuristic. Framing and `SubagentStdoutLine` live in **`events.rs`**.

### Two causes that survived, and are worse than described

**Post-hoc salvage does not work.** The obvious fix — reconstruct a result from `session.jsonl`
when a run is killed — produces garbage. `derive_summary` walks back for the last assistant
text block, and in the recorded runs:

| runs | assistant text messages | what salvage would return |
|---|---|---|
| 5 of 10 | **zero** | *"Task completed with no textual summary."* |
| 4 of 10 | 1 | a progress note (*"I'll start by exploring…"*) |
| 1 of 10 | 3 | a real summary — but it died anyway |

The one run that had a real summary (`dispatch-1790866463943-0ozuar`) still failed, because
`result.json` is written only after `prompt()` returns cleanly. **Salvage has to be
pre-armed: the child must be told to wrap up before the kill, not reconstructed after it.**

**Delta token accounting would not have fixed the budget.** `usage.totalTokens` is the
*context size* of the turn, so summing it is quadratic — the recorded run reported 1 654 795
against a 35 000 budget when its real generated total is 4 111. The obvious repair is to
difference it. Measured across all eight runs with a transcript, the delta still crosses 35k
at **turn 1 or 2 in 7 of 8**, because turn-1 context alone is 25k-62k (system prompt + tool
schemas) and no subagent can shrink it. Delta accounting plus an enforced budget would have
hard-stopped nearly every subagent on its first turn. The metric was the problem, not the
arithmetic: `output` tokens run 1.2k-22k, and they are the only figure that tracks work done.

## 2. What shipped

Seven fixes to the child-process model. The pool's public API is unchanged; every consumer
(Task tool, TaskOutput, inbox, TUI task panel, `/subagent`, the warm pool) ports untouched.

| | Fix | Where |
|---|---|---|
| T1 | Timeout table keyed on agents we do not ship, so **every** agent got the 5-minute default. Real arms added; the warm pool's flat 180s now tracks the same table | `lifeguard.rs`, `warm.rs` |
| T2 | `used` summed cumulative `totalTokens` (quadratic). Now sums `output + cacheWrite`; context reported separately as `peak_context` | `token_budget.rs` |
| T3 | Children are killed at their deadline with `result.json` never written. The parent now passes `--deadline-ms`; the child wraps up 90s before it and settles `partial` | `pool.rs`, `runtime.rs`, `result.rs`, `args.rs` |
| T4 | Killed tasks returned before the fallback ladder was consulted, so a run that died because its model was unreachable never retried. They consult it now — except a user cancellation, which is never retried | `pool.rs` |
| T5 | The classifier matched neither `requires Global regions` nor `finish_reason: error`. Both added | `pool.rs` |
| T6 | With `complexity` passed, the fallback re-resolved the same category and retried the identical model. A new `inherited_model` field carries the parent's concrete model | `pool.rs`, `tools.rs` |
| T7 | `output.json` embedded a 256 KB stdout tail. Now outcome + cause + an 8 KB stderr tail | `pool.rs` |

### T3 in detail — the only structural change

`--deadline-ms` is an internal flag (not in `--help`, like `--task-id`). The pool sends the
**base** per-agent deadline, not the load-scaled budget: under load the parent widens its
kill, so the wrap-up always lands first. Ninety seconds of lead covers the one or two turns a
model needs to turn findings into a summary — recorded subagent turns took 5-65s.

The pool side needed no change. `settle` already treats a valid `result.json` as success even
when the task was killed (`kill_reason.filter(|_| !verification.valid)`), so all the child has
to do is produce one. A deadline wrap-up settles `partial` with confidence 0.6, which clears
the verifier's 0.5 floor.

## 3. Deviations from hoocode

Every one is commented in the source with the evidence.

- **Timeout table** — hoocode's arms are kept for plugin agents that use those names, plus
  real arms for the shipped five. (`lifeguard.rs`)
- **Warm run timeout** — hoocode's `WARM_RUN_TIMEOUT_MS` is a flat 180s for every agent; we
  use the same per-agent table, never tighter. (`warm.rs`)
- **Token accounting** — `used` counts generated tokens; `peak_context` carries the context
  size. (`token_budget.rs`)
- **`--deadline-ms`** and the wrap-up steer. (`runtime.rs`)
- **Fallback classification** — hoocode's pattern plus region and unexplained-provider-failure
  groups. Kept as one pattern rather than a classifier enum; the alternatives are grouped so
  they can be lifted out later. (`pool.rs`)
- **`inherited_model`** — hoocode falls back to `task.model`, which is a `complexity` category
  when the caller passed one. (`pool.rs`, `tools.rs`)
- **`output.json`** — no embedded `stdout`. (`pool.rs`)

One candidate was **rejected during review**: setting a prose `error` on killed tasks. It
changes a user-visible TUI line from hoocode's `subagent stalled`, and no parity scenario
covers a killed task, so it would have shipped an uncovered divergence inside a correctness
pass. `error` stays unset.

## 4. Verification

- `cargo nextest run --workspace` — **3494 passed**, 0 failed (was 3479; +15 tests).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --check`, `migration/check_dep_firewall.py` — clean.
- **L2 parity: `subagent-task`, `subagent-command`, `json-subagent-child` all pass** against
  real hoocode.

**End to end**, against the release binary and the harness's mock LLM in an isolated `HOME`:

| | duration | requests | status | confidence | steers |
|---|---|---|---|---|---|
| `--deadline-ms 95000` | 44s | 25 | **partial** | 0.6 | 1 |
| control, no flag | 40s | 25 | **complete** | 0.9 | 0 |

Both recovered the same real summary from a run that otherwise would not have. The steer is
delivered exactly once and then persists as ordinary conversation history — verified as one
copy per request body, not re-injection.

A full `Task` tool dispatch — parent, pool, real child process, verified `result.json` — settled
`complete` and the parent received the summary. **The first clean subagent success in the ten
recorded runs.**

### Pre-existing L2 failures, not ours

`harness.py run all` is not fully green. Attributed with a clean `git worktree` at HEAD:

| scenario | ours | clean HEAD | cause |
|---|---|---|---|
| 9 x `print-*` | fail | **fail** | hoocode 0.6.0 ships a `SearchHooCode` tool the port does not implement |
| `config-selector` | fail | **fail** | trailing slash in a config resource path |
| `cd-reload`, `export-import` | invalid | **invalid** | *hoocode itself* fails the scenario |
| 3 x subagent | pass | **pass** | — |

## 5. Decisions taken

- **The budget stays advisory.** T2 made the number honest; nothing enforces it. With `output`
  at 1.2k-22k against a 35k limit, enforcement would essentially never fire. Wall-clock is the
  primary stop, `--max-turns 50` the secondary one, the budget telemetry. Owner, 2026-10-03.
- **The new deadlines stand as written** — `explore`/`plan` 10 min, `code-review`/
  `security-review` 15 min, `general-purpose` 20 min — from four data points. Not measured
  against a task corpus. Owner, 2026-10-03.
- **Track 2 (in-process) not started.** Owner, 2026-10-03.

## 6. Open: the in-process migration

The original design here proposed replacing child processes with tokio tasks. It was not
adopted, for these reasons:

**In favour.** The pattern is precedented twice in-repo — `run_rpc_mode`
(`code-rpc/src/mode.rs`) and `RpcClient::attach` over `tokio::io::duplex`, with tests wiring
them against the faux provider. `warm.rs` is the near-exact template: same shape, RPC child
instead of a task. `AgentSession` is `Send + Sync + 'static` (proven at `session.rs:746`,
where it moves itself into a detached task). It would remove a whole class of transport bugs:
stdout framing, ping-as-liveness, kill data loss, the capture cap.

**Against, and not yet resolved:**

1. **Re-entrant nesting.** The hard part is not depth bookkeeping, it is that an in-process
   child holding `Task` needs a *pool reference* at depth+1, from inside a session the pool
   owns. Unproven.
2. **Depth is propagated through process-global env vars.** `depth.rs` states the premise
   outright. In-process, `SUBAGENT_DEPTH` and `DELEGATE_ALLOW` must become per-run fields —
   `runtime.rs:subagent_tools` does `set_var`/`remove_var` at startup. **This is smaller than
   it looks:** MCP server discovery is `deferred` in the ledger (10.11) and never wired in the
   Rust port, so `SKIP_MCP` and `DEFER_MCP_SCHEMAS` are already dead env vars. The work is
   depth plus delegate-allow.
3. **The tool allowlist is nearly free.** `CreateAgentSessionOptions` already carries `tools`,
   `disallowed_tools` and `permission_gate` per session — what `--tools` does today.
4. **Panic containment has no precedent.** The only `catch_unwind` in the workspace wraps a
   synchronous count callback. Nothing wraps a `tokio::spawn` body, and the pool's state is a
   `Mutex`, so a panic mid-run poisons it for the parent.
5. **Depth caps are budgeted in processes** (`depth.rs`: *"worst case 5 x (2^3-1) = 35
   processes"*). 35 concurrent sessions in one address space is a different blast radius.
6. **The test seam is the real cost.** Every pool test substitutes a child via
   `executable`/`prefix_args` — a *path* seam — using `/bin/sh` mocks; 31 usages across 5 test
   files. There is no injectable runner, and `cortexcode-ai-provider-faux` is used by no
   subagent test. "Feature-flagged so both paths run against the same tests" is not achievable
   without building that seam first.

If it is taken up, build the seam against the faux provider **first**, and treat (1) as the
gate: if re-entrant nesting does not work cleanly, keep the process model permanently and
spend the effort on liveness instead. Child-process isolation is worth real money.

## 7. Footnote: the evidence is not permanent

`sweep_old_agents` deletes dispatch dirs older than 24h when the pool starts — hoocode-faithful.
Five of the ten recorded runs have already been swept. The figures this document cites are
baked into the tests as literals (`RECORDED_TURNS` holds the exact 15 context/output pairs;
the timeout, agent-type and status tables are in assertions), so nothing needed is lost. Copy
the survivors in as fixtures if the raw evidence is worth keeping.
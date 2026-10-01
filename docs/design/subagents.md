# Subagents: in-process sessions

Status: **designed 2026-10-01, not started.** Architecture only; no code.
Not a migration-ledger task: the migration is paused and the user took this
out of it. The Rust subagent system fails almost every run, so this is a
correctness project before it is a design project.

## Problem

Subagents do not work. In the eight recorded runs in
`hoobot/.cortexcode/dispatch/` (2026-10-01), **zero succeeded**. A clean
success deletes its dispatch dir (`pool.rs:1290`), so every dir still on disk
is a failure:

| Outcome | Count | What the parent was told |
|---|---|---|
| `timeout` | 3 | `task_timeout`, no partial work |
| `cancelled` | 3 | `subagent cancelled` |
| `stalled` | 1 | a 10 KB stdout dump |
| `failed` | 1 | `Provider finish_reason: error` |
| *(no `output.json` at all)* | 1 | nothing |

The same session shows the parent seeing:

```
- explore#1  failed ✗  — Task failed: Provider finish_reason: error
- explore#2  cancelled ⊘  — subagent cancelled
```

Six independent causes, each verified in the logs and in the code.

## Root causes

### 1. The token budget is quadratic and always fires

`token_budget.rs:176` accumulates `message.usage.totalTokens` per assistant
message. That field is the **cumulative context size of the turn**, not a
delta. Measured on `dispatch-1790866463943-0ozuar`:

| | |
|---|---|
| per-turn `totalTokens` | 4,186 → 6,968 → … → 79,051, monotonic |
| sum of those values | **1,654,795** |
| `used` in `budget.json` | **1,654,795** (exact match) |
| budget | 35,000 |

So every run reports ~20x its real usage. All 8 runs have
`"exceeded": true`; several are 47x over.

### 2. The budget cannot stop anything

Its own module doc says *"Advisory only: it never stops a subagent (the hard
stop is `--max-turns`)"*. Hitting the limit injects one line of text — "You
are near token limit. Summarize and write result.json now." — into a child
that keeps going. The one run that got furthest spent `input 188553`,
`cacheRead 1458688`, **$1.12**, and still ended `failed`.

### 3. The model tier is chosen by price, with no reachability check

`model_categories.rs` derives `fast` as the **cheapest model in the catalog**
and `capable` as the priciest, sorting on `cost.input + cost.output`. Nothing
asks whether the account can actually call it. Live proof during this
investigation: every dispatched subagent died with

```
400 Upstream request failed: This Go model requires Global regions.
429 Go usage limit exceeded (… asked to wait 2h 8m …)
```

### 4. The fallback ladder is too narrow, and hangs skip it entirely

`INHERITED_MODEL_FALLBACK_ERROR` (`pool.rs:45`) matches 429, quota, auth and
"model unavailable". It does **not** match `requires Global regions`, and does
not match `Provider finish_reason: error` — the error that actually killed
the one run that reached completion. Separately, the killed-task branch
(`pool.rs:1234-1259`) returns before any fallback is considered, so
`timeout`, `stalled` and `cancelled` **never** retry on another model even
when the model is the cause.

### 5. Heartbeat measures liveness, not progress

The child pings every 30s (`runtime.rs:1070`), but `pool.rs:1121` treats
*any* stdout byte as a heartbeat, and the pings share the same channel the
main loop drains. A child blocked in a long call stops emitting pings while
the process is alive. `dispatch-1790868633330-ow9n4f` emitted five pings,
**zero `message_end` events**, and was reaped `stalled` — mid-way through
writing a whole crate.

Worse, the stall threshold *grows with load*: `load_multiplier()` adds 50% per
concurrent task up to 4x (`lifeguard.rs:37`), so running subagents in
parallel makes each one **more** likely to be killed for being slow. The base
hard timeout is 5 minutes (`lifeguard.rs:25`).

### 6. The failure report is the whole transcript

`write_output_json` (`pool.rs:829`) stores the entire captured stdout. Real
files are 267–275 KB, prompt included. The parent gets that instead of an
error, and on timeout the partial work is discarded.

## Decisions (2026-10-01, with the owner)

1. **Subagents run in-process**, as sessions on tokio tasks, not child
   processes. Removes stdout parsing, the blocked-heartbeat writer, and
   process-kill data loss.
2. **Budget is enforced, and hitting it salvages the work**: one final turn
   to write `result.json`, settled as `partial`.
3. **Validate the model before spawning, and fall back on any provider
   rejection** — including region, quota and generic provider errors.
4. **Containment**: isolated session objects, a per-agent tool allowlist, and
   panic catching at the task boundary.
5. **Dispatch directories stay**, written natively instead of parsed.
6. **Data migration is automatic, one-time, with a backup** (see
   `naming-and-paths.md`).

## Design

### Execution model

`SubagentPool` keeps its public surface — `spawn`, `cancel`, `list`,
`wait`, the `PoolEvent` stream, `SubagentPoolTask`, `SubagentResult` — but a
task becomes a tokio task instead of a `Command`:

```rust
// cortexcode-code-subagents/src/runner.rs (new)
pub struct SubagentRun {
    task:  SubagentPoolTask,
    sess:  crate::agent_session::Session,   // its own session object
    model: ResolvedModel,
    budget: TokenBudget,
    last_progress: Arc<AtomicU64>,          // ms, bumped on every event
    cancel: CancellationToken,
}
```

The lifecycle mirrors today's, minus the process:

| Today | In-process |
|---|---|
| spawn `cortex --mode json --task-id …` | `Session::new(AgentSessionConfig)` + `prompt(task)` |
| parse `message_end` for usage | subscribe to `SessionEvent`, read `usage` on `MessageEnd` |
| `{"ping":true}` heartbeat | `last_progress` bumped by **every** event |
| wait for exit | await the task's `JoinHandle` |
| read `result.json` from disk | the run holds `result_data`; still written to disk |
| `kill_process_tree` on cancel | `sess.abort()` + `cancel.cancel()` + drop |

`Session` already offers what this needs: `Session::new(AgentSessionConfig)`,
`prompt`, `abort`, and a `subscribe` returning a `SessionSubscription`
(`cortexcode-code-agent-session/src/session.rs:523,1294,1590`).

`--mode json` and `--task-id` stay in the CLI for external callers and for
the L2 parity harness; they are simply no longer the pool's transport.

### Containment

Three layers, because in-process means a subagent bug is a parent bug:

1. **Session isolation.** Each run owns its `Session`, its message list and
   its compaction state. No shared `Arc` of conversation state with the
   parent; the task text is passed in as a single user message.
2. **Tool allowlist.** `AgentDefinition` gains a `tools` allowlist, resolved
   at dispatch. `explore` and `plan` get reads only (`read`, `grep`, `glob`,
   `search`, `list`, `webfetch`); `code-review` adds `bash` read-only
   commands; `general-purpose` gets the full set. An agent with no
   allowlist gets the parent's set minus `Task`, so it cannot nest past the
   depth guard by accident.
3. **Panic catching.** The run body is wrapped so a panic becomes a
   `ResultStatus::Failed` with the panic message, never a dead parent. A
   poisoned lock is recovered with `unwrap_or_else(|e| e.into_inner())`, the
   pattern the pool already uses.

### Liveness: progress, not pings

`last_progress` is bumped by **every** `SessionEvent` the run receives:
message start/end, tool call, tool result, compaction, notice. The stall
watchdog (`SubagentLifeguard`, kept for its timeout arm) compares
`last_progress`, not process liveness, so a hung provider call is still
caught and a busy child is never killed.

The load multiplier is replaced. A single task's deadline no longer grows
because siblings exist; instead a task gets a fixed progress deadline that
pauses while it is legitimately working:

- **Progress deadline** — no event for 3 minutes → `stalled`.
- **Hard deadline** — wall clock, from the agent definition: `explore` 10
  min, `code-review` 15 min, `general-purpose` 30 min (raised from 5; the old
  value was below real task times, which is why 3 of 8 runs timed out).

### Budget: count deltas, then stop

`TokenBudget` changes in three ways:

1. **Delta accounting.** `used += (totalTokens_n - totalTokens_{n-1})`, with
   `totalTokens_{n-1}` remembered per run. On the run above this yields
   ~79k instead of 1.65M. If a provider reports a total that went backwards
   or restarted, treat it as a new series and reset the baseline.
2. **Hard stop at the limit.** Crossing the limit sets `exceeded`, injects
   the wrap-up instruction, and arms a **final-turn** allowance (one more
   assistant turn, and a wall-clock fence of 2 minutes) for the run to write
   `result.json`. It then settles `partial` — the work so far is returned to
   the parent. If the run does not produce a result within the fence, the
   pool synthesizes a `result.json` from whatever the transcript holds:
   `status: "partial"`, `confidence: 0.5`, the last assistant text as
   `summary`.
3. **Accurate reporting.** `budget.json` gains `used_estimate` next to the
   corrected `used`, so the quadratic figure stays auditable during rollout.

Budgets remain per agent type (`explore`/`plan` 35k, `general-purpose` 60k)
and overridable via the definition, per the owner's choice.

### Model selection: validate, then a ladder

Resolution order for a subagent's model:

1. An explicit `model` on the agent definition.
2. `resolve_model_category` for the requested tier
   (`model_categories.rs:161`), unchanged in spirit — but see step 3.
3. **Validation.** Before spawning, check the resolved `(provider, model)`
   against a cached health table: known-good, unknown, or known-bad with a
   cooldown expiry. Unknown pairs are probed with a one-token request. A pair
   that fails validation is skipped, not used.
4. **Fallback ladder**, tried in order until one survives the first turn:
   the tier model → the next tier down → the **parent's** model (always
   known-good, since the parent is running on it). The ladder is recorded in
   `dispatch-log.json` as `model_attempts`.

The `INHERITED_MODEL_FALLBACK_ERROR` regex is replaced by **provider error
classification**: a shared classifier maps an error to
`Auth | Quota | Region | ModelUnavailable | RateLimit | Transient | Fatal`.
`Auth`, `Quota`, `Region`, `ModelUnavailable` and `RateLimit` all advance the
ladder. `Region` is what today's regex misses, and it is the error that broke
every subagent in this investigation. `Transient` retries with backoff on the
same model.

The health table is process-local and in-memory for now. Persistence is
deliberately left out: a stale "bad model" entry must never outlive a restart,
and a bad entry only costs one skipped spawn.

### Result contract

`ResultStatus` keeps all six variants. Two rules change:

- **`timeout` and `stalled` now salvage.** Both run the same final-turn path
  as the budget stop and settle `partial` when anything was produced. A run
  that produced nothing at all settles `failed` with the concrete cause.
- **`failed` carries a cause enum**, not prose. `SubagentResult.error` becomes
  a structured `FailureCause` (e.g. `Provider(Region)`, `BudgetExceeded`,
  `NoProgress`, `ToolDenied`), rendered to text only at the display boundary.

`OutputVerifier` keeps its checks (summary non-empty, `files_changed` array
of strings, `confidence >= 0.5`, status in `complete|partial|failed`). The
confidence floor is the one rule to revisit: a legitimate `partial` salvage
gets `0.5`, exactly at the threshold, so partial results are accepted by
construction rather than by luck.

### Artifacts

Same directory, same names, written directly by the pool instead of scraped
from a child's stdout:

| File | Written by | Contents |
|---|---|---|
| `dispatch-log.json` | pool | task, agent, depth, reason, complexity, `model_attempts` |
| `budget.json` | `TokenBudget` | `used` (corrected), `used_estimate`, limit, warned, exceeded |
| `session.jsonl` | run | the subagent's own transcript |
| `result.json` | run | the verified result contract |
| `output.json` | pool | a **summary**, not a transcript: status, cause, duration, tokens, the final assistant text |

`output.json` shrinks from ~270 KB to a few hundred bytes. The full
transcript stays available in `session.jsonl`, which is what made this
investigation possible.

### Concurrency

Depth and caps are unchanged in spirit (`depth.rs`), and the load multiplier
is gone. Concurrency is bounded by a semaphore; `CORTEXCODE_SUBAGENT_DEPTH`
and `CORTEXCODE_NESTED_SUBAGENT_CONCURRENCY` keep working. A run is
cancellable at any await point through `abort()` plus the cancellation token,
so cancelling no longer depends on finding a process to kill.

## Migration from the process model

The pool's public API is unchanged, so the Task/TaskOutput tools, the TUI
task panel, `warm.rs` (the RPC warm pool) and the inbox bookkeeping port
without redesign. What goes: `dispatch.rs`'s stdout line framing, the
`SubagentStdoutLine` parser, `kill_process_tree` for subagents, and the
`--mode json --task-id` spawn path. What stays and is re-pointed:
`lifeguard.rs` (progress deadlines), `token_budget.rs`, `output_verifier.rs`,
`depth.rs`, `model_categories.rs`, `inbox.rs`, `result.rs`.

`cortexcode-code-cli/src/runtime.rs` keeps `--mode json` and the 30s ping for
non-pool callers; the pool simply stops depending on them.

## Plan

1. Delta accounting in `TokenBudget` + a regression test on the recorded
   1,654,795 → ~79k case.
2. Provider error classification + the fallback ladder (fixes the failures
   in this document on its own; everything below is about not losing work).
3. Final-turn salvage shared by budget, timeout and stall.
4. `runner.rs`: in-process execution behind the existing pool API, feature-
   flagged so both paths can run against the same tests.
5. Tool allowlist on `AgentDefinition` + depth guard re-check.
6. Structured `FailureCause`; shrink `output.json`.
7. Flip the flag, delete the process path, retire `dispatch.rs` framing.

Steps 1–3 are independently shippable and fix the user-visible failures. If
the in-process move turns out to be too large, stopping after step 3 still
leaves a working subagent system.

## Tests

- **Budget:** delta accounting on a fixture transcript; the recorded
  quadratic case; the final-turn salvage produces a `partial` result.
- **Routing:** region, quota and generic provider errors all advance the
  ladder; `Transient` retries the same model; an exhausted ladder settles
  `failed` with `FailureCause::ModelUnavailable`.
- **Liveness:** a session that stops emitting events is reaped as `stalled`;
  one under load is not; the load multiplier is gone.
- **Containment:** a panicking run yields `failed` and the parent survives;
  `explore` cannot reach a write tool.
- **Parity:** L2 (`migration/tui-parity/harness.py`) — a subagent task renders
  the same in the TUI as under hoocode TS.

## Open

- Should a `partial` result count as success for `TaskOutput`, or keep
  `ok: false` with usable `result_data`? Leaning: usable data, `ok: false`,
  so the parent model knows the work is partial rather than complete.
- Health table persistence across restarts (currently in-memory by design).
- Whether `--max-turns 50` is still the right hard stop once the budget
  enforces itself.
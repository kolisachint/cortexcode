---
name: continue-migration
description: Resume and keep advancing the hoocode → cortexcode Rust migration from exactly where it was left. Use when the user says "continue migration", "continue", "keep migrating", "next migration task", or asks for migration status.
---

# Continue the hoocode → cortexcode migration

State lives in the repo, not in chat history. Read it, pick up the current task, and
work in small verified increments. Keep looping until the user stops you or you hit
a real blocker.

## 0. Orient (every time, ~1 minute)

```bash
cd <cortexcode checkout>
git status --short && git log --oneline -5
python3 migration/ledger.py status
python3 migration/ledger.py next          # CONTINUE / START / RE-VERIFY + the task card
sed -n '1,60p' migration/PROGRESS.md      # latest handoff notes: read before touching code
migration/tui-parity/setup_hoocode.sh     # idempotent: builds the pinned hoocode reference
```

- `docs/design/hoocode-to-cortexcode-migration.md` explains why (§0 audit, §5.5 crate
  split and volatility tiers, §9 phases). `migration/ledger.json` is the what and status.
  If they disagree, the ledger wins; fix the doc.
- Port only from the pinned hoocode at `target/hoocode-pin`, which is checked out at
  `[workspace.metadata.cortex.source].hoocode-commit`. Never port from another hoocode
  checkout or from memory. **Never modify hoocode.**

## 1. Work one task

1. `python3 migration/ledger.py start <id>`. Skip this if `next` said CONTINUE.
2. Read the task's hoocode sources and TS tests in full, at the pin.
3. Check for reuse before writing code: the ecosystem crates in the plan (§3.4) and the
   crates already in this workspace (§3.4.2). Volatile third-party crates go only in the
   adapter crate listed in `migration/dep-firewall.json`.
4. Create new crates when the task names them (follow §4.5, add to the root
   `Cargo.toml` members and `[workspace.dependencies]`, `publish = false` until stable).
   Split existing crates in the task that owns the split. No big-bang restructures.
5. Port the TS tests as Rust tests (Level 1). Keep wire formats byte-compatible with
   hoocode's JSON (sessions, RPC, json mode, settings, auth, models).
6. If the task lists L2 scenarios that don't exist yet, write them in
   `migration/tui-parity/scenarios/` (format in its README), then run
   `python3 migration/tui-parity/harness.py selfcheck <name>`. It must print `stable`.
   The scenario encodes hoocode's behavior, so write it from what hoocode actually
   renders (`harness.py run <name> --app hoocode`, look at
   `target/tui-parity/<name>/hoocode/*.txt`), never from what cortex happens to do.
7. Gate: `python3 migration/ledger.py verify <id>`.
   - Level 1 = fmt + clippy `-D warnings` + tests for the task's crates + dependency
     firewall + the task's `l1_cmds`.
   - Level 2 = each scenario's rendered screen, and optionally its model requests,
     identical to hoocode after normalization.
   - Both pass → `done`. L1 only → `l1_done`, which is fine when the scenario needs a
     later task. Run `cargo test --workspace` before committing as well.
   - To look at a rendered result: open `target/tui-parity/<scenario>/report.md`, or
     `harness.py png <scenario>` and read the PNG.
8. Commit, one task per commit (or a few commits). Use the message `migrate(<id>): <title>`,
   including `migration/ledger.json` and `migration/PROGRESS.md` changes. Push to the
   current branch with `git push -u origin <branch>`. Open a PR only if asked.
9. Write a handoff entry at the top of `migration/PROGRESS.md` log: date, task, what
   changed, what's left, and the exact next step. Keep it short.
10. Go back to 0 and take the next task.

## Rules

- **Never mark done by hand.** Only `ledger.py verify` sets `l1_done`/`done`. Don't edit
  statuses in the JSON except to fix a bookkeeping mistake, and say so in the log.
- **Don't weaken the gates** to go green: no loosening normalization to hide a real
  difference, no deleting snapshots or assertions, no `#[ignore]` on a failing ported
  test. A normalization rule is allowed only for real nondeterminism (random ids,
  temp paths, durations) or branding (hoocode↔cortex), with a comment.
- If a task is too big for one sitting, split it in the ledger (`10.3` → `10.3a`,
  `10.3b` with dependencies) and note why.
- Stuck, or you need a decision (Phase 12 go/no-go, a design choice)? Run
  `ledger.py block <id> "<question>"` or `note`, record it in PROGRESS.md, and ask the user.
- Moving the hoocode pin is a separate, user-approved task (plan §0.1). Never do it as
  a side effect.
- Before the session ends or context gets long: commit, push, and make sure PROGRESS.md
  says exactly where to resume.

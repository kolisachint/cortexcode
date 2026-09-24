#!/usr/bin/env python3
"""Migration ledger CLI: where are we, what's next, and the two-level done gate.

    ledger.py status              # progress per phase + current task
    ledger.py next                # the task to work on now (in_progress, else first ready todo)
    ledger.py show <id>           # full task card: hoocode sources at the pin, TS tests, gates
    ledger.py start <id>          # mark in_progress (only one task at a time)
    ledger.py note <id> <text>    # append a handoff note to the task log
    ledger.py block <id> <text>   # mark blocked with a reason
    ledger.py verify <id>         # run L1 + L2 gates and set l1_done / done from the results
    ledger.py check               # validate ledger structure (CI)

Level 1 (programmatic): cargo fmt --check, clippy -D warnings and tests for the task's
crates, the dependency firewall, and the task's extra ``l1_cmds``.
Level 2 (rendered): every scenario in the task's ``l2`` list passes
``migration/tui-parity/harness.py run`` (hoocode vs cortex, same mock LLM).
A task is ``done`` only when both levels pass; with L1 only it is ``l1_done``.
"""

from __future__ import annotations

import datetime as dt
import json
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "migration" / "ledger.json"
SCENARIOS = ROOT / "migration" / "tui-parity" / "scenarios"
DONEISH = {"done", "l1_done"}
STATUSES = {"todo", "in_progress", "l1_done", "done", "blocked", "deferred"}


def load() -> dict:
    return json.loads(LEDGER.read_text())


def save(led: dict) -> None:
    LEDGER.write_text(json.dumps(led, indent=1, ensure_ascii=False) + "\n")


def task(led: dict, tid: str) -> dict:
    for t in led["tasks"]:
        if t["id"] == tid:
            return t
    sys.exit(f"unknown task {tid}")


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def head() -> str:
    return subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def log(t: dict, kind: str, text: str) -> None:
    t.setdefault("log", []).append({"at": now(), "commit": head(), "kind": kind, "text": text})


def ready(led: dict, t: dict) -> bool:
    by_id = {x["id"]: x for x in led["tasks"]}
    return all(by_id[d]["status"] in DONEISH for d in t["depends"])


def workspace_crates() -> set[str]:
    return {tomllib.loads(p.read_text())["package"]["name"] for p in (ROOT / "crates").glob("*/Cargo.toml")}


# ---------------------------------------------------------------------------


def cmd_status(led: dict) -> None:
    phases: dict[int, dict[str, int]] = {}
    for t in led["tasks"]:
        phases.setdefault(t["phase"], {}).setdefault(t["status"], 0)
        phases[t["phase"]][t["status"]] += 1
    print(f"hoocode pin {led['pin']['hoocode_version']} ({led['pin']['hoocode_commit'][:8]})")
    for ph in sorted(phases):
        counts = phases[ph]
        total = sum(counts.values())
        done = counts.get("done", 0)
        parts = " ".join(f"{k}={v}" for k, v in sorted(counts.items()))
        print(f"  phase {ph:>2}: {done}/{total} done   {parts}")
    cur = [t for t in led["tasks"] if t["status"] == "in_progress"]
    for t in cur:
        print(f"in progress: {t['id']} {t['title']}")
    l1 = [t["id"] for t in led["tasks"] if t["status"] == "l1_done"]
    if l1:
        print(f"awaiting L2: {', '.join(l1)}")


def cmd_next(led: dict) -> None:
    cur = [t for t in led["tasks"] if t["status"] == "in_progress"]
    if cur:
        print(f"CONTINUE {cur[0]['id']}")
        show(cur[0])
        return
    for t in led["tasks"]:
        if t["status"] == "todo" and ready(led, t):
            print(f"START {t['id']}")
            show(t)
            return
    # Nothing ready: maybe an l1_done task can now reach done.
    for t in led["tasks"]:
        if t["status"] == "l1_done":
            print(f"RE-VERIFY {t['id']} (L2 may pass now)")
            show(t)
            return
    print("nothing ready: all remaining tasks are blocked/deferred or waiting on dependencies")


def show(t: dict) -> None:
    pin_dir = ROOT / "target" / "hoocode-pin"
    print(f"\n[{t['id']}] {t['title']}\n  phase {t['phase']} · status {t['status']} · depends {t['depends'] or '-'}")
    print(f"  crates: {', '.join(t['crates']) or '-'}")
    print("  hoocode sources (at pin, in target/hoocode-pin):")
    for s in t["sources"]:
        mark = "" if (pin_dir / s).exists() or not pin_dir.exists() else "  (missing at pin!)"
        print(f"    {s}{mark}")
    if t["ts_tests"]:
        print("  TS tests to port: " + ", ".join(t["ts_tests"]))
    if t["l1_cmds"]:
        print("  extra L1 commands: " + "; ".join(t["l1_cmds"]))
    l2 = t["l2"]
    if isinstance(l2, list):
        for s in l2:
            exists = (SCENARIOS / f"{s}.json").exists()
            print(f"  L2 scenario: {s}{'' if exists else '  (not written yet: write it, selfcheck it)'}")
    else:
        print(f"  L2: {l2}")
    if t["notes"]:
        print(f"  notes: {t['notes']}")
    for entry in t.get("log", [])[-5:]:
        print(f"  log {entry['at']} {entry['commit']} [{entry['kind']}] {entry['text']}")


def run(cmd: list[str] | str, shell: bool = False) -> bool:
    print(f"$ {cmd if isinstance(cmd, str) else ' '.join(cmd)}", flush=True)
    return subprocess.run(cmd, cwd=ROOT, shell=shell).returncode == 0


def cmd_verify(led: dict, tid: str) -> int:
    t = task(led, tid)
    existing = workspace_crates()
    missing = [c for c in t["crates"] if c not in existing]
    if missing:
        print(f"L1 FAIL: crates not in workspace yet: {missing}")
        return 1
    pkgs = [a for c in t["crates"] for a in ("-p", c)]
    l1 = run(["cargo", "fmt", "--all", "--", "--check"])
    if pkgs:
        l1 = run(["cargo", "clippy", *pkgs, "--all-targets", "--", "-D", "warnings"]) and l1
        l1 = run(["cargo", "test", *pkgs]) and l1
    l1 = run([sys.executable, "migration/check_dep_firewall.py"]) and l1
    for c in t["l1_cmds"]:
        l1 = run(c, shell=True) and l1
    if not l1:
        log(t, "verify", "L1 failed")
        save(led)
        print(f"\n{tid}: L1 FAILED; status stays {t['status']}")
        return 1

    l2_ok, detail = True, "n/a"
    if isinstance(t["l2"], list):
        unwritten = [s for s in t["l2"] if not (SCENARIOS / f"{s}.json").exists()]
        results = []
        for s in t["l2"]:
            if s in unwritten:
                continue
            ok = run([sys.executable, "migration/tui-parity/harness.py", "run", s])
            results.append(f"{s}={'pass' if ok else 'fail'}")
            l2_ok = l2_ok and ok
        if unwritten:
            l2_ok = False
            results.append(f"unwritten={unwritten}")
        detail = ", ".join(results)
    t["status"] = "done" if l2_ok else "l1_done"
    log(t, "verify", f"L1 pass; L2 {detail}")
    save(led)
    print(f"\n{tid}: L1 pass · L2 {'pass' if l2_ok else 'pending'} ({detail}) -> status {t['status']}")
    return 0


def cmd_check(led: dict) -> int:
    ids = [t["id"] for t in led["tasks"]]
    errs = []
    if len(ids) != len(set(ids)):
        errs.append("duplicate task ids")
    for t in led["tasks"]:
        if t["status"] not in STATUSES:
            errs.append(f"{t['id']}: bad status {t['status']}")
        for d in t["depends"]:
            if d not in ids:
                errs.append(f"{t['id']}: unknown dependency {d}")
        if t["status"] == "done" and isinstance(t["l2"], list):
            for s in t["l2"]:
                if not (SCENARIOS / f"{s}.json").exists():
                    errs.append(f"{t['id']}: done but L2 scenario {s} does not exist")
        if isinstance(t["l2"], str) and not t["l2"].startswith("n/a"):
            errs.append(f"{t['id']}: l2 must be a scenario list or 'n/a: <reason>'")
    if sum(t["status"] == "in_progress" for t in led["tasks"]) > 1:
        errs.append("more than one task in_progress")
    for e in errs:
        print(f"error: {e}", file=sys.stderr)
    print(f"ledger ok: {len(ids)} tasks" if not errs else f"{len(errs)} error(s)")
    return 1 if errs else 0


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    led = load()
    cmd = args[0]
    if cmd == "status":
        cmd_status(led)
    elif cmd == "next":
        cmd_next(led)
    elif cmd == "show":
        show(task(led, args[1]))
    elif cmd == "start":
        for t in led["tasks"]:
            if t["status"] == "in_progress" and t["id"] != args[1]:
                sys.exit(f"{t['id']} is already in_progress; finish, block or note it first")
        t = task(led, args[1])
        if not ready(led, t):
            sys.exit(f"{args[1]} has unfinished dependencies: {t['depends']}")
        t["status"] = "in_progress"
        log(t, "start", "started")
        save(led)
    elif cmd == "note":
        log(task(led, args[1]), "note", " ".join(args[2:]))
        save(led)
    elif cmd == "block":
        t = task(led, args[1])
        t["status"] = "blocked"
        log(t, "block", " ".join(args[2:]))
        save(led)
    elif cmd == "verify":
        return cmd_verify(led, args[1])
    elif cmd == "check":
        return cmd_check(led)
    else:
        sys.exit(f"unknown command {cmd}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

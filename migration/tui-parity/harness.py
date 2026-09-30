#!/usr/bin/env python3
"""Level-2 (rendered TUI) parity harness: hoocode vs cortex, side by side.

Each scenario runs the *real* interactive app of both implementations inside a
fixed-size tmux terminal, against the same scripted mock LLM
(``mockllm.py``), in identical throwaway HOME/workspace directories. The harness
sends the scripted keystrokes, waits for screen conditions, and captures the
rendered screen (plain text and styled cells) at each named snapshot. The two
apps' normalized snapshots are then compared.

Result per scenario:

* ``pass``       — every snapshot is identical after normalization (text AND style
                   unless the scenario sets ``"compare": "text"``), and every
                   assertion holds for both apps.
* ``fail``       — cortex differs from hoocode (diff written to the report).
* ``invalid``    — hoocode itself failed an assertion or a wait. Fix the
                   scenario, not cortex.

Artifacts land in ``target/tui-parity/<scenario>/`` (gitignored):
``<app>/<snapshot>.txt``, ``<app>/<snapshot>.style``, ``<app>/requests.jsonl``,
``report.md``, ``report.html`` (side-by-side rendered screens).

Usage::

    harness.py list
    harness.py run <scenario|all> [--app both|hoocode|cortex] [--keep]
    harness.py selfcheck <scenario|all>  # run hoocode twice; scenario must be deterministic
    harness.py png <scenario>            # render report.html to report.png (needs playwright)
    harness.py record <scenario|all>     # Level-1 replay fixtures from hoocode (replay.json)

Scenario format: see ``scenarios/README.md``.
"""

from __future__ import annotations

import argparse
import base64
import difflib
import html
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
SCENARIOS = HERE / "scenarios"
OUT = ROOT / "target" / "tui-parity"
NORMALIZE = HERE / "normalize.json"
APPS = ("hoocode", "cortex")
# Each app's project config dir (`{config}` in `work_files` paths).
CONFIG_DIRS = {"hoocode": ".hoocode", "cortex": ".cortexcode"}


# ---------------------------------------------------------------------------
# App launch configuration
# ---------------------------------------------------------------------------


def hoocode_cmd() -> list[str]:
    cli = Path(os.environ.get("HOOCODE_PIN_DIR", ROOT / "target" / "hoocode-pin")) / "packages/coding-agent/dist/cli.js"
    if not cli.exists():
        sys.exit(f"hoocode reference not built: {cli}\nrun migration/tui-parity/setup_hoocode.sh")
    return ["node", str(cli)]


def cortex_cmd() -> list[str]:
    if "CORTEX_BIN" in os.environ:
        return [os.environ["CORTEX_BIN"]]
    # Always build (a no-op when up to date): `ledger.py verify` runs `cargo test`
    # for the task's crates only, which does not rebuild the binary, and L2 must
    # never run against a stale one.
    subprocess.run(["cargo", "build", "-q", "-p", "cortexcode-code-main", "--bin", "cortex"], cwd=ROOT, check=True)
    return [str(ROOT / "target" / "debug" / "cortex")]


def app_cmd(app: str) -> list[str]:
    return hoocode_cmd() if app == "hoocode" else cortex_cmd()


# ---------------------------------------------------------------------------
# tmux driver
# ---------------------------------------------------------------------------


class Tmux:
    def __init__(self, name: str, cols: int, rows: int) -> None:
        self.name = name
        self.cols = cols
        self.rows = rows
        self.socket = f"cortex-parity-{os.getpid()}"

    def _run(self, *args: str, check: bool = True) -> str:
        res = subprocess.run(["tmux", "-L", self.socket, *args], capture_output=True, text=True)
        if check and res.returncode != 0:
            raise RuntimeError(f"tmux {' '.join(args)} failed: {res.stderr.strip()}")
        return res.stdout

    def start(self, argv: list[str], cwd: Path, env: dict[str, str], stdout: Path | None = None) -> None:
        env_args = ["env", "-i"] + [f"{k}={v}" for k, v in sorted(env.items())]
        # Keep the pane around after exit so a crash is still captured. The options
        # come from the server's config file so they are in effect before the app
        # starts: set afterwards, an app that exits within milliseconds (e.g.
        # --list-models) races them and loses the "Pane is dead" line, and
        # history-limit only applies to panes created after it is set.
        conf = Path(tempfile.gettempdir()) / f"{self.socket}.conf"
        conf.write_text("set-option -g remain-on-exit on\nset-option -g history-limit 10000\n")
        # The app runs under a silent `sh` that passes its exit status through: when
        # the pane's direct child is a process that exits right after writing (node
        # running hoocode --list-models), tmux 3.4 loses the race between the pty
        # EOF and the child's exit in ~1 of 8 runs and never draws the
        # "Pane is dead" line. With sh as the direct child it always does.
        wrapped = ["sh", "-c", '"$@"; exit $?', "sh", *argv]
        if stdout is not None:
            # `stdout_jsonl` scenarios: stdout goes to a file (compared line by line),
            # stderr and the exit status stay on the screen.
            wrapped = ["sh", "-c", 'out="$1"; shift; "$@" > "$out"; exit $?', "sh", str(stdout), *argv]
        self._run("-f", str(conf), "new-session", "-d", "-s", self.name, "-x", str(self.cols), "-y", str(self.rows), "-c", str(cwd), *env_args, *wrapped)

    def send_text(self, text: str) -> None:
        self._run("send-keys", "-t", self.name, "-l", text)

    def send_keys(self, keys: list[str]) -> None:
        for k in keys:
            self._run("send-keys", "-t", self.name, k)

    def capture(self, styled: bool = False, history: bool = False) -> str:
        args = ["capture-pane", "-p", "-t", self.name]
        if styled:
            args.append("-e")
        if history:
            args += ["-S", "-"]
        return self._run(*args)

    def dead(self) -> bool:
        out = self._run("display-message", "-p", "-t", self.name, "#{pane_dead}", check=False)
        return out.strip() == "1"

    def kill(self) -> None:
        self._run("kill-server", check=False)


# ---------------------------------------------------------------------------
# Screen model: a grid of (char, style) cells parsed from `capture-pane -e`
# ---------------------------------------------------------------------------

Cell = tuple[str, str]
SGR_RE = re.compile(r"\x1b\[([0-9;:]*)m")
SGR_16 = ["#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
          "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#ffffff"]


def xterm256(n: int) -> str:
    if n < 16:
        return SGR_16[n]
    if n < 232:
        n -= 16
        steps = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (steps[n // 36], steps[(n // 6) % 6], steps[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


FLAGS = {1: "bold", 2: "dim", 3: "italic", 4: "underline", 21: "underline", 5: "blink", 7: "inverse", 8: "hidden", 9: "strike"}
UNFLAGS = {22: ("bold", "dim"), 23: ("italic",), 24: ("underline",), 25: ("blink",), 27: ("inverse",), 28: ("hidden",), 29: ("strike",)}


def apply_sgr(st: dict, raw: str) -> None:
    params = [int(p) if p.isdigit() else 0 for p in re.split("[;:]", raw or "0")]
    i = 0
    while i < len(params):
        p = params[i]
        if p == 0:
            st.clear()
        elif p in FLAGS:
            st[FLAGS[p]] = True
        elif p in UNFLAGS:
            for k in UNFLAGS[p]:
                st.pop(k, None)
        elif 30 <= p <= 37:
            st["fg"] = SGR_16[p - 30]
        elif 90 <= p <= 97:
            st["fg"] = SGR_16[p - 82]
        elif 40 <= p <= 47:
            st["bg"] = SGR_16[p - 40]
        elif 100 <= p <= 107:
            st["bg"] = SGR_16[p - 92]
        elif p == 39:
            st.pop("fg", None)
        elif p == 49:
            st.pop("bg", None)
        elif p in (38, 48, 58) and i + 1 < len(params):
            key = {38: "fg", 48: "bg", 58: "ul"}[p]
            if params[i + 1] == 5 and i + 2 < len(params):
                st[key] = xterm256(params[i + 2])
                i += 2
            elif params[i + 1] == 2 and i + 4 < len(params):
                st[key] = "#%02x%02x%02x" % tuple(params[i + 2 : i + 5])
                i += 4
        i += 1


def style_key(st: dict) -> str:
    return ";".join(f"{k}={v}" if v is not True else k for k, v in sorted(st.items()))


def parse_screen(styled: str) -> list[list[Cell]]:
    st: dict = {}
    grid = []
    for raw in styled.rstrip("\n").split("\n"):
        cells: list[Cell] = []
        pos = 0
        for m in SGR_RE.finditer(raw):
            key = style_key(st)
            cells.extend((ch, key) for ch in raw[pos : m.start()])
            apply_sgr(st, m.group(1))
            pos = m.end()
        key = style_key(st)
        cells.extend((ch, key) for ch in raw[pos:])
        grid.append(cells)
    return grid


def grid_text(grid: list[list[Cell]]) -> str:
    return "\n".join("".join(ch for ch, _ in line).rstrip() for line in grid).rstrip("\n") + "\n"


def grid_styled(grid: list[list[Cell]]) -> str:
    """Canonical style serialization: one line per row, runs as «style»text."""
    rows = []
    for line in grid:
        while line and line[-1] == (" ", ""):
            line = line[:-1]
        parts, cur = [], None
        for ch, key in line:
            if key != cur:
                parts.append(f"«{key}»")
                cur = key
            parts.append(ch)
        rows.append("".join(parts))
    return "\n".join(rows).rstrip("\n") + "\n"


@dataclass
class Normalizer:
    """Line-wise regex rules applied to the cell grid, so text and style stay aligned.

    A rule's replacement cells inherit the style of the first matched cell, or the
    rule's explicit ``style`` (use it to mask a style that is itself random, such as
    a session-color badge)."""

    rules: list[tuple[re.Pattern, str, str | None]] = field(default_factory=list)

    @classmethod
    def load(cls, extra: list[dict] | None, subs: dict[str, str]) -> "Normalizer":
        spec = json.loads(NORMALIZE.read_text())["rules"] + (extra or [])
        rules = []
        for r in spec:
            pattern = r["pattern"]
            for key, value in subs.items():
                pattern = pattern.replace("{" + key + "}", re.escape(value))
            rules.append((re.compile(pattern), r["replace"], r.get("style")))
        return cls(rules)

    def apply_text(self, text: str) -> str:
        return grid_text(self.apply([[(ch, "") for ch in line] for line in text.split("\n")]))

    def apply(self, grid: list[list[Cell]]) -> list[list[Cell]]:
        out = []
        for cells in grid:
            for pattern, repl, forced in self.rules:
                text = "".join(ch for ch, _ in cells)
                new: list[Cell] = []
                last = 0
                for m in pattern.finditer(text):
                    new.extend(cells[last : m.start()])
                    style = forced if forced is not None else (cells[m.start()][1] if m.start() < len(cells) else "")
                    new.extend((ch, style) for ch in m.expand(repl))
                    last = m.end()
                if last:
                    new.extend(cells[last:])
                    cells = new
            out.append(cells)
        return out


# ---------------------------------------------------------------------------
# Scenario execution
# ---------------------------------------------------------------------------


class StepError(Exception):
    pass


def load_scenario(name: str) -> dict:
    path = SCENARIOS / f"{name}.json"
    if not path.exists():
        sys.exit(f"unknown scenario {name}")
    sc = json.loads(path.read_text())
    sc.setdefault("id", name)
    return sc


def all_scenarios() -> list[str]:
    return sorted(p.stem for p in SCENARIOS.glob("*.json"))


def write_models_json(home: Path, port: int, scenario: dict) -> None:
    models = scenario.get("models") or [{"id": "mock-model", "name": "Mock Model", "contextWindow": 128000, "maxTokens": 4096}]
    doc = {
        "providers": {
            "mock": {
                "baseUrl": f"http://127.0.0.1:{port}/v1",
                "api": "openai-completions",
                "apiKey": "mock-key",
                "models": models,
            }
        }
    }
    for d in (".hoocode", ".cortexcode"):
        (home / d).mkdir(parents=True, exist_ok=True)
        (home / d / "models.json").write_text(json.dumps(doc, indent=2))
        if "settings" in scenario:
            (home / d / "settings.json").write_text(json.dumps(scenario["settings"], indent=2))


def start_mock(script: list, workdir: Path) -> tuple[subprocess.Popen, int, Path]:
    script_path = workdir / "llm-script.json"
    script_path.write_text(json.dumps(script))
    port_file = workdir / "llm-port"
    log = workdir / "requests.jsonl"
    proc = subprocess.Popen(
        [sys.executable, str(HERE / "mockllm.py"), "--script", str(script_path), "--port-file", str(port_file), "--log", str(log)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    for _ in range(100):
        if port_file.exists() and port_file.read_text().strip():
            return proc, int(port_file.read_text()), log
        time.sleep(0.05)
    proc.kill()
    raise RuntimeError("mock LLM did not start")


def wait_for(tmux: Tmux, pattern: str, timeout: float, absent: bool = False) -> str:
    rx = re.compile(pattern, re.M)
    deadline = time.time() + timeout
    screen = ""
    while time.time() < deadline:
        screen = tmux.capture()
        found = bool(rx.search(screen))
        if found != absent:
            return screen
        if tmux.dead():
            break
        time.sleep(0.1)
    what = "disappear" if absent else "appear"
    raise StepError(f"timeout waiting for /{pattern}/ to {what}; screen was:\n{screen}")


def wait_stable(tmux: Tmux, quiet: float, timeout: float, normalizer: "Normalizer") -> None:
    """Wait until the *normalized* screen stops changing (masked animations don't count)."""

    def snap() -> str:
        return grid_styled(normalizer.apply(parse_screen(tmux.capture(styled=True))))

    deadline = time.time() + timeout
    last = snap()
    since = time.time()
    while time.time() < deadline:
        time.sleep(0.05)
        cur = snap()
        if cur != last:
            last, since = cur, time.time()
        elif time.time() - since >= quiet:
            return
    raise StepError(f"screen did not settle for {quiet}s within {timeout}s")


def write_files(sc: dict, work: Path) -> None:
    """The scenario's `files` (text) and `binary_files` (base64) into `work`."""
    for rel, content in (sc.get("files") or {}).items():
        p = work / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
    for rel, content in (sc.get("binary_files") or {}).items():
        p = work / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(base64.b64decode(content))


def run_app(app: str, sc: dict, out: Path, keep: bool) -> dict:
    """Run one scenario against one app. Returns {"ok", "error", "snapshots"}."""
    out.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix="parity-"))
    home, work = tmp / "home", tmp / "work"
    home.mkdir()
    work.mkdir()
    write_files(sc, work)
    # Links into the pinned hoocode package (read-only), e.g. so a scenario can
    # point HOOCODE_PACKAGE_DIR at a dir with its own CHANGELOG.md and still
    # have the pin's package.json and themes. `{HOOCODE_PKG}` is that package.
    pin_pkg = Path(os.environ.get("HOOCODE_PIN_DIR", ROOT / "target" / "hoocode-pin")) / "packages/coding-agent"
    for rel, target in (sc.get("symlinks") or {}).items():
        p = work / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.symlink_to(target.replace("{HOOCODE_PKG}", str(pin_pkg)))
    if sc.get("git"):
        subprocess.run(["git", "init", "-q", "-b", "main"], cwd=work, check=True)

    mock, port, log = start_mock(sc.get("llm", []), tmp)
    write_models_json(home, port, sc)

    term = sc.get("terminal", {})
    tmux = Tmux(f"{app}", int(term.get("cols", 100)), int(term.get("rows", 30)))
    env = {
        "HOME": str(home),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "TERM": term.get("term", "xterm-256color"),
        "COLORTERM": term.get("colorterm", "truecolor"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        # `{WORK}`, `{HOME}` and `{TMP}` in a value name this run's temp dirs.
        **{
            k: v.replace("{WORK}", str(work)).replace("{HOME}", str(home)).replace("{TMP}", str(tmp))
            for k, v in (sc.get("env") or {}).items()
        },
    }
    argv = app_cmd(app) + list(sc.get("args", ["--offline", "--provider", "mock", "--model", "mock-model"]))
    normalizer = Normalizer.load(sc.get("normalize"), {"HOME": str(home), "WORK": str(work), "TMP": str(tmp)})
    result: dict = {"ok": True, "error": None, "snapshots": {}}
    stdout_file = tmp / "stdout.jsonl" if sc.get("stdout_jsonl") is not None else None
    try:
        # `pre_runs`: argument lists run to completion first (same workspace,
        # home and mock LLM), e.g. to leave a session behind for --continue.
        for pre in sc.get("pre_runs") or []:
            done = subprocess.run(
                app_cmd(app) + list(pre), cwd=work, env=env, stdin=subprocess.DEVNULL,
                capture_output=True, text=True, timeout=120,
            )
            if done.returncode != 0:
                raise StepError(f"pre_run {pre} exited {done.returncode}: {done.stderr[-2000:]}")
        tmux.start(argv, work, env, stdout_file)
        for i, step in enumerate(sc["steps"]):
            try:
                run_step(tmux, step, out, normalizer, result, stdout_file)
            except StepError as e:
                raise StepError(f"step {i} {json.dumps(step)}: {e}") from None
    except (StepError, RuntimeError) as e:
        result["ok"] = False
        result["error"] = str(e)
        (out / "error.txt").write_text(str(e))
        try:
            (out / "last-screen.txt").write_text(tmux.capture(history=True))
        except RuntimeError:
            pass
    finally:
        tmux.kill()
        mock.send_signal(signal.SIGTERM)
        mock.wait(timeout=5)
        if log.exists():
            shutil.copy(log, out / "requests.jsonl")
            result["requests"] = normalize_requests(log, normalizer, sc.get("request_fields"))
            (out / "requests.normalized.json").write_text(result["requests"])
        if stdout_file is not None:
            raw = stdout_file.read_text() if stdout_file.exists() else ""
            (out / "stdout.jsonl").write_text(raw)
            result["stdout"] = normalize_jsonl(raw, normalizer, sc["stdout_jsonl"])
            (out / "stdout.normalized.jsonl").write_text(result["stdout"])
        # `work_files`: files the run leaves in the workspace, compared after
        # masking (e.g. a subagent's result.json). {"<path>": {"mask_keys": [...]}},
        # `{config}` in a path = the app's config dir.
        if sc.get("work_files"):
            result["files"] = {}
            for rel, opts in sc["work_files"].items():
                path = work / rel.format(config=CONFIG_DIRS[app])
                raw = path.read_text() if path.exists() else None
                if raw is None:
                    text = "<missing>\n"
                else:
                    text = normalize_jsonl(json.dumps(json.loads(raw)), normalizer, opts or {})
                result["files"][rel] = text
                (out / ("file-" + rel.replace("/", "_").replace("{config}", "config"))).write_text(text)
        if keep:
            (out / "tmpdir.txt").write_text(str(tmp))
        else:
            shutil.rmtree(tmp, ignore_errors=True)
    return result


DEFAULT_REQUEST_FIELDS = ["messages", "tools", "tool_choice", "model"]


def normalize_requests(log: Path, normalizer: "Normalizer", fields: list[str] | None) -> str:
    """What the app sent to the model, reduced to the fields that shape model behavior."""
    reqs = []
    for line in log.read_text().splitlines():
        body = json.loads(line)["body"]
        reqs.append({k: body.get(k) for k in (fields or DEFAULT_REQUEST_FIELDS) if k in body})
    return normalizer.apply_text(json.dumps(reqs, indent=1, sort_keys=True, ensure_ascii=False))


# Wall-clock times differ run to run in every JSON event stream; masked by key
# wherever they appear.
DEFAULT_JSONL_MASK_KEYS = ["timestamp"]


def normalize_jsonl(raw: str, normalizer: "Normalizer", opts: dict) -> str:
    """One JSON value per line, re-serialized with key order kept (JSON.stringify
    order is part of the wire format). Nondeterministic values are masked:
    scalars by key anywhere (`mask_keys`, default timestamp), and whole fields of
    one event type (`mask_fields`: {"<type>": ["a.b", ...]}, dotted paths). Then
    the text normalizer is applied."""
    mask = set(opts.get("mask_keys", DEFAULT_JSONL_MASK_KEYS))
    fields = opts.get("mask_fields", {})

    def walk(v):
        if isinstance(v, dict):
            return {k: ("<masked>" if k in mask and not isinstance(x, (dict, list)) else walk(x)) for k, x in v.items()}
        if isinstance(v, list):
            return [walk(x) for x in v]
        return v

    def mask_path(v, path: list[str]) -> None:
        for key in path[:-1]:
            v = v.get(key) if isinstance(v, dict) else None
        if isinstance(v, dict) and path[-1] in v:
            v[path[-1]] = "<masked>"

    lines = []
    for line in raw.splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            lines.append(f"<not json> {line}")
            continue
        if isinstance(value, dict):
            for path in fields.get(value.get("type"), []):
                mask_path(value, path.split("."))
        lines.append(json.dumps(walk(value), ensure_ascii=False, separators=(",", ":")))
    return normalizer.apply_text("\n".join(lines) + ("\n" if lines else ""))


def run_step(tmux: Tmux, step: dict, out: Path, normalizer: Normalizer, result: dict, stdout_file: Path | None = None) -> None:
    timeout = float(step.get("timeout", 15))
    if "wait_stdout" in step:
        # `stdout_jsonl` scenarios: wait until the captured stdout matches.
        if stdout_file is None:
            raise StepError("wait_stdout needs stdout_jsonl")
        pattern = re.compile(step["wait_stdout"], re.M)
        deadline = time.time() + timeout
        while not (stdout_file.exists() and pattern.search(stdout_file.read_text(errors="replace"))):
            if time.time() > deadline:
                raise StepError(f"stdout did not match {step['wait_stdout']!r} within {timeout}s")
            time.sleep(0.05)
    elif "type" in step:
        tmux.send_text(step["type"])
    elif "keys" in step:
        tmux.send_keys(step["keys"] if isinstance(step["keys"], list) else [step["keys"]])
    elif "wait_for" in step:
        wait_for(tmux, step["wait_for"], timeout)
    elif "wait_gone" in step:
        wait_for(tmux, step["wait_gone"], timeout, absent=True)
    elif "wait_stable" in step:
        wait_stable(tmux, float(step["wait_stable"]), timeout, normalizer)
    elif "wait_exit" in step:
        deadline = time.time() + timeout
        while not tmux.dead():
            if time.time() > deadline:
                raise StepError(f"app did not exit within {timeout}s")
            time.sleep(0.1)
        # tmux flips #{pane_dead} before it draws the "Pane is dead (status N, ...)"
        # line, so a snapshot taken right away can miss the exit status. Wait for it.
        marker_deadline = time.time() + 5
        while "Pane is dead" not in tmux.capture(history=True) and time.time() < marker_deadline:
            time.sleep(0.05)
    elif "sleep" in step:
        time.sleep(float(step["sleep"]))
    elif "snapshot" in step:
        name = step["snapshot"]
        history = bool(step.get("history", False))
        grid = normalizer.apply(parse_screen(tmux.capture(styled=True, history=history)))
        plain, styled = grid_text(grid), grid_styled(grid)
        (out / f"{name}.txt").write_text(plain)
        (out / f"{name}.style").write_text(styled)
        result["snapshots"][name] = {"text": plain, "style": styled, "grid": grid}
        for needle in step.get("contains", []):
            if needle not in plain:
                raise StepError(f"snapshot {name}: expected to contain {needle!r}")
        for needle in step.get("not_contains", []):
            if needle in plain:
                raise StepError(f"snapshot {name}: expected NOT to contain {needle!r}")
    else:
        raise StepError(f"unknown step {step}")


# ---------------------------------------------------------------------------
# Level-1 fixture recording (13.2): headless replay of non-interactive scenarios
# ---------------------------------------------------------------------------
#
# `record` runs hoocode for each scenario in `replay.json` WITHOUT tmux: stdin is a
# pipe (or /dev/null), stdout/stderr go to files. The Rust integration test
# `crates/cortexcode-code-main/tests/replay.rs` runs `cortex` the same way against
# a port of `mockllm.py` and must render the same text. The recording keeps
# hoocode's raw output plus the rendered (normalized) text as an insta snapshot;
# the Rust test re-normalizes the raw output to check its normalizer agrees with
# this one.

REPLAY = HERE / "replay.json"
FIXTURES = ROOT / "crates" / "cortexcode-code-main" / "tests" / "fixtures" / "hoocode-0.5.89" / "replay"
SNAPSHOTS = ROOT / "crates" / "cortexcode-code-main" / "tests" / "snapshots"
REPLAY_STEPS = {"type", "keys", "wait_stdout", "wait_exit", "snapshot", "sleep"}
REPLAY_KEYS = {"Enter": "\n"}


def replay_manifest() -> dict:
    return json.loads(REPLAY.read_text())


def pin_commit() -> str:
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["metadata"]["cortex"]["source"]["hoocode-commit"]


def run_headless(app: str, sc: dict, keep: bool = False) -> dict:
    """Run a scenario's replayable steps with pipes instead of a terminal.

    Returns the raw artifacts: exit status, stdout, stderr, request log lines,
    session files, work files, and the run's temp paths (for normalization)."""
    tmp = Path(tempfile.mkdtemp(prefix="replay-"))
    home, work = tmp / "home", tmp / "work"
    home.mkdir()
    work.mkdir()
    write_files(sc, work)
    if sc.get("symlinks") or sc.get("git"):
        raise StepError("symlinks/git scenarios are not replayable")
    mock, port, log = start_mock(sc.get("llm", []), tmp)
    write_models_json(home, port, sc)
    env = {
        "HOME": str(home),
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        **{
            k: v.replace("{WORK}", str(work)).replace("{HOME}", str(home)).replace("{TMP}", str(tmp))
            for k, v in (sc.get("env") or {}).items()
        },
    }
    steps = sc["steps"]
    for step in steps:
        kind = next(k for k in step if k not in ("timeout", "contains", "not_contains", "history"))
        if kind not in REPLAY_STEPS:
            raise StepError(f"step {kind!r} is not replayable")
    interactive = any("type" in s or "keys" in s for s in steps)
    out_path, err_path = tmp / "stdout", tmp / "stderr"
    try:
        for pre in sc.get("pre_runs") or []:
            done = subprocess.run(app_cmd(app) + list(pre), cwd=work, env=env, stdin=subprocess.DEVNULL,
                                  capture_output=True, text=True, timeout=120)
            if done.returncode != 0:
                raise StepError(f"pre_run {pre} exited {done.returncode}: {done.stderr[-2000:]}")
        argv = app_cmd(app) + list(sc.get("args", ["--offline", "--provider", "mock", "--model", "mock-model"]))
        with open(out_path, "wb") as out_f, open(err_path, "wb") as err_f:
            proc = subprocess.Popen(argv, cwd=work, env=env, stdout=out_f, stderr=err_f,
                                    stdin=subprocess.PIPE if interactive else subprocess.DEVNULL)
            try:
                for i, step in enumerate(steps):
                    timeout = float(step.get("timeout", 15))
                    if "type" in step:
                        proc.stdin.write(step["type"].encode())
                        proc.stdin.flush()
                    elif "keys" in step:
                        for key in step["keys"] if isinstance(step["keys"], list) else [step["keys"]]:
                            if key == "C-d":
                                proc.stdin.close()
                            elif key in REPLAY_KEYS:
                                proc.stdin.write(REPLAY_KEYS[key].encode())
                                proc.stdin.flush()
                            else:
                                raise StepError(f"key {key!r} is not replayable")
                    elif "wait_stdout" in step:
                        pattern = re.compile(step["wait_stdout"], re.M)
                        deadline = time.time() + timeout
                        while not pattern.search(out_path.read_text(errors="replace")):
                            if time.time() > deadline or proc.poll() is not None and not pattern.search(out_path.read_text(errors="replace")):
                                raise StepError(f"step {i}: stdout did not match {step['wait_stdout']!r}")
                            time.sleep(0.05)
                    elif "wait_exit" in step:
                        proc.wait(timeout=timeout)
                    elif "sleep" in step:
                        time.sleep(float(step["sleep"]))
                    # `snapshot` is a screen capture: Level 2 only.
                status = proc.wait(timeout=30)
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()
    finally:
        mock.send_signal(signal.SIGTERM)
        mock.wait(timeout=5)
    config = CONFIG_DIRS[app]
    sessions = [p.read_text() for p in sorted((home / config / "sessions").rglob("*.jsonl"))] if (home / config / "sessions").exists() else []
    files = {}
    for rel in sc.get("work_files") or {}:
        path = work / rel.format(config=config)
        files[rel] = path.read_text() if path.exists() else None
    result = {
        "paths": {"HOME": str(home), "WORK": str(work), "TMP": str(tmp)},
        "exit_status": status,
        "stdout": out_path.read_text(),
        "stderr": err_path.read_text(),
        "requests": log.read_text().splitlines() if log.exists() else [],
        "sessions": sessions,
        "files": files,
    }
    if keep:
        print(f"kept {tmp}")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    return result


def normalize_session(raw: str, normalizer: "Normalizer", opts: dict) -> str:
    """A session JSONL file: random entry ids become `<id-N>` by first appearance
    (so the tree shape is still compared), then the usual JSON-lines masking."""
    ids: dict[str, str] = {}
    remap = opts.get("remap_keys", [])
    lines = []
    for line in raw.splitlines():
        value = json.loads(line)
        for key in remap:
            if isinstance(value.get(key), str):
                value[key] = ids.setdefault(value[key], f"<id-{len(ids) + 1}>")
        lines.append(json.dumps(value, ensure_ascii=False, separators=(",", ":")))
    return normalize_jsonl("\n".join(lines), normalizer, opts)


def render_replay(sc: dict, raw: dict, manifest: dict) -> str:
    """The text both apps must agree on: exit status, stdout, stderr, the model
    requests (when the scenario compares them), session files and work files."""
    normalizer = Normalizer.load(sc.get("normalize"), raw["paths"])
    parts = [f"## exit\n{raw['exit_status']}\n"]
    if sc.get("stdout_jsonl") is not None:
        parts.append("## stdout (jsonl)\n" + normalize_jsonl(raw["stdout"], normalizer, sc["stdout_jsonl"]))
    else:
        parts.append("## stdout\n" + normalizer.apply_text(raw["stdout"]))
    parts.append("## stderr\n" + normalizer.apply_text(raw["stderr"]))
    if sc.get("compare_requests"):
        reqs = []
        for line in raw["requests"]:
            body = json.loads(line)["body"]
            reqs.append({k: body.get(k) for k in (sc.get("request_fields") or DEFAULT_REQUEST_FIELDS) if k in body})
        parts.append("## requests\n" + normalizer.apply_text(json.dumps(reqs, indent=1, sort_keys=True, ensure_ascii=False)))
    sessions = sorted(normalize_session(s, normalizer, manifest["session"]) for s in raw["sessions"])
    for i, s in enumerate(sessions):
        parts.append(f"## session {i + 1}/{len(sessions)}\n{s}")
    for rel, text in raw["files"].items():
        body = "<missing>\n" if text is None else normalize_jsonl(json.dumps(json.loads(text)), normalizer, sc["work_files"][rel] or {})
        parts.append(f"## file {rel}\n{body}")
    return "\n".join(parts)


def snap_file(name: str) -> Path:
    return SNAPSHOTS / f"replay__{name}.snap"


def cmd_record(names: list[str]) -> int:
    """Record hoocode fixtures for the Level-1 replay test (never run with cortex)."""
    manifest = replay_manifest()
    commit = pin_commit()
    for name in names:
        sc = load_scenario(name)
        raw = run_headless("hoocode", sc)
        rendered = render_replay(sc, raw, manifest)
        again = render_replay(sc, run_headless("hoocode", sc), manifest)
        if again != rendered:
            diff = difflib.unified_diff(rendered.splitlines(), again.splitlines(), "run1", "run2", lineterm="")
            print(f"unstable {name}: two hoocode runs render differently\n" + "\n".join(list(diff)[:60]))
            return 1
        d = FIXTURES / name
        shutil.rmtree(d, ignore_errors=True)
        (d / "sessions").mkdir(parents=True)
        (d / "meta.json").write_text(json.dumps({
            "scenario": name,
            "hoocode_commit": commit,
            "paths": raw["paths"],
            "exit_status": raw["exit_status"],
            "files": {rel: (None if t is None else f"file-{i}") for i, (rel, t) in enumerate(raw["files"].items())},
        }, indent=2) + "\n")
        (d / "stdout").write_text(raw["stdout"])
        (d / "stderr").write_text(raw["stderr"])
        (d / "requests.jsonl").write_text("".join(l + "\n" for l in raw["requests"]))
        for i, s in enumerate(raw["sessions"]):
            (d / "sessions" / f"{i}.jsonl").write_text(s)
        for i, (rel, t) in enumerate(raw["files"].items()):
            if t is not None:
                (d / f"file-{i}").write_text(t)
        SNAPSHOTS.mkdir(parents=True, exist_ok=True)
        header = (
            "---\nsource: crates/cortexcode-code-main/tests/replay.rs\n"
            f"description: \"recorded from hoocode {commit[:8]} by harness.py record; never accept cortex output here\"\n"
            f"expression: {name}\n---\n"
        )
        snap_file(name).write_text(header + rendered)
        print(f"recorded {name}")
    return 0


# ---------------------------------------------------------------------------
# Comparison + reports
# ---------------------------------------------------------------------------


def compare(sc: dict, results: dict[str, dict], out: Path) -> str:
    hoo, cor = results.get("hoocode"), results.get("cortex")
    lines = [f"# TUI parity: {sc['id']}", "", sc.get("description", ""), ""]
    if hoo is None or cor is None:
        status = "partial"
    elif not hoo["ok"]:
        status = "invalid"
        lines += ["**invalid**: hoocode failed the scenario:", "```", hoo["error"], "```"]
    else:
        status = "pass"
        mode = sc.get("compare", "style")
        if not cor["ok"]:
            status = "fail"
            lines += ["**cortex failed a step:**", "```", cor["error"], "```"]
        for name, h in hoo["snapshots"].items():
            c = cor["snapshots"].get(name)
            if c is None:
                status = "fail"
                lines.append(f"- `{name}`: missing in cortex")
                continue
            text_ok = h["text"] == c["text"]
            style_ok = h["style"] == c["style"]
            ok = text_ok and (style_ok or mode == "text")
            lines.append(f"- `{name}`: text {'✓' if text_ok else '✗'} · style {'✓' if style_ok else '✗'}{' (not required)' if mode == 'text' else ''}")
            if not ok:
                status = "fail"
                a, b = (h["text"], c["text"]) if not text_ok else (h["style"], c["style"])
                diff = difflib.unified_diff(a.splitlines(), b.splitlines(), "hoocode", "cortex", lineterm="")
                lines += ["", "```diff", *list(diff)[:200], "```", ""]
    if hoo and cor and hoo["ok"] and sc.get("compare_requests"):
        h, c = hoo.get("requests", ""), cor.get("requests", "")
        lines.append(f"- `requests` (what the model saw): {'✓' if h == c else '✗'}")
        if h != c:
            status = "fail"
            diff = difflib.unified_diff(h.splitlines(), c.splitlines(), "hoocode", "cortex", lineterm="")
            lines += ["", "```diff", *list(diff)[:300], "```", ""]
    if hoo and cor and hoo["ok"] and sc.get("stdout_jsonl") is not None:
        h, c = hoo.get("stdout", ""), cor.get("stdout", "")
        lines.append(f"- `stdout` (JSON lines): {'✓' if h == c else '✗'}")
        if h != c:
            status = "fail"
            # One key per line so a diff points at the field, not a 2 KB line.
            def explode(t: str) -> list[str]:
                out = []
                for i, l in enumerate(t.splitlines()):
                    try:
                        out += [f"[{i}] {x}" for x in json.dumps(json.loads(l), indent=1, ensure_ascii=False).splitlines()]
                    except json.JSONDecodeError:
                        out.append(f"[{i}] {l}")
                return out
            diff = difflib.unified_diff(explode(h), explode(c), "hoocode", "cortex", lineterm="")
            lines += ["", "```diff", *list(diff)[:400], "```", ""]
    if hoo and cor and hoo["ok"] and sc.get("work_files"):
        for rel in sc["work_files"]:
            h, c = hoo.get("files", {}).get(rel, ""), cor.get("files", {}).get(rel, "")
            lines.append(f"- `{rel}`: {'✓' if h == c else '✗'}")
            if h != c:
                status = "fail"
                diff = difflib.unified_diff(
                    json.dumps(json.loads(h), indent=1).splitlines() if h.startswith("{") else [h],
                    json.dumps(json.loads(c), indent=1).splitlines() if c.startswith("{") else [c],
                    "hoocode", "cortex", lineterm="",
                )
                lines += ["", "```diff", *list(diff)[:200], "```", ""]
    lines.insert(1, f"\n**Result: {status}**\n")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    write_html(sc, results, out, status)
    return status


def grid_to_html(grid: list[list[Cell]]) -> str:
    rows = []
    for line in grid:
        parts, cur, buf = [], None, []

        def flush() -> None:
            if not buf:
                return
            st = dict(kv.split("=", 1) if "=" in kv else (kv, True) for kv in cur.split(";") if kv) if cur else {}
            fg, bg = st.get("fg"), st.get("bg")
            if st.get("inverse"):
                fg, bg = bg or "#1e1e1e", fg or "#d4d4d4"
            css = [f"color:{fg}"] if fg else []
            css += [f"background:{bg}"] if bg else []
            css += ["font-weight:bold"] if st.get("bold") else []
            css += ["opacity:.6"] if st.get("dim") else []
            css += ["font-style:italic"] if st.get("italic") else []
            css += ["text-decoration:underline"] if st.get("underline") else []
            css += ["text-decoration:line-through"] if st.get("strike") else []
            text = html.escape("".join(buf))
            parts.append(f'<span style="{";".join(css)}">{text}</span>' if css else text)
            buf.clear()

        for ch, key in line:
            if key != cur:
                flush()
                cur = key
            buf.append(ch)
        flush()
        rows.append("".join(parts))
    return "\n".join(rows)


def write_html(sc: dict, results: dict[str, dict], out: Path, status: str) -> None:
    names: list[str] = []
    for r in results.values():
        for n in r["snapshots"]:
            if n not in names:
                names.append(n)
    rows = []
    for n in names:
        cells = []
        for app in APPS:
            snap = results.get(app, {}).get("snapshots", {}).get(n)
            body = grid_to_html(snap["grid"]) if snap else "<em>missing</em>"
            cells.append(f"<td><div class=app>{app}</div><pre class=term>{body}</pre></td>")
        rows.append(f"<tr><th colspan=2>{html.escape(n)}</th></tr><tr>{''.join(cells)}</tr>")
    errors = "".join(
        f"<p class=err><b>{app}</b>: {html.escape(r['error'])}</p>" for app, r in results.items() if r.get("error")
    )
    doc = f"""<!doctype html><html><head><meta charset=utf-8><title>TUI parity {html.escape(sc['id'])}</title>
<style>
body{{background:#111;color:#ddd;font-family:system-ui,sans-serif;margin:16px}}
table{{border-collapse:collapse}} td{{vertical-align:top;padding:6px}}
th{{text-align:left;padding-top:18px;color:#9cf}}
pre.term{{background:#1e1e1e;color:#d4d4d4;font:13px/1.25 'DejaVu Sans Mono',Menlo,monospace;padding:8px;margin:0;border:1px solid #333}}
.app{{font-size:12px;color:#888;margin-bottom:4px}} .err{{color:#f88;white-space:pre-wrap}}
.status{{font-size:20px}}
</style></head><body>
<div class=status>{html.escape(sc['id'])}: <b>{status}</b></div><p>{html.escape(sc.get('description', ''))}</p>{errors}
<table>{''.join(rows)}</table></body></html>"""
    (out / "report.html").write_text(doc)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def cmd_run(names: list[str], apps: list[str], keep: bool) -> int:
    summary = {}
    for name in names:
        sc = load_scenario(name)
        out = OUT / name
        shutil.rmtree(out, ignore_errors=True)
        results = {}
        for app in apps:
            results[app] = run_app(app, sc, out / app, keep)
        status = compare(sc, results, out)
        summary[name] = status
        print(f"{status:8} {name}   ({out / 'report.md'})")
    (OUT / "summary.json").write_text(json.dumps(summary, indent=2))
    return 0 if all(s == "pass" for s in summary.values()) else 1


def cmd_selfcheck(names: list[str]) -> int:
    """A scenario is only trustworthy if hoocode renders it identically twice."""
    bad = 0
    for name in names:
        sc = load_scenario(name)
        runs = [run_app("hoocode", sc, OUT / name / f"selfcheck-{i}", keep=False) for i in (1, 2)]
        if not all(r["ok"] for r in runs):
            print(f"invalid  {name}: {next(r['error'] for r in runs if not r['ok'])[:300]}")
            bad += 1
            continue
        diffs = [n for n, s in runs[0]["snapshots"].items() if runs[1]["snapshots"].get(n, {}).get("style") != s["style"]]
        if runs[0].get("stdout") != runs[1].get("stdout"):
            diffs.append("stdout")
        if runs[0].get("files") != runs[1].get("files"):
            diffs.append("work_files")
        if diffs:
            bad += 1
            print(f"unstable {name}: snapshots {diffs} differ between two hoocode runs (see {OUT / name}/selfcheck-*)")
        else:
            print(f"stable   {name}")
    return 1 if bad else 0


def cmd_png(name: str) -> int:
    report = OUT / name / "report.html"
    script = HERE / "render_png.mjs"
    groot = subprocess.run(["npm", "root", "-g"], capture_output=True, text=True).stdout.strip()
    env = {**os.environ, "NODE_PATH": groot}
    return subprocess.run(["node", str(script), str(report), str(report.with_suffix(".png"))], env=env).returncode


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")
    r = sub.add_parser("run")
    r.add_argument("scenario")
    r.add_argument("--app", choices=["both", *APPS], default="both")
    r.add_argument("--keep", action="store_true", help="keep the temp HOME/workspace for debugging")
    sc = sub.add_parser("selfcheck")
    sc.add_argument("scenario")
    rec = sub.add_parser("record", help="record hoocode fixtures for the Level-1 replay test (replay.json)")
    rec.add_argument("scenario")
    p = sub.add_parser("png")
    p.add_argument("scenario")
    args = ap.parse_args()

    if shutil.which("tmux") is None:
        sys.exit("tmux is required")
    if args.cmd == "list":
        for n in all_scenarios():
            print(f"{n:32} {load_scenario(n).get('description', '')}")
        return 0
    if args.cmd == "png":
        return cmd_png(args.scenario)
    if args.cmd == "record":
        return cmd_record(replay_manifest()["scenarios"] if args.scenario == "all" else [args.scenario])
    names = all_scenarios() if args.scenario == "all" else [args.scenario]
    if args.cmd == "selfcheck":
        return cmd_selfcheck(names)
    apps = list(APPS) if args.app == "both" else [args.app]
    return cmd_run(names, apps, args.keep)


if __name__ == "__main__":
    sys.exit(main())

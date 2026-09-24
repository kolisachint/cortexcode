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

Scenario format: see ``scenarios/README.md``.
"""

from __future__ import annotations

import argparse
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


# ---------------------------------------------------------------------------
# App launch configuration
# ---------------------------------------------------------------------------


def hoocode_cmd() -> list[str]:
    cli = Path(os.environ.get("HOOCODE_PIN_DIR", ROOT / "target" / "hoocode-pin")) / "packages/coding-agent/dist/cli.js"
    if not cli.exists():
        sys.exit(f"hoocode reference not built: {cli}\nrun migration/tui-parity/setup_hoocode.sh")
    return ["node", str(cli)]


def cortex_cmd() -> list[str]:
    exe = Path(os.environ.get("CORTEX_BIN", ROOT / "target" / "debug" / "cortex"))
    if not exe.exists():
        subprocess.run(["cargo", "build", "-q", "-p", "cortexcode-code-main", "--bin", "cortex"], cwd=ROOT, check=True)
    return [str(exe)]


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

    def start(self, argv: list[str], cwd: Path, env: dict[str, str]) -> None:
        env_args = ["env", "-i"] + [f"{k}={v}" for k, v in sorted(env.items())]
        # Keep the pane around after exit so a crash is still captured.
        self._run("-f", "/dev/null", "new-session", "-d", "-s", self.name, "-x", str(self.cols), "-y", str(self.rows), "-c", str(cwd), *env_args, *argv)
        self._run("set-option", "-t", self.name, "remain-on-exit", "on")
        self._run("set-option", "-t", self.name, "history-limit", "10000")

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


def wait_stable(tmux: Tmux, quiet: float, timeout: float) -> None:
    deadline = time.time() + timeout
    last = tmux.capture(styled=True)
    since = time.time()
    while time.time() < deadline:
        time.sleep(0.05)
        cur = tmux.capture(styled=True)
        if cur != last:
            last, since = cur, time.time()
        elif time.time() - since >= quiet:
            return
    raise StepError(f"screen did not settle for {quiet}s within {timeout}s")


def run_app(app: str, sc: dict, out: Path, keep: bool) -> dict:
    """Run one scenario against one app. Returns {"ok", "error", "snapshots"}."""
    out.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix="parity-"))
    home, work = tmp / "home", tmp / "work"
    home.mkdir()
    work.mkdir()
    for rel, content in (sc.get("files") or {}).items():
        p = work / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)
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
        **(sc.get("env") or {}),
    }
    argv = app_cmd(app) + list(sc.get("args", ["--offline", "--provider", "mock", "--model", "mock-model"]))
    normalizer = Normalizer.load(sc.get("normalize"), {"HOME": str(home), "WORK": str(work), "TMP": str(tmp)})
    result: dict = {"ok": True, "error": None, "snapshots": {}}
    try:
        tmux.start(argv, work, env)
        for i, step in enumerate(sc["steps"]):
            try:
                run_step(tmux, step, out, normalizer, result)
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


def run_step(tmux: Tmux, step: dict, out: Path, normalizer: Normalizer, result: dict) -> None:
    timeout = float(step.get("timeout", 15))
    if "type" in step:
        tmux.send_text(step["type"])
    elif "keys" in step:
        tmux.send_keys(step["keys"] if isinstance(step["keys"], list) else [step["keys"]])
    elif "wait_for" in step:
        wait_for(tmux, step["wait_for"], timeout)
    elif "wait_gone" in step:
        wait_for(tmux, step["wait_gone"], timeout, absent=True)
    elif "wait_stable" in step:
        wait_stable(tmux, float(step["wait_stable"]), timeout)
    elif "wait_exit" in step:
        deadline = time.time() + timeout
        while not tmux.dead():
            if time.time() > deadline:
                raise StepError(f"app did not exit within {timeout}s")
            time.sleep(0.1)
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
    names = all_scenarios() if args.scenario == "all" else [args.scenario]
    if args.cmd == "selfcheck":
        return cmd_selfcheck(names)
    apps = list(APPS) if args.app == "both" else [args.app]
    return cmd_run(names, apps, args.keep)


if __name__ == "__main__":
    sys.exit(main())

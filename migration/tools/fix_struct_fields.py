#!/usr/bin/env python3
"""Compiler-driven mechanical fixer for struct-shape migrations.

Runs ``cargo check --message-format=json`` and rewrites source at the exact spans
rustc reports, repeating until no fixable errors remain:

* E0063 missing field(s) in a struct literal  → insert ``field: <default>,``
* E0560 struct has no field named X (X in REMOVED) → delete ``X: <expr>,``
* E0308 mismatched types where the literal is ``None`` or ``Some(expr)`` and the
  expected type is no longer an Option → replace with a default / unwrap ``expr``.

Defaults come from the tables below; ``{now}`` expands to ``cortexcode_ai_types::now_ms()``
outside ``#[cfg(test)]`` modules and to ``0`` inside them.

Usage: fix_struct_fields.py [-p crate ...]
Never run blindly on unrelated errors: it only touches the error codes above,
and only for the fields/types listed here. Review the diff afterwards.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# (struct, field) -> default expression; ("*", field) applies to any struct.
FIELD_DEFAULTS = {
    ("TextContent", "text_signature"): "None",
    ("ThinkingContent", "redacted"): "false",
    ("ToolCallContent", "thought_signature"): "None",
    ("ToolResultMessage", "details"): "None",
    ("AssistantMessage", "api"): "String::new()",
    ("AssistantMessage", "provider"): "String::new()",
    ("AssistantMessage", "model"): "String::new()",
    ("AssistantMessage", "response_model"): "None",
    ("AssistantMessage", "response_id"): "None",
    ("AssistantMessage", "diagnostics"): "None",
    ("Header", "branch"): "None",
    ("Model", "compat"): "None",
    ("SessionInfo", "color"): "None",
    ("*", "cache_retention"): "None",
    ("AgentTool", "plain_json_schema"): "false",
    ("Tool", "defer_loading"): "None",
}
REMOVED = {"stop_sequence", "cache_control", "cache_control_format", "supports_long_cache_retention"}
# expected type (as rustc prints it) -> replacement for a bare `None`
NONE_REPLACEMENTS = {
    "i64": "{now}",
    "Usage": "Default::default()",
    "cortexcode_ai_types::Usage": "Default::default()",
    "StopReason": "cortexcode_ai_types::StopReason::Stop",
    "cortexcode_ai_types::StopReason": "cortexcode_ai_types::StopReason::Stop",
}


def diagnostics(pkgs: list[str]) -> list[dict]:
    cmd = ["cargo", "check", "--message-format=json", "--all-targets", *pkgs]
    res = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    out = []
    for line in res.stdout.splitlines():
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        if msg.get("reason") == "compiler-message" and msg["message"]["level"] == "error":
            out.append(msg["message"])
    return out


def in_test_module(text: str, offset: int) -> bool:
    idx = text.rfind("#[cfg(test)]", 0, offset)
    return idx != -1


def find_expr_end(text: str, start: int) -> int:
    """Index just past the expression starting at `start` (stops at `,` or `}` at depth 0)."""
    depth = 0
    i = start
    in_str = False
    while i < len(text):
        c = text[i]
        if in_str:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                in_str = False
        elif c == '"':
            in_str = True
        elif c in "([{":
            depth += 1
        elif c in ")]}":
            if depth == 0:
                return i
            depth -= 1
        elif c == "," and depth == 0:
            return i
        i += 1
    return i


def plan_edits(msgs: list[dict]) -> dict[Path, list[tuple[int, int, str]]]:
    edits: dict[Path, list[tuple[int, int, str]]] = {}
    for m in msgs:
        code = (m.get("code") or {}).get("code")
        spans = [s for s in m["spans"] if s["is_primary"]]
        if not spans:
            continue
        sp = spans[0]
        path = ROOT / sp["file_name"]
        # rustc spans are byte offsets: work on bytes decoded 1:1 (latin-1 keeps offsets).
        text = path.read_bytes().decode("latin-1")
        lo, hi = sp["byte_start"], sp["byte_end"]
        if code == "E0063":
            mm = re.search(r"initializer of `(?:[\w:]+::)?(\w+)(?:<.*>)?`", m["message"])
            struct = mm.group(1) if mm else ""
            fields = re.findall(r"`(\w+)`", m["message"].split(" in initializer")[0])
            if "other field" in m["message"]:
                # rustc truncates the list; the children notes don't list them either,
                # so fix what we can see and let the next round report the rest.
                pass
            inserts = []
            for f in fields:
                d = FIELD_DEFAULTS.get((struct, f), FIELD_DEFAULTS.get(("*", f)))
                if d is None:
                    continue
                inserts.append(f"{f}: {d}")
            if not inserts:
                continue
            brace = text.index("{", hi)
            edits.setdefault(path, []).append((brace + 1, brace + 1, " " + ", ".join(inserts) + ","))
        elif code == "E0560":
            mm = re.search(r"has no field named `(\w+)`", m["message"])
            if not mm or mm.group(1) not in REMOVED:
                continue
            colon = text.index(":", hi)
            end = find_expr_end(text, colon + 1)
            if end < len(text) and text[end] == ",":
                end += 1
            edits.setdefault(path, []).append((lo, end, ""))
        elif code == "E0308":
            snippet = text[lo:hi]
            mm = re.search(r"expected `([^`]+)`, found `([^`]+)`", m["message"] + " " + " ".join(c["message"] for c in m.get("children", [])))
            label = sp.get("label") or ""
            ml = re.search(r"expected `([^`]+)`, found `([^`]+)`", label)
            exp = (ml or mm).group(1) if (ml or mm) else ""
            found = (ml or mm).group(2) if (ml or mm) else ""
            if snippet == "None" and exp in NONE_REPLACEMENTS:
                rep = NONE_REPLACEMENTS[exp]
                rep = rep.replace("{now}", "0" if in_test_module(text, lo) else "cortexcode_ai_types::now_ms()")
                edits.setdefault(path, []).append((lo, hi, rep))
            elif snippet.startswith("Some(") and snippet.endswith(")") and found.startswith("Option<"):
                inner = snippet[5:-1].strip()
                if inner.endswith(","):
                    inner = inner[:-1].rstrip()
                edits.setdefault(path, []).append((lo, hi, inner))
    return edits


def apply(edits: dict[Path, list[tuple[int, int, str]]], broken: set[Path]) -> int:
    n = 0
    for path, es in edits.items():
        if path in broken:
            continue
        data = path.read_bytes()
        # dedupe identical edits, apply from the end
        for lo, hi, rep in sorted(set(es), key=lambda e: (e[0], e[1]), reverse=True):
            data = data[:lo] + rep.encode("latin-1") + data[hi:]
            n += 1
        path.write_bytes(data)
    return n


def main() -> int:
    pkgs: list[str] = []
    args = sys.argv[1:]
    while args:
        if args[0] == "-p":
            pkgs += ["-p", args[1]]
            args = args[2:]
        else:
            args = args[1:]
    total = 0
    for round_ in range(20):
        msgs = diagnostics(pkgs)
        # Files with syntax errors: rustc's other diagnostics there are unreliable.
        broken = {ROOT / s["file_name"] for m in msgs if not m.get("code") for s in m["spans"] if s["is_primary"]}
        if broken:
            print(f"syntax errors in {sorted(str(b) for b in broken)}; not editing them")
        edits = plan_edits(msgs)
        n = apply(edits, broken)
        total += n
        print(f"round {round_}: {len(msgs)} errors, applied {n} edits")
        if n == 0:
            break
    left = diagnostics(pkgs)
    for m in left[:40]:
        sp = next((s for s in m["spans"] if s["is_primary"]), None)
        loc = f"{sp['file_name']}:{sp['line_start']}" if sp else "?"
        print(f"  remaining: {loc}: {m['message']}")
    print(f"total edits {total}; {len(left)} errors remain")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Lockstep version bump for the cortexcode workspace.

Bumps the shared version in the workspace root Cargo.toml. Member crates inherit
`version.workspace = true`, so no per-crate edits are needed.

Usage: python3 scripts/bump_versions.py <patch|minor|major>
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKSPACE_TOML = REPO_ROOT / "Cargo.toml"


def bump(version: str, kind: str) -> str:
    m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:-.+)?", version)
    if not m:
        raise ValueError(f"Cannot parse version: {version}")
    major, minor, patch = (int(g) for g in m.groups())
    if kind == "major":
        return f"{major + 1}.0.0"
    if kind == "minor":
        return f"{major}.{minor + 1}.0"
    return f"{major}.{minor}.{patch + 1}"


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("patch", "minor", "major"):
        print("Usage: python3 scripts/bump_versions.py <patch|minor|major>", file=sys.stderr)
        return 1

    kind = sys.argv[1]
    text = WORKSPACE_TOML.read_text()

    # Find the workspace version.
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, flags=re.M)
    if not m:
        print("Could not find workspace version", file=sys.stderr)
        return 1

    old = m.group(1)
    new = bump(old, kind)

    # Replace only the first occurrence, which is the workspace package version.
    new_text, n = re.subn(
        rf'^version\s*=\s*"{re.escape(old)}"',
        f'version = "{new}"',
        text,
        count=1,
        flags=re.M,
    )
    if n != 1:
        print(f"Could not bump version from {old}", file=sys.stderr)
        return 1

    WORKSPACE_TOML.write_text(new_text)
    print(f"Bumped workspace version: {old} -> {new}")
    print(new)
    return 0


if __name__ == "__main__":
    sys.exit(main())

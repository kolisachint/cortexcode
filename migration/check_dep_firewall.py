#!/usr/bin/env python3
"""Enforce migration/dep-firewall.json: volatile deps live only in their adapter crates."""

import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    cfg = json.loads((ROOT / "migration" / "dep-firewall.json").read_text())
    owners: dict[str, list[str]] = cfg["owners"]
    pending: dict[str, dict[str, str]] = cfg.get("pending", {})
    errors = []
    for manifest in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        data = tomllib.loads(manifest.read_text())
        crate = data["package"]["name"]
        deps = {}
        for section in ("dependencies", "build-dependencies"):
            deps.update(data.get(section, {}))
        for target in data.get("target", {}).values():
            deps.update(target.get("dependencies", {}))
        for dep in deps:
            if dep in owners and crate not in owners[dep]:
                if crate in pending.get(dep, {}):
                    continue
                errors.append(f"{crate} depends on volatile crate '{dep}'; only {owners[dep]} may (see migration/dep-firewall.json)")
    for dep, crates in pending.items():
        for crate, task in crates.items():
            print(f"pending: {crate} -> {dep} until task {task}")
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())

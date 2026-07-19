#!/usr/bin/env python3
"""Publish workspace crates to crates.io.

Publishes every crate marked `[package.metadata.cortex] publish = true` in
dependency order (leaves first, umbrellas last). Skips versions that already
exist on crates.io (idempotent re-runs). CI-only: expects CRATES_IO_TOKEN in
stdin from `cargo login` or in the environment.

Usage: python3 scripts/publish_packages.py [--dry-run]
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def crate_toml_files() -> list[Path]:
    return sorted((REPO_ROOT / "crates").glob("*/Cargo.toml"))


def parse_crate(toml: Path, workspace_version: str) -> dict:
    text = toml.read_text()
    name = re.search(r'^name\s*=\s*"([^"]+)"', text, flags=re.M)
    version = re.search(r'^version\s*=\s*"([^"]+)"', text, flags=re.M)
    version_workspace = re.search(r'^version\.workspace\s*=\s*true', text, flags=re.M)
    publish_meta = re.search(
        r'^\[package\.metadata\.cortex\]\s*\n(?:.*\n)*?^publish\s*=\s*(true|false)',
        text,
        flags=re.M,
    )
    deps = re.findall(r'^([a-zA-Z0-9_-]+)\s*=\s*\{\s*workspace\s*=\s*true\s*\}', text, flags=re.M)
    return {
        "path": toml,
        "name": name.group(1) if name else None,
        "version": version.group(1) if version else workspace_version if version_workspace else None,
        "publish": publish_meta.group(1) == "true" if publish_meta else True,
        "deps": deps,
    }


def workspace_version() -> str:
    text = (REPO_ROOT / "Cargo.toml").read_text()
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, flags=re.M)
    if not m:
        raise RuntimeError("Could not find workspace version")
    return m.group(1)


def index_url(name: str) -> str:
    # crates.io sparse index layout: https://index.crates.io/{2}/{2}/{name}
    lower = name.lower()
    if len(lower) == 1:
        prefix = f"1/{lower[0]}"
    elif len(lower) == 2:
        prefix = f"2/{lower[0]}{lower[1]}"
    elif len(lower) == 3:
        prefix = f"3/{lower[0]}/{lower[1]}{lower[2]}"
    else:
        prefix = f"{lower[0:2]}/{lower[2:4]}"
    return f"https://index.crates.io/{prefix}/{lower}"


def on_crates_io(name: str, version: str) -> bool:
    url = index_url(name)
    try:
        with urllib.request.urlopen(url, timeout=10) as resp:
            body = resp.read().decode("utf-8")
        for line in body.strip().splitlines():
            try:
                data = json.loads(line)
                if data.get("vers") == version:
                    return True
            except json.JSONDecodeError:
                continue
        return False
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return False
        raise


def topological_sort(crates: list[dict]) -> list[dict]:
    names = {c["name"] for c in crates}
    by_name = {c["name"]: c for c in crates}
    visited: set[str] = set()
    result: list[dict] = []

    def visit(name: str) -> None:
        if name in visited:
            return
        if name not in by_name:
            return
        visited.add(name)
        for dep in by_name[name]["deps"]:
            if dep in names:
                visit(dep)
        result.append(by_name[name])

    for c in crates:
        visit(c["name"])

    return result


def _parse_retry_after(stderr: str) -> float | None:
    """Parse the 'try again after' timestamp from crates.io 429 errors."""
    m = re.search(r"try again after ([^\s]+ [^\s]+ [^\s]+ [^\s]+ [^\s]+)", stderr)
    if not m:
        return None
    ts = m.group(1)
    try:
        from datetime import datetime, timezone

        # Example: Sun, 19 Jul 2026 15:12:39 GMT
        dt = datetime.strptime(ts, "%a, %d %b %Y %H:%M:%S %Z")
        dt = dt.replace(tzinfo=timezone.utc)
        return max(0, (dt - datetime.now(timezone.utc)).total_seconds())
    except ValueError:
        return None


def publish_crate(toml: Path, dry_run: bool) -> None:
    name = toml.parent.name
    if dry_run:
        print(f"[dry-run] cargo publish -p {name}")
        return

    max_retries = 3
    for attempt in range(max_retries):
        proc = subprocess.run(
            ["cargo", "publish", "-p", name, "--allow-dirty"],
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )
        if proc.returncode == 0:
            return

        stderr = proc.stderr
        if "429" in stderr or "Too Many Requests" in stderr:
            wait = _parse_retry_after(stderr)
            if wait is None:
                wait = 120.0
            print(
                f"rate-limited publishing {name}; waiting {wait:.0f}s before retry "
                f"(attempt {attempt + 1}/{max_retries})"
            )
            time.sleep(wait)
            continue

        print(stderr, file=sys.stderr)
        proc.check_returncode()

    raise RuntimeError(f"failed to publish {name} after {max_retries} attempts")


def check_crate(crate: dict) -> tuple[dict, bool]:
    return crate, on_crates_io(crate["name"], crate["version"])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    if not args.dry_run and not os.environ.get("CRATES_IO_TOKEN"):
        print("CRATES_IO_TOKEN not set", file=sys.stderr)
        return 1

    ws_version = workspace_version()
    crates = [parse_crate(t, ws_version) for t in crate_toml_files()]
    for c in crates:
        if c["version"] != ws_version:
            print(
                f"warning: {c['name']} version {c['version']} != workspace {ws_version}",
                file=sys.stderr,
            )

    to_publish = topological_sort([c for c in crates if c["publish"]])

    # Pre-flight: check crates.io presence in parallel.
    print("Checking crates.io index...")
    presence: dict[str, bool] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
        futures = {executor.submit(check_crate, c): c for c in to_publish}
        for future in concurrent.futures.as_completed(futures):
            c, exists = future.result()
            presence[c["name"]] = exists
            if exists:
                print(f"skip  {c['name']} {c['version']} (already on crates.io)")

    published = 0
    for c in to_publish:
        if presence.get(c["name"], False):
            continue
        print(f"build {c['name']} {c['version']}")
        publish_crate(c["path"], args.dry_run)
        # New project creation on crates.io is throttled; sleep between publishes.
        # Observed limit: ~5 new crates per 2-minute window. 25s is a conservative
        # base delay; the retry loop handles 429s with the exact Retry-After window.
        if not args.dry_run:
            time.sleep(25)
        published += 1

    print(f"published {published} crate(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

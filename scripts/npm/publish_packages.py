#!/usr/bin/env python3
"""Publish the built npm packages and prove they reached the registry.

    python3 scripts/npm/publish_packages.py <dist-dir>
    python3 scripts/npm/publish_packages.py <dist-dir> --verify-only

<dist-dir> is what build_packages.py wrote: one directory per package, each
with a package.json carrying its name and version.

Platform packages publish before the main one (which depends on them). A
version already on the registry is skipped, so a re-run only does the missing
work. After publishing, every package is checked against the registry packument
— the same document `npm install` reads — and the script exits non-zero if any
of them never turns up.

That check exists because `npm publish` printing a success line does not mean
the version is readable yet: after the v0.1.5 release, darwin-x64 took ~11
minutes to appear on the packument, so a green job and a working install were
indistinguishable from a green job and a silently missing package.
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REGISTRY = "https://registry.npmjs.org"
# The packument is cached at the CDN edge; a unique param forces the origin.
VERIFY_TIMEOUT_S = 900
VERIFY_INTERVAL_S = 15


def package_list(dist: Path) -> list[tuple[str, str, Path]]:
    """(name, version, dir) for every package, platforms first."""
    found = []
    for manifest in sorted(dist.glob("*/package.json")):
        data = json.loads(manifest.read_text())
        found.append((data["name"], data["version"], manifest.parent))
    if not found:
        sys.exit(f"no packages in {dist}")
    # The main package (no platform suffix) depends on the others, so it goes last.
    return sorted(found, key=lambda p: (p[0].split("/")[-1] == "hoocode", p[0]))


def on_registry(name: str, version: str) -> bool:
    """True once `npm install name@version` could resolve it.

    Reads the packument, not the version endpoint: install resolves through the
    packument, and that is the document that lagged after v0.1.5.
    """
    url = f"{REGISTRY}/{name.replace('/', '%2f')}?cb={time.time_ns()}"
    request = urllib.request.Request(url, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            data = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        raise
    return version in data.get("versions", {})


def publish(directory: Path) -> bool:
    """Publish one package. False means it is already there."""
    result = subprocess.run(
        ["npm", "publish", "--access", "public", "--provenance"],
        cwd=directory,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        print(f"published {directory.name}")
        return True
    # A concurrent publisher winning the race is success, not failure.
    manifest = json.loads((directory / "package.json").read_text())
    if on_registry(manifest["name"], manifest["version"]):
        print(f"{manifest['name']} was already published")
        return True
    print(result.stdout[-2000:], result.stderr[-2000:], sep="\n", file=sys.stderr)
    return False


def verify(packages: list[tuple[str, str, Path]]) -> bool:
    deadline = time.monotonic() + VERIFY_TIMEOUT_S
    pending = list(packages)
    while True:
        pending = [p for p in pending if not on_registry(p[0], p[1])]
        if not pending:
            print(f"all {len(packages)} packages are on the registry")
            return True
        if time.monotonic() >= deadline:
            names = ", ".join(f"{n}@{v}" for n, v, _ in pending)
            sys.exit(
                f"not on the registry after {VERIFY_TIMEOUT_S}s: {names}\n"
                "The publish reported success, so this is either registry-side lag that has "
                "not cleared or the version was rolled back. Re-run this workflow for the same "
                "tag once you have checked: npm view <name> versions"
            )
        remaining = int(deadline - time.monotonic())
        print(f"waiting for {len(pending)} package(s): {[n for n, _, _ in pending]} ({remaining}s left)")
        time.sleep(VERIFY_INTERVAL_S)


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    verify_only = "--verify-only" in sys.argv[1:]
    if len(args) != 1:
        sys.exit(__doc__)
    packages = package_list(Path(args[0]))
    if verify_only:
        print(f"checking {len(packages)} packages (no publish)")
    else:
        for name, version, directory in packages:
            if on_registry(name, version):
                print(f"skip {name}@{version} (already on the registry)")
                continue
            if not publish(directory):
                sys.exit(f"publish failed for {name}@{version}")
    return 0 if verify(packages) else 1


if __name__ == "__main__":
    sys.exit(main())
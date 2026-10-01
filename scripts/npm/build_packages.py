#!/usr/bin/env python3
"""Build the npm packages for a release from its binary archives.

    python3 scripts/npm/build_packages.py <version> <archives-dir> <out-dir>

<archives-dir> holds the release's hoocode-<rust-target>.tar.gz files. Writes:

    <out-dir>/hoocode-<os>-<cpu>/   one per platform: the binary, os/cpu-gated
    <out-dir>/hoocode/              @kolisachint/hoocode: a JS launcher that runs
                                    whichever platform package got installed

The platform packages are optionalDependencies of the main one, so npm and bun
install only the one matching the machine. No postinstall script.
"""

from __future__ import annotations

import json
import shutil
import sys
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCOPE = "@kolisachint"
REPO_URL = "git+https://github.com/kolisachint/hoocode.git"

# npm os/cpu -> Rust target of the release archive.
PLATFORMS = {
    ("darwin", "arm64"): "aarch64-apple-darwin",
    ("darwin", "x64"): "x86_64-apple-darwin",
    ("linux", "arm64"): "aarch64-unknown-linux-musl",
    ("linux", "x64"): "x86_64-unknown-linux-musl",
}

COMMON = {
    "license": "MIT",
    "repository": {"type": "git", "url": REPO_URL},
    "homepage": "https://kolisachint.github.io/hoocode/",
}


def write_json(path: Path, data: dict) -> None:
    path.write_text(json.dumps(data, indent=2) + "\n")


def build(version: str, archives: Path, out: Path) -> None:
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)

    optional = {}
    for (os_, cpu), target in PLATFORMS.items():
        name = f"hoocode-{os_}-{cpu}"
        archive = archives / f"hoocode-{target}.tar.gz"
        if not archive.is_file():
            sys.exit(f"missing archive: {archive}")
        pkg = out / name
        (pkg / "bin").mkdir(parents=True)
        with tarfile.open(archive) as tar:
            member = tar.getmember(f"hoocode-{target}/hoocode")
            src = tar.extractfile(member)
            assert src is not None
            dest = pkg / "bin" / "hoocode"
            dest.write_bytes(src.read())
            dest.chmod(0o755)
        shutil.copy(ROOT / "LICENSE", pkg / "LICENSE")
        (pkg / "README.md").write_text(
            f"# {SCOPE}/{name}\n\nThe `hoocode` binary for {os_}-{cpu}. "
            f"Install [`{SCOPE}/hoocode`](https://www.npmjs.com/package/{SCOPE}/hoocode) instead.\n"
        )
        write_json(
            pkg / "package.json",
            {
                "name": f"{SCOPE}/{name}",
                "version": version,
                "description": f"hoocode binary for {os_}-{cpu}",
                "os": [os_],
                "cpu": [cpu],
                "files": ["bin/hoocode"],
                **COMMON,
            },
        )
        optional[f"{SCOPE}/{name}"] = version

    main = out / "hoocode"
    shutil.copytree(ROOT / "npm" / "hoocode", main)
    shutil.copy(ROOT / "LICENSE", main / "LICENSE")
    shutil.copy(ROOT / "README.md", main / "README.md")
    write_json(
        main / "package.json",
        {
            "name": f"{SCOPE}/hoocode",
            "version": version,
            "description": "HooCode, the terminal coding agent (Rust build)",
            "bin": {"hoocode": "bin/hoocode.js", "hoo": "bin/hoocode.js"},
            "files": ["bin/hoocode.js"],
            "engines": {"node": ">=18"},
            "optionalDependencies": optional,
            **COMMON,
        },
    )
    print(f"built {len(optional) + 1} packages for {version} in {out}")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    build(sys.argv[1].removeprefix("v"), Path(sys.argv[2]), Path(sys.argv[3]))

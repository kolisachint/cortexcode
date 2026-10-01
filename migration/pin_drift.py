#!/usr/bin/env python3
"""How far the hoocode pin is behind upstream, and what a bump must port.

The pin (`[workspace.metadata.cortex.source]` in Cargo.toml) only moves by a
deliberate bump (plan §0.1). Between bumps hoocode keeps releasing: new catalog
models, provider wire fixes (e.g. OpenCode Go's `x-opencode-session`). Nothing
showed that drift, so users hit missing models and 400s before anyone noticed.

    pin_drift.py status            # pin vs latest upstream release tag (exit 0)
    pin_drift.py check             # exit 1 when a newer release exists (CI / nightly)
    pin_drift.py delta [<to-ref>]  # bump checklist: commits + changed src files -> crates

`delta` needs a hoocode checkout with history (default target/hoocode-pin, or
HOOCODE_PIN_DIR); `status`/`check` only need network access to the repository.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIN_DIR = Path(os.environ.get("HOOCODE_PIN_DIR", ROOT / "target" / "hoocode-pin"))

# Changed hoocode source path (prefix) -> Rust crates that port it. First match wins.
SOURCE_MAP: list[tuple[str, str]] = [
    ("packages/ai/src/models.generated.ts", "ai-models-catalog (scripts/convert_models_to_json.py)"),
    ("packages/ai/src/image-models.generated.ts", "ai-models-catalog (scripts/convert_models_to_json.py)"),
    ("packages/ai/src/providers/anthropic.ts", "ai-provider-anthropic"),
    ("packages/ai/src/providers/openai-completions.ts", "ai-provider-openai"),
    ("packages/ai/src/providers/openai-responses", "ai-provider-openai-responses"),
    ("packages/ai/src/providers/openai-codex", "ai-provider-openai-codex"),
    ("packages/ai/src/providers/azure", "ai-provider-azure"),
    ("packages/ai/src/providers/google-gemini-cli", "ai-provider-google-gemini-cli"),
    ("packages/ai/src/providers/google", "ai-provider-google"),
    ("packages/ai/src/providers/images", "ai-images"),
    ("packages/ai/src/utils/oauth", "ai-oauth*"),
    ("packages/ai/src/env-api-keys", "ai-env"),
    ("packages/ai/src/models.ts", "ai-models"),
    ("packages/ai/src", "ai-types / ai-util / ai-stream"),
    ("packages/agent/src", "agent-*"),
    ("packages/tui/src", "tui-*"),
    ("packages/coding-agent/src/core/model-resolver.ts", "code-models (resolver.rs)"),
    ("packages/coding-agent/src/core/model-registry.ts", "code-models"),
    ("packages/coding-agent/src/modes/interactive/theme", "code-tui-theme (themes/*.json, copy verbatim)"),
    ("packages/coding-agent/src/modes/interactive", "code-tui-app / code-tui-*"),
    ("packages/coding-agent/src/core/settings", "code-settings"),
    ("packages/coding-agent/src/core/tools", "code-tool* / code-tools*"),
    ("packages/coding-agent/src", "code-* (see plan §5.5)"),
]


def pin() -> tuple[str, str, str]:
    src = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["metadata"]["cortex"]["source"]
    return src["hoocode-version"], src["hoocode-commit"], src["repository"]


def git(*args: str, cwd: Path | None = None) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def version_key(tag: str) -> tuple[int, ...]:
    return tuple(int(x) for x in re.findall(r"\d+", tag))


def latest_release(repo: str) -> tuple[str, str]:
    """(tag, commit) of the highest `vX.Y.Z` tag upstream."""
    tags = []
    for line in git("ls-remote", "--tags", repo, "v*").splitlines():
        sha, ref = line.split("\t")
        tag = ref.removeprefix("refs/tags/")
        if re.fullmatch(r"v\d+\.\d+\.\d+", tag):
            tags.append((version_key(tag), tag, sha))
    if not tags:
        sys.exit(f"no release tags at {repo}")
    _, tag, sha = max(tags)
    return tag, sha


def status(fail_when_behind: bool) -> int:
    version, commit, repo = pin()
    tag, sha = latest_release(repo)
    behind = version_key(tag) > version_key(version)
    print(f"pin     v{version} ({commit[:8]})")
    print(f"latest  {tag} ({sha[:8]}) at {repo}")
    if not behind:
        print("pin is at the latest release")
        return 0
    print(f"BEHIND: hoocode {tag} is released. Catalog models and provider fixes after the pin")
    print("are missing from cortex. Ask the user before bumping (plan §0.1), then run:")
    print(f"  python3 migration/pin_drift.py delta {tag}")
    return 1 if fail_when_behind else 0


def delta(to_ref: str | None) -> int:
    _, commit, repo = pin()
    if not (PIN_DIR / ".git").exists():
        sys.exit(f"{PIN_DIR} is not a git checkout; run migration/tui-parity/setup_hoocode.sh")
    if to_ref is None:
        to_ref = latest_release(repo)[0]
    git("fetch", "-q", "--filter=blob:none", "origin", "tag", to_ref, cwd=PIN_DIR)
    # HOOCODE_DELTA_BASE re-derives a past bump's checklist (base = the old pin).
    base = os.environ.get("HOOCODE_DELTA_BASE", commit)
    print(f"## hoocode delta {base[:8]}..{to_ref}\n")
    print("### Commits\n")
    for line in git("log", "--no-merges", "--format=- %h %s", f"{base}..{to_ref}", cwd=PIN_DIR).splitlines():
        print(line)
    files = git("diff", "--name-only", f"{base}..{to_ref}", "--", "packages", cwd=PIN_DIR).split()
    src = [f for f in files if "/src/" in f]
    tests = [f for f in files if "/test/" in f]
    print("\n### Source files to port\n")
    for f in src:
        owner = next((crate for prefix, crate in SOURCE_MAP if f.startswith(prefix)), "unmapped: decide")
        print(f"- [ ] `{f}` -> {owner}")
    print("\n### TS tests changed (port or update the Rust counterpart)\n")
    for f in tests:
        print(f"- [ ] `{f}`")
    print("\n### Every bump\n")
    print("- [ ] Cargo.toml pin + migration/ledger.json pin + plan §0.1 header")
    print("- [ ] migration/tui-parity/setup_hoocode.sh, then python3 scripts/convert_models_to_json.py")
    print("- [ ] cargo test -p cortexcode-code-models (every default is in the catalog)")
    print("- [ ] python3 migration/ts_tests.py generate; L1 + L2 gates")
    return 0


def main() -> int:
    cmd = sys.argv[1] if len(sys.argv) > 1 else "status"
    if cmd == "status":
        return status(False)
    if cmd == "check":
        return status(True)
    if cmd == "delta":
        return delta(sys.argv[2] if len(sys.argv) > 2 else None)
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())

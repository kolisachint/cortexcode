#!/usr/bin/env bash
# Fetch only the pinned hoocode files the Rust tests read (fixtures, templates)
# into target/hoocode-pin, without installing or building hoocode. CI uses this;
# L2 parity needs the full build from migration/tui-parity/setup_hoocode.sh,
# which turns the sparse checkout back into a full one.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="${HOOCODE_PIN_DIR:-$ROOT/target/hoocode-pin}"
COMMIT="$(python3 -c "import tomllib;print(tomllib.load(open('$ROOT/Cargo.toml','rb'))['workspace']['metadata']['cortex']['source']['hoocode-commit'])")"
REPO="${HOOCODE_REPO:-https://github.com/kolisachint/hoocode-ts}"

if [[ ! -d "$DEST/.git" ]]; then
  git init -q "$DEST"
  git -C "$DEST" remote add origin "$REPO"
fi
git -C "$DEST" sparse-checkout set --no-cone \
  /packages/coding-agent/test/fixtures/ \
  /packages/coding-agent/templates/
git -C "$DEST" fetch -q --depth 1 --filter=blob:none origin "$COMMIT"
git -C "$DEST" checkout -q --force --detach FETCH_HEAD
echo "hoocode $COMMIT fixtures at $DEST"

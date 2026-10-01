#!/usr/bin/env bash
# Build the pinned hoocode reference into target/hoocode-pin (gitignored).
# Reads the pin from [workspace.metadata.cortex.source] in the root Cargo.toml.
# Idempotent: skips clone/build when the pinned commit is already built.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="${HOOCODE_PIN_DIR:-$ROOT/target/hoocode-pin}"
COMMIT="$(python3 -c "import tomllib;print(tomllib.load(open('$ROOT/Cargo.toml','rb'))['workspace']['metadata']['cortex']['source']['hoocode-commit'])")"
REPO="${HOOCODE_REPO:-https://github.com/kolisachint/hoocode}"

# The file-autocomplete scenario walks files with fd; --offline never downloads it.
if ! command -v fd >/dev/null && ! command -v fdfind >/dev/null; then
  echo "warning: fd not on PATH (apt-get install fd-find); L2 file-autocomplete will fail" >&2
fi

if [[ -f "$DEST/.built-$COMMIT" ]]; then
  echo "hoocode $COMMIT already built at $DEST"
  exit 0
fi

if [[ ! -d "$DEST/.git" ]]; then
  git clone --filter=blob:none --no-checkout "$REPO" "$DEST"
fi
# A sparse checkout left by scripts/ci/fetch_hoocode_fixtures.sh: build needs every file.
git -C "$DEST" sparse-checkout disable 2>/dev/null || true
git -C "$DEST" fetch --filter=blob:none origin "$COMMIT" 2>/dev/null || git -C "$DEST" fetch origin
git -C "$DEST" checkout --force --detach "$COMMIT"

cd "$DEST"
rm -f .built-*
bun install --frozen-lockfile
npm run build
touch ".built-$COMMIT"
echo "hoocode $COMMIT built at $DEST"

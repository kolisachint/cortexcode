#!/usr/bin/env bash
# Build the pinned hoocode reference into target/hoocode-pin (gitignored).
# Reads the pin from [workspace.metadata.cortex.source] in the root Cargo.toml.
# Idempotent: skips clone/build when the pinned commit is already built.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEST="${HOOCODE_PIN_DIR:-$ROOT/target/hoocode-pin}"
COMMIT="$(python3 -c "import tomllib;print(tomllib.load(open('$ROOT/Cargo.toml','rb'))['workspace']['metadata']['cortex']['source']['hoocode-commit'])")"
REPO="${HOOCODE_REPO:-https://github.com/kolisachint/hoocode}"

if [[ -f "$DEST/.built-$COMMIT" ]]; then
  echo "hoocode $COMMIT already built at $DEST"
  exit 0
fi

if [[ ! -d "$DEST/.git" ]]; then
  git clone --filter=blob:none --no-checkout "$REPO" "$DEST"
fi
git -C "$DEST" fetch --filter=blob:none origin "$COMMIT" 2>/dev/null || git -C "$DEST" fetch origin
git -C "$DEST" checkout --force --detach "$COMMIT"

cd "$DEST"
rm -f .built-*
bun install --frozen-lockfile
npm run build
touch ".built-$COMMIT"
echo "hoocode $COMMIT built at $DEST"

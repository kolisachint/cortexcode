#!/usr/bin/env bash
# Install the Rust build as `hoocode` (plus the `hoocode-ts` shim for the
# TypeScript one). The binary is built as `cortex`; only the installed name
# changes, so nothing in the code needs renaming.
#
# Usage: scripts/install.sh [--prefix DIR] [--also-cortex]
#   --prefix DIR    install into DIR (default: ~/.local/bin)
#   --also-cortex   keep a `cortex` link too
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="$HOME/.local/bin"
ALSO_CORTEX=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="$2"; shift 2 ;;
    --also-cortex) ALSO_CORTEX=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" --bin cortex
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps --manifest-path "$ROOT/Cargo.toml" \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"

mkdir -p "$PREFIX"
install -m 755 "$TARGET_DIR/release/cortex" "$PREFIX/hoocode"
install -m 755 "$ROOT/scripts/shims/hoocode-ts" "$PREFIX/hoocode-ts"
if [[ $ALSO_CORTEX == 1 ]]; then
  ln -sf hoocode "$PREFIX/cortex"
fi

echo "installed: $PREFIX/hoocode (Rust), $PREFIX/hoocode-ts (TypeScript shim)"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) echo "note: $PREFIX is not on PATH" >&2 ;;
esac
if [[ "$(command -v hoocode || true)" != "$PREFIX/hoocode" && -n "$(command -v hoocode || true)" ]]; then
  echo "note: another hoocode comes first on PATH: $(command -v hoocode)" >&2
  echo "      (an npm global install of the TS one?) put $PREFIX earlier on PATH" >&2
fi

#!/usr/bin/env bash
# Generate the Codex app-server JSON schemas into target/codex-schema/ for the
# conformance test. Test-time only: Codex is Apache-2.0, so the output is never
# checked in. Exits 0 (with a message) when no codex binary is available.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/target/codex-schema"
codex="${CODEX_BIN:-}"
if [[ -z "$codex" ]]; then
  if command -v codex >/dev/null 2>&1; then
    codex="$(command -v codex)"
  elif [[ -x "$HOME/.codex/packages/standalone/current/codex" ]]; then
    codex="$HOME/.codex/packages/standalone/current/codex"
  else
    echo "codex-schema: no codex binary (set CODEX_BIN); skipping" >&2
    exit 0
  fi
fi
rm -rf "$out"
"$codex" app-server generate-json-schema --out "$out" >/dev/null
echo "codex-schema: $("$codex" --version) -> $out" >&2

#!/usr/bin/env bash
# Usage: run.sh <script.mjs> <output.json>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
dist="$here/../../../target/hoocode-pin/packages/coding-agent/dist"
name="_golden_$(basename "$1")"
cp "$1" "$dist/$name"
trap 'rm -f "$dist/$name"' EXIT
COLORTERM=truecolor FORCE_COLOR=3 node "$dist/$name" > "$2"

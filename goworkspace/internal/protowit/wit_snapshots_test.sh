#!/usr/bin/env bash
set -euo pipefail

wasm_tools="$1"
snapshots="$2"

directories="$(find -L "$snapshots" -name '*.wit' -exec dirname {} \; | sort -u)"
if [ -z "$directories" ]; then
  echo "no WIT snapshots found under $snapshots" >&2
  exit 1
fi

while IFS= read -r directory; do
  "$wasm_tools" component wit "$directory" --wasm -o /dev/null
done <<<"$directories"

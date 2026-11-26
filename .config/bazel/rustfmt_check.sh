#!/usr/bin/env bash
# Usage: rustfmt_check.sh <marker> <rustfmt> <config> <edition> <file>...
set -euo pipefail

marker="$1"
rustfmt="$2"
config="$3"
edition="$4"
shift 4

files=()
for file in "$@"; do
  first_line=""
  IFS= read -r first_line <"$file" || true
  if [[ "$first_line" != *@generated* ]]; then
    files+=("$file")
  fi
done

if ((${#files[@]})); then
  "$rustfmt" --config-path "$config" --edition "$edition" --config skip_children=true --check "${files[@]}"
fi
touch "$marker"

#!/usr/bin/env bash
# Fails when a decider component declares an import or does not validate.
set -euo pipefail

wasm_tools="$1"
component="$2"

wit_text="$("$wasm_tools" component wit "$component")"
printf '%s\n' "$wit_text"
if grep -E '^[[:space:]]*import ' <<<"$wit_text"; then
    echo "component declares imports: $component" >&2
    exit 1
fi
"$wasm_tools" validate "$component"

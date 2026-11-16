#!/usr/bin/env bash
# Fails when the wasm components' crate_universe lockfile pins a registry crate
# at a version the workspace Cargo.lock does not, so the Bazel wasm build keeps
# the dependency versions `cargo component build` uses.
set -euo pipefail

wasm_lock="$1"
cargo_lock="$2"

registry_packages() {
    awk '
        /^\[\[package\]\]/ { name = ""; version = ""; next }
        /^name = / { name = $3; gsub(/"/, "", name); next }
        /^version = / { version = $3; gsub(/"/, "", version); next }
        /^source = "registry\+/ { print name " " version }
    ' "$1" | sort -u
}

missing="$(comm -23 <(registry_packages "$wasm_lock") <(registry_packages "$cargo_lock"))"
if [[ -n "$missing" ]]; then
    echo "$wasm_lock pins registry crates that $cargo_lock does not:" >&2
    while IFS= read -r package; do
        echo "  $package" >&2
    done <<<"$missing"
    echo "align the crate.spec versions in .config/bazel/deps.MODULE.bazel with $cargo_lock and repin" >&2
    exit 1
fi
echo "$(registry_packages "$wasm_lock" | wc -l | tr -d ' ') registry crates match $cargo_lock"

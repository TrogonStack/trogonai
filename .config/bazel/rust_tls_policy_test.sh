#!/usr/bin/env bash
# Enforces ADR#0015: fails if native-tls or OpenSSL enter the Rust dependency graph.
set -euo pipefail

# cargo metadata reads every workspace member's manifest and sources, which no
# single Bazel package can list, so the check runs against the source tree the
# workspace manifest links back to.
workspace="$(dirname "$(readlink -f "${CARGO_MANIFEST:?}")")"

export CARGO="${PWD}/${CARGO:?}"
export RUSTC="${PWD}/${RUSTC:?}"
export CARGO_HOME="${TEST_TMPDIR:?}/cargo-home"

# --locked keeps cargo from rewriting the checked-in lockfile.
exec "${PWD}/${CARGO_DENY:?}" --manifest-path "${workspace}/Cargo.toml" --locked check bans

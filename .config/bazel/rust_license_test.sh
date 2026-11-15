#!/usr/bin/env bash
# Fails unless every Cargo workspace member declares the expected SPDX license
# in its own manifest.
set -euo pipefail

expected_license="Apache-2.0"

# Workspace members come from globs over directories that are separate Bazel
# packages, so cargo reads them from the source tree the workspace manifest
# links back to.
workspace="$(dirname "$(readlink -f "${CARGO_MANIFEST:?}")")"

export CARGO_HOME="${TEST_TMPDIR:?}/cargo-home"
export RUSTC="${PWD}/${RUSTC:?}"

metadata="$("${PWD}/${CARGO:?}" metadata --no-deps --offline --format-version 1 \
  --manifest-path "${workspace}/Cargo.toml")"

manifests="$(grep -o '"manifest_path":"[^"]*"' <<<"${metadata}" | cut -d'"' -f4)"
if [ -z "${manifests}" ]; then
  echo "cargo metadata listed no workspace members" >&2
  exit 1
fi

violations=0
while IFS= read -r manifest; do
  # Only a literal value in [package] counts; an inherited one would not be
  # visible to tools that read the crate's manifest alone.
  declared="$(awk '
    /^[[:space:]]*\[/ { in_package = ($0 ~ /^[[:space:]]*\[package\][[:space:]]*(#.*)?$/) }
    in_package && /^[[:space:]]*license[[:space:]]*=/ { sub(/^[^=]*=[[:space:]]*/, ""); sub(/[[:space:]]*(#.*)?$/, ""); print; exit }
  ' "${manifest}")"
  if [ "${declared}" != "\"${expected_license}\"" ]; then
    echo "${manifest#"${workspace}/"}: license is ${declared:-missing}" >&2
    violations=$((violations + 1))
  fi
done <<<"${manifests}"

if [ "${violations}" -gt 0 ]; then
  echo "Every Rust crate must declare license = \"${expected_license}\" in Cargo.toml." >&2
  exit 1
fi

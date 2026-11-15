#!/usr/bin/env bash
# Runs one check against a private copy of the Buf workspace, offline, with the
# registry modules from the //proto:buf_cache archive.
#
# Usage: buf_test.sh BUF CACHE_ARCHIVE CHECK [CHECK_ARGS...] -- WORKSPACE_FILES...
set -euo pipefail

buf="$PWD/$1"
cache="$PWD/$2"
check="$3"
shift 3

check_args=()
while [[ $# -gt 0 && "$1" != "--" ]]; do
  check_args+=("$1")
  shift
done
shift

# Runfiles are symlinks into the source tree, and generators write next to the
# files they replace, so every check works on real copies.
ws="$TEST_TMPDIR/ws"
mkdir -p "$ws"
for file in "$@"; do
  mkdir -p "$ws/$(dirname "$file")"
  cp -L "$file" "$ws/$file"
done

export HOME="$TEST_TMPDIR"
export BUF_CACHE_DIR="$TEST_TMPDIR/buf-cache"
mkdir -p "$BUF_CACHE_DIR"
tar -xf "$cache" -C "$BUF_CACHE_DIR"
# Any registry request means a module is missing from the cache, so it fails
# instead of quietly reaching the network.
export HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 NO_PROXY=

# Fails with the difference between the committed tree and the regenerated one.
assert_generated() {
  local committed="$1" generated="$2" regenerate="$3"
  if ! diff -ru "$committed" "$generated"; then
    echo "$committed is stale; regenerate it with \`$regenerate\`." >&2
    exit 1
  fi
}

case "$check" in
format)
  cd "$ws"
  exec "$buf" format --diff --exit-code proto
  ;;
lint)
  cd "$ws"
  exec "$buf" lint
  ;;
build)
  cd "$ws"
  exec "$buf" build
  ;;
schedules-descriptor)
  cd "$ws"
  exec "$buf" build -o "${check_args[0]}" --as-file-descriptor-set --path proto/trogonai/scheduler/schedules
  ;;
rust)
  # CHECK_ARGS: OUT_DIR CARGO_TOML PROTOC_GEN_BUFFA PROTOC_GEN_BUFFA_PACKAGING RUSTFMT RUSTFMT_TOML
  out="${check_args[0]}"
  cargo_toml="${check_args[1]}"
  rustfmt="$PWD/${check_args[4]}"
  rustfmt_toml="$PWD/${check_args[5]}"

  # buf.gen.yaml starts each plugin through the mise launcher; this one runs the
  # plugin Bazel provides under the same name.
  mkdir -p "$ws/.config/mise/lib" "$TEST_TMPDIR/bin"
  ln -s "$PWD/${check_args[2]}" "$TEST_TMPDIR/bin/protoc-gen-buffa"
  ln -s "$PWD/${check_args[3]}" "$TEST_TMPDIR/bin/protoc-gen-buffa-packaging"
  printf '#!/bin/sh\nplugin=$1\nshift\nexec "%s/bin/$plugin" "$@"\n' "$TEST_TMPDIR" >"$ws/.config/mise/lib/proto-plugin"
  chmod +x "$ws/.config/mise/lib/proto-plugin"

  (cd "$ws" && "$buf" generate)

  # proto:generate finishes with cargo fmt, which reaches the generated tree
  # through its mod.rs with the crate's edition and the workspace rustfmt.toml.
  edition="$(sed -n 's/^edition = "\(.*\)"$/\1/p' "$cargo_toml")"
  "$rustfmt" --edition "$edition" --config-path "$rustfmt_toml" "$ws/$out/mod.rs"

  assert_generated "$out" "$ws/$out" "mise run proto:generate"
  ;;
contracts)
  # CHECK_ARGS: RUNNER
  runner="$PWD/${check_args[0]}"
  exec "$runner" "$ws/tests/proto/validate.py" --buf "$buf"
  ;;
typescript)
  # CHECK_ARGS: PACKAGE_DIR PROTOC_GEN_ES
  package="${check_args[0]}"
  protoc_gen_es="$PWD/${check_args[1]}"

  # buf.key-custody.gen.yaml runs the plugin pnpm links into node_modules. The
  # launcher refuses to start without BAZEL_BINDIR, and "." keeps it in the
  # directory buf starts it from.
  mkdir -p "$ws/$package/node_modules/.bin"
  printf '#!/bin/sh\nBAZEL_BINDIR=. exec "%s" "$@"\n' "$protoc_gen_es" >"$ws/$package/node_modules/.bin/protoc-gen-es"
  chmod +x "$ws/$package/node_modules/.bin/protoc-gen-es"

  "$ws/$package/generate-key-custody.sh" "$buf"

  assert_generated "$package/src/gen" "$ws/$package/src/gen" "mise run proto:generate-key-custody-typescript"
  ;;
*)
  echo "unknown check: $check" >&2
  exit 2
  ;;
esac

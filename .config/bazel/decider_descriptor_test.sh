#!/usr/bin/env bash
set -euo pipefail

component="$1"
descriptor="${component%.wasm}.binpb"
if [[ ! -f "$descriptor" ]]; then
  echo "Missing descriptor beside component: $descriptor" >&2
  exit 1
fi

buf="$PWD/$2"
cache="$PWD/$3"
shift 3
ws="$TEST_TMPDIR/workspace"
mkdir -p "$ws"
for file in "$@"; do
  mkdir -p "$ws/$(dirname "$file")"
  cp -L "$file" "$ws/$file"
done
export BUF_CACHE_DIR="$TEST_TMPDIR/buf-cache"
mkdir -p "$BUF_CACHE_DIR"
tar -xf "$cache" -C "$BUF_CACHE_DIR"
export HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=http://127.0.0.1:9 NO_PROXY=

# The established proto:schedules-descriptor-set command, with its output kept
# in TEST_TMPDIR rather than the Cargo target tree.
(cd "$ws" && "$buf" build proto -o "$TEST_TMPDIR/reference.binpb" --as-file-descriptor-set --path proto/trogonai/scheduler/schedules)
cmp "$descriptor" "$TEST_TMPDIR/reference.binpb"
echo "Schedules FileDescriptorSet matches proto:schedules-descriptor-set byte for byte"

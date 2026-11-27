#!/usr/bin/env bash
# Fetches the Buf registry modules pinned in buf.lock into a cache archive, so
# the proto tests run offline against exactly those modules.
set -euo pipefail

buf="$PWD/$1"
buf_yaml="$2"
buf_lock="$3"
archive="$PWD/$4"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# A module with one empty file still resolves every dependency in buf.lock, and
# keeps the archive independent of the proto sources.
mkdir -p "$work/module/proto/bufcache"
cp "$buf_yaml" "$buf_lock" "$work/module/proto/"
printf 'syntax = "proto3";\n\npackage bufcache;\n' >"$work/module/proto/bufcache/bufcache.proto"

export HOME="$work"
export BUF_CACHE_DIR="$work/cache"
(cd "$work/module" && "$buf" build proto)
tar -cf "$archive" -C "$BUF_CACHE_DIR" .

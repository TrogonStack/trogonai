#!/usr/bin/env bash
set -euo pipefail

weaver="$(pwd)/$1"
semconv="$2"

# Weaver keeps its state under HOME, which a test must not write outside of.
export HOME="$TEST_TMPDIR"

cd "$semconv"
exec "$weaver" registry check --registry registry --policy policies

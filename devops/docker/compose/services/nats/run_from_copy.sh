#!/bin/sh
# Runs a bootstrap contract test from a real copy of this directory. The test
# bind-mounts its own directory into a container, which cannot follow the
# runfiles symlinks back to the source tree.
set -eu

test_script="$1"
shift

dir="${TEST_TMPDIR:?}/bootstrap"
mkdir -p "$dir"
for file in "$@"; do
  cp -L "$file" "$dir/"
done

exec sh "$dir/$(basename "$test_script")"

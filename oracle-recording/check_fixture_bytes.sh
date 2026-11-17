#!/usr/bin/env bash
# Every tracked file the oracle and the recorders read hashes to its committed blob byte for byte, so no checkout
# line-ending conversion or filter rewrote a fixture, a golden or a script before the run.
#
# Usage: check_fixture_bytes.sh <repository> <output folder>
set -euo pipefail

repo="$1" out="$2"
mkdir -p "$out"
cd "$repo"

git -c core.quotePath=false ls-files -s -- oracle-recording rsworkspace/crates/platform/trogon-claude-agent-sdk \
  | awk -F'\t' '{ split($1, entry, " "); if (entry[1] != "120000" && entry[1] != "160000") print entry[2] "\t" $2 }' \
  > "$out/fixture_blobs.tsv"
test -s "$out/fixture_blobs.tsv"
cut -f2 "$out/fixture_blobs.tsv" | git hash-object --no-filters --stdin-paths > "$out/fixture_bytes.txt"
paste "$out/fixture_blobs.tsv" "$out/fixture_bytes.txt" \
  | awk -F'\t' '$1 != $3 { print "differs: " $2; bad++ } END { print NR " tracked files, " bad + 0 " differ"; exit bad > 0 }' \
  | tee "$out/fixture_check.txt"

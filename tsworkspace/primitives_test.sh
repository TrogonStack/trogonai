#!/usr/bin/env bash
set -euo pipefail

lockfile="$1"

if grep -n '@radix-ui/' "$lockfile" >&2; then
  echo "pnpm-lock.yaml resolves @radix-ui packages; build on Base UI primitives instead" >&2
  exit 1
fi

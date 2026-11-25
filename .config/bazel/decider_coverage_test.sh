#!/usr/bin/env bash
# Reports the decider suite and parity coverage that decider.bzl resolved from
# Cargo metadata and the checked-in suites, and fails on any violation.
set -euo pipefail

if [[ -n "${DECIDER_NOTES:-}" ]]; then
    printf '%s\n' "$DECIDER_NOTES"
fi
if [[ -n "${DECIDER_VIOLATIONS:-}" ]]; then
    echo "decider conformance gate failed:" >&2
    while IFS= read -r violation; do
        echo "  $violation" >&2
    done <<<"$DECIDER_VIOLATIONS"
    exit 1
fi

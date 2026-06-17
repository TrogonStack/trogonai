#!/usr/bin/env bash
# Deterministic local mirror of the rsworkspace CI gate on a single crate.
# Mirrors .github/workflows/ci-rust.yml: fmt --check, clippy --all-targets
# --all-features, cargo cov nextest --cobertura, coverage gate >=95%.
#
# Usage: rsworkspace/scripts/pr-check.sh <crate-name>
# Example: rsworkspace/scripts/pr-check.sh a2a-nats
#
# Exits non-zero on the FIRST step that fails. Prints a one-line PASS/FAIL
# summary per step so the failing gate is unambiguous before pushing.

set -u
set -o pipefail

CRATE="${1:-}"
if [[ -z "$CRATE" ]]; then
    echo "usage: $0 <crate-name>" >&2
    exit 2
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
RSWS="$REPO_ROOT/rsworkspace"
THRESHOLD=95

if [[ ! -d "$RSWS/crates/$CRATE" ]]; then
    echo "FAIL: crate '$CRATE' not found under $RSWS/crates/" >&2
    exit 2
fi

cd "$RSWS"

step() {
    local name="$1"; shift
    echo
    echo "================================================================"
    echo "STEP: $name"
    echo "  cmd: $*"
    echo "================================================================"
    if "$@"; then
        echo "  RESULT: PASS — $name"
    else
        local rc=$?
        echo "  RESULT: FAIL — $name (exit $rc)"
        echo
        echo "OVERALL: FAIL ($name)"
        exit "$rc"
    fi
}

# 1. fmt --check — mirrors CI 'Check formatting' step exactly.
step "fmt-check" mise exec -- cargo fmt --all -- --check

# 2. clippy from a clean cache — workspace lints (deny warnings, unwrap_used,
#    expect_used, panic) only fire on a real rebuild; cached builds skip them.
step "clean-crate" mise exec -- cargo clean -p "$CRATE"
step "clippy" mise exec -- cargo clippy -p "$CRATE" --all-targets --all-features

# 3. Unit tests — fast feedback before coverage.
step "test" mise exec -- cargo test -p "$CRATE" --all-targets

# 4. doc tests — CI runs these too.
step "doc-test" mise exec -- cargo test -p "$CRATE" --doc

# 5. Coverage gate — uses the same `cargo cov` alias the CI workflow does and
#    parses pycobertura output the same way the action does.
echo
echo "================================================================"
echo "STEP: coverage"
echo "  cmd: mise exec -- cargo cov nextest --cobertura --output-path coverage.xml"
echo "================================================================"
COV_CMD=(mise exec -- cargo cov nextest --cobertura --output-path coverage.xml)
if ! command -v cargo-nextest >/dev/null 2>&1 && ! mise exec -- cargo nextest --version >/dev/null 2>&1; then
    echo "  NOTE: cargo-nextest not installed locally; falling back to 'cargo cov --cobertura'."
    COV_CMD=(mise exec -- cargo cov --cobertura --output-path coverage.xml)
fi
if ! "${COV_CMD[@]}"; then
    echo "  RESULT: FAIL — coverage run"
    exit 1
fi

if ! command -v pycobertura >/dev/null 2>&1; then
    echo "  NOTE: pycobertura not installed; install with 'pipx install pycobertura' to mirror CI."
    echo "  RESULT: SKIP — coverage threshold check"
else
    REPORT="$(pycobertura show coverage.xml)"
    echo "$REPORT" | grep -E "^crates/$CRATE/.*\.rs\s" || true
    UNCOVERED_FILES="$(echo "$REPORT" | awk -v c="crates/$CRATE/" '$1 ~ "^"c && $3 != 0 {print $1": "$3" uncovered ("$4")"}')"
    if [[ -n "$UNCOVERED_FILES" ]]; then
        echo
        echo "  Files in $CRATE with uncovered statements:"
        echo "$UNCOVERED_FILES"
    fi
    TOTAL_LINE="$(echo "$REPORT" | awk '/^TOTAL /')"
    TOTAL_PCT="$(echo "$TOTAL_LINE" | awk '{print $NF}' | tr -d '%')"
    if [[ -z "$TOTAL_PCT" ]]; then
        echo "  RESULT: FAIL — could not parse total coverage from pycobertura"
        exit 1
    fi
    awk -v p="$TOTAL_PCT" -v t="$THRESHOLD" 'BEGIN { exit !(p+0 >= t+0) }'
    if [[ $? -ne 0 ]]; then
        echo "  RESULT: FAIL — total coverage ${TOTAL_PCT}% < ${THRESHOLD}% threshold"
        exit 1
    fi
    echo "  RESULT: PASS — total coverage ${TOTAL_PCT}% >= ${THRESHOLD}%"
fi

echo
echo "================================================================"
echo "OVERALL: PASS — all CI-mirrored gates green for $CRATE"
echo "================================================================"

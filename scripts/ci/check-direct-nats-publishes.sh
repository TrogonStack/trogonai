#!/usr/bin/env bash
# Fail when a non-SDK crate publishes directly to mcp.gateway.request.* subjects.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RSWORKSPACE="${ROOT}/rsworkspace"
FIXTURES="${ROOT}/scripts/ci/fixtures"

WHITELIST_PREFIXES=(
  "crates/trogon-a2a-sdk/"
)

is_whitelisted() {
  local rel="$1"
  for prefix in "${WHITELIST_PREFIXES[@]}"; do
    if [[ "${rel}" == "${prefix}"* ]]; then
      return 0
    fi
  done
  if [[ "${rel}" == */tests/* ]] || [[ "${rel}" == */test/* ]] || [[ "${rel}" == *"_test.rs" ]] || [[ "${rel}" == *"/tests.rs" ]]; then
    return 0
  fi
  return 1
}

find_violations_in() {
  local scan_root="$1"
  local violations=()
  while IFS= read -r file; do
    [[ -z "${file}" ]] && continue
    local rel="${file#${RSWORKSPACE}/}"
    if [[ "${scan_root}" != "${RSWORKSPACE}/crates"* ]]; then
      rel="${file#${ROOT}/}"
    fi
    if is_whitelisted "${rel}"; then
      continue
    fi
    if rg -n '\.publish\([^)]*mcp\.gateway\.request' "${file}" >/dev/null 2>&1; then
      violations+=("${rel}")
    fi
  done < <(rg -l '\.publish\(' "${scan_root}" --glob '*.rs' 2>/dev/null || true)
  if ((${#violations[@]} > 0)); then
    printf '%s\n' "${violations[@]}"
    return 1
  fi
  return 0
}

if ! find_violations_in "${RSWORKSPACE}/crates" >/dev/null; then
  echo "Direct mcp.gateway.request publish violations (use trogon-a2a-sdk instead):" >&2
  find_violations_in "${RSWORKSPACE}/crates" >&2 || true
  exit 1
fi

if find_violations_in "${FIXTURES}" >/dev/null; then
  echo "fixture regression: expected violation in ${FIXTURES} but scan was clean" >&2
  exit 1
fi

echo "check-direct-nats-publishes: ok"

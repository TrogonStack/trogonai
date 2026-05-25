#!/usr/bin/env bash
set -euo pipefail
real=/usr/local/bin/nsc-real
args=()
skip_next=0
for arg in "$@"; do
  if [ "${skip_next}" -eq 1 ]; then
    skip_next=0
    continue
  fi
  if [ "${arg}" = "--operator" ]; then
    skip_next=1
    continue
  fi
  args+=("${arg}")
done
output="$("${real}" "${args[@]}" 2>&1)" || {
  status=$?
  printf '%s\n' "${output}" >&2
  if printf '%s' "${output}" | grep -qi 'already exists'; then
    exit 0
  fi
  exit "${status}"
}
printf '%s\n' "${output}"

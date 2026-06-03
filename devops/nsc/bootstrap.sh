#!/bin/sh
# Thin wrapper around scripts/a2a-nsc-bootstrap.sh for release artifacts.
# Sources operator + account env files alongside this script, then
# delegates to the canonical bootstrap.
#
# Usage:
#   devops/nsc/bootstrap.sh

set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "${HERE}/../.." && pwd)"

# Source overrides first; env vars already exported by the caller win.
[ -f "${HERE}/operator.template.env" ] && . "${HERE}/operator.template.env"
[ -f "${HERE}/account.template.env" ]  && . "${HERE}/account.template.env"

: "${A2A_OPERATOR_NAME:=${NSC_OPERATOR_NAME:-}}"
: "${A2A_NSC_DIR:=${NSC_HOME:-}}"
export A2A_OPERATOR_NAME A2A_NSC_DIR

exec "${ROOT}/scripts/a2a-nsc-bootstrap.sh" "$@"

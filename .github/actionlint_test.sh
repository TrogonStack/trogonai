#!/usr/bin/env bash
set -euo pipefail

actionlint="$(pwd)/$1"
shift

# shellcheck and pyflakes would come from whatever the host has on PATH, which
# makes the result depend on the machine, so only actionlint's own checks run.
exec "$actionlint" -shellcheck= -pyflakes= "$@"

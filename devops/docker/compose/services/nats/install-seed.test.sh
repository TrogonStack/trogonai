#!/bin/sh
set -eu

. "$(dirname "$0")/install-seed.sh"

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
seed="$workdir/callout-xkey.nk"
printf '%s\n' 'existing-seed' > "$seed"

missing_seed() {
  return 0
}

if install_seed "$seed" missing_seed; then
  echo "expected a missing seed to fail" >&2
  exit 1
fi

if [ "$(cat "$seed")" != "existing-seed" ]; then
  echo "a missing seed replaced the existing destination" >&2
  exit 1
fi

failed_seed() {
  printf '%s\n' 'partial-seed'
  return 1
}

if install_seed "$seed" failed_seed; then
  echo "expected a failed seed lookup to fail" >&2
  exit 1
fi

if [ "$(cat "$seed")" != "existing-seed" ]; then
  echo "a failed seed lookup replaced the existing destination" >&2
  exit 1
fi

replacement_seed() {
  printf '%s\n' 'replacement-seed'
}

install_seed "$seed" replacement_seed

if [ "$(cat "$seed")" != "replacement-seed" ]; then
  echo "a valid seed did not replace the destination" >&2
  exit 1
fi

if find "$workdir" -name '*.tmp.*' -print -quit | grep -q .; then
  echo "seed staging files were not cleaned up" >&2
  exit 1
fi

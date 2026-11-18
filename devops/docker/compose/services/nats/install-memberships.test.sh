#!/bin/sh
set -eu

. "$(dirname "$0")/install-memberships.sh"

membership_workdir=$(mktemp -d)
trap 'rm -rf "$membership_workdir"' EXIT
membership_registry="$membership_workdir/tenant-memberships.json"
install_tenant_memberships "$membership_registry" 'test-account' "$(id -u)"
jq -e '.version == 1 and (.memberships | length) == 1
  and .memberships[0].account == "test-account"
  and .memberships[0].identity == {type: "legacy_mtls", subject: "CN=nats-cli"}' \
  "$membership_registry" >/dev/null

chmod 0600 "$membership_registry"
printf '%s\n' '{"version":1,"memberships":[]}' > "$membership_registry"
chmod 0400 "$membership_registry"
install_tenant_memberships "$membership_registry" 'different-account' "$(id -u)"
jq -e '.memberships == []' "$membership_registry" >/dev/null

chmod 0600 "$membership_registry"
printf '%s\n' 'invalid operator update' > "$membership_registry"
chmod 0400 "$membership_registry"
install_tenant_memberships "$membership_registry" 'different-account' "$(id -u)"
if jq -e . "$membership_registry" >/dev/null 2>&1; then
  echo 'bootstrap replaced an invalid authority with a default permit' >&2
  exit 1
fi

if [ -n "$(find "$membership_workdir" -name '*.tmp.*' -print -quit)" ]; then
  echo 'membership staging files were not cleaned up' >&2
  exit 1
fi

#!/bin/sh

install_tenant_memberships() {
  membership_destination=$1
  membership_account=$2
  membership_owner=$3

  if [ -e "$membership_destination" ]; then
    return 0
  fi

  membership_staging=$(mktemp "$membership_destination.tmp.XXXXXX") || return 1
  if jq -n --arg account "$membership_account" '{
    version: 1,
    memberships: [{
      account: $account,
      identity: {type: "legacy_mtls", subject: "CN=nats-cli"}
    }]
  }' > "$membership_staging" \
    && chown "$membership_owner" "$membership_staging" \
    && chmod 0400 "$membership_staging"; then
    if ln "$membership_staging" "$membership_destination" 2>/dev/null \
      || [ -e "$membership_destination" ]; then
      rm -f "$membership_staging"
      return 0
    fi
  fi
  rm -f "$membership_staging"
  return 1
}

#!/bin/sh
# Onboards the accounts nats-bootstrap signed by pushing each account JWT to the
# full resolver as the system account, the same path a deployment uses. The
# server keeps pushed JWTs in its resolver directory, so pushing again on the
# next `up` only confirms what it already has.
set -eu

ACCOUNTS_DIR=/accounts

for jwt in "$ACCOUNTS_DIR"/*.jwt; do
  name=$(basename "$jwt" .jwt)
  reply=$(nats request '$SYS.REQ.CLAIMS.UPDATE' "$(cat "$jwt")" --raw --timeout 5s)
  if ! printf '%s' "$reply" | jq -e '.data.code == 200' >/dev/null 2>&1; then
    echo "nats-account-push: the server rejected $name: $reply" >&2
    exit 1
  fi
  echo "nats-account-push: $name $(printf '%s' "$reply" | jq -r '.data.message')"
done

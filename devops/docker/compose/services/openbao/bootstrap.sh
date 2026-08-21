#!/bin/sh
# One-shot bootstrap for the local-dev OpenBao: initialize on a fresh database,
# then hand every developer the same root token so nothing has to be copied out
# of the container logs. Re-running it against an already-initialized server is
# a no-op, which is what lets it run on every `docker compose up`.
set -eu

BAO_ADDR="${BAO_ADDR:?BAO_ADDR must be set}"
export BAO_ADDR

DEV_ROOT_TOKEN="${OPENBAO_DEV_ROOT_TOKEN:?OPENBAO_DEV_ROOT_TOKEN must be set}"
TRANSIT_MOUNT="${OPENBAO_DEV_TRANSIT_MOUNT:-transit}"
TRANSIT_KEY="${OPENBAO_DEV_TRANSIT_KEY:-local-dev-kek}"

# `bao status` exits 1 while the server is still binding its listener, 2 once it
# answers but is sealed or uninitialized, and 0 when it is unsealed.
wait_for_api() {
  i=0
  while [ "$i" -lt 60 ]; do
    if bao status >/dev/null 2>&1 || [ $? -eq 2 ]; then
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  echo "openbao-bootstrap: timed out waiting for ${BAO_ADDR}" >&2
  return 1
}

wait_for_unseal() {
  i=0
  while [ "$i" -lt 60 ]; do
    if bao status >/dev/null 2>&1; then
      return 0
    fi
    i=$((i + 1))
    sleep 1
  done
  echo "openbao-bootstrap: timed out waiting for auto-unseal" >&2
  return 1
}

wait_for_api

if bao status -format=json 2>/dev/null | grep -q '"initialized": true'; then
  echo "openbao-bootstrap: already initialized"
  wait_for_unseal
  exit 0
fi

echo "openbao-bootstrap: initializing"
# The static seal stores the root key itself, so init hands back recovery keys
# rather than unseal shares. One share is plenty for a throwaway dev database.
init_output=$(bao operator init -recovery-shares=1 -recovery-threshold=1)
initial_root_token=$(echo "$init_output" | awk '/^Initial Root Token:/ { print $4 }')

if [ -z "$initial_root_token" ]; then
  echo "openbao-bootstrap: could not read the initial root token from init output" >&2
  exit 1
fi

wait_for_unseal

BAO_TOKEN="$initial_root_token"
export BAO_TOKEN

# A fixed token id keeps BAO_TOKEN stable across `down -v` cycles, so shell
# history and scratch scripts keep working. Only a root-authorized caller may
# choose a token id, which is why this runs before the initial token is dropped.
bao token create -id="$DEV_ROOT_TOKEN" -policy=root -orphan -display-name=local-dev >/dev/null

bao secrets enable -path="$TRANSIT_MOUNT" transit >/dev/null
bao write -f "${TRANSIT_MOUNT}/keys/${TRANSIT_KEY}" type=aes256-gcm96 >/dev/null

# The initial root token has served its purpose; the fixed dev token replaces it.
bao token revoke -self >/dev/null

echo "openbao-bootstrap: ready (token: ${DEV_ROOT_TOKEN}, transit key: ${TRANSIT_MOUNT}/${TRANSIT_KEY})"

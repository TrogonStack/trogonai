#!/bin/sh
# One-shot bootstrap for the local-dev OpenBao: initialize on a fresh database,
# then hand every developer the same root token so nothing has to be copied out
# of the container logs.
#
# Everything after `bao operator init` runs under the fixed dev token rather than
# the initial root token, and every provisioning step reconciles rather than
# assumes. A run killed partway therefore finishes on the next `up`, and a run
# against a fully provisioned server changes nothing.
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

# Each step is skipped only when the thing it creates is already there, so this
# doubles as the repair path for an interrupted first run.
provision() {
  if ! bao secrets list -format=json | grep -q "\"${TRANSIT_MOUNT}/\""; then
    bao secrets enable -path="$TRANSIT_MOUNT" transit >/dev/null
  fi

  # Registration rejects a transit mount that leaves upsert enabled, and OpenBao
  # defaults it to off, so the local mount is shaped like the one a customer
  # deployment must present (ADR#0030 Decision 5).
  bao write "${TRANSIT_MOUNT}/config/keys" disable_upsert=true >/dev/null

  if ! bao read "${TRANSIT_MOUNT}/keys/${TRANSIT_KEY}" >/dev/null 2>&1; then
    bao write -f "${TRANSIT_MOUNT}/keys/${TRANSIT_KEY}" type=aes256-gcm96 >/dev/null
  fi
}

wait_for_api

if bao status -format=json 2>/dev/null | grep -q '"initialized": true'; then
  wait_for_unseal

  BAO_TOKEN="$DEV_ROOT_TOKEN"
  export BAO_TOKEN

  # Storage says initialized but the dev token is gone, so there is no root
  # credential left to provision with. Say so instead of exiting 0 and leaving
  # the server looking ready.
  if ! bao token lookup >/dev/null 2>&1; then
    echo "openbao-bootstrap: ${BAO_ADDR} is initialized but the dev root token is missing or invalid." >&2
    echo "openbao-bootstrap: no credential remains to repair it; reset with 'docker compose down -v'." >&2
    exit 1
  fi

  provision
  echo "openbao-bootstrap: ready (token: ${DEV_ROOT_TOKEN}, transit key: ${TRANSIT_MOUNT}/${TRANSIT_KEY})"
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
# choose a token id, which is why this runs while the initial token is still
# held. It is also the one command with no retry: the initial token only ever
# exists in this process, so a crash before the orphan lands is unrecoverable.
bao token create -id="$DEV_ROOT_TOKEN" -policy=root -orphan -display-name=local-dev >/dev/null

BAO_TOKEN="$DEV_ROOT_TOKEN"
export BAO_TOKEN

provision

# The initial root token has served its purpose; the fixed dev token replaces it.
bao token revoke "$initial_root_token" >/dev/null

echo "openbao-bootstrap: ready (token: ${DEV_ROOT_TOKEN}, transit key: ${TRANSIT_MOUNT}/${TRANSIT_KEY})"

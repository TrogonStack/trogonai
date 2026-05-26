#!/usr/bin/env bash
set -euo pipefail

BOOTSTRAP_DIR="${A2A_BOOTSTRAP_DIR:-/run/a2a-bootstrap}"
MARKER="${BOOTSTRAP_DIR}/.spicedb-seed-complete"
ENDPOINT="${A2A_SPICEDB_SEED_ENDPOINT:-spicedb:50051}"
TOKEN="${A2A_SPICEDB_SEED_TOKEN:-devkey}"

mkdir -p "${BOOTSTRAP_DIR}"

SPICEDB_HOST="${ENDPOINT#*://}"
SPICEDB_HOST="${SPICEDB_HOST%%:*}"
SPICEDB_PORT="${ENDPOINT##*:}"

for attempt in $(seq 1 30); do
  if timeout 1 bash -c "echo >/dev/tcp/${SPICEDB_HOST}/${SPICEDB_PORT}" 2>/dev/null; then
    break
  fi
  if [[ "${attempt}" -eq 30 ]]; then
    echo "[a2a-spicedb-seed] SpiceDB not reachable at ${ENDPOINT}" >&2
    exit 1
  fi
  sleep 2
done

zed schema write /opt/a2a/spicedb/schema.zed \
  --endpoint "${ENDPOINT}" \
  --token "${TOKEN}" \
  --insecure

zed import --schema=false /opt/a2a/spicedb/relationships.yaml \
  --endpoint "${ENDPOINT}" \
  --token "${TOKEN}" \
  --insecure

touch "${MARKER}"
echo "[a2a-spicedb-seed] complete -> ${MARKER}"

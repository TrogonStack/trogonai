#!/usr/bin/env bash
set -euo pipefail

BOOTSTRAP_DIR="${A2A_BOOTSTRAP_DIR:-/run/a2a-bootstrap}"
MARKER="${BOOTSTRAP_DIR}/.bootstrap-complete"
REPO_SCRIPTS="/opt/a2a/scripts"
SMOKE_NATS_CONF="/opt/a2a/a2a-nats-server-smoke.conf"

if [[ -f "${MARKER}" ]]; then
  echo "[a2a-bootstrap] already complete (${MARKER})"
  exit 0
fi

mkdir -p "${BOOTSTRAP_DIR}/jwt" "${BOOTSTRAP_DIR}/nsc/config" "${BOOTSTRAP_DIR}/nsc/data" "${BOOTSTRAP_DIR}/nsc/keys"

export A2A_BOOTSTRAP_DIR="${BOOTSTRAP_DIR}"
export A2A_OPERATOR_NAME="${A2A_OPERATOR_NAME:-smoke-op}"
export A2A_ACCOUNT_NAME="${A2A_ACCOUNT_NAME:-SMOKE}"
export A2A_APP_ACCOUNT_NAME="${A2A_APP_ACCOUNT_NAME:-SMOKE}"
export A2A_NSC_DIR="${BOOTSTRAP_DIR}/nsc"
export A2A_PREFIX="${A2A_PREFIX:-a2a}"
export A2A_NATS_URL_HOST="${A2A_NATS_URL_HOST:-nats:4222}"
export A2A_AUTH_CALLOUT_USER="${A2A_AUTH_CALLOUT_USER:-auth}"
export A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS="${A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS:-SMOKE}"
export A2A_AUTH_CALLOUT_KEYS_DIR="${BOOTSTRAP_DIR}/auth-callout-keys"
export A2A_AUTH_CALLOUT_OUTPUT_FILE="${BOOTSTRAP_DIR}/auth-callout.env"
export A2A_PUSH_JWT=0

NSC_FLAGS="--config-dir ${A2A_NSC_DIR}/config --data-dir ${A2A_NSC_DIR}/data --keystore-dir ${A2A_NSC_DIR}/keys"

if ! nsc describe operator ${NSC_FLAGS} -n "${A2A_OPERATOR_NAME}" >/dev/null 2>&1; then
  echo "[a2a-bootstrap] creating operator ${A2A_OPERATOR_NAME}"
  nsc add operator ${NSC_FLAGS} -n "${A2A_OPERATOR_NAME}"
else
  echo "[a2a-bootstrap] reusing operator ${A2A_OPERATOR_NAME}"
fi

nsc env ${NSC_FLAGS} --operator "${A2A_OPERATOR_NAME}"

bash "${REPO_SCRIPTS}/a2a-nsc-bootstrap.sh"
bash "${REPO_SCRIPTS}/a2a-auth-callout-bootstrap.sh"

KEYS_DIR="${A2A_AUTH_CALLOUT_KEYS_DIR}"
ISSUER_PUBLIC="$(tr -d '\n' < "${KEYS_DIR}/issuer.public")"
AUTH_PASSWORD="$(tr -d '\n' < "${KEYS_DIR}/auth-callout-user.password")"

cp "${KEYS_DIR}/signing-key.current" "${BOOTSTRAP_DIR}/signing-key.current"
cp "${KEYS_DIR}/signing-key.current" "${BOOTSTRAP_DIR}/signing-key.previous"
chmod 600 "${BOOTSTRAP_DIR}/signing-key.current" "${BOOTSTRAP_DIR}/signing-key.previous"

sed -e "s|<replace-with-nsc-bootstrap-issuer-public>|${ISSUER_PUBLIC}|g" \
  -e "s|<replace-with-callout-auth-password>|${AUTH_PASSWORD}|g" \
  "${SMOKE_NATS_CONF}" \
  > "${BOOTSTRAP_DIR}/nats-server.conf"

NATS_BASE_URL="nats://nats:4222"

patch_nats_env() {
  local file="$1"
  grep -v -E '^(NATS_URL|NATS_USER|NATS_PASSWORD)=' "${file}" > "${file}.tmp" || true
  {
    cat "${file}.tmp"
    echo "NATS_URL=${NATS_BASE_URL}"
    echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
    echo "NATS_PASSWORD=${AUTH_PASSWORD}"
  } > "${file}"
  rm -f "${file}.tmp"
}

nsc generate creds ${NSC_FLAGS} -a "${A2A_ACCOUNT_NAME}" -n a2a-gateway \
  > "${BOOTSTRAP_DIR}/gateway.creds" || {
  echo "[a2a-bootstrap] WARN: nsc generate creds for a2a-gateway failed; services may need manual creds"
}

nsc add user ${NSC_FLAGS} -a "${A2A_ACCOUNT_NAME}" -n a2a-smoke-agent \
  --allow-pub ">" \
  --allow-sub ">" 2>/dev/null || true

nsc generate creds ${NSC_FLAGS} --operator "${A2A_OPERATOR_NAME}" -a "${A2A_ACCOUNT_NAME}" -n a2a-smoke-agent \
  > "${BOOTSTRAP_DIR}/agent.creds" || {
  echo "[a2a-bootstrap] WARN: nsc generate creds for a2a-smoke-agent failed"
}

a2a-smoke-test mint-jwt \
  --signing-key-path "${BOOTSTRAP_DIR}/signing-key.current" \
  --caller-id "${A2A_SMOKE_CALLER_ID:-smoke-caller-1}" \
  --audience "${A2A_ACCOUNT_NAME}" \
  --ttl-secs "${A2A_SMOKE_JWT_TTL_SECS:-86400}" \
  --out "${BOOTSTRAP_DIR}/nats-server-caller.jwt" \
  --creds-out "${BOOTSTRAP_DIR}/smoke-caller.creds"

{
  echo "NATS_URL=${NATS_BASE_URL}"
  echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
  echo "NATS_PASSWORD=${AUTH_PASSWORD}"
  echo "A2A_PREFIX=${A2A_PREFIX}"
  echo "A2A_GATEWAY_TRUST_CALLER_HEADERS=0"
  echo "A2A_GATEWAY_TIER1_SPICEDB_ENABLED=0"
  echo "A2A_GATEWAY_TIER3_REDACTION_ENABLED=0"
  echo "A2A_GATEWAY_AUDIT_PUBLISH=1"
  echo "A2A_GATEWAY_JWT_AUDIENCE=${A2A_ACCOUNT_NAME}"
  echo "AUTH_CALLOUT_SIGNING_KEY_SOURCE=file"
  echo "AUTH_CALLOUT_SIGNING_KEY_PATH=${BOOTSTRAP_DIR}/signing-key.current"
  echo "AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH=${BOOTSTRAP_DIR}/signing-key.previous"
} > "${BOOTSTRAP_DIR}/gateway.env"

{
  echo "NATS_URL=${NATS_BASE_URL}"
  echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
  echo "NATS_PASSWORD=${AUTH_PASSWORD}"
  echo "A2A_BRIDGE_TRANSPORT=nats"
  echo "BRIDGE_LISTEN_ADDR=0.0.0.0:7443"
  echo "BRIDGE_TENANT_ACCOUNT=${A2A_ACCOUNT_NAME}"
} > "${BOOTSTRAP_DIR}/bridge.env"

{
  echo "NATS_URL=${NATS_BASE_URL}"
  echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
  echo "NATS_PASSWORD=${AUTH_PASSWORD}"
  echo "A2A_AGENT_ID=${A2A_SMOKE_AGENT_ID:-echo}"
  echo "A2A_PREFIX=${A2A_PREFIX}"
} > "${BOOTSTRAP_DIR}/agent.env"

CALLER_JWT="$(tr -d '\n' < "${BOOTSTRAP_DIR}/nats-server-caller.jwt")"
{
  echo "NATS_URL=${NATS_BASE_URL}"
  echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
  echo "NATS_PASSWORD=${AUTH_PASSWORD}"
  echo "A2A_AGENT_ID=${A2A_SMOKE_AGENT_ID:-echo}"
  echo "A2A_PREFIX=${A2A_PREFIX}"
  echo "A2A_USE_GATEWAY=1"
  echo "A2A_GATEWAY_CALLER_JWT=${CALLER_JWT}"
  echo "A2A_HTTP_BIND=0.0.0.0:8080"
} > "${BOOTSTRAP_DIR}/nats-server.env"

sed "s|AUTH_CALLOUT_SIGNING_KEY_PATH=.*|AUTH_CALLOUT_SIGNING_KEY_PATH=${BOOTSTRAP_DIR}/signing-key.current|" \
  "${BOOTSTRAP_DIR}/auth-callout.env" > "${BOOTSTRAP_DIR}/auth-callout.env.tmp"
mv "${BOOTSTRAP_DIR}/auth-callout.env.tmp" "${BOOTSTRAP_DIR}/auth-callout.env"
echo "AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH=${BOOTSTRAP_DIR}/signing-key.previous" >> "${BOOTSTRAP_DIR}/auth-callout.env"
echo "AUTH_CALLOUT_READY_FILE=${BOOTSTRAP_DIR}/auth-callout-ready" >> "${BOOTSTRAP_DIR}/auth-callout.env"
patch_nats_env "${BOOTSTRAP_DIR}/auth-callout.env"

touch "${MARKER}"
echo "[a2a-bootstrap] complete -> ${MARKER}"

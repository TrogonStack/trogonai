#!/usr/bin/env bash
set -euo pipefail

BOOTSTRAP_DIR="${A2A_BOOTSTRAP_DIR:-/run/a2a-bootstrap}"
MARKER="${BOOTSTRAP_DIR}/.bootstrap-complete"
REPO_SCRIPTS="/opt/a2a/scripts"
SMOKE_NATS_CONF="/opt/a2a/a2a-nats-server-smoke.conf"

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
COMPOSE_PROFILE="${A2A_COMPOSE_PROFILE:-smoke}"
NATS_BASE_URL="nats://nats:4222"

write_full_gateway_env() {
  local tier1_dir="$1"
  local tier3_dir="$2"
  local tier3_pubkey="$3"
  local discovery_pubkey="$4"
  {
    echo "NATS_URL=${NATS_BASE_URL}"
    echo "NATS_USER=${A2A_AUTH_CALLOUT_USER}"
    echo "NATS_PASSWORD=${AUTH_PASSWORD}"
    echo "A2A_PREFIX=${A2A_PREFIX}"
    echo "A2A_GATEWAY_TRUST_CALLER_HEADERS=0"
    echo "A2A_GATEWAY_TIER1_SPICEDB_ENABLED=on"
    echo "A2A_GATEWAY_TIER1_SPICEDB_ENDPOINT=http://spicedb:50051"
    echo "A2A_GATEWAY_TIER1_SPICEDB_TOKEN=devkey"
    echo "A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED=on"
    echo "A2A_GATEWAY_TIER1_BUNDLE_DIR=${tier1_dir}"
    echo "A2A_GATEWAY_TIER3_REDACTION_ENABLED=on"
    echo "A2A_GATEWAY_POLICY_BUNDLE_DIR=${tier3_dir}"
    echo "A2A_GATEWAY_POLICY_SKILLS=pii-regex-redactor,smoke-tier3-refuse"
    echo "A2A_GATEWAY_TIER3_SIGNING_PUBKEY=${tier3_pubkey}"
    echo "A2A_DISCOVERY_OPERATOR_KEYS=${A2A_OPERATOR_NAME}:${discovery_pubkey}"
    echo "A2A_SMOKE_DISCOVERY_OPERATOR_SEED_PATH=${BOOTSTRAP_DIR}/discovery-operator-seed.hex"
    echo "A2A_GATEWAY_AUDIT_PUBLISH=1"
    echo "A2A_GATEWAY_JWT_AUDIENCE=${A2A_ACCOUNT_NAME}"
    echo "AUTH_CALLOUT_SIGNING_KEY_SOURCE=file"
    echo "AUTH_CALLOUT_SIGNING_KEY_PATH=${BOOTSTRAP_DIR}/signing-key.current"
    echo "AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH=${BOOTSTRAP_DIR}/signing-key.previous"
  } > "${BOOTSTRAP_DIR}/gateway.env"
}

bootstrap_full_policy_bundles() {
  local tier1_dir="${BOOTSTRAP_DIR}/tier1-bundles"
  local tier3_dir="${BOOTSTRAP_DIR}/tier3-bundles"
  mkdir -p "${tier1_dir}" "${tier3_dir}"
  cp /opt/a2a/policies/per-method-allowlist.tier1.toml "${tier1_dir}/"

  stage_tier3_skill() {
    local slug="$1"
    local wasm_src="$2"
    cp "${wasm_src}" "${tier3_dir}/${slug}.wasm"
    cp "/opt/a2a/tier3-manifests/${slug}.manifest.json" "${tier3_dir}/${slug}.manifest.json"
  }
  stage_tier3_skill "pii-regex-redactor" "/opt/a2a/tier3-staging/pii_regex_redactor.wasm"
  stage_tier3_skill "smoke-tier3-refuse" "/opt/a2a/tier3-staging/smoke_tier3_refuse.wasm"

  local tier3_signing_seed discovery_signing_seed
  tier3_signing_seed="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  a2a-sign-bundle --key "${tier3_signing_seed}" --skill-dir "${tier3_dir}"
  local tier3_signing_pubkey
  tier3_signing_pubkey="$(a2a-smoke-test ed25519-pubkey --seed-hex "${tier3_signing_seed}")"

  discovery_signing_seed="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
  local discovery_operator_pubkey
  discovery_operator_pubkey="$(a2a-smoke-test ed25519-pubkey --seed-hex "${discovery_signing_seed}")"
  printf '%s' "${discovery_signing_seed}" > "${BOOTSTRAP_DIR}/discovery-operator-seed.hex"
  chmod 600 "${BOOTSTRAP_DIR}/discovery-operator-seed.hex"

  write_full_gateway_env "${tier1_dir}" "${tier3_dir}" "${tier3_signing_pubkey}" "${discovery_operator_pubkey}"
  echo "[a2a-bootstrap] full policy bundles refreshed"
}

if [[ -f "${MARKER}" && "${COMPOSE_PROFILE}" == "full" ]]; then
  AUTH_PASSWORD="$(grep -E '^NATS_PASSWORD=' "${BOOTSTRAP_DIR}/auth-callout.env" | head -1 | cut -d= -f2-)"
  bootstrap_full_policy_bundles
  exit 0
fi

if [[ -f "${MARKER}" ]]; then
  echo "[a2a-bootstrap] already complete (${MARKER})"
  exit 0
fi

mkdir -p "${BOOTSTRAP_DIR}/jwt" "${BOOTSTRAP_DIR}/nsc/config" "${BOOTSTRAP_DIR}/nsc/data" "${BOOTSTRAP_DIR}/nsc/keys"

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

if [[ "${COMPOSE_PROFILE}" == "full" ]]; then
  bootstrap_full_policy_bundles
else
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
fi

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

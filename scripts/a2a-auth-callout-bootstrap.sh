#!/bin/sh
# a2a-auth-callout-bootstrap.sh — NSC keys + AUTH/APP/SYS accounts for auth callout
#
# Provisions fleet-level NATS accounts (AUTH / APP / SYS), generates auth-callout
# NKey / XKey material, and emits a copy-paste env block for a2a-auth-callout.
# Centralized nats-server layout: scripts/a2a-auth-callout-nats-server.conf
#
# Tenant subject ACLs remain scripts/a2a-nsc-bootstrap.sh (per-Account gateway/caller/registrar).
#
# Required env vars:
#   A2A_OPERATOR_NAME   — nsc operator name
#   A2A_NSC_DIR           — nsc config/data/keystore root (--config-dir, --data-dir, --keystore-dir)
#
# Optional env vars:
#   A2A_AUTH_ACCOUNT_NAME          — callout host account (default: AUTH)
#   A2A_APP_ACCOUNT_NAME           — application account name (default: APP)
#   A2A_SYS_ACCOUNT_NAME           — system account name (default: SYS)
#   A2A_AUTH_CALLOUT_USER          — callout NATS username for centralized config (default: auth)
#   A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS — comma-separated tenant Accounts for minting (default: APP)
#   A2A_AUTH_CALLOUT_KEYS_DIR      — generated key dir (default: ${A2A_NSC_DIR}/auth-callout-keys)
#   A2A_AUTH_CALLOUT_OUTPUT_FILE   — write env block here (default: ${KEYS_DIR}/auth-callout.env)
#   A2A_AUTH_CALLOUT_ENABLE_XKEY   — set to 1 to generate account XKey (default: 0)
#   A2A_PUSH_JWT                   — push Account JWTs after account steps (default: 1; 0 to skip)
#   A2A_NATS_URL_HOST              — host in emitted NATS_URL (default: 127.0.0.1:4222)
#
# Flags:
#   --enable-xkey   Same as A2A_AUTH_CALLOUT_ENABLE_XKEY=1
#
# Usage:
#   A2A_OPERATOR_NAME=myop A2A_NSC_DIR=/home/nsc ./scripts/a2a-auth-callout-bootstrap.sh
#
# nsc flag references:
#   nsc generate nkey : https://nats-io.github.io/nsc/nsc_generate_nkey.html
#   nsc add account   : https://nats-io.github.io/nsc/nsc_add_account.html
#   nsc describe account : https://nats-io.github.io/nsc/nsc_describe_account.html
#   nsc push          : https://docs.nats.io/using-nats/nats-tools/nsc/basics

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() {
    printf '[a2a-auth-callout-bootstrap] %s\n' "$*" >&2
}

die() {
    printf '[a2a-auth-callout-bootstrap] ERROR: %s\n' "$*" >&2
    exit 1
}

run_nsc() {
    printf '[a2a-auth-callout-bootstrap] + nsc %s\n' "$*" >&2
    nsc "$@"
}

# ---------------------------------------------------------------------------
# CLI / env
# ---------------------------------------------------------------------------

parse_cli_flags() {
    for arg in "$@"; do
        case "${arg}" in
            --enable-xkey) A2A_AUTH_CALLOUT_ENABLE_XKEY=1 ;;
            *) die "Unknown flag: ${arg}. Supported: --enable-xkey" ;;
        esac
    done
}

parse_cli_flags "$@"

: "${A2A_OPERATOR_NAME:?A2A_OPERATOR_NAME is required}"
: "${A2A_NSC_DIR:?A2A_NSC_DIR is required}"

A2A_AUTH_ACCOUNT_NAME="${A2A_AUTH_ACCOUNT_NAME:-AUTH}"
A2A_APP_ACCOUNT_NAME="${A2A_APP_ACCOUNT_NAME:-APP}"
A2A_SYS_ACCOUNT_NAME="${A2A_SYS_ACCOUNT_NAME:-SYS}"
A2A_AUTH_CALLOUT_USER="${A2A_AUTH_CALLOUT_USER:-auth}"
A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS="${A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS:-${A2A_APP_ACCOUNT_NAME}}"
A2A_AUTH_CALLOUT_ENABLE_XKEY="${A2A_AUTH_CALLOUT_ENABLE_XKEY:-0}"
A2A_PUSH_JWT="${A2A_PUSH_JWT:-1}"
A2A_NATS_URL_HOST="${A2A_NATS_URL_HOST:-127.0.0.1:4222}"

KEYS_DIR="${A2A_AUTH_CALLOUT_KEYS_DIR:-${A2A_NSC_DIR}/auth-callout-keys}"
OUTPUT_FILE="${A2A_AUTH_CALLOUT_OUTPUT_FILE:-${KEYS_DIR}/auth-callout.env}"

NSC_CONFIG_DIR="${A2A_NSC_DIR}/config"
NSC_DATA_DIR="${A2A_NSC_DIR}/data"
NSC_KEYSTORE_DIR="${A2A_NSC_DIR}/keys"
NSC_DIR_FLAGS="--config-dir ${NSC_CONFIG_DIR} --data-dir ${NSC_DATA_DIR} --keystore-dir ${NSC_KEYSTORE_DIR}"

ISSUER_SEED_FILE="${KEYS_DIR}/issuer.seed"
ISSUER_PUBLIC_FILE="${KEYS_DIR}/issuer.public"
XKEY_SEED_FILE="${KEYS_DIR}/account.xkey.seed"
XKEY_PUBLIC_FILE="${KEYS_DIR}/account.xkey.public"
SIGNING_KEY_FILE="${KEYS_DIR}/signing-key.current"
AUTH_PASSWORD_FILE="${KEYS_DIR}/auth-callout-user.password"
JWT_EXPORT_DIR="${KEYS_DIR}/account-jwts"

# ---------------------------------------------------------------------------
# Key generation (idempotent: reuse files under KEYS_DIR)
# ---------------------------------------------------------------------------

require_nsc() {
    if ! command -v nsc >/dev/null 2>&1; then
        die "nsc not found in PATH. Install nsc before running this script."
    fi
}

load_or_generate_account_issuer() {
    if [ -f "${ISSUER_SEED_FILE}" ] && [ -f "${ISSUER_PUBLIC_FILE}" ]; then
        log "Reusing existing issuer NKey in ${KEYS_DIR}"
        ISSUER_SEED=$(cat "${ISSUER_SEED_FILE}")
        ISSUER_PUBLIC=$(cat "${ISSUER_PUBLIC_FILE}")
        return
    fi
    log "Generating auth_callout issuer NKey (nsc generate nkey --account)"
    _out=$(run_nsc generate nkey --account)
    ISSUER_SEED=$(printf '%s\n' "${_out}" | sed -n '1p')
    ISSUER_PUBLIC=$(printf '%s\n' "${_out}" | sed -n '2p')
    if [ -z "${ISSUER_SEED}" ] || [ -z "${ISSUER_PUBLIC}" ]; then
        die "unexpected nsc generate nkey --account output (expected seed + public on two lines)"
    fi
    umask 077
    mkdir -p "${KEYS_DIR}"
    printf '%s\n' "${ISSUER_SEED}" >"${ISSUER_SEED_FILE}"
    printf '%s\n' "${ISSUER_PUBLIC}" >"${ISSUER_PUBLIC_FILE}"
    chmod 600 "${ISSUER_SEED_FILE}" "${ISSUER_PUBLIC_FILE}"
}

load_or_generate_account_xkey() {
    if [ "${A2A_AUTH_CALLOUT_ENABLE_XKEY}" != "1" ]; then
        ACCOUNT_XKEY_SEED=""
        ACCOUNT_XKEY_PUBLIC=""
        return
    fi
    if [ -f "${XKEY_SEED_FILE}" ] && [ -f "${XKEY_PUBLIC_FILE}" ]; then
        log "Reusing existing account XKey in ${KEYS_DIR}"
        ACCOUNT_XKEY_SEED=$(cat "${XKEY_SEED_FILE}")
        ACCOUNT_XKEY_PUBLIC=$(cat "${XKEY_PUBLIC_FILE}")
        return
    fi
    log "Generating auth_callout account XKey (nsc generate nkey --curve)"
    _out=$(run_nsc generate nkey --curve)
    ACCOUNT_XKEY_SEED=$(printf '%s\n' "${_out}" | sed -n '1p')
    ACCOUNT_XKEY_PUBLIC=$(printf '%s\n' "${_out}" | sed -n '2p')
    if [ -z "${ACCOUNT_XKEY_SEED}" ] || [ -z "${ACCOUNT_XKEY_PUBLIC}" ]; then
        die "unexpected nsc generate nkey --curve output (expected seed + public on two lines)"
    fi
    umask 077
    printf '%s\n' "${ACCOUNT_XKEY_SEED}" >"${XKEY_SEED_FILE}"
    printf '%s\n' "${ACCOUNT_XKEY_PUBLIC}" >"${XKEY_PUBLIC_FILE}"
    chmod 600 "${XKEY_SEED_FILE}" "${XKEY_PUBLIC_FILE}"
}

ensure_signing_key_file() {
    if [ -f "${SIGNING_KEY_FILE}" ]; then
        log "Reusing signing key file ${SIGNING_KEY_FILE}"
        return
    fi
    umask 077
    printf '%s\n' "${ISSUER_SEED}" >"${SIGNING_KEY_FILE}"
    chmod 600 "${SIGNING_KEY_FILE}"
    log "Wrote Account NKey seed for inner User JWT signing to ${SIGNING_KEY_FILE}"
}

ensure_auth_callout_password() {
    if [ -f "${AUTH_PASSWORD_FILE}" ]; then
        AUTH_CALLOUT_PASSWORD=$(cat "${AUTH_PASSWORD_FILE}")
        return
    fi
    umask 077
    if command -v openssl >/dev/null 2>&1; then
        AUTH_CALLOUT_PASSWORD=$(openssl rand -hex 16)
    else
        AUTH_CALLOUT_PASSWORD=$(dd if=/dev/urandom bs=16 count=1 2>/dev/null | od -An -tx1 | tr -d ' \n')
    fi
    printf '%s\n' "${AUTH_CALLOUT_PASSWORD}" >"${AUTH_PASSWORD_FILE}"
    chmod 600 "${AUTH_PASSWORD_FILE}"
}

ensure_nsc_account() {
    _acct_name="${1:?}"
    log "Ensuring Account '${_acct_name}' exists (idempotent)"
    run_nsc add account ${NSC_DIR_FLAGS} \
        --operator "${A2A_OPERATOR_NAME}" \
        -n "${_acct_name}"
}

export_account_jwt() {
    _acct_name="${1:?}"
    _dest="${JWT_EXPORT_DIR}/${_acct_name}.jwt"
    mkdir -p "${JWT_EXPORT_DIR}"
    run_nsc describe account ${NSC_DIR_FLAGS} \
        --operator "${A2A_OPERATOR_NAME}" \
        -n "${_acct_name}" \
        -R \
        -o "${_dest}"
    log "  Account JWT: ${_dest}"
}

push_accounts() {
    if [ "${A2A_PUSH_JWT}" = "0" ]; then
        log "Skipping JWT push (A2A_PUSH_JWT=0)"
        return
    fi
    for _acct in "${A2A_AUTH_ACCOUNT_NAME}" "${A2A_APP_ACCOUNT_NAME}" "${A2A_SYS_ACCOUNT_NAME}"; do
        log "Pushing Account JWT for '${_acct}'"
        run_nsc push ${NSC_DIR_FLAGS} \
            --operator "${A2A_OPERATOR_NAME}" \
            -a "${_acct}"
    done
}

emit_env_block() {
    _target="${1:?}"
    {
        printf '# SENSITIVE — move to your secret store; do not commit.\n'
        printf '# Generated by scripts/a2a-auth-callout-bootstrap.sh\n\n'
        printf 'NATS_URL=nats://%s:%s@%s\n' \
            "${A2A_AUTH_CALLOUT_USER}" "${AUTH_CALLOUT_PASSWORD}" "${A2A_NATS_URL_HOST}"
        printf 'AUTH_CALLOUT_SERVER_NKEY_PUBLIC=%s\n' "${ISSUER_PUBLIC}"
        printf 'AUTH_CALLOUT_ISSUER_NKEY_SEED=%s\n' "${ISSUER_SEED}"
        if [ "${A2A_AUTH_CALLOUT_ENABLE_XKEY}" = "1" ]; then
            printf 'AUTH_CALLOUT_XKEY_SEED=%s\n' "${ACCOUNT_XKEY_SEED}"
            printf '# Set after nats-server is running with auth_callout.xkey (see runbook):\n'
            printf '# AUTH_CALLOUT_SERVER_XKEY_PUBLIC=<from-server-varz-or-Nats-Server-Xkey-header>\n'
        fi
        printf 'AUTH_CALLOUT_ALLOWED_ACCOUNTS=%s\n' "${A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS}"
        printf 'AUTH_CALLOUT_SIGNING_KEY_SOURCE=file\n'
        printf 'AUTH_CALLOUT_SIGNING_KEY_PATH=%s\n' "${SIGNING_KEY_FILE}"
        printf 'AUTH_CALLOUT_USER_JWT_TTL_SECS=300\n'
        printf '# AUTH_CALLOUT_OIDC_ISSUER=\n'
        printf '# AUTH_CALLOUT_OIDC_AUDIENCES=\n'
        printf '# AUTH_CALLOUT_MTLS_TRUST_ANCHORS=\n'
    } >>"${_target}"
}

emit_nats_server_substitution_hints() {
    log "nats-server.conf substitutions (scripts/a2a-auth-callout-nats-server.conf):"
    log "  authorization.auth_callout.issuer = ${ISSUER_PUBLIC}"
    if [ "${A2A_AUTH_CALLOUT_ENABLE_XKEY}" = "1" ]; then
        log "  authorization.auth_callout.xkey   = ${ACCOUNT_XKEY_PUBLIC}"
    fi
    log "  AUTH account user password        = (see ${AUTH_PASSWORD_FILE})"
}

emit_next_steps() {
    log ""
    log "Next steps:"
    log "  1. Substitute placeholders in scripts/a2a-auth-callout-nats-server.conf"
    log "     (issuer, optional xkey, auth user password from ${AUTH_PASSWORD_FILE})."
    log "  2. Start nats-server (2.14.x):"
    log "       nats-server -c scripts/a2a-auth-callout-nats-server.conf"
    if [ "${A2A_AUTH_CALLOUT_ENABLE_XKEY}" = "1" ]; then
        log "     Then set AUTH_CALLOUT_SERVER_XKEY_PUBLIC from server /varz Key or the"
        log "     Nats-Server-Xkey header on the first encrypted auth request."
    fi
    log "  3. Install ${OUTPUT_FILE} into your secret store (systemd EnvironmentFile,"
    log "     compose env_file, etc.)."
    log "  4. Run the callout binary:"
    log "       cargo build --release -p a2a-auth-callout --manifest-path rsworkspace/Cargo.toml"
    log "       # or: systemctl start a2a-auth-callout  (scripts/a2a-auth-callout.service)"
    log "  5. Provision tenant ACLs: ./scripts/a2a-nsc-bootstrap.sh (per tenant Account)."
    log "  Reference: docs/A2A_AUTH_CALLOUT_DEPLOYMENT.md"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

require_nsc

log "Starting A2A auth callout bootstrap"
log "  Operator    : ${A2A_OPERATOR_NAME}"
log "  NSC dir     : ${A2A_NSC_DIR}"
log "  Keys dir    : ${KEYS_DIR}"
log "  Accounts    : AUTH=${A2A_AUTH_ACCOUNT_NAME} APP=${A2A_APP_ACCOUNT_NAME} SYS=${A2A_SYS_ACCOUNT_NAME}"
log "  Mint allow  : ${A2A_AUTH_CALLOUT_ALLOWED_ACCOUNTS}"
log "  XKey        : ${A2A_AUTH_CALLOUT_ENABLE_XKEY}"

log "Step 1: selecting operator context"
run_nsc env ${NSC_DIR_FLAGS} --operator "${A2A_OPERATOR_NAME}"

log "Step 2: ensuring AUTH / APP / SYS Accounts exist"
ensure_nsc_account "${A2A_AUTH_ACCOUNT_NAME}"
ensure_nsc_account "${A2A_APP_ACCOUNT_NAME}"
ensure_nsc_account "${A2A_SYS_ACCOUNT_NAME}"

log "Step 3: generating or loading callout NKeys"
load_or_generate_account_issuer
load_or_generate_account_xkey
ensure_signing_key_file
ensure_auth_callout_password

log "Step 4: exporting Account JWTs for operator resolver deployments"
export_account_jwt "${A2A_AUTH_ACCOUNT_NAME}"
export_account_jwt "${A2A_APP_ACCOUNT_NAME}"
export_account_jwt "${A2A_SYS_ACCOUNT_NAME}"

push_accounts

umask 077
mkdir -p "${KEYS_DIR}"
: >"${OUTPUT_FILE}"
emit_env_block "${OUTPUT_FILE}"
chmod 600 "${OUTPUT_FILE}"

emit_nats_server_substitution_hints
emit_next_steps

log ""
log "=== copy-paste env block (${OUTPUT_FILE}) ==="
cat "${OUTPUT_FILE}"
log "=== end env block ==="
log "Bootstrap complete. Keys and JWT exports live under ${KEYS_DIR}."

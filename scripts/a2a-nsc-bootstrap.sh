#!/bin/sh
# a2a-nsc-bootstrap.sh — idempotent NSC account bootstrap for one A2A tenant
#
# Provisions one NATS Account and three service Users (caller-template, gateway,
# registrar) with the Phase 0 subject ACLs defined in:
#   docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md
#   docs/A2A_SUBJECT_ACL_QUICKREF.md
#
# ACL subjects are declared in scripts/acl-templates/{caller,gateway,registrar}.acl
# but the nsc flags below are derived directly from those files so the script is
# self-contained and auditable.
#
# Required env vars:
#   A2A_OPERATOR_NAME   — nsc operator name (already pushed to cluster)
#   A2A_ACCOUNT_NAME    — tenant Account name to create / idempotently re-apply
#   A2A_NSC_DIR         — root of the nsc config/data/keystore tree (passed to
#                         --config-dir, --data-dir, --keystore-dir)
#
# Optional env vars:
#   A2A_PREFIX          — subject prefix (default: a2a)
#   A2A_CATALOG_KV_BUCKET — JetStream KV bucket name for AgentCards (default: A2A_AGENT_CARDS)
#   A2A_JS_MEM_STORAGE  — JetStream mem-storage quota (default: 1G)
#   A2A_JS_DISK_STORAGE — JetStream disk-storage quota (default: 10G)
#   A2A_JS_STREAMS      — max streams (default: 10)
#   A2A_JS_CONSUMERS    — max consumers (default: 50)
#   A2A_PUSH_JWT        — optional: skip push to cluster (set to "0" to skip)
#
# Usage:
#   A2A_OPERATOR_NAME=myop A2A_ACCOUNT_NAME=tenant1 A2A_NSC_DIR=/home/nsc \
#     ./scripts/a2a-nsc-bootstrap.sh
#
# Automated path: this script. Manual path: docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md.
# Auth callout fleet keys / AUTH·APP·SYS layout: scripts/a2a-auth-callout-bootstrap.sh.
#
# nsc flag references (all flags used below are from these sources):
#   nsc add account  : https://nats-io.github.io/nsc/nsc_add_account.html
#   nsc edit account : https://nats-io.github.io/nsc/nsc_edit_account.html
#   nsc add user     : https://nats-io.github.io/nsc/nsc_add_user.html
#   nsc push         : https://docs.nats.io/using-nats/nats-tools/nsc/basics
#
# NOTE: nsc was not available in the build environment when this script was
# written. All flags are taken verbatim from docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md
# (which itself cites the official nsc docs) and the official nsc CLI reference
# at https://nats-io.github.io/nsc/. Do NOT add flags beyond those cited above
# without verifying against `nsc <subcommand> --help`.

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() {
    printf '[a2a-nsc-bootstrap] %s\n' "$*" >&2
}

die() {
    printf '[a2a-nsc-bootstrap] ERROR: %s\n' "$*" >&2
    exit 1
}

# run_nsc: echo the command, then execute it.
run_nsc() {
    printf '[a2a-nsc-bootstrap] + nsc %s\n' "$*" >&2
    nsc "$@"
}

# ---------------------------------------------------------------------------
# Input validation
# ---------------------------------------------------------------------------

: "${A2A_OPERATOR_NAME:?A2A_OPERATOR_NAME is required}"
: "${A2A_ACCOUNT_NAME:?A2A_ACCOUNT_NAME is required}"
: "${A2A_NSC_DIR:?A2A_NSC_DIR is required}"

A2A_PREFIX="${A2A_PREFIX:-a2a}"
A2A_JS_MEM_STORAGE="${A2A_JS_MEM_STORAGE:-1G}"
A2A_JS_DISK_STORAGE="${A2A_JS_DISK_STORAGE:-10G}"
A2A_JS_STREAMS="${A2A_JS_STREAMS:-10}"
A2A_JS_CONSUMERS="${A2A_JS_CONSUMERS:-50}"
A2A_PUSH_JWT="${A2A_PUSH_JWT:-1}"
A2A_CATALOG_KV_BUCKET="${A2A_CATALOG_KV_BUCKET:-A2A_AGENT_CARDS}"

NSC_CONFIG_DIR="${A2A_NSC_DIR}/config"
NSC_DATA_DIR="${A2A_NSC_DIR}/data"
NSC_KEYSTORE_DIR="${A2A_NSC_DIR}/keys"

# Common nsc directory flags — appended to every nsc invocation.
# Reference: nsc global flags https://nats-io.github.io/nsc/nsc.html
NSC_DIR_FLAGS="--config-dir ${NSC_CONFIG_DIR} --data-dir ${NSC_DATA_DIR} --keystore-dir ${NSC_KEYSTORE_DIR}"

# Build subject patterns from prefix for readability.
PREFIX="${A2A_PREFIX}"
GATEWAY_INGRESS="${PREFIX}.gateway.>"
AGENT_PUB="${PREFIX}.agent.>"
TASK_PUB="${PREFIX}.task.>"
PUSH_PUB="${PREFIX}.push.>"
CATALOG_REG="${PREFIX}.catalog.register.>"
DISCOVER="${PREFIX}.discover.>"
# JetStream KV data-plane publish wildcard (literal $KV.<bucket>.> for nsc)
KV_PUB="\$KV.${A2A_CATALOG_KV_BUCKET}.>"

log "Starting A2A NSC bootstrap"
log "  Operator   : ${A2A_OPERATOR_NAME}"
log "  Account    : ${A2A_ACCOUNT_NAME}"
log "  NSC dir    : ${A2A_NSC_DIR}"
log "  Prefix     : ${PREFIX}"
log "  KV bucket  : ${A2A_CATALOG_KV_BUCKET}"

# Verify nsc is available.
if ! command -v nsc >/dev/null 2>&1; then
    die "nsc not found in PATH. Install nsc before running this script."
fi

# ---------------------------------------------------------------------------
# Step 1 — Confirm operator context
# ---------------------------------------------------------------------------
# Reference: https://docs.nats.io/using-nats/nats-tools/nsc/basics
log "Step 1: selecting operator context"
run_nsc env ${NSC_DIR_FLAGS} --operator "${A2A_OPERATOR_NAME}"

# ---------------------------------------------------------------------------
# Step 2 — Add the tenant Account (idempotent: nsc add account is a no-op if
# the account already exists with the same name)
# ---------------------------------------------------------------------------
# Reference: https://nats-io.github.io/nsc/nsc_add_account.html
log "Step 2: creating Account '${A2A_ACCOUNT_NAME}' (idempotent)"
run_nsc add account ${NSC_DIR_FLAGS} \
    --operator "${A2A_OPERATOR_NAME}" \
    -n "${A2A_ACCOUNT_NAME}"

# ---------------------------------------------------------------------------
# Step 3 — Enable JetStream limits on the Account
# ---------------------------------------------------------------------------
# Flags:
#   --js-mem-storage   : in-memory storage quota (e.g. 1G, 512M)
#   --js-disk-storage  : disk storage quota
#   --js-streams       : max concurrent streams
#   --js-consumer      : max concurrent consumers
# Reference: https://nats-io.github.io/nsc/nsc_edit_account.html
#            https://docs.nats.io/running-a-nats-service/configuration/resource_management
log "Step 3: enabling JetStream limits on Account '${A2A_ACCOUNT_NAME}'"
run_nsc edit account ${NSC_DIR_FLAGS} \
    --operator "${A2A_OPERATOR_NAME}" \
    -n "${A2A_ACCOUNT_NAME}" \
    --js-mem-storage "${A2A_JS_MEM_STORAGE}" \
    --js-disk-storage "${A2A_JS_DISK_STORAGE}" \
    --js-streams "${A2A_JS_STREAMS}" \
    --js-consumer "${A2A_JS_CONSUMERS}"

# ---------------------------------------------------------------------------
# Step 4 — Create the Gateway service User
# ---------------------------------------------------------------------------
# ACL template: scripts/acl-templates/gateway.acl
#
# Flags used:
#   --allow-sub   : subjects this User may subscribe to
#   --allow-pub   : subjects this User may publish to
#   --deny-pub    : subjects this User must not publish to (defense in depth)
#   --deny-sub    : subjects this User must not subscribe to (catalog ingress)
# Reference: https://nats-io.github.io/nsc/nsc_add_user.html
#
# Note: nsc add user is idempotent for existing user names — it updates the
# existing JWT rather than failing.
log "Step 4: creating Gateway User 'a2a-gateway'"
run_nsc add user ${NSC_DIR_FLAGS} \
    --operator "${A2A_OPERATOR_NAME}" \
    -a "${A2A_ACCOUNT_NAME}" \
    -n "a2a-gateway" \
    --allow-sub "${GATEWAY_INGRESS}" \
    --allow-sub "${DISCOVER}" \
    --allow-sub "${AGENT_PUB}" \
    --allow-sub "${TASK_PUB}" \
    --allow-sub "${PUSH_PUB}" \
    --allow-pub "${AGENT_PUB}" \
    --allow-pub "${TASK_PUB}" \
    --allow-pub "${PUSH_PUB}" \
    --deny-pub "${CATALOG_REG}" \
    --deny-sub "${CATALOG_REG}"

# ---------------------------------------------------------------------------
# Step 5 — Create the Caller template User
# ---------------------------------------------------------------------------
# ACL template: scripts/acl-templates/caller.acl
#
# Production callers are minted dynamically by the auth callout service.
# This static User is a reference template only; the {caller_id} placeholder
# below should be replaced with the real stable caller identifier when minting
# per-caller credentials manually.
#
# Flags used:
#   --allow-pub          : gateway ingress publish
#   --allow-sub          : inbox subscribe (replace {caller_id} per caller)
#   --allow-pub-response : allows one reply publish per matched subscription;
#                          needed when the client library uses dynamic reply
#                          subjects beyond the explicit inbox ACL
#   --deny-pub           : catalog register writes are registrar-only
# Reference: https://nats-io.github.io/nsc/nsc_add_user.html
log "Step 5: creating Caller template User 'a2a-caller-template'"
run_nsc add user ${NSC_DIR_FLAGS} \
    --operator "${A2A_OPERATOR_NAME}" \
    -a "${A2A_ACCOUNT_NAME}" \
    -n "a2a-caller-template" \
    --allow-pub "${GATEWAY_INGRESS}" \
    --allow-sub "_INBOX.{caller_id}.>" \
    --allow-sub "${PREFIX}.push.{caller_id}.>" \
    --allow-pub-response \
    --deny-pub "${AGENT_PUB}" \
    --deny-pub "${TASK_PUB}" \
    --deny-pub "${PUSH_PUB}" \
    --deny-pub "${CATALOG_REG}" \
    --deny-pub "${DISCOVER}" \
    --deny-sub "${GATEWAY_INGRESS}" \
    --deny-sub "${AGENT_PUB}" \
    --deny-sub "${TASK_PUB}" \
    --deny-sub "${CATALOG_REG}" \
    --deny-sub "${DISCOVER}"

# ---------------------------------------------------------------------------
# Step 6 — Create the Registrar service User
# ---------------------------------------------------------------------------
# ACL template: scripts/acl-templates/registrar.acl
#
# Long-lived identity for a2a-nats-discovery. Subscribes to catalog.register.*
# and discover.* subjects; replies via --allow-pub-response.
#
# KV write exclusivity: catalog bucket `${A2A_CATALOG_KV_BUCKET}` data-plane publishes are
# enumerated via `--allow-pub` on `$KV.<bucket>.>` — keep gateway/caller Users off that class
# and still validate fleet IAM (docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md §JetStream/KV).
#
# Flags used:
#   --allow-pub          — JetStream KV `$KV.<bucket>.>` publishes (AgentCard payloads)
#   --allow-sub          — catalog.register + discover ingress; `_INBOX.>` correlates KV / RR
#   --allow-pub-response — RR replies plus dynamic reply publishes on scoped paths
#   --deny-pub/sub       — forbid gateway/agent/task/push subjects (outside registrar scope)
# Reference: https://nats-io.github.io/nsc/nsc_add_user.html
log "Step 6: creating Registrar service User 'a2a-registrar'"
run_nsc add user ${NSC_DIR_FLAGS} \
    --operator "${A2A_OPERATOR_NAME}" \
    -a "${A2A_ACCOUNT_NAME}" \
    -n "a2a-registrar" \
    --allow-pub "${KV_PUB}" \
    --allow-sub "${CATALOG_REG}" \
    --allow-sub "${DISCOVER}" \
    --allow-sub "_INBOX.>" \
    --allow-pub-response \
    --deny-pub "${GATEWAY_INGRESS}" \
    --deny-pub "${AGENT_PUB}" \
    --deny-pub "${TASK_PUB}" \
    --deny-pub "${PUSH_PUB}" \
    --deny-sub "${GATEWAY_INGRESS}" \
    --deny-sub "${AGENT_PUB}" \
    --deny-sub "${TASK_PUB}" \
    --deny-sub "${PUSH_PUB}"

# ---------------------------------------------------------------------------
# Step 7 — (Optional) Push Account and User JWTs to the operator resolver
# ---------------------------------------------------------------------------
# Reference: https://docs.nats.io/using-nats/nats-tools/nsc/basics
# Skip by setting A2A_PUSH_JWT=0.
if [ "${A2A_PUSH_JWT}" != "0" ]; then
    log "Step 7: pushing Account JWTs to resolver"
    run_nsc push ${NSC_DIR_FLAGS} \
        --operator "${A2A_OPERATOR_NAME}" \
        -a "${A2A_ACCOUNT_NAME}"
else
    log "Step 7: skipping JWT push (A2A_PUSH_JWT=0)"
fi

# ---------------------------------------------------------------------------
# Step 8 — Reminder: bootstrap JetStream assets
# ---------------------------------------------------------------------------
# nsc configures JWT permissions only. After the Account JWT is pushed,
# create the following JetStream assets using the nats CLI or application
# bootstrap (a2a-nats provision_streams / a2a-nats-discovery for KV):
#
#   Stream   A2A_EVENTS        subject: ${PREFIX}.task.*.events.*   max_age: 24h
#   KV       A2A_AGENT_CARDS   history: 1   max_value_size: 65536
#   Stream   A2A_PUSH_DLQ      subject: ${PREFIX}.push.dlq.*.*
#
# See docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md §JetStream and KV assets.
log "Step 8: JetStream assets are NOT managed by nsc."
log "  After JWT push, create using 'nats' CLI or application bootstrap:"
log "    Stream   ${PREFIX}_EVENTS       subject: ${PREFIX}.task.*.events.*  (max_age 24h)"
log "    KV       ${PREFIX}_AGENT_CARDS  history: 1  max_value_size: 65536"
log "    Stream   ${PREFIX}_PUSH_DLQ     subject: ${PREFIX}.push.dlq.*.*"
log "  Reference: docs/A2A_NSC_ACCOUNT_BOOTSTRAP.md §JetStream and KV assets"

log "Bootstrap complete for Account '${A2A_ACCOUNT_NAME}'."

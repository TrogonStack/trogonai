#!/bin/sh
# a2a-nsc-export-discovery.sh — Phase 4 federated discovery export/import scaffold
#
# Wraps the operator-signed nsc add export / nsc add import flow described in:
#   docs/a2a/explanation/federated-discovery-sketch.md  §Operator-signed Account export contract
#
# Federation is OFF by default. Operators opt in by running this script to
# create signed exports on a publisher Account and matching imports on one or
# more consumer Accounts. Only {prefix}.discover.> crosses Account boundaries;
# gateway/task/push/catalog subjects stay Account-local.
#
# This is a SCAFFOLD: no discovery automation is in-tree yet. The commands
# below emit the nsc invocations that will be needed; use --dry-run to print
# without executing.
#
# Required env vars:
#   A2A_OPERATOR_NAME        — nsc operator name
#   A2A_NSC_DIR              — root of the nsc config/data/keystore tree
#   A2A_PUBLISHER_ACCOUNT    — Account that exports {prefix}.discover.>
#
# For import mode (--import flag), also required:
#   A2A_CONSUMER_ACCOUNT     — Account that imports from the publisher
#   A2A_PUBLISHER_PUBKEY     — public NKEY of the publisher Account
#
# Optional env vars:
#   A2A_PREFIX               — subject prefix (default: a2a)
#   A2A_ALLOWED_CONSUMERS    — comma-separated list of consumer Account public
#                              keys allowed to import (export side only).
#                              Leave empty for an unrestricted export (not
#                              recommended in production).
#
# Flags:
#   --dry-run   Print the nsc commands without executing them.
#   --import    Emit the consumer-Account import command instead of the
#               publisher-Account export command.
#
# Usage examples:
#   # Dry-run the publisher export
#   A2A_OPERATOR_NAME=myop A2A_NSC_DIR=/home/nsc A2A_PUBLISHER_ACCOUNT=orgA \
#     A2A_ALLOWED_CONSUMERS=ACONSUMERPUBKEY1,ACONSUMERPUBKEY2 \
#     ./scripts/a2a-nsc-export-discovery.sh --dry-run
#
#   # Execute the publisher export
#   A2A_OPERATOR_NAME=myop A2A_NSC_DIR=/home/nsc A2A_PUBLISHER_ACCOUNT=orgA \
#     A2A_ALLOWED_CONSUMERS=ACONSUMERPUBKEY1 \
#     ./scripts/a2a-nsc-export-discovery.sh
#
#   # Dry-run the consumer import
#   A2A_OPERATOR_NAME=myop A2A_NSC_DIR=/home/nsc \
#     A2A_CONSUMER_ACCOUNT=orgB A2A_PUBLISHER_ACCOUNT=orgA \
#     A2A_PUBLISHER_PUBKEY=APUBKEYOFPUBLISHER \
#     ./scripts/a2a-nsc-export-discovery.sh --import --dry-run
#
# nsc flag references (all flags are sourced from official nsc docs only):
#   nsc add export : https://nats-io.github.io/nsc/nsc_add_export.html
#   nsc add import : https://nats-io.github.io/nsc/nsc_add_import.html
#   nsc push       : https://docs.nats.io/using-nats/nats-tools/nsc/basics
#
# NOTE: nsc was not available in the build environment when this script was
# written. All flags are taken from docs/a2a/explanation/federated-discovery-sketch.md
# (which cites official nsc docs) and the nsc CLI reference at
# https://nats-io.github.io/nsc/. Do NOT add flags beyond those cited above
# without verifying against `nsc <subcommand> --help`.

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log() {
    printf '[a2a-nsc-export-discovery] %s\n' "$*" >&2
}

die() {
    printf '[a2a-nsc-export-discovery] ERROR: %s\n' "$*" >&2
    exit 1
}

# maybe_run: in --dry-run mode, print the command; otherwise execute it.
maybe_run() {
    if [ "${DRY_RUN}" = "1" ]; then
        printf '[DRY-RUN] nsc %s\n' "$*"
    else
        printf '[a2a-nsc-export-discovery] + nsc %s\n' "$*" >&2
        nsc "$@"
    fi
}

# ---------------------------------------------------------------------------
# Parse flags
# ---------------------------------------------------------------------------

DRY_RUN=0
MODE="export"  # "export" or "import"

for arg in "$@"; do
    case "${arg}" in
        --dry-run) DRY_RUN=1 ;;
        --import)  MODE="import" ;;
        *) die "Unknown flag: ${arg}. Supported flags: --dry-run, --import" ;;
    esac
done

# ---------------------------------------------------------------------------
# Input validation
# ---------------------------------------------------------------------------

: "${A2A_OPERATOR_NAME:?A2A_OPERATOR_NAME is required}"
: "${A2A_NSC_DIR:?A2A_NSC_DIR is required}"
: "${A2A_PUBLISHER_ACCOUNT:?A2A_PUBLISHER_ACCOUNT is required}"

A2A_PREFIX="${A2A_PREFIX:-a2a}"
A2A_ALLOWED_CONSUMERS="${A2A_ALLOWED_CONSUMERS:-}"

NSC_CONFIG_DIR="${A2A_NSC_DIR}/config"
NSC_DATA_DIR="${A2A_NSC_DIR}/data"
NSC_KEYSTORE_DIR="${A2A_NSC_DIR}/keys"

# Common nsc directory flags.
NSC_DIR_FLAGS="--config-dir ${NSC_CONFIG_DIR} --data-dir ${NSC_DATA_DIR} --keystore-dir ${NSC_KEYSTORE_DIR}"

DISCOVER_SUBJECT="${A2A_PREFIX}.discover.>"
EXPORT_NAME="a2a-discover"

# Verify nsc is available (skip check in dry-run so the script can be used
# on machines without nsc for documentation/review purposes).
if [ "${DRY_RUN}" = "0" ] && ! command -v nsc >/dev/null 2>&1; then
    die "nsc not found in PATH. Install nsc before running this script, or use --dry-run."
fi

# ---------------------------------------------------------------------------
# Export mode — publisher Account
# ---------------------------------------------------------------------------

if [ "${MODE}" = "export" ]; then
    log "Mode: EXPORT (publisher Account)"
    log "  Operator  : ${A2A_OPERATOR_NAME}"
    log "  Publisher : ${A2A_PUBLISHER_ACCOUNT}"
    log "  Subject   : ${DISCOVER_SUBJECT}"
    log "  Dry-run   : ${DRY_RUN}"

    # Build optional --accounts flag.
    # Flags:
    #   --name     : export name (human-readable label)
    #   --service  : service export (request/reply semantics, not stream)
    #   --subject  : subject pattern to export
    #   --accounts : comma-separated list of consumer Account public keys
    #                allowed to import; omit for unrestricted (not recommended)
    # Reference: https://nats-io.github.io/nsc/nsc_add_export.html
    if [ -n "${A2A_ALLOWED_CONSUMERS}" ]; then
        maybe_run add export ${NSC_DIR_FLAGS} \
            --operator "${A2A_OPERATOR_NAME}" \
            -a "${A2A_PUBLISHER_ACCOUNT}" \
            --name "${EXPORT_NAME}" \
            --service \
            --subject "${DISCOVER_SUBJECT}" \
            --accounts "${A2A_ALLOWED_CONSUMERS}"
    else
        log "WARNING: A2A_ALLOWED_CONSUMERS is empty — export will be unrestricted."
        log "         Set A2A_ALLOWED_CONSUMERS to a comma-separated list of consumer"
        log "         Account public keys to restrict federation to known partners."
        maybe_run add export ${NSC_DIR_FLAGS} \
            --operator "${A2A_OPERATOR_NAME}" \
            -a "${A2A_PUBLISHER_ACCOUNT}" \
            --name "${EXPORT_NAME}" \
            --service \
            --subject "${DISCOVER_SUBJECT}"
    fi

    # Push the updated Account JWT so the resolver sees the export.
    # Skip push in dry-run; the caller can run nsc push separately.
    if [ "${DRY_RUN}" = "0" ]; then
        log "Pushing updated Account JWT for '${A2A_PUBLISHER_ACCOUNT}'"
        printf '[a2a-nsc-export-discovery] + nsc push %s --operator %s -a %s\n' \
            "${NSC_DIR_FLAGS}" "${A2A_OPERATOR_NAME}" "${A2A_PUBLISHER_ACCOUNT}" >&2
        nsc push ${NSC_DIR_FLAGS} \
            --operator "${A2A_OPERATOR_NAME}" \
            -a "${A2A_PUBLISHER_ACCOUNT}"
    else
        printf '[DRY-RUN] nsc push %s --operator %s -a %s\n' \
            "${NSC_DIR_FLAGS}" "${A2A_OPERATOR_NAME}" "${A2A_PUBLISHER_ACCOUNT}"
    fi

    log "Export scaffold complete."
    log "Next steps (not automated here):"
    log "  1. Obtain the publisher Account public key:"
    log "       nsc describe account -n ${A2A_PUBLISHER_ACCOUNT} | grep 'Account ID'"
    log "  2. Provide that key to each consumer operator to run the --import variant:"
    log "       A2A_PUBLISHER_PUBKEY=<key> ./scripts/a2a-nsc-export-discovery.sh --import"
    log "  3. Configure gateway federation resolution:"
    log "       (publisher_account_id → import_name) mapping — see"
    log "       docs/a2a/explanation/federated-discovery-sketch.md §Consumer Account"

# ---------------------------------------------------------------------------
# Import mode — consumer Account
# ---------------------------------------------------------------------------

elif [ "${MODE}" = "import" ]; then
    : "${A2A_CONSUMER_ACCOUNT:?A2A_CONSUMER_ACCOUNT is required for --import mode}"
    : "${A2A_PUBLISHER_PUBKEY:?A2A_PUBLISHER_PUBKEY is required for --import mode}"

    IMPORT_NAME="${EXPORT_NAME}-from-${A2A_PUBLISHER_ACCOUNT}"

    log "Mode: IMPORT (consumer Account)"
    log "  Operator        : ${A2A_OPERATOR_NAME}"
    log "  Consumer        : ${A2A_CONSUMER_ACCOUNT}"
    log "  Publisher key   : ${A2A_PUBLISHER_PUBKEY}"
    log "  Import name     : ${IMPORT_NAME}"
    log "  Remote subject  : ${DISCOVER_SUBJECT}"
    log "  Dry-run         : ${DRY_RUN}"

    # Flags:
    #   --name           : import name (human-readable label)
    #   --service        : service import (request/reply)
    #   --subject        : local subject to map the import onto
    #   --account        : publisher Account public key
    #   --remote-subject : subject on the publisher side to forward requests to
    # Reference: https://nats-io.github.io/nsc/nsc_add_import.html
    maybe_run add import ${NSC_DIR_FLAGS} \
        --operator "${A2A_OPERATOR_NAME}" \
        -a "${A2A_CONSUMER_ACCOUNT}" \
        --name "${IMPORT_NAME}" \
        --service \
        --subject "${DISCOVER_SUBJECT}" \
        --account "${A2A_PUBLISHER_PUBKEY}" \
        --remote-subject "${DISCOVER_SUBJECT}"

    if [ "${DRY_RUN}" = "0" ]; then
        log "Pushing updated Account JWT for '${A2A_CONSUMER_ACCOUNT}'"
        printf '[a2a-nsc-export-discovery] + nsc push %s --operator %s -a %s\n' \
            "${NSC_DIR_FLAGS}" "${A2A_OPERATOR_NAME}" "${A2A_CONSUMER_ACCOUNT}" >&2
        nsc push ${NSC_DIR_FLAGS} \
            --operator "${A2A_OPERATOR_NAME}" \
            -a "${A2A_CONSUMER_ACCOUNT}"
    else
        printf '[DRY-RUN] nsc push %s --operator %s -a %s\n' \
            "${NSC_DIR_FLAGS}" "${A2A_OPERATOR_NAME}" "${A2A_CONSUMER_ACCOUNT}"
    fi

    log "Import scaffold complete."
    log "Next steps (not automated here):"
    log "  1. Verify gateway can reach the publisher DiscoverService:"
    log "       nats request ${DISCOVER_SUBJECT/<agent_id>} '' --creds consumer.creds"
    log "  2. Configure gateway federation merge — see"
    log "       docs/a2a/explanation/federated-discovery-sketch.md §Client-visible catalog merge"
    log "  3. Wire SpiceDB BulkCheckPermission for federated agent tuples (Phase 4)."
fi

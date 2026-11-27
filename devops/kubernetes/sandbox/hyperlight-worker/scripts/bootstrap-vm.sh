#!/usr/bin/env bash
# Prepares a Debian or Ubuntu VM to build and run the hyperlight worker:
# checks for a hypervisor device, installs the build packages and builds
# trogon-sandbox-hyperlight-worker from this repository.
#
# The VM needs /dev/kvm or /dev/mshv, so give it a host CPU model with
# nested virtualization, or use a bare-metal instance. Rust must already be
# installed (cargo on PATH). Re-running the script is safe.
#
# Usage: bootstrap-vm.sh [--no-build]
#   --no-build  check the host and install packages without building

set -euo pipefail

BUILD=1
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=0 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

log() { printf '[bootstrap] %s\n' "$*"; }
fail() { printf '[bootstrap] ERROR: %s\n' "$*" >&2; exit 1; }

if [[ "$(uname -s)" != "Linux" ]]; then
  fail "this script runs on a Linux host with KVM or MSHV"
fi

if [[ "$EUID" -eq 0 ]]; then
  SUDO=""
else
  SUDO="sudo"
fi

log "checking for a hypervisor device"
if [[ -e /dev/kvm ]]; then
  log "  /dev/kvm present (KVM)"
elif [[ -e /dev/mshv ]]; then
  log "  /dev/mshv present (MSHV)"
else
  fail "neither /dev/kvm nor /dev/mshv exists. Give the VM a host CPU model with nested virtualization enabled on its hypervisor, or use a bare-metal instance."
fi

if [[ -e /dev/kvm ]] && ! [[ -r /dev/kvm && -w /dev/kvm ]]; then
  log "  adding $USER to the kvm group (log in again for it to apply)"
  $SUDO usermod -aG kvm "$USER" || true
fi

log "installing build packages"
export DEBIAN_FRONTEND=noninteractive
$SUDO apt-get update -y
$SUDO apt-get install -y --no-install-recommends \
  build-essential \
  ca-certificates \
  cmake \
  git \
  pkg-config

if [[ "$BUILD" -eq 0 ]]; then
  log "done (build skipped)"
  exit 0
fi

command -v cargo >/dev/null 2>&1 || fail "cargo is not on PATH; install a Rust toolchain first"
log "using $(cargo --version)"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../../.." && pwd)"
WORKSPACE_DIR="$REPO_ROOT/rsworkspace"
[[ -f "$WORKSPACE_DIR/Cargo.toml" ]] || fail "no Cargo workspace at $WORKSPACE_DIR"

log "building trogon-sandbox-hyperlight-worker"
( cd "$WORKSPACE_DIR" && cargo build --locked --release --package trogon-sandbox-hyperlight-worker )

TARGET_DIR="$(cd "$WORKSPACE_DIR" && cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
BINARY="${TARGET_DIR:-$WORKSPACE_DIR/target}/release/trogon-sandbox-hyperlight-worker"

log "done. binary at: $BINARY"
log
log "next steps:"
log "  1. put the .wasm components under \$SANDBOX_HYPERLIGHT_COMPONENT_ROOT"
log "  2. SANDBOX_NATS_URL=nats://<nats-host>:4222 SANDBOX_HYPERLIGHT_COMPONENT_ROOT=/srv/components \\"
log "       $BINARY"
log "  The stock binary has no hypervisor backend and exits at startup; run a"
log "  build that passes a backend to trogon_sandbox_hyperlight_worker::serve."

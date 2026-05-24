#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)/rsworkspace"
cargo test -p a2a-bridge nats_transport_
if [[ "${1:-}" == "--live" ]]; then
  export A2A_BRIDGE_TRANSPORT=nats
  cargo test -p a2a-bridge -- --ignored nats_transport_live
fi

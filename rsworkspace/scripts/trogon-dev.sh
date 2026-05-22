#!/usr/bin/env bash
# rsworkspace/scripts/trogon-dev.sh
set -e
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RSWORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
ENV_FILE="$RSWORKSPACE/.env.local"

if [ ! -f "$ENV_FILE" ]; then
  echo "Missing $ENV_FILE — copy from .env.local.example"
  exit 1
fi

set -a; source "$ENV_FILE"; set +a

BIN="$RSWORKSPACE/target/release"
LOG_DIR="${TROGON_LOG_DIR:-/tmp}"

# 1. NATS + JetStream (skip if port already open)
if ! nc -z localhost 4222 2>/dev/null; then
  echo "Starting nats-server -js"
  nats-server -p 4222 -js > "$LOG_DIR/trogon-nats.log" 2>&1 &
  # Wait until NATS accepts TCP connections (max 10s)
  for i in $(seq 1 20); do
    nc -z localhost 4222 2>/dev/null && break
    sleep 0.5
    if [ "$i" -eq 20 ]; then
      echo "ERROR: nats-server did not start within 10s — check $LOG_DIR/trogon-nats.log"
      exit 1
    fi
  done
fi

# 2. Execution backend — REQUIRED for bash
echo "Starting trogon-wasm-runtime (prefix=acp.wasm)"
WASM_ONLY=0 \
WASM_AUTO_ALLOW_PERMISSIONS=1 \
ACP_PREFIX=acp.wasm \
AGENT_TYPE=wasm \
NATS_URL="${NATS_URL:-nats://localhost:4222}" \
  "$BIN/trogon-wasm-runtime" > "$LOG_DIR/trogon-wasm.log" 2>&1 &

# 3. Compactor — REQUIRED for long sessions + /compact (Stage 2 fixes CLI wiring)
if [ -n "$ANTHROPIC_TOKEN" ]; then
  echo "Starting trogon-compactor"
  PROXY_URL="${PROXY_URL:-http://localhost:8080}" \
  ANTHROPIC_TOKEN="$ANTHROPIC_TOKEN" \
  NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-compactor" > "$LOG_DIR/trogon-compactor.log" 2>&1 &
fi

# 4. LLM runners — start only when credentials present
if [ -n "$ANTHROPIC_TOKEN" ]; then
  echo "Starting trogon-acp-runner (prefix=acp.claude)"
  ACP_PREFIX=acp.claude NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-acp-runner" > "$LOG_DIR/trogon-acp.log" 2>&1 &
fi

if [ -n "$XAI_API_KEY" ]; then
  echo "Starting trogon-xai-runner (prefix=acp.grok)"
  ACP_PREFIX=acp.grok XAI_API_KEY="$XAI_API_KEY" NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-xai-runner" > "$LOG_DIR/trogon-xai.log" 2>&1 &
fi

if [ -n "$OPENROUTER_API_KEY" ]; then
  echo "Starting trogon-openrouter-runner (prefix=acp.openrouter)"
  ACP_PREFIX=acp.openrouter OPENROUTER_API_KEY="$OPENROUTER_API_KEY" \
    NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-openrouter-runner" > "$LOG_DIR/trogon-openrouter.log" 2>&1 &
fi

if [ "${CODEX_ENABLED:-0}" = "1" ]; then
  echo "Starting trogon-codex-runner (prefix=acp.codex)"
  ACP_PREFIX=acp.codex NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-codex-runner" > "$LOG_DIR/trogon-codex.log" 2>&1 &
fi

echo ""
echo "Trogon dev stack started. Logs: $LOG_DIR/trogon-*.log"
echo "Run: $BIN/trogon --doctor"
echo "Then: cd /your/project && $BIN/trogon"

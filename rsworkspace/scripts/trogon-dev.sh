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

# Returns 0 (true) if a process with the given binary name is already running.
# pgrep -x truncates comm to 15 chars so it misses long names; match full path instead.
running() { pgrep -f "release/$1" > /dev/null 2>&1; }

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
if running trogon-wasm-runtime; then
  echo "trogon-wasm-runtime already running — skipping"
else
  echo "Starting trogon-wasm-runtime (prefix=acp.wasm)"
  WASM_ONLY=0 \
  WASM_AUTO_ALLOW_PERMISSIONS=0 \
  ACP_PREFIX=acp.wasm \
  AGENT_TYPE=wasm \
  NATS_URL="${NATS_URL:-nats://localhost:4222}" \
    "$BIN/trogon-wasm-runtime" > "$LOG_DIR/trogon-wasm.log" 2>&1 &
fi

# 3. Compactor — REQUIRED for long sessions + /compact (Stage 2 fixes CLI wiring)
if [ -n "$ANTHROPIC_TOKEN" ]; then
  if running trogon-compactor; then
    echo "trogon-compactor already running — skipping"
  else
    echo "Starting trogon-compactor"
    PROXY_URL="${PROXY_URL:-http://localhost:8080}" \
    ANTHROPIC_TOKEN="$ANTHROPIC_TOKEN" \
    NATS_URL="${NATS_URL:-nats://localhost:4222}" \
      "$BIN/trogon-compactor" > "$LOG_DIR/trogon-compactor.log" 2>&1 &
  fi
fi

# 4. LLM runners — start only when credentials present
if [ -n "$ANTHROPIC_TOKEN" ]; then
  if running trogon-acp-runner; then
    echo "trogon-acp-runner already running — skipping"
  else
    echo "Starting trogon-acp-runner (prefix=acp.claude)"
    ACP_PREFIX=acp.claude NATS_URL="${NATS_URL:-nats://localhost:4222}" \
      "$BIN/trogon-acp-runner" > "$LOG_DIR/trogon-acp.log" 2>&1 &
  fi
fi

if [ -n "$XAI_API_KEY" ]; then
  if running trogon-xai-runner; then
    echo "trogon-xai-runner already running — skipping"
  else
    echo "Starting trogon-xai-runner (prefix=acp.grok)"
    ACP_PREFIX=acp.grok XAI_API_KEY="$XAI_API_KEY" NATS_URL="${NATS_URL:-nats://localhost:4222}" \
      "$BIN/trogon-xai-runner" > "$LOG_DIR/trogon-xai.log" 2>&1 &
  fi
fi

if [ -n "$OPENROUTER_API_KEY" ]; then
  if running trogon-openrouter-runner; then
    echo "trogon-openrouter-runner already running — skipping"
  else
    echo "Starting trogon-openrouter-runner (prefix=acp.openrouter)"
    ACP_PREFIX=acp.openrouter OPENROUTER_API_KEY="$OPENROUTER_API_KEY" \
      NATS_URL="${NATS_URL:-nats://localhost:4222}" \
      "$BIN/trogon-openrouter-runner" > "$LOG_DIR/trogon-openrouter.log" 2>&1 &
  fi
fi

if [ "${CODEX_ENABLED:-0}" = "1" ]; then
  if running trogon-codex-runner; then
    echo "trogon-codex-runner already running — skipping"
  else
    echo "Starting trogon-codex-runner (prefix=acp.codex)"
    ACP_PREFIX=acp.codex NATS_URL="${NATS_URL:-nats://localhost:4222}" \
      "$BIN/trogon-codex-runner" > "$LOG_DIR/trogon-codex.log" 2>&1 &
  fi
fi

echo ""
echo "Trogon dev stack started. Logs: $LOG_DIR/trogon-*.log"
echo "Run: $BIN/trogon --doctor"
echo "Then: cd /your/project && $BIN/trogon"

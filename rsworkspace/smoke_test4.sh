#!/usr/bin/env bash
# Smoke test 4: last remaining env var gaps
#
#   1. XAI_DEFAULT_MODEL used in xAI HTTP request body
#   2. XAI_PROMPT_TIMEOUT_SECS cancels prompt after N seconds (mid-stream hang)
#   3. XAI_PROMPT_TIMEOUT_SECS also covers connection-hang (no HTTP headers)
#
# Requires: NATS at localhost:4222, python3, binary built in target/debug/

set -euo pipefail

NATS_URL="${NATS_URL:-nats://localhost:4222}"
BINARY="./target/debug/trogon-xai-runner"
BASE="smoke4-$$"
FAKE_PORT=$((31000 + (RANDOM % 1000)))
PASS=0; FAIL=0

red="\033[31m"; green="\033[32m"; yellow="\033[33m"; reset="\033[0m"
pass() { echo -e "  ${green}PASS${reset} $1"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${red}FAIL${reset} $1"; FAIL=$((FAIL + 1)); }
info() { echo -e "  ${yellow}INFO${reset} $1"; }

phase_prefix() { echo "${BASE}-p${1}"; }
phase_bucket() { echo "SMOKE4_${$}_P${1}"; }
PREFIX="${BASE}-p0"; KV_BUCKET="SMOKE4_${$}_P0"

nats_req() {
  nats --server "$NATS_URL" request "$1" "$2" --timeout "${3:-10s}" 2>/dev/null
}

fake_log() { curl -s "http://127.0.0.1:${FAKE_PORT}/log"; }

start_runner() {
  local extra="${1:-}"
  eval "ACP_PREFIX=\"$PREFIX\" \
    XAI_SESSION_BUCKET=\"$KV_BUCKET\" \
    XAI_BASE_URL=\"http://127.0.0.1:${FAKE_PORT}/v1\" \
    NATS_URL=\"$NATS_URL\" \
    RUST_LOG=warn \
    $extra \
    $BINARY" &
  RUNNER_PID=$!
  sleep 0.5
}

stop_runner() {
  [[ -n "${RUNNER_PID:-}" ]] && { kill "$RUNNER_PID" 2>/dev/null || true; wait "$RUNNER_PID" 2>/dev/null || true; unset RUNNER_PID; }
  sleep 0.2
}

start_fake_xai() {
  python3 fake_xai_server.py "$FAKE_PORT" "${1:-normal}" &
  FAKE_PID=$!
  sleep 0.3
}

stop_fake_xai() {
  [[ -n "${FAKE_PID:-}" ]] && { kill "$FAKE_PID" 2>/dev/null || true; wait "$FAKE_PID" 2>/dev/null || true; unset FAKE_PID; }
}

cleanup() {
  set +e
  stop_runner; stop_fake_xai
  for i in 1 2 3; do nats --server "$NATS_URL" kv del "$(phase_bucket $i)" --force 2>/dev/null || true; done
}
trap cleanup EXIT

echo ""
echo "=== trogon-xai-runner smoke test 4 ==="
echo "fake port: $FAKE_PORT"
echo ""

# ════════════════════════════════════════════════════════
echo "--- [1] XAI_DEFAULT_MODEL used in xAI HTTP request body ---"
PREFIX=$(phase_prefix 1); KV_BUCKET=$(phase_bucket 1)
start_fake_xai normal
start_runner 'XAI_API_KEY="sk-fake" XAI_DEFAULT_MODEL="grok-3-mini" XAI_MODELS="grok-3-mini:Grok 3 Mini"'

SID=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}' | \
      python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session: $SID"

nats_req "${PREFIX}.session.${SID}.agent.prompt" \
  "{\"sessionId\":\"${SID}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hi\"}]}" 15s > /dev/null

if fake_log | python3 -c "
import sys,json
reqs = json.load(sys.stdin)
body = json.loads(reqs[-1]['body'])
m = body.get('model','')
assert m == 'grok-3-mini', f'expected grok-3-mini but got: {m!r}'
print('ok model:', m)
" 2>/dev/null; then
  pass "XAI_DEFAULT_MODEL=grok-3-mini sent as 'model' in HTTP request body"
else
  fail "wrong model in request: $(fake_log | python3 -c "import sys,json; r=json.load(sys.stdin); print(json.loads(r[-1]['body']).get('model'))" 2>/dev/null)"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [2] XAI_PROMPT_TIMEOUT_SECS cancels prompt after N seconds ---"
PREFIX=$(phase_prefix 2); KV_BUCKET=$(phase_bucket 2)
start_fake_xai hang_stream  # sends headers then hangs mid-stream (per-chunk timeout)
start_runner 'XAI_API_KEY="sk-fake" XAI_PROMPT_TIMEOUT_SECS="2"'

SID=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}' | \
      python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session: $SID (timeout configured to 2s)"

T0=$SECONDS
RESP=$(nats_req "${PREFIX}.session.${SID}.agent.prompt" \
  "{\"sessionId\":\"${SID}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hi\"}]}" 15s)
ELAPSED=$((SECONDS - T0))
info "prompt returned after ~${ELAPSED}s: $RESP"

# Timeout fires as stopReason=cancelled within a few seconds of the 2s deadline
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
sr = d.get('stopReason','')
# 'cancelled' when timed out mid-stream; an error code is also acceptable
ok = sr in ('cancelled','end_turn') or 'code' in d
assert ok, f'unexpected response: {d}'
print('stopReason:', sr or d.get('code'))
" 2>/dev/null && [[ $ELAPSED -le 5 ]]; then
  pass "prompt cancelled within ~${ELAPSED}s (XAI_PROMPT_TIMEOUT_SECS=2 respected)"
else
  fail "timeout not respected: elapsed=${ELAPSED}s, response=$RESP"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [3] XAI_PROMPT_TIMEOUT_SECS also covers connection hang (no headers) ---"
PREFIX=$(phase_prefix 3); KV_BUCKET=$(phase_bucket 3)
start_fake_xai hang  # never sends HTTP headers at all
start_runner 'XAI_API_KEY="sk-fake" XAI_PROMPT_TIMEOUT_SECS="2"'

SID=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}' | \
      python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session: $SID (hang mode — no HTTP headers ever sent)"

T0=$SECONDS
RESP=$(nats_req "${PREFIX}.session.${SID}.agent.prompt" \
  "{\"sessionId\":\"${SID}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hi\"}]}" 15s)
ELAPSED=$((SECONDS - T0))
info "prompt returned after ~${ELAPSED}s: $RESP"

if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
sr = d.get('stopReason','')
ok = sr in ('cancelled','end_turn') or 'code' in d
assert ok, f'unexpected response: {d}'
print('stopReason/code:', sr or d.get('code'))
" 2>/dev/null && [[ $ELAPSED -le 5 ]]; then
  pass "connection-hang cancelled within ~${ELAPSED}s (XAI_PROMPT_TIMEOUT_SECS=2 respected)"
else
  fail "connection timeout not respected: elapsed=${ELAPSED}s, response=$RESP"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "=== Results ==="
echo -e "  ${green}passed${reset}: $PASS"
echo -e "  ${red}failed${reset}: $FAIL"
if [[ $FAIL -gt 0 ]]; then exit 1
else echo -e "\n${green}All smoke tests 4 passed.${reset}"; fi

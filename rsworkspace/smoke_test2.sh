#!/usr/bin/env bash
# Extended smoke test for trogon-xai-runner.
# Tests the gaps not covered by integration tests that require a running binary:
#
#   1. XAI_MODELS controls which models are valid for set_session_model
#   2. authenticate end-to-end via NATS (xai-api-key, unknown, empty-key)
#   3. set_session_model / set_session_config_option via NATS
#   4. Session notifications arrive on the real NATS bus (with fake xAI server)
#   5. SIGTERM during active prompt: binary exits cleanly, KV survives
#
# Requires: NATS at localhost:4222, python3, binary built in target/debug/

set -euo pipefail

NATS_URL="${NATS_URL:-nats://localhost:4222}"
BINARY="./target/debug/trogon-xai-runner"
BASE="smoke2-$$"
FAKE_PORT=$((29000 + (RANDOM % 1000)))

# Each phase uses its own prefix+bucket so stale runners can't interfere
phase_prefix() { echo "${BASE}-p${1}"; }
phase_bucket() { echo "SMOKE2_${$}_P${1}"; }

# Current phase vars (updated per phase)
PREFIX="${BASE}-p0"
KV_BUCKET="SMOKE2_${$}_P0"
PASS=0
FAIL=0

red="\033[31m"; green="\033[32m"; yellow="\033[33m"; reset="\033[0m"

pass() { echo -e "  ${green}PASS${reset} $1"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${red}FAIL${reset} $1"; FAIL=$((FAIL + 1)); }
info() { echo -e "  ${yellow}INFO${reset} $1"; }

nats_req() {
  local subject="$1" payload="$2" timeout="${3:-10s}"
  nats --server "$NATS_URL" request "$subject" "$payload" --timeout "$timeout" 2>/dev/null
}

start_runner() {
  local extra_env="${1:-}"
  eval "ACP_PREFIX=\"$PREFIX\" \
    XAI_SESSION_BUCKET=\"$KV_BUCKET\" \
    XAI_BASE_URL=\"http://127.0.0.1:${FAKE_PORT}/v1\" \
    NATS_URL=\"$NATS_URL\" \
    RUST_LOG=warn \
    $extra_env \
    $BINARY" &
  RUNNER_PID=$!
  sleep 0.5
}

stop_runner() {
  if [[ -n "${RUNNER_PID:-}" ]]; then
    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
    unset RUNNER_PID
  fi
  sleep 0.2
}

start_fake_xai() {
  local mode="${1:-normal}"
  python3 fake_xai_server.py "$FAKE_PORT" "$mode" &
  FAKE_PID=$!
  sleep 0.3
}

stop_fake_xai() {
  if [[ -n "${FAKE_PID:-}" ]]; then
    kill "$FAKE_PID" 2>/dev/null || true
    wait "$FAKE_PID" 2>/dev/null || true
    unset FAKE_PID
  fi
}

cleanup() {
  stop_runner
  stop_fake_xai
  # Delete all phase buckets
  for i in 1 2 3 4 5; do
    nats --server "$NATS_URL" kv del "$(phase_bucket $i)" --force 2>/dev/null || true
  done
}
trap cleanup EXIT

echo ""
echo "=== trogon-xai-runner extended smoke test ==="
echo "binary : $BINARY"
echo "prefix : $PREFIX"
echo "bucket : $KV_BUCKET"
echo "nats   : $NATS_URL"
echo "fake   : http://127.0.0.1:${FAKE_PORT}/v1"
echo ""

# ════════════════════════════════════════════════════════
echo "--- [1] XAI_MODELS controls valid models and initialize advertises auth ---"
PREFIX=$(phase_prefix 1); KV_BUCKET=$(phase_bucket 1)
start_fake_xai normal
# Use custom models — grok-3 is NOT in this list, but binary adds default automatically
start_runner "XAI_MODELS=\"custom-grok:Custom Grok,other-model:Other Model\" XAI_API_KEY=\"sk-fake\" XAI_DEFAULT_MODEL=\"custom-grok\""

INIT_RESP=$(nats_req "${PREFIX}.agent.initialize" '{"protocolVersion":1}')
info "initialize response (abbrev): $(echo "$INIT_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print('authMethods:', [m['id'] for m in d.get('authMethods',[])])" 2>/dev/null)"

# Phase 1 runs with XAI_API_KEY="sk-fake" so BOTH auth methods are advertised
if echo "$INIT_RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
methods = [m.get('id','') for m in d.get('authMethods',[])]
assert 'xai-api-key' in methods, f'xai-api-key missing: {methods}'
assert 'agent' in methods, f'agent missing (expected with server key): {methods}'
assert d.get('agentInfo',{}).get('name') == 'trogon-xai-runner'
" 2>/dev/null; then
  pass "initialize: both auth methods advertised when server key is set"
else
  fail "initialize response missing expected fields: $INIT_RESP"
fi

# Create session to test model validation
RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
MODELS_SESSION=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)

# custom-grok is in the list → should succeed
RESP=$(nats_req "${PREFIX}.session.${MODELS_SESSION}.agent.set_model" \
  "{\"sessionId\":\"${MODELS_SESSION}\",\"modelId\":\"other-model\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "XAI_MODELS: model in list (other-model) accepted by set_session_model"
else
  fail "XAI_MODELS: set_session_model rejected valid model: $RESP"
fi

# grok-3 is NOT in the list (and not the default) → should fail
RESP=$(nats_req "${PREFIX}.session.${MODELS_SESSION}.agent.set_model" \
  "{\"sessionId\":\"${MODELS_SESSION}\",\"modelId\":\"grok-3\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "XAI_MODELS: model not in list (grok-3) rejected by set_session_model"
else
  fail "XAI_MODELS: set_session_model should have rejected grok-3: $RESP"
fi

stop_runner
stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [2] authenticate end-to-end (xai-api-key method) ---"
PREFIX=$(phase_prefix 2); KV_BUCKET=$(phase_bucket 2)
start_fake_xai normal
# No server-side API key → only xai-api-key method available
start_runner "XAI_API_KEY=\"\""

# authenticate with a fake key via _meta
AUTH_PAYLOAD='{"methodId":"xai-api-key","_meta":{"XAI_API_KEY":"sk-fake-test-key-1234"}}'
RESP=$(nats_req "${PREFIX}.agent.authenticate" "$AUTH_PAYLOAD")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "authenticate with xai-api-key returns success"
else
  fail "authenticate failed: $RESP"
fi

# authenticate with server key fails when no server key configured
RESP_AGENT=$(nats_req "${PREFIX}.agent.authenticate" '{"methodId":"agent"}' 2>/dev/null || echo '{"code":-1}')
if echo "$RESP_AGENT" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "authenticate with 'agent' method fails when no server key set"
else
  fail "expected error for 'agent' method with no server key: $RESP_AGENT"
fi

# authenticate with unknown method must fail
RESP_BAD=$(nats_req "${PREFIX}.agent.authenticate" '{"methodId":"bogus-method"}' 2>/dev/null || echo '{"code":-1}')
if echo "$RESP_BAD" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "authenticate with unknown method returns error"
else
  fail "expected error for unknown method: $RESP_BAD"
fi

# authenticate with empty key must fail
RESP_EMPTY=$(nats_req "${PREFIX}.agent.authenticate" '{"methodId":"xai-api-key","_meta":{"XAI_API_KEY":""}}' 2>/dev/null || echo '{"code":-1}')
if echo "$RESP_EMPTY" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "authenticate with empty key returns error"
else
  fail "expected error for empty key: $RESP_EMPTY"
fi

stop_runner
stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [3] set_session_model and set_session_config_option ---"
PREFIX=$(phase_prefix 3); KV_BUCKET=$(phase_bucket 3)
start_fake_xai normal
start_runner "XAI_API_KEY=\"sk-fake\" XAI_MODELS=\"grok-3:Grok 3,grok-3-mini:Grok 3 Mini\""

RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
SESSION_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session for config tests: $SESSION_ID"

# set_session_model
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.set_model" \
  "{\"sessionId\":\"${SESSION_ID}\",\"modelId\":\"grok-3-mini\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "set_session_model to grok-3-mini succeeds"
else
  fail "set_session_model failed: $RESP"
fi

# set_session_model to unknown model must fail
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.set_model" \
  "{\"sessionId\":\"${SESSION_ID}\",\"modelId\":\"nonexistent-model\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "set_session_model to unknown model returns error"
else
  fail "expected error for unknown model: $RESP"
fi

# set_session_config_option: enable web_search
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.set_config_option" \
  "{\"sessionId\":\"${SESSION_ID}\",\"configId\":\"web_search\",\"value\":\"on\"}")
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert 'code' not in d, f'got error: {d}'
opts = d.get('configOptions', [])
ws = next((o for o in opts if o.get('id') == 'web_search'), None)
assert ws is not None, f'web_search missing from: {opts}'
assert ws.get('currentValue') == 'on', f'expected currentValue=on but got: {ws}'
" 2>/dev/null; then
  pass "set_session_config_option web_search=on returns updated config"
else
  fail "set_session_config_option failed: $RESP"
fi

# Verify model and tool persisted via load_session
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.load" \
  "{\"sessionId\":\"${SESSION_ID}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert 'code' not in d, f'got error: {d}'
# active model should be grok-3-mini (field: currentModelId)
active = d.get('models',{}).get('currentModelId','')
assert active == 'grok-3-mini', f'expected grok-3-mini but got: {active!r}'
# web_search should be on (field: currentValue)
opts = d.get('configOptions', [])
ws = next((o for o in opts if o.get('id') == 'web_search'), None)
assert ws is not None and ws.get('currentValue') == 'on', f'web_search not on: {ws}'
" 2>/dev/null; then
  pass "model and config persisted in KV (verified via load_session)"
else
  fail "model/config not persisted: $RESP"
fi

stop_runner
stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [4] Session notifications arrive on real NATS bus ---"
PREFIX=$(phase_prefix 4); KV_BUCKET=$(phase_bucket 4)
start_fake_xai normal
start_runner "XAI_API_KEY=\"sk-fake\""

RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
NOTIF_SESSION=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session for notification test: $NOTIF_SESSION"

NOTIF_SUBJECT="${PREFIX}.session.${NOTIF_SESSION}.client.session.update"
NOTIF_FILE="$(mktemp)"

# Subscribe to notification subject in background
nats --server "$NATS_URL" subscribe "$NOTIF_SUBJECT" --count 10 \
  > "$NOTIF_FILE" 2>&1 &
NOTIF_SUB_PID=$!
sleep 0.2

# Send prompt (fake xAI returns immediately)
PROMPT_REQ="{\"sessionId\":\"${NOTIF_SESSION}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hello\"}]}"
PROMPT_RESP=$(nats_req "${PREFIX}.session.${NOTIF_SESSION}.agent.prompt" "$PROMPT_REQ" 15s)
info "prompt stop reason: $(echo "$PROMPT_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('stopReason','error'))" 2>/dev/null)"

sleep 0.5
kill "$NOTIF_SUB_PID" 2>/dev/null || true
wait "$NOTIF_SUB_PID" 2>/dev/null || true

NOTIF_CONTENT=$(cat "$NOTIF_FILE")
rm -f "$NOTIF_FILE"
info "notifications file: $(echo "$NOTIF_CONTENT" | wc -l) lines"

# nats sub prints "Received on <subject>" for each message
if echo "$NOTIF_CONTENT" | grep -qE "Received on|sessionId|sessionUpdate"; then
  pass "session notifications published to NATS subject during prompt"
else
  fail "no notifications on $NOTIF_SUBJECT (output: $NOTIF_CONTENT)"
fi

if echo "$PROMPT_RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "prompt via NATS completes successfully with fake xAI server"
else
  fail "prompt response error: $PROMPT_RESP"
fi

stop_runner
stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [5] SIGTERM during active prompt: binary exits cleanly, KV survives ---"
PREFIX=$(phase_prefix 5); KV_BUCKET=$(phase_bucket 5)
start_fake_xai hang
start_runner "XAI_API_KEY=\"sk-fake\""

RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
SIGTERM_SESSION=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
info "session for SIGTERM test: $SIGTERM_SESSION"

# Fire prompt asynchronously (hangs because fake server hangs)
PROMPT_REQ="{\"sessionId\":\"${SIGTERM_SESSION}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hello\"}]}"
nats --server "$NATS_URL" request \
  "${PREFIX}.session.${SIGTERM_SESSION}.agent.prompt" \
  "$PROMPT_REQ" --timeout 30s > /dev/null 2>&1 &
PROMPT_BG=$!
sleep 0.5  # let the prompt reach the binary

info "sending SIGTERM to runner (PID=$RUNNER_PID)"
kill -TERM "$RUNNER_PID" 2>/dev/null || true
SIGTERM_TIME=$SECONDS

# Wait for exit (max 5s)
EXITED=false
for i in $(seq 1 50); do
  if ! kill -0 "$RUNNER_PID" 2>/dev/null; then
    EXITED=true
    break
  fi
  sleep 0.1
done
ELAPSED=$((SECONDS - SIGTERM_TIME))

kill "$PROMPT_BG" 2>/dev/null || true
wait "$PROMPT_BG" 2>/dev/null || true
unset RUNNER_PID

if $EXITED; then
  pass "binary exited within ~${ELAPSED}s after SIGTERM (not hung)"
else
  fail "binary did NOT exit within 5s after SIGTERM"
  kill -9 "${RUNNER_PID:-0}" 2>/dev/null || true
fi

stop_fake_xai

# Restart with normal fake xAI — verify KV session survives (keep same API key)
start_fake_xai normal
start_runner "XAI_API_KEY=\"sk-fake\" XAI_MODELS=\"grok-3:Grok 3\""
sleep 0.2

RESP=$(nats_req "${PREFIX}.session.${SIGTERM_SESSION}.agent.load" \
  "{\"sessionId\":\"${SIGTERM_SESSION}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "session survives SIGTERM (KV not corrupted)"
else
  fail "session corrupted or missing after SIGTERM: $RESP"
fi

# Verify a new prompt works on the recovered session
RESP=$(nats_req "${PREFIX}.session.${SIGTERM_SESSION}.agent.prompt" \
  "{\"sessionId\":\"${SIGTERM_SESSION}\",\"prompt\":[{\"type\":\"text\",\"text\":\"hi again\"}]}" 15s)
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "prompt works on recovered session after restart"
else
  fail "prompt failed on recovered session: $RESP"
fi

# ════════════════════════════════════════════════════════
echo ""
echo "=== Results ==="
echo -e "  ${green}passed${reset}: $PASS"
echo -e "  ${red}failed${reset}: $FAIL"

if [[ $FAIL -gt 0 ]]; then
  exit 1
else
  echo -e "\n${green}All extended smoke tests passed.${reset}"
fi

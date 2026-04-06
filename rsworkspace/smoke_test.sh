#!/usr/bin/env bash
# Smoke test for trogon-xai-runner: exercises the running binary end-to-end
# via raw NATS request-reply without needing an xAI API key.
#
# Tests:
#   1. initialize -> capabilities
#   2. session.new -> session_id
#   3. session.list -> session appears
#   4. session.load -> state matches
#   5. session.fork -> new id, old still present
#   6. session.close -> session removed
#   7. KV persistence: restart binary, session survives
#
# Requires: nats CLI, NATS server on localhost:4222

set -euo pipefail

NATS_URL="${NATS_URL:-nats://localhost:4222}"
BINARY="./target/debug/trogon-xai-runner"
PREFIX="smoke-$$"
KV_BUCKET="SMOKE_SESSIONS_$$"
PASS=0
FAIL=0

red="\033[31m"
green="\033[32m"
yellow="\033[33m"
reset="\033[0m"

pass() { echo -e "  ${green}PASS${reset} $1"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${red}FAIL${reset} $1"; FAIL=$((FAIL + 1)); }
info() { echo -e "  ${yellow}INFO${reset} $1"; }

nats_req() {
  local subject="$1"
  local payload="$2"
  nats --server "$NATS_URL" request "$subject" "$payload" --timeout 5s 2>/dev/null
}

start_runner() {
  ACP_PREFIX="$PREFIX" \
  XAI_SESSION_BUCKET="$KV_BUCKET" \
  XAI_API_KEY="" \
  NATS_URL="$NATS_URL" \
    "$BINARY" &
  RUNNER_PID=$!
  sleep 0.5  # let it connect
}

stop_runner() {
  if [[ -n "${RUNNER_PID:-}" ]]; then
    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
    unset RUNNER_PID
  fi
}

cleanup() {
  stop_runner
  # Delete KV bucket
  nats --server "$NATS_URL" kv del "$KV_BUCKET" --force 2>/dev/null || true
}

trap cleanup EXIT

echo ""
echo "=== trogon-xai-runner smoke test ==="
echo "binary : $BINARY"
echo "prefix : $PREFIX"
echo "bucket : $KV_BUCKET"
echo "nats   : $NATS_URL"
echo ""

# ── 1. Start runner ──────────────────────────────────────────────────────────
echo "--- Phase 1: ACP protocol flow ---"
start_runner
info "runner PID=$RUNNER_PID"

# ── 2. initialize ────────────────────────────────────────────────────────────
echo ""
echo "[1] initialize"
RESP=$(nats_req "${PREFIX}.agent.initialize" '{"protocolVersion":1}')
if echo "$RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'protocolVersion' in d" 2>/dev/null; then
  pass "protocol_version present in response"
else
  fail "unexpected initialize response: $RESP"
fi

# ── 3. session.new ───────────────────────────────────────────────────────────
echo ""
echo "[2] session.new"
RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
SESSION_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
if [[ -n "$SESSION_ID" ]]; then
  pass "got session_id: $SESSION_ID"
else
  fail "no session_id in response: $RESP"
  SESSION_ID=""
fi

if [[ -z "$SESSION_ID" ]]; then
  echo "Cannot continue without a session_id"
  exit 1
fi

# ── 4. session.list ──────────────────────────────────────────────────────────
echo ""
echo "[3] session.list"
RESP=$(nats_req "${PREFIX}.agent.session.list" '{}')
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids=[s['sessionId'] for s in d.get('sessions',[])]
assert '$SESSION_ID' in ids, f'$SESSION_ID not in {ids}'
" 2>/dev/null; then
  pass "session_id appears in list"
else
  fail "session_id missing from list: $RESP"
fi

# ── 5. session.load ──────────────────────────────────────────────────────────
echo ""
echo "[4] session.load"
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.load" \
  "{\"sessionId\":\"${SESSION_ID}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert 'code' not in d, f'got error: {d}'
" 2>/dev/null; then
  pass "load returns success (no error)"
else
  fail "load response error: $RESP"
fi

# ── 6. session.fork ──────────────────────────────────────────────────────────
echo ""
echo "[5] session.fork"
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.fork" \
  "{\"sessionId\":\"${SESSION_ID}\",\"cwd\":\"/tmp\"}")
FORK_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
if [[ -n "$FORK_ID" && "$FORK_ID" != "$SESSION_ID" ]]; then
  pass "fork returned new session_id: $FORK_ID"
else
  fail "fork response unexpected: $RESP"
  FORK_ID=""
fi

# Both sessions should appear in list
RESP=$(nats_req "${PREFIX}.agent.session.list" '{}')
BOTH=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids=[s['sessionId'] for s in d.get('sessions',[])]
print(('$SESSION_ID' in ids) and ('$FORK_ID' in ids))
" 2>/dev/null)
if [[ "$BOTH" == "True" ]]; then
  pass "both original and fork appear in list"
else
  fail "one or both sessions missing after fork: $RESP"
fi

# ── 7. session.close ─────────────────────────────────────────────────────────
echo ""
echo "[6] session.close"
nats_req "${PREFIX}.session.${SESSION_ID}.agent.close" \
  "{\"sessionId\":\"${SESSION_ID}\"}" > /dev/null
RESP=$(nats_req "${PREFIX}.agent.session.list" '{}')
CLOSED_GONE=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids=[s['sessionId'] for s in d.get('sessions',[])]
print('$SESSION_ID' not in ids and '$FORK_ID' in ids)
" 2>/dev/null)
if [[ "$CLOSED_GONE" == "True" ]]; then
  pass "closed session removed, fork still present"
else
  fail "after close, list unexpected: $RESP"
fi

# ── 8. KV persistence across restart ─────────────────────────────────────────
echo ""
echo "--- Phase 2: KV persistence ---"

# Use fork session (still alive) to test persistence
PERSIST_ID="$FORK_ID"
info "testing persistence for session $PERSIST_ID"

stop_runner
sleep 0.3
info "runner stopped"

start_runner
info "runner restarted with PID=$RUNNER_PID"

RESP=$(nats_req "${PREFIX}.agent.session.list" '{}')
SURVIVED=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
ids=[s['sessionId'] for s in d.get('sessions',[])]
print('$PERSIST_ID' in ids)
" 2>/dev/null)
if [[ "$SURVIVED" == "True" ]]; then
  pass "session survived binary restart (KV persistence works)"
else
  fail "session lost after restart: $RESP"
fi

# Also load it
RESP=$(nats_req "${PREFIX}.session.${PERSIST_ID}.agent.load" \
  "{\"sessionId\":\"${PERSIST_ID}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
assert 'code' not in d, f'got error: {d}'
" 2>/dev/null; then
  pass "load after restart returns success"
else
  fail "load after restart failed: $RESP"
fi

# ── 9. Summary ───────────────────────────────────────────────────────────────
echo ""
echo "=== Results ==="
echo -e "  ${green}passed${reset}: $PASS"
if [[ $FAIL -gt 0 ]]; then
  echo -e "  ${red}failed${reset}: $FAIL"
  exit 1
else
  echo -e "  ${red}failed${reset}: $FAIL"
  echo -e "\n${green}All smoke tests passed.${reset}"
fi

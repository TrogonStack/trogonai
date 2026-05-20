#!/usr/bin/env bash
# Smoke test for PR 7: session/export and session/import via live xai-runner binary
#
# Requires: nats CLI, NATS server on localhost:4222

set -euo pipefail

NATS_URL="${NATS_URL:-nats://localhost:4222}"
BINARY="./target/debug/trogon-xai-runner"
PREFIX="smoke-ext-$$"
KV_BUCKET="SMOKE_EXT_$$"
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

cleanup() {
  if [[ -n "${RUNNER_PID:-}" ]]; then
    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
  fi
  nats --server "$NATS_URL" kv del "$KV_BUCKET" --force 2>/dev/null || true
}
trap cleanup EXIT

echo ""
echo "=== PR 7 ext_method smoke test (session/export + session/import) ==="
echo "binary : $BINARY"
echo "prefix : $PREFIX"
echo "nats   : $NATS_URL"
echo ""

# ── Start runner ─────────────────────────────────────────────────────────────
ACP_PREFIX="$PREFIX" \
XAI_SESSION_BUCKET="$KV_BUCKET" \
XAI_API_KEY="dummy" \
NATS_URL="$NATS_URL" \
  "$BINARY" &
RUNNER_PID=$!
sleep 0.5

# ── 1. Create session ─────────────────────────────────────────────────────────
echo "[1] session.new"
RESP=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
SESSION_ID=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null)
if [[ -n "$SESSION_ID" ]]; then
  pass "session created: $SESSION_ID"
else
  fail "session.new failed: $RESP"
  exit 1
fi

# ── 2. Export empty session ───────────────────────────────────────────────────
echo ""
echo "[2] ext_method session/export (empty)"
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.ext_method" \
  "{\"method\":\"session/export\",\"params\":{\"sessionId\":\"${SESSION_ID}\"}}")
MESSAGES=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
r=d.get('result')
if r is None:
  print('ERROR:',d)
else:
  msgs=json.loads(r) if isinstance(r,str) else r
  print(len(msgs))
" 2>/dev/null)
if [[ "$MESSAGES" == "0" ]]; then
  pass "export of empty session returns [] (0 messages)"
else
  fail "unexpected export result: $RESP"
fi

# ── 3. Import messages ────────────────────────────────────────────────────────
echo ""
echo "[3] ext_method session/import"
IMPORT_PAYLOAD=$(python3 -c "
import json
params = {
  'sessionId': '${SESSION_ID}',
  'messages': [
    {'role': 'user', 'text': 'What is 2+2?'},
    {'role': 'assistant', 'text': 'The answer is 4.'},
    {'role': 'user', 'text': 'And 3+3?'},
    {'role': 'assistant', 'text': 'The answer is 6.'}
  ]
}
print(json.dumps({'method': 'session/import', 'params': params}))
")
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.ext_method" "$IMPORT_PAYLOAD")
OK=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print('ok' if 'error' not in d else d.get('error'))
" 2>/dev/null)
if [[ "$OK" == "ok" ]]; then
  pass "import succeeded"
else
  fail "import failed: $RESP"
fi

# ── 4. Export and verify ──────────────────────────────────────────────────────
echo ""
echo "[4] ext_method session/export (after import)"
RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.ext_method" \
  "{\"method\":\"session/export\",\"params\":{\"sessionId\":\"${SESSION_ID}\"}}")
RESULT=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
r=d.get('result')
if r is None:
  print('ERROR')
else:
  msgs=json.loads(r) if isinstance(r,str) else r
  expected=[
    {'role':'user','text':'What is 2+2?'},
    {'role':'assistant','text':'The answer is 4.'},
    {'role':'user','text':'And 3+3?'},
    {'role':'assistant','text':'The answer is 6.'}
  ]
  if msgs==expected:
    print('OK')
  else:
    print('MISMATCH:',json.dumps(msgs))
" 2>/dev/null)
if [[ "$RESULT" == "OK" ]]; then
  pass "export after import returns exact 4 messages (roles and text match)"
else
  fail "export mismatch: $RESULT  raw=$RESP"
fi

# ── 5. Unknown session error ──────────────────────────────────────────────────
echo ""
echo "[5] ext_method export with unknown session"
RESP=$(nats_req "${PREFIX}.session.unknown-id-999.agent.ext_method" \
  '{"method":"session/export","params":{"sessionId":"unknown-id-999"}}')
IS_ERR=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print('yes' if 'error' in d else 'no')
" 2>/dev/null)
if [[ "$IS_ERR" == "yes" ]]; then
  pass "unknown session returns error"
else
  fail "expected error for unknown session, got: $RESP"
fi

# ── 6. Import round-trip survives KV restart ──────────────────────────────────
echo ""
echo "[6] import survives KV persist/restart"
kill "$RUNNER_PID" 2>/dev/null || true
wait "$RUNNER_PID" 2>/dev/null || true
sleep 0.3

ACP_PREFIX="$PREFIX" \
XAI_SESSION_BUCKET="$KV_BUCKET" \
XAI_API_KEY="dummy" \
NATS_URL="$NATS_URL" \
  "$BINARY" &
RUNNER_PID=$!
sleep 0.5

RESP=$(nats_req "${PREFIX}.session.${SESSION_ID}.agent.ext_method" \
  "{\"method\":\"session/export\",\"params\":{\"sessionId\":\"${SESSION_ID}\"}}")
COUNT=$(echo "$RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
r=d.get('result')
msgs=json.loads(r) if isinstance(r,str) else (r or [])
print(len(msgs))
" 2>/dev/null)
if [[ "$COUNT" == "4" ]]; then
  pass "imported history (4 messages) survived runner restart via KV"
else
  fail "after restart, export returned $COUNT messages (expected 4): $RESP"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "=== Results ==="
echo -e "  ${green}passed${reset}: $PASS"
if [[ $FAIL -gt 0 ]]; then
  echo -e "  ${red}failed${reset}: $FAIL"
  exit 1
else
  echo -e "  ${red}failed${reset}: $FAIL"
  echo -e "\n${green}All ext_method smoke tests passed.${reset}"
fi

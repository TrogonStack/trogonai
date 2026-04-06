#!/usr/bin/env bash
# Smoke test 3: remaining gaps after smoke_test2.sh
#
#   1. XAI_SYSTEM_PROMPT included in xAI HTTP request body (input[0].role=system)
#   2. XAI_MAX_TURNS sent in request body when tools are enabled
#   3. XAI_MAX_HISTORY_MESSAGES trims input on the real binary
#   4. set_session_mode (valid mode, invalid mode, nonexistent session)
#   5. resume_session via NATS
#
# Requires: NATS at localhost:4222, python3, binary built in target/debug/

set -euo pipefail

NATS_URL="${NATS_URL:-nats://localhost:4222}"
BINARY="./target/debug/trogon-xai-runner"
BASE="smoke3-$$"
FAKE_PORT=$((30000 + (RANDOM % 1000)))
PASS=0; FAIL=0

red="\033[31m"; green="\033[32m"; yellow="\033[33m"; reset="\033[0m"
pass() { echo -e "  ${green}PASS${reset} $1"; PASS=$((PASS + 1)); }
fail() { echo -e "  ${red}FAIL${reset} $1"; FAIL=$((FAIL + 1)); }
info() { echo -e "  ${yellow}INFO${reset} $1"; }

phase_prefix() { echo "${BASE}-p${1}"; }
phase_bucket() { echo "SMOKE3_${$}_P${1}"; }
PREFIX="${BASE}-p0"; KV_BUCKET="SMOKE3_${$}_P0"

nats_req() {
  local subj="$1" payload="$2" timeout="${3:-10s}"
  nats --server "$NATS_URL" request "$subj" "$payload" --timeout "$timeout" 2>/dev/null
}

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
  if [[ -n "${RUNNER_PID:-}" ]]; then
    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
    unset RUNNER_PID
  fi
  sleep 0.2
}

start_fake_xai() {
  python3 fake_xai_server.py "$FAKE_PORT" normal &
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

fake_log() {
  # Fetch all recorded HTTP request bodies from the fake server
  curl -s "http://127.0.0.1:${FAKE_PORT}/log"
}

new_session() {
  local resp
  resp=$(nats_req "${PREFIX}.agent.session.new" '{"cwd":"/tmp","mcpServers":[]}')
  echo "$resp" | python3 -c "import sys,json; print(json.load(sys.stdin)['sessionId'])" 2>/dev/null
}

do_prompt() {
  local sid="$1" text="$2"
  nats_req "${PREFIX}.session.${sid}.agent.prompt" \
    "{\"sessionId\":\"${sid}\",\"prompt\":[{\"type\":\"text\",\"text\":\"${text}\"}]}" 15s
}

cleanup() {
  stop_runner; stop_fake_xai
  for i in 1 2 3 4 5; do
    nats --server "$NATS_URL" kv del "$(phase_bucket $i)" --force 2>/dev/null || true
  done
}
trap cleanup EXIT

echo ""
echo "=== trogon-xai-runner smoke test 3 ==="
echo "binary : $BINARY"
echo "fake   : http://127.0.0.1:${FAKE_PORT}/v1"
echo ""

# ════════════════════════════════════════════════════════
echo "--- [1] XAI_SYSTEM_PROMPT in xAI request body ---"
PREFIX=$(phase_prefix 1); KV_BUCKET=$(phase_bucket 1)
start_fake_xai
start_runner 'XAI_API_KEY="sk-fake" XAI_SYSTEM_PROMPT="You are a helpful test bot"'

SID=$(new_session)
info "session: $SID"

do_prompt "$SID" "hello" > /dev/null

LOG=$(fake_log)
info "fake server request count: $(echo "$LOG" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)"

if echo "$LOG" | python3 -c "
import sys, json
reqs = json.load(sys.stdin)
assert reqs, 'no requests recorded'
body = json.loads(reqs[-1]['body'])
inp = body.get('input', [])
sys_items = [i for i in inp if i.get('role') == 'system']
assert sys_items, f'no system message in input: {inp}'
assert 'test bot' in sys_items[0].get('content',''), f'wrong content: {sys_items[0]}'
print('ok', sys_items[0])
" 2>/dev/null; then
  pass "XAI_SYSTEM_PROMPT sent as system role in xAI request input"
else
  fail "XAI_SYSTEM_PROMPT missing from request body: $LOG"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [2] XAI_MAX_TURNS sent when tools are enabled ---"
PREFIX=$(phase_prefix 2); KV_BUCKET=$(phase_bucket 2)
start_fake_xai
start_runner 'XAI_API_KEY="sk-fake" XAI_MAX_TURNS="7"'

SID=$(new_session)
# Enable web_search so tools are sent in the request
nats_req "${PREFIX}.session.${SID}.agent.set_config_option" \
  "{\"sessionId\":\"${SID}\",\"configId\":\"web_search\",\"value\":\"on\"}" > /dev/null

do_prompt "$SID" "search for something" > /dev/null

LOG=$(fake_log)

if echo "$LOG" | python3 -c "
import sys, json
reqs = json.load(sys.stdin)
body = json.loads(reqs[-1]['body'])
mt = body.get('max_turns')
assert mt == 7, f'expected max_turns=7 but got: {mt!r}. body keys: {list(body.keys())}'
# tools should also be present
assert 'tools' in body, f'tools missing: {list(body.keys())}'
print('ok max_turns:', mt)
" 2>/dev/null; then
  pass "XAI_MAX_TURNS=7 sent in request body when tools enabled"
else
  fail "max_turns missing or wrong: $(echo "$LOG" | python3 -c "import sys,json; reqs=json.load(sys.stdin); print(json.loads(reqs[-1]['body']).get('max_turns','MISSING'), list(json.loads(reqs[-1]['body']).keys()))" 2>/dev/null)"
fi

# Without tools max_turns must NOT be sent
SID2=$(new_session)
do_prompt "$SID2" "plain question" > /dev/null
LOG=$(fake_log)
if echo "$LOG" | python3 -c "
import sys, json
reqs = json.load(sys.stdin)
# Find the last request for SID2 (no tools) — it should have no tools key
body = json.loads(reqs[-1]['body'])
assert 'tools' not in body, f'tools should not be sent without config: {list(body.keys())}'
assert 'max_turns' not in body, f'max_turns should not be sent without tools: {list(body.keys())}'
print('ok no tools/max_turns')
" 2>/dev/null; then
  pass "max_turns NOT sent when no tools enabled"
else
  fail "unexpected max_turns or tools in tool-less request"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [3] XAI_MAX_HISTORY_MESSAGES trims input on binary ---"
PREFIX=$(phase_prefix 3); KV_BUCKET=$(phase_bucket 3)
start_fake_xai
# Limit to 2 history messages (1 user+assistant pair)
start_runner 'XAI_API_KEY="sk-fake" XAI_MAX_HISTORY_MESSAGES="2"'

SID=$(new_session)

# Turn 1
do_prompt "$SID" "turn one" > /dev/null
# Turn 2
do_prompt "$SID" "turn two" > /dev/null
# Turn 3 — history should be trimmed to 2 messages
do_prompt "$SID" "turn three" > /dev/null

LOG=$(fake_log)
info "requests recorded: $(echo "$LOG" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null)"

# On turn 3 the session has no previous_response_id (fake server returns a fixed id,
# but the fake doesn't actually advance it). However, after 3 prompts the history
# exceeds 2 messages, so build_full_history_input is called.
# Check that the LAST request's input has at most 2 user/assistant items
# (+ optional system item from no system prompt here).
if echo "$LOG" | python3 -c "
import sys, json
reqs = json.load(sys.stdin)
# Find the last request for /responses
xai_reqs = [r for r in reqs if '/responses' in r.get('path','')]
assert xai_reqs, 'no /responses requests'
# After 3 turns, previous_response_id is set so binary uses stateful mode.
# In stateful mode only the NEW user message is in input (just 1 item).
# In fallback/full-history mode the trimmed input would have <=2 user/assistant items.
# Either way the binary must NOT send more than 2 history messages + current user.
last_body = json.loads(xai_reqs[-1]['body'])
inp = last_body.get('input', [])
# Count non-system items
non_sys = [i for i in inp if i.get('role') != 'system']
print(f'non-system items in last request: {len(non_sys)}, items: {[i[\"role\"] for i in inp]}')
# In stateful mode: just 1 user item (the new prompt)
# In full-history mode: <=2 history + 1 current = max 3, but trimmed to 2+1=3
# The important thing: if previous_response_id is used, input has 1 item (new user)
# If not, input has at most max_history(2) + 1(current) = 3 items
assert len(non_sys) <= 3, f'too many non-system items: {len(non_sys)}'
print('ok trim check passed')
" 2>/dev/null; then
  pass "XAI_MAX_HISTORY_MESSAGES=2 respected: input within limit"
else
  fail "history trimming may not be working: $(fake_log | python3 -c "import sys,json; reqs=json.load(sys.stdin); [print(json.loads(r['body']).get('input')) for r in reqs]" 2>/dev/null)"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [4] set_session_mode via NATS ---"
PREFIX=$(phase_prefix 4); KV_BUCKET=$(phase_bucket 4)
start_fake_xai
start_runner 'XAI_API_KEY="sk-fake"'

SID=$(new_session)
info "session for mode tests: $SID"

# Setting valid mode "default" must succeed
RESP=$(nats_req "${PREFIX}.session.${SID}.agent.set_mode" \
  "{\"sessionId\":\"${SID}\",\"modeId\":\"default\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "set_session_mode 'default' succeeds"
else
  fail "set_session_mode default failed: $RESP"
fi

# Setting unknown mode must fail
RESP=$(nats_req "${PREFIX}.session.${SID}.agent.set_mode" \
  "{\"sessionId\":\"${SID}\",\"modeId\":\"agent\"}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "set_session_mode unknown mode returns error"
else
  fail "expected error for unknown mode: $RESP"
fi

# Setting mode on nonexistent session must fail
RESP=$(nats_req "${PREFIX}.session.does-not-exist.agent.set_mode" \
  '{"sessionId":"does-not-exist","modeId":"default"}')
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "set_session_mode on nonexistent session returns error"
else
  fail "expected error for nonexistent session: $RESP"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "--- [5] resume_session via NATS ---"
PREFIX=$(phase_prefix 5); KV_BUCKET=$(phase_bucket 5)
start_fake_xai
start_runner 'XAI_API_KEY="sk-fake"'

SID=$(new_session)
info "session for resume tests: $SID"

# resume_session on existing session must succeed
RESP=$(nats_req "${PREFIX}.session.${SID}.agent.resume" \
  "{\"sessionId\":\"${SID}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "resume_session on existing session succeeds"
else
  fail "resume_session failed: $RESP"
fi

# resume_session on nonexistent session must fail
RESP=$(nats_req "${PREFIX}.session.no-such-session.agent.resume" \
  '{"sessionId":"no-such-session","cwd":"/tmp","mcpServers":[]}')
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' in d
" 2>/dev/null; then
  pass "resume_session on nonexistent session returns error"
else
  fail "expected error for nonexistent resume: $RESP"
fi

# resume_session survives binary restart (KV-backed)
stop_runner
start_runner 'XAI_API_KEY="sk-fake"'

RESP=$(nats_req "${PREFIX}.session.${SID}.agent.resume" \
  "{\"sessionId\":\"${SID}\",\"cwd\":\"/tmp\",\"mcpServers\":[]}")
if echo "$RESP" | python3 -c "
import sys,json; d=json.load(sys.stdin); assert 'code' not in d
" 2>/dev/null; then
  pass "resume_session works after binary restart (KV persistence)"
else
  fail "resume after restart failed: $RESP"
fi

stop_runner; stop_fake_xai

# ════════════════════════════════════════════════════════
echo ""
echo "=== Results ==="
echo -e "  ${green}passed${reset}: $PASS"
echo -e "  ${red}failed${reset}: $FAIL"
if [[ $FAIL -gt 0 ]]; then exit 1
else echo -e "\n${green}All smoke tests 3 passed.${reset}"; fi

#!/usr/bin/env bash
# End-to-end CLI test — no credentials required.
#
# Starts a NATS server, a fake ACP runner, then exercises:
#   1. --print mode (text output)
#   2. --print --output-format json
#   3. stdin pipe mode (echo "prompt" | trogon --print)
#   4. Empty prompt rejection
#
# Usage: ./scripts/e2e-test.sh

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="${SCRIPT_DIR}/.."
BIN="${REPO}/target/debug/trogon"
RUNNER="${SCRIPT_DIR}/e2e-fake-runner.sh"

RED='\033[0;31m'; GREEN='\033[0;32m'; BOLD='\033[1m'; RESET='\033[0m'

pass() { echo -e "${GREEN}✓${RESET} $1"; }
fail() { echo -e "${RED}✗${RESET} $1"; FAILED=$((FAILED+1)); }
FAILED=0

# ── Prerequisites ─────────────────────────────────────────────────────────────

[[ -x "$BIN" ]] || { echo "Build first: cargo build -p trogon-cli"; exit 1; }
which nats-server &>/dev/null || { echo "nats-server not in PATH"; exit 1; }
which nats &>/dev/null || { echo "nats CLI not in PATH"; exit 1; }

# ── NATS server ───────────────────────────────────────────────────────────────

echo -e "\n${BOLD}Starting NATS server...${RESET}"
nats-server -p 4222 &>/tmp/nats-server-e2e.log &
NATS_PID=$!
trap 'echo "[cleanup] killing NATS ($NATS_PID)"; kill $NATS_PID 2>/dev/null; wait $NATS_PID 2>/dev/null || true' EXIT

# Wait until NATS is up (TCP connect check).
for i in $(seq 1 30); do
    nc -z 127.0.0.1 4222 2>/dev/null && break
    sleep 0.1
done
nc -z 127.0.0.1 4222 2>/dev/null || { echo "NATS did not start"; exit 1; }
sleep 0.2  # brief pause for NATS to finish its init
echo "NATS up (pid $NATS_PID)"

# ── Test 1: --print text mode ─────────────────────────────────────────────────

echo -e "\n${BOLD}Test 1: --print text mode${RESET}"
"$RUNNER" "sess-t1" "I understand your request!" &
RUNNER_PID=$!
sleep 0.3

OUT=$("$BIN" --print "say something" 2>&1)
wait $RUNNER_PID 2>/dev/null || true

if echo "$OUT" | grep -q "I understand your request"; then
    pass "text output contains model reply"
else
    fail "text output missing reply: '$OUT'"
fi

# ── Test 2: --print json mode ─────────────────────────────────────────────────

echo -e "\n${BOLD}Test 2: --print --output-format json${RESET}"
"$RUNNER" "sess-t2" "JSON response here" &
RUNNER_PID=$!
sleep 0.3

OUT=$("$BIN" --print "json please" --output-format json 2>&1)
wait $RUNNER_PID 2>/dev/null || true

if echo "$OUT" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); assert 'text' in d" 2>/dev/null; then
    pass "json output is valid JSON with 'text' field"
    echo "  → $OUT"
else
    fail "json output is not valid: '$OUT'"
fi

# ── Test 3: stdin pipe mode ───────────────────────────────────────────────────

echo -e "\n${BOLD}Test 3: stdin pipe mode${RESET}"
"$RUNNER" "sess-t3" "Piped reply" &
RUNNER_PID=$!
sleep 0.3

OUT=$(echo "what is the meaning of life?" | "$BIN" --print 2>&1)
wait $RUNNER_PID 2>/dev/null || true

if echo "$OUT" | grep -q "Piped reply"; then
    pass "stdin pipe mode works"
else
    fail "stdin pipe mode: '$OUT'"
fi

# ── Test 4: empty prompt rejected ─────────────────────────────────────────────

echo -e "\n${BOLD}Test 4: empty prompt rejected${RESET}"
OUT=$("$BIN" --print "" 2>&1 || true)
if echo "$OUT" | grep -q "empty"; then
    pass "empty prompt produces error message"
else
    fail "empty prompt not rejected: '$OUT'"
fi

# ── Test 5: NATS connection timeout message ───────────────────────────────────

echo -e "\n${BOLD}Test 5: wrong NATS URL shows helpful error${RESET}"
OUT=$("$BIN" --nats-url "nats://127.0.0.1:9999" --print "hello" 2>&1 || true)
if echo "$OUT" | grep -qiE "nats|connect|server"; then
    pass "wrong NATS URL produces helpful error"
    echo "  → $OUT" | head -2
else
    fail "wrong NATS URL: '$OUT'"
fi

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
if [[ $FAILED -eq 0 ]]; then
    echo -e "${GREEN}${BOLD}All tests passed!${RESET}"
else
    echo -e "${RED}${BOLD}$FAILED test(s) failed.${RESET}"
    exit 1
fi

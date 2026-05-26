#!/usr/bin/env bash
# Fake ACP runner for end-to-end CLI testing (no credentials needed).
#
# Usage: ./scripts/e2e-fake-runner.sh [session-id] [text-reply]
#
# Protocol (B6): the CLI publishes the prompt with an `X-Req-Id` header (it does
# NOT set a NATS reply-to) and listens for the terminal result on
#   {prefix}.session.{sid}.agent.prompt.response.{req_id}
# So this runner:
#   1. replies to session.new with the session id,
#   2. subscribes to the prompt subject and reads the `X-Req-Id` header,
#   3. publishes the agent_message_chunk notification, and
#   4. publishes {"stopReason":"end_turn"} to the per-request response subject.

set -euo pipefail

SID="${1:-fake-sess-1}"
REPLY="${2:-Hello! I am a fake AI runner. No credentials needed.}"
PREFIX="${ACP_PREFIX:-acp}"
NOTIF_SUBJ="${PREFIX}.session.${SID}.client.session.update"
PROMPT_SUBJ="${PREFIX}.session.${SID}.agent.prompt"
SERVER="${NATS_URL:-nats://127.0.0.1:4222}"

CAP_FILE=$(mktemp /tmp/fake-prompt-XXXXXX.txt)
trap 'rm -f "$CAP_FILE"' EXIT

NOTIF_PAYLOAD="{\"sessionId\":\"${SID}\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"${REPLY}\"}}}"
DONE_PAYLOAD='{"stopReason":"end_turn"}'

echo "[fake-runner] session_id=${SID} prefix=${PREFIX}"

# 1. Reply to session.new.
nats --server "$SERVER" reply "${PREFIX}.agent.session.new" \
     "{\"sessionId\":\"${SID}\"}" \
     --count=1 &
SESSION_PID=$!

# 2. Subscribe to the prompt subject (capturing headers) and handle it.
(
    # Default `nats sub` output renders header lines like "X-Req-Id: <value>".
    nats --server "$SERVER" sub "${PROMPT_SUBJ}" --count=1 > "$CAP_FILE" 2>&1

    # Extract the request id from the X-Req-Id header line.
    REQ_ID=$(grep -i '^X-Req-Id:' "$CAP_FILE" | head -1 | sed 's/^[Xx]-[Rr]eq-[Ii]d:[[:space:]]*//')
    if [[ -z "$REQ_ID" ]]; then
        echo "[fake-runner] ERROR: no X-Req-Id header in prompt message" >&2
        cat "$CAP_FILE" >&2
        exit 1
    fi
    RESP_SUBJ="${PREFIX}.session.${SID}.agent.prompt.response.${REQ_ID}"

    # 3. Publish the streaming notification chunk.
    nats --server "$SERVER" pub "${NOTIF_SUBJ}" "${NOTIF_PAYLOAD}"
    sleep 0.05
    # 4. Publish the terminal result to the per-request response subject.
    nats --server "$SERVER" pub "${RESP_SUBJ}" "${DONE_PAYLOAD}"
) &
PROMPT_PID=$!

wait $SESSION_PID $PROMPT_PID
echo "[fake-runner] done"

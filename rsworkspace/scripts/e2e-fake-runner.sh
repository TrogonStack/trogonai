#!/usr/bin/env bash
# Fake ACP runner for end-to-end CLI testing (no credentials needed).
#
# Usage: ./scripts/e2e-fake-runner.sh [session-id] [text-reply]

set -euo pipefail

SID="${1:-fake-sess-1}"
REPLY="${2:-Hello! I am a fake AI runner. No credentials needed.}"
PREFIX="acp"
NOTIF_SUBJ="${PREFIX}.session.${SID}.client.session.update"
PROMPT_SUBJ="${PREFIX}.session.${SID}.agent.prompt"

# Write payloads to temp files to avoid nested-quote hell.
NOTIF_FILE=$(mktemp /tmp/fake-notif-XXXXXX.json)
DONE_FILE=$(mktemp /tmp/fake-done-XXXXXX.json)
HELPER=$(mktemp /tmp/fake-prompt-cmd-XXXXXX.sh)
trap 'rm -f "$NOTIF_FILE" "$DONE_FILE" "$HELPER"' EXIT

cat > "$NOTIF_FILE" <<EOF
{"sessionId":"${SID}","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"${REPLY}"}}}
EOF

cat > "$DONE_FILE" <<'EOF'
{"stopReason":"end_turn"}
EOF

# The helper script: publish notification, wait, then print Done to stdout
# (nats reply --command uses stdout as the reply body).
cat > "$HELPER" <<EOF
#!/usr/bin/env bash
nats pub '${NOTIF_SUBJ}' "\$(cat '${NOTIF_FILE}')"
sleep 0.05
cat '${DONE_FILE}'
EOF
chmod +x "$HELPER"

echo "[fake-runner] session_id=${SID}"

# 1. Reply to session.new.
nats reply "${PREFIX}.agent.session.new" \
     "{\"sessionId\":\"${SID}\"}" \
     --count=1 &
SESSION_PID=$!

# 2. Reply to prompt: run helper (publishes notification + echoes Done).
nats reply "${PROMPT_SUBJ}" \
     --command "${HELPER}" \
     --count=1 &
PROMPT_PID=$!

wait $SESSION_PID $PROMPT_PID
echo "[fake-runner] done"

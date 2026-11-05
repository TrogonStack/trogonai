#!/bin/sh
# Runs provision-audit.sh against a stand-in `nats` that keeps each stream's
# configuration in a file, so it checks what the script creates and what it
# refuses without a server.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
mkdir "$workdir/bin" "$workdir/streams"

cat > "$workdir/bin/nats" <<'STUB'
#!/bin/sh
# nats req '$JS.API.STREAM.INFO.NAME' '' --raw, and nats stream add NAME ...
set -eu
case "$1" in
  req)
    [ ! -f "$STUB_DIR/unreachable" ] || { echo "nats: error: setup failed" >&2; exit 1; }
    [ ! -f "$STUB_DIR/api-error" ] || { cat "$STUB_DIR/api-error"; exit 0; }
    name=${2##*.}
    if [ -f "$STUB_DIR/streams/$name.json" ]; then
      cat "$STUB_DIR/streams/$name.json"
    else
      echo '{"error":{"code":404,"err_code":10059,"description":"stream not found"}}'
    fi
    ;;
  stream)
    name=$3
    echo "$name $*" >> "$STUB_DIR/added"
    cp "$STUB_DIR/created-$name.json" "$STUB_DIR/streams/$name.json"
    ;;
esac
STUB
chmod +x "$workdir/bin/nats"

stream_json() {
  jq -n --arg name "$1" --argjson subjects "$2" '{config: {
    name: $name, subjects: $subjects, storage: "file", retention: "limits", discard: "new",
    deny_delete: true, deny_purge: true, max_age: 0, max_msgs_per_subject: -1,
    duplicate_window: 120000000000, num_replicas: 1}}'
}

audit_subjects='["platform.v1.audit.decision.>","platform.v1.audit.credential.>","platform.v1.audit.change.>","platform.v1.audit.integrity.>"]'
checkpoint_subjects='["platform.v1.audit.checkpoint.>"]'

reset() {
  rm -rf "$workdir/streams" "$workdir/added" "$workdir/unreachable" "$workdir/api-error"
  mkdir "$workdir/streams"
}

provision() {
  status=0
  PATH="$workdir/bin:$PATH" STUB_DIR="$workdir" sh "$here/provision-audit.sh" > "$workdir/out" 2> "$workdir/err" || status=$?
}

fail() {
  echo "$1" >&2
  exit 1
}

# Both streams are created, in full, when they are missing.
reset
stream_json AUDIT "$audit_subjects" > "$workdir/created-AUDIT.json"
stream_json AUDIT_CHECKPOINT "$checkpoint_subjects" > "$workdir/created-AUDIT_CHECKPOINT.json"
provision
[ "$status" -eq 0 ] || fail "a missing stream was not created: $(cat "$workdir/err")"
[ "$(cut -d' ' -f1 "$workdir/added" | tr '\n' ' ')" = "AUDIT AUDIT_CHECKPOINT " ] || fail "expected both streams to be added"
for flag in "--storage file" "--retention limits" "--discard new" "--deny-delete" "--deny-purge" "--ack" "--max-age=-1" "--dupe-window 2m" "--replicas 1"; do
  grep -q -- "$flag" "$workdir/added" || fail "the created streams lack $flag"
done
grep -q "platform.v1.audit.checkpoint.>" "$workdir/added" || fail "AUDIT_CHECKPOINT lacks its subject"

# Running again adds nothing.
rm "$workdir/added"
provision
[ "$status" -eq 0 ] || fail "a second run failed: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a second run created a stream again"

# Each way an existing stream could weaken the trail is refused, naming the field.
refuses() {
  fragment=$1 edit=$2
  reset
  stream_json AUDIT "$audit_subjects" | jq "$edit" > "$workdir/streams/AUDIT.json"
  stream_json AUDIT_CHECKPOINT "$checkpoint_subjects" > "$workdir/streams/AUDIT_CHECKPOINT.json"
  provision
  [ "$status" -ne 0 ] || fail "a stream that $fragment was accepted"
  grep -q -- "$fragment" "$workdir/err" || fail "the refusal does not name \"$fragment\": $(cat "$workdir/err")"
  [ ! -f "$workdir/added" ] || fail "a refused stream was replaced"
}

refuses "keeps storage memory" '.config.storage = "memory"'
refuses "keeps retention interest" '.config.retention = "interest"'
refuses "keeps discard old" '.config.discard = "old"'
refuses "requires deny_delete" '.config.deny_delete = false'
refuses "requires deny_purge" '.config.deny_purge = false'
refuses "publishes without acknowledging" '.config.no_ack = true'
refuses "expires records by max age" '.config.max_age = 3600000000000'
refuses "limits the records kept per subject" '.config.max_msgs_per_subject = 1'
refuses "requires message TTL off" '.config.allow_msg_ttl = true'
refuses "keeps a duplicate window under 2 minutes" '.config.duplicate_window = 30000000000'
refuses "lacks subject platform.v1.audit.change.>" '.config.subjects |= map(select(. != "platform.v1.audit.change.>"))'

# A stream with more replicas than asked for is stronger, not weaker.
reset
stream_json AUDIT "$audit_subjects" | jq '.config.num_replicas = 3' > "$workdir/streams/AUDIT.json"
stream_json AUDIT_CHECKPOINT "$checkpoint_subjects" > "$workdir/streams/AUDIT_CHECKPOINT.json"
provision
[ "$status" -eq 0 ] || fail "a stream with extra replicas was refused: $(cat "$workdir/err")"

# One with fewer than the deployment asked for is refused.
reset
stream_json AUDIT "$audit_subjects" > "$workdir/streams/AUDIT.json"
stream_json AUDIT_CHECKPOINT "$checkpoint_subjects" > "$workdir/streams/AUDIT_CHECKPOINT.json"
status=0
PATH="$workdir/bin:$PATH" STUB_DIR="$workdir" AUDIT_REPLICAS=3 sh "$here/provision-audit.sh" > "$workdir/out" 2> "$workdir/err" || status=$?
[ "$status" -ne 0 ] || fail "a stream with fewer replicas than asked for was accepted"
grep -q "keeps fewer than 3 replicas" "$workdir/err" || fail "the refusal does not name the replicas: $(cat "$workdir/err")"

# A server that does not answer, or answers with anything but "stream not
# found", leaves the stream alone instead of creating over it.
reset
touch "$workdir/unreachable"
provision
[ "$status" -ne 0 ] || fail "an unreachable server was taken for a missing stream"
grep -q "could not read AUDIT" "$workdir/err" || fail "the failure does not name the read: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a stream was created while the server did not answer"

reset
echo '{"error":{"code":503,"err_code":10008,"description":"JetStream system temporarily unavailable"}}' > "$workdir/api-error"
provision
[ "$status" -ne 0 ] || fail "an API error was taken for a missing stream"
grep -q "reading AUDIT failed: JetStream system temporarily unavailable" "$workdir/err" || fail "the failure does not name the API error: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a stream was created after an API error"

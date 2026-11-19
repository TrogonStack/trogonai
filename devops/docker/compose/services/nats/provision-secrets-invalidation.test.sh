#!/bin/sh
# Runs provision-secrets-invalidation.sh against a stand-in `nats` that keeps
# the stream's configuration in a file, so it checks what the script creates
# and what it refuses without a server.
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
  jq -n '{config: {
    name: "SECRETS_INVALIDATION", subjects: ["secrets.invalidation.v1.*.>"], storage: "file",
    retention: "limits", discard: "old", deny_delete: true, deny_purge: true, allow_direct: true,
    max_age: 3600000000000, max_msg_size: 1024, max_msgs_per_subject: -1,
    duplicate_window: 120000000000, num_replicas: 1}}'
}

reset() {
  rm -rf "$workdir/streams" "$workdir/added" "$workdir/unreachable" "$workdir/api-error"
  mkdir "$workdir/streams"
}

provision() {
  status=0
  PATH="$workdir/bin:$PATH" STUB_DIR="$workdir" sh "$here/provision-secrets-invalidation.sh" > "$workdir/out" 2> "$workdir/err" || status=$?
}

fail() {
  echo "$1" >&2
  exit 1
}

# The stream is created, in full, when it is missing.
reset
stream_json > "$workdir/created-SECRETS_INVALIDATION.json"
provision
[ "$status" -eq 0 ] || fail "a missing stream was not created: $(cat "$workdir/err")"
[ "$(cut -d' ' -f1 "$workdir/added")" = "SECRETS_INVALIDATION" ] || fail "expected SECRETS_INVALIDATION to be added"
for flag in "--subjects secrets.invalidation.v1.\*.>" "--storage file" "--retention limits" "--discard old" \
  "--deny-delete" "--deny-purge" "--no-allow-rollup" "--allow-direct" "--ack" "--max-age 1h" \
  "--dupe-window 2m" "--max-msg-size 1024" "--replicas 1"; do
  grep -q -- "$flag" "$workdir/added" || fail "the created stream lacks $flag"
done

# Running again adds nothing.
rm "$workdir/added"
provision
[ "$status" -eq 0 ] || fail "a second run failed: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a second run created the stream again"

# Each way an existing stream could lose a notice is refused, naming the field.
refuses() {
  fragment=$1 edit=$2
  reset
  stream_json | jq "$edit" > "$workdir/streams/SECRETS_INVALIDATION.json"
  provision
  [ "$status" -ne 0 ] || fail "a stream that $fragment was accepted"
  grep -q -- "$fragment" "$workdir/err" || fail "the refusal does not name \"$fragment\": $(cat "$workdir/err")"
  [ ! -f "$workdir/added" ] || fail "a refused stream was replaced"
}

refuses "keeps storage memory" '.config.storage = "memory"'
refuses "keeps retention interest" '.config.retention = "interest"'
refuses "requires deny_delete" '.config.deny_delete = false'
refuses "requires deny_purge" '.config.deny_purge = false'
refuses "lets a publish roll up stored notices" '.config.allow_rollup_hdrs = true'
refuses "publishes without acknowledging" '.config.no_ack = true'
refuses "is sealed" '.config.sealed = true'
refuses "acknowledges before writing to disk" '.config.persist_mode = "async"'
refuses "expires notices before the 330 second replay window" '.config.max_age = 329000000000'
refuses "requires message TTL off" '.config.allow_msg_ttl = true'
refuses "is a counter stream" '.config.allow_msg_counter = true'
refuses "limits the notices kept per subject" '.config.max_msgs_per_subject = 1'
refuses "requires at least 1024" '.config.max_msg_size = 1023'
refuses "keeps a duplicate window under 2 minutes" '.config.duplicate_window = 30000000000'
refuses "stores notices under another subject" '.config.subject_transform = {src: ">", dest: "other.>"}'
refuses "lacks subject secrets.invalidation.v1.*.>" '.config.subjects = ["*.secrets.invalidation.v1.>"]'

# What only makes the stream stronger, or is capacity and placement, is
# accepted: no max age, a larger or unlimited message size, discard new,
# direct get off, and more replicas than asked for.
accepts() {
  what=$1 edit=$2
  reset
  stream_json | jq "$edit" > "$workdir/streams/SECRETS_INVALIDATION.json"
  provision
  [ "$status" -eq 0 ] || fail "a stream with $what was refused: $(cat "$workdir/err")"
}

accepts "no max age" '.config.max_age = 0'
accepts "the replay window as its max age" '.config.max_age = 330000000000'
accepts "a larger message size" '.config.max_msg_size = 4096'
accepts "an unlimited message size" '.config.max_msg_size = -1'
accepts "discard new" '.config.discard = "new"'
accepts "direct get off" '.config.allow_direct = false'
accepts "extra replicas" '.config.num_replicas = 3'
accepts "the default persist mode" '.config.persist_mode = "default"'

# One with fewer replicas than the deployment asked for is refused.
reset
stream_json > "$workdir/streams/SECRETS_INVALIDATION.json"
status=0
PATH="$workdir/bin:$PATH" STUB_DIR="$workdir" TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS=3 \
  sh "$here/provision-secrets-invalidation.sh" > "$workdir/out" 2> "$workdir/err" || status=$?
[ "$status" -ne 0 ] || fail "a stream with fewer replicas than asked for was accepted"
grep -q "keeps fewer than 3 replicas" "$workdir/err" || fail "the refusal does not name the replicas: $(cat "$workdir/err")"

# A missing stream is created with the replicas the deployment asks for.
reset
stream_json | jq '.config.num_replicas = 3' > "$workdir/created-SECRETS_INVALIDATION.json"
status=0
PATH="$workdir/bin:$PATH" STUB_DIR="$workdir" TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS=3 \
  sh "$here/provision-secrets-invalidation.sh" > "$workdir/out" 2> "$workdir/err" || status=$?
[ "$status" -eq 0 ] || fail "a stream asked for with three replicas was not created: $(cat "$workdir/err")"
grep -q -- "--replicas 3" "$workdir/added" || fail "the created stream does not keep the replicas asked for"

# A server that does not answer, or answers with anything but "stream not
# found", leaves the stream alone instead of creating over it.
reset
touch "$workdir/unreachable"
provision
[ "$status" -ne 0 ] || fail "an unreachable server was taken for a missing stream"
grep -q "could not read SECRETS_INVALIDATION" "$workdir/err" || fail "the failure does not name the read: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a stream was created while the server did not answer"

reset
echo '{"error":{"code":503,"err_code":10008,"description":"JetStream system temporarily unavailable"}}' > "$workdir/api-error"
provision
[ "$status" -ne 0 ] || fail "an API error was taken for a missing stream"
grep -q "reading SECRETS_INVALIDATION failed: JetStream system temporarily unavailable" "$workdir/err" || fail "the failure does not name the API error: $(cat "$workdir/err")"
[ ! -f "$workdir/added" ] || fail "a stream was created after an API error"

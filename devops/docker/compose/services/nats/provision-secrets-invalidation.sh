#!/bin/sh
# Creates the PLATFORM stream SECRETS_INVALIDATION, which the secrets service
# publishes release invalidation notices to but never creates: it refuses to
# start without the stream (ADR#0081 Decision 3 amendment). Runs on every `up`
# and creates the stream only when it is missing, then fails when it keeps a
# configuration that would lose a notice before a holder can read it. It never
# edits an existing stream.
#
# Mirrors trogon_secrets_service::InvalidationStream, the check the service
# applies at startup, and the configuration it documents.
#
# The stream is read through the JetStream API rather than `nats stream info`,
# whose failure looks the same whether the stream is missing or the server did
# not answer. Only the API's "stream not found" leads to a create, because a
# create against a stream that exists can rewrite it in place.
set -eu

# JetStream's error code for a stream that does not exist.
STREAM_NOT_FOUND=10059

STREAM=SECRETS_INVALIDATION
# The literal prefix comes first because the server refuses a stream subject
# that starts with a wildcard unless it drops publish acks.
SUBJECTS='secrets.invalidation.v1.*.>'

# How far back a holder's consumer that resumes reads: the 300 second release
# bound plus its 30 seconds of jitter, in nanoseconds.
REPLAY_WINDOW_NS=330000000000

# One replica in the dev compose; production runs three. The service checks
# the stream against the same setting.
REPLICAS=${TROGON_SECRETS_INVALIDATION_STREAM_REPLICAS:-1}

# Fails unless the stream keeps `expected` for `field`, the jq filter reading it.
require() {
  field=$1 expected=$2 filter=$3
  actual=$(printf '%s' "$info" | jq -r "$filter")
  if [ "$actual" != "$expected" ]; then
    echo "nats-secrets-invalidation-provision: $STREAM keeps $field $actual, but invalidation requires $expected" >&2
    return 1
  fi
}

# Fails when the jq filter, read against the stream, is true.
refuse_when() {
  problem=$1 filter=$2
  if [ "$(printf '%s' "$info" | jq -r "$filter")" = true ]; then
    echo "nats-secrets-invalidation-provision: $STREAM $problem" >&2
    return 1
  fi
}

# Reads the stream's info into `info`, or fails when the server does not answer.
read_stream() {
  if ! info=$(nats req "\$JS.API.STREAM.INFO.$STREAM" '' --raw); then
    echo "nats-secrets-invalidation-provision: could not read $STREAM, so it is left alone" >&2
    return 1
  fi
}

# The API error code in `info`, empty when the read succeeded.
error_code() {
  printf '%s' "$info" | jq -r '.error.err_code // empty'
}

read_stream
if [ "$(error_code)" = "$STREAM_NOT_FOUND" ]; then
  nats stream add "$STREAM" --subjects "$SUBJECTS" --storage file --retention limits \
    --discard old --replicas "$REPLICAS" --dupe-window 2m --max-age 1h \
    --max-msgs=-1 --max-bytes=-1 --max-msg-size 1024 --max-msgs-per-subject=-1 \
    --deny-delete --deny-purge --no-allow-rollup --allow-direct --ack --defaults >/dev/null
  read_stream
fi
if [ -n "$(error_code)" ]; then
  echo "nats-secrets-invalidation-provision: reading $STREAM failed: $(printf '%s' "$info" | jq -r '.error.description')" >&2
  exit 1
fi
require storage file '.config.storage'
require retention limits '.config.retention'
refuse_when "allows deleting a notice, but invalidation requires deny_delete" '.config.deny_delete != true'
refuse_when "allows purging notices, but invalidation requires deny_purge" '.config.deny_purge != true'
refuse_when "lets a publish roll up stored notices, but invalidation requires rollup off" '.config.allow_rollup_hdrs // false'
refuse_when "publishes without acknowledging, but invalidation requires acks" '.config.no_ack // false'
refuse_when "is sealed, but invalidation must take new notices" '.config.sealed // false'
refuse_when "acknowledges before writing to disk, but invalidation requires the default persist mode" '(.config.persist_mode // "default") == "async"'
refuse_when "expires notices before the 330 second replay window, but invalidation requires at least that" \
  "(.config.max_age // 0) > 0 and .config.max_age < $REPLAY_WINDOW_NS"
refuse_when "lets a notice set its own expiry, but invalidation requires message TTL off" '.config.allow_msg_ttl // false'
refuse_when "is a counter stream, which refuses every notice, but invalidation requires counters off" '.config.allow_msg_counter // false'
refuse_when "limits the notices kept per subject, but invalidation requires no limit" '(.config.max_msgs_per_subject // -1) > 0'
refuse_when "refuses messages over its size limit under 1024 bytes, but invalidation requires at least 1024" \
  '(.config.max_msg_size // -1) > 0 and .config.max_msg_size < 1024'
refuse_when "keeps a duplicate window under 2 minutes, but invalidation requires at least 2" '.config.duplicate_window < 120000000000'
refuse_when "stores notices under another subject, but invalidation requires no subject transform" '.config.subject_transform != null'
refuse_when "keeps fewer than $REPLICAS replicas, but the deployment asked for $REPLICAS" ".config.num_replicas < $REPLICAS"
refuse_when "lacks subject $SUBJECTS, but the secrets service publishes to it" \
  "(.config.subjects // []) | index(\"$SUBJECTS\") == null"
echo "nats-secrets-invalidation-provision: $STREAM keeps $SUBJECTS on $REPLICAS replica(s)"

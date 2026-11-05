#!/bin/sh
# Creates the PLATFORM audit streams that writers publish to but never create
# (ADR#0086 Decision 2), the way a deployment provisions them before those
# writers start. Runs on every `up` and creates only what is missing, then
# fails when a stream keeps a configuration that would weaken the audit trail:
# one that drops the oldest record when full, allows deleting or purging, or
# expires on a timer. It never edits an existing stream.
#
# Mirrors trogon_audit::AuditStream, the check the Rust provisioning applies.
#
# The stream is read through the JetStream API rather than `nats stream info`,
# whose failure looks the same whether the stream is missing or the server did
# not answer. Only the API's "stream not found" leads to a create, because a
# create against a stream that exists can rewrite it in place.
set -eu

# JetStream's error code for a stream that does not exist.
STREAM_NOT_FOUND=10059

# One replica in the dev compose; production runs three.
REPLICAS=${AUDIT_REPLICAS:-1}

# Fails unless the stream keeps `expected` for `field`, the jq filter reading it.
require() {
  stream=$1 field=$2 expected=$3 filter=$4
  actual=$(printf '%s' "$info" | jq -r "$filter")
  if [ "$actual" != "$expected" ]; then
    echo "nats-audit-provision: $stream keeps $field $actual, but audit requires $expected" >&2
    return 1
  fi
}

# Fails when the jq filter, read against the stream, is true.
refuse_when() {
  stream=$1 problem=$2 filter=$3
  if [ "$(printf '%s' "$info" | jq -r "$filter")" = true ]; then
    echo "nats-audit-provision: $stream $problem" >&2
    return 1
  fi
}

# Reads the stream's info into `info`, or fails when the server does not answer.
read_stream() {
  if ! info=$(nats req "\$JS.API.STREAM.INFO.$1" '' --raw); then
    echo "nats-audit-provision: could not read $1, so it is left alone" >&2
    return 1
  fi
}

# The API error code in `info`, empty when the read succeeded.
error_code() {
  printf '%s' "$info" | jq -r '.error.err_code // empty'
}

# stream and its subjects, comma separated.
ensure_stream() {
  stream=$1 subjects=$2
  read_stream "$stream"
  if [ "$(error_code)" = "$STREAM_NOT_FOUND" ]; then
    nats stream add "$stream" --subjects "$subjects" --storage file --retention limits \
      --discard new --replicas "$REPLICAS" --dupe-window 2m --max-age=-1 \
      --max-msgs=-1 --max-bytes=-1 --max-msg-size=-1 --max-msgs-per-subject=-1 \
      --deny-delete --deny-purge --no-allow-rollup --ack --defaults >/dev/null
    read_stream "$stream"
  fi
  if [ -n "$(error_code)" ]; then
    echo "nats-audit-provision: reading $stream failed: $(printf '%s' "$info" | jq -r '.error.description')" >&2
    return 1
  fi
  require "$stream" storage file '.config.storage'
  require "$stream" retention limits '.config.retention'
  require "$stream" discard new '.config.discard'
  refuse_when "$stream" "allows deleting a record, but audit requires deny_delete" '.config.deny_delete != true'
  refuse_when "$stream" "allows purging records, but audit requires deny_purge" '.config.deny_purge != true'
  refuse_when "$stream" "publishes without acknowledging, but audit requires acks" '.config.no_ack // false'
  refuse_when "$stream" "expires records by max age, but audit requires none" '.config.max_age != 0'
  refuse_when "$stream" "limits the records kept per subject, but audit requires no limit" '(.config.max_msgs_per_subject // -1) > 0'
  refuse_when "$stream" "lets a record set its own expiry, but audit requires message TTL off" '.config.allow_msg_ttl // false'
  refuse_when "$stream" "keeps a duplicate window under 2 minutes, but audit requires at least 2" '.config.duplicate_window < 120000000000'
  refuse_when "$stream" "keeps fewer than $REPLICAS replicas, but the deployment asked for $REPLICAS" ".config.num_replicas < $REPLICAS"
  for subject in $(printf '%s' "$subjects" | tr ',' ' '); do
    refuse_when "$stream" "lacks subject $subject, but audit publishes to it" "(.config.subjects // []) | index(\"$subject\") == null"
  done
  echo "nats-audit-provision: $stream keeps $subjects on $REPLICAS replica(s)"
}

ensure_stream AUDIT "platform.v1.audit.decision.>,platform.v1.audit.credential.>,platform.v1.audit.change.>,platform.v1.audit.integrity.>"
ensure_stream AUDIT_CHECKPOINT "platform.v1.audit.checkpoint.>"

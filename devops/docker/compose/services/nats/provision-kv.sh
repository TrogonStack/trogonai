#!/bin/sh
# Creates the PLATFORM key-value buckets that workloads open but never create,
# the way a deployment provisions them before those workloads start. Runs on
# every `up` and creates only what is missing, then fails when a bucket keeps
# another configuration than the one its workloads check for.
set -eu

# bucket, the revisions it keeps per key, and its maximum age in seconds.
ensure_bucket() {
  bucket=$1 history=$2 max_age=$3
  expected="history $history and max age ${max_age}s"
  if ! info=$(nats stream info "KV_$bucket" --json 2>/dev/null); then
    nats kv add "$bucket" --history "$history" --ttl "${max_age}s" --storage file >/dev/null
    info=$(nats stream info "KV_$bucket" --json)
  fi
  actual=$(printf '%s' "$info" | jq -r '"history \(.config.max_msgs_per_subject) and max age \(.config.max_age / 1000000000)s"')
  if [ "$actual" != "$expected" ]; then
    echo "nats-kv-provision: $bucket keeps $actual, but its workloads require $expected" >&2
    return 1
  fi
  echo "nats-kv-provision: $bucket keeps $expected"
}

# The replay records of the a2a-bridge AAuth edge (ADR#0082 Decision 4). In
# enforce the bridge refuses to start unless the bucket keeps one revision per
# key for 180 seconds: the 2 minute signature validity ceiling plus 30 seconds
# of clock skew on either side.
ensure_bucket AAUTH_REPLAY 1 180

#!/bin/sh
# Proves, against a real nats-server running the generated operator
# configuration, that no PLATFORM user can forge a release invalidation notice
# or tamper with the SECRETS_INVALIDATION stream, while the provisioner can
# still manage the stream and a reader can still read it.
#
# The server and every client share one network namespace with no bridge
# network, so the test needs Docker but no address from it.
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
suffix=$$
volume="nats-deny-test-$suffix"
server="nats-deny-test-$suffix"
box_image=natsio/nats-box:0.20.1
server_image=nats:2.14.2-alpine

cleanup() {
  docker rm -f "$server" >/dev/null 2>&1 || true
  docker volume rm -f "$volume" >/dev/null 2>&1 || true
}
trap cleanup EXIT
docker volume create "$volume" >/dev/null

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# Runs a script on stdin in a disposable container that holds the secrets
# volume and the bootstrap directory.
box() {
  docker run --rm -i --network none --user 0 \
    -v "$volume:/secrets" --tmpfs /tmp \
    --mount "type=bind,source=$here,target=/bootstrap,readonly" \
    --entrypoint /bin/sh "$box_image" -eu -s -- "$@"
}

bootstrap() {
  box <<'RUN'
if ! sh /bootstrap/bootstrap.sh > /tmp/bootstrap.log 2>&1; then
  echo "bootstrap failed; credential-bearing output suppressed" >&2
  exit 1
fi
RUN
}

# The deny entries every PLATFORM user must carry, whatever else it holds.
notice_subject='secrets.invalidation.v1.>'
stream_denies='$JS.API.STREAM.CREATE.SECRETS_INVALIDATION
$JS.API.STREAM.UPDATE.SECRETS_INVALIDATION
$JS.API.STREAM.DELETE.SECRETS_INVALIDATION
$JS.API.STREAM.PURGE.SECRETS_INVALIDATION
$JS.API.STREAM.MSG.DELETE.SECRETS_INVALIDATION
$JS.API.STREAM.RESTORE.SECRETS_INVALIDATION'

# Prints the PLATFORM user names, one per line.
platform_users() {
  box <<'RUN'
for jwt in /secrets/nsc/trogonai-dev/accounts/PLATFORM/users/*.jwt; do
  basename "$jwt" .jwt
done
RUN
}

# One JWT describe per user, to compare before and after a rerun.
user_fingerprint() {
  box <<'RUN'
for jwt in /secrets/nsc/trogonai-dev/accounts/*/users/*.jwt; do
  cksum "$jwt"
done
RUN
}

# Prints the entries of `wanted` (one per line) that the user JWT does not carry
# under nats.<kind>.deny.
missing_denies() {
  identity=$1 kind=$2 wanted=$3
  box "$identity" "$kind" "$wanted" <<'RUN'
user=$1 kind=$2 wanted=$3
jwt=$(nsc describe user --account PLATFORM --name "$user" --all-dirs /secrets/nsc --json)
printf '%s\n' "$wanted" | while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  if ! printf '%s' "$jwt" | jq -e --arg kind "$kind" --arg entry "$entry" \
    '(.nats[$kind].deny // []) | index($entry) != null' >/dev/null; then
    echo "$entry"
  fi
done
RUN
}

echo "--- fresh bootstrap"
bootstrap

users=$(platform_users)
echo "PLATFORM users: $(echo "$users" | tr '\n' ' ')"

echo "--- control publisher, an identity without the denies"
box <<'RUN'
nsc add user --account PLATFORM --name test-notice-publisher --all-dirs /secrets/nsc >/dev/null 2>&1
mkdir -p /secrets/clients/test-notice-publisher
nsc generate creds --account PLATFORM --name test-notice-publisher --all-dirs /secrets/nsc \
  --output-file /secrets/clients/test-notice-publisher/nats.creds >/dev/null 2>&1
RUN

echo "--- an existing deployment picks up the denies on rerun"
# Strip the new denies from one user the way an older bootstrap issued it.
box <<'RUN'
nsc edit user --account PLATFORM --name trogon-gateway --all-dirs /secrets/nsc \
  --rm-deny-pub 'secrets.invalidation.v1.>' \
  --rm-deny-pub '$JS.API.STREAM.CREATE.SECRETS_INVALIDATION' \
  --rm-deny-pub '$JS.API.STREAM.UPDATE.SECRETS_INVALIDATION' \
  --rm-deny-pub '$JS.API.STREAM.DELETE.SECRETS_INVALIDATION' \
  --rm-deny-pub '$JS.API.STREAM.PURGE.SECRETS_INVALIDATION' \
  --rm-deny-pub '$JS.API.STREAM.MSG.DELETE.SECRETS_INVALIDATION' \
  --rm-deny-pub '$JS.API.STREAM.RESTORE.SECRETS_INVALIDATION' >/dev/null 2>&1 || true
RUN
bootstrap

for user in $users; do
  if [ "$user" = secrets-invalidation-provisioner ]; then
    want=$notice_subject
  else
    want="$notice_subject
$stream_denies"
  fi
  missing=$(missing_denies "$user" pub "$want")
  if [ -n "$missing" ]; then
    fail "$user lacks deny-pub: $(echo "$missing" | tr '\n' ' ')"
  fi
done

echo "--- a second run changes nothing"
before=$(user_fingerprint)
bootstrap
after=$(user_fingerprint)
if [ "$before" != "$after" ]; then
  fail "a rerun reissued user JWTs"
fi

echo "--- starting nats-server on the generated operator"
box <<'RUN'
sys=$(nsc describe account --name SYS --all-dirs /secrets/nsc --json | jq -r .sub)
platform=$(nsc describe account --name PLATFORM --all-dirs /secrets/nsc --json | jq -r .sub)
auth=$(nsc describe account --name AUTH --all-dirs /secrets/nsc --json | jq -r .sub)
cat > /secrets/test-server.conf <<CONF
port: 4222
operator: /secrets/nats-server/operator.jwt
system_account: $sys
resolver: MEMORY
resolver_preload: {
  $sys: "$(cat /secrets/nsc/trogonai-dev/accounts/SYS/SYS.jwt)"
  $platform: "$(cat /secrets/nsc/trogonai-dev/accounts/PLATFORM/PLATFORM.jwt)"
  $auth: "$(cat /secrets/nsc/trogonai-dev/accounts/AUTH/AUTH.jwt)"
}
jetstream { store_dir: /tmp/jetstream }
CONF
RUN
docker run -d --name "$server" --network none -v "$volume:/secrets:ro" \
  "$server_image" -c /secrets/test-server.conf >/dev/null
ready=false
for _ in $(seq 1 30); do
  if docker logs "$server" 2>&1 | grep -q 'Server is ready'; then
    ready=true
    break
  fi
  sleep 1
done
if [ "$ready" != true ]; then
  docker logs "$server" >&2 || true
  echo "nats-server did not start" >&2
  exit 1
fi

# The nats CLI exits 0 after reporting a permissions violation, so a request
# counts as made only when it got a reply and no violation was reported.
prelude='req() {
  err=$(mktemp)
  if ! nats --timeout 2s req "$@" 2>"$err"; then
    return 1
  fi
  if grep -qi "permissions violation" "$err"; then
    return 1
  fi
}'

# Runs a script on stdin as a NATS user, in the server's network namespace.
as_user() {
  identity=$1
  shift
  { printf '%s\n' "$prelude"; cat; } | docker run --rm -i --network "container:$server" --user 0 \
    -v "$volume:/secrets:ro" -v "$here:/bootstrap:ro" \
    -e "NATS_URL=nats://127.0.0.1:4222" \
    -e "NATS_CREDS=/secrets/clients/$identity/nats.creds" \
    --entrypoint /bin/sh "$box_image" -eu -s -- "$@"
}

provision() {
  as_user secrets-invalidation-provisioner <<'RUN' >/dev/null
sh /bootstrap/provision-secrets-invalidation.sh
RUN
}

# Whether the stream is there: prints the number of messages it holds, or none.
stream_messages() {
  as_user secrets-invalidation-provisioner <<'RUN'
req '$JS.API.STREAM.INFO.SECRETS_INVALIDATION' '' --raw | jq -r '.state.messages // "none"'
RUN
}

# Requests each subject as the user and prints those that got a reply. A denied
# publish gets none, so the request times out.
reachable() {
  identity=$1
  shift
  as_user "$identity" "$@" <<'RUN'
for subject in "$@"; do
  (
    if req "$subject" '{}' >/dev/null 2>&1; then
      echo "$subject"
    fi
  ) &
done
wait
RUN
}

echo "--- no PLATFORM user can create the stream before the provisioner does"
create_denied=true
for user in $users; do
  [ "$user" != secrets-invalidation-provisioner ] || continue
  hits=$(reachable "$user" '$JS.API.STREAM.CREATE.SECRETS_INVALIDATION')
  if [ -n "$hits" ]; then
    fail "$user reached $hits"
    create_denied=false
  fi
done
if [ "$create_denied" != true ]; then
  # A user that could create it left a stream behind the provisioner would keep.
  as_user secrets-invalidation-provisioner <<'RUN' >/dev/null 2>&1 || true
req '$JS.API.STREAM.DELETE.SECRETS_INVALIDATION' ''
RUN
fi

echo "--- the provisioner creates the stream"
provision

for user in $users; do
  echo "--- $user"
  provision
  forge_targets="secrets.invalidation.v1.forged.owner.credential"
  if [ "$user" = secrets-invalidation-provisioner ]; then
    hits=$(reachable "$user" "$forge_targets")
  else
    hits=$(reachable "$user" "$forge_targets" \
      '$JS.API.STREAM.UPDATE.SECRETS_INVALIDATION' \
      '$JS.API.STREAM.PURGE.SECRETS_INVALIDATION' \
      '$JS.API.STREAM.MSG.DELETE.SECRETS_INVALIDATION' \
      '$JS.API.STREAM.RESTORE.SECRETS_INVALIDATION' \
      '$JS.API.STREAM.CREATE.SECRETS_INVALIDATION')
    # DELETE last, because a user that may delete it ends the stream.
    delete_hits=$(reachable "$user" '$JS.API.STREAM.DELETE.SECRETS_INVALIDATION')
    hits="$hits $delete_hits"
  fi
  for hit in $hits; do
    fail "$user reached $hit"
  done
done

echo "--- the stream survived and holds no forged notice"
provision
messages=$(stream_messages)
if [ "$messages" != 0 ]; then
  fail "the stream holds $messages messages after the forgery attempts, expected 0"
fi

echo "--- the provisioner keeps its stream rights"
as_user secrets-invalidation-provisioner <<'RUN' || fail "the provisioner could not reach STREAM.UPDATE or STREAM.INFO"
req '$JS.API.STREAM.INFO.SECRETS_INVALIDATION' '' --raw >/dev/null
req '$JS.API.STREAM.UPDATE.SECRETS_INVALIDATION' '{}' --raw >/dev/null
RUN

echo "--- the control publisher can publish, so the denies and not the stream stop the others"
as_user test-notice-publisher <<'RUN' || fail "the control publisher could not publish a notice"
req 'secrets.invalidation.v1.tenant.owner.credential' 'control' >/dev/null
RUN

echo "--- a reader can still read the stream"
as_user trogon-gateway <<'RUN' || fail "a PLATFORM reader could not read the invalidation stream"
req '$JS.API.STREAM.INFO.SECRETS_INVALIDATION' '' --raw | jq -e '.config.name == "SECRETS_INVALIDATION"' >/dev/null
# The shipped reader's unnamed consumer: legacy create subject, no durable.
consumer=$(req '$JS.API.CONSUMER.CREATE.SECRETS_INVALIDATION' \
  '{"stream_name":"SECRETS_INVALIDATION","config":{"ack_policy":"none","deliver_policy":"last","filter_subject":"secrets.invalidation.v1.*.>"}}' --raw |
  jq -r '.name // empty')
test -n "$consumer"
req "\$JS.API.CONSUMER.INFO.SECRETS_INVALIDATION.$consumer" '' --raw >/dev/null
req "\$JS.API.CONSUMER.MSG.NEXT.SECRETS_INVALIDATION.$consumer" '1' | grep -q control
RUN

if [ "$failures" -ne 0 ]; then
  echo "$failures check(s) failed" >&2
  exit 1
fi
echo "secrets invalidation deny lists passed"

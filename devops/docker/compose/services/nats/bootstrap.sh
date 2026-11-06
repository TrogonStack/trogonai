#!/bin/sh
# Generates the dev operator, its accounts and every NATS client identity, then
# renders the parts of the server config that name them. Runs on every `up` and
# creates only what is missing, so identities outlive restarts and only
# `docker compose down -v` replaces them.
#
# Client material lands in /secrets/clients/<name>/, which pki-bootstrap then
# fills with a client certificate, so a client listed here gets both halves.
#
# The auth callout's signing seeds come from nk rather than nsc, so they exist
# only in the callout's own directory and never in nsc's keystore.
set -eu

. /bootstrap/install-seed.sh

SECRETS=/secrets
NSC_HOME="$SECRETS/nsc"
OPERATOR=trogonai-dev
STORE="$NSC_HOME/$OPERATOR"
SERVER_DIR="$SECRETS/nats-server"
ACCOUNTS_DIR="$SECRETS/accounts"
CLIENTS_DIR="$SECRETS/clients"
CALLOUT_DIR="$CLIENTS_DIR/auth-callout"
CALLOUT_KEYS_DIR="$CALLOUT_DIR/keys"
# Where the callout container mounts its directory, so the references in
# callout.env resolve there.
CALLOUT_MOUNT=/run/secrets/nats
# The gateway verifies the user JWTs the callout mints for tenant DEV, so its
# gateway.env names that tenant's issuer public key. It never holds the seed.
GATEWAY_DIR="$CLIENTS_DIR/a2a-gateway"
# The subject an a2a-bridge sends its mint requests to and the callout answers
# on. It is the default of both, so neither sets AUTH_CALLOUT_MINT_SUBJECT.
BRIDGE_MINT_SUBJECT=a2a.bridge.auth.callout.request

umask 077

nsc() {
  command nsc "$@" --all-dirs "$NSC_HOME"
}

quietly() {
  if ! out=$("$@" 2>&1); then
    printf '%s\n' "$out" >&2
    return 1
  fi
}

account_key() {
  nsc describe account --name "$1" --json | jq -r '.sub'
}

signing_keys() {
  nsc describe account --name "$1" --json | jq -r '.nats.signing_keys // [] | .[] | if type == "object" then .key else . end'
}

has_seed() {
  [ -n "$(find "$NSC_HOME/keys" -name "$1.nk")" ]
}

# The callout's signing keys are registered on accounts too, but nsc holds no
# seed for them, so users are issued with the first key it does hold.
account_signing_key() {
  for key in $(signing_keys "$1"); do
    if has_seed "$key"; then
      echo "$key"
      return 0
    fi
  done
  echo "nats-bootstrap: nsc holds no signing seed for $1" >&2
  return 1
}

user_key() {
  nsc describe user --account "$1" --name "$2" --json | jq -r '.sub'
}

seed_of() {
  find "$NSC_HOME/keys" -name "$1.nk" -exec cat {} \;
}

ensure_account() {
  if [ ! -f "$STORE/accounts/$1/$1.jwt" ]; then
    nsc add account --name "$1"
  fi
  if [ "$(nsc describe account --name "$1" --json | jq '.nats.signing_keys // [] | length')" -eq 0 ]; then
    nsc edit account --name "$1" --sk generate
  fi
}

ensure_jetstream() {
  if [ "$(nsc describe account --name "$1" --json | jq '.nats.limits.disk_storage // 0')" -eq 0 ]; then
    nsc edit account --name "$1" --js-mem-storage -1 --js-disk-storage -1 --js-streams -1 --js-consumer -1
  fi
}

# Any further arguments are nsc permission flags, applied when the user is
# created. A user that already exists keeps the permissions it was issued with.
ensure_user() {
  account=$1 name=$2 owner=$3
  shift 3
  if [ ! -f "$STORE/accounts/$account/users/$name.jwt" ]; then
    nsc add user --account "$account" --name "$name" --private-key "$(account_signing_key "$account")" "$@"
  fi
  dir="$CLIENTS_DIR/$name"
  mkdir -p "$dir"
  quietly nsc generate creds --account "$account" --name "$name" --output-file "$dir/nats.creds"
  chown "$owner" "$dir" "$dir/nats.creds"
  chmod 0700 "$dir"
  chmod 0400 "$dir/nats.creds"
}

# The bridge's mint request crosses from PLATFORM into AUTH as a service. AUTH
# exports it privately, so only an account holding an activation token for
# PLATFORM can import it, and PLATFORM imports it under the same subject.
# Replies travel back through the service on their own.
ensure_bridge_mint_exchange() {
  if [ "$(nsc describe account --name AUTH --json |
    jq --arg subject "$BRIDGE_MINT_SUBJECT" '[.nats.exports // [] | .[] | select(.subject == $subject)] | length')" -eq 0 ]; then
    nsc add export --account AUTH --name bridge-mint --subject "$BRIDGE_MINT_SUBJECT" --service --private
  fi
  if [ "$(nsc describe account --name PLATFORM --json |
    jq --arg subject "$BRIDGE_MINT_SUBJECT" '[.nats.imports // [] | .[] | select(.subject == $subject)] | length')" -eq 0 ]; then
    token=$(mktemp)
    nsc generate activation --account AUTH --target-account "$(account_key PLATFORM)" \
      --subject "$BRIDGE_MINT_SUBJECT" --output-file "$token"
    nsc add import --account PLATFORM --name bridge-mint --token "$token" --service
    rm -f "$token"
  fi
}

# The identity of every client that presents no user JWT. It may neither
# publish nor subscribe, so all it can do is reach the callout.
ensure_sentinel() {
  if [ ! -f "$STORE/accounts/AUTH/users/sentinel.jwt" ]; then
    nsc add user --account AUTH --name sentinel --bearer --deny-pubsub '>' \
      --private-key "$(account_signing_key AUTH)"
  fi
}

# Registered unscoped, because nats-server gives a user signed by a scoped key
# the scope's permissions instead of the ones the callout minted.
ensure_callout_signing_key() {
  account=$1 seed="$CALLOUT_KEYS_DIR/$2.nk"
  if [ ! -s "$seed" ]; then
    nk -gen account > "$seed"
  fi
  key=$(nk -inkey "$seed" -pubout)
  if ! signing_keys "$account" | grep -qx "$key"; then
    nsc edit account --name "$account" --sk "$key"
  fi
}

install_env_file() {
  chown "$(stat -c %u "$(dirname "$1")")" "$1"
  chmod 0400 "$1"
}

if [ ! -f "$STORE/$OPERATOR.jwt" ]; then
  nsc add operator --name "$OPERATOR" --sys --generate-signing-key
  nsc edit operator --require-signing-keys
fi
nsc env --operator "$OPERATOR" >/dev/null 2>&1

for account in SYS AUTH PLATFORM DEV; do
  ensure_account "$account"
done

for account in PLATFORM DEV; do
  ensure_jetstream "$account"
done

ensure_bridge_mint_exchange

# account, user name, and the uid the consuming image runs as. Only the bridge
# may publish a mint request, and no PLATFORM user may subscribe to the subject,
# so nothing in PLATFORM can read the requests or race the callout's answer.
# AUTH holds no user but the callout and the sentinel, which may not subscribe,
# so the callout is the only one that can answer.
while read -r account name owner; do
  case "$account/$name" in
    PLATFORM/a2a-bridge) ensure_user "$account" "$name" "$owner" --deny-sub "$BRIDGE_MINT_SUBJECT" ;;
    PLATFORM/*)
      ensure_user "$account" "$name" "$owner" --deny-pub "$BRIDGE_MINT_SUBJECT" --deny-sub "$BRIDGE_MINT_SUBJECT"
      ;;
    *) ensure_user "$account" "$name" "$owner" ;;
  esac
done <<'USERS'
SYS sys 0
AUTH auth-callout 1000
PLATFORM trogon-gateway 1000
PLATFORM decider-host 0
PLATFORM a2a-gateway 1000
PLATFORM a2a-bridge 0
PLATFORM acp-nats 0
PLATFORM mcp-nats 0
PLATFORM channel-bridge-telegram 0
PLATFORM nats-cli 0
PLATFORM kv-provisioner 0
PLATFORM audit-provisioner 0
USERS

ensure_sentinel

if [ "$(nsc describe account --name AUTH --json | jq '.nats.authorization.auth_users // [] | length')" -eq 0 ]; then
  nsc edit authcallout --account AUTH \
    --auth-user "$(user_key AUTH auth-callout)" \
    --allowed-account "$(account_key DEV)" \
    --curve generate
fi

mkdir -p "$CALLOUT_KEYS_DIR"
ensure_callout_signing_key AUTH auth-response
ensure_callout_signing_key DEV tenant-dev

xkey=$(nsc describe account --name AUTH --json | jq -r '.nats.authorization.xkey')
if ! install_seed "$CALLOUT_KEYS_DIR/callout-xkey.nk" seed_of "$xkey"; then
  echo "nats-bootstrap: nsc holds no seed for the callout xkey $xkey" >&2
  exit 1
fi
chown -R "$(stat -c %u "$CALLOUT_DIR")" "$CALLOUT_KEYS_DIR"
chmod 0700 "$CALLOUT_KEYS_DIR"
chmod 0400 "$CALLOUT_KEYS_DIR"/*.nk

mkdir -p "$SERVER_DIR" "$ACCOUNTS_DIR"

cp "$STORE/$OPERATOR.jwt" "$SERVER_DIR/operator.jwt"
sys_key=$(account_key SYS)
cat > "$SERVER_DIR/identity.conf" <<IDENTITY
SYSTEM_ACCOUNT: "$sys_key"
SYSTEM_ACCOUNT_PRELOAD: {
  $sys_key: "$(cat "$STORE/accounts/SYS/SYS.jwt")"
}
DEFAULT_SENTINEL: "$(cat "$STORE/accounts/AUTH/users/sentinel.jwt")"
IDENTITY

# The store is encrypted with this key, so it is generated once and never
# replaced: a different key leaves every existing stream unreadable.
if [ ! -s "$SERVER_DIR/jetstream-key.conf" ]; then
  printf 'key: "%s"\n' "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')" > "$SERVER_DIR/jetstream-key.conf"
fi

for account in AUTH PLATFORM DEV; do
  cp "$STORE/accounts/$account/$account.jwt" "$ACCOUNTS_DIR/$account.jwt"
done

cat > "$CALLOUT_DIR/callout.env" <<CALLOUT
AUTH_CALLOUT_SIGNING_KEY_SOURCE=file
AUTH_CALLOUT_AUTH_ACCOUNT=$(account_key AUTH)
AUTH_CALLOUT_RESPONSE_SIGNING_KEY=file://$CALLOUT_MOUNT/keys/auth-response.nk
AUTH_CALLOUT_TENANT_SIGNING_KEYS=$(account_key DEV)=file://$CALLOUT_MOUNT/keys/tenant-dev.nk
AUTH_CALLOUT_XKEY_SEED=file://$CALLOUT_MOUNT/keys/callout-xkey.nk
AUTH_CALLOUT_MTLS_TRUST_ANCHORS=$CALLOUT_MOUNT/ca.pem
CALLOUT
install_env_file "$CALLOUT_DIR/callout.env"

if ! tenant_dev_public=$(nk -inkey "$CALLOUT_KEYS_DIR/tenant-dev.nk" -pubout); then
  echo "nats-bootstrap: the callout holds no tenant-dev seed to derive the caller JWT issuer key from" >&2
  exit 1
fi
# The callout mints a caller JWT with `aud` set to the tenant account it admits
# the caller into, so the gateway expects the DEV account public key there.
cat > "$GATEWAY_DIR/gateway.env" <<GATEWAY
A2A_GATEWAY_CALLER_JWT_ISSUER_KEYS=$tenant_dev_public
A2A_GATEWAY_JWT_AUDIENCE=$(account_key DEV)
GATEWAY
install_env_file "$GATEWAY_DIR/gateway.env"

# The callout mints the bridge's users into this tenant, and a request that
# names none is refused.
printf 'AUTH_CALLOUT_XKEY_PUBLIC=%s\nBRIDGE_TENANT_ACCOUNT=%s\n' "$xkey" "$(account_key DEV)" > "$CLIENTS_DIR/a2a-bridge/bridge.env"
install_env_file "$CLIENTS_DIR/a2a-bridge/bridge.env"

for account in SYS AUTH PLATFORM DEV; do
  echo "nats-bootstrap: account $account $(account_key "$account")"
done
echo "nats-bootstrap: callout xkey $xkey"
echo "nats-bootstrap: ready (operator $OPERATOR)"

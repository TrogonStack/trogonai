#!/bin/sh
# Generates the dev operator, its accounts and every NATS client identity, then
# renders the parts of the server config that name them. Runs on every `up` and
# creates only what is missing, so identities outlive restarts and only
# `docker compose down -v` replaces them.
#
# Client material lands in /secrets/clients/<name>/, which pki-bootstrap then
# fills with a client certificate, so a client listed here gets both halves.
set -eu

SECRETS=/secrets
NSC_HOME="$SECRETS/nsc"
OPERATOR=trogonai-dev
STORE="$NSC_HOME/$OPERATOR"
SERVER_DIR="$SECRETS/nats-server"
ACCOUNTS_DIR="$SECRETS/accounts"
CLIENTS_DIR="$SECRETS/clients"

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

account_signing_key() {
  nsc describe account --name "$1" --json | jq -r '.nats.signing_keys[0] | if type == "object" then .key else . end'
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

ensure_user() {
  account=$1 name=$2 owner=$3
  if [ ! -f "$STORE/accounts/$account/users/$name.jwt" ]; then
    nsc add user --account "$account" --name "$name" --private-key "$(account_signing_key "$account")"
  fi
  dir="$CLIENTS_DIR/$name"
  mkdir -p "$dir"
  quietly nsc generate creds --account "$account" --name "$name" --output-file "$dir/nats.creds"
  chown "$owner" "$dir" "$dir/nats.creds"
  chmod 0700 "$dir"
  chmod 0400 "$dir/nats.creds"
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

# account, user name, and the uid the consuming image runs as.
while read -r account name owner; do
  ensure_user "$account" "$name" "$owner"
done <<'USERS'
SYS sys 0
AUTH auth-callout 0
PLATFORM trogon-gateway 1000
PLATFORM decider-host 0
PLATFORM a2a-gateway 0
PLATFORM a2a-bridge 0
PLATFORM acp-nats 0
PLATFORM mcp-nats 0
PLATFORM channel-bridge-telegram 0
PLATFORM nats-cli 0
USERS

if [ "$(nsc describe account --name AUTH --json | jq '.nats.authorization.auth_users // [] | length')" -eq 0 ]; then
  nsc edit authcallout --account AUTH \
    --auth-user "$(user_key AUTH auth-callout)" \
    --allowed-account "$(account_key DEV)" \
    --curve generate
fi

mkdir -p "$SERVER_DIR" "$ACCOUNTS_DIR"

cp "$STORE/$OPERATOR.jwt" "$SERVER_DIR/operator.jwt"
sys_key=$(account_key SYS)
cat > "$SERVER_DIR/identity.conf" <<IDENTITY
SYSTEM_ACCOUNT: "$sys_key"
SYSTEM_ACCOUNT_PRELOAD: {
  $sys_key: "$(cat "$STORE/accounts/SYS/SYS.jwt")"
}
IDENTITY

# The store is encrypted with this key, so it is generated once and never
# replaced: a different key leaves every existing stream unreadable.
if [ ! -s "$SERVER_DIR/jetstream-key.conf" ]; then
  printf 'key: "%s"\n' "$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')" > "$SERVER_DIR/jetstream-key.conf"
fi

for account in AUTH PLATFORM DEV; do
  cp "$STORE/accounts/$account/$account.jwt" "$ACCOUNTS_DIR/$account.jwt"
done

xkey=$(nsc describe account --name AUTH --json | jq -r '.nats.authorization.xkey')
cat > "$CLIENTS_DIR/auth-callout/callout.env" <<CALLOUT
NATS_URL=tls://nats:4222
NATS_CREDS=/run/secrets/nats/nats.creds
NATS_CA=/run/secrets/nats/ca.pem
NATS_CERT=/run/secrets/nats/client.pem
NATS_KEY=/run/secrets/nats/client-key.pem
AUTH_CALLOUT_ALLOWED_ACCOUNTS=$(account_key DEV)
AUTH_CALLOUT_XKEY_SEED=$(seed_of "$xkey")
CALLOUT
chmod 0400 "$CLIENTS_DIR/auth-callout/callout.env"

for account in SYS AUTH PLATFORM DEV; do
  echo "nats-bootstrap: account $account $(account_key "$account")"
done
echo "nats-bootstrap: ready (operator $OPERATOR)"

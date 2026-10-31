#!/bin/sh
# Issues the dev CA and every certificate the stack serves or presents. Runs on
# every `up`, keeps what is still valid, and reissues a certificate that is
# missing, no longer chains to the CA, or expires within 30 days. Only
# `docker compose down -v` replaces the CA.
#
# Each client directory that nats-bootstrap created under /secrets/clients/
# gets a client certificate owned by the same uid as its creds.
set -eu

SECRETS=/secrets
CA_DIR="$SECRETS/pki"
CA_DAYS=3650
LEAF_DAYS=365
RENEW_WITHIN_SECONDS=2592000

umask 077

quietly() {
  if ! out=$("$@" 2>&1); then
    printf '%s\n' "$out" >&2
    return 1
  fi
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

mkdir -p "$CA_DIR"

if [ ! -s "$CA_DIR/ca.pem" ] || [ ! -s "$CA_DIR/ca-key.pem" ]; then
  cat > "$WORK/ca.cnf" <<'CONF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = ext
[dn]
CN = trogonai dev CA
[ext]
basicConstraints = critical,CA:TRUE,pathlen:0
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
CONF
  openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$CA_DIR/ca-key.pem"
  openssl req -x509 -new -config "$WORK/ca.cnf" -key "$CA_DIR/ca-key.pem" \
    -sha256 -days "$CA_DAYS" -out "$CA_DIR/ca.pem"
  echo "pki-bootstrap: issued a new dev CA"
fi

# issue <dir> <file prefix> <common name> <extended key usage> [subject alt names]
issue() {
  dir=$1 prefix=$2 cn=$3 eku=$4 san=${5:-}
  cert="$dir/$prefix.pem"
  key="$dir/$prefix-key.pem"
  owner=$(stat -c %u "$dir")

  if [ -s "$cert" ] && [ -s "$key" ] \
    && openssl verify -CAfile "$CA_DIR/ca.pem" "$cert" >/dev/null 2>&1 \
    && openssl x509 -in "$cert" -noout -checkend "$RENEW_WITHIN_SECONDS" >/dev/null; then
    return 0
  fi

  {
    echo '[req]'
    echo 'distinguished_name = dn'
    echo 'prompt = no'
    echo '[dn]'
    echo "CN = $cn"
    echo '[ext]'
    echo 'basicConstraints = critical,CA:FALSE'
    echo 'keyUsage = critical,digitalSignature'
    echo "extendedKeyUsage = $eku"
    echo 'subjectKeyIdentifier = hash'
    echo 'authorityKeyIdentifier = keyid'
    if [ -n "$san" ]; then
      echo "subjectAltName = $san"
    fi
  } > "$WORK/leaf.cnf"

  openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$key"
  openssl req -new -config "$WORK/leaf.cnf" -key "$key" -out "$WORK/leaf.csr"
  quietly openssl x509 -req -in "$WORK/leaf.csr" -CA "$CA_DIR/ca.pem" -CAkey "$CA_DIR/ca-key.pem" \
    -extfile "$WORK/leaf.cnf" -extensions ext -sha256 -days "$LEAF_DAYS" -out "$cert"
  chown "$owner" "$cert" "$key"
  chmod 0444 "$cert"
  chmod 0400 "$key"
  echo "pki-bootstrap: issued $cert for $cn"
}

install_ca() {
  cp "$CA_DIR/ca.pem" "$1/ca.pem"
  chmod 0444 "$1/ca.pem"
}

mkdir -p "$SECRETS/nats-server"

issue "$SECRETS/nats-server" server nats serverAuth \
  "DNS:nats,DNS:localhost,DNS:nats.trogonai.orb.local,IP:127.0.0.1,IP:::1"

install_ca "$SECRETS/nats-server"

for dir in "$SECRETS"/clients/*/; do
  [ -d "$dir" ] || continue
  dir=${dir%/}
  issue "$dir" client "$(basename "$dir")" clientAuth
  install_ca "$dir"
done

echo "pki-bootstrap: ready (CA $(openssl x509 -in "$CA_DIR/ca.pem" -noout -fingerprint -sha256 | cut -d= -f2))"

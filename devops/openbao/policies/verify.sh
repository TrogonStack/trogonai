#!/usr/bin/env bash
#
# Applies every policy in this directory to a throwaway dev OpenBao and asserts
# what each role can and cannot do, using the exact percent-encoded paths
# OpenBaoSecretStore builds. Run it after editing any .hcl here.
#
#   ./verify.sh
#
set -euo pipefail

CONTAINER=bao-policy-verify
IMAGE=openbao/openbao:2.5.5
PORT=18201
TOKEN=dev-only-token
ADDR="http://127.0.0.1:${PORT}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cleanup() { docker rm -f "$CONTAINER" >/dev/null 2>&1 || true; }
trap cleanup EXIT
cleanup

docker run -d --rm --name "$CONTAINER" -p "${PORT}:8200" "$IMAGE" \
  server -dev "-dev-root-token-id=${TOKEN}" -dev-listen-address=0.0.0.0:8200 >/dev/null

for _ in $(seq 1 60); do
  docker exec "$CONTAINER" bao status -address=http://127.0.0.1:8200 >/dev/null 2>&1 && break
  sleep 0.5
done

for f in "$DIR"/*.hcl; do
  name="$(basename "$f" .hcl)"
  docker cp "$f" "$CONTAINER:/tmp/policy.hcl" >/dev/null
  docker exec -e BAO_ADDR=http://127.0.0.1:8200 -e BAO_TOKEN="$TOKEN" "$CONTAINER" \
    bao policy write "${name//-/_}" /tmp/policy.hcl >/dev/null
done

mktok() {
  docker exec -e BAO_ADDR=http://127.0.0.1:8200 -e BAO_TOKEN="$TOKEN" "$CONTAINER" \
    bao token create -policy="$1" -field=token -ttl=30m
}
CPW=$(mktok control_plane_write)
GWR=$(mktok gateway_read)
AUD=$(mktok audit_read)
LWC=$(mktok lifecycle_worker_cleanup)
BGA=$(mktok break_glass_admin)

# Percent-encoded exactly as encode_path_segment emits it. OpenBao decodes this
# back to `openbao:tenant-2:github/primary:webhook_secret`, which is why the
# policies glob the credential position instead of matching one segment.
CRED='openbao%3Atenant-2%3Agithub%2Fprimary%3Awebhook_secret'
BASE="${ADDR}/v1/secret"
P="trogonai/tenant-2/credentials/${CRED}"
fails=0

chk() {
  local label="$1" want="$2"
  shift 2
  local got
  got=$(curl -s -o /dev/null -w '%{http_code}' "$@")
  if [[ "$got" == "$want" ]]; then
    printf '%-24s want %-3s  PASS\n' "$label" "$want"
  else
    printf '%-24s want %-3s  FAIL (got %s)\n' "$label" "$want" "$got"
    fails=$((fails + 1))
  fi
}

chk "cpw POST data" 200 -X POST -H "X-Vault-Token: $CPW" -d '{"data":{"value":"s3cr3t"}}' "$BASE/data/$P"
chk "cpw POST metadata" 204 -X POST -H "X-Vault-Token: $CPW" -d '{"custom_metadata":{"owner_id":"tenant-2"}}' "$BASE/metadata/$P"
chk "cpw GET data" 403 -H "X-Vault-Token: $CPW" "$BASE/data/$P"
chk "gwr GET metadata" 200 -H "X-Vault-Token: $GWR" "$BASE/metadata/$P"
chk "gwr GET data v1" 200 -H "X-Vault-Token: $GWR" "$BASE/data/$P?version=1"
chk "gwr POST data" 403 -X POST -H "X-Vault-Token: $GWR" -d '{"data":{"value":"x"}}' "$BASE/data/$P"
chk "gwr LIST metadata" 403 -X LIST -H "X-Vault-Token: $GWR" "$BASE/metadata/trogonai/tenant-2/credentials"
chk "aud GET metadata" 200 -H "X-Vault-Token: $AUD" "$BASE/metadata/$P"
chk "aud GET data" 403 -H "X-Vault-Token: $AUD" "$BASE/data/$P"
chk "aud LIST creds" 200 -X LIST -H "X-Vault-Token: $AUD" "$BASE/metadata/trogonai/tenant-2/credentials"
chk "aud LIST nested" 200 -X LIST -H "X-Vault-Token: $AUD" "$BASE/metadata/trogonai/tenant-2/credentials/openbao:tenant-2:github"
chk "lwc GET data" 403 -H "X-Vault-Token: $LWC" "$BASE/data/$P"
chk "lwc POST delete" 204 -X POST -H "X-Vault-Token: $LWC" -d '{"versions":[1]}' "$BASE/delete/$P"
chk "lwc POST undelete" 403 -X POST -H "X-Vault-Token: $LWC" -d '{"versions":[1]}' "$BASE/undelete/$P"
chk "bga POST undelete" 204 -X POST -H "X-Vault-Token: $BGA" -d '{"versions":[1]}' "$BASE/undelete/$P"
chk "lwc POST destroy" 204 -X POST -H "X-Vault-Token: $LWC" -d '{"versions":[1]}' "$BASE/destroy/$P"

echo "--- escape checks ---"
chk "gwr other subtree" 403 -H "X-Vault-Token: $GWR" "${ADDR}/v1/secret/data/elsewhere/x"
chk "bga sys/policies" 403 -H "X-Vault-Token: $BGA" "${ADDR}/v1/sys/policies/acl/gateway_read"
chk "cpw outside creds" 403 -X POST -H "X-Vault-Token: $CPW" -d '{"data":{"value":"x"}}' "${ADDR}/v1/secret/data/trogonai/tenant-2/other/x"

echo
if [[ "$fails" -eq 0 ]]; then
  echo "all checks passed"
else
  echo "${fails} check(s) failed"
  exit 1
fi

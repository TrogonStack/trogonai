#!/bin/sh
set -eu

bootstrap_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

# Keep generated credentials and bootstrap output inside the disposable container.
docker run --rm -i --network none --user 0 \
  --tmpfs /secrets --tmpfs /tmp \
  --mount "type=bind,source=$bootstrap_dir,target=/bootstrap,readonly" \
  --entrypoint /bin/sh natsio/nats-box:0.20.1 -eu -s <<'CONTRACT'
  check_gateway_env() {
    . /secrets/clients/a2a-gateway/gateway.env
    . /secrets/clients/auth-callout/callout.env
    expected_audience=${AUTH_CALLOUT_TENANT_SIGNING_KEYS%%=*}
    if [ "${A2A_GATEWAY_JWT_AUDIENCE:-}" != "$expected_audience" ]; then
      echo "gateway audience does not match the callout tenant account" >&2
      exit 1
    fi
    expected_issuer=$(nk -inkey /secrets/clients/auth-callout/keys/tenant-dev.nk -pubout)
    if [ "$A2A_GATEWAY_CALLER_JWT_ISSUER_KEYS" != "$expected_issuer" ]; then
      echo "gateway issuer does not match the callout signing key" >&2
      exit 1
    fi
    test "$(stat -c %u /secrets/clients/a2a-gateway/gateway.env)" = 1000
    test "$(stat -c %a /secrets/clients/a2a-gateway/gateway.env)" = 400
    test "$(stat -c %a /secrets/clients/a2a-gateway)" = 700
    if [ "${A2A_PREFIX:-}" != "$expected_audience.a2a" ]; then
      echo "gateway prefix does not bind the imported tenant account" >&2
      exit 1
    fi
    platform=$(nsc describe account --name PLATFORM --all-dirs /secrets/nsc --json)
    tenant=$(nsc describe account --name DEV --all-dirs /secrets/nsc --json)
    platform_key=$(printf "%s" "$platform" | jq -r .sub)
    printf "%s" "$platform" | jq -e '
      any(.nats.exports[]?;
        .subject == "*.a2a.v1.gateway.*.*.*.>" and
        .type == "service" and .account_token_position == 1 and .response_type == "Stream")
    ' >/dev/null
    printf "%s" "$tenant" | jq -e --arg platform "$platform_key" --arg tenant "$expected_audience" '
      any(.nats.imports[]?;
        .account == $platform and
        .subject == ($tenant + ".a2a.v1.gateway.*.*.*.>") and
        .local_subject == "a2a.v1.gateway.*.*.*.>" and .type == "service")
    ' >/dev/null
    printf "%s" "$platform" | jq -e '
      any(.nats.exports[]?;
        .subject == "*.a2a.v1.gateway.*.*.events" and
        .type == "stream" and .account_token_position == 1)
    ' >/dev/null
    printf "%s" "$tenant" | jq -e --arg platform "$platform_key" --arg tenant "$expected_audience" '
      any(.nats.imports[]?;
        .account == $platform and
        .subject == ($tenant + ".a2a.v1.gateway.*.*.events") and
        .local_subject == "a2a.v1.gateway.*.*.events" and .type == "stream")
    ' >/dev/null
    . /secrets/clients/a2a-bridge/bridge.env
    test "$BRIDGE_TENANT_ACCOUNT" = "$expected_audience"
    auth=$(nsc describe account --name AUTH --all-dirs /secrets/nsc --json)
    printf "%s" "$auth" | jq -e '
      any(.nats.exports[]?; .subject == "a2a.bridge.auth.callout.request" and .token_req == true)
    ' >/dev/null
    printf "%s" "$platform" | jq -e '
      any(.nats.imports[]?; .subject == "a2a.bridge.auth.callout.request" and .token != null)
    ' >/dev/null
    test "$(wc -l < /secrets/clients/a2a-gateway/gateway.env)" -eq 3
    test -z "$(find /secrets/clients/a2a-gateway -name "*.nk")"
  }
  for pass in fresh restart; do
    if ! sh /bootstrap/bootstrap.sh > /tmp/bootstrap.log 2>&1; then
      echo "bootstrap failed; credential-bearing output suppressed" >&2
      exit 1
    fi
    check_gateway_env
    if [ "$pass" = fresh ]; then
      cp /secrets/clients/a2a-gateway/gateway.env /tmp/first-gateway.env
      nsc describe account --name DEV --all-dirs /secrets/nsc --json >/tmp/first-tenant.json
      nsc edit export --all-dirs /secrets/nsc --account PLATFORM \
        --subject '*.a2a.v1.gateway.*.*.*.>' --service --response-type Singleton \
        >/tmp/export-edit.log 2>&1
    else
      cmp /tmp/first-gateway.env /secrets/clients/a2a-gateway/gateway.env
      nsc describe account --name DEV --all-dirs /secrets/nsc --json >/tmp/refreshed-tenant.json
      if cmp -s /tmp/first-tenant.json /tmp/refreshed-tenant.json; then
        echo "export migration did not refresh the importing account" >&2
        exit 1
      fi
    fi
  done
  echo "gateway bootstrap contract passed on fresh initialization and restart"
CONTRACT

#!/usr/bin/env bash
# Lints and renders the sandbox Helm charts, then checks and unit-tests the
# Prometheus alert rules the charts ship.
#
# Needs helm, yq (mikefarah) and promtool on PATH.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
chart="${here}/.."
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

echo "== helm lint (umbrella and every subchart)"
helm lint "${chart}"
# A subchart linted alone has no parent values, so hand it the umbrella's
# global block.
yq eval '{"global": .global}' "${chart}/values.yaml" > "${work}/global.yaml"
for sub in "${chart}"/charts/*/; do
  helm lint "${sub}" --values "${work}/global.yaml"
done

echo "== helm template (defaults)"
helm template ci "${chart}" > "${work}/default.yaml"

echo "== helm template (authz enforced by default)"
grep -q 'SANDBOX_INSECURE_DEV: "false"' "${work}/default.yaml"
grep -q 'name: SPICEDB_TOKEN' "${work}/default.yaml"
grep -q 'SANDBOX_API_KEY_BUCKET: "sandbox-api-keys"' "${work}/default.yaml"
grep -q 'SANDBOX_SPICEDB_TENANT_RESOURCE_TYPE: "sandbox_tenant"' "${work}/default.yaml"
grep -q 'definition sandbox_tenant {' "${work}/default.yaml"
grep -q 'SANDBOX_K8S_RESOURCE_NAMING: "sandbox"' "${work}/default.yaml"
if grep -q 'insecureDev: true' "${chart}/values.yaml" "${chart}"/charts/sandbox-broker/values.yaml; then
  echo "the default values must not turn authz off" >&2
  exit 1
fi

if helm template ci "${chart}" --set global.spicedb.tenantResourceType=unknown \
  > "${work}/invalid.yaml" 2> "${work}/invalid.log"; then
  echo "the bundled SpiceDB must reject a tenant resource type its schema does not define" >&2
  exit 1
fi
helm template ci "${chart}" --set spicedb.enabled=false \
  --set global.spicedb.endpoint=authz.example:50051 \
  --set global.spicedb.tenantResourceType=custom_tenant > "${work}/custom-tenant.yaml"
grep -q 'SANDBOX_SPICEDB_TENANT_RESOURCE_TYPE: "custom_tenant"' "${work}/custom-tenant.yaml"
if helm template ci "${chart}" --set sandbox-k8s-worker.resourceNaming=unknown \
  > "${work}/invalid-worker.yaml" 2> "${work}/invalid-worker.log"; then
  echo "the chart must reject an unknown worker resource naming mode" >&2
  exit 1
fi

echo "== helm template (dev values, external SpiceDB)"
helm template ci "${chart}" -f "${chart}/values-dev.yaml" | grep -q 'SANDBOX_INSECURE_DEV: "true"'
helm template ci "${chart}" --set spicedb.enabled=false --set global.spicedb.endpoint=authz.example:50051 \
  | grep -q 'SPICEDB_ENDPOINT: "authz.example:50051"'

# Every subchart that ships a PrometheusRule gates it on
# metrics.prometheusRule.enabled; turn them all on.
echo "== helm template (alert rules on)"
sets=()
for sub in "${chart}"/charts/*/; do
  name="$(basename "${sub}")"
  if grep -q 'prometheusRule' "${sub}/values.yaml" 2>/dev/null; then
    sets+=(--set "${name}.metrics.prometheusRule.enabled=true")
  fi
done
helm template ci "${chart}" "${sets[@]}" > "${work}/rules-on.yaml"

# One plain Prometheus rule file per PrometheusRule, named after the resource.
rules="${work}/rules"
mkdir -p "${rules}"
yq eval-all -s "\"${rules}/\" + .metadata.name" 'select(.kind == "PrometheusRule")' "${work}/rules-on.yaml"
count=0
for f in "${rules}"/*.yml; do
  [ -e "${f}" ] || break
  yq -i '.spec' "${f}"
  mv "${f}" "${f%.yml}.rules.yaml"
  count=$((count + 1))
done
if [ "${count}" -eq 0 ]; then
  echo "no PrometheusRule rendered; the alert rule checks have nothing to check" >&2
  exit 1
fi

echo "== promtool check rules (${count} rule files)"
promtool check rules "${rules}"/*.rules.yaml

echo "== promtool test rules"
for t in "${here}"/rule-tests/*.test.yaml; do
  # A test file names its rule files relative to itself; run it beside them.
  cp "${t}" "${rules}/"
  (cd "${rules}" && promtool test rules "$(basename "${t}")")
done
echo "ok"

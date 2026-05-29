# trogon-gateway-k8s

Kubernetes controller that projects **Gateway API** (`Gateway`, `HTTPRoute`) and Trogon **`MCPGatewayConfig`** resources into the MCP gateway JetStream KV bucket (`mcp-gateway-config`). The gateway bundle loader in `trogon-mcp-gateway` watches the same bucket for policy hot-swap.

Design reference: [`docs/identity/k8s-controller.md`](../../../docs/identity/k8s-controller.md) (read-only). This crate implements Block G item 3 with a minimal v1 surface; `MCPServer` / `MCPPolicyBundle` / `MCPTrustBundle` from the design doc are not wired yet.

## Install CRDs

Gateway API CRDs are **not** shipped here — install the upstream Gateway API bundle in your cluster first (for example [Gateway API standard install](https://gateway-api.sigs.k8s.io/guides/)).

Install the Trogon CRD:

```bash
kubectl apply -f crds/gateway.trogon.ai_mcpgatewayconfigs.yaml
```

Validate manifests without applying:

```bash
kubectl apply --dry-run=client -f crds/gateway.trogon.ai_mcpgatewayconfigs.yaml
```

## RBAC

The controller needs read/write on Trogon CRDs, read on Gateway API resources, and permission to patch status. Example `ClusterRole` (bind with a **namespaced** `RoleBinding` when using `--watch-namespace`):

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: trogon-gateway-k8s
rules:
  - apiGroups: ["gateway.trogon.ai"]
    resources: ["mcpgatewayconfigs", "mcpgatewayconfigs/status"]
    verbs: ["get", "list", "watch", "patch", "update"]
  - apiGroups: ["gateway.networking.k8s.io"]
    resources: ["gateways", "httproutes"]
    verbs: ["get", "list", "watch"]
```

NATS credentials need JetStream KV **put/get** on `$KV.mcp-gateway-config.>` per the design doc NATS ACL table.

## Run the controller

```bash
cargo run -p trogon-gateway-k8s -- \
  --nats-url nats://127.0.0.1:4222 \
  --nats-creds /etc/nats/creds/controller.creds \
  --watch-namespace tenant-acme \
  --health-addr 0.0.0.0:8080
```

Environment variables: `NATS_URL`, `NATS_CREDS`, `MCP_GATEWAY_CONTROLLER_WATCH_NAMESPACE`.

Health: `GET /healthz` on `--health-addr` (stdlib HTTP; returns `ok`).

## KV layout (`mcp-gateway-config` bucket)

| Key pattern | Source CRD | Notes |
|-------------|------------|-------|
| `mcp.gateway.config.{namespace}.{name}` | `MCPGatewayConfig` | Full config JSON + bundle scope hint; uses `trogon-mcp-gateway` manifest scope validation |
| `bundle/active` | `MCPGatewayConfig` | Active bundle pointer JSON (stub `artifact_sha256` until OCI/ConfigMap publish lands) |
| `mcp.gateway.route.{namespace}.{name}` | `Gateway` | Listener snapshot; **stub** `server_bindings_stub: true` |
| `mcp.gateway.httproute.{namespace}.{name}` | `HTTPRoute` | Parent gateways, hostnames, backend names; **stub** `path_rules_stub: true` |

Writes are **idempotent**: SHA-256 of canonical JSON is compared before `kv.put`; unchanged payloads skip the write.

## Tests

```bash
cargo test -p trogon-gateway-k8s
```

Unit tests cover projection round-trips. `tests/reconcile_smoke.rs` exercises one KV publish with `MemoryConfigKv` (no cluster).

### Manual cluster smoke (optional)

When `kubectl` and a cluster (`kind`, `k3s`, or `minikube`) are available:

1. Install Gateway API + Trogon CRD.
2. Apply an `MCPGatewayConfig` in a tenant namespace.
3. Run the controller with NATS reachable.
4. Confirm KV: `nats kv get mcp-gateway-config mcp.gateway.config.<namespace>.<name>`

## What is stubbed

- Gateway API → MCP `server_id` routing and path rules
- Policy bundle body fetch (OCI / ConfigMap); only pointer + scope validation
- `MCPServer`, `MCPPolicyBundle`, `MCPTrustBundle` reconcilers from the design doc
- Drift detection interval, finalizers, and Prometheus metrics

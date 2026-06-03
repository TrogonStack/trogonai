# Deploy the agentgateway in 15 minutes

This tutorial walks from a clean Kubernetes cluster to your first
MCP `tools/call` **allow** decision, first **deny** decision, and an
audit-tail showing both. It assumes:

- A reachable Kubernetes cluster (`kubectl` works).
- Helm 3.13+.
- A reachable NATS JetStream cluster (or use the embedded
  `nats-box` for development).
- A reachable SpiceDB endpoint (or `spicedb serve` locally).

## 1. Provision the NATS substrate

```sh
nats stream add --config devops/nats/streams/mcp-audit.json
nats stream add --config devops/nats/streams/a2a-audit.json
nats kv add     --config devops/nats/kv/policy-bundle.json
nats kv add     --config devops/nats/kv/trogon-claims.json
# See devops/nats/README.md for the full list.
```

For decentralized operator-mode auth, paste-ready server config and
NSC bootstrap live at `devops/nats/nats-server.example.conf` and
`devops/nsc/README.md`.

## 2. Apply the SpiceDB schema

```sh
zed import --endpoint "$SPICEDB_ENDPOINT" \
           --token "$SPICEDB_PRESHARED_KEY" \
           devops/spicedb/schema.zed

# Allow user:alice to call MCP tool weather.
zed relationship create trogon/mcp_tool:weather caller trogon/principal:alice
```

## 3. Install the gateway

```sh
kubectl create namespace trogon-system

# Sensitive material via existingSecret — see charts/agentgateway/README.md.
kubectl -n trogon-system create secret generic agentgateway-secrets \
  --from-literal=MCP_GATEWAY_SPICEDB_TOKEN="$SPICEDB_PRESHARED_KEY"

helm install -n trogon-system mcp-gateway charts/agentgateway \
  --set existingSecret=agentgateway-secrets \
  --set env.MCP_GATEWAY_SPICEDB_ENDPOINT="spicedb.trogon.svc.cluster.local:50051" \
  --set env.NATS_URL="nats://nats.trogon.svc.cluster.local:4222"
```

## 4. First allow

```sh
nats request mcp.gateway.request.weather \
  '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"weather","arguments":{"city":"oslo"}}}' \
  -H "trogon-mcp-tenant: alice"
```

You should see a JSON-RPC response from the upstream MCP server.

## 5. First deny

```sh
nats request mcp.gateway.request.weather \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"weather","arguments":{"city":"oslo"}}}' \
  -H "trogon-mcp-tenant: bob"
```

`bob` has no `caller` tuple, so the gateway replies with JSON-RPC
error `-32100` (authorization denied).

## 6. Audit tail

```sh
nats stream view MCP_AUDIT
```

Each call (allow and deny) lands as one `AuditEnvelope` with the
caller ID, decision, and policy attribution. Splunk HEC / Elastic ECS
shape is available — see
`rsworkspace/crates/trogon-mcp-gateway/src/observability/audit_bridge.rs`.

## Next

- Deploy more binaries (A2A, STS, JWKS publisher, registry,
  traffic-view): `charts/agentgateway/README.md`.
- Operator runbook: `docs/runbook/agentgateway.md`.
- Threat model: `docs/security/threat-model.md`.

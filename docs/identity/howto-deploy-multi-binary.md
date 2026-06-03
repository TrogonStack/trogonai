# How to deploy a multi-binary agentgateway topology

The v0.1 release ships ten binaries that operators deploy together:

| Binary | Role |
|--------|------|
| `trogon-mcp-gateway` | MCP JSON-RPC gateway (Tier-1 SpiceDB + JWT). |
| `a2a-gateway` | A2A protocol gateway (message/send + stream + tasks). |
| `a2a-bridge` | HTTPS → NATS bridge (`0.0.0-skeleton` in v0.1). |
| `a2a-auth-callout` | NATS `$SYS.REQ.USER.AUTH` callout subscriber. |
| `a2a-nats-discovery` | Federated AgentCard registrar + discoverer. |
| `mcp-nats-server` | NATS-side MCP server runner. |
| `acp-nats-server` | NATS-side ACP server runner. |
| `trogon-sts` | Security Token Service (delegated JWT minting). |
| `trogon-jwks-publisher` | Publishes JWKS into the `mcp-jwks` KV. |
| `trogon-agent-registry` | KV-backed registry consumer. |
| `trogon-traffic-view` | Read-only audit projector + UI. |

This is a how-to: each step assumes you've completed
[`docs/get-started/deploy-agentgateway.md`](../get-started/deploy-agentgateway.md)
for the gateway baseline and have the substrate (NATS + SpiceDB)
provisioned.

## Topology choice

There is **one Helm chart**, `charts/agentgateway`. The same templates
deploy every binary above by overriding `image.bin` and the
binary-specific env. Pre-baked overlays live as
`charts/agentgateway/values-<binary>.yaml`.

```sh
# MCP gateway (default).
helm install mcp-gw   charts/agentgateway

# A2A gateway.
helm install a2a-gw   charts/agentgateway -f charts/agentgateway/values-a2a-gateway.yaml

# STS, JWKS publisher, registry, traffic-view, …
helm install sts      charts/agentgateway -f charts/agentgateway/values-trogon-sts.yaml
helm install jwks     charts/agentgateway -f charts/agentgateway/values-trogon-jwks-publisher.yaml
helm install registry charts/agentgateway -f charts/agentgateway/values-trogon-agent-registry.yaml
helm install view     charts/agentgateway -f charts/agentgateway/values-trogon-traffic-view.yaml
```

Every release shares one Account-scoped signing-key Secret
(`devops/nsc/README.md`); reference it as `existingSecret` on each
release that needs JWT signing material (STS, JWKS publisher,
auth-callout, discovery).

## What runs where

| Concern | Owner binary | Substrate it reads/writes |
|---|---|---|
| MCP authz decision | `trogon-mcp-gateway` | SpiceDB (read), `MCP_AUDIT` (write). |
| A2A authz decision | `a2a-gateway` | SpiceDB (read), `A2A_AUDIT` (write). |
| Push DLQ | `a2a-gateway` mirror + `a2a-nats-agent` source | `A2A_PUSH_DLQ` stream. |
| AgentCard discovery | `a2a-nats-discovery` | `a2a-catalog` KV. |
| JWT minting | `a2a-auth-callout` | Account signing key Secret. |
| JWKS publishing | `trogon-jwks-publisher` | `mcp-jwks` KV. |
| Token exchange | `trogon-sts` | `mcp-jwks` KV (rotation). |
| Registry consumer | `trogon-agent-registry` | `agent-registry` KV. |
| Audit projection / UI | `trogon-traffic-view` | `MCP_AUDIT` / `A2A_AUDIT` (read). |

## Single Account vs multi-Account

- **Single Account** — appropriate for a single tenant or
  pre-production. One NSC Account JWT covers every binary; one
  Helm namespace.
- **Multi-Account** — one NATS Account per tenant. Run an
  `auth-callout` and `nats-discovery` instance per Account; the
  gateways are tenant-agnostic and can be deployed once per region
  if the substrate supports it.

NSC bootstrap automation: `devops/nsc/bootstrap.sh` per tenant.

## NetworkPolicy + PDB

Both are opt-in (see `charts/agentgateway/values.yaml`). Production
deployments should:

- Enable `networkPolicy` on every release, locking ingress to the
  client namespace and egress to NATS / SpiceDB / OTel ports.
- Enable `pdb` on releases with `replicaCount > 1`.

## v0.2 deltas

- KV-sync rate limiter — coordination across replicas.
- Multi-region routing — `RegionRouter` wiring.
- HTTPS ingress — `a2a-bridge` graduates from `0.0.0-skeleton`.

See `docs/roadmap/agentgateway-v0.2.md`.

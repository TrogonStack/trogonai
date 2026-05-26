# A2A bridge — HTTPS sidecar sketch

Engineering sketch for the future **`a2a-bridge`** crate (Phase 4). The bridge is the HTTPS↔NATS interop sidecar that lets standard A2A HTTP clients participate in the NATS binding without rewriting their transport stack. It is **not** implemented in-tree yet; shape is decided in [A2A architecture](architecture.md) and tracked in [A2A architecture](architecture.md) §Phase 4.

## Related links

| Document | Purpose |
|----------|---------|
| [Gateway roadmap](gateway-roadmap.md) | Gateway ingress checklist — auth-callout, policy, audit on `{prefix}.gateway.>` |
| [Push DLQ triage](../how-to/operators/push-dlq-triage.md) | Push DLQ triage — terminal failures publish from the agent `Bridge`, not from gateway or bridge ingress |
| [A2A architecture](architecture.md) | Open engineering items and suggested ordering |
| [Auth callout design](auth-callout-design.md) | Auth callout contract the bridge reuses for credential minting |

---

## When `a2a-bridge` fits

Use the bridge when callers speak **standard A2A over HTTPS** but agents and task events live on **NATS inside a tenant Account**. Typical deployment patterns:

| Pattern | Why the bridge |
|---------|----------------|
| **External TLS termination** | Envoy, nginx, or a cloud load balancer terminates TLS and forwards plain HTTP to the bridge pod. The bridge owns A2A auth validation and NATS credential minting; the edge proxy stays transport-only. |
| **Multi-cluster federation** | Callers in one cluster or region reach agents in another without holding long-lived NATS credentials for every tenant Account. The bridge connects outbound to the correct Account using the same auth-callout path as first-party NATS clients. |
| **HTTPS-only clients** | Browsers, SaaS webhooks, and third-party A2A agents that cannot adopt the NATS client stack still publish on `{prefix}.gateway.{agent_id}.{method}` after the bridge re-mints a short-lived User JWT. |
| **Symmetric HTTPS agents** | External HTTPS agents register proxied AgentCards via the catalog registrar; inbound `{prefix}.agent.{agent_id}.>` traffic from the gateway is forwarded over HTTPS on the reverse path. |

Do **not** reach for the bridge when a process can connect to NATS directly with org-standard credentials. [`a2a-nats-server`](../../../rsworkspace/crates/a2a-nats-server) and [`a2a-nats-stdio`](../../../rsworkspace/crates/a2a-nats-stdio) are narrower local adapters over `a2a_nats::Client`; they do not terminate foreign HTTPS A2A or re-mint per-request Users for audit attribution.

---

## Placement in the perimeter

Tenancy remains **one NATS Account per tenant**. Subjects carry **no `{tenant}` segment** — the Account namespace is the boundary. The bridge does not introduce a shared bridge User; that would collapse caller identity in audit and DLQ segments.

```
  [HTTPS client] ──TLS──► [Envoy / nginx] ──HTTP──► [a2a-bridge]
                                                          │
                    OIDC / mTLS / API key (ingress)       │
                                                          ▼
                                              [auth callout on $SYS.REQ.USER.AUTH]
                                                          │
                              Account-bound User JWT      │
                              (sub, aud=Account, caller_id)
                                                          ▼
                                              [NATS — tenant Account]
                                                          │
                              publish {prefix}.gateway.{agent_id}.{method}
                              subscribe _INBOX.{caller_id}.>
                                                          ▼
                                              [a2a-gateway] ──► {prefix}.agent.{agent_id}.{method}
```

This aligns with the **NATS CONNECT URL security model** described in [Gateway roadmap](gateway-roadmap.md): external credentials terminate at the perimeter, the auth callout mints an Account-scoped User JWT, and all subsequent NATS traffic uses the same subject ACL templates as a natively connected client (`{prefix}.gateway.>` publish, `_INBOX.{caller_id}.>` subscribe). Bridge outbound NATS connections use the same `NATS_URL` / creds env conventions documented in [Runtime env](../reference/runtime-env.md).

---

## Ingress auth (HTTPS side)

The bridge terminates **HTTPS-facing** auth before any NATS publish:

1. Validate the caller's external credential — OIDC bearer token (primary), mTLS client certificate (service-to-service), or transitional API key.
2. Resolve the target **tenant Account** and stable **`caller_id`** (same mapping contract as [Auth callout design](auth-callout-design.md)).
3. Call the auth callout (or an equivalent minting API with the same JWT shape) to obtain a **short-lived User JWT** bound to that Account.
4. Open (or reuse from a pool keyed by caller session) a NATS connection authenticated with the minted User.

JWT ingress at the bridge must **not** substitute a single shared bridge User. Per-request re-mint preserves caller attribution on gateway spans, audit envelopes, and push DLQ subject segments.

Optional **JWT/mTLS at the bridge listener** complements edge TLS termination: mTLS for service meshes, JWT validation for user-facing APIs. The edge proxy and bridge can split concerns — TLS and WAF at the edge, A2A-aware auth at the bridge.

---

## Outbound NATS path

Once authenticated inside the tenant Account, the bridge behaves like a first-class caller:

| Direction | Behavior |
|-----------|----------|
| **HTTPS in → NATS out** | Translate A2A JSON-RPC POST to NATS request/reply on `{prefix}.gateway.{agent_id}.{method}` with reply inbox `_INBOX.{caller_id}.{nonce}`. |
| **Streaming** | Attach a JetStream pull consumer on `{prefix}.task.{task_id}.events.>` and map events to **SSE** on the HTTPS response. |
| **NATS in → HTTPS out** | Gateway forwards agent RPC to a proxied HTTPS endpoint; bridge adapts SSE from the external agent back into JetStream events on the task subject. |

Gateway policy, SpiceDB Tier 1, and ingress audit land on **`a2a-gateway`** per [Gateway roadmap](gateway-roadmap.md). The bridge's job is transport interop and credential re-mint, not policy evaluation.

Unary **`message/send`** inherits the gateway **30s deadline** once wired; longer work must transition to **`message/stream`**.

---

## Subject prefix and tenancy notes

| Topic | Rule |
|-------|------|
| **Prefix** | Shared `{prefix}` (default `a2a`) inside each Account — set via `A2A_PREFIX` / bridge config. No tenant segment in subjects. |
| **Gateway publish** | `{prefix}.gateway.{agent_id}.{method…}` only; ACL bounds the minted caller User to `{prefix}.gateway.>`. |
| **Reply inbox** | `_INBOX.{caller_id}.>` subscribe ACL on the minted User; bridge generates per-request inbox tokens under that namespace. |
| **Task events** | Consumer on `{prefix}.task.{task_id}.events.>` for SSE egress; stream **`A2A_EVENTS`** is Account-scoped (see [JetStream account streams](../reference/jetstream-account-streams.md)). |
| **Push DLQ** | Terminal push failures publish to `{prefix}.push.dlq.{caller_id}.{task_id}` from the agent **`Bridge`** streaming pump — see [Push DLQ triage](../how-to/operators/push-dlq-triage.md). Bridge ingress does not own DLQ. |
| **Federation** | Cross-Account discovery is opt-in via operator-signed exports of `{prefix}.discover.>`; bridge deployments in separate clusters still connect to one Account per tenant. |

---

## Explicit non-goals

| Area | Owner |
|------|-------|
| JetStream provisioning | `a2a-nats` / `a2a-nats-discovery` / Account bootstrap |
| Push dispatch and DLQ emission | Agent `Bridge` in `a2a-nats` |
| Tier 1–3 policy | `a2a-gateway` + `a2a-pack` bundles |
| AgentCard KV writes | `a2a-nats-discovery` registrar |

---

## Implementation tracker

See [A2A architecture](architecture.md) §Phase 4 — `a2a-bridge` crate, federated discovery exports, and cross-binding collaboration tests. Suggested ordering places bridge work after auth callout, gateway auth integration, and hardened push DLQ caller attribution.

---

## Testing the nats transport locally

Default runtime uses `A2A_BRIDGE_TRANSPORT=stub` (no outbound NATS). To exercise the **nats** wiring:

| Command | Purpose |
|---------|---------|
| `cd rsworkspace && cargo test -p a2a-bridge nats_transport_` | In-process harness (`StubAuthCalloutMint` + mock gateway/agent + audit assertions) — **no live NATS** |
| `cd rsworkspace && cargo test -p a2a-bridge -- --ignored nats_transport_live` | Optional smoke against `NATS_URL` when `nats-server` is running locally |

The in-process harness lives in `a2a-bridge::nats_transport_harness` and mirrors `bootstrap_nats_transport` (`AuthCalloutJsonMintClient` + `GatewayInboundPublisher` + JetStream SSE intake) using `trogon-nats::AdvancedMockNatsClient`. Production auth-callout mint deployment is still required for real clusters.

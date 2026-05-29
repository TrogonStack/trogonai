# ADR 0004 — STS Form Factor

## Status

**Accepted (2026-05-27).** Authorizes a NATS-service implementation of STS on `mcp.sts.exchange` (Block 2.1).

## Context

The Security Token Service (STS) is the mesh identity plane. It exchanges a caller's current credential (bootstrap or upstream mesh token) plus workload attestation for a short-lived, audience-scoped mesh JWT with an appended `act_chain` entry. Every cross-hop agent call and every gateway egress to a backend is expected to pass through STS before the target sees the token.

The question this ADR resolves is **where that service lives at runtime** — not what it validates (registry, SVID, `aud`, scope) or how keys are stored (ADR 0006). Those behaviors are identical across form factors; only transport, deployment topology, and operational boundaries change.

Today the branch has perimeter auth (`a2a-auth-callout` minting connect-time NATS User JWTs) and gateway ingress validation, but **no token-exchange service**. Block 2.4 requires the MCP gateway to call STS on egress rather than propagate inbound identity. Block 3 requires the A2A SDK to call STS before every outbound hop. Both consumers need a stable, low-latency endpoint contract.

Related decisions (not decided here):

| ADR | Question |
|-----|----------|
| 0003 | Bootstrap vs. mesh tokens — is connect-time JWT exchanged at STS or used directly? |
| 0005 | TTL, `aud` granularity, refresh strategy |
| 0006 | Signing-key custody and JWKS publication |

## Options

### (a) NATS service on `mcp.sts.exchange`

A dedicated Rust binary (or decider-backed service) subscribes to `mcp.sts.exchange` as a **queue-group consumer**. Callers use NATS request/reply with a per-request reply inbox. The exchange payload follows an RFC 8693–inspired JSON contract (see Block 2.1). Success and failure outcomes emit audit events to `mcp.audit.sts.{outcome}` on JetStream.

**Pros:** Reuses existing NATS connections and mTLS; no second transport stack; queue groups give horizontal scale; latency is one in-mesh RTT; audit and domain events share JetStream with `trogon-decider`; dev mode runs against the same local NATS cluster as gateways and agents.

**Cons:** Non-NATS clients (browser UI, external batch jobs) cannot call STS without a bridge; NATS subject ACL must explicitly grant `mcp.sts.exchange` to exchange-eligible identities; regional routing depends on NATS cluster/leaf topology rather than DNS.

### (b) HTTP service behind mTLS

Standalone service at `sts.<env>.<domain>`, exposing `POST /oauth/token` (token exchange). Clients present mTLS client certs (SVID) and JSON body. JWKS at `/.well-known/jwks.json`.

**Pros:** Familiar OAuth/RFC 8693 tooling; easy to put behind regional load balancers; callable from non-NATS runtimes without a NATS client; integrates cleanly with service meshes that terminate mTLS at sidecars.

**Cons:** Extra connection setup and TLS handshake per exchange unless pooled; agents already on NATS pay a transport tax; HTTP ingress is a new attack surface and ops surface (cert rotation, WAF, rate limits at edge); audit events must be bridged into JetStream separately or duplicated.

### (c) Library embedded in the gateway (in-process)

STS validation and minting logic compiled into `trogon-mcp-gateway` (and optionally `a2a-gateway`) as a crate. Gateway egress calls the library directly; no network hop for gateway-initiated exchanges.

**Pros:** Lowest theoretical latency for gateway egress; no STS outage for the gateway-only path if library is local.

**Cons:** **Does not serve agent-to-agent hops** — agents calling other agents still need a remote STS; signing keys and registry hot-path caches replicate to every gateway instance (ADR 0006 blast radius); policy and validation logic drift across replicas unless pinned to identical bundle versions; audit emission is per-gateway-instance rather than a single choke point; harder to rate-limit and circuit-break globally.

### (d) External IdP add-on (extend existing OIDC provider)

Delegate token exchange to Auth0, Keycloak, Cognito, or an in-house OIDC server via RFC 8693 token exchange, extended with custom claims (`act_chain`, `wkl`, `agent_id`).

**Pros:** Leverages existing IdP HA, rotation, and compliance tooling; OIDC-native clients get a standard HTTP surface.

**Cons:** IdPs do not natively understand agent registry, SPIFFE trust bundles, or `act_chain` append semantics; custom actions/plugins become the real STS anyway; mesh-specific audit shape (`mcp.audit.sts.*`) requires a sidecar or event webhook; couples mesh identity evolution to IdP release cycles; dev-mode ergonomics require a cloud IdP or heavy local Keycloak.

## Decision drivers

| Driver | Weight | Notes |
|--------|--------|-------|
| **Latency budget** | High | P99 exchange < 40 ms (Uber published target, Block 2.1). Budget covers signature verify, registry lookup, SVID check, mint, and audit emit. |
| **Key custody** | High | Minting keys must not sprawl to every gateway replica. Centralized STS simplifies ADR 0006 (single signer, single JWKS publisher). |
| **Blast radius on outage** | High | STS is on the critical path once ADR 0003 mandates exchange-before-hop. Form factor affects whether outage is one service vs. N gateway instances. |
| **Multi-region deployment** | Medium | NATS supercluster/leaf nodes vs. regional HTTP STS behind GSLB. Trust-bundle and registry cache freshness must meet exchange latency. |
| **Operational simplicity** | High | Prefer one deployable, one subject, one audit stream over dual HTTP+NATS stacks. |
| **Audit-as-events (`trogon-decider`)** | Medium | Exchange decisions are domain events (success, deny, rate-limit). NATS JetStream is the existing decider transport; HTTP STS needs an explicit bridge. |
| **Dev-mode ergonomics** | Medium | Local `nats-server`, file-based SVIDs, and a single `trogon-sts` binary should suffice without SPIRE or external IdP. |

## Recommendation

**Adopt option (a): NATS service on `mcp.sts.exchange`**, implemented as a standalone queue-group consumer (`trogon-sts` or decider-backed equivalent), with the exchange contract also documented for future HTTP compatibility.

### Rationale

1. **Transport alignment.** Agents, gateways, and audit already speak NATS. An exchange is one request/reply on an existing connection — the best fit for the 40 ms P99 budget without opening parallel HTTP pools.

2. **Single choke point.** One service holds minting keys (ADR 0006), registry hot-path cache, trust-bundle cache, and global rate limits. Gateway egress (Block 2.4) and SDK outbound calls (Block 3) share the same endpoint and semantics.

3. **Audit and decider fit.** Every exchange emits to `mcp.audit.sts.{outcome}` with full inputs and minted claims (not the signature). A `trogon-decider` aggregate can consume these events for lineage, anomaly scoring, and compensating actions without a transport adapter.

4. **HA without new infra.** Queue-group consumers scale horizontally; NATS handles failover. Multi-region follows existing supercluster/leaf patterns rather than introducing regional HTTP STS + JWKS replication.

5. **Dev-mode path.** Same as production transport: run `trogon-sts` against local NATS with file-based SVID trust anchor and in-memory registry stub. No IdP, no second port, no mTLS cert provisioning beyond what dev NATS already uses.

6. **Option (c) is rejected** for scope — it only optimizes gateway egress and duplicates key material. **Option (d) is rejected** — mesh-specific validation belongs in our STS, not an IdP plugin. **Option (b) is deferred** — not chosen initially, but the wire contract should be transport-agnostic so an HTTP facade can be added later for UI or non-NATS batch callers without changing claim semantics.

### Sketch

```
Agent / Gateway                    trogon-sts (queue group)
      |                                   |
      |-- request/reply mcp.sts.exchange ->|
      |   { subject_token, audience,       | verify + registry + SVID
      |     actor_token (SVID), purpose }  | append act_chain, mint JWT
      |<- { access_token, expires_in } ----|
      |                                   +-- publish mcp.audit.sts.{outcome}
```

Gateway egress cache (Block 2.4): key `(caller_sub, target_aud, session_id, scope)`, TTL ≤ half mesh-token TTL, lives in gateway memory — does not change STS form factor.

## Consequences

### Block 2.1 implementation

- Spec and implement `trogon-sts` as a queue-group consumer on `mcp.sts.exchange`.
- Pin the RFC 8693–inspired request/response schema in `docs/identity/sts-exchange.md` (Block 7).
- NATS ACL: exchange-eligible identities get publish to `mcp.sts.exchange` and subscribe to `_INBOX.>`; no broad mint permissions on callers.
- In-memory caches for registry and trust bundles with async refresh; circuit breaker when dependencies are unhealthy.
- Rate limits per `wkl` and per `agent_id`.

### ADR 0006 (key custody)

- Mesh-token signing keys live with the STS service (KMS/Vault/file — decided in 0006), not in gateways.
- JWKS publication via NATS KV (`mcp-jwks` or control subject) and/or HTTP mirror for validators that prefer HTTPS.

### Block 3 — A2A SDK dependency graph

```
SDK constructor
  ├── SVID source (file / SPIRE)
  ├── STS endpoint: NATS subject `mcp.sts.exchange` (+ connection handle)
  ├── registry endpoint (optional): `mcp.registry.agent.lookup`
  └── call() → exchange → publish to target
```

SDK must not embed minting logic; it calls STS over NATS. CI lint (Block 3) flags direct gateway publishes that bypass exchange.

### Gateway (Block 2.4)

- `trogon-mcp-gateway` gains an STS client crate dependency, not an embedded minter.
- Egress path: validate inbound → call STS with `aud=<backend>` → attach new token → strip inbound credential.

### Operations

- New deployable, health subject, metrics (`sts.exchange.latency`, `sts.exchange.denied`, cache hit rate).
- Rollout gated by `MCP_GATEWAY_AGENT_IDENTITY` shadow/enforce modes (Block 6).

## Open questions

1. **Failure mode — STS unavailable.** Default recommendation: **fail-closed** (mesh stops; gateways return structured error, agents retry with backoff). A degraded-mode escape hatch (propagate bootstrap token without exchange) increases blast radius and breaks `aud` guarantees — if ever allowed, it must be explicit, tenant-scoped, and heavily audited. **Needs human sign-off.**

2. **Is STS itself a `trogon-decider` aggregate?** Exchange validation could be event-sourced (command → events → minted token projection). Lighter alternative: stateless service with JetStream audit only. Decider adds replay/consistency benefits at implementation cost. **Resolved: stateless service + JetStream audit; not event-sourced. Reopen only with a concrete replay-or-consistency requirement.**

3. **Registry outage behavior.** Fail-closed vs. stale-cache grace period (how stale is acceptable for `allowed_workloads`?).

4. **Multi-region topology.** Regional STS instances with local registry cache vs. global STS with cross-region NATS latency. Trust-bundle replication SLA. **Resolved: single-region in v1. Regional STS instances with local registry cache when expanding; trust-bundle replication SLA is set at first multi-region deploy (no SLA without a region pair).**

5. **HTTP facade timing.** Do user-facing clients (Block 5 step-up, UI) need STS access in v1, or only mesh-internal services? **Resolved: mesh-internal only in v1. Wire contract is transport-agnostic, so the HTTP facade is a future PR triggered by the first user-facing caller, not a planning gap.**

6. **Non-SPIFFE callers.** Documented allow-list (`wkl: "human"`, `auth_method: "oidc"`) — does STS run the same NATS subject or a separate `mcp.sts.exchange.human`?

7. **SpiceDB on exchange path.** Block 2.1 mentions circuit breaker on SpiceDB outages — is SpiceDB consulted synchronously during exchange or only via precomputed registry ACLs?

8. **OAuth-MCP composition** (`MCP_GATEWAY_PLAN.md:67`). If MCP OAuth tokens enter via callout, does STS accept them as `subject_token`, or only NATS User JWTs? **Resolved: deferred until first OAuth-fronted MCP client. ADR 0005 §OQ6 pins the semantics (OAuth access token enters STS as `subject_token`); the multi-issuer config is a same-day add when the first caller arrives.**

## References

- `MCP_GATEWAY_PLAN.md` — audit envelope, JetStream subjects, `trogon-decider` touch-points
- [Uber — Solving the Agent Identity Crisis](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/) — P99 latency target, per-hop token model
- [RFC 8693 — OAuth 2.0 Token Exchange](https://www.rfc-editor.org/rfc/rfc8693)

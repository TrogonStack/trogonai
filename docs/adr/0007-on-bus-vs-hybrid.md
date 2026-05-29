# ADR 0007: On-bus vs Hybrid MCP Gateway Deployment

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | On-bus vs hybrid deployment shape; unblocks operator docs and third-party integration how-to |
| **Related** | [On-bus vs hybrid spec](../identity/on-bus-vs-hybrid.md); [How to integrate third-party MCP](../identity/howto-integrate-third-party-mcp.md); [Reference subject grammar](../identity/reference-subject-grammar.md); [MCP gateway operator overview](../identity/mcp-gateway-operator-overview.md); [ADR 0001](0001-tenancy-model.md); [ADR 0003](0003-bootstrap-vs-mesh-tokens.md); [ADR 0004](0004-sts-form-factor.md); [ADR 0006](0006-mesh-token-signing-keys.md) |

## Context

The MCP gateway is a policy enforcement point on JSON-RPC traffic. TrogonStack-native agents, gateways, STS, registry, SpiceDB, and JetStream audit already run on NATS. Phase 1 proved the substrate: `trogon-mcp-gateway` consumes `{prefix}.gateway.request.>` as a queue group, evaluates CEL and SpiceDB on gated methods, rewrites egress to `{prefix}.server.{server_id}.{method_suffix}`, and publishes audit to JetStream.

The strategic fork is not whether the gateway exists, but **which transports terminate at the gateway edge** and **how backend MCP servers attach**. The community MCP ecosystem overwhelmingly ships **stdio** or **local HTTP/SSE** for IDE integration. Trogon-native participants are already NATS principals. Choosing a default deployment shape locks operator mental models, SDK ingress contracts, compliance scope (one hot-path transport vs two), and how third-party servers join the mesh.

**On-bus** means every MCP JSON-RPC frame the gateway fronts arrives on NATS subjects under `{prefix}.gateway.*` and `{prefix}.server.*`. Perimeter identity is bootstrap NATS User JWT at CONNECT (auth callout); mesh tokens gate cross-hop and egress paths per [ADR 0003](0003-bootstrap-vs-mesh-tokens.md). Third-party binaries that speak stdio never hold NATS credentials; an operator-managed bridge subscribes to `{prefix}.server.{server_id}.>` and translates wire formats.

**Hybrid** means the gateway retains the on-bus data plane and **additionally** exposes a north-south HTTP listener (Streamable HTTP and/or SSE per the MCP transport specification) so remote third-party MCP servers and browser-adjacent clients can attach without a NATS client library. OAuth resource-server behavior, CORS, per-route rate limits, and HTTP session affinity become shipping criteria—not stretch goals.

The product positioning question — *which MCP servers must the gateway front—TrogonStack-native only, or the broader third-party ecosystem too?* — remained open until this ADR. The design analysis in [on-bus-vs-hybrid.md](../identity/on-bus-vs-hybrid.md) argues the trade-offs (policy surface, audit envelope, trust bundles, load-balancer story, vendor onboarding friction). This ADR records the durable choice; the spec remains the explanation and migration narrative.

### Decision drivers

| Driver | Weight | On-bus bias | Hybrid bias |
|--------|--------|-------------|-------------|
| Hot-path transport count | High | One family (NATS) | Two (NATS + HTTP/SSE) |
| Phase 1 code alignment | High | Queue group already shipped | Requires new listener crate surface |
| Third-party stdio MCP prevalence | High | Sidecar (`mcp-nats-stdio`) matches market | HTTP helps SaaS-only vendors |
| Policy / audit parity | High | Subject grammar is routing source of truth | Requires HTTP route table + merged deny semantics |
| OAuth MCP HTTP profile | Medium | CONNECT + callout sufficient for v1 | Resource-server on gateway hostname |
| Operator skill base | Medium | NATS ACL + queue groups | Adds L7 ingress, CORS, SSE stickiness |
| Design partner signal | High (gate) | Default until triggers fire | Mandatory when HTTP ingress is non-negotiable |

### Topology (chosen default: on-bus)

```text
                    ON-BUS (Phase 1–2 default)
  ┌─────────────┐         ┌──────────────────┐         ┌─────────────────┐
  │ MCP client  │  NATS   │ trogon-mcp-      │  NATS   │ Backend MCP     │
  │ (SDK/agent) │────────►│ gateway (queue)  │────────►│ server OR       │
  └─────────────┘         └──────────────────┘         │ mcp-nats-stdio  │
        │                         │                    └────────┬────────┘
        │                         │                             │ stdio
        │                         ▼                             ▼
        │                  {prefix}.audit.>              [vendor MCP binary]
        └──────────────── STS / registry / SpiceDB (NATS-adjacent control plane)
```

Hybrid (Phase 3+, feature-flagged) adds a north-south HTTP ear into the same policy engine; bus path remains authoritative for rollback and Trogon-native agents.

**What breaks if this stays undecided:**

- Operator overview and OAuth docs cannot state whether HTTP MCP ingress is supported.
- Audit schema work forks between NATS-only metadata and half-populated HTTP network fields.
- SDK and `agctl` examples assume either NATS-only or dual ingress without a phase gate.
- Third-party onboarding docs either over-promise gateway-embedded HTTP or under-document the stdio bridge path.
- CI and failure-mode matrices double in scope if hybrid is treated as day-one.

## Decision

**On-bus by default for Phase 1 and Phase 2.** All production and near-term roadmap work assumes a single hot-path transport family: NATS request/reply on `{prefix}.gateway.request.>` with queue-group scale-out, inbox correlation, and egress rewrite to `{prefix}.server.*`.

**Hybrid (gateway-embedded HTTP/SSE listener plus existing `mcp-nats-stdio` and remote adapter patterns for backends) is supported behind a feature flag from Phase 3 onward**, only when a customer or design partner requires third-party MCP ecosystem fronting that cannot be satisfied by on-bus sidecars or operator-managed `mcp-nats-server` / `http-bridge` adapters. Hybrid is not a fork of the product line; it is an optional ingress ear on the same policy engine and audit envelope.

Phase mapping:

| Phase | Deployment shape | Gateway HTTP MCP ingress |
|-------|------------------|--------------------------|
| Phase 1 (vertical slice) | On-bus only | Not shipped (`trogon-mcp-gateway` has no axum MCP listener) |
| Phase 2 (policy, sessions, HA) | On-bus only | Remains out of scope; docs state sidecar path for third parties |
| Phase 3+ | On-bus default; hybrid opt-in | `MCP_GATEWAY_HTTP_LISTEN` (proposed) or equivalent feature flag; disabled by default |

v2 trigger checklist from the spec (any two justify prioritizing hybrid implementation):

1. Signed design partner requirement for HTTP MCP ingress without a NATS client.
2. Revenue blocked on MCP Authorization HTTP resource-server on the gateway hostname.
3. Sidecar fleet operational cost exceeds agreed SLO at scale.
4. Documented competitive loss where Trogon policy was named gap vs HTTP-only MCP gateways.

Until triggers fire, mark gateway-embedded HTTP MCP ingress as **proposed / out of Phase 1–2 scope** in dependent identity docs.

## Consequences

### Positive

- **Substrate consistency.** Security reviews, pen tests, and SOC playbooks cover NATS TLS, auth callout, and subject ACL on one transport. STS (`mcp.sts.exchange`), registry, SpiceDB, and JetStream audit stay NATS-adjacent without a parallel HTTP attack surface on the gateway hot path.
- **No extra protocol port on the gateway for MCP JSON-RPC in Phase 1–2.** Operators expose NATS (and optional HTTPS for JWKS/metadata), not a second MCP HTTP listener with CORS, body limits, and SSE lifecycle.
- **Queue-group scale is free.** Any `trogon-mcp-gateway` replica may consume `{prefix}.gateway.request.>`; HA follows NATS semantics documented in [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md). Regional scaling is more gateway pods plus cluster capacity, not L7 stickiness for SSE.
- **Policy parity is trivial in on-bus mode.** CEL `nats.subject` variables align with [reference-subject-grammar.md](../identity/reference-subject-grammar.md) without HTTP route indirection. The same `policy_id` and `decision_reason` audit semantics apply to every gated RPC.
- **Third-party stdio MCP remains first-class** via `mcp-nats-stdio` (or successor bridge) on the backend lane—matching how most community servers ship today without requiring vendors to implement Streamable HTTP.
- **Phase 1 code alignment.** The branch already implements queue-group ingress, inbox correlation, and egress rewrite; choosing on-bus avoids building HTTP ingress before demand signals exist.

### Negative

- **NATS client required for every bus participant.** Pure HTTP agents need an SDK or bridge before calling the gateway; Postman-style HTTP MCP against `trogon-mcp-gateway` itself is unavailable in Phase 1–2.
- **Third-party MCP servers need a bridge.** Stdio vendors connect through `mcp-nats-stdio` (subscribe `{prefix}.server.{server_id}.>`, translate to subprocess stdio). Remote HTTP SaaS uses operator `http-bridge` / `mcp-nats-server` patterns—not gateway-embedded reverse proxy—in Phase 1–2 ([howto-integrate-third-party-mcp.md](../identity/howto-integrate-third-party-mcp.md)).
- **No native HTTP ingress in Phase 1.** MCP OAuth HTTP resource-server profiles on the gateway hostname wait for Phase 3 hybrid; v1 OAuth composition remains NATS CONNECT plus auth callout ([oauth-mcp-integration.md](../identity/oauth-mcp-integration.md)).
- **Per-server sidecar footprint.** Each stdio MCP server typically needs a bridge pod or process; large catalogs increase ops burden versus a centralized HTTP proxy (deferred).
- **Vendor refusal risk.** A partner that will not run any Trogon-managed process and will not operate NATS forces an earlier hybrid pivot—acceptable if scope stayed TrogonStack-native through Phase 2.

### Neutral

- **Hybrid mode is a future feature flag, not a fork.** The same binary (or crate split) can add an HTTP listener later; bus clients and subject grammar remain authoritative. Migration is dual ingress with shared `server_id` and SpiceDB tuples, not rip-and-replace ([on-bus-vs-hybrid.md § Migration](../identity/on-bus-vs-hybrid.md#6-migration-path)).
- **OAuth and HTTP transport research are preserved** for Phase 3; deferring implementation does not delete spec work.
- **A2A north-south patterns** (`a2a-bridge` sketch) demonstrate Trogon can build HTTP bridges without merging A2A and MCP listeners prematurely—MCP HTTP has distinct session and OAuth binding rules.
- **Registry `transport` values** (`nats-native`, `stdio-sidecar`, `http-bridge`) remain valid in both modes; only gateway-embedded north-south HTTP is gated.

## Alternatives considered

### Pure hybrid (HTTP-first from day one)

Deploy `trogon-mcp-gateway` with a north-south HTTP/SSE MCP listener as the primary ingress from the first production release, treating NATS as an optimization for Trogon-native agents rather than the default edge.

**Rejected.** Phase 1 already validated policy, SpiceDB, mesh egress minting, and audit on the bus; rebuilding the hot path around HTTP doubles the security test matrix (CORS, CSRF posture, per-IP rate limits, SSE timeouts, OAuth metadata routes) before a paying design partner requires it. HTTP ingress introduces split-brain policy risk—NATS path enforcing mesh mint while HTTP path skips SpiceDB—and concentrates upstream SaaS secrets in the gateway vault instead of isolated sidecars. Audit must gain normative HTTP network metadata (`client_ip`, `user_agent`, `transport`) before SIEM work in Phase 2, forcing schema churn. The product positioning is an MCP-aware feature inside TrogonStack's event-modeling platform, not a generic API gateway (see [ADR 0017](0017-product-positioning.md)); HTTP-first optimizes for ecosystem familiarity at the cost of differentiated mesh identity and NATS-native audit.

### Dual-listener from day one (NATS + HTTP always on)

Ship both queue-group NATS ingress and HTTP MCP ingress in every environment, with both code paths enabled without a feature flag.

**Rejected.** Operators would run two credential systems, two rate-limit configurations, and two failure-mode matrices for the same `server_id` from the first customer. Session affinity splits between JetStream KV (`mcp-sessions`) and HTTP cookies or MCP HTTP session tokens—design work that Block C session HA has not completed. [reference-subject-grammar.md](../identity/reference-subject-grammar.md) is NATS-complete; hybrid-from-day-one requires a parallel HTTP route reference and explicit mapping to `server_id` before route count is known. Competitive pressure for HTTP-only MCP gateways does not yet justify spreading Phase 2 capacity across HTTP infrastructure instead of catalog shaping, hierarchical policy merge, and decider integration (Blocks D–F). Dual-listener without demand duplicates the hybrid cons while forfeiting the on-bus pros that Phase 1 code already embodies.

## Implementation notes

### NATS auth callout and connection identity

Bus clients obtain a **bootstrap NATS User JWT** at CONNECT from the auth callout. ACLs grant publish on `{prefix}.gateway.request.>` (and the caller callback subtree) but not `{prefix}.server.*`—backends and sidecars use backend-zone credentials. The gateway validates mesh JWTs on ingress in enforce mode and mints downstream tokens on egress; it does not forward inbound bearer tokens unchanged ([ADR 0003](0003-bootstrap-vs-mesh-tokens.md), [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md)).

HTTP hybrid (Phase 3+) adds bearer validation and optional mTLS at the gateway edge before the same STS → mesh → egress chain; NATS CONNECT path remains unchanged for bus clients.

### Subject grammar

Authoritative ingress subscribe pattern:

```
{prefix}.gateway.request.>
```

Per-request rewrite:

```
{prefix}.gateway.request.{server_id}.{method_suffix}
  → {prefix}.server.{server_id}.{method_suffix}
```

Callback lanes: `{prefix}.client.{client_id}.>` and `{prefix}.gateway.callback.{client_id}.>` per [reference-subject-grammar.md](../identity/reference-subject-grammar.md). Default `MCP_PREFIX=mcp` yields `mcp.gateway.request.>` in examples.

Tenancy is **not** a subject segment ([ADR 0001](0001-tenancy-model.md)); isolation is NATS Account and/or JWT `tenant` claim.

### Control plane (unchanged in both modes)

Regardless of ingress family, the gateway internal control plane stays NATS-native:

| Concern | Subject / surface |
|---------|-------------------|
| STS exchange | `mcp.sts.exchange` ([ADR 0004](0004-sts-form-factor.md)) |
| Agent registry lookup | `mcp.registry.agent.lookup` |
| JetStream audit | `{prefix}.audit.{outcome}.{direction}.{method_root}` |
| SpiceDB | gRPC from gateway worker |
| Session / policy KV | `mcp-sessions`, `mcp-gateway-config`, trust bundles |

Hybrid adds HTTP edge validation before the same chain; it does not replace STS or registry transports.

### Third-party integration (on-bus path)

Documented Phase 1–2 operator path ([howto-integrate-third-party-mcp.md](../identity/howto-integrate-third-party-mcp.md)):

1. Register `server_id` in the agent registry with `transport: stdio-sidecar` or `nats-native`.
2. Run **`mcp-nats-stdio`** (or `trogon-mcp-bridge serve-stdio` when available) subscribed to `{prefix}.server.{server_id}.>`.
3. Pass vendor secrets only to the sidecar environment; never to clients or the gateway hot path.
4. Verify gated traffic via `{prefix}.gateway.request.{server_id}.tools.call` with bootstrap → STS → mesh token flow.

Remote HTTP SaaS MCP uses operator-managed **`mcp-nats-server`** or `http-bridge` registry transport—not gateway-embedded HTTP—in Phase 1–2.

### Phase 3 hybrid (feature-flagged)

When enabled:

- Gateway crate surface adds MCP Streamable HTTP / SSE listener (route prefix and handler split TBD in a follow-on ADR once a design partner specifies the MCP HTTP profile).
- Internal path may publish to `{prefix}.gateway.request.>` for policy unity (proposed shadow/canary pattern in spec migration §6.1).
- OAuth Protected Resource Metadata and `401` + `WWW-Authenticate` attach to the HTTP listener ([oauth-mcp-integration.md](../identity/oauth-mcp-integration.md)).
- Audit envelope extends with proposed `transport`, `client_ip`, `user_agent` fields ([reference-audit-envelope.md](../identity/reference-audit-envelope.md)).
- Env gate (proposed): `MCP_GATEWAY_HTTP_LISTEN=0` default off; canary LB before fleet-wide enablement.

`mcp-nats-stdio` remains the stdio bridge in **both** modes; hybrid adds client ingress over HTTP, not a replacement for backend sidecars.

### Code and config touchpoints

| Area | Phase 1–2 (on-bus) | Phase 3+ (hybrid flag) |
|------|-------------------|-------------------------|
| `rsworkspace/crates/trogon-mcp-gateway` | `gateway.rs` queue group on `{prefix}.gateway.request.>` | Optional HTTP listener module |
| NATS ACL templates | Client publish `gateway.request`; gateway publish `server.*` | Unchanged for bus; HTTP auth at edge |
| `mcp-nats-stdio` | Backend lane bridge | Same |
| Identity docs | State HTTP ingress not supported v1 | Promote OAuth HTTP sections from proposed to normative |

## Status of supporting work

| Item | Status |
|------|--------|
| Phase 1 vertical slice (queue group, CEL, SpiceDB, audit) | **Done** |
| [on-bus-vs-hybrid.md](../identity/on-bus-vs-hybrid.md) design paper | **Done** (2026-05-28); distilled by this ADR |
| [howto-integrate-third-party-mcp.md](../identity/howto-integrate-third-party-mcp.md) | **Draft / in progress** (Block H); aligned with on-bus + `mcp-nats-stdio` |
| [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) | **Needs hygiene** — explicit "no HTTP MCP ingress Phase 1–2" callout |
| [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md) | **Needs hygiene** — HTTP listener sections marked v2/proposed until Phase 3 |
| Gateway HTTP MCP listener implementation | **Pending** — Phase 3, feature-flagged |
| `reference-http-route-grammar.md` (proposed) | **Pending** — if hybrid route count warrants |

## Documentation hygiene (post-acceptance)

| Document | Required update |
|----------|-----------------|
| [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) | State HTTP MCP ingress **not supported** Phase 1–2; sidecar is third-party path |
| [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md) | Keep HTTP listener sections as **Phase 3 / proposed** until hybrid ships |
| [howto-integrate-third-party-mcp.md](../identity/howto-integrate-third-party-mcp.md) | Demote implication that gateway terminates vendor HTTP; elevate `mcp-nats-stdio` |
| [integration-touchpoints.md](../identity/integration-touchpoints.md) | No HTTP ingress row until Phase 3; footnote to this ADR |

Do not delete OAuth or HTTP research—Phase 3 hybrid reuses the existing specs verbatim with implementation tickets.

## FAQ

**Does on-bus mean Trogon-native servers only?**

No. Third-party servers are on-bus when a sidecar or remote adapter subscribes to `{prefix}.server.{server_id}.>`. "On-bus" describes the gateway-facing transport, not the vendor's internal stdio.

**Can a tenant run HTTP in front of NATS without Trogon hybrid?**

Yes. An operator-managed HTTP→NATS bridge outside `trogon-mcp-gateway` is a customer integration pattern, not Trogon hybrid mode. Trogon hybrid means **this gateway binary** terminates MCP HTTP with unified policy and audit.

**Does this block OAuth work in Phase 1–2?**

No. OAuth composition at NATS CONNECT remains in scope. HTTP OAuth discovery on the gateway hostname waits for Phase 3 hybrid.

## Open questions (for deciders)

1. **Exact feature flag name and default** — `MCP_GATEWAY_HTTP_LISTEN` vs compile-time feature vs tenant KV override.
2. **HTTP route → `server_id` mapping** — single table in `mcp-gateway-config` KV vs convention-based paths.
3. **Whether Phase 3 hybrid includes gateway reverse-proxy to upstream HTTP MCP** or only client ingress while backends stay on-bus.
4. **SIEM minimum metadata** — which HTTP audit fields are required before hybrid can ship to regulated customers.
5. **Per-tenant hybrid enablement** — account-level flag vs global operator flag ([ADR 0001](0001-tenancy-model.md) hybrid tenancy model).

---

*Design context: [docs/identity/on-bus-vs-hybrid.md](../identity/on-bus-vs-hybrid.md). This ADR is the formal decision record; the spec document remains the long-form explanation, comparison matrices, and migration sequences.*

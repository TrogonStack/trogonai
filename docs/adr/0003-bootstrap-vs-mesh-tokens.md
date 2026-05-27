# ADR 0003 — Bootstrap vs. Mesh Tokens

## Status

**Accepted (2026-05-27).** Authorizes implementation of STS exchange (Block 2.1) and gateway egress minting (Block 2.4).

## Context

TrogonStack's MCP and A2A gateways today authenticate callers at the NATS perimeter. The **`a2a-auth-callout`** service (and the MCP auth-callout analogue) terminates external credentials—OIDC, mTLS, API key—and mints a **short-lived NATS User JWT** at connect time. That JWT carries:

- Standard claims: `sub`, `aud` (tenant NATS Account), `exp` / `iat`, `kid`
- Custom claims: `caller_id`, `data` (SpiceDB-ready principal payload)
- NATS subject permissions: publish on `{prefix}.gateway.>`, subscribe on `_INBOX.{caller_id}.>` and push/callback subtrees

Once connected, the caller uses this JWT for **every** gateway interaction. The **`trogon-mcp-gateway`** validates the JWT at ingress (`MCP_GATEWAY_JWT_*` phased `off` / `validate` / `require`), derives identity for SpiceDB and CEL, and **propagates verified context** to backends via gateway-set headers (`mcp-caller-sub`, `mcp-tenant`, etc.). Per `MCP_GATEWAY_PLAN.md` § Wire-Format Pins, the gateway **strips** any client-supplied identity headers on ingress—clients cannot forge `mcp-caller-sub`, `mcp-tenant`, `mcp-instance-id`, or `mcp-schema`.

This model proves **perimeter authentication and gateway enforcement**. It does **not** implement Uber's agent-delegation identity plane:

| Capability | Today | Uber target (`PENDING_TODO.md`) |
|---|---|---|
| Connect-time credential | Auth-callout User JWT | Bootstrap credential only |
| Token scope | Broad NATS subject ACL + gateway policy | Narrow `aud` per hop, short TTL |
| Cross-agent / cross-backend hop | Same JWT end-to-end | STS exchange before each hop |
| Delegation lineage | None | `act_chain` appended at each exchange |
| Gateway egress | Propagate verified headers | Mint downstream STS token; drop inbound |

`PENDING_TODO.md` Block 0 states this decision is **the single most consequential choice** in the agent-identity gap analysis. It determines whether Block 2 (STS, `act_chain`, gateway egress minting) is **additive enrichment** of the current JWT or a **replacement** of the connect-time JWT as the mesh credential.

Reference: [Uber — Solving the Agent Identity Crisis](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/).

### Current token lifecycle (simplified)

```
External cred ──▶ auth-callout ──▶ User JWT (TTL ~300s, broad ACL)
                                        │
                    connect + all hops ─┤
                                        ▼
                              gateway ingress (validate JWT)
                                        │
                              propagate mcp-caller-sub, mcp-tenant
                                        ▼
                              backend MCP server / next agent
```

The question: **Is the auth-callout JWT the only credential needed for the entire call graph, or a bootstrap that must be exchanged at an STS before any cross-agent or gateway-to-backend hop?**

---

## Options

### (a) Auth-callout JWT is the sole credential

The connect-time User JWT remains authoritative for NATS subject ACL **and** gateway policy for the full session. No STS, no per-hop exchange.

| Aspect | Behavior |
|---|---|
| NATS ACL | Broad `{prefix}.gateway.>` publish; unchanged |
| Gateway ingress | Validate auth-callout JWT; CEL/SpiceDB as today |
| Gateway egress | Propagate verified context headers (current commit behavior) |
| Cross-agent calls | Caller presents same JWT at each gateway ingress |
| Block 2 work | **Additive only** — optional `agent_id`, `wkl`, `act_chain` claims for audit/CEL; no hard `aud` gate |

**Pros:** Zero new latency on the hot path; smallest implementation cost; no STS dependency; trivial migration from current branch.

**Cons:** Leaked JWT grants full gateway publish scope until expiry; no per-hop audience narrowing; audit cannot distinguish "token minted for hop A" vs. "token reused at hop C"; cross-agent delegation is policy-simulated, not cryptographically bound; misaligned with Uber reference and `PENDING_TODO.md` Block 2.4 target.

### (b) Auth-callout JWT is a bootstrap; STS issues mesh tokens per hop

Connect-time JWT proves **who connected to NATS** and authorizes **edge-zone subject ACL only**. Before any cross-agent call or gateway-to-backend egress, the caller (or gateway on behalf of the caller) exchanges the bootstrap token at an STS for a **short-lived, audience-scoped mesh JWT** with appended `act_chain` entry.

| Aspect | Behavior |
|---|---|
| NATS ACL | Bootstrap JWT retains edge ACL; mesh JWT may carry tighter claims only (no broader NATS permissions than bootstrap) |
| Gateway ingress | Validate **mesh token** where enforce mode applies; bootstrap alone insufficient for gated RPCs |
| Gateway egress | Gateway calls STS with `(subject_token=bootstrap or prior mesh, audience=backend_id)`; attaches mesh token; **drops inbound token** (`PENDING_TODO.md` Block 2.4) |
| Cross-agent calls | A2A SDK: `lookup → exchange(aud=target) → send` |
| Block 2 work | **Replacement** — STS, registry, workload attestation, `act_chain` are on the critical path |

**Pros:** Minimal blast radius per hop; cryptographic `aud` binding; clear audit lineage; matches Uber model and downstream ADRs (0004–0006); enables purpose/chain-aware policy (Block 2.3, Block 5).

**Cons:** STS on critical path (fail-closed stops mesh); P99 latency budget (~40 ms per Uber) requires caching and hot-path optimization; largest implementation and migration cost; new signing key custody (ADR 0006).

### (c) Hybrid — bootstrap for read/list; exchange for write/tools-call

Bootstrap JWT suffices for **low-risk, idempotent** operations (`tools/list`, `resources/list`, `prompts/list`, `ping`, `initialize`). **Mutating or sensitive** operations (`tools/call`, `resources/read`, callbacks with side effects) require a prior STS exchange yielding a mesh token with narrowed `aud` and `scope`.

| Aspect | Behavior |
|---|---|
| Gateway ingress | Dual path: bootstrap-only for allowlisted methods; mesh token required for gated methods |
| Gateway egress | Exchange before backend publish on gated methods; propagate headers on list-only paths |
| Cross-agent | Exchange required when target method is in the gated set |
| Block 2 work | **Partial replacement** — STS required for write path only; read path stays on bootstrap until a later tighten |

**Pros:** Reduces STS call volume (~list-heavy MCP traffic); eases migration (shadow mesh validation on reads first); contains blast radius on highest-risk operations; lower initial latency impact.

**Cons:** Two credential types on the wire indefinitely unless later unified; policy and SDK complexity ("which token do I attach?"); read paths still leak bootstrap scope; audit asymmetry between read and write hops; CEL and ingress hardening must reason about both token types.

---

## Decision drivers

| Driver | Weight | Favors |
|---|---|---|
| **Blast radius of leaked token** | High | (b) > (c) > (a) |
| **Latency budget** (Uber P99 ~40 ms per exchange; gateway adds one exchange on egress) | High | (a) > (c) > (b) |
| **Implementation cost** | Medium | (a) > (c) > (b) |
| **Audit clarity / agent-centric lineage** | High | (b) > (c) > (a) |
| **Migration cost from current branch** | Medium | (a) > (c) > (b) |
| **Alignment with Uber / `PENDING_TODO` target** | High | (b) > (c) >> (a) |
| **Operational resilience** (STS outage) | Medium | (a) > (c) > (b) — all fail-closed except dangerous degraded-mode escape hatches |

**Latency note:** MCP gateway egress exchange is cacheable by `(caller_sub, target_aud, session_id, scope)` up to TTL/2 (`PENDING_TODO.md` Block 2.4). Bootstrap-only option (a) avoids STS entirely but externalizes risk to gateway policy and broad NATS ACL.

**Audit note:** Option (a) can log `act_chain`-shaped data from headers, but it is not cryptographically bound to the credential the backend trusts. Options (b) and (c) embed lineage in the signed mesh JWT.

---

## Recommendation

**Adopt Option (b) as the target architecture.** Use **Option (c) only as a transitional enforce-mode profile** during migration—not as the permanent steady state.

### Rationale

1. **Security matches the threat model.** A connect-time JWT with `{prefix}.gateway.>` publish permission is a high-value bearer. Uber's model treats it as bootstrap precisely because perimeter proof ≠ per-hop authorization. Leaked bootstrap still matters, but mesh tokens limit damage to one audience and one TTL window.

2. **Block 2 is designed as replacement, not decoration.** `PENDING_TODO.md` Block 2.4 explicitly requires gateway egress to **mint, not propagate**. That only makes sense if the inbound credential is not the same artifact the backend should trust. Option (a) would leave Block 2 as optional telemetry; most of the Uber gap would remain.

3. **Audit and adaptive access depend on signed chains.** Block 4 (agent-traffic view) and Block 5 (step-up, human-in-the-loop) require a verifiable `act_chain` in the credential backends and SIEM consume—not reconstruct from gateway logs.

4. **Latency is manageable with caching and phased STS rollout.** A single cached egress exchange per `(session, backend)` stays within the 40 ms P99 budget when registry/trust-bundle are memory-resident (Block 2.1). Option (c) saves STS calls but perpetuates dual-mode complexity; the savings do not justify permanent architectural split given list traffic is already shaped by CEL without backend-side trust of list credentials.

5. **Migration does not require big-bang cutover.** Shadow mode can validate mesh tokens alongside bootstrap before enforce (see below). Option (c)'s method-split is useful **during** shadow→enforce, then retired in favor of full (b).

---

## Consequences

### Block 2 scope: replacement, not additive

| Area | Under accepted ADR |
|---|---|
| STS (Block 2.1) | **Required** on mesh hot path |
| `act_chain` (Block 2.2) | **Required**; appended at every exchange |
| Gateway egress (Block 2.4) | **Replace** header propagation with STS mint + `mcp-mesh-token` (or equivalent) header; strip inbound bearer |
| A2A SDK (Block 3) | **Must** call STS before `call()`; bootstrap alone is invalid for cross-agent sends |
| Auth-callout | **Unchanged role** — still mints bootstrap at connect; does not become STS |

### Gateway egress behavior

Today (`MCP_GATEWAY_PLAN.md` § Wire-Format Pins): gateway sets `mcp-caller-sub`, `mcp-tenant` from validated JWT and strips client forgeries.

After this ADR:

```
ingress: validate mesh JWT (or bootstrap in shadow/off)
         strip client identity headers + inbound bearer
egress:  STS.exchange(subject=validated_token, aud=target_backend, purpose=…)
         attach mesh JWT on backend message (header TBD in wire-format pin)
         do NOT forward bootstrap or inbound mesh token verbatim
audit:   envelope records act_chain, aud, iss=sts, bootstrap_sub (reference only)
```

Ingress hardening extends to strip client-supplied `act_chain`, `agent_id`, `wkl` (`PENDING_TODO.md` Block 1.3) in addition to existing pinned headers.

### Bootstrap JWT role (unchanged but narrowed semantics)

- Proves NATS connect identity and edge-zone subject ACL
- Acts as `subject_token` input to STS
- **Not** accepted as sole credential for gateway-gated RPCs in enforce mode
- TTL remains ~300 s at callout; mesh TTL governed by ADR 0005 (target 60–300 s)

### Dependencies on sibling ADRs

| ADR | Dependency |
|---|---|
| 0001 Tenancy model | STS `aud` naming, audit scoping |
| 0002 Identity layers | `agent_id`, `wkl`, `user_sub` in exchange inputs |
| 0004 STS form factor | NATS `mcp.sts.exchange` vs HTTP vs embedded |
| 0005 TTL and `aud` discipline | Mesh token lifetime, hard CEL gate on `aud` |
| 0006 Signing keys | STS mints; gateway validates JWKS |

### Explicit non-goals under this ADR

- Degraded-mode "skip STS when down" for production (fail-closed)
- User clients calling STS directly from browser/UI (mesh-internal unless ADR 0004 says otherwise)

---

## Migration path

Phasing mirrors `MCP_GATEWAY_AGENT_IDENTITY` and existing `MCP_GATEWAY_JWT_*` modes (`PENDING_TODO.md` Block 6).

### Phase 0 — Today (unchanged)

Auth-callout bootstrap + gateway JWT ingress + header propagation. Document bootstrap as **provisional** per `PENDING_TODO.md` verified snapshot.

### Phase 1 — Shadow (`MCP_GATEWAY_AGENT_IDENTITY=shadow`)

- Deploy STS, registry (minimal), claim schema (Block 1.3)
- Gateway and SDK **perform exchanges** but bootstrap remains authoritative for allow/deny
- Log violations: missing mesh token, `aud` mismatch, invalid `act_chain`, depth exceeded
- Egress: run STS mint in parallel; compare claims to propagated headers; audit delta
- **Optional hybrid (c):** enforce exchange only on `tools/call` / `resources/read` in shadow metrics first

**Exit criteria:** Shadow violations near zero for 7–14 days; STS P99 < 40 ms; rollback runbook reviewed.

### Phase 2 — Enforce (`MCP_GATEWAY_AGENT_IDENTITY=enforce`)

- Gateway rejects gated RPCs without valid mesh JWT and matching `aud`
- Egress mint mandatory; inbound bearer stripped
- Bootstrap alone: NATS connect + non-gated methods only (if hybrid retained temporarily)
- Rate limits keyed by `(agent_id, purpose, tenant)` where claims present

### Phase 3 — Full mesh (retire hybrid)

- All gateway-handled methods require mesh token at ingress (including `*/list`)
- Remove dual-token SDK paths
- Backend trust: mesh JWT only (headers are hints, not authority)

### Agent backfill (parallel)

1. Inventory bootstrap callers → assign `agent_id`
2. Register in agent registry (Block 1.1)
3. Provision workload attestation (Block 1.2; file-based SVID in dev)
4. Cut over per agent team with shadow → enforce per tenant

### Rollback

Flip `MCP_GATEWAY_AGENT_IDENTITY` to `shadow` or `off`; bootstrap path remains wired. STS outage runbook: fail-closed in enforce (correct); temporary rollback to shadow restores service at elevated risk (documented exception, time-boxed).

---

## Open questions

1. **Bootstrap on NATS reconnect:** Does a reconnect re-bootstrap and invalidate in-flight mesh tokens tied to the prior session, or are mesh tokens session-independent?
2. **Mesh token on the NATS wire:** Separate JWT header vs. re-minted User JWT with unchanged subject ACL but enriched claims—which minimizes NATS server and gateway changes?
3. **Gateway's own hop:** Does the gateway append itself to `act_chain` as an actor (privileged workload SPIFFE) or only as infrastructure (`iss` metadata)? Related: does the gateway have an `agent_id` (`PENDING_TODO.md` open questions)?
4. **Callback direction:** Server → client callbacks (`mcp.gateway.callback.*`)—exchange with `aud=client_id` before egress is likely required; confirm symmetric enforce rules.
5. **Bridge reminting:** `a2a-bridge` already mints per-request caller JWTs—is that a second bootstrap or an ad-hoc STS? Align with ADR 0004.
6. **Hybrid sunset date:** If (c) is used during migration, what metric triggers retiring bootstrap-only list paths?
7. **OAuth-MCP composition:** Does the OAuth access token map to bootstrap only, with STS downstream (`MCP_GATEWAY_PLAN.md` Block C open item)?
8. **Offline dev:** File-based SVID + local STS mock sufficient for CI, or require SPIRE in integration tests?

---

## References

- `PENDING_TODO.md` — Block 0 (bootstrap decision), Block 2.1 (STS), Block 2.4 (gateway egress), Block 6 (migration)
- `MCP_GATEWAY_PLAN.md` — § Wire-Format Pins (headers, ingress hardening), § Audit envelope, § CEL namespace
- `docs/a2a/explanation/auth-callout-design.md` — current bootstrap JWT layout
- [Uber — Solving the Agent Identity Crisis](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/)
- RFC 8693 — OAuth 2.0 Token Exchange (STS contract baseline)

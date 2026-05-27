# Agent identity overview

**Status:** Operator reference (2026-05-27). Anchors ADRs 0001–0006 and downstream implementation.

---

## Mental model

Trogon mesh identity separates **who connected** from **who may act at this hop**.

A **bootstrap NATS User JWT** (auth-callout at CONNECT) proves perimeter identity and NATS subject ACL. It is **not** the credential backends trust in enforce mode ([ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md)). Before every cross-agent call or gateway-to-backend egress, the caller exchanges at the **Security Token Service (STS)** on `mcp.sts.exchange` ([ADR 0004](../adr/0004-sts-form-factor.md)) for a **mesh token**: short-lived (default 120 s, range 60–300 s), audience-scoped, with an appended `act_chain` entry.

Identity uses **two claims for the current hop** ([ADR 0002](../adr/0002-identity-layers.md)): `sub` names the logical actor (human, `agent:{tenant}/{name}`, or gateway principal); `wkl` names the attested workload (SPIFFE ID or documented sentinel such as `sentinel:human`). On STS exchange, **`wkl` is derived from the caller’s X.509 SVID** when `MCP_STS_REQUIRE_ATTESTATION=1`; otherwise shadow mode accepts claim-shaped `actor_token` values and audits `wkl_unattested`. Trust bundles for verification are distributed on NATS KV `mcp-trust-bundles/<trust-domain>`. Optional `agent_id` binds a registered agent when `wkl` is an attested agent workload. `originator_sub` and `act_chain[0]` preserve the chain root across hops.

**Per-hop exchange** means each receiver validates `aud` equals its own identity URI ([ADR 0005](../adr/0005-token-ttl-and-audience.md)). Tool restrictions live in `scope`, not `aud`. The gateway **mints** downstream tokens on egress; it does not propagate inbound bearers.

**`act_chain`** ([act-chain.md](act-chain.md)) is an append-only array of `{ sub, agent_id?, wkl, iat }` entries—oldest originator first, current actor last. STS copies the inbound chain and appends one entry per successful exchange. Integrity is the outer JWT signature only. Default max depth: 8 entries; loop detection rejects duplicate `(agent_id, wkl)` pairs.

**Tenancy** ([ADR 0001](../adr/0001-tenancy-model.md)): production uses NATS account per tenant; JWT `tenant` remains the portable field for audit, CEL, and SpiceDB. STS, registry, and audit scope all work inside one tenant boundary.

---

## Four-hop call (`user → agent A → agent B → backend`)

```
  user          agent A              agent B           MCP gateway         backend
   │               │                    │                  │                  │
   │ bootstrap     │                    │                  │                  │
   │ (auth-callout)│                    │                  │                  │
   │               │                    │                  │                  │
   │──call A──────►│                    │                  │                  │
   │  STS: user    │                    │                  │                  │
   │  aud=urn:trogon:a2a:agent:acme:agent-a               │                  │
   │  chain: [user]│                    │                  │                  │
   │               │                    │                  │                  │
   │               │──call B───────────►│                  │                  │
   │               │  STS: A            │                  │                  │
   │               │  aud=urn:trogon:a2a:agent:acme:agent-b                  │
   │               │  chain: [user, A]  │                  │                  │
   │               │                    │                  │                  │
   │               │                    │──tools/call─────►│                  │
   │               │                    │  (mesh token)    │ STS: B           │
   │               │                    │                  │ aud=urn:trogon:mcp:backend:acme:github
   │               │                    │                  │ chain: [user,A,B]│
   │               │                    │                  │ (+gateway hop)   │
   │               │                    │                  │─────────────────►│
   │               │                    │                  │  mesh JWT        │
   │               │                    │                  │  aud=backend     │
   │               │                    │                  │  chain len 4     │
```

At each STS call: **caller** presents `subject_token` + `actor_token` (SVID); **aud** names exactly one downstream URI; **act_chain** grows by one entry for the exchanger.

---

## Claim cheatsheet

Full schema: [jwt-claim-schema.md](jwt-claim-schema.md).

| Claim | Bootstrap | Mesh | Notes |
|---|---|---|---|
| `iss`, `exp`, `iat`, `sub` | Required | Required | Mesh `iss` = STS |
| `aud` | NATS account | Target hop URI | Per [ADR 0005](../adr/0005-token-ttl-and-audience.md) |
| `tenant` | Optional | Required | [ADR 0001](../adr/0001-tenancy-model.md) |
| `caller_id`, `data`, `nats` | Required | Absent | Bootstrap NATS ACL only |
| `wkl`, `auth_method` | Optional (shadow) | **Required** (enforce) | `wkl_attested_at` when SPIFFE |
| `agent_id`, `agent_version` | Optional | Required for agent workloads | Registry-bound |
| `originator_sub` | First hop sets | Preserved | Immutable chain root |
| `act_chain` | Optional (1 entry) | Required when delegated | [act-chain.md](act-chain.md) |
| `purpose`, `scope` | Optional | Optional / narrowed | Tools in `scope`, not `aud` |
| `session_id` | — | Recommended | Binds egress cache |

---

## Audience URI shapes ([ADR 0005](../adr/0005-token-ttl-and-audience.md))

| URI pattern | Use |
|---|---|
| `urn:trogon:mcp:gateway:{tenant}:{gateway_instance_id}` | Ingress to a specific MCP gateway instance |
| `urn:trogon:mcp:backend:{tenant}:{server_id}` | Gateway egress to an MCP backend server |
| `urn:trogon:a2a:agent:{tenant}:{agent_id}` | Agent-to-agent hop (target agent service) |
| `urn:trogon:mcp:client:{tenant}:{client_id}` | Server→client callback direction |

---

## Failure modes

| Condition | Enforce behavior | Code / audit |
|---|---|---|
| Missing `wkl` (agent/SVID path) | Reject at gateway ingress | `-32110` invalid_token class |
| `aud` ≠ receiver identity | Reject before policy | `-32109` `audience_mismatch` |
| `act_chain` depth > 8 | Reject; STS must not mint | `-32113` `act_chain_depth_exceeded` |
| `act_chain` loop / malformed | Reject | `-32114` … `-32115` ([act-chain.md](act-chain.md)) |
| STS unavailable | **Fail-closed** — no bypass | Gateway/backend error; agents retry with backoff |
| Registry stale / agent revoked | Reject at STS or verifier | STS `invalid_target`; audit deny |
| Expired mesh token | Reject | `-32106` `auth_expired` |

Shadow mode logs the same violations with `would_deny: true` but allows the bootstrap path ([Block 6](../../PENDING_TODO.md)).

---

## Rollout modes (`MCP_GATEWAY_AGENT_IDENTITY`)

| Mode | Behavior |
|---|---|
| `off` (default) | Ignore mesh claims for authz; bootstrap authoritative |
| `shadow` | Full validation + audit; bootstrap still authoritative for allow/deny; exchange runs in parallel for metrics |
| `enforce` | Mesh token required for gated RPCs; hard reject on violations; egress mint mandatory |

Cutover: zero shadow violations for 7 days, STS P99 < 40 ms, then flip to `enforce`. Rollback: set `shadow`, drain mesh cache ([ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md), [Block 6](../../PENDING_TODO.md)).

---

## References

| ADRs | Spec docs |
|---|---|
| [0001 Tenancy](../adr/0001-tenancy-model.md) | [JWT claim schema](jwt-claim-schema.md) |
| [0002 Identity layers](../adr/0002-identity-layers.md) | [Actor chain](act-chain.md) |
| [0003 Bootstrap vs mesh](../adr/0003-bootstrap-vs-mesh-tokens.md) | [STS exchange](sts-exchange.md) |
| [0004 STS form factor](../adr/0004-sts-form-factor.md) | [Agent registry](registry.md) |
| [0005 TTL and audience](../adr/0005-token-ttl-and-audience.md) | [A2A SDK contract](sdk.md) |
| [0006 Signing keys](../adr/0006-mesh-token-signing-keys.md) | [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) |

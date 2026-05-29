# ADR 0002 — Identity Layering: Workload vs. Agent vs. User

## Status

**Accepted (2026-05-27).** Drives Block 1 (registry, attestation, claim schema) and the downstream ADRs 0003–0006.

## Context

The TrogonAI MCP gateway and A2A surface today authenticate callers at the NATS perimeter and enforce policy on ingress. A validated mesh JWT exposes roughly:

| Claim / field | What the gateway sees today |
|---|---|
| `jwt.sub` | External identity from OIDC, mTLS, or API key (see auth-callout design) |
| `jwt.tenant` | Tenant boundary |
| `jwt.roles` | Coarse authorization roles |
| `caller_id` | Token-safe NATS ACL segment (not the same as `sub`) |

This is sufficient for **perimeter authentication** and subject ACL binding. It is **not** sufficient for agent-delegation semantics described in [Uber's agent identity model](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/):

- No distinction between the **workload** running code (process/pod) and the **agent** (registered logical actor).
- No stable **originator** (human or system principal) separate from the current hop.
- No registry binding between `agent_id` and allowed workloads.
- Audit and CEL policy reason over `jwt.sub` as a single flat principal.

Block 0 requires a canonical **identity triplet** before wire-format pins, STS exchange (ADR 0003), or registry implementation proceed.

### Current branch snapshot (relevant)

**Already present:** auth-callout minting with `sub`, `caller_id`, tenant; gateway JWT validation; Tier-1/2/3 policy; audit envelopes with `caller.sub`; bridge per-request reminting foundation.

**Missing:** Agent Registry, workload attestation (SPIFFE/SVID), STS token exchange, per-hop mesh tokens, `act_chain`, `purpose`, and agent-centric audit views.

Treat the auth-callout User JWT as a **bootstrap credential** until ADR 0003 decides exchange requirements. This ADR defines what identity *layers* mean inside bootstrap and mesh tokens alike.

---

## The identity triplet

Three orthogonal identifiers answer three different questions. All three may appear in a single JWT and in each `act_chain` entry (Block 2.2).

### 1. `workload_id` → JWT claim `wkl`

**Question:** *Where is this code running right now?*

| Property | Value |
|---|---|
| Canonical form | SPIFFE ID URI, e.g. `spiffe://acme.local/ns/prod/sa/oncall-agent` |
| Source of truth | Workload attestation (SPIRE SVID, mTLS SAN, or documented dev fallback) |
| JWT claims | `wkl` (string), `wkl_attested_at` (RFC 3339 timestamp, set by STS or auth-callout when attestation succeeds) |
| Trust | Must be validated against a trust bundle before minting or accepting a mesh token |

Non-SPIFFE callers use **sentinel** values (see Decision points below), not fabricated SPIFFE paths.

### 2. `agent_id` → JWT claim `agent_id`

**Question:** *Which registered agent definition is acting?*

| Property | Value |
|---|---|
| Canonical form | Namespaced string: `{tenant}/{agent_name}` (e.g. `acme/oncall-agent`). Opaque UUID allowed only as a registry alias; namespaced form is preferred for humans and audit. |
| Source of truth | Agent Registry (`mcp.registry.agent.lookup`) |
| Related claims | `agent_version` (semver or digest pin), optional `agent_definition_digest` |
| Trust | STS/registry must confirm `wkl ∈ agent.allowed_workloads` before minting |

The registry record (Block 1.1) binds `agent_id` to owner, version, definition digest, allowed workloads, allowed tools, and lifecycle state.

### 3. `user_sub` → JWT claim `originator_sub` (and `act_chain[0].sub`)

**Question:** *On whose behalf did this chain start?*

| Property | Value |
|---|---|
| Canonical form | Stable external subject from the IdP or service account that initiated the chain, e.g. `oidc|tenant-acme|user-7f3a…` (same encoding auth-callout uses today) |
| Source of truth | First hop — OIDC token at UI login, service account at batch start, or explicit system principal |
| Propagation | Immutable once set; preserved across STS exchanges; duplicated as first entry in `act_chain` |
| Absence | Batch/system jobs with no human may omit `originator_sub`; `act_chain[0]` then uses a documented system principal |

**Current-hop actor** remains available as `jwt.sub` (see relationship to `wkl` below). `originator_sub` is specifically the chain root, not necessarily the presenter of the current token.

### Triplet example (four-hop agent chain)

```
originator_sub = oidc|acme|alice          # human who clicked "run"
act_chain[0]   = { sub: alice,  agent_id: null,           wkl: sentinel:human }
act_chain[1]   = { sub: agent:acme/router, agent_id: acme/router, wkl: spiffe://…/router }
act_chain[2]   = { sub: agent:acme/oncall, agent_id: acme/oncall, wkl: spiffe://…/oncall }
act_chain[3]   = { sub: agent:acme/oncall, agent_id: acme/oncall, wkl: spiffe://…/oncall }  # current token
```

---

## Decision points

### D1 — Is `agent_id` mandatory on every mesh JWT?

| Option | Description |
|---|---|
| **A. Always required** | Every token carries `agent_id`; humans map to a synthetic "passthrough" agent. Simple CEL; awkward registry churn. |
| **B. Required only for agent-originated calls** | Direct human/tool calls omit `agent_id`; agent hops require it. Matches Uber's registry model. |
| **C. Required at gateway enforce mode only when `wkl` is a real SPIFFE workload** | Hybrid: sentinels skip `agent_id`; attested workloads must have one. |

**Recommendation: B**, with gateway enforce mode rejecting tokens where `wkl` matches `spiffe://*` and `agent_id` is absent (see Recommendation).

Rationale: forcing registry entries for every human UI session adds noise; agent-specific policy (`jwt.agent_id`, allowed tools, purpose) applies only when an agent is actually acting.

### D2 — How are non-agent callers represented?

All caller classes must populate `wkl` (enforce mode rejects missing `wkl`) and `auth_method`. `agent_id` is absent unless an agent is delegated.

| Caller class | `wkl` | `auth_method` | `agent_id` | `originator_sub` |
|---|---|---|---|---|
| Human via UI (OIDC) | `sentinel:human` | `oidc` | *(absent)* | OIDC `sub` |
| Human via UI (future OAuth-MCP) | `sentinel:human` | `oauth-mcp` | *(absent)* | resource owner `sub` |
| Batch / cron job | Attested SPIFFE of runner **or** `sentinel:batch` | `service-account` | *(absent)* | `system:batch` or job SA `sub` |
| API key (transitional) | `sentinel:api-key` or mTLS-derived SPIFFE | `api_key` | *(absent)* | mapped principal `sub` |
| Registered agent | Attested SPIFFE from `allowed_workloads` | `svid` / `spire` | `{tenant}/{name}` | preserved from chain |
| Gateway (STS hop) | Gateway workload SPIFFE | `gateway` | *(absent)* | preserved; gateway not an agent |

**Sentinel namespace:** URIs under `sentinel:` prefix (not valid SPIFFE) to avoid collision with real trust domains:

- `sentinel:human`
- `sentinel:batch`
- `sentinel:api-key`

STS maintains an allow-list of `(auth_method, wkl_prefix)` pairs that may mint tokens without SVID proof. All other `(auth_method, wkl)` combinations require valid attestation (Block 1.2).

Additional claim: `auth_method` (string, enum) — set only by auth-callout or STS; stripped on ingress like other identity headers.

### D3 — Relationship between `wkl` and `jwt.sub`

| Option | Description |
|---|---|
| **Same claim** | Overload `sub` with SPIFFE ID. Breaks existing SpiceDB `user:{sub}` patterns and human-readable audit. |
| **Two claims; `sub` = logical actor** | `sub` names the current hop's policy principal; `wkl` names the attested runtime. **Recommended.** |
| **`sub == wkl` for workloads only** | Inconsistent rules per caller class; complicates CEL and act_chain. |

**Recommendation: two claims.**

Convention for `jwt.sub` on mesh tokens:

| Hop type | `jwt.sub` value |
|---|---|
| Human direct | Same as `originator_sub` (external IdP subject) |
| Agent hop | `agent:{tenant}/{name}` (namespaced, stable in SpiceDB) |
| Gateway egress hop | `gateway:{instance_id}` or gateway SPIFFE-derived principal |

`caller_id` (NATS ACL) remains a separate, derived, token-safe segment — unchanged from auth-callout. Do **not** conflate `caller_id`, `sub`, and `wkl`.

---

## Recommendation

Adopt the **triplet model** with **two-claim separation** (`sub` = logical current actor, `wkl` = attested workload) and **optional `agent_id`** except when an attested agent workload presents a registry-backed agent identity.

### Summary table (mesh token, enforce mode)

| Claim | Required | Set by |
|---|---|---|
| `sub` | yes | STS / auth-callout |
| `wkl` | yes | STS / auth-callout (attested or sentinel) |
| `wkl_attested_at` | yes when `wkl` is SPIFFE | STS |
| `auth_method` | yes | STS / auth-callout |
| `agent_id` | yes iff `wkl` is SPIFFE **and** caller is an agent workload | STS after registry check |
| `agent_version` | yes when `agent_id` present | STS from registry |
| `originator_sub` | yes for interactive chains; optional for pure system jobs | First hop; preserved |
| `tenant` | yes | unchanged |
| `roles` | optional | unchanged |

### Rationale

1. **Separation of concerns** — Policy ("what agent may call tool X"), attestation ("which pod proved it"), and delegation ("for which user") are independently auditable.
2. **Backward-compatible bootstrap path** — Phase 0 tokens gain `wkl` + `auth_method` in shadow mode without requiring registry or SPIRE on day one.
3. **Registry gate** — STS refuses `agent_id=X` unless `wkl ∈ allowed_workloads(X)` (Block 1.1 acceptance criterion).
4. **Uber alignment** — Matches workload attestation + agent registry + originator chain without forcing humans into the registry.

Phasing (Block 6): `MCP_GATEWAY_AGENT_IDENTITY=off|shadow|enforce` — shadow validates and logs; enforce rejects missing `wkl`, invalid sentinels, and SPIFFE tokens without matching `agent_id`.

---

## Consequences

### CEL namespace

Extend pinned `jwt.*` roots (`MCP_GATEWAY_PLAN.md` §8):

| Variable | Type | Notes |
|---|---|---|
| `jwt.wkl` | string | SPIFFE URI or `sentinel:*` |
| `jwt.wkl_attested_at` | timestamp | absent for sentinels |
| `jwt.agent_id` | string | empty/absent for non-agent hops |
| `jwt.agent_version` | string | |
| `jwt.originator_sub` | string | chain root |
| `jwt.auth_method` | string | enum |
| `jwt.act_chain` | list\<map\> | Block 2.2; helpers `chain.depth()`, `chain.originator()` |
| `jwt.purpose` | string | Block 2.3 |

Example rules:

```cel
// Deny if attested agent workload lacks registry id
jwt.wkl.startsWith("spiffe://") && !has(jwt.agent_id) -> deny

// Human direct access — no agent impersonation
jwt.auth_method == "oidc" && jwt.wkl == "sentinel:human" && !has(jwt.agent_id)
  && mcp.tool.name in jwt.roles
```

Ingress hardening: strip/refuse client-supplied `agent_id`, `wkl`, `originator_sub`, `auth_method`, `act_chain` (extends existing `mcp-caller-sub` rule).

Rate limits (Block 5): add scopes keyed by `(jwt.agent_id, jwt.purpose, jwt.tenant)` alongside `jwt.sub`.

### Audit envelope

Evolve `caller` object from flat `{ sub, via }` to layered identity:

```json
{
  "caller": {
    "sub": "agent:acme/oncall",
    "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
    "agent_id": "acme/oncall",
    "agent_version": "1.4.2",
    "originator_sub": "oidc|acme|alice",
    "auth_method": "svid",
    "via": "sts"
  },
  "act_chain": [ "..." ],
  "session_id": "..."
}
```

Index fields for Block 4 observability: `originator_sub`, `agent_id`, `chain_root`, `session_id`, `trace_id`. Agent-centric queries ("what did agent X do for user Y") use `agent_id` + `originator_sub`, not `caller.sub` alone.

Wire headers (additive): `mcp-wkl`, `mcp-agent-id`, `mcp-originator-sub`, `mcp-act-chain` — all gateway-set, ingress-stripped.

### Registry write path

Identity layering constrains **who may register agents** and **what `allowed_workloads` contains**:

| Writer | Allowed operations |
|---|---|
| Control plane (signed manifest) | create/update/deprecate/revoke agent records |
| Runtime agents | **no** self-registration |
| STS | read-only lookup; enforces `wkl ∈ allowed_workloads` at exchange time |

Registry mutation audit (`mcp.audit.registry.*`) must record: manifest signer, `agent_id`, `allowed_workloads` delta, and correlation to deployment SPIFFE IDs.

Agent manifests declare SPIFFE ID patterns (exact or prefix), not NATS `caller_id`. Mapping SPIFFE → NATS ACL remains auth-callout/STS responsibility.

Bootstrap callers without registry entries still operate in shadow mode; enforce mode for agent workloads requires prior registry registration (Block 6 backfill plan).

---

## Open questions

1. **Gateway as actor** — Should the gateway have an `agent_id`, or only `sub=gateway:…` + workload SPIFFE? (Affects `act_chain` last hop before backend.)
2. **Batch originator** — Is `system:batch` sufficient, or should each job carry a stable `job_id` as synthetic `originator_sub`?
3. **`sub` for agent hops** — Is `agent:{tenant}/{name}` the long-term SpiceDB subject, or should SpiceDB use `agent_id` directly with a new relation type?
4. **Sentinel evolution** — Should sentinels migrate to real SPIFFE IDs under a `trogon.local/sentinel/…` trust domain for uniform parsing?
5. **Bootstrap token claims** — Does auth-callout mint `wkl`/`auth_method` at connect time, or only STS after first exchange? (Coupled to ADR 0003.)
6. **Cross-tenant agents** — Can one `agent_id` list workloads in multiple tenants, or is `agent_id` always tenant-scoped via ADR 0001?
7. **Historical validity** — Must registry lookups at receipt time reflect current state, or state at each chain entry's `iat`?
8. **OAuth-MCP composition** — How does resource-owner `sub` map to `originator_sub` when the MCP client is a user agent vs. autonomous agent?

---

## References

- `MCP_GATEWAY_PLAN.md` — § Audit, § Wire-Format Pins, § CEL variable namespace
- `docs/a2a/explanation/auth-callout-design.md` — bootstrap JWT claim layout
- [Uber: Solving the Agent Identity Crisis](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/)

## Related ADRs

| ADR | Topic |
|---|---|
| 0001 | Tenancy model (namespacing for `agent_id`, audit retention) |
| 0003 | Bootstrap vs. mesh tokens |
| 0004 | STS form factor |
| 0005 | TTL and `aud` discipline |
| 0006 | Mesh token signing keys |

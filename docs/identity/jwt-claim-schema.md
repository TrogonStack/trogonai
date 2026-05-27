# JWT claim schema

## Status

**DRAFT** — for human review. Several claims depend on ADRs `0001`–`0006` (not yet finalized). Claims marked **ADR-pending** must not be treated as wire pins until the referenced ADR merges.

Related docs: [`act-chain.md`](act-chain.md) (Block 2.2), [`MCP_GATEWAY_PLAN.md`](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pins, [`PENDING_TODO.md`](../../PENDING_TODO.md) Block 1.3.

## Scope

Standardize the JWT claim set used across the Trogon mesh:

1. **Existing claims** validated or projected today by `trogon-mcp-gateway` and minted by `a2a-auth-callout` (bootstrap NATS User JWT).
2. **New claims** required for agent identity, workload attestation, delegation lineage, and intent-aware policy (Blocks 1.3 and 2.x).

This document defines claim names, types, validation, CEL exposure, ingress hardening, and rollout behavior. It does **not** define STS exchange, registry entities, or `act_chain` entry internals (see linked docs).

### Token kinds

| Kind | Minted by | Lifetime | Typical `aud` | Role |
|---|---|---|---|---|
| **Bootstrap NATS User JWT** | `a2a-auth-callout` at CONNECT | Connect-scoped (hours) | NATS account name | Subject ACL + caller attribution |
| **Mesh token** | STS (future) | 60–300 s (**ADR 0005**) | Target hop (gateway/backend) | Per-hop identity + narrowed scope |
| **Gateway-validated bearer** | Either kind above (or reminted egress token) | As minted | `trogon-mcp-gateway` (ingress default) | MCP gateway policy input |

Until Block 2 ships, treat the auth-callout JWT as the **bootstrap credential** (`PENDING_TODO.md` verified snapshot). Mesh tokens are additive; bootstrap claims remain for NATS transport ACLs.

---

## Existing claims

Claims below are **in production or pinned in plan** as of the `yordis/agentgateway` branch snapshot (`PENDING_TODO.md`, 2026-05-26).

### Registered (RFC 7519)

| Claim | Type | Required | Validation | Source | Example |
|---|---|---|---|---|---|
| `iss` | string | **Yes** (when JWT ingress active) | Must match `MCP_GATEWAY_JWT_ISSUERS` allowlist | Minting service (callout / STS) | `"https://id.trogon.ai/callout"` |
| `aud` | string or array | **Yes** (when JWT ingress active) | Must include configured gateway audience (default `trogon-mcp-gateway`) | Minting service | `"trogon-mcp-gateway"` |
| `exp` | number (Unix s) | **Yes** | `exp > now − leeway`; reject expired | Minting service | `1748347200` |
| `iat` | number (Unix s) | **Yes** (mesh); recommended (bootstrap) | Must not be far in the future (> leeway) | Minting service | `1748343600` |
| `sub` | string | **Yes** | Non-empty; stable principal id for SpiceDB / audit | IdP / callout mapping | `"user:alice"` |
| `nbf` | number (Unix s) | Optional | Reject if `nbf > now + leeway` | Minting service | `1748343600` |
| `jti` | string | Optional | Opaque; useful for replay / audit correlation | Minting service | `"01JXYZ…"` |

Gateway implementation today (`trogon-mcp-gateway`): validates `iss`, `aud`, `exp`, requires `sub`, extracts `jti` and tenant (below). `iat`/`nbf` follow library defaults when present.

### Tenancy and authorization (Trogon / plan-pinned)

| Claim | Type | Required | Validation | Source | Example |
|---|---|---|---|---|---|
| `tenant` | string | Optional (**ADR 0001**) | Non-empty when present; authoritative for soft tenancy | Auth callout / STS from IdP | `"acme"` |
| `https://trogon.ai/tenant` | string | Optional (alias) | Same as `tenant`; gateway accepts via `MCP_GATEWAY_JWT_TENANT_CLAIM` | Auth callout / STS | `"acme"` |
| `roles` | array of strings | Optional | Each element non-empty string; no duplicates enforced at gateway | IdP / callout `data` projection | `["engineer","mcp-user"]` |

**ADR 0001 (tenancy model):** whether `tenant` is required, namespaced, or redundant under hard NATS account tenancy.

CEL namespace (`MCP_GATEWAY_PLAN.md:941`) already documents `jwt.tenant` and `jwt.roles`; gateway code extracts tenant but does not yet surface `roles` to CEL.

### Bootstrap NATS User JWT extensions (auth-callout)

Present on connect-time User JWTs, not all consumed by MCP gateway ingress.

| Claim | Type | Required | Validation | Source | Example |
|---|---|---|---|---|---|
| `caller_id` | string | **Yes** (bootstrap) | Subject-safe token: non-empty, no `.` | Derived from external `sub` + tenant account | `"a1b2c3d4e5f6…"` |
| `data` | object | **Yes** (bootstrap) | JSON object; must include `spicedb_subject` for A2A paths | Auth callout from IdP claims | `{"spicedb_subject":"user/alice"}` |
| `nats` | object | **Yes** (bootstrap) | NATS JWT v2 user permissions block | Auth callout policy template | `{"pub":{"allow":[…]},"sub":{"allow":[…]}}` |
| `kid` | string | **Yes** (bootstrap) | Signing key id matching issuer JWKS / NKey | Auth callout | `"UABC…"` |

These claims support NATS subject ACLs (`a2a.gateway.>`, `_INBOX.{caller_id}.>`) and A2A caller attribution (`A2a-Caller-Jwt` header). They are **not** forgeable mesh-identity claims; clients must never supply them on MCP gateway ingress.

### Session (header today)

| Claim | Type | Required | Validation | Source | Example |
|---|---|---|---|---|---|
| *(header)* `mcp-session-id` | string | Per MCP session | Opaque; gateway-issued at `initialize` | Gateway session KV | `"sess_01J…"` |

**Proposed lift:** `session_id` claim (see New claims). Until then, CEL exposes `request.session_id` from the header (`MCP_GATEWAY_PLAN.md:943`).

---

## New claims (proposed)

### Agent and workload identity

| Claim | Type | Required | Validation | Source | Example | ADR |
|---|---|---|---|---|---|---|
| `agent_id` | string | Optional for non-agent callers; **required for agent-originated mesh tokens in enforce mode** | Non-empty when present; must exist in agent registry and match `wkl ∈ allowed_workloads` at STS | STS after registry lookup | `"acme/oncall-agent"` | **0002**, **0003** |
| `agent_version` | string | Required when `agent_id` set | Semver or opaque registry version string | Agent registry / STS | `"2.4.1"` | **0002** |
| `wkl` | string (SPIFFE ID) | **Required** in mesh enforce mode | URI matching `spiffe://<trust-domain>/…`; or documented sentinel (e.g. `"human"`) for non-SPIFFE auth | STS from validated SVID (**Block 1.2**) | `"spiffe://acme.local/ns/prod/sa/oncall-agent"` | **0002**, **0001** |
| `wkl_attested_at` | number (Unix s) **or** string (RFC 3339) | Required when `wkl` is a SPIFFE ID (not sentinel) | Attestation time ≤ `iat`; skew ≤ configured leeway | STS from SVID proof timestamp | `1748343600` or `"2026-05-27T06:00:00Z"` | **0002** |

**ADR 0002 (identity layers):** mandatory `agent_id` on every mesh JWT vs. agent-originated only; representation of human/batch callers.

**ADR 0003 (bootstrap vs. mesh):** whether bootstrap tokens may carry `agent_id`/`wkl` or only mesh tokens after STS exchange.

### Delegation lineage

| Claim | Type | Required | Validation | Source | Example | ADR |
|---|---|---|---|---|---|---|
| `act_chain` | array of objects | Required on mesh tokens after first delegation hop; optional on bootstrap | Max depth (default 8); loop detection; entry shape per [`act-chain.md`](act-chain.md) | STS appends each hop; gateway verifies | See act-chain doc | **0002**, **0003** |

Each entry (summary — full schema deferred):

```json
{ "sub": "user:alice", "agent_id": "acme/oncall-agent", "wkl": "spiffe://…", "iat": 1748343600 }
```

Order: oldest originator first, current actor last (`PENDING_TODO.md` Block 2.2).

### Intent

| Claim | Type | Required | Validation | Source | Example | ADR |
|---|---|---|---|---|---|---|
| `purpose` | string | Optional (enforce TBD) | Non-empty; enumerated vs. free-form **pending Block 2.3** | Originating client; preserved or refined at STS | `"incident.triage"` | **0002** |
| `intent` | string | **Deprecated alias** | Same as `purpose` if both present → reject or prefer `purpose` (TBD Block 2.3) | Legacy clients only | `"incident.triage"` | Block 2.3 |

### Session and audience discipline

| Claim | Type | Required | Validation | Source | Example | ADR |
|---|---|---|---|---|---|---|
| `session_id` | string | Recommended | Must equal gateway-issued session when present on MCP ingress | Gateway at `initialize` or STS copy from bootstrap | `"sess_01J…"` | — |
| `scope` | string or array | Optional | Tool-name list or coarse capability tokens; narrowed each STS hop | STS | `["tools:read","github::create_issue"]` | **0005** |

**ADR 0005 (TTL and `aud` discipline):** mesh tokens must carry hop-specific `aud`; gateway enforces `aud == this_gateway` in enforce mode.

---

## CEL variable namespace

Pinned roots per `MCP_GATEWAY_PLAN.md:934–941`. Proposed extensions (additive; no root renames):

| CEL variable | Type | Source claim / context | Notes |
|---|---|---|---|
| `jwt.sub` | string | `sub` | Existing |
| `jwt.tenant` | string | `tenant` or namespaced alias | Existing |
| `jwt.roles` | list\<string\> | `roles` | Existing in plan; wire extraction TBD |
| `jwt.iss` | string | `iss` | Existing |
| `jwt.aud` | string | `aud` (first element if array) | Existing |
| `jwt.agent_id` | string | `agent_id` | **New**; empty string if absent |
| `jwt.agent_version` | string | `agent_version` | **New** |
| `jwt.wkl` | string | `wkl` | **New** |
| `jwt.wkl_attested_at` | timestamp | `wkl_attested_at` | Coerce RFC 3339 or Unix s |
| `jwt.act_chain` | list\<map\> | `act_chain` | **New**; helpers TBD: `chain.depth()`, `chain.originator()` |
| `jwt.purpose` | string | `purpose` | **New** |
| `jwt.session_id` | string | `session_id` or fallback `request.session_id` | **New** |
| `jwt.scope` | list\<string\> | `scope` | **New** when **ADR 0005** pins shape |
| `jwt.<custom>` | dynamic | any other validated claim | Existing escape hatch |
| `has(jwt.<claim>)` | bool | — | Existing |

Custom claims remain accessible via `jwt.<name>` without a plan bump. Renaming or moving pinned fields requires a CEL namespace version bump.

---

## Ingress hardening

**Rule (existing):** On every message entering the gateway from the edge zone, drop client-supplied identity headers and replace with JWT-derived values (`MCP_GATEWAY_PLAN.md:835`).

### Headers — strip / overwrite on ingress

| Header | Action | Authoritative source |
|---|---|---|
| `mcp-caller-sub` | Strip; set from JWT `sub` | Gateway |
| `mcp-tenant` | Strip; set from JWT `tenant` | Gateway |
| `mcp-instance-id` | Strip; set to gateway instance | Gateway |
| `mcp-schema` | Strip; set to pinned schema version | Gateway |
| `trogon-mcp-tenant` | Strip on egress to backend when JWT ingress active | Gateway (legacy) |
| `trogon-mcp-verified-sub` | Set on egress only | Gateway |
| `trogon-mcp-verified-tenant` | Set on egress only | Gateway |
| **`mcp-act-chain`** *(proposed)* | Strip on ingress; set from JWT `act_chain` on egress | Gateway |
| **`mcp-agent-id`** *(proposed)* | Strip on ingress; set from JWT `agent_id` on egress | Gateway |
| **`mcp-wkl`** *(proposed)* | Strip on ingress; set from JWT `wkl` on egress | Gateway |
| **`mcp-purpose`** *(proposed)* | Strip on ingress; set from JWT `purpose` on egress | Gateway |

Backends and audit may log projected headers for legibility; they must **not** treat them as authority.

### JWT claims — refuse if client-supplied on ingress

When JWT ingress is `validate` or `require`, the gateway validates the **bearer token only**. Clients must not attempt to inject parallel identity via headers. Additionally, for mesh hardening:

| Claim | Edge ingress policy |
|---|---|
| `act_chain`, `agent_id`, `agent_version`, `wkl`, `wkl_attested_at`, `purpose`, `scope` | Must be **issuer-signed** in the bearer JWT. If verification fails or claims appear only in unsigned headers/query → reject in enforce mode. |
| `tenant`, `sub`, `roles` | Same as above; legacy tenant header ignored when JWT succeeds. |
| `caller_id`, `data`, `nats` | Bootstrap-only; **reject** on MCP gateway bearer if present without trusted bootstrap issuer (**ADR 0003**). |

STS and auth-callout are the only approved minters for forgeable identity claims.

---

## Compatibility: shadow vs. enforce

Controlled by `MCP_GATEWAY_AGENT_IDENTITY` (`PENDING_TODO.md` Block 6), mirroring `MCP_GATEWAY_JWT_MODE` (`off` / `validate` / `require`).

| Mode | Missing optional new claim | Invalid / unsigned new claim | Missing required new claim (per enforce table) |
|---|---|---|---|
| **off** | Ignored | Ignored | Ignored |
| **shadow** | Allow; log `identity.shadow.missing_<claim>` | Allow; log `identity.shadow.violation` with detail | Allow; log violation |
| **enforce** | Allow if claim marked optional for this token kind | **Reject** (`-32101` invalid_token class) | **Reject** |

Suggested enforce requirements (pending ADR finalization):

| Claim | Bootstrap JWT | Mesh token at gateway |
|---|---|---|
| `wkl` | Optional (shadow logs) | **Required** |
| `agent_id` | Optional | Required when caller is an agent |
| `act_chain` | Optional | Required when `agent_id` present |
| `aud` | Gateway audience | Must match receiving hop |
| `purpose` | Optional until Block 2.3 | Optional unless registry mandates |

During shadow rollout, existing connect-time JWTs remain authoritative for NATS subject ACL; new claims inform CEL and audit only (`PENDING_TODO.md` Block 6).

---

## Example tokens

### Bootstrap NATS User JWT (truncated payload)

```json
{
  "iss": "ABCD…",
  "sub": "user:alice",
  "aud": "tenant-acme",
  "iat": 1748343600,
  "exp": 1748430000,
  "caller_id": "8f3a2b1c0d9e8f7a",
  "data": { "spicedb_subject": "user/alice" },
  "nats": { "pub": { "allow": ["a2a.gateway.>"] }, "sub": { "allow": ["_INBOX.8f3a….>", "a2a.push.8f3a….>"] } }
}
```

### Mesh token after STS exchange (illustrative)

```json
{
  "iss": "https://sts.trogon.ai/acme",
  "sub": "user:alice",
  "aud": "trogon-mcp-gateway",
  "iat": 1748347200,
  "exp": 1748347500,
  "tenant": "acme",
  "agent_id": "acme/oncall-agent",
  "agent_version": "2.4.1",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
  "wkl_attested_at": "2026-05-27T06:00:00Z",
  "purpose": "incident.triage",
  "session_id": "sess_01JABC",
  "act_chain": [
    { "sub": "user:alice", "agent_id": "", "wkl": "human", "iat": 1748343600 },
    { "sub": "user:alice", "agent_id": "acme/oncall-agent", "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent", "iat": 1748347200 }
  ]
}
```

---

## Open questions

1. **`purpose` vs. `intent`:** single canonical claim name and refinement rules (Block 2.3).
2. **`wkl_attested_at` wire form:** standardize Unix s vs. RFC 3339 for all minters.
3. **Bootstrap carry-over:** which new claims may appear on connect JWT vs. STS-only (**ADR 0003**).
4. **`agent_id` on gateway process:** does the gateway itself carry an `agent_id`, or only `wkl` + privileged role? (`PENDING_TODO.md` open questions).
5. **Batch / non-human originators:** sentinel `wkl` values and `act_chain` root shape.
6. **`roles` extraction:** promote from plan-only CEL to gateway-validated claim, or keep in `data` for bootstrap only.
7. **Per-entry `act_chain` signatures** vs. outer-JWT integrity only (affects size and STS cost).
8. **Signing keys and JWKS** for mesh tokens (**ADR 0006**): publication subject/URL for gateway validation of STS-issued JWTs.
9. **OAuth-MCP composition:** which claims OAuth access tokens map to before callout minting (`MCP_GATEWAY_PLAN.md:67`).

---

## Acceptance criteria (Block 1.3)

- [ ] Human reviewers agree on claim names, types, and enforce vs. optional matrix.
- [ ] ADR dependencies (`0001`–`0006`) are explicit before wire-format pin in `MCP_GATEWAY_PLAN.md`.
- [ ] CEL and ingress-hardening tables in the plan can be updated from this doc without ambiguity.
- [ ] Shadow mode can emit structured violations for every new claim without rejecting traffic.

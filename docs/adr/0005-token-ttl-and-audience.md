# ADR 0005 — Mesh Token TTL and Audience Discipline

## Status

**Accepted (2026-05-27).** Builds on [ADR 0003](0003-bootstrap-vs-mesh-tokens.md) and [ADR 0004](0004-sts-form-factor.md). Drives Block 2.1 (STS exchange) and Block 2.4 (gateway egress minting).

## Context

Uber's agent-identity model treats long-lived, broad-scope credentials as a bootstrap artifact only. Every hop in a delegation chain receives a **fresh, short-lived JWT** whose `aud` names exactly one downstream recipient and whose `scope`/`sub` permissions are no wider than the hop requires. TrogonStack's current path validates connect-time NATS User JWTs at the gateway perimeter but **propagates** verified context headers to backends rather than minting a downstream token.

This ADR pins the numeric and semantic contracts for mesh tokens so STS, gateway egress, CEL policy, and audit envelopes can be implemented without coordinated migration. Wire-format surfaces affected: `jwt.aud` in the CEL namespace (`MCP_GATEWAY_PLAN.md` § Wire-Format Pins §8), future egress header `Authorization: Bearer <mesh-token>`, and audit fields for token `exp` / `aud` mismatch.

References:

- Uber: [Solving the Agent Identity Crisis](https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/)

## Decision Points

### 1. Mesh-token TTL

**Recommendation: default 120 s; allowed range 60–300 s per agent or bundle policy.**

| Tier | TTL | Rationale |
|---|---|---|
| Default (all hops) | **120 s** | Balances replay window (~2 min) against STS QPS; covers P99 gateway→backend round-trip with margin for one cache miss + exchange. |
| High-sensitivity agents | **60 s** | Registry flag `mesh_token_ttl_s: 60` for agents touching PII, financial, or admin tools. |
| Low-risk internal hops | **300 s** | Registry opt-in only; reduces STS load for stable agent→agent chains with infrequent tool calls. |

STS sets `exp = iat + ttl`. The inbound bootstrap token's remaining lifetime does **not** extend the mesh token — each exchange gets a fresh clock.

### 2. Clock-skew tolerance

**Recommendation: ±30 s on `iat` and `exp` validation.**

Receivers (gateway, backend MCP server, downstream agent) accept tokens where `now - 30s ≤ iat ≤ now + 30s` and `exp + 30s ≥ now`. This matches common JWT practice and NATS cluster clock drift without materially widening the replay window (effective max skew adds 60 s to worst-case reuse, bounded by the 60 s minimum TTL).

### 3. Refresh strategy

**Recommendation: on-demand exchange at hop time, with proactive refresh inside the cache layer.**

1. **On miss or expiry:** gateway (or A2A SDK client) calls STS with `subject_token`, requested `audience`, optional `scope`, and workload proof before egress.
2. **Proactive refresh:** when a cached entry's remaining lifetime drops below **TTL / 4** (30 s at default 120 s TTL), a background refresh replaces it before the next request pays exchange latency. Refresh is best-effort; the hot path still blocks on exchange if refresh failed.
3. **No long-lived refresh tokens** in the mesh — each hop re-exchanges from its current mesh token or bootstrap token.

**Cache key shape** (gateway egress, per instance):

```
mesh_cache:{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}
```

- `target_aud` — full audience string (see §4).
- `session_id` — from `mcp-session-id` header / session KV; prevents cross-session token reuse.
- `scope_fingerprint` — SHA-256 of sorted scope list, or `*` when absent.

**Cache entry TTL:** `min(floor(exp - now - skew_leeway), configured_cap)` where `configured_cap = mesh_token_ttl / 2` (60 s at default). Never serve a cached token past `exp - 30s`.

Rejected alternative: **proactive-only refresh** on a timer without hop-time validation — simpler but allows stale `aud` if routing changes mid-session.

### 4. `aud` granularity

**Recommendation: per-backend service identity (one `aud` per logical MCP backend / agent service), not per-tool or per-subject-prefix.**

Audience strings are stable URIs namespaced by tenant and deployment:

```
urn:trogon:mcp:backend:{tenant}:{server_id}
urn:trogon:a2a:agent:{tenant}:{agent_id}
urn:trogon:mcp:gateway:{tenant}:{gateway_instance_id}   # for ingress to a gateway hop
urn:trogon:mcp:client:{tenant}:{client_id}              # callback direction
```

Examples:

| Hop | Inbound `aud` (must match receiver) | Notes |
|---|---|---|
| Agent A → MCP gateway | `urn:trogon:mcp:gateway:acme:gw-prod-1` | Gateway validates before policy eval. |
| Gateway → `github` MCP server | `urn:trogon:mcp:backend:acme:github` | Minted at egress; inbound client token dropped. |
| Agent A → Agent B | `urn:trogon:a2a:agent:acme:oncall-agent` | STS narrows scope; B verifies `aud` is self. |
| Server → client callback | `urn:trogon:mcp:client:acme:client-1` | Same exchange path as Block 2.4 callback note. |

**Tool-level restriction** belongs in `scope` (OAuth-style space-delimited list, e.g. `tool:github::create_issue tool:github::search_issues`) or in CEL/SpiceDB — not in `aud`. **Subject-prefix audiences** (`mcp.server.github.>`) were rejected: they conflate NATS routing with JWT semantics, complicate multi-gateway federation, and encourage over-broad tokens when one backend serves many subjects.

Registry field `allowed_audiences` lists permitted target URIs per agent; STS rejects exchanges to audiences not on the list.

### 5. Hard vs. soft enforcement

**Recommendation: mode-gated, aligned with `MCP_GATEWAY_AGENT_IDENTITY` (`off` / `shadow` / `enforce`).**

| Mode | `aud` mismatch behavior | CEL |
|---|---|---|
| `off` | Not evaluated | — |
| `shadow` | **Log + audit** `aud_mismatch`; request proceeds on bootstrap path | `jwt.aud != expected_aud` → `audit.emit({ violation: "aud_mismatch", ... })` |
| `enforce` | **Hard reject** before policy eval | `jwt.aud != expected_aud` → deny; JSON-RPC `-32106` (`auth_expired` family) or new `-32109` `audience_mismatch` |

Same rule at gateway ingress (`aud == self`) and backend ingress (backend verifies `aud == urn:trogon:mcp:backend:{tenant}:{my_server_id}`). Shadow mode must emit to `mcp.audit.sts.*` / gateway audit with expected vs. actual `aud` for cutover metrics (Block 6).

## Recommendation Summary

| Topic | Decision |
|---|---|
| Default mesh TTL | **120 s** (range **60–300 s** via registry) |
| Clock skew | **±30 s** on `iat` / `exp` |
| Refresh | On-demand at hop; proactive refresh at **TTL/4** remaining in cache |
| Cache key | `{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}` |
| Cache max age | **TTL / 2**, never past `exp - 30s` |
| `aud` granularity | **Per-backend / per-agent service URI**; tools in `scope` |
| Enforcement | **Hard reject** in `enforce`; **log only** in `shadow` |

## Consequences

### STS load

TTL and cache directly determine exchange QPS:

```
exchange_qps ≈ active_sessions × (1 / effective_cache_ttl) × egress_hops_per_request
```

At default 120 s TTL and 60 s cache cap, a warm cache yields at most **~1 exchange per session per minute per unique (aud, scope)**. Shortening TTL to 60 s **doubles** STS load unless cache hit rate stays high. Mitigations: in-memory cache per gateway instance, registry/ trust-bundle caching inside STS (P99 < 40 ms target), per-`wkl` and per-`agent_id` rate limits, circuit breaker when STS or registry is degraded (**fail-closed** — mesh stops; no degraded bypass).

### Cache sizing

Per gateway instance, budget **~256 bytes × entry count**. Rule of thumb: `entries ≈ active_sessions × avg_distinct_backends_per_session × avg_scope_variants`. Default cap **10 000 entries** LRU-evicted; tunable via `MCP_GATEWAY_MESH_CACHE_MAX_ENTRIES`. Multi-instance clusters do not share the cache — each instance may exchange once per key; acceptable given short TTL and low marginal STS cost at P99 < 40 ms.

### Abuse mitigation

- Short TTL limits replay of stolen mesh tokens to the TTL window (60–120 s typical).
- Narrow `aud` prevents token relay to unintended backends even if NATS subject ACLs are misconfigured.
- Session-bound cache keys prevent token sharing across MCP sessions.
- STS rate limits: **100 exchanges / 10 s / `wkl`**, **500 / 10 s / `agent_id`** (defaults; override in bundle).
- Audit every exchange (success and failure) to `mcp.audit.sts.{outcome}` with minted claims (not signature).

### Downstream spec work

- Add `jwt.aud` enforcement CEL snippet to default bundle.
- Extend audit envelope with `mesh_token: { aud, exp, iss, act_chain_depth }`.
- Document `-32109 audience_mismatch` in Wire-Format Pins §6 (or reuse `-32106` with `data.reason`).
- STS exchange API: `audience` parameter accepts these URIs only.

## Open Questions

1. **Single global default vs. mandatory registry TTL** — should agents without `mesh_token_ttl_s` inherit 120 s, or must registration set an explicit value?
2. **Gateway instance in `aud`** — is `urn:trogon:mcp:gateway:{tenant}:{instance_id}` too fine-grained for clients behind load balancers, or should ingress `aud` be `{tenant}` only with instance in a separate claim?
3. **Scope canonicalization** — exact string format for tool scopes (`tool:{server}::{name}` vs. SpiceDB resource id) before `scope_fingerprint` hashing.
4. **Callback exchange symmetry** — confirm server→client callbacks always re-exchange with `aud=client_id` even when the inbound mesh token has minutes remaining.
5. **Cross-region STS** — does cache key need a `region` segment when STS is regional (see Block 0 open questions on trust-bundle replication)?
6. **OAuth-MCP composition** — does the OAuth access token become the bootstrap credential with the same TTL/`aud` rules, or a parallel profile? **Resolved: deferred until first OAuth-fronted MCP client. Semantics pinned (OAuth access token enters STS as `subject_token`); multi-issuer config is a same-day add when the first caller arrives, not standing work.**
7. **Error code** — dedicate `-32109 audience_mismatch` vs. overload `-32106 auth_expired` with structured `data.reason`.

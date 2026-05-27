# STS exchange wire contract

**Status:** Operator reference (2026-05-27). Implements [ADR 0004](../adr/0004-sts-form-factor.md) (NATS transport) refined by [ADR 0005](../adr/0005-token-ttl-and-audience.md) and [ADR 0006](../adr/0006-mesh-token-signing-keys.md).

Related: [overview.md](overview.md), [jwt-claim-schema.md](jwt-claim-schema.md), [act-chain.md](act-chain.md), [registry.md](registry.md).

---

## Transport

| Field | Value |
|---|---|
| **Subject** | `mcp.sts.exchange` |
| **Pattern** | NATS request/reply |
| **Consumer** | Queue group `trogon-sts` (horizontal scale) |
| **Reply** | Caller-provided `_INBOX.*` |
| **Payload** | JSON (UTF-8) |

Callers MUST hold NATS ACL: publish `mcp.sts.exchange`, subscribe `_INBOX.>`.

An HTTP facade (`POST /oauth/token`, RFC 8693) is a **future option** ([ADR 0004](../adr/0004-sts-form-factor.md)); the v1 contract is **NATS-only**. Request/response JSON bodies are transport-agnostic so an HTTP adapter can reuse them unchanged.

---

## Request schema

RFC 8693–inspired token exchange. All fields are JSON object properties on the NATS request body.

| Field | Type | Required | Description |
|---|---|---|---|
| `subject_token` | string | **Yes** | Bearer JWT being exchanged (bootstrap NATS User JWT or upstream mesh JWT) |
| `subject_token_type` | string | **Yes** | `urn:ietf:params:oauth:token-type:jwt` |
| `actor_token` | string | **Yes** | Caller's workload proof: SPIFFE JWT-SVID, PEM-wrapped SVID, or documented sentinel token for allow-listed non-SPIFFE callers |
| `audience` | string | **Yes** | Target hop URI (see [Audience URIs](#audience-uris)) |
| `scope` | string | No | Space-delimited capability list (e.g. `tool:github::create_issue`); narrowed each hop |
| `purpose` | string | No | Intent label; must ∈ `allowed_purposes` when registry enumerates |
| `requested_token_type` | string | No | Default `urn:ietf:params:oauth:token-type:jwt` |

### Bootstrap-as-subject example

First mesh token after connect. `subject_token` is the auth-callout User JWT; `actor_token` is the caller's SVID or sentinel proof.

```json
{
  "subject_token": "eyJhbGciOiJSUzI1NiIs…",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "actor_token": "eyJhbGciOiES256…",
  "audience": "urn:trogon:a2a:agent:acme:triage-router",
  "scope": "tool:github::search_issues",
  "purpose": "incident.triage",
  "requested_token_type": "urn:ietf:params:oauth:token-type:jwt"
}
```

### Mesh-as-subject example

Agent A calls Agent B. `subject_token` is A's current mesh JWT (already carries `act_chain`).

```json
{
  "subject_token": "eyJhbGciOiRSUzI1NiIs…",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "actor_token": "eyJhbGciOiES256…",
  "audience": "urn:trogon:a2a:agent:acme:oncall-agent",
  "scope": "tool:pagerduty::page",
  "purpose": "incident.response",
  "requested_token_type": "urn:ietf:params:oauth:token-type:jwt"
}
```

### RFC 8693 alignment

| RFC 8693 | This contract |
|---|---|
| `grant_type=urn:ietf:params:oauth:grant-type:token-exchange` | Implicit (NATS subject identifies exchange) |
| `subject_token` / `subject_token_type` | Same semantics |
| `actor_token` / `actor_token_type` | `actor_token` required; type inferred from SVID format |
| `audience` | Same; URIs per ADR 0005 |
| `scope` | Same; tool-level narrowing |
| `requested_token_type` | Same default |

---

## Response schema (success)

HTTP-equivalent 200 body on the NATS reply.

| Field | Type | Required | Description |
|---|---|---|---|
| `access_token` | string | **Yes** | Signed mesh JWT |
| `issued_token_type` | string | **Yes** | `urn:ietf:params:oauth:token-type:jwt` |
| `expires_in` | integer | **Yes** | Seconds until `exp` (≤ mesh TTL from registry, default 120) |
| `token_type` | string | **Yes** | `Bearer` |

### Example JWT payload (decoded `access_token`)

```json
{
  "iss": "https://sts.trogon.ai/acme",
  "sub": "agent:acme/oncall-agent",
  "aud": "urn:trogon:a2a:agent:acme:oncall-agent",
  "iat": 1748347200,
  "exp": 1748347320,
  "tenant": "acme",
  "agent_id": "acme/oncall-agent",
  "agent_version": "2.4.1",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
  "wkl_attested_at": "2026-05-27T06:00:00Z",
  "auth_method": "svid",
  "originator_sub": "oidc|acme|alice",
  "purpose": "incident.response",
  "scope": "tool:pagerduty::page",
  "session_id": "sess_01JABC",
  "act_chain": [
    { "sub": "oidc|acme|alice", "wkl": "sentinel:human", "iat": 1748343600 },
    { "sub": "agent:acme/triage-router", "agent_id": "acme/triage-router", "wkl": "spiffe://acme.local/ns/prod/sa/triage-router", "iat": 1748347100 },
    { "sub": "agent:acme/oncall-agent", "agent_id": "acme/oncall-agent", "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent", "iat": 1748347200 }
  ]
}
```

Signing keys and JWKS distribution: [ADR 0006](../adr/0006-mesh-token-signing-keys.md). Clock skew ±30 s on `iat`/`exp` validation.

---

## Response schema (error)

Structured error on the NATS reply (exchange denied).

| Field | Type | Required | Description |
|---|---|---|---|
| `error` | string | **Yes** | RFC 8693 error code or extension |
| `error_description` | string | **Yes** | Human-readable detail (not contract-stable) |
| `error_uri` | string | No | Link to documentation |

### RFC 8693 error codes

| `error` | When |
|---|---|
| `invalid_request` | Malformed JSON, missing required field, unknown `subject_token_type` |
| `invalid_target` | `audience` not in agent's `allowed_audiences`, unknown backend, cross-tenant target |
| `unauthorized_client` | `actor_token` / SVID invalid, `wkl` not attested, sentinel not allow-listed |
| `invalid_grant` | `subject_token` expired, bad signature, untrusted `iss`, bootstrap used after enforce cutoff |
| `unsupported_grant_type` | Wrong exchange semantics |
| `access_denied` | Registry deny: revoked agent, `wkl ∉ allowed_workloads`, purpose not allowed, `act_chain` depth/loop |

### Mesh-specific JSON-RPC mapping

When the **gateway** surfaces STS or token-validation failures to MCP clients, use Trogon allocation `-32100` … `-32199` ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §6):

| Code | Symbol | STS / token condition |
|---|---|---|
| `-32106` | `auth_expired` | `subject_token` or mesh token past `exp` (± skew) |
| `-32109` | `audience_mismatch` | Token `aud` ≠ receiver identity |
| `-32113` | `act_chain_depth_exceeded` | Chain would exceed max depth (8) |
| `-32114` | `act_chain_loop_detected` | Duplicate `(agent_id, wkl)` in chain |
| `-32115` | `act_chain_principal_unknown` | Registry lookup failure at verification |

Gateway `data` shape includes `trace_id`; audience mismatch adds `{ expected_aud, actual_aud }`.

Example STS error reply:

```json
{
  "error": "invalid_target",
  "error_description": "audience urn:trogon:mcp:backend:acme:github not in allowed_audiences for agent acme/oncall-agent",
  "error_uri": "https://docs.trogon.ai/identity/sts-exchange#invalid_target"
}
```

Example gateway JSON-RPC error (enforce mode):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32109,
    "message": "audience_mismatch",
    "data": {
      "trace_id": "0af7651916cd43dd8448eb211c80319c",
      "expected_aud": "urn:trogon:mcp:gateway:acme:gw-prod-1",
      "actual_aud": "urn:trogon:mcp:backend:acme:github"
    }
  }
}
```

---

## Validation pipeline

STS executes these steps in order; any failure short-circuits with an error response and audit deny.

1. **Signature** — Verify `subject_token` JWS against trusted issuer set (auth-callout JWKS for bootstrap; mesh JWKS for upstream mesh tokens).
2. **SVID** — Validate `actor_token` against SPIFFE trust bundle; extract `wkl`; for allow-listed sentinels, verify `(auth_method, wkl)` pair.
3. **Registry lookup** — Resolve `agent_id` from subject token; load agent record from `mcp.registry.agent.lookup` (or hot-path cache).
4. **`aud ∈ allowed_audiences`** — Reject if requested `audience` is not listed on the agent record.
5. **`wkl ∈ allowed_workloads`** — Reject if attested workload cannot act as this `agent_id`.
6. **`purpose ∈ allowed_purposes`** — When registry enumerates purposes, reject unknown or escalated `purpose`.
7. **`act_chain` append + depth + loop detect** — Copy inbound chain; reject if `len ≥ max_depth` (default 8) or duplicate `(agent_id, wkl)`; append exchanger entry `{ sub, agent_id, wkl, iat }` matching inbound token claims.
8. **Mint** — Sign mesh JWT with STS key (`kid` in header); set `exp = iat + ttl` (registry override or default 120 s); emit success audit.

---

## Caching contract

### Gateway egress cache ([ADR 0005](../adr/0005-token-ttl-and-audience.md))

| Property | Value |
|---|---|
| **Key** | `mesh_cache:{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}` |
| **scope_fingerprint** | SHA-256 of sorted scope tokens, or `*` if absent |
| **Entry TTL** | `min(floor(exp - now - 30s), mesh_token_ttl / 2)` — default cap **60 s** |
| **Proactive refresh** | When remaining lifetime < **TTL / 4** (30 s at default), background refresh |
| **Storage** | Per gateway instance (not shared) |

Cached tokens MUST NOT be served past `exp - 30s`.

### Callback-direction exchanges

Server→client callbacks (`mcp.gateway.callback.*`) use the **same cache key shape** with `target_aud = urn:trogon:mcp:client:{tenant}:{client_id}`. Gateway re-exchanges before callback egress even when the inbound mesh token has remaining lifetime ([ADR 0005](../adr/0005-token-ttl-and-audience.md)).

---

## Rate limit defaults

| Scope | Limit | Window | On exceed |
|---|---|---|---|
| Per `wkl` | 100 exchanges | 10 s | `access_denied` + audit `rate_limited` |
| Per `agent_id` | 500 exchanges | 10 s | same |

Override via gateway/agent bundle config. Exceeded limits map to gateway `-32105` `rate_limited` when the gateway proxies the call.

---

## Audit emission

Every exchange publishes to JetStream subject `mcp.audit.sts.{outcome}` where `{outcome}` ∈ `success`, `deny`, `rate_limited`.

Envelope shape (illustrative):

```json
{
  "schema": "trogon.mcp.audit.sts/v1",
  "ts": "2026-05-27T12:00:01Z",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "outcome": "success",
  "request": {
    "audience": "urn:trogon:a2a:agent:acme:oncall-agent",
    "scope": "tool:pagerduty::page",
    "purpose": "incident.response",
    "subject_iss": "https://sts.trogon.ai/acme",
    "subject_sub": "agent:acme/triage-router",
    "wkl": "spiffe://acme.local/ns/prod/sa/triage-router"
  },
  "minted": {
    "aud": "urn:trogon:a2a:agent:acme:oncall-agent",
    "exp": 1748347320,
    "iss": "https://sts.trogon.ai/acme",
    "sub": "agent:acme/oncall-agent",
    "agent_id": "acme/oncall-agent",
    "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
    "purpose": "incident.response",
    "act_chain_depth": 3
  },
  "latency_us": 12400
}
```

**Included:** minted claims, request metadata, inbound chain depth, deny reason. **Excluded:** raw JWT signatures, private keys, full `subject_token` string (log hash or `jti` only).

---

## Failure modes

| Condition | Behavior |
|---|---|
| **STS unavailable** | **Fail-closed.** Gateway/agent receives no `access_token`; mesh traffic stops. Gateway returns structured error; clients retry with backoff. No bootstrap bypass in enforce mode ([ADR 0004](../adr/0004-sts-form-factor.md)). |
| **Registry stale / outage** | **Reject.** No stale-cache grace in v1; circuit breaker opens after configured unhealthy threshold. |
| **Signing-key rotation** | JWKS is source of truth: KV `mcp-jwks/mesh/current`, HTTPS `/.well-known/jwks.json`, `mcp.jwks.mesh.get`. Overlap window ≥ max mesh TTL + skew; verifiers accept all active `kid` values ([ADR 0006](../adr/0006-mesh-token-signing-keys.md)). |
| **Trust bundle stale** | Reject SVID validation until bundle refreshes. |
| **SpiceDB / PDP outage** | Circuit breaker; fail-closed if synchronous check required (registry ACLs preferred on hot path). |

---

## Worked examples

### 1. Bootstrap → mesh (first hop)

Human operator's client exchanges connect JWT to call Agent `triage-router`.

**Request:** `subject_token` = auth-callout JWT (`sub=oidc|acme|alice`, no mesh claims); `actor_token` = sentinel proof; `audience` = `urn:trogon:a2a:agent:acme:triage-router`; `purpose` = `incident.triage`.

**Result:** Mesh JWT with `act_chain` length 1 (originator: alice / `sentinel:human`), `aud` = target agent, TTL 120 s.

### 2. Mesh → mesh (second hop)

Agent `triage-router` delegates to Agent `oncall-agent`.

**Request:** `subject_token` = mesh JWT from example 1 (`act_chain` len 2 after router append); `actor_token` = router pod SVID; `audience` = `urn:trogon:a2a:agent:acme:oncall-agent`; `purpose` = `incident.response`.

**Result:** Mesh JWT with `act_chain` len 3, `aud` = oncall agent, `scope` narrowed to permitted tools.

### 3. Server → client callback exchange

MCP server initiates callback to client `client-1` via gateway.

**Request:** `subject_token` = mesh JWT presented at gateway (chain includes user → agents); `actor_token` = backend workload SVID; `audience` = `urn:trogon:mcp:client:acme:client-1`; `scope` = callback method scope.

**Result:** Mesh JWT with `aud` = client URI; gateway attaches on callback egress; inbound token stripped. Cache key uses `session_id` from MCP session.

---

## Audience URIs

Exact strings from [ADR 0005](../adr/0005-token-ttl-and-audience.md):

```
urn:trogon:mcp:gateway:{tenant}:{gateway_instance_id}
urn:trogon:mcp:backend:{tenant}:{server_id}
urn:trogon:a2a:agent:{tenant}:{agent_id}
urn:trogon:mcp:client:{tenant}:{client_id}
```

STS rejects any other `audience` format.

---

## References

- [ADR 0003 — Bootstrap vs mesh tokens](../adr/0003-bootstrap-vs-mesh-tokens.md)
- [ADR 0004 — STS form factor](../adr/0004-sts-form-factor.md)
- [ADR 0005 — TTL and audience](../adr/0005-token-ttl-and-audience.md)
- [ADR 0006 — Signing keys](../adr/0006-mesh-token-signing-keys.md)
- [RFC 8693 — OAuth 2.0 Token Exchange](https://www.rfc-editor.org/rfc/rfc8693)

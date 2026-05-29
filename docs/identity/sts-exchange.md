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

When the **gateway** surfaces STS or token-validation failures to MCP clients, use Trogon allocation `-32100` … `-32199`:

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
2. **SVID / workload attestation** — `WorkloadAttestor` validates `actor_token` (X.509 SVID PEM or future mTLS peer cert) against the SPIFFE trust bundle for the SVID’s trust domain. Minted `wkl` is the attested SPIFFE ID (`spiffe://<trust-domain>/<path>`), not a bootstrap JWT claim. Trust bundles are loaded from `MCP_STS_TRUST_BUNDLE_PATH` and watched on NATS KV bucket `mcp-trust-bundles` (key = trust domain). Publish bundles with `trogon-sts-publish-trust-bundle`.
3. **Registry lookup** — Resolve `agent_id` from subject token; load agent record from `mcp.registry.agent.lookup` (or hot-path cache).
4. **`aud ∈ allowed_audiences`** — Reject if requested `audience` is not listed on the agent record.
5. **`wkl ∈ allowed_workloads`** — Reject if attested SPIFFE ID (derived `wkl`) is not listed on the agent record (same `access_denied` path as registry workload mismatch).
6. **`purpose ∈ allowed_purposes`** — When registry enumerates purposes, reject unknown or escalated `purpose`.
7. **`act_chain` append + depth + loop detect** — Copy inbound chain; reject if `len ≥ max_depth` (default 8) or duplicate `(agent_id, wkl)`; append exchanger entry `{ sub, agent_id, wkl, iat }` matching inbound token claims.
8. **Mint** — Sign mesh JWT with STS key (`kid` in header); set `exp = iat + ttl` (registry override or default 120 s); emit success audit.

### STS attestation modes (`MCP_STS_REQUIRE_ATTESTATION`)

| Env | `actor_token` | Minted `wkl` | Audit |
|---|---|---|---|
| unset (shadow) | `spiffe://…` URI or `sha256:` fingerprint accepted without chain verify | Claim/sentinel-derived | Exchange may succeed; **`mcp.audit.sts.deny`** with `reason: wkl_unattested` on every request |
| `1` / `true` | X.509 SVID PEM required; chain verified against trust bundle | SPIFFE ID from SVID SAN | Success/deny only; unattested path rejected at attestor |

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

The table below extends the v1 operational contract. Protocol sections above are unchanged.

| Failure | Trigger | STS behavior | Audit subject + `reason` | Gateway JSON-RPC code | Recovery |
|---|---|---|---|---|---|
| **Registry unavailable** | `mcp.registry.agent.lookup` NACK/timeout, or registry circuit breaker open after consecutive dependency failures | Fail-closed: exchange error `server_error` / `dependency_unavailable` | `mcp.audit.sts.deny` or `mcp.audit.sts.error` with `reason: "registry_unavailable"` | `-32107` `authz_unreachable` | Restore registry consumer; breaker auto half-opens after 5 s cool-down (default). No stale-cache grace in v1. |
| **SpiceDB / PDP unavailable** | Synchronous SpiceDB check required and unreachable, or SpiceDB circuit breaker open | Fail-closed: `dependency_unavailable` | `mcp.audit.sts.error` with `reason: "spicedb_unavailable"` | `-32107` `authz_unreachable` | Restore SpiceDB; prefer registry ACLs on the hot path where possible. Breaker cool-down default 5 s. |
| **JWKS stale** (verification only) | Mesh JWKS KV watch lagging; inbound mesh `subject_token` signed with unknown `kid` | Reject at signature step: `invalid_grant` | `mcp.audit.sts.deny` with grant/signature reason | `-32110` invalid token class at gateway ingress | Refresh `mcp-jwks/mesh/current`; maintain overlap window ≥ max mesh TTL + skew ([ADR 0006](../adr/0006-mesh-token-signing-keys.md)). |
| **Signer unreachable** | KMS `Sign` or Vault Transit `sign` failure; file PEM unreadable at startup | Fail-closed: `server_error` / signer unavailable | `mcp.audit.sts.error` with `reason: "signer_unavailable"` | `-32103` `backend_unreachable` | Fix KMS/Vault credentials or key policy; fall back to overlapping `kid` only after JWKS publisher confirms new key. |
| **Act-chain entry revoked / unknown** | Inbound `act_chain` entry `agent_id` resolves to `not_found` or `revoked` in registry during chain walk | Fail-closed: `act_chain_entry_revoked` | `mcp.audit.sts.deny` with `reason: "act_chain_entry_revoked"`, `offending_index`, `offending_agent_id` | `-32115` `act_chain_principal_unknown` | Remove or re-register agent; audit includes offending hop index. |

### Chain resolution latency

STS walks every inbound `act_chain` entry with an `agent_id` before minting. Resolution uses a TTL-bounded chain cache (default 60 s) separate from the per-agent registry cache. Env `STS_CHAIN_RESOLUTION_MODE`:

| Mode | Behavior |
|---|---|
| `off` | Skip chain walk (emergency only) |
| `cache` | **Default v1.** Walk with chain-resolution cache |
| `strict` | Walk on every exchange (no chain cache) |

If chain walk adds > 5 ms to an exchange, STS logs a warning span. Target exchange P99 remains < 40 ms with `cache` mode; use `strict` only when audit requires cold registry reads every hop.

### Latency probe and alerting

Run `trogon-sts-probe` as a sidecar (not inside the STS process). It fires a synthetic exchange every `probe_interval_secs` (default 10 s), tracks rolling P50/P95/P99, and publishes to `mcp.metrics.sts.latency`. When P99 exceeds `p99_threshold_ms` (default 40 ms), it also emits `mcp.audit.sts.latency_violation`. Wire `mcp.audit.*` subjects to PagerDuty via the standard MCP audit ETL.

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

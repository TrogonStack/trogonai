# ADR 0011: NATS Auth-Callout Integration

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | OAuth 2.0 MCP integration; NATS-callout plugin tier — perimeter auth-callout, distinct from Tier 2.5 `mcp.plugin.*` policy callouts; Phase 0 auth callout + subject ACL |
| **Related** | `docs/identity/nats-callout-plugin.md`; `docs/identity/oauth-mcp-integration.md`; `docs/identity/sts-exchange.md`; `docs/adr/0001-tenancy-model.md`; `docs/adr/0002-identity-layers.md`; `docs/adr/0003-bootstrap-vs-mesh-tokens.md`; `docs/a2a/explanation/auth-callout-design.md` |

## Context

NATS Authorization Callout is the canonical mechanism for delegating **CONNECT-time** authentication to an external service. When auth callout is enabled on a NATS Account, the server does not accept static credentials alone for User connections. On each CONNECT it publishes an authorization request to a trusted subscriber; the subscriber validates external credentials and replies with either a signed **User JWT** (`nats.jwt` in the response claims) or a signed **denial** (`nats.error` with an opaque category string). Official reference: [NATS Authorization Callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout).

Trogon's MCP gateway program already owns OAuth / OIDC integration, SPIFFE trust-bundle consumption, and perimeter identity mapping for HTTP and NATS clients. The gateway is the natural external auth-callout authority because:

1. **Single identity pipeline.** OAuth access tokens, SPIFFE JWT-SVIDs, and mTLS client certificates all converge on the same bootstrap User JWT claim shape (`sub`, `tenant`, `caller_id`, `roles`, `nats.pub` / `nats.sub`) documented in `docs/identity/oauth-mcp-integration.md` and `docs/identity/jwt-claim-schema.md`.
2. **Bootstrap credential role.** ADR 0003 accepts the auth-callout User JWT as the **bootstrap** credential; STS exchange on `mcp.sts.exchange` (ADR 0004) consumes that bootstrap downstream. Callout is the only mint path for connect-time NATS credentials at the edge zone.
3. **Tenancy alignment.** ADR 0001 hybrid model maps resolved tenant → NATS Account (`aud` on the minted User JWT). Callout resolves account hints from CONNECT metadata before minting ACL templates scoped to the edge zone only.

Without this ADR, every NATS connection would independently re-validate OAuth tokens against the IdP (JWKS or RFC 7662 introspection), re-load SPIFFE bundles, or re-parse mTLS chains. That pattern is expensive under reconnect storms, inconsistent when gateway and a hypothetical standalone verifier drift on claim mapping, and incompatible with centralized revocation and audit.

**What stays undecided and breaks downstream work:**

- Permission templates for MCP edge subjects (`mcp.gateway.request.>`, callback subtrees) vs. A2A templates — blocks Phase 0 subject ACL enforcement for MCP clients.
- Cache key shape and TTL caps — blocks IdP load modeling and revocation SLOs.
- Whether callout runs embedded in `trogon-mcp-gateway` or as an extended `a2a-auth-callout` binary — blocks deployment topology and HA runbooks.
- Audit subject naming for callout decisions — blocks SIEM wiring for perimeter auth.

**Current branch snapshot:**

- `a2a-auth-callout` crate ships a working `$SYS.REQ.USER.AUTH` subscriber, wire envelopes, and A2A permission templates; plain `subscribe` without queue group today.
- `trogon-mcp-gateway` validates inbound JWTs at ingress but does **not** host the CONNECT-time callout handler for MCP tenants.
- Design spec `docs/identity/nats-callout-plugin.md` (Block F / Block C paper) defines target MCP behavior; this ADR locks the integration shape for implementation.

### Perimeter vs. post-connect policy

Callout answers **who connected to NATS** and **which subject ACL** the session receives. It does **not** replace:

| Concern | Owner | Notes |
|---------|-------|-------|
| Per-tool SpiceDB checks | `trogon-mcp-gateway` ingress | After CONNECT |
| Mesh token mint | `trogon-sts` on `mcp.sts.exchange` | Bootstrap → mesh (ADR 0003) |
| Tier 2.5 policy plugins | Gateway on `mcp.plugin.{plugin_name}` | Post-connect message path |
| Bridge internal mint | `a2a.bridge.auth.callout.request` | JSON path; not `$SYS.REQ.USER.AUTH` |

Confusing Tier 2.5 **policy** callouts on `mcp.plugin.*` (Block F item 7 in the extensibility sense) with **auth** callout on `$SYS.REQ.USER.AUTH` is a common integration mistake. This ADR covers **auth** callout only.

---

## Decision

**The MCP gateway is the NATS server's auth-callout service.** The gateway fleet (or a dedicated callout deployment sharing the same handler code) exposes a **queue-group consumer** on the auth-callout subject. On CONNECT, the NATS server delegates to the gateway; the gateway maps the OAuth token / SPIFFE SVID / mTLS cert to a NATS User JWT with a **permission template per tenant role** (`admin` / `agent` / `observer`). A **per-token cache** keyed by `{tenant, token_hash}` (see Implementation notes for `auth_method` segment) uses TTL `min(token_exp - now, max_ttl)`. **Revocation** propagates via short bootstrap JWT TTL, a denylist checked before cache promotion, and an explicit **`auth.revoke`** subject **(proposed)** for early session teardown hints.

### Wire contract (pinned)

| Field | Value |
|-------|-------|
| **Request subject** | `$SYS.REQ.USER.AUTH` |
| **Reply target** | Message `reply-to` inbox (mandatory) |
| **Queue group (proposed)** | `trogon-mcp-callout` |
| **NATS server pin** | 2.14.x (matches `a2a-auth-callout` wire) |

NATS does **not** use a generic `auth.callout` subject. The server publishes authorization requests on `$SYS.REQ.USER.AUTH` only.

### Credential extraction order

The callout handler inspects CONNECT material in this order:

| Priority | Source | Credential kind |
|----------|--------|-----------------|
| 1 | `connect_opts.auth_token` or `connect_opts.jwt` | OAuth access token / SPIFFE JWT-SVID |
| 2 | `connect_opts.pass` | Opaque secret (API key transitional path) |
| 3 | `client_tls.verified_chains` or `client_tls.certs` | mTLS client certificate (when bearer empty) |

Task shorthand `auth.token` maps to **`nats.connect_opts.auth_token`** in the server JWT JSON.

### End-to-end flow

```
External credential (OAuth / SVID / mTLS)
        |
        v
  NATS CONNECT --> nats-server --> $SYS.REQ.USER.AUTH
                                        |
                                        v
                           MCP auth-callout handler
                           (gateway fleet, queue group)
                                        |
                                        v mint User JWT (edge ACL)
                           Caller NATS session --> mcp.gateway.request.>
                                        |
                                        v
                              trogon-mcp-gateway (queue group)
```

HTTP MCP clients may validate OAuth at the gateway HTTP listener (`oauth-mcp-integration.md` Option A/B). NATS-native MCP clients use **Option C** — OAuth bearer at CONNECT → callout. Both paths converge on the **same** bootstrap claim shape and permission templates so STS and SpiceDB see one principal model.

---

## Consequences

### Positive

- **One source of truth for perimeter identity.** OAuth verification, SPIFFE bundle checks, mTLS mapping, tenant resolution, and User JWT minting live in one service family. Gateway ingress, STS exchange, and audit envelopes consume the same bootstrap claims.
- **Cache amortizes OAuth provider load.** Successful mint decisions are cached by credential fingerprint; cache hits skip IdP JWKS/introspection while re-signing a fresh User JWT for the current ephemeral `user_nkey`. SPIFFE and mTLS paths benefit similarly on bursty reconnects.
- **Revocation is bounded and actionable.** Short bootstrap User JWT TTL (`AUTH_CALLOUT_USER_JWT_TTL_SECS`, default 300 s) caps blast radius when OAuth tokens are revoked. Denylist checks before cache hits deny re-CONNECT with revoked credentials. Proposed `auth.revoke` broadcast lets gateway and STS invalidate caches and nudge active MCP sessions early.
- **Horizontal scale via queue group.** Each auth request is delivered to one member of `trogon-mcp-callout`; operators scale callout pods independently of gateway ingress replicas.
- **Audit completeness.** Every allow/deny/error emits to `mcp.audit.gateway.callout.{outcome}` — no silent denials at the perimeter.

### Negative

- **Gateway (callout fleet) becomes a hard dependency of the NATS auth path.** If no callout consumer responds within `authorization.timeout`, `nats-server` falls back to default authorization. MCP tenant Accounts **must** configure default auth as **deny** (no broad static Users). Gateway/callout outage means **default deny on new connections**.
- **Mitigation — cached credentials on the server.** NATS holds the minted User JWT for the TCP session lifetime; existing connections may persist until bootstrap JWT expiry even if callout is down. Operators must not raise bootstrap TTL above ~900 s without compensating revoke latency SLOs.
- **Operational coupling to IdP availability.** OAuth issuer unreachable → deny on cache miss (fail-closed). Dev-only offline grace for cache hits is explicitly **not** a production path.
- **Dual deployment form factors to test.** Extended `a2a-auth-callout` vs. embedded callout in `trogon-mcp-gateway` both must honor identical templates, cache, and audit semantics.

### Neutral

- **Queue group on the callout subject distributes auth load across gateway replicas** (or dedicated callout pods) without changing the wire protocol. NATS delivers each `$SYS.REQ.USER.AUTH` message to one group member.
- **SPIFFE in callout remains partially implemented** in the research backlog; this ADR defines target behavior without blocking A2A-only clusters.
- **Multi-role merge default** (`admin` > `agent` > `observer`) is configurable; intersection merge is an opt-in operator setting.

---

## Alternatives considered

### (a) Pure OAuth validation on every CONNECT (no callout cache, no central gateway handler)

Each CONNECT triggers a full IdP round-trip (JWKS verify or introspection). Some clients might call OAuth libraries locally before CONNECT, but NATS server still needs a signed User JWT response from **some** callout subscriber.

| Assessment | |
|------------|---|
| **Rejected because** | Latency and IdP QPS scale linearly with reconnect storms; no shared positive cache; inconsistent if multiple verifier implementations exist; contradicts shipped `a2a-auth-callout` architecture and NATS callout requirement for Account-scoped User minting. |

### (b) Per-tenant separate auth-callout services

Each customer Account runs its own callout deployment, keys, and IdP registry — maximum blast-radius isolation.

| Assessment | |
|------------|---|
| **Rejected because** | Operational sprawl (N deployments, N signing key rotations, N audit pipelines); duplicates OAuth/SPIFFE mapping logic; conflicts with central gateway fleet model and ADR 0001 escape hatch for operator-level SIEM aggregation; acceptable only as a future regulated split, not the default Trogon integration shape. |

### (c) Static NSC User JWTs without callout

Pre-provision long-lived Users per client; skip CONNECT-time delegation.

| Assessment | |
|------------|---|
| **Rejected because** | No OAuth/SPIFFE/mTLS binding at connect; no tenant role templates; incompatible with MCP OAuth flows and bootstrap → mesh model (ADR 0003); rotation and revocation require NSC churn instead of IdP lifecycle. |

### (d) HTTP-only auth termination (no NATS callout)

MCP clients use HTTP transport only; NATS connections use unrelated static creds.

| Assessment | |
|------------|---|
| **Rejected for NATS-native MCP** | `mcp-nats` transport requires CONNECT-time User JWT minting. HTTP path remains complementary (Option A/B) but does not replace Option C for NATS clients. |

---

## Implementation notes

### Callout subject and subscription

| Item | Value |
|------|-------|
| Subscribe subject | `$SYS.REQ.USER.AUTH` |
| Queue group **(proposed)** | `trogon-mcp-callout` |
| Connect identity | Dedicated service User on AUTH/sys Account (`authorization.auth_callout.auth_users`) |
| Readiness | Write `AUTH_CALLOUT_READY_FILE` when subscriber active (matches `a2a-auth-callout`) |
| Response signing | Callout issuer NKey seed; server trusts issuer public key in `authorization.auth_callout.issuer` |

Implementation constants today: `AUTH_CALLOUT_SUBJECT = "$SYS.REQ.USER.AUTH"` in `a2a-auth-callout/src/subscriber.rs`.

### Deployment form factors (either acceptable)

| Form factor | Description |
|-------------|-------------|
| **A — Extended `a2a-auth-callout`** | Shipped binary gains MCP permission templates and MCP-specific audit; same subscriber |
| **B — Embedded in `trogon-mcp-gateway`** | Gateway process hosts `Subscriber` + `CalloutDispatcher` alongside ingress |

Both **must** use queue group `trogon-mcp-callout` for HA. Plain subscribe without queue group is legacy A2A-only behavior.

### Permission templates per tenant role

Roles arrive on the bootstrap JWT as `roles[]` from IdP groups or SPIFFE mapping. Three templates drive callout minting:

| Role | Intended principal | Edge publish (summary) | Edge subscribe (summary) |
|------|-------------------|------------------------|--------------------------|
| `admin` | Tenant operator | `{prefix}.gateway.request.>`, `{prefix}.sts.exchange`, `{prefix}.control.>` **(proposed)**, `_INBOX.client.>` | `{prefix}.gateway.callback.>`, `{prefix}.audit.>`, `{prefix}.control.>` **(proposed)**, `_INBOX.>` **(proposed)** |
| `agent` | Standard MCP client / workload | `{prefix}.gateway.request.>`, `{prefix}.sts.exchange`, `_INBOX.client.>` | `{prefix}.gateway.callback.{caller_id}.>`, `_INBOX.client.>` |
| `observer` | Read-only auditor | `_INBOX.client.>` only | `{prefix}.audit.>`, `{prefix}.control.discovery.>` **(proposed)**, `_INBOX.client.>` |

`{prefix}` defaults to `mcp` (`MCP_PREFIX`). Explicit **deny** patterns block backend zone (`{prefix}.server.>`, `{prefix}.client.>`, `{prefix}.plugin.>`) for agent/observer roles.

When multiple roles are present, effective template = **most privileged** by order `admin` > `agent` > `observer` unless `MCP_CALLOUT_ROLE_MERGE=intersection`.

`caller_id` must be a **single subject token** (no `.` characters) — substituted into callback subscribe patterns at mint time.

Gateway service and backend MCP server principals are **not** callout-minted; NSC provisions long-lived service Users per `MCP_GATEWAY_PLAN.md` Subject ACL.

### Cache key shape and TTL

| Component | Value |
|-----------|-------|
| **Cache key (proposed)** | `{tenant}:{auth_method}:{token_hash}` |
| Task shorthand | `{tenant, token_hash}` — `auth_method` segment (`oidc` \| `svid` \| `mtls` \| `api_key`) prevents cross-method collisions |
| `token_hash` | SHA-256 hex of raw bearer bytes or leaf cert DER fingerprint |
| **TTL** | `min(token_exp - now, MCP_CALLOUT_CACHE_MAX_TTL_SECS)` — default max TTL **60 s** |
| User JWT mint cap | Also capped by `AUTH_CALLOUT_USER_JWT_TTL_SECS` (default **300 s**) |

On cache hit: skip IdP/bundle verification, re-sign User JWT with fresh `iat`/`exp` and request `user_nkey`. Never log raw tokens; use `oauth_jti_hash` in audit only.

Environment **(proposed):**

| Variable | Default | Meaning |
|----------|---------|---------|
| `MCP_CALLOUT_CACHE_MAX_TTL_SECS` | `60` | Upper bound for cache entry |
| `MCP_CALLOUT_CACHE_MAX_ENTRIES` | `50000` | In-memory LRU bound per instance |

### Revocation propagation

| Mechanism | Behavior |
|-----------|----------|
| Short bootstrap TTL | Primary safety bound even without disconnect wiring |
| JetStream KV denylist **(proposed)** | Key `mcp-oauth-revocations/{iss}/{jti}`; checked before cache hit |
| **`auth.revoke` subject (proposed)** | `auth.revoke.{tenant}.{principal_segment}` — payload `{ jti, sub, revoked_at, reason }`; subscribers include gateway (`mcp.control.disconnect.*`) and STS |
| IdP webhook **(proposed)** | Revoke ingester writes KV + publishes `auth.revoke` |

`auth.revoke` is a **Trogon convention**, not a NATS built-in. Do not confuse with `$SYS.REQ.USER.AUTH`.

### Failure mode = NATS server default-deny

| Condition | Callout behavior | NATS server behavior |
|-----------|------------------|---------------------|
| Callout handler down / slow | No response within `authorization.timeout` | Fallback to default auth — **must be deny** for MCP Accounts |
| OAuth issuer down | Deny on cache miss | Authorization error response |
| OAuth token expired / revoked | Deny `CredentialExpired` / `CredentialRevoked` **(proposed)** | Error response |
| SPIFFE bundle missing | Deny fail-closed | Error response |
| Signing key unavailable | Deny internal error category | Error response |

Tenant Accounts must have **no** broad static Users — only callout-minted JWTs. Break-glass Users require explicit audit.

### Audit

Every callout authorization decision **must** emit an audit event:

| Subject | When |
|---------|------|
| `mcp.audit.gateway.callout.allow` | User JWT minted |
| `mcp.audit.gateway.callout.deny` | Credential or policy denial |
| `mcp.audit.gateway.callout.error` | Signing failure, internal error |

Published to JetStream stream **`MCP_AUDIT`** with filter `mcp.audit.>`. Envelope includes `tenant`, `subject`, `auth_method`, `cache_hit`, `latency_ms`, optional `caller_id`, `roles`, `denial_category`, `oauth_jti_hash` — never raw tokens.

Distinct from gateway MCP tool traffic audit subjects `{prefix}.audit.{outcome}.{direction}.{method_root}`; the literal segment `gateway` here identifies the **callout decision plane**.

### Code and config surfaces affected

| Surface | Change |
|---------|--------|
| `a2a-auth-callout` crate | MCP templates, queue group subscribe, MCP audit emit |
| `trogon-mcp-gateway` | Optional embedded subscriber; shares OAuth verifier config with HTTP path |
| `nats-server` config | `authorization.auth_callout` block per tenant Account |
| `docs/identity/oauth-mcp-integration.md` | Claim mapping Path A; revocation §7.3 |
| `docs/identity/sts-exchange.md` | Bootstrap User JWT as `subject_token` input |
| Operator runbooks | Extend `auth-callout-deployment.md` with MCP env vars |

Required env pins (extends A2A table): `AUTH_CALLOUT_ISSUER_NKEY_SEED`, `AUTH_CALLOUT_SERVER_NKEY_PUBLIC`, `AUTH_CALLOUT_OIDC_ISSUER`, `AUTH_CALLOUT_ALLOWED_ACCOUNTS`, `AUTH_CALLOUT_USER_JWT_TTL_SECS`; MCP additions **(proposed)**: `MCP_PREFIX`, `MCP_CALLOUT_CACHE_MAX_TTL_SECS`, `MCP_CALLOUT_DEFAULT_ROLE`, `AUTH_CALLOUT_OIDC_AUDIENCES`.

---

## Status of supporting work

### Done (branch snapshot)

| Item | Evidence |
|------|----------|
| A2A auth-callout wire protocol | `a2a-auth-callout` crate: `$SYS.REQ.USER.AUTH` subscriber, request/response JWT envelopes, denial categories |
| A2A permission templates | `auth-callout-design.md`, shipped mint paths for OIDC / mTLS |
| Gateway ingress JWT validation | `trogon-mcp-gateway` phased `MCP_GATEWAY_JWT_*` |
| Bootstrap vs. mesh decision | ADR 0003 — callout JWT is bootstrap; STS on critical path for enforce mode |
| Tenancy model | ADR 0001 — account resolution at mint time |
| Design spec (paper) | `docs/identity/nats-callout-plugin.md` — full MCP callout design |

### Pending (implementation)

| Item | Block / owner |
|------|---------------|
| MCP permission templates (`admin` / `agent` / `observer`) in callout handler | Phase 0 / gateway |
| Queue group `trogon-mcp-callout` on subscribe | Callout HA |
| Per-token cache with `{tenant}:{auth_method}:{token_hash}` and TTL formula | Callout handler |
| OAuth → bootstrap claim mapping in MCP callout path | Block C item 6 |
| SPIFFE JWT-SVID path (Path B) | Block 1 attestation / gap ID-10 |
| Revocation KV + `auth.revoke` publisher | Block C / webhook ingester |
| Audit emit `mcp.audit.gateway.callout.{outcome}` | Gateway / callout |
| MCP tenant `nats-server` auth_callout config in operator docs | Block H |
| Embed vs. standalone form factor choice per cluster | Operator decision |
| E2E harness: CONNECT with OAuth bearer → gateway RPC | Block D follow-on |

### Open questions (non-blocking for ADR acceptance)

1. **Central callout minting for many Accounts** vs. per-account callout service — ADR 0001 open question 4; default remains central handler with `AUTH_CALLOUT_ALLOWED_ACCOUNTS`.
2. **Audit subject unification** — `mcp.audit.callout.mint.success` vs. `mcp.audit.gateway.callout.allow`; emit `gateway.callout.*` until envelope unification lands.
3. **`auth.revoke` → `mcp.control.disconnect.*` wiring** — optional; bootstrap TTL cap is mandatory regardless.
4. **Offline grace** (`MCP_CALLOUT_OIDC_OFFLINE_GRACE`) — dev only; production remains fail-closed on cache miss when IdP is down.

---

*Design context: `docs/identity/nats-callout-plugin.md`. This ADR is the durable decision record; implementation tickets follow the spec sections cited above.*

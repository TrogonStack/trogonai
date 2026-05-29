# ADR 0019: OAuth 2.0 MCP Integration

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block C item 6 (OAuth 2.0 MCP integration); unblocks Phase 0 MCP OAuth ingress, auth-callout OAuth CONNECT path, and STS bootstrap exchange wiring |
| **Related** | [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md); [sts-exchange.md](../identity/sts-exchange.md); [jwt-claim-schema.md](../identity/jwt-claim-schema.md); [nats-callout-plugin.md](../identity/nats-callout-plugin.md); [auth-callout-design.md](../a2a/explanation/auth-callout-design.md); [0001-tenancy-model.md](0001-tenancy-model.md); [0003-bootstrap-vs-mesh-tokens.md](0003-bootstrap-vs-mesh-tokens.md); [0004-sts-form-factor.md](0004-sts-form-factor.md); [0005-token-ttl-and-audience.md](0005-token-ttl-and-audience.md); [0011-nats-auth-callout.md](0011-nats-auth-callout.md) |

## Context

The [MCP Authorization specification (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization) mandates OAuth 2.0 / OIDC for HTTP-based MCP server authorization. Trogon's MCP gateway program must accept third-party MCP clients (IDE plugins, desktop agents, CI runners) that obtain access tokens from tenant authorization servers and present them at the Trogon perimeter.

Trogon already terminates external credentials at the NATS boundary through **auth callout** on `$SYS.REQ.USER.AUTH` ([ADR 0011](0011-nats-auth-callout.md)). That path verifies OIDC bearer tokens, SPIFFE JWT-SVIDs, or mTLS client certificates once at CONNECT, mints a **bootstrap NATS User JWT**, and applies **NATS subject ACL templates** per tenant role. Downstream, **STS** on `mcp.sts.exchange` ([ADR 0004](0004-sts-form-factor.md)) exchanges the bootstrap credential for short-lived **mesh JWTs** with per-hop `aud` and narrowed `scope` ([ADR 0003](0003-bootstrap-vs-mesh-tokens.md), [ADR 0005](0005-token-ttl-and-audience.md)).

The composition problem is not "whether OAuth exists" — tenant IdPs already issue OAuth access tokens — but **how MCP OAuth semantics map onto Trogon's two-layer identity model**:

| Layer | Credential | Minted by | Lifetime (default) |
|-------|------------|-----------|-------------------|
| Perimeter | OAuth access token | Tenant authorization server | IdP-defined (often 15–60 min) |
| NATS bootstrap | User JWT | Auth callout | `AUTH_CALLOUT_USER_JWT_TTL_SECS` (300 s) |
| Mesh hop | Mesh JWT | `trogon-sts` | 60–300 s (default 120 s) |

Without pinning this ADR:

- HTTP MCP clients cannot discover how to obtain tokens (Protected Resource Metadata, RFC 8707 `resource` binding) or where validation occurs.
- NATS-native MCP SDK clients cannot CONNECT with OAuth bearers without diverging from A2A callout claim mapping.
- OAuth `scope` strings (`mcp:tools:read`, `mcp:tools:invoke`) remain disconnected from NATS `nats.pub` / `nats.sub` templates and STS tool-scope narrowing.
- Token refresh and revocation semantics across three independent lifetimes stay ambiguous — operators cannot size IdP load, revocation latency, or refresh-token handling.
- Implementation may re-verify OAuth tokens on every NATS publish or call RFC 7662 introspection per RPC, contradicting callout cache design in ADR 0011.

The normative design input is [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md) (Block C paper). This ADR formalizes its recommendation; it does **not** extend behavior beyond that spec.

### Mental model (pinned)

```
OAuth access token (tenant IdP)
        |
        +-- HTTP MCP path --> gateway validates bearer (resource server)
        |                         |
        |                         +--> policy / STS / egress mint
        |
        +-- NATS MCP path --> auth callout verifies bearer at CONNECT
                                  |
                                  +--> bootstrap NATS User JWT (aud = tenant Account)
                                            |
                                            +--> STS exchange --> mesh JWT (aud = hop URI)
```

OAuth access tokens **never** propagate to MCP backends. The gateway mints downstream mesh tokens on egress per [overview.md](../identity/overview.md). Clients must not expect passthrough of inbound OAuth bearers past the gateway edge.

---

## Decision

**Adopt composite Option B + Option C from [oauth-mcp-integration.md §3](../identity/oauth-mcp-integration.md)**, with Option A behavior embedded in the gateway HTTP listener. The tenant authorization server remains the OAuth authorization server; Trogon gateway and auth callout are **resource servers / token validators only** — they never mint OAuth codes or access tokens.

| Transport | OAuth termination | Rationale |
|-----------|-------------------|-----------|
| **HTTP MCP** (remote clients, IDE over HTTPS) | Gateway acts as MCP resource server (**Option A**) **or** trusts a pinned upstream OAuth proxy (**Option B**) via `MCP_GATEWAY_OAUTH_TERMINATION={local,trusted_proxy}` | Satisfies MCP Protected Resource Metadata (RFC 9728), RFC 8707 audience binding, and enterprise SSO edge termination |
| **NATS MCP** (in-mesh SDK, agents) | OAuth bearer at CONNECT --> auth callout --> bootstrap User JWT (**Option C**) | Reuses shipped OIDC verifier; NATS Account ACL enforces tenant boundary ([ADR 0001](0001-tenancy-model.md)) |

**Not adopted alone:** Option A only (NATS CONNECT unauthenticated for NATS-native paths); Option B only (no direct NATS SDK path); Option C only (fails MCP HTTP discovery without embedded HTTP resource server).

### Supported OAuth grant types

Grant type selection follows MCP client kind per [oauth-mcp-integration.md §1.3](../identity/oauth-mcp-integration.md):

| Grant type | Client kind | Requirement | Trogon support |
|------------|-------------|-------------|----------------|
| `authorization_code` + PKCE (`S256`) | Interactive MCP clients (desktop, IDE plugin) | **Required** for public clients per OAuth 2.1 / MCP spec | **Supported** — human operator connects Cursor, Claude Desktop, etc. |
| `client_credentials` | Confidential automation clients | May use step-up on `insufficient_scope`; no browser | **Supported** — CI jobs, headless agent runners with pre-registered client |
| `refresh_token` | Session continuation | Public clients: authorization server **must** rotate refresh tokens (OAuth 2.1 §4.3.1) | **Supported** — client refreshes OAuth access token at IdP; Trogon validates new bearer on next HTTP request or NATS re-CONNECT |

**Explicitly out of scope for v1:** Trogon as OAuth authorization server; OAuth token minting at gateway or callout; implicit grant; resource-owner password grant.

Discovery (MCP-mandated two-stage):

1. **Protected Resource Metadata (RFC 9728)** — gateway serves `/.well-known/oauth-protected-resource` (or path-suffixed variant); `401` responses include `WWW-Authenticate` with `resource_metadata` URL.
2. **Authorization Server Metadata** — client probes tenant IdP endpoints (`/.well-known/oauth-authorization-server`, OIDC discovery). Gateway publishes `authorization_servers` in Protected Resource Metadata; it does not replace IdP metadata endpoints.

### OAuth `aud` / resource binding (pinned)

MCP servers **must** validate tokens issued for the MCP resource server URI ([RFC 8707](https://www.rfc-editor.org/rfc/rfc8707)):

| Token shape | Validation |
|-------------|------------|
| JWT access token | Signature via IdP JWKS; `aud` or `resource` claim matches `MCP_GATEWAY_OAUTH_RESOURCE_URI` (default: gateway public MCP base URL) |
| Opaque access token | RFC 7662 introspection with cache (see Revocation); `active: true` and audience/resource field matches gateway URI |

**Distinction from mesh `aud`:** OAuth token `aud` names the **MCP resource server URI** (e.g. `https://mcp.acme.trogon.ai/mcp`). Mesh token `aud` names a **Trogon hop URI** (e.g. `urn:trogon:mcp:gateway:acme:gw-prod-1`) per [ADR 0005](0005-token-ttl-and-audience.md). These are different namespaces; the gateway maps between them during bootstrap mint and STS exchange.

Invalid or wrong-audience tokens: HTTP `401 Unauthorized` at MCP ingress; NATS CONNECT denial at auth callout with opaque `DenialCategory`.

### Where token verification happens

Verification is **perimeter-only**, not per-message:

| Path | When OAuth is verified | Cache behavior |
|------|------------------------|----------------|
| **HTTP MCP** | Every HTTP MCP request at gateway ingress (`local` mode) **or** once at upstream OAuth proxy (`trusted_proxy` mode) | Gateway may cache introspection results ≤ `MCP_OAUTH_INTROSPECTION_CACHE_SECS` (proposed default 30 s) for opaque tokens |
| **NATS CONNECT** | Auth callout on `$SYS.REQ.USER.AUTH` at CONNECT only | Per-token cache keyed `{tenant}:{auth_method}:{token_hash}` per [ADR 0011](0011-nats-auth-callout.md); TTL `min(oauth_exp - now, MCP_CALLOUT_CACHE_MAX_TTL_SECS)` (default max 60 s) |
| **NATS publish/subscribe after CONNECT** | **Not re-verified** — session uses minted User JWT; NATS server enforces subject ACL | Cache hit skips IdP round-trip; User JWT re-signed with fresh `iat`/`exp` |
| **STS exchange** | Bootstrap User JWT verified by STS; OAuth token **not** re-presented | Mesh mint uses bootstrap as `subject_token` |
| **Gateway MCP RPC (post-CONNECT)** | Validates bootstrap or mesh JWT per [ADR 0003](0003-bootstrap-vs-mesh-tokens.md) enforce mode — **not** OAuth bearer | Mesh proactive refresh at TTL/4 per [ADR 0005](0005-token-ttl-and-audience.md) |

**Trusted proxy mode (`MCP_GATEWAY_OAUTH_TERMINATION=trusted_proxy`):** upstream proxy validates OAuth bearer and forwards attested identity (signed internal JWT or pinned headers). Gateway verifies proxy-issued JWT via `MCP_GATEWAY_OAUTH_TRUSTED_PROXY_ISSUERS`; strips client-forged identity headers per [jwt-claim-schema.md](../identity/jwt-claim-schema.md).

### OAuth scope to NATS subject ACL mapping

OAuth scopes and NATS ACL templates compose **two orthogonal dimensions**:

| Dimension | Source | Governs |
|-----------|--------|---------|
| **NATS subject ACL** | IdP roles/groups --> callout role template (`admin` / `agent` / `observer`) per [ADR 0011](0011-nats-auth-callout.md) | Which NATS subjects the connection may publish/subscribe |
| **OAuth `scope` claim** | IdP-issued space-delimited scopes (e.g. `mcp:tools:read mcp:tools:invoke`) | Capability narrowing for CEL policy and STS mesh `scope` projection |

#### IdP roles --> NATS ACL templates (edge zone)

Resolved at auth callout mint time from IdP groups/roles claim. Templates use `{prefix}` default `mcp`, `{caller_id}` substituted at mint:

| Role | Edge publish (summary) | Edge subscribe (summary) |
|------|------------------------|--------------------------|
| `admin` | `{prefix}.gateway.request.>`, `{prefix}.sts.exchange`, `{prefix}.control.>` (proposed), `_INBOX.client.>` | `{prefix}.gateway.callback.>`, `{prefix}.audit.>`, `{prefix}.control.>` (proposed), `_INBOX.>` (proposed) |
| `agent` | `{prefix}.gateway.request.>`, `{prefix}.sts.exchange`, `_INBOX.client.>` | `{prefix}.gateway.callback.{caller_id}.>`, `_INBOX.client.>` |
| `observer` | `_INBOX.client.>` only | `{prefix}.audit.>`, `{prefix}.control.discovery.>` (proposed), `_INBOX.client.>` |

Explicit **deny** patterns block backend zone (`{prefix}.server.>`, `{prefix}.client.>`, `{prefix}.plugin.>`) for `agent` / `observer`. Multiple roles merge **most privileged** by default (`admin` > `agent` > `observer`).

Bootstrap JWT `aud` = **NATS Account name** (tenant boundary), not OAuth resource URI.

#### OAuth scopes --> bootstrap and mesh claims

| OAuth scope (illustrative) | Bootstrap `scope` (proposed) | Mesh `scope` (STS narrowing) |
|----------------------------|------------------------------|------------------------------|
| `mcp:tools:read` | Copied verbatim to bootstrap `scope` | Gateway/STS may narrow to `tool:{server}::{read_ops}` per registry |
| `mcp:tools:invoke` | Copied verbatim | Narrowed to specific `tool:{server}::{method}` on `tools/call` |
| `mcp:resources:read` | Copied verbatim | Resource read capabilities in CEL |
| `insufficient_scope` at runtime | — | MCP `403` + `WWW-Authenticate` `error="insufficient_scope"`; client step-up authorization |

OAuth `scope` is stored on bootstrap JWT for CEL / STS narrowing; it does **not** directly rewrite `nats.pub` / `nats.sub` arrays. NATS ACL bounds the connection; OAuth scope bounds MCP capability inside that connection.

#### Claim mapping reference (OAuth --> bootstrap --> mesh)

At auth callout mint (OAuth --> bootstrap):

| OAuth / OIDC claim | Bootstrap claim | Transformation |
|--------------------|-----------------|----------------|
| `iss` | *(verification only)* | Must match issuer registry entry; drives tenant resolution |
| `sub` | `sub` | ExternalSubject, e.g. `oidc\|acme\|{sub}` |
| `aud` / `resource` | *(verification only)* | Must match `MCP_GATEWAY_OAUTH_RESOURCE_URI`; not copied to bootstrap `aud` |
| `scope` | `scope` (proposed) | Space-delimited OAuth scopes preserved |
| `client_id` / `azp` | `data.oauth_client_id` (proposed) | Audit + rate limits |
| `exp`, `iat`, `nbf` | `exp`, `iat`, `nbf` | Bootstrap TTL = `min(oauth_exp, AUTH_CALLOUT_USER_JWT_TTL_SECS)` |
| — | `aud` | NATS Account name |
| — | `caller_id`, `tenant`, `data.spicedb_subject`, `nats` | Per [auth-callout-design.md](../a2a/explanation/auth-callout-design.md) |

At STS first exchange (bootstrap --> mesh):

| Bootstrap claim | Mesh claim | Notes |
|-----------------|------------|-------|
| `sub` | `sub`, `originator_sub`, `act_chain[0].sub` | First hop originator |
| `tenant` | `tenant` | Required |
| `scope` | `scope` | Narrowed per registry + request |
| `data`, `nats`, `caller_id` | *(absent)* | Must not appear on mesh token |
| — | `aud` | Requested STS `audience` URI |
| — | `wkl`, `auth_method`, `act_chain`, `session_id` | Per [sts-exchange.md](../identity/sts-exchange.md) |

### Token translation pipeline (pinned)

```
1. VERIFY oauth_access_token
   - signature (JWKS) or introspection (opaque)
   - iss in trusted issuers for tenant
   - aud/resource == MCP_GATEWAY_OAUTH_RESOURCE_URI
   - exp valid (+/- leeway)

2. RESOLVE tenant
   - iss --> tenant_id via issuer registry
   - load SPIFFE / OIDC trust bundle for iss

3. MINT bootstrap User JWT (auth callout)
   - aud = NATS Account for tenant
   - sub, caller_id, data.spicedb_subject, nats ACL from role template
   - optional: scope, data.oauth_client_id (proposed)

4. CONNECT / MCP session established

5. STS EXCHANGE (first mesh hop, when enforce or shadow)
   - subject_token = bootstrap User JWT
   - actor_token = SVID or sentinel
   - audience = urn:trogon:mcp:gateway:{tenant}:{gateway_id}
   - mint mesh JWT with act_chain[0] = originator
```

OAuth access tokens are **not** accepted as mesh tokens at backends.

### Token rotation on inbound JWT rotation

Three credentials have **independent lifetimes**. Refreshing one does not automatically refresh the others ([oauth-mcp-integration.md §5](../identity/oauth-mcp-integration.md)).

| Credential | Typical TTL | Refresh mechanism |
|------------|-------------|-------------------|
| OAuth access token | 15–60 min | OAuth `refresh_token` grant at IdP |
| Bootstrap User JWT | 300 s | Re-CONNECT with valid OAuth bearer --> auth callout re-mint |
| Mesh JWT | 120 s (default) | STS re-exchange on `mcp.sts.exchange` |

#### HTTP MCP path — OAuth refresh

1. Client detects OAuth access token near expiry (proactive threshold, e.g. 60 s before `exp`).
2. Client calls IdP token endpoint with `refresh_token`; IdP rotates refresh token per OAuth 2.1 for public clients.
3. Client resumes MCP HTTP requests with new bearer; gateway validates new token at ingress.
4. If bootstrap or mesh tokens remain valid, no STS call required.
5. If mesh token expired, gateway (or SDK) performs STS exchange using **current** bootstrap as `subject_token`.

#### NATS MCP path — OAuth refresh and bootstrap rotation

1. Client refreshes OAuth access token out-of-band (same as HTTP).
2. Before bootstrap JWT expiry, client **re-CONNECTs** to NATS presenting fresh OAuth bearer.
3. Auth callout verifies new OAuth token (cache miss or new `token_hash`) and mints new User JWT.
4. SDK triggers STS exchange when mesh TTL threshold reached (proactive refresh at TTL/4 per [ADR 0005](0005-token-ttl-and-audience.md)).

#### Inbound JWT rotation at callout (cache behavior)

When the OAuth access token rotates (new `jti`, new `exp`, or refresh):

| Event | Callout behavior |
|-------|------------------|
| Same OAuth bearer, cache hit | Skip IdP verify; re-sign User JWT for current `user_nkey` |
| New OAuth bearer (refresh) | Cache miss on `token_hash`; full JWKS verify; new bootstrap mint |
| OAuth revoked | Denylist check before cache promotion; deny CONNECT with `CredentialRevoked` (proposed) |
| Bootstrap expired mid-session | Existing NATS connection valid until bootstrap JWT `exp`; new operations requiring mesh token need STS re-exchange; re-CONNECT required for new bootstrap |

**(proposed) `oauth_jti` binding:** store IdP `jti` (or token hash) in bootstrap `data.oauth_token_hash` so STS can reject bootstrap minted from a since-revoked OAuth token when introspection cache reports inactive. Not required for v1 if bootstrap TTL ≤ OAuth TTL.

OAuth refresh updates the **OAuth access token only**. It does **not** automatically extend bootstrap or mesh JWT lifetime.

### Multi-tenant isolation (pinned)

Per [ADR 0001](0001-tenancy-model.md), OAuth `iss` is the primary key for tenant resolution at verification. A single OAuth access token must not authorize cross-tenant resources. Validation fails closed if issuer maps to tenant A but requested MCP resource URI belongs to tenant B.

Issuer registry (proposed `mcp-issuer-registry` KV): key `{iss}` --> `{ tenant, nats_account, oauth_resource_uri, jwks_uri, introspection_uri? }`.

After resolution, bootstrap JWT `aud` = NATS Account name; NATS server enforces account subject space regardless of MCP-layer bugs.

### Revocation (defense in depth)

| Strategy | When | Effect |
|----------|------|--------|
| Short TTL | Always | OAuth, bootstrap, mesh `exp` bound exposure window |
| OAuth introspection | Opaque tokens or high-security tenants | Gateway/callout RFC 7662; cache ≤ 30 s (`MCP_OAUTH_INTROSPECTION_CACHE_SECS`) — **not on every NATS message** |
| OAuth JWT `jti` denylist | JWT access tokens with `jti` | JetStream KV `mcp-oauth-revocations/{iss}/{jti}` watched by gateway + callout (proposed) |
| Bootstrap reconnect gate | NATS path | New CONNECT requires fresh OAuth verification |
| Mesh STS deny | Principal revoked in registry | STS rejects exchange; existing mesh tokens expire ≤ 120 s |
| NATS disconnect (proposed) | Critical offboard | `mcp.control.disconnect.{tenant}.{caller_id}` severs MCP session |

When revocation detected: auth callout denies new CONNECT; gateway rejects HTTP MCP with `401` + `invalid_token`; active NATS connections with unexpired bootstrap JWT persist until JWT `exp` unless disconnect control message is used.

### Enforce mode alignment

Per [ADR 0003](0003-bootstrap-vs-mesh-tokens.md): bootstrap User JWT proves CONNECT identity; gated MCP RPCs additionally require mesh token from STS when `MCP_GATEWAY_AGENT_IDENTITY=enforce`.

---

## Consequences

### Positive

- **MCP spec compliance for HTTP clients.** Protected Resource Metadata, RFC 8707 resource binding, PKCE for authorization code, and scope challenge handling are first-class — third-party MCP clients can integrate without Trogon-specific OAuth extensions.
- **Single perimeter pipeline.** HTTP gateway ingress and NATS auth callout converge on identical bootstrap claim shape, role templates, and audit subjects — STS and SpiceDB see one principal model.
- **Reuse of shipped OIDC verifier.** Option C extends existing `a2a-auth-callout` JWKS verification with MCP resource URI check rather than a parallel OAuth stack.
- **Clear separation of OAuth vs mesh namespaces.** Operators cannot conflate OAuth `aud` (MCP resource URI) with mesh `aud` (hop URI); translation is explicit at callout mint and STS exchange.
- **Bounded revocation exposure.** Short bootstrap TTL (300 s) and mesh TTL (120 s) cap blast radius; denylist and introspection cache provide faster cut-off for high-security tenants without per-message OAuth verification.

### Negative

- **IdP load scales with CONNECT rate, not message rate.** Cache amortizes verification, but reconnect storms (SDK refresh, network flap) still generate JWKS/introspection traffic on cache miss. Operators must size IdP capacity and tune `MCP_CALLOUT_CACHE_MAX_TTL_SECS` against revocation SLO.
- **Revocation latency window.** Without denylist/webhook, revoked OAuth tokens remain valid until bootstrap JWT `exp` (≤ 300 s) for active NATS sessions. Production deployments should enable introspection cache or JWT denylist for OAuth layer.
- **Refresh-token handling complexity.** Public MCP clients must implement OAuth 2.1 refresh rotation; NATS transport clients must re-CONNECT when bootstrap approaches expiry even if OAuth was refreshed out-of-band — three lifetimes to coordinate.
- **Composite deployment surface.** HTTP (`local` vs `trusted_proxy`) plus NATS callout doubles configuration and test matrix; misconfigured `MCP_GATEWAY_OAUTH_RESOURCE_URI` causes `-32109 audience_mismatch` class failures.
- **Opaque token path adds introspection dependency.** JWT-first IdPs avoid RFC 7662; opaque tokens require introspection endpoint in issuer registry with fail-closed behavior when unreachable.

### Mitigations

| Risk | Mitigation |
|------|------------|
| IdP overload | Callout cache per ADR 0011; JWKS cache with background refresh; rate limits on OAuth verify path ([ADR 0012](0012-rate-limit-state.md)) |
| Revocation latency | JetStream KV denylist + IdP webhook ingester; optional `mcp.control.disconnect.*` for critical offboard |
| Refresh confusion | Document three-lifetime model in operator how-to; MCP JSON-RPC `-32106 auth_expired` when mesh/bootstrap expired |
| Config drift | Issuer registry KV centralizes `iss` --> tenant + resource URI; audit `mcp.audit.oauth.verify.{success,deny}` (proposed) |

---

## Alternatives considered

### Re-verify the OAuth token on every NATS message

Each publish/subscribe on `mcp.gateway.request.>` would present or re-validate the original OAuth bearer against IdP JWKS or introspection before gateway processing.

| Assessment | |
|------------|---|
| **Rejected because** | Latency and IdP QPS scale linearly with MCP RPC volume, not connection rate; contradicts NATS session model where CONNECT mints a User JWT for the TCP lifetime; incompatible with ADR 0011 callout cache design (`{tenant}:{auth_method}:{token_hash}` with TTL cap); wastes CPU on messages already bounded by NATS subject ACL and mesh JWT validation at gateway ingress. |

### RFC 7662 OAuth introspection on every request

Call introspection endpoint for every HTTP MCP request and every NATS message instead of JWT signature verification with JWKS or cached introspection.

| Assessment | |
|------------|---|
| **Rejected because** | Adds 10–100+ ms per check and couples hot path to IdP availability; introspection is appropriate for opaque tokens and periodic cache refresh, not per-RPC synchronous calls; ADR 0011 explicitly caches successful verification decisions; production pattern is short TTL + denylist with introspection cache ≤ 30 s, not inline introspection per operation. |

### Option A only — HTTP gateway as sole OAuth termination (no NATS callout OAuth)

All MCP clients use HTTP transport; NATS connections use static NSC credentials unrelated to OAuth.

| Assessment | |
|------------|---|
| **Rejected because** | NATS-native MCP SDK clients require CONNECT-time User JWT minting; leaves NATS perimeter unbound to OAuth identity; incompatible with account-per-tenant isolation for in-mesh agents; HTTP-only path remains necessary but insufficient alone. |

### Option C only — NATS callout without HTTP resource server

OAuth verification only at CONNECT; no Protected Resource Metadata or HTTP bearer validation.

| Assessment | |
|------------|---|
| **Rejected because** | Fails MCP Authorization spec for third-party HTTP clients (discovery, RFC 9728, RFC 8707); IDE plugins over HTTPS cannot integrate without a separate HTTP OAuth resource server — which is Option A embedded in the gateway listener. |

### Trogon gateway as OAuth authorization server

Gateway mints authorization codes and access tokens; tenant IdP eliminated.

| Assessment | |
|------------|---|
| **Rejected because** | Out of scope per paper spec; duplicates enterprise IdP investments (Keycloak, Auth0, Okta); MCP spec expects resource server / validator role; couples OAuth consent UI and client registration to gateway release cycle. |

### Passthrough of OAuth bearer to MCP backends

Gateway forwards inbound OAuth access token to backend MCP servers for validation.

| Assessment | |
|------------|---|
| **Rejected because** | Backends would need IdP trust material and OAuth scope logic; violates mesh identity model where gateway mints downstream mesh JWT on egress ([ADR 0003](0003-bootstrap-vs-mesh-tokens.md)); exposes long-lived OAuth token to backend blast radius. |

---

## Open questions

Deferred to later ADRs or implementation; do not wire-format pin until decided.

| ID | Question | Notes |
|----|----------|-------|
| OQ-1 | **PKCE for SPA clients** | MCP requires PKCE S256 for authorization code flow. Do all tenant IdPs expose `code_challenge_methods_supported`? Gateway metadata should document minimum IdP requirements per tenant. Headless `client_credentials` clients skip PKCE — confirm MCP step-up semantics. |
| OQ-2 | **DPoP support** | OAuth 2.1 DPoP (demonstrating proof-of-possession) is not in MCP 2025-11-25 normative text. Evaluate whether high-security tenants require DPoP-bound access tokens at gateway ingress in a future ADR. |
| OQ-3 | **Public vs confidential client posture** | MCP prefers Client ID Metadata Documents over pre-registration. Operator policy for auto-approve vs review queue for registered clients; whether Trogon IdP exposes RFC 7591 DCR. |
| OQ-4 | Single vs per-tenant OAuth resource URI on shared gateway hostname | Protected Resource Metadata publishing |
| OQ-5 | Standard mapping from OAuth `scope` to Trogon tool `scope` tokens | CEL policy templates; partial guidance in paper OQ-3 |
| OQ-6 | Introspection cache TTL vs revocation SLA tradeoff | Enterprise security review |
| OQ-7 | `MCP_GATEWAY_OAUTH_TERMINATION` trusted-proxy header contract | Phase 2 gateway config schema |
| OQ-8 | Issuer registry in NATS KV vs static env | Multi-tenant ops automation |
| OQ-9 | Client credentials + human delegation: `act` claim --> `act_chain[0]` | Agent identity Phase 3 |
| OQ-10 | mTLS-bound tokens (RFC 8705) at token endpoint | Parallel to `AUTH_CALLOUT_MTLS` path; unified `CalloutDispatcher` preference order |

---

## Rollback plan

OAuth MCP integration is **optional per tenant**. Rollback does not require code deployment if flags are pre-provisioned.

| Control | Behavior |
|---------|----------|
| **`MCP_TENANT_OAUTH_ENABLED=false`** (proposed per-tenant flag in issuer registry or gateway config KV) | Disables OAuth bearer acceptance at HTTP ingress and OAuth credential extraction at auth callout for that tenant |
| **Fallback credentials** | Tenant reverts to **API key** (opaque `connect_opts.pass`) + **mTLS client certificate** paths already supported by auth callout per [ADR 0011](0011-nats-auth-callout.md) credential extraction order |
| **Protected Resource Metadata** | Gateway returns `404` or omits OAuth discovery document for disabled tenants; existing MCP clients using API key/mTLS continue on NATS CONNECT |
| **STS path unchanged** | Bootstrap mint from API key/mTLS uses same claim shape; mesh exchange unaffected |
| **Deprecation cycle** | Enable OAuth in shadow mode (`MCP_GATEWAY_AGENT_IDENTITY=shadow`) before enforce; disable by flipping tenant flag — no schema migration required |
| **Emergency global kill-switch** | `MCP_GATEWAY_OAUTH_GLOBAL_ENABLED=false` (proposed) disables OAuth verification fleet-wide; all tenants fall back to non-OAuth credential kinds only |

Rollback validation: integration test CONNECT with API key + mTLS succeeds when OAuth disabled; HTTP MCP returns `401` without OAuth bearer (no anonymous access).

---

## Implementation notes

### Configuration reference (pinned names)

| Variable | Component | Description |
|----------|-----------|-------------|
| `MCP_GATEWAY_OAUTH_RESOURCE_URI` | Gateway | Canonical MCP resource URI for RFC 8707 validation |
| `MCP_GATEWAY_OAUTH_TERMINATION` | Gateway | `local` (gateway validates) or `trusted_proxy` |
| `MCP_GATEWAY_OAUTH_TRUSTED_PROXY_ISSUERS` | Gateway | JWKS issuers for proxy-minted internal JWTs |
| `MCP_GATEWAY_OAUTH_INTROSPECTION_URL` | Gateway | Optional RFC 7662 endpoint override per tenant |
| `MCP_OAUTH_INTROSPECTION_CACHE_SECS` | Gateway, callout | Introspection cache (proposed default 30) |
| `AUTH_CALLOUT_OIDC_ISSUER` | Callout | Trusted issuer URL |
| `AUTH_CALLOUT_OIDC_AUDIENCES` | Callout | Expected OAuth audiences (includes MCP resource URI) |
| `AUTH_CALLOUT_USER_JWT_TTL_SECS` | Callout | Bootstrap lifetime (default 300) |
| `AUTH_CALLOUT_ALLOWED_ACCOUNTS` | Callout | Tenant NATS Account allowlist |
| `MCP_TENANT_OAUTH_ENABLED` | Gateway, callout | Per-tenant OAuth enable (proposed; rollback) |

### JSON-RPC error mapping

| Code | Symbol | OAuth-related trigger |
|------|--------|----------------------|
| `-32106` | `auth_expired` | OAuth, bootstrap, or mesh past `exp` |
| `-32107` | `authz_unreachable` | Introspection / STS / registry dependency down |
| `-32109` | `audience_mismatch` | OAuth resource URI or mesh hop `aud` mismatch |
| `-32110` | `invalid_token` | Signature, malformed JWT, wrong issuer |
| `-32105` | `rate_limited` | OAuth verification or STS rate limit exceeded |

### Audit subjects (proposed extensions)

| Subject | When emitted |
|---------|--------------|
| `mcp.audit.oauth.verify.success` | OAuth bearer validated at gateway HTTP ingress |
| `mcp.audit.oauth.verify.deny` | OAuth validation failed |
| `mcp.audit.callout.mint.success` | Auth callout minted bootstrap JWT from OAuth |
| `mcp.audit.oauth.revoke.propagate` | Revocation event written to KV |

Raw OAuth access tokens, refresh tokens, and authorization codes are **excluded** from audit payloads; log `jti` or SHA-256 token hash only.

### Block C exit criteria (from paper)

- [ ] Protected Resource Metadata served at gateway HTTP listener
- [ ] OAuth bearer validation on HTTP MCP ingress (`local` termination mode)
- [ ] Auth callout accepts OAuth JWT at CONNECT with MCP resource URI check
- [ ] Issuer --> tenant registry documented and deployable
- [ ] Claim mapping tables reflected in [jwt-claim-schema.md](../identity/jwt-claim-schema.md) after ADR sign-off
- [ ] Audit subjects wired for OAuth verify and callout mint paths
- [ ] Sequence diagrams validated in shadow mode integration test plan

---

## Status of supporting work

| Work item | Status | Owner / anchor |
|-----------|--------|----------------|
| Block C OAuth MCP integration ADR | **Done** (this document) | `docs/adr/0019-oauth-mcp-integration.md` |
| Normative design spec | **Done** | [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md) |
| NATS auth callout integration ADR | **Done** | [0011-nats-auth-callout.md](0011-nats-auth-callout.md) |
| STS form factor | **Done** | [0004-sts-form-factor.md](0004-sts-form-factor.md) |
| Token TTL and audience | **Done** | [0005-token-ttl-and-audience.md](0005-token-ttl-and-audience.md) |
| Protected Resource Metadata HTTP endpoint | **Pending** | Gateway Block C implementation |
| OAuth bearer validation at HTTP ingress | **Pending** | Gateway |
| MCP resource URI check in callout OIDC verifier | **Pending** | `a2a-auth-callout` / gateway callout |
| Issuer registry KV | **Pending** | Platform ops |
| Per-tenant OAuth rollback flag | **Pending** | Gateway config schema |
| Claim schema updates (`scope`, `data.oauth_client_id`) | **Pending** | Separate PR post-ADR |

---

*Design context: [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md). This ADR is the durable decision record for Block C item 6; implementation tickets follow the spec sections cited above.*

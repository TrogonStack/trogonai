# Tenancy boundary

**Status:** Reference + explanation (Block A, paper). Single source of truth for what may and may not cross tenant boundaries in the Trogon MCP / A2A mesh.

**Diátaxis:** This page is both a **reference** (normative MUST/MUST NOT tables, key shapes, enforcement surfaces) and an **explanation** (why the hybrid model exists, trade-offs, failure modes). Operators and reviewers use it to answer: *is this leak a bug or expected?*

**Related:** [Agent identity overview](overview.md) · [JWT claim schema](jwt-claim-schema.md) · [OAuth MCP integration](oauth-mcp-integration.md) · [Audit envelope reference](reference-audit-envelope.md) · [NATS subject grammar](reference-subject-grammar.md) · [ADR 0001 Tenancy model](../adr/0001-tenancy-model.md) · [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Block A

**Implementation anchors (branch snapshot):**

| Component | Tenant field | Source |
|---|---|---|
| Gateway ingress JWT verify | `tenant` / `https://trogon.ai/tenant` | `trogon-mcp-gateway/src/jwt.rs:143-168`, `406-414`, `660-673` |
| Gateway legacy header (JWT off only) | `trogon-mcp-tenant` | `gateway.rs:30`, `670+`, `644-652` |
| Gateway audit envelope | `tenant` (optional on wire) | `audit.rs:39` |
| Gateway egress mesh cache | `tenant` in `CacheKeyParts` | `egress/cache.rs:12-27` |
| Gateway anomaly features | `tenant` | `anomaly.rs:11-21` |
| STS minted claims | `tenant` | `trogon-sts/src/exchange.rs:244-245`, `token_verify.rs:60` |
| Agent registry lookup | `tenant_hint` vs `agent_id` prefix | `trogon-agent-registry/src/consumer.rs:51-56` |
| Traffic indexer | `tenant` (defaults missing to `"default"`) | `trogon-traffic-view/src/projector/normalize.rs:39-41` |

---

## 1. What is a tenant

### 1.1 Definition

A **tenant** is the smallest unit of **organizational ownership** in the Trogon mesh. Concretely, a tenant is simultaneously:

| Role | Meaning |
|---|---|
| **Organisation** | A customer, business unit, or regulated partition whose agents, users, backends, and audit data belong together. |
| **Billing unit** | Metering, quotas, and retention policies attach to tenant id (directly or via 1:1 NATS Account mapping). |
| **Policy isolation unit** | SpiceDB tuples, CEL policy bundles, registry records, trust bundles, and approval workflows are evaluated **inside** one tenant context. Cross-tenant policy merge is forbidden unless explicitly federated at the operator tier. |

The wire field is **`tenant_id`**: a non-empty slug (lowercase alphanumeric plus `-` / `_`, max length per operator convention; examples in code use `acme`, `tenant-1`). On JWTs the claim name is **`tenant`** or the namespaced alias **`https://trogon.ai/tenant`** ([jwt-claim-schema.md](jwt-claim-schema.md)). Audit envelopes target the canonical name **`tenant_id`** ([reference-audit-envelope.md](reference-audit-envelope.md)); producers today often emit **`tenant`** — indexers normalize both.

A tenant is **not** a NATS subject segment. Subject grammar is fixed without `{tenant}` ([reference-subject-grammar.md](reference-subject-grammar.md)). Tenant identity lives in **connection context** (NATS Account), **signed JWT claims**, and **KV key prefixes** — never in `mcp.gateway.request.{server_id}.{method}`.

### 1.2 Where `tenant_id` first enters a request

Authority flows from the perimeter inward. Client-supplied tenant headers are **never** authoritative when JWT ingress is active.

```
External credential (OAuth / IdP / mTLS)
        │
        ▼
┌───────────────────────────────────────────────────────────┐
│ 1. OAuth / OIDC (HTTP MCP or callout input)                 │
│    iss → tenant_id via issuer registry (proposed §6 oauth)│
└───────────────────────────┬───────────────────────────────┘
                            ▼
┌───────────────────────────────────────────────────────────┐
│ 2. Auth callout at NATS CONNECT                           │
│    Mints bootstrap User JWT: aud = tenant NATS Account    │
│    Optional tenant claim from IdP mapping                 │
└───────────────────────────┬───────────────────────────────┘
                            ▼
┌───────────────────────────────────────────────────────────┐
│ 3. Gateway JWT verify (MCP ingress)                       │
│    Extract tenant from signed bearer only                 │
│    MCP_GATEWAY_JWT_TENANT_CLAIM (default namespaced URL)  │
└───────────────────────────┬───────────────────────────────┘
                            ▼
┌───────────────────────────────────────────────────────────┐
│ 4. STS exchange (mesh hops)                               │
│    tenant copied into minted mesh JWT claims              │
└───────────────────────────┬───────────────────────────────┘
                            ▼
        Registry / SpiceDB / audit / KV — all scoped by tenant_id
```

**Entry points (reference):**

| Stage | Mechanism | Tenant source | Authoritative? |
|---|---|---|---|
| OAuth HTTP MCP | Gateway validates OAuth access token | **`iss` → tenant_id** via issuer registry ([oauth-mcp-integration.md §4.3](oauth-mcp-integration.md#43-translation-pipeline-reference)) | Yes (after token verify) |
| NATS CONNECT | `a2a-auth-callout` | IdP claims projected into bootstrap JWT; **`aud`** = tenant NATS Account name | Yes |
| MCP gateway ingress | `MCP_GATEWAY_JWT_MODE=validate\|require` | JWT claim `tenant` or `https://trogon.ai/tenant` (`jwt.rs:406-414`) | Yes |
| MCP gateway ingress | `MCP_GATEWAY_JWT_MODE=off` | Header `trogon-mcp-tenant` only (`gateway.rs:644-652`) | **Dev only** — forgeable |
| NATS connection (hard tenancy) | Account isolation | **`nats.account`** from CONNECT JWT / connection | Yes in production |
| STS mint | `mcp.sts.exchange` | From `subject_token` claims (`exchange.rs:244-245`) | Yes if subject token verified |

**Authority order** ([ADR 0001](../adr/0001-tenancy-model.md)):

```text
nats.account (when distinct accounts)  >  validated jwt.tenant  >  NEVER client headers
```

When both account and claim are present, they **must agree** per operator mapping table. Mismatch is a **deny** in enforce mode (see §6.1).

### 1.3 Hybrid tenancy model (accepted)

[ADR 0001](../adr/0001-tenancy-model.md) adopts **hybrid** isolation:

| Environment | Mode | Boundary mechanism |
|---|---|---|
| Production multi-customer | **Hard** — NATS Account per tenant | Account cannot publish/subscribe outside its namespace without signed export/import |
| Dev / CI / single-tenant | **Soft** — JWT `tenant` claim | Application-layer scoping (CEL, SpiceDB prefix, KV key prefix) |
| Escape hatch | **`MCP_PREFIX` override** | Coarse subject partition (`acme.mcp.*`) without NSC — labs only, not a customer isolation substitute |

JWT **`tenant` remains required** for audit, CEL (`jwt.tenant`), SpiceDB principal naming (`tenant:{tenant_id}/user:{sub}`), and cross-account SIEM aggregation even under hard tenancy.

### 1.4 `TenantContext` (conceptual)

Downstream services should thread a single value object, not raw strings at boundaries:

```rust
/// Reference — not committed code. Illustrates required fields for reviews.
struct TenantContext {
    tenant_id: TenantId,           // validated slug from claim or mapping
    nats_account: Option<AccountId>, // present in hard mode
    source: TenantSource,          // JwtClaim | NatsAccount | LegacyHeader(dev)
}
```

`TenantId` and `AccountId` are distinct newtypes even when 1:1 mapped — sub-tenant designs may diverge later ([ADR 0001 open question 1](../adr/0001-tenancy-model.md)).

---

## 2. What MUST NEVER cross a tenant boundary

The following artifacts are **tenant-private**. Any read, write, cache hit, subscription, or policy decision that uses tenant A's artifact to serve tenant B is a **security defect**, not an integration feature.

### 2.1 Normative table

| Artifact | Why isolated | Hard mode (account/tenant) | Soft mode (JWT claim) | Code / doc anchor |
|---|---|---|---|---|
| **Signing / verification keys** | Forge cross-tenant tokens | JWKS KV per account; mesh key `mcp-jwks/mesh/current` scoped by account connection | Same bucket, **must not** share process-global verifier state across tenants without `tenant_id` in cache key | `trogon-sts/src/lib.rs:28-30`, [ADR 0006](../adr/0006-mesh-token-signing-keys.md) |
| **Audit envelopes** | Compliance partition, SIEM retention | Stream `MCP_AUDIT` per account; envelope `tenant_id` must match account mapping | Single stream; consumers **filter** by `tenant_id` — publish ACL must prevent cross-tenant forge | `audit.rs:39`, [reference-audit-envelope.md](reference-audit-envelope.md) |
| **ZedTokens** | SpiceDB snapshot cursor; stale allow = wrong tenant graph | Session KV per account; cache key includes `tenant_id` | Key prefix `{tenant_id}/` on `mcp-sessions` entries | [bulk-check-permission.md §4](bulk-check-permission.md#4-cache-key-reference) |
| **Schema-cache entries** | Tool JSON schemas may embed tenant-specific metadata | Cache keyed per gateway process — **must** include `tenant_id` when soft mode shares replica | Today: `SchemaCacheKey { server_id, schema_hash }` only — **gap** under multi-tenant replica | `schema_cache/key.rs:80-84` |
| **Anomaly feature vectors** | Behavioural model per tenant | Metrics subject shared; payload `tenant` required for scorer partition | Same | `anomaly.rs:11-21`, `mcp.metrics.anomaly.features` |
| **Approval requests** | HITL context is tenant-private | `mcp.approvals.{request_id}` — request_id must be unguessable; waiter binds tenant from ingress JWT | Same subject space — **must** reject decision if approver tenant ≠ request tenant | `approvals/types.rs:40-45` |
| **NATS audit subscriptions** | JetStream consumer sees all messages on filter | Durable consumer filter scoped to account's stream only | Consumer filter + envelope `tenant_id` check on ingest | `audit.rs:130`, `projector/mod.rs:17` |
| **JetStream KV reads** | Registry, sessions, config, trust bundles | Bucket per account (same logical name, different account) | Single bucket; key **must** start with `{tenant_id}/` | [reference-subject-grammar.md §7](reference-subject-grammar.md#7-kv-buckets) |
| **Trust bundles (SPIFFE)** | Wrong bundle → accept wrong SVID | KV `mcp-trust-bundles/{trust_domain}` per account; trust domain maps 1:1 to tenant | Key prefix or separate trust_domain per tenant (`spiffe://{tenant}.…`) | `trogon-sts/src/lib.rs:30`, [sts-exchange.md](sts-exchange.md) |

### 2.2 Explanation — why each class is sensitive

**Keys.** Mesh JWT signing keys and JWKS material in `mcp-jwks` let a holder mint arbitrary `sub` / `aud` pairs. Sharing verifiers without tenant partitioning allows tenant B to validate tokens intended for tenant A's backends.

**Audit envelopes.** Legal and SOC2 evidence chains partition by customer. A gateway bug that writes tenant B's `caller_sub` into tenant A's envelope breaks non-repudiation and voids retention contracts.

**ZedTokens.** A cached `checked_at` token from tenant A's SpiceDB graph must never satisfy a `CheckBulkPermissions` for tenant B's resources — the graphs are disjoint even in a shared SpiceDB cluster (tuple ids are tenant-prefixed).

**Schema-cache.** Cached tool parameter schemas are derived from backend replies. Under soft tenancy, two tenants may share a `server_id` string (`github`) with different backend instances — cache keys without `tenant_id` cause cross-tenant schema bleed and incorrect redaction on `tools/call`.

**Anomaly features.** Adaptive access ([adaptive-access.md](adaptive-access.md)) trains on per-tenant denial rates (`recent_denials_60s`). Cross-tenant feature mixing triggers false HITL or missed abuse.

**Approval requests.** Approvers act on high-risk tool invocations with tenant-specific policy. A decision published to `mcp.approvals.{request_id}` must be accepted only when the gateway's verified `tenant_id` matches the parked request.

**NATS audit subscriptions.** A durable consumer on `mcp.audit.>` in soft mode receives all tenants' traffic. Downstream **must** drop or route by envelope `tenant_id` before storage — subscribing is not isolation; **writing** to the wrong tenant index is a leak.

**JetStream KV reads.** Registry keys (`{agent_id}/@latest`), session records, and trust bundles are authoritative. Reading tenant A's registry key while handling tenant B's STS exchange is an elevation path.

**Policy bundles.** Bundles define deny rules, egress allowlists, and rate limits. Activating tenant A's bundle for tenant B's traffic is full policy bypass.

**Trust bundles.** STS validates SVIDs against PEM in `mcp-trust-bundles/{trust_domain}`. Using tenant A's bundle to verify a workload claiming tenant B's SPIFFE path accepts forged agent identity.

### 2.3 Registry agent id convention

Agent ids **must** embed tenant prefix: `{tenant_id}/{agent_slug}` ([registry.md](registry.md), `trogon-a2a-sdk` `AgentId`). Registry lookup honors `tenant_hint`:

```rust
// consumer.rs:51-56 — hint mismatch → NotFound (fail closed)
if !request.agent_id.starts_with("{hint}/") { return NotFound; }
```

Cross-tenant lookup by id alone without hint is a **bug** if the store is shared.

---

## 3. What MAY cross a tenant boundary

Some data is intentionally shared at the **operator** or **platform** tier. These flows must be **aggregated, anonymised, or explicitly exported** — never raw tenant payloads on shared subjects without controls.

| Category | Examples | Conditions |
|---|---|---|
| **Anonymised metrics** | Gateway queue depth, STS P99 latency, SpiceDB error rate, process RSS | No `tenant_id`, `sub`, `agent_id`, or tool names; histograms only |
| **Platform probes** | `mcp.metrics.sts.latency` from STS probe binary | Schema tagged; no customer payload |
| **Cross-tenant compliance reports** | Operator SIEM dashboard "total denials per day" | Aggregated in warehouse **after** tenant partition at ingest; raw envelopes never land in a shared index without `tenant_id` column enforced |
| **Signed policy bundle _templates_** | Org-wide baseline CEL shipped to all tenants | Template is public; **activated** bundle version is still per-tenant |
| **NATS system / $SYS** | Server health | Standard NATS ops — out of MCP scope |

**Explicitly NOT allowed without federation design:**

| Flow | Status |
|---|---|
| STS exchange with `aud` targeting another tenant's backend | **Forbidden** — [ADR 0001](../adr/0001-tenancy-model.md) § Block 2 |
| Shared `mcp-agent-registry` KV without `{tenant}/` key prefix in soft mode | **Forbidden** |
| SpiceDB `CheckPermission` with subject from tenant A on resource owned by tenant B | **Forbidden** |
| Gateway worker serving tenant A traffic reading tenant B session KV | **Forbidden** |

**proposed:** Operator-tier **read-only** audit export via NATS account export/import to a dedicated `SIEM` account — cross-tenant at the **evidence warehouse**, not on the hot MCP path.

---

## 4. Enforcement surfaces (defence in depth)

Isolation is layered. No single layer is sufficient in soft mode; in hard mode, layers 1–2 (account + JWT) are jointly required.

```
Layer 1 ─ JWT tenant claim verify (gateway, STS)
Layer 2 ─ NATS account / subject ACL (production)
Layer 3 ─ JetStream KV key scoping
Layer 4 ─ SpiceDB resource + subject prefix
Layer 5 ─ Policy bundle key prefix
Layer 6 ─ Audit + metrics tagging (detective control)
```

### 4.1 JWT `tenant_id` claim verification

**Where:** Gateway ingress (`trogon-mcp-gateway/src/jwt.rs`), STS subject token verify (`token_verify.rs`), auth callout mint path.

| Check | Enforce mode behavior |
|---|---|
| Bearer signature valid | Reject if invalid (`-32101` class) |
| `tenant` claim present when required | Mesh tokens: **required** ([overview.md](overview.md) cheatsheet); bootstrap: optional but recommended |
| Claim non-empty | Empty string treated as absent (`jwt.rs:414`) |
| Claim matches `nats.account` mapping | **Required** in hard mode when mapping table configured |
| Legacy header `trogon-mcp-tenant` | Ignored when JWT succeeds; stripped on egress (`gateway.rs:715-722`) |

Configuration:

| Env var | Default | Purpose |
|---|---|---|
| `MCP_GATEWAY_JWT_TENANT_CLAIM` | `https://trogon.ai/tenant` | Claim key; falls back to plain `tenant` |
| `MCP_GATEWAY_JWT_MODE` | `off` | `require` for production soft tenancy |

CEL exposure: `jwt.tenant` ([jwt-claim-schema.md](jwt-claim-schema.md)). Policies **should** include `jwt.tenant == expected` for tenant-scoped rules.

### 4.2 NATS subject scoping

**Decision:** Production multi-tenant isolation uses **NATS Account per tenant**, not a `{tenant}` subject segment.

**Rejected alternative (proposed, not adopted):** `mcp.{tenant}.gateway.request.*`

| Criterion | Account per tenant | Tenant in subject (`mcp.{tenant}.gateway…`) |
|---|---|---|
| Aligns with ADR 0001 / A2A Phase 0 | Yes | No — contradicts non-goal |
| Blast radius on ACL bug | Contained to account | Entire cluster |
| Existing grammar / `mcp-nats` | Unchanged backend subjects | Breaking change across crates |
| Gateway HA | One queue group **may** serve multiple accounts via imports (see §5) | Would require prefix in every subscription |
| Operator mental model | Same subjects everywhere; account is boundary | Subject encodes tenant — duplicates topology in two places |

**Soft mode subject partition:** Optional **`MCP_PREFIX=acme.mcp`** per deployment. This shifts all subjects (`acme.mcp.gateway.request.>`) without embedding tenant in the middle of the grammar. It is an escape hatch for labs, **not** a substitute for account isolation in production.

**Subject ACL invariant (hard mode):** Caller User JWT allows only `mcp.gateway.>` (or `{prefix}.gateway.>`) **inside its account**. Cross-account publish is impossible without export/import.

**Known gap:** STS, registry, approvals, and metrics subjects use hardcoded `mcp.*` prefix ([reference-subject-grammar.md §1.3](reference-subject-grammar.md#13-propagation)). In hard mode, those services run **inside the tenant account** — the subject string is duplicated per account namespace, not shared. Prefix unification is a future refactor, not a tenancy model change.

### 4.3 KV scoping

| Bucket | Hard mode | Soft mode |
|---|---|---|
| `mcp-agent-registry` | Per account (isolated bucket instance) | Keys `{tenant_id}/{agent_id}/@latest` |
| `mcp-trust-bundles` | Per account; key `{trust_domain}` | Keys `{tenant_id}/{trust_domain}` **proposed** if shared bucket |
| `mcp-jwks` | Per account | Per-tenant signing key rotation **proposed** or per account |
| `mcp-gateway-config` | Per account | Keys `{tenant_id}/{bundle_id}` |
| `mcp-sessions` | Per account | Keys `{tenant_id}/{session_id}` |

**Read rule:** A gateway worker **must** derive `tenant_id` before any KV get/put and construct the key with that prefix in soft mode. In hard mode, the NATS connection determines the bucket — **still** verify JWT `tenant` matches account mapping.

**Write rule:** Only controller / STS / gateway processes with tenant-scoped credentials may write. Dev `put` without prefix in soft mode is a **data leak** waiting for the next lookup.

### 4.4 SpiceDB resource prefixing

**Principal shape (preferred):**

```text
tenant:{tenant_id}/user:{sub}
tenant:{tenant_id}/agent:{agent_id}
```

**Resource shape (MCP tools):**

```text
object_type: trogon/mcp_tool  (configurable)
object_id:   {server_id}|{tool_name}   — scoped because gateway evaluates inside tenant context
```

**Bulk permission cache key** includes `tenant_id` ([bulk-check-permission.md §4.1](bulk-check-permission.md#41-proposed-key-structure)):

```text
v1|{tenant_id}|{resource_type}|{resource_id}|{permission}|{subject_type}|{subject_id}|{zed_token}|{caveat_hash_or_-}
```

Session-scoped ZedToken in `mcp-sessions` KV: `{tenant_id}/{session_id}` → latest token ([mcp-session-model.md](mcp-session-model.md)).

**Invariant:** SpiceDB requests **must** include tenant in subject or resource namespace such that Authzed cannot return a relationship from another tenant's graph.

### 4.5 Policy bundle key prefix

Policy bundles live in **`mcp-gateway-config`**.

| Mode | Key pattern | Reload watcher scope |
|---|---|---|
| Hard | `{bundle_name}` (bucket already per account) | Gateway connected to that account |
| Soft | `{tenant_id}/{bundle_name}` | Gateway filters watch events by prefix |

Canary activation (`policy.bundle.{id}.canary` vs active) is **per tenant** — never promote tenant A's canary to tenant B's traffic.

Signed bundle verification ([registry.md](registry.md) signing note) uses org release key; **activation** is still tenant-local.

---

## 5. Multi-tenant operator model

### 5.1 Recommended: shared gateway replica pool

**One gateway deployment (queue group `mcp-gateway`) MAY serve multiple tenants** when each tenant is isolated by **NATS Account** and the gateway holds **multiple NATS connections** (one per account) or uses **account imports** to reach tenant namespaces from a shared service account.

| Aspect | Shared replica pool | One gateway deployment per tenant |
|---|---|---|
| **Ops cost** | Lower — single Helm release, horizontal pod autoscaling | Higher — N releases, N connection secrets |
| **Blast radius** | Bug in shared process memory (cache, trace store) can cross tenants if keys omit `tenant_id` | Process memory scoped by deployment; smaller blast radius |
| **Noisy neighbour** | Tenant A spike consumes queue group capacity for B | Isolated queue groups per tenant possible |
| **Connection count** | Gateway opens N account connections or uses imports | One connection per deployment |
| **Suitable for** | Production at scale with strict cache key discipline | Regulated tenants, dedicated SLA, air-gapped customers |

**Recommendation:** Shared replica pool **with**:

1. Hard NATS account isolation in production.
2. All in-memory caches keyed by `tenant_id` (mesh egress cache already: `egress/cache.rs:22-27`).
3. Separate JetStream stream instances per account (same stream name `MCP_AUDIT`, different account).
4. Resource limits per tenant (inflight throttle — `throttle.rs` uses `tenant: String`).

**Dev / CI:** Single account + JWT claim + optional `MCP_PREFIX` — single gateway process is correct.

### 5.2 STS and registry placement

| Service | Multi-tenant pattern |
|---|---|
| **STS** | Queue group `trogon-sts` per account **or** shared STS with strict tenant claim validation and registry `tenant_hint` |
| **Agent registry** | Lookup consumer per account; KV isolated |
| **Auth callout** | Per-account callout preferred ([ADR 0001 open question 4](../adr/0001-tenancy-model.md)); central callout must mint into correct account only |

### 5.3 A2A + MCP unification

For customer X, **A2A and MCP share one NATS Account** when both protocols are offered ([ADR 0001](../adr/0001-tenancy-model.md)). Subject trees differ (`a2a.*` vs `mcp.*`) but **`tenant_id` is identical** across audit, registry, and SpiceDB.

---

## 6. Failure modes

| Failure | Attack / mistake | Expected behavior | Detective signal |
|---|---|---|---|
| **Forged `tenant_id` claim** | Client sends JWT with `tenant: victim` | Reject at verify if iss not trusted; in hard mode reject if claim ≠ account mapping | Audit `deny` + `identity.shadow.violation` in shadow mode |
| **Forgeable header only** | `trogon-mcp-tenant: victim` with JWT off | **Dev only** — production must use `JWT_MODE=require` | Config audit; ingress logs `identity_source: legacy_header` |
| **Trust bundle A for tenant B** | STS loads wrong KV key | Reject SVID — attestor path fails; no mesh mint | `mcp.audit.sts.deny` with attestation reason |
| **Cross-tenant correlation by `request_id`** | Attacker guesses JSON-RPC id used by another tenant | Trace store is in-memory per gateway process today — **must not** expose cross-tenant lookup API; request ids are not globally unique secrets | Do not build global request_id index without tenant partition |
| **Cross-tenant DoS via shared replica** | Tenant A floods `mcp.gateway.request.>` | Per-tenant inflight limits (`throttle.rs`); rate limit `-32105`; queue group fairness is NATS-level — monitor depth per account | `mcp.metrics.gateway.*`, account-level alerts |
| **Registry hint bypass** | Lookup `agent_id=victim/other` with `tenant_hint=attacker` | `NotFound` (`consumer.rs:51-56`) | `mcp.audit.registry.lookup.notfound` |
| **Audit consumer without filter** | SIEM ingests all tenants, indexes wrong partition | **Bug** if indexer writes without `tenant_id` column | Integration test: two tenants → two index partitions |
| **Shared schema cache** | Same `server_id` two tenants | Wrong validation on `tools/call` | Add `tenant_id` to `SchemaCacheKey` before multi-tenant soft prod |
| **ZedToken cache bleed** | Cache key missing `tenant_id` | Allow/deny from wrong graph | Permission cache tests ([bulk-check-permission.md](bulk-check-permission.md)) |
| **Approval subject guessing** | Subscribe to `mcp.approvals.{guessed}` | request_id must be high entropy; gateway binds tenant on park | Short TTL on waiters |

### 6.1 Account vs claim mismatch

When `TenantContext` carries both values:

```text
IF nats_account is known AND jwt.tenant is present:
    REQUIRE mapping(nats_account) == jwt.tenant
    ELSE deny (-32101 invalid_token / tenant_mismatch proposed)
```

Shadow mode: log `would_deny: true`, metric increment, allow (during rollout only).

---

## 7. Audit invariants

### 7.1 Emission rules

| Invariant | Requirement |
|---|---|
| **A1 — Single tenant tag** | Every emitted audit envelope **must** carry exactly one `tenant_id` (or `tenant` alias until unified). No multi-tenant arrays. |
| **A2 — Non-empty in production** | Production enforce mode: `tenant_id` **must** be non-empty. Missing tenant → `error` outcome or reject before emit (policy TBD). |
| **A3 — Source alignment** | Envelope `tenant_id` **must** equal verified JWT tenant, not header echo. |
| **A4 — Producer attribution** | `producer` field or subject family identifies gateway vs STS vs registry ([reference-audit-envelope.md](reference-audit-envelope.md)). |
| **A5 — No cross-tenant PII in metrics** | Anomaly features include `tenant` for partition; platform metrics do not. |

### 7.2 Anomaly features per tenant

`AnomalyFeature` (`anomaly.rs:11-21`) **must** set `tenant` from verified ingress context before publish to `mcp.metrics.anomaly.features`. Scorers **must** filter by tenant before updating risk scores.

### 7.3 Indexer behavior

Traffic projector (`trogon-traffic-view`):

- Queries **require** `tenant` filter (`indexer/postgres.rs:89-91`).
- Missing tenant on ingest defaults to `"default"` — **operational risk** in multi-tenant soft mode; production should reject or quarantine such envelopes.

### 7.4 Tests that should exist (not implemented in this task)

| # | Test | Validates |
|---|---|---|
| T1 | Gateway ingress: JWT tenant A + header tenant B → egress uses A; header stripped | §4.1 |
| T2 | Hard mode: CONNECT account A + JWT tenant B (mismatch) → deny | §6.1 |
| T3 | Registry lookup: `tenant_hint=acme`, `agent_id=globex/foo` → NotFound | §2.3 |
| T4 | STS exchange: subject token tenant A, aud for tenant B backend → deny | §3 |
| T5 | Mesh egress cache: same key parts except tenant → cache miss | `egress/cache.rs` |
| T6 | SpiceDB permission cache: tenant in key prevents cross-tenant hit | [bulk-check-permission.md](bulk-check-permission.md) |
| T7 | Audit publish: every gateway allow/deny/error path sets tenant when JWT present | §7.1 |
| T8 | Two tenants soft mode: interleaved traffic → indexer rows partitioned | §7.3 |
| T9 | Trust bundle KV: tenant A connection cannot read tenant B bucket | §4.3 |
| T10 | Approval flow: decision for tenant A not applied to tenant B parked request | §2.1 |
| T11 | Schema cache: same server_id, different tenant → distinct entries | §2.1 gap |
| T12 | Anomaly publish: feature.tenant matches ingress verified tenant | §7.2 |
| T13 | OAuth iss mapping: unknown iss → no bootstrap mint | [oauth-mcp-integration.md](oauth-mcp-integration.md) |
| T14 | JetStream consumer: envelope with wrong stream account never ingested | §4.2 hard mode |

---

## 8. Cross-references

| Document | Relevance |
|---|---|
| [overview.md](overview.md) | Identity mental model; tenancy paragraph; audience URI shapes include `{tenant}` |
| [oauth-mcp-integration.md](oauth-mcp-integration.md) | `iss` → tenant_id; OAuth perimeter before bootstrap JWT |
| [reference-audit-envelope.md](reference-audit-envelope.md) | `tenant_id` header field; producer matrix |
| [reference-subject-grammar.md](reference-subject-grammar.md) | Subject patterns; KV buckets; hardcoded `mcp.*` services |
| [jwt-claim-schema.md](jwt-claim-schema.md) | Claim names, ingress hardening, CEL `jwt.tenant` |
| [sts-exchange.md](sts-exchange.md) | Trust bundle KV; tenant on minted claims |
| [registry.md](registry.md) | Agent id `{tenant}/{name}`; lookup contract |
| [bulk-check-permission.md](bulk-check-permission.md) | ZedToken cache key with `tenant_id` |
| [mcp-session-model.md](mcp-session-model.md) | Session KV tenant namespacing |
| [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) | Deployment topology; shared control plane |
| [../adr/0001-tenancy-model.md](../adr/0001-tenancy-model.md) | Accepted hybrid decision |

---

## 9. Quick reference card

| Question | Answer |
|---|---|
| Is tenant in the subject path? | **No** |
| Production isolation? | **NATS Account per tenant** |
| Dev isolation? | **JWT `tenant` claim** (+ optional `MCP_PREFIX`) |
| Authoritative tenant source? | **`nats.account` > JWT > never headers** |
| Shared gateway OK? | **Yes**, with per-tenant cache keys and account-scoped streams/KV |
| Cross-tenant audit OK? | **Only** at operator SIEM after explicit export |
| Field name in JWT? | `tenant` or `https://trogon.ai/tenant` |
| Field name in audit? | `tenant_id` (target); `tenant` on wire today |

---

*Block A deliverable. When implementation closes gaps (schema cache tenant key, envelope unification), update §2.1 and §7.4 without changing the isolation model.*

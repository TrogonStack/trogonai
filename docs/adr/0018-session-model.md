# ADR 0018: MCP Session Model

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Session model — where session state lives across queue-group members for `initialize` → operate → close under HA |
| **Related** | [mcp-session-model.md](../identity/mcp-session-model.md) (design context); [overview.md](../identity/overview.md); [act-chain.md](../identity/act-chain.md); [jwt-claim-schema.md](../identity/jwt-claim-schema.md); [agent-traffic.md](../identity/agent-traffic.md); [sts-exchange.md](../identity/sts-exchange.md); [ADR 0009](0009-reply-correlation.md) (reply correlation); [ADR 0016](0016-multi-region-topology.md) (multi-region topology) |

## Context

The Trogon MCP gateway runs as a **NATS queue-group worker** on `mcp.gateway.request.>`: any healthy replica may consume any client message. That shape is correct for throughput and failover on **new** ingress, but it conflicts with how MCP defines a **session** — a sequence of logically related JSON-RPC interactions that begin with `initialize`, negotiate capabilities once, carry a stable session identifier on subsequent turns, and may involve long-lived server→client callbacks (`notifications/progress`, `sampling/createMessage`, …).

Without an agreed session model, Phase 2 features that depend on session-scoped state cannot behave consistently under HA:

| Concern | Why session placement matters |
|---|---|
| Negotiated `protocolVersion` and capabilities | Fixed at `initialize`; every post-init RPC must see the same negotiated view regardless of which replica handles the message |
| ZedToken cache keyed to session id | SpiceDB consistency token scoped to one MCP session ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) SpiceDB section) |
| Lazy backend `initialize` per `(session, server_id)` | Gateway terminates client `initialize`; backend init snapshot must survive replica switch |
| Mesh egress token cache | Cache keys include `session_id`; mint on replica A must be valid when replica B serves the next call |
| Schema cache invalidation | `notifications/tools/list_changed` bumps generation per session binding |
| Rate budgets (ADR 0012) | Session-scoped context may compose with cluster rate state |

**The queue-group tension.** NATS load-balances each message independently. Two consecutive messages from the same client may land on different gateway pods after reconnect, connection churn, or concurrent publishes. Process-local memory — the correct store for **in-flight reply correlation** per [ADR 0009](0009-reply-correlation.md) — is invisible to sibling replicas. Session data placed in the same map would vanish on crash and would be unreachable from peers handling the next turn.

The tension appears concretely the first time:

- A client sends `notifications/initialized` to replica B after replica A answered `initialize`.
- A mesh egress cache lookup uses `session_id` but the mint happened on another replica.
- A `notifications/cancelled` arrives while the original `tools/call` is awaiting a backend reply on replica C.
- An operator scales the gateway Deployment — existing sessions must continue without client re-`initialize`.

**Failover requirement:** survive replica loss **without** requiring client re-`initialize`, except when the session record itself is gone (TTL expiry, explicit close, or KV unavailability beyond SLO).

**Design context:** The full MCP lifecycle mapping, decision matrix, JSON schema, failure-mode table, and observability contract live in [mcp-session-model.md](../identity/mcp-session-model.md). Block C item 2 in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) left the backing-store choice open until this ADR.

**What stays undecided if this decision is not pinned:**

- Gateway implementers cannot code `initialize` / `notifications/initialized` / post-init RPC against a stable schema.
- Operators cannot provision the `mcp-sessions` JetStream KV bucket with key and TTL conventions.
- ZedToken cache, lazy backend init, and mesh egress cache cannot share one HA story.
- Audit and metrics lack authoritative session lifecycle subjects and gauge definitions.
- Phase 3 perf optimization (optional sticky routing) has no authoritative baseline to extend.

### Session state vs reply correlation (orthogonal stores)

| State class | Lifetime | HA requirement | Store (ADR) |
|---|---|---|---|
| In-flight request map (`nuid_g` → client inbox) | One request | Lost on replica crash; client timeout + retry acceptable | In-memory per replica ([ADR 0009](0009-reply-correlation.md)) |
| Session record (`session_id` → init context) | Many requests | **Must survive** replica crash and client reconnect to a different replica | JetStream KV (this ADR) |
| Progress / cancel routing | One long request | Must reach the replica awaiting the backend reply, or be recoverable via inflight sub-keys | KV sub-keys + optional control fan-out |

The gateway **terminates** client `initialize` by default: the client sees `trogon-mcp-gateway` as the MCP server. Backend servers receive a lazy `initialize` on first use per session. Session KV holds both **client-facing** init context and **per-backend** init snapshots.

---

## Decision

**Adopt Strategy A: JetStream KV-backed session state** as the v1 session model. Edge subject grammar remains unchanged (`mcp.gateway.request.{server_id}.{method}`). Session identity travels in the NATS header **`mcp-session-id`** (wire pin in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pins). Any gateway replica reads and writes the session record in JetStream KV on each session-touching turn.

Sticky routing via session-embedded subjects (Strategy B) and per-session ephemeral NATS consumers (Strategy C) are **rejected for v1**; see Alternatives considered. Phase 3 optional read-through LRU or hybrid sticky **hint** headers may layer on top without changing the authoritative KV schema.

### KV bucket

| Property | Value |
|---|---|
| Bucket name **(proposed)** | `mcp-sessions` |
| Storage | JetStream KV |
| Tenancy — hard (NATS account per tenant) | One bucket per account (same pattern as `mcp-gateway-config`) |
| Tenancy — soft (JWT `tenant` claim) | Single bucket; tenant encoded in key prefix |
| Default idle TTL **(proposed)** | 30 minutes, refreshed on each session-touching message |
| Max TTL cap **(proposed)** | 24 hours absolute (`created_at` + cap) regardless of activity |
| History | 1 (latest value only) |
| Regional placement | Per-region bucket only; **not** mirrored cross-region ([ADR 0016](0016-multi-region-topology.md)) |

Provision in the same Terraform/Helm path as `mcp-gateway-config`, `mcp-jwks`, and `mcp-agent-registry`.

### Session id grammar

| Property | Value |
|---|---|
| Issuer | Gateway at successful client `initialize` |
| Wire transport | NATS header `mcp-session-id`; optional lift to mesh JWT `session_id` claim |
| Character set | `[A-Za-z0-9._~-]+` |
| Max length | 128 characters |
| Uniqueness | Gateway-generated opaque id (UUID/ULID-style); client must not supply id at first init |
| MCP alignment | Maps MCP Streamable HTTP `MCP-Session-Id` semantics to NATS headers |

Recommended prefix: `sess_` for log filterability (example: `sess_01JXYZabc`).

### Key format

```
{tenant}.{session_id}
```

| Segment | Constraints | Meaning |
|---|---|---|
| `{tenant}` | `[a-z0-9-]+` | JWT `tenant` claim or NATS account alias; omitted when bucket is already account-scoped (key reduces to `{session_id}`) |
| `{session_id}` | Grammar above | Gateway-issued opaque id; matches `mcp-session-id` header and audit `session_id` |

Optional inflight sub-keys:

```
{tenant}.{session_id}/inflight/{request_id}
```

Sub-keys hold per-request metadata for progress/cancel routing so progress floods do not rewrite the main session document on every notification.

Examples (soft tenancy):

```
acme.sess_01JXYZabc
acme.sess_01JXYZabc/inflight/42
```

Examples (hard tenancy, account-scoped bucket):

```
sess_01JXYZabc
sess_01JXYZabc/inflight/42
```

### Value schema (`trogon.mcp.session/v1`)

Session records are JSON documents with schema discriminator `"trogon.mcp.session/v1"`. Full field semantics are normative in [mcp-session-model.md](../identity/mcp-session-model.md) § Concrete schema.

**Required top-level fields:**

| Field | Type | Meaning |
|---|---|---|
| `schema` | string | `"trogon.mcp.session/v1"` |
| `session_id` | string | Redundant copy of key suffix; aids audit export |
| `tenant` | string | Tenant claim at init |
| `phase` | enum | `initializing` → `operational` → `closing` → `closed` |
| `created_at` | RFC3339 | Session birth |
| `last_seen_at` | RFC3339 | Updated every session-touching message; drives idle TTL refresh |
| `expires_at` | RFC3339 | `min(last_seen_at + idle_ttl, created_at + max_ttl)` |
| `client` | object | `client_id`, `caller_sub`, `protocol_version`, `client_info`, `capabilities` |
| `gateway` | object | `instance_id_init`, `capabilities_aggregate`, `server_info` |
| `inflight_count` | int | Outstanding gateway→backend requests; close guard |

**Optional nested fields:**

| Field | Meaning |
|---|---|
| `bindings.{server_id}` | Per-backend lazy init: `backend_init`, `backend_init_pending`, `zedtoken`, `schema_generation` |
| `mesh.egress_cache_generation` | Invalidates in-process mesh token cache across replicas when incremented |

**Inflight sub-key value** (`…/inflight/{request_id}`):

| Field | Meaning |
|---|---|
| `request_id` | JSON-RPC id |
| `method` | e.g. `tools/call` |
| `gateway_instance_id` | Owning replica for cancel fan-out |
| `gateway_inbox_nuid` | Correlation with [ADR 0009](0009-reply-correlation.md) inbox map |
| `progress_token` | Associates `notifications/progress` |
| `started_at` | RFC3339 |

Writes to the main record use KV compare-and-swap on revision. Inflight sub-keys use short TTL (request deadline + slack) and delete on reply completion.

### Lifecycle and eviction rules

| Event | Action |
|---|---|
| `initialize` (no `mcp-session-id`) | `put` new record; `phase=initializing`; mint `session_id`; return `InitializeResult` + header |
| Any message with valid `mcp-session-id` | `GET` record; reject if `phase=closed`; refresh `last_seen_at` and `expires_at` |
| `notifications/initialized` | CAS `phase=operational` (idempotent) |
| First post-init RPC if client skips notification | Promote to `operational` (logged deviation) |
| Backend lazy init on first `tools/call` etc. | Forward `initialize` to backend if `bindings.{server_id}.backend_init` missing; CAS result |
| Reply completes | Delete inflight sub-key; decrement `inflight_count` |
| Idle past `expires_at` | KV TTL eviction; subsequent requests → `-32106 auth_expired` |
| Operator revoke | `put` with `phase=closed` or explicit delete; bump `mesh.egress_cache_generation` when bootstrap JWT revoked |
| Explicit client close (future convention) | `phase=closing` → wait `inflight_count=0` → `phase=closed` → delete |
| Second `initialize` without close | Mint **new** session id (MCP re-init semantics) |
| KV `create` collision | Fail `-32101 policy_fault`; increment `session_id_collision` counter **(proposed)** |

**Phase transitions:**

```text
(none) --initialize--> initializing --initialized--> operational
operational --explicit close--> closing --inflight=0--> closed
any --TTL/operator revoke--> (record gone) --> client re-initialize
```

### Failure modes (client-visible)

| Condition | Behaviour | Client-visible |
|---|---|---|
| Replica crash mid-session (no inflight) | Session persists in KV; next message hits any replica | Transparent |
| Replica crash mid-request | In-flight inbox map lost; session record remains | JSON-RPC timeout; retry same `mcp-session-id` |
| KV unavailable | **Fail-closed** for session-touching RPCs | `-32103 backend_unreachable` or `-32111 session_store_unavailable` **(proposed)** |
| Stale / unknown `session_id` | KV miss or `phase=closed` / TTL expired | `-32106 auth_expired` — re-`initialize` |
| Valid `session_id` after reconnect | Normal GET; refresh TTL | Success |
| CAS conflict on update | Optimistic lock retry with backoff | Then `-32105 rate_limited` or fault |

### Optional read-through cache (non-normative implementer guidance)

Each gateway replica **may** keep a 5–15 s LRU of session records keyed `{tenant}.{session_id}` to cut KV reads on chatty loops. Invalidate on CAS conflict or `mcp.control.cache.invalidate.*` broadcast. Writes remain authoritative to KV. **Do not** store full in-flight inbox maps in KV except optional inflight sub-keys.

---

## Consequences

### Positive

- **HA without subject changes.** Any replica serves any post-init message after peer crash or rolling deploy; clients need not know which replica handled `initialize`.
- **Orthogonal to reply correlation.** Phase 1/2 correctly keeps per-request inbox maps in memory ([ADR 0009](0009-reply-correlation.md)); session KV handles only cross-turn state — the same separation as JWT state vs request/reply elsewhere in NATS deployments.
- **Already the plan default.** [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Session correlation names bucket `mcp-sessions` and lists stored fields; this ADR makes the schema and lifecycle normative for implementers.
- **No client ACL impact.** Session id stays in headers, not subjects; edge publish patterns remain `mcp.gateway.request.>`.
- **Incremental perf path.** Optional LRU or Phase 3 sticky hint can reduce KV read RTT without forking the schema; Strategy B alone is insufficient for failover unless KV backs mapping anyway.
- **Audit-ready.** Session lifecycle maps to existing `mcp.audit.allow/deny/error.request.*` grammar with `session_id` in envelope ([agent-traffic.md](../identity/agent-traffic.md)).

### Negative

- **+1 KV round-trip per session-touching request.** Typical ~1–3 ms same-cluster; higher cross-AZ. Dominated by backend/policy cost for most MCP methods; chatty progress loops need coalesced writes.
- **Hot-key risk on chatty sessions.** One KV key per active session; progress floods update the same key unless inflight sub-keys and write coalescing (target 1 Hz on progress) are used.
- **CAS contention under concurrent updates.** Rare for typical agent sessions; retry policy required; `session_cas_conflict` metric **(proposed)**.
- **Mesh egress cache still partially in-process.** Today `MeshEgressCache` is process-local; Phase 2 must tie cache lifetime to `mesh.egress_cache_generation` or accept one redundant STS exchange after replica switch.
- **Gateway depends on JetStream KV for session path.** KV outage blocks new sessions and session-touching RPCs (fail-closed).

### Mitigations

| Risk | Mitigation |
|---|---|
| KV read latency | Optional per-replica LRU (TTL ≤ few seconds); profile in Block G before sticky hints |
| Hot keys | Inflight sub-keys; progress write coalescing; separate sub-key TTL |
| CAS conflicts | Single retry with backoff; bounded fault surface |
| KV outage | Fail-closed; metric `mcp.metrics.gateway.session_kv_error` **(proposed)**; runbook for bucket health |
| Cancel routing across replicas | KV inflight sub-key + proposed `mcp.control.session.cancel.{session_id}` fan-out |

### Observability (proposed unless noted)

**Metrics** (precedent: `mcp.metrics.sts.latency` in [sts-exchange.md](../identity/sts-exchange.md)):

| Subject **(proposed)** | Schema **(proposed)** | Purpose |
|---|---|---|
| `mcp.metrics.gateway.session_kv.latency` | `trogon.mcp.metrics.gateway.session_kv.latency/v1` | `op` (`get`/`put`/`delete`), latency percentiles, `outcome` |
| `mcp.metrics.gateway.session.active` | `trogon.mcp.metrics.gateway.session.active/v1` | Gauge `count` — periodic publish per replica |
| `mcp.metrics.gateway.session.expired` | `trogon.mcp.metrics.gateway.session.expired/v1` | Counter on TTL miss / `-32106` session_not_found |
| `mcp.metrics.gateway.session.cas_conflict` | `trogon.mcp.metrics.gateway.session.cas_conflict/v1` | CAS retry exhaustion |

Task shorthand `mcp_sessions_active` maps to the **`session.active`** gauge above.

**Trace spans** (OpenTelemetry, Block G):

| Span | Attributes |
|---|---|
| `mcp.session.initialize` | `mcp.session_id`, `tenant`, `client_id`, `protocol_version` |
| `mcp.session.load` | `mcp.session_id`, `kv.revision`, `session.phase` |
| `mcp.session.update` | `kv.op`, `cas.retry_count` |
| `mcp.session.inflight.register` | `request.id`, `gateway.instance_id` |
| `mcp.session.inflight.cancel` | `request.id`, `cancel.route` (`local`/`control_fanout`) |
| `mcp.session.close` | `inflight_count`, `outcome` |

**Audit subjects** (existing grammar):

| Event | Subject |
|---|---|
| Session created | `mcp.audit.allow.request.initialize` |
| Session denied at init | `mcp.audit.deny.request.initialize` |
| Unknown / expired session | `mcp.audit.deny.request.{method_root}` with `reason: session_not_found` |
| `notifications/initialized` | `mcp.audit.allow.request.notification` |
| Session closed | `mcp.audit.allow.request.initialize` with `extra.event: session_closed` |
| KV store fault | `mcp.audit.error.request.initialize` with `error.tier: session_kv` |

Envelopes use schema `trogon.mcp.audit/v1`. Propagate `traceparent` through session operations per Wire-Format Pins.

**Control-plane signals:**

| Subject | Status | Use |
|---|---|---|
| `mcp.control.gateway.heartbeat.{instance_id}` | Existing (plan) | Detect replica loss; correlate inflight owner death |
| `mcp.control.cache.invalidate.{server_id}` | Existing (plan) | Bump `bindings.{server_id}.schema_generation` in session records |
| `mcp.control.session.cancel.{session_id}` | **Proposed** | Cross-replica cancel delivery |

---

## Alternatives considered

### (a) In-process map only (session state on the replica that handled `initialize`)

Store `{session_id → init context}` in a `HashMap` alongside the reply correlation map on the owning gateway replica.

| Assessment | |
|---|---|
| **Pros** | Lowest steady-state latency; no KV round-trip; simplest code path for single-replica dev. |
| **Cons** | Session vanishes on replica crash or roll; next message routed to a peer has no context — client must re-`initialize` or sees inconsistent capabilities and ZedToken scope; violates operator expectation for HA gateway fleets; conflates two state classes that [ADR 0009](0009-reply-correlation.md) explicitly separates. |
| **Verdict** | **Rejected.** Does not survive replica failover; incompatible with queue-group load balancing across `initialize` and subsequent turns. Acceptable only under the rollback single-replica mode below. |

### (b) Sticky routing via `session_id` → subject mapping (Strategy B)

Introduce a session segment or parallel subject tree, e.g. `mcp.gateway.session.{session_id}.request.{server_id}.{method}` **(proposed — not in codebase)**, consumed only by the replica that created the session.

| Assessment | |
|---|---|
| **Pros** | Lowest steady-state latency when sticky delivery works; session id visible in subject for log filtering. |
| **Cons** | NATS queue groups do not provide sticky delivery by default; requires custom partition assignment or dedicated subscription per session; failover weak unless KV backs mapping anyway; new subject tree, auth-callout ACL updates, client adapter changes, federation rewrite rules; conflicts with "tenancy is not a subject segment" discipline; hot session pins one replica CPU. |
| **Verdict** | **Rejected for v1.** Documented as Phase 3 optional perf optimization in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) only when KV remains authoritative. Near-miss: latency profile may justify optional `mcp-session-affinity` response header hint without changing edge grammar. |

### (c) Per-session NATS subject ownership / ephemeral consumer (Strategy C)

The replica that handles `initialize` creates a dedicated subscription or JetStream consumer for `mcp.gateway.session.{session_id}.>` until close.

| Assessment | |
|---|---|
| **Pros** | Low per-message latency on direct subscription. |
| **Cons** | High subscription churn at session create/teardown; thousands of concurrent sessions ⇒ thousands of subscriptions per replica — NATS memory and consumer limits; consumer tied to one connection context; failover requires orphan detection and handoff protocol (not native in queue groups); still needs KV ownership lease regardless. |
| **Verdict** | **Rejected.** Does not scale for multi-thousand concurrent MCP agent sessions; highest complexity for weakest failover story. |

### (d) Pure sticky NATS subject hashing without KV

Hash `session_id` into queue-group partition so all messages for one session hit the same replica, with no shared store.

| Assessment | |
|---|---|
| **Pros** | No KV read on steady state. |
| **Cons** | Replica death loses session with no recovery; rescheduling hash ring on scale events re-homes sessions silently; cannot satisfy "survive replica loss without re-initialize" requirement. |
| **Verdict** | **Rejected.** Subcase of (b) without the KV backstop that would make hybrid viable. |

---

## Implementation notes

### Session-touching message path

```text
NATS ingress (mcp.gateway.request.{server_id}.{method})
     |
     v
Extract mcp-session-id header
     |
     +-- missing on initialize --> mint session_id, KV PUT (phase=initializing)
     |
     +-- present --> KV GET; reject if missing/closed/expired
     |
     v
Refresh last_seen_at / expires_at (CAS)
     |
     v
Policy / SpiceDB (session-scoped ZedToken from bindings.*.zedtoken)
     |
     v
Lazy backend init if bindings.{server_id}.backend_init null
     |
     v
Register inflight sub-key if long-running; forward to backend
     |
     v
On reply: delete inflight sub-key; audit with session_id
```

### Relationship to other identity docs

| Doc | Interaction |
|---|---|
| [overview.md](../identity/overview.md) | Mesh tokens carry optional `session_id`; binds egress cache and audit correlation |
| [act-chain.md](../identity/act-chain.md) | `act_chain` is delegation lineage; `session_id` is transport session — both in audit |
| [jwt-claim-schema.md](../identity/jwt-claim-schema.md) | `session_id` claim lift from header at init |
| [sts-exchange.md](../identity/sts-exchange.md) | Egress cache key: `(tenant, caller_sub, target_aud, session_id, scope fingerprint)` |

### Environment and feature flags **(proposed)**

| Variable | Default | Meaning |
|---|---|---|
| `MCP_SESSION_KV_BUCKET` | `mcp-sessions` | JetStream KV bucket name |
| `MCP_SESSION_IDLE_TTL_SECS` | `1800` | Idle refresh window (30 min) |
| `MCP_SESSION_MAX_TTL_SECS` | `86400` | Absolute cap (24 h) |
| `MCP_SESSION_KV_ENABLED` | `true` | When `false`, single-replica rollback mode (see Rollback plan) |
| `MCP_SESSION_LRU_TTL_SECS` | `0` | `0` = disabled; else read-through cache TTL |

### Status of supporting work

| Work item | Status | Owner / anchor |
|---|---|---|
| Block C session model ADR | **Done** (this document) | `docs/adr/0018-session-model.md` |
| Normative design spec | **Done** | [mcp-session-model.md](../identity/mcp-session-model.md) |
| Wire pin `mcp-session-id` | **Specified** | [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pins |
| Provision KV bucket `mcp-sessions` | **Pending** (Phase 2) | Terraform/Helm with gateway config |
| Gateway session load/store/CAS | **Pending** (Phase 2) | `trogon-mcp-gateway` |
| Inflight sub-keys + cancel fan-out | **Pending** (Phase 2) | `mcp.control.session.cancel.*` |
| Mesh egress cache generation hook | **Pending** (Phase 2) | `egress/cache.rs` |
| Metrics + trace spans | **Pending** (Block G) | NATS metrics subjects above |
| Close `MCP_GATEWAY_PLAN.md` Block C item 2 checkbox | **Pending** | Editorial after ADR acceptance |

---

## Open questions

1. **Cross-region session affinity** — Sessions are regional (`mcp-sessions` not mirrored per [ADR 0016](0016-multi-region-topology.md)). Clients must connect to the regional NATS URL where the session was created. Global session migration or federation is **deferred to ADR 0016** follow-on and [multi-region.md](../identity/multi-region.md); no cross-region session KV mirror in v1.

2. **Session migration on tenant move** — When a tenant is re-homed between NATS accounts or regions, existing session records in the source bucket are not automatically portable. Operator runbook: drain sessions (TTL) or force `phase=closed` before cutover; **deferred** to tenancy migration ADR / controller work.

3. **Explicit NATS close RPC** — Map HTTP DELETE to a JSON-RPC notification vs new method vs TTL-only ([mcp-session-model.md](../identity/mcp-session-model.md) open question 1).

4. **Cancel fan-out required for v1?** — Is `mcp.control.session.cancel.*` mandatory or is client retry-after-timeout sufficient for initial ship?

5. **Session ↔ bootstrap JWT binding** — Reject session if connect JWT rotated to a different `sub` mid-session?

6. **Virtual MCP sessions** — Single session record with multiple `bindings.*` vs one session per federated target.

7. **Hybrid sticky optimization** — Optional `mcp-session-affinity` response header for clients that can target a subset of connections (Phase 3).

---

## Rollback plan

If JetStream KV session storage causes unacceptable latency, operational incidents, or schema churn in production, operators can revert to **single-replica session semantics** without changing client wire format:

| Step | Action |
|---|---|
| 1 | Set `MCP_SESSION_KV_ENABLED=false` on all gateway replicas (config flag / feature gate). |
| 2 | Scale gateway Deployment to **one replica** (or ensure queue group effectively single-consumer via replica count = 1). |
| 3 | Gateway falls back to in-process session map on the sole replica — functionally equivalent to alternative (a) above. |
| 4 | Monitor `-32106 auth_expired` and session-not-found rates; clients may need re-`initialize` on the sole replica restart. |
| 5 | Document incident; root-cause KV latency, bucket health, or CAS storms before re-enabling HA mode. |

**Re-enable HA path:** Provision/verify `mcp-sessions` bucket health → rolling deploy with `MCP_SESSION_KV_ENABLED=true` and replica count ≥ 2 → validate `mcp.metrics.gateway.session_kv.latency` and absence of elevated `session_kv_error`.

**Deprecation cycle:** v1 ships KV as default ON. Single-replica rollback is supported through Phase 2; removal of the in-process-only fallback is a **Phase 3+** cleanup once KV SLOs are proven in production.

**Schema rollback:** Session JSON schema changes require new `schema` discriminator (`trogon.mcp.session/v2`); gateway reads v1 until migration job completes. Prefer additive fields within v1; breaking changes bump discriminator and gate on dual-read period.

---

*Design context: [mcp-session-model.md](../identity/mcp-session-model.md). This ADR is the durable decision record for Block C item 2; implementation tickets follow the spec sections cited above.*

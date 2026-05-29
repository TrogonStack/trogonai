# ADR 0023: Schema Cache and Invalidation

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Schema cache + invalidation; unblocks schema cache populated by sniffing `tools/list` replies (invalidation wired) and schema-driven redaction |
| **Related** | `docs/identity/tools-list-filtering.md` (schema cache interaction; no dedicated schema-cache paper spec yet); control-plane subject `mcp.control.cache.invalidate.{server_id}`; `rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs`; `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/`; ADR 0015 (tools/list filtering + sniff hook); ADR 0014 (shared `list_changed` invalidation path); ADR 0001 (tenancy / KV namespace) |

## Context

Every `tools/call` through `trogon-mcp-gateway` must know the target tool's JSON Schema (`inputSchema` at minimum) before the policy engine can validate arguments, apply schema-driven redaction (JSONPath rules keyed off schema shape), and emit accurate audit metadata. The same schema material is required on the response path when output redaction lands in Block E.

Fetching schemas from the backend MCP server on **every** `tools/call` is too expensive:

| Cost | Why it matters |
|------|----------------|
| Extra NATS round-trip | Gateway already forwards `tools/call` to `mcp.server.{server_id}.tools.call`; a separate schema-fetch lane doubles backend interaction on the hot path. |
| Backend load | Chatty agents invoke tools in tight loops; uncached schema reads scale with call volume, not catalogue churn. |
| Latency | Schema fetch on the critical path adds p99 to every call, including cache-warm workloads where `tools/list` already returned the same `inputSchema` payloads. |
| Queue-group duplication | Without a shared invalidation story, each gateway replica would independently re-fetch identical schemas after every process restart. |

The gateway therefore maintains an **in-process schema cache** populated by **sniffing** successful `tools/list` replies (and by explicit fetch on cache miss). Entries are keyed by **`(server_id, schema_hash)`** -- not by tool name -- so multiple tools that share an identical `inputSchema` canonical form collapse to one cache row per backend server.

**Why `(server_id, schema_hash)` and not `(server_id, tool_name, schema_hash)`**

Earlier plan text mentioned `{server_id, tool_name, schema_hash}`. The integration test scaffold and `schema_cache` crate pin a leaner key: identical schema bytes on one server must dedupe regardless of tool name. Redaction and validation care about schema **shape**, not catalogue label. Tool name resolution happens before cache lookup on `tools/call`; the cache answers "what is the canonical schema for this hash on this server?"

**Why invalidation cannot be optional**

A stale schema is a **security bug**: redaction rules keyed to JSONPath locations may silently skip new sensitive fields, or over-redact after a benign schema tweak. MCP backends signal catalogue mutation via `notifications/tools/list_changed`. Backend disconnect/reconnect can swap process or session state without a notification if the network drops. TTL is the safety net when notifications are lost.

**What stays undecided and breaks downstream work without this ADR**

- Block E schema-driven redaction has no stable schema source on the `tools/call` path.
- ADR 0015 upstream list filtering can sniff schemas but invalidation semantics are unspecified across queue-group members.
- ADR 0014 permission cache invalidation on `list_changed` lacks a sibling contract for schema rows.
- Operator observability cannot distinguish cache effectiveness from backend slowness.
- Multi-tenant soft mode cannot assess whether `SchemaCacheKey` must gain `tenant_id` (see Open questions).

**Current branch snapshot**

- `trogon_mcp_gateway::schema_cache` scaffold ships `SchemaCacheKey { server_id, schema_hash }`, `InMemorySchemaCache`, `CachedSchema { schema, fetched_at, source }`, `SchemaSource::ToolsListSniff`, and `should_invalidate("notifications/tools/list_changed")`.
- `tests/schema_cache_invalidation.rs` defines the acceptance matrix (population, hit/miss, invalidation triggers, cross-server isolation, singleflight) -- all `#[ignore]` until wiring lands.
- No dedicated paper spec exists under `docs/identity/` for schema cache alone; `docs/identity/tools-list-filtering.md` §5.4 and §6.3 describe interaction points. **This ADR is the canonical decision record** until a focused identity doc is written.

---

## Decision

**Adopt an in-process, per-gateway-replica schema cache** keyed by `(server_id, schema_hash)`, populated primarily by sniffing `tools/list` replies, with three invalidation triggers (`notifications/tools/list_changed`, TTL expiry, server reconnect version bump), server-scoped invalidation, singleflight dedupe on concurrent misses, LRU eviction under a configurable memory cap, and a `schema_cache_enabled` config gate that falls back to direct-fetch behavior.

### Cache key shape

| Component | Definition |
|-----------|------------|
| `server_id` | Backend MCP server identifier from ingress subject (`mcp.gateway.request.{server_id}.*`) -- typed as `ServerId` in code. |
| `schema_hash` | **SHA-256** (32 bytes, hex-encoded for logs) of the **canonical JSON bytes** of the tool's `inputSchema` object. Canonicalization: JSON object keys sorted lexicographically at each object level; UTF-8 encoding; no insignificant whitespace (stable serde/json canonical form pinned at implementation time). |
| **Cache key** | `SchemaCacheKey { server_id, schema_hash }` -- display form `{server_id}:{hex_hash}`. |

Properties:

- Two tools on the same server with byte-identical `inputSchema` share one entry (integration test: `tools_list_stores_schemas_keyed_by_server_id_and_hash`).
- The same `schema_hash` on **different** `server_id` values are distinct entries; invalidating one does not evict the other.
- The cache stores the **unredacted** canonical schema. List responses may carry redacted catalogue views (ADR 0015); `tools/call` redaction reads the canonical entry.

`outputSchema` caching follows the same keying rules when Block E output redaction requires it; Phase 2 acceptance tests focus on `inputSchema` as the gating contract.

### Population

| Path | When | Behavior |
|------|------|----------|
| **Tools/list sniff (primary)** | Gateway receives successful upstream `tools/list` reply (before or after list filtering per ADR 0015) | For each tool in `result.tools[]` with non-null `inputSchema`, compute `schema_hash`, `put` `CachedSchema { schema, fetched_at: now, source: ToolsListSniff }`. |
| **Tools/call miss (secondary)** | `tools/call` arrives and no valid cache entry exists for the resolved `(server_id, schema_hash)` | Singleflight-coordinated backend schema fetch (see below), then `put` with `source: ToolsListSniff` unless an explicit registration API sets `SchemaSource::ExplicitRegistration` in a future extension. |
| **Explicit registration (reserved)** | Operator or bundle pre-registers schemas | `SchemaSource::ExplicitRegistration` -- not required for Block E acceptance; enum exists in scaffold for forward compatibility. |

Sniffing runs on the **gateway worker that handled the upstream reply**; other replicas learn entries via their own upstream traffic or after invalidation-driven refetch. Cross-replica warm sharing is explicitly deferred (Open questions).

### Lookup on `tools/call`

```text
resolve tool name -> inputSchema hash (from call params + catalogue hint or inline schema fetch on miss)
if schema_cache_enabled && cache.get(key) returns fresh entry:
    use CachedSchema.schema for validation / redaction
else:
    singleflight fetch -> populate -> use
```

A **fresh** entry satisfies `now < fetched_at + TTL`. Expired entries behave as cache miss (evicted lazily on `get` or by background sweep).

### Invalidation triggers

All invalidation is **scoped to one `server_id`**. `InMemorySchemaCache::invalidate(server_id)` removes every `(server_id, *)` entry; entries for other servers are untouched.

| Trigger | Detection | Action |
|---------|-----------|--------|
| **`notifications/tools/list_changed`** | MCP notification method equals exactly `notifications/tools/list_changed` (`should_invalidate`) on `mcp.client.{client_id}.notifications.>` | Local `invalidate(server_id)`; publish control-plane fan-out (below). |
| **TTL expiry** | `get` observes `fetched_at + TTL <= now` | Treat as miss; optional eager delete on read path. |
| **Server reconnect version bump** | Gateway backend session layer detects disconnect + successful reconnect for `server_id` (monotonic per-server **schema generation** counter incremented on reconnect) | `invalidate(server_id)` same as notification path. |

**Control-plane fan-out (queue group)**

When any gateway worker invalidates due to `list_changed`, it publishes to:

```text
{prefix}.control.cache.invalidate.{server_id}
```

Payload **(proposed minimal JSON)**:

```json
{ "server_id": "...", "reason": "list_changed", "emitted_at": "<RFC3339>" }
```

All gateway replicas subscribe (queue group **not** used -- every member must drop local state). This aligns with the gateway control-plane subject grammar and `docs/identity/tools-list-filtering.md` §9.2.

Invalidation also drops sibling caches on the same signal where wired (ADR 0014 permission tuple cache, filtered-list cache keys scoped by `server_id`) -- shared handler entry point, distinct storage.

### Cross-server isolation

Invalidation and reconnect handling always receive an explicit `server_id`. Tests require:

- `list_changed` for server A does not evict server B entries.
- Reconnect on server A preserves server B cache hits on subsequent `tools/call`.

Implementation MUST NOT use a global flush except operator emergency API (out of scope Block E).

### Singleflight dedupe

Concurrent cache misses for the **same** `SchemaCacheKey` coalesce to **one** backend schema-fetch. Waiters block on an in-flight future keyed by `SchemaCacheKey`; the leader populates the cache; waiters receive the same `CachedSchema` outcome.

| Property | Requirement |
|----------|-------------|
| Scope | One singleflight map entry per `SchemaCacheKey` |
| Unrelated keys | Concurrent misses for different `(server_id, schema_hash)` pairs proceed in parallel |
| Failure | If leader fetch fails, waiters observe the same error; no negative caching in Block E |
| Blocking | Miss path adds latency equal to one backend fetch; hit path remains in-process only |

### Memory cap and eviction policy

| Parameter | Default **(proposed)** | Meaning |
|-----------|------------------------|---------|
| `MCP_SCHEMA_CACHE_MAX_ENTRIES` | `10000` | Maximum `(server_id, schema_hash)` rows per gateway process |
| `MCP_SCHEMA_CACHE_TTL_SECS` | `120` | Entry soft lifetime; aligned with provisional list-cache TTL in `tools-list-filtering.md` §9.1 |
| `schema_cache_enabled` | `true` | Master switch (Rollback plan) |

**Eviction policy: LRU (Least Recently Used)**

When `put` would exceed `MCP_SCHEMA_CACHE_MAX_ENTRIES`, evict the least-recently **read or written** entry before insert. Track last-access timestamp on `get` and `put`.

| Policy | Verdict |
|--------|---------|
| **LRU** | **Chosen.** Hot tools on busy servers stay resident; cold schemas fall out naturally. Matches gateway cache precedent (ADR 0011 callout cache, ADR 0014 ZedToken cache, mesh token LRU in ADR 0005). |
| **LFU** | Rejected for default -- frequency skew across tools is weak signal; one-shot admin tools would never evict under a small cap while daily drivers churn. |
| **FIFO** | Rejected -- insertion order ignores continued use; a schema referenced on every call could be evicted while long-idle entries remain. |

Eviction is **orthogonal** to invalidation: explicit `invalidate(server_id)` removes all matching keys regardless of LRU age. TTL expiry may delete before LRU pressure.

### Configuration surface

| Setting | Type | Default | Effect |
|---------|------|---------|--------|
| `schema_cache_enabled` | bool | `true` | When `false`, skip cache read/write; every `tools/call` uses direct-fetch path |
| `MCP_SCHEMA_CACHE_TTL_SECS` | u64 | `120` | Entry freshness bound |
| `MCP_SCHEMA_CACHE_MAX_ENTRIES` | usize | `10000` | LRU cap per process |

Environment names follow existing `MCP_*` gateway conventions; exact loader wiring is Block E implementation.

### Observability

| Metric | Type | Labels **(proposed)** | When |
|--------|------|----------------------|------|
| `mcp_schema_cache_hits_total` | counter | `server_id`, `outcome` (`hit`, `miss`, `expired`, `disabled`) | Each `tools/call` schema resolution |
| `mcp_schema_cache_size` | gauge | *(none)* | Current entry count after put/evict/invalidate |
| `mcp_schema_cache_invalidate_total` | counter | `server_id`, `reason` (`list_changed`, `ttl`, `reconnect`, `control_broadcast`) | Each invalidation event |
| `mcp_schema_cache_singleflight_waits_total` | counter | `server_id` | Waiters attached to in-flight fetch |

Structured logs on invalidation include `server_id`, `reason`, and `entries_removed` count; never log full schema bodies (may contain sensitive field names).

---

## Consequences

### Positive

- **Hot-path latency:** After warm `tools/list` or first `tools/call`, schema resolution is an in-memory map lookup -- no extra NATS RTT on cache hit.
- **Backend protection:** Schema bytes are fetched once per distinct `(server_id, schema_hash)` per replica (modulo eviction), not once per tool invocation.
- **Redaction readiness:** Block E JSONPath redaction attaches to stable schema objects the gateway already holds; sniff path reuses the same upstream `tools/list` traffic ADR 0015 requires.
- **Explicit invalidation contract:** `list_changed`, TTL, and reconnect give operators predictable freshness bounds; control broadcast limits queue-group staleness window.
- **Safe degradation:** `schema_cache_enabled=false` restores direct-fetch without code deploy rollback (Rollback plan).
- **Deduplication under burst:** Singleflight prevents thundering herd on cold cache or post-invalidation refetch for popular tools.

### Negative

- **Memory footprint:** Each entry holds full `inputSchema` JSON. Tenants with large catalogue and diverse schemas approach `MCP_SCHEMA_CACHE_MAX_ENTRIES`; LRU evictions cause miss storms until re-warmed.
- **Mitigation:** Size cap + LRU; monitor `mcp_schema_cache_size` and miss ratio; operators raise cap or TTL per fleet shape.
- **Replica inconsistency window:** Without shared KV, replica A may serve from cache while replica B still holds stale rows until TTL, reconnect handling, or control broadcast arrives.
- **Mitigation:** Short TTL (120 s default); mandatory control broadcast on `list_changed`; integration tests for cross-replica invalidation in Block E.
- **Miss-path blocking:** Cold `tools/call` or post-invalidation calls block on backend schema fetch; singleflight limits duplicate fetches but not latency of the first caller.
- **Mitigation:** Encourage client `tools/list` before call bursts; optional pre-warm (Open questions); metrics on `outcome=miss`.
- **Canonicalization drift risk:** If hash algorithm changes between releases, cache keys shift -- one-time miss spike on upgrade.
- **Mitigation:** Pin canonical JSON algorithm in implementation notes; version prefix in hash if algorithm ever changes (future ADR).

### Neutral

- **Filtered-list cache is separate:** `ToolsListCacheKey` in `tools-list-filtering.md` §6 includes `agent_id`, `policy_version`, etc. Schema cache holds canonical schemas only; invalidation on `server_id` clears both stores when wired.
- **Downstream list synthesis deferred:** ADR 0015 ships upstream filtering first; serving `tools/list` entirely from schema cache requires additional acceptance criteria beyond this ADR.
- **outputSchema:** Same machinery applies when output redaction lands; not gating Phase 2 schema-cache E2E.

---

## Rejected alternatives

### No cache; fetch schema on every `tools/call`

Each invocation performs a dedicated backend schema retrieval before policy evaluation.

| Assessment | |
|------------|---|
| **Pros** | Always fresh; simplest mental model; no invalidation bugs. |
| **Cons** | Latency and NATS load scale linearly with call volume; duplicates work already available in recent `tools/list` replies; queue-group replicas repeat identical fetches. |
| **Verdict** | **Rejected** on latency and backend load. Unacceptable for chatty agent workloads and Block E redaction on every call. |

### Cache forever without TTL

Populate on first sniff; invalidate only on explicit `list_changed` or reconnect.

| Assessment | |
|------------|---|
| **Pros** | Maximum hit rate; no TTL miss storms. |
| **Cons** | Unbounded memory if catalogue churns or many distinct schemas; if notification is lost (network partition, missed subscription), stale schema persists indefinitely -- security risk for redaction. |
| **Verdict** | **Rejected** on memory growth and stale-schema risk. TTL is mandatory safety net per `tools-list-filtering.md` §9.1. |

### Key by `(server_id, tool_name)` without schema hash

One cache row per tool name; update row on any schema change for that name.

| Assessment | |
|------------|---|
| **Pros** | Simple lookup from tool name on `tools/call`. |
| **Cons** | Duplicate storage when tools share schemas; tool rename looks like new entry + stale old key unless extra invalidation rules; does not match scaffold types or integration tests. |
| **Verdict** | **Rejected.** Tests pin `(server_id, schema_hash)` dedupe; schema shape is the identity for redaction. |

### Cluster-wide JetStream KV as primary store (default path)

Every `get`/`put` goes to KV bucket `mcp-schema-cache` for cross-replica consistency.

| Assessment | |
|------------|---|
| **Pros** | Strong cross-replica consistency; survives process restart without re-sniff. |
| **Cons** | Adds ~1--3 ms KV RTT on hot path; write amplification on sniff (many tools per list); large JSON values in KV; contradicts in-process LRU precedent for high-churn derived data (ADR 0012, ADR 0014). |
| **Verdict** | **Rejected** as default Block E path. Remains Open question for optional warm tier. |

### LFU or FIFO eviction under cap

Use least-frequently-used or first-in-first-out instead of LRU when `MCP_SCHEMA_CACHE_MAX_ENTRIES` is exceeded.

| Assessment | |
|------------|---|
| **Pros** | LFU retains statistically popular schemas; FIFO is simple to implement. |
| **Cons** | LFU punishes legitimate one-off tools under cap pressure; FIFO evicts actively used entries inserted early. |
| **Verdict** | **Rejected.** LRU matches gateway cache norms and access patterns (repeated calls to hot tools). |

---

## Open questions

1. **Pre-warm on bundle load?** Should gateway startup or bundle hot-swap issue background `tools/list` to each registered `server_id` to populate schema cache before first client call? Trade-off: startup NATS storm vs colder first-call latency. Default: **lazy** (sniff on traffic only) until operator opt-in flag is specified.
2. **Cross-replica cache sharing via JetStream KV?** Optional read-through tier (`mcp-schema-cache` bucket per `docs/identity/multi-region.md`) could reduce duplicate sniff after rolling restart. Not required for Block E; needs latency SLO and value size limits study.
3. **`tenant_id` in `SchemaCacheKey`?** `docs/identity/tenancy-boundary.md` flags gap: soft multi-tenant mode on one replica sharing `server_id` across tenants may require `{tenant, server_id, schema_hash}` before multi-tenant production. Hard NATS-account isolation may keep current key. Track in tenancy ADR follow-up.
4. **Schema hash over full catalogue vs per-tool?** Tests pin per-tool `inputSchema` hash. A aggregate catalogue hash is used for filtered-list cache (`tools-list-filtering.md` §6.1) -- distinct purpose; do not merge key spaces.
5. **Negative caching?** Should failed schema fetch cache a short-lived "not found" tombstone to prevent retry storms on misconfigured tools? Default **no** for Block E (fail closed per call).
6. **Reconnect detection source of truth?** Backend NATS disconnect callback vs explicit `mcp.control.discovery.deregister/register` pairing -- implementation chooses one; both must bump schema generation for `server_id`.

---

## Rollback plan

| Control | Behavior |
|---------|----------|
| **`schema_cache_enabled=false`** | Gateway skips `SchemaCache::get` / `put` on the hot path. Every `tools/call` resolves schema via direct backend fetch (same as pre-cache Phase 1 behavior). No invalidation subscribers required for correctness -- stale-cache class of bugs disappears. |
| **Config reload** | Toggle via environment or KV config reload without binary rollback. Existing in-memory entries are discarded when disabled. |
| **Re-enable** | Set `schema_cache_enabled=true`; cache starts cold. Clients may experience miss latency until `tools/list` or calls re-warm entries. |
| **Emergency flush** | Operator publishes `{prefix}.control.cache.invalidate.{server_id}` with `reason: "operator_flush"` **(proposed)** to drop replica state without disabling feature globally. |

Rollback does **not** remove sniff hooks on `tools/list`; they become no-ops when cache is disabled. Block E tests must assert direct-fetch parity when flag is off (`tests/schema_cache_invalidation.rs` may add a dedicated case in implementation).

---

## Implementation notes

### Code surfaces

| Surface | Responsibility |
|---------|----------------|
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/key.rs` | `ServerId`, `SchemaHash`, `SchemaCacheKey` |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/entry.rs` | `CachedSchema`, `SchemaSource` |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/store.rs` | `SchemaCache` trait, `InMemorySchemaCache` -- extend with TTL check, LRU cap, metrics hooks |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/invalidate.rs` | `should_invalidate`; wire notification + control subscriber |
| `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs` | Sniff hook on upstream `tools/list`; schema resolve on `tools/call` |
| `rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs` | Remove `#[ignore]` as behavior lands |

### Sniff algorithm (normative sketch)

```text
for tool in result.tools:
    if tool.inputSchema is null: continue
    bytes = canonical_json(tool.inputSchema)
    hash = SHA256(bytes)
    key = SchemaCacheKey { server_id, schema_hash: hash }
    cache.put(key, CachedSchema { schema: tool.inputSchema, fetched_at: now, source: ToolsListSniff })
```

Run sniff on the **backend** tool definitions (pre-redaction catalogue) so the cache retains canonical shapes for call-path policy even when list responses are redacted for clients.

### Invalidation handler (normative sketch)

```text
on notification OR control message OR reconnect(server_id):
    cache.invalidate(server_id)
    permission_cache.invalidate_server(server_id)   // ADR 0014
    tools_list_cache.invalidate_server(server_id)   // tools-list-filtering.md §6.3

on cache.get(key):
    if entry.fetched_at + TTL <= now: return None
    touch_lru(key)
    return Some(entry)
```

### Request lifecycle (schema resolution on call)

```text
tools/call ingress
     |
     v
resolve server_id, tool name -> schema_hash hint
     |
     v
schema_cache_enabled?
  no --> direct backend schema fetch --> policy/redaction
  yes --> cache.get(key)
            hit --> policy/redaction
            miss --> singleflight --> fetch --> put --> policy/redaction
```

### Acceptance tests (from scaffold)

| Module | Behavior pinned |
|--------|-----------------|
| `tools_list_populates_cache` | Sniff `inputSchema`; key `(server_id, schema_hash)`; `SchemaSource::ToolsListSniff` |
| `tools_call_cache_hit` | Hit avoids backend schema fetch; miss populates then hits |
| `list_changed_invalidation` | Notification on client notifications subject; refetch after invalidate |
| `ttl_expiry` | Expired entry miss; fresh entry hit |
| `reconnect_version_invalidation` | Reconnect clears server rows; no stale serve |
| `cross_server_isolation` | Invalidate/reconnect scoped per server |
| `concurrent_singleflight` | One backend fetch per key under concurrency |

---

## Status of supporting work

| Item | Status |
|------|--------|
| ADR 0023 (this document) | **Done** |
| `schema_cache` scaffold types + in-memory store | **Done** (branch snapshot) |
| Integration test scaffold | **Done** (`#[ignore]`) |
| Dedicated `docs/identity/schema-cache.md` paper spec | **Not started** -- ADR is interim canonical source |
| Sniff hook on upstream `tools/list` | **Pending** (Block E) |
| TTL + LRU on `InMemorySchemaCache` | **Pending** (Block E) |
| Singleflight layer | **Pending** (Block E) |
| Notification + control broadcast subscribers | **Pending** (Block E) |
| Reconnect version bump wiring | **Pending** (Block E) |
| Metrics `mcp_schema_cache_hits_total`, `mcp_schema_cache_size` | **Pending** (Block E) |
| `schema_cache_enabled` config gate | **Pending** (Block E) |
| Schema-driven redaction consumer | **Pending** (Block E item 5) |

---

*Contract sources: `rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs`, `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/`, `docs/identity/tools-list-filtering.md` §5.4 and §6.3. This ADR formalizes the design until a focused identity paper spec is authored.*

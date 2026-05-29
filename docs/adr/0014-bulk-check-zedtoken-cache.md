# ADR 0014: BulkCheckPermission + ZedToken Cache

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `BulkCheckPermission` + ZedToken cache keyed to MCP session id |
| **Related** | [bulk-check-permission.md](../identity/bulk-check-permission.md), [tools-list-filtering.md](../identity/tools-list-filtering.md), [reference-host-abi.md](../identity/reference-host-abi.md) |

## Context

Phase 1 of `trogon-mcp-gateway` gates SpiceDB only on mutating read paths (`tools/call`, `resources/read`). Each gated request issues one `CheckBulkPermissions` gRPC call with a single item — functionally one permission check per request. `tools/list` is not SpiceDB-gated today; the backend catalogue is forwarded verbatim.

Block E adds **catalog shaping**: every tool in a `*/list` response (starting with `tools/list`, later `prompts/list` and `resources/list`) must be filtered by the same permission the gateway will enforce on `*/call`. The design spec in [bulk-check-permission.md](../identity/bulk-check-permission.md) and [tools-list-filtering.md](../identity/tools-list-filtering.md) defines the integration path.

### The latency floor

Without batching, catalog filtering performs **one SpiceDB RPC per list item**. A typical MCP backend exposes 50–200 tools. A single `tools/list` therefore costs 50–200 round-trips to the PDP — dominating gateway P99 latency and blowing the operator hot-path budget documented in [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md).

Per-item unary `CheckPermission` (or bulk API with one item per tool) is the **latency floor** for `*/list` shaping. It is unacceptable at production catalogue sizes.

### The load problem

Agent sessions poll `tools/list` frequently — on reconnect, after errors, and when clients refresh capability discovery. Naive per-item checks make SpiceDB load **proportional to read rate × catalogue size**. A session that lists 80 tools every 30 seconds generates 9,600 PDP calls per hour per session before any `tools/call`.

Batching all per-item checks into one `BulkCheckPermission` call reduces RPC count from **N to 1** per list response. Caching the returned `ZedToken` and permission results keyed to the MCP session cuts repeat-list cost toward **zero** in steady state.

Together, bulk + cache reduces SpiceDB load by roughly **100×** for typical `tools/list` workloads (first list: 1 RPC; repeat lists within TTL: 0 RPC; `tools/call` on tools seen in the last list: cache hit, 0 RPC).

### Why ZedToken caching is sensitive

SpiceDB returns a `ZedToken` (`checked_at`) with every bulk response. The token is an opaque snapshot cursor — not a capability JWT. The gateway uses it as the `at_least_as_fresh` consistency header on subsequent checks within the same session.

Cached permission results must not outlive the freshness implied by their associated token. **Stale tokens authorize stale data**: if relationships are revoked but the gateway serves a cached allow from an older snapshot, the agent sees tools it should not invoke. The cache design must bind entries to session identity, bound TTL, and explicit invalidation — not unbounded reuse.

### What breaks if this stays undecided

- Block E `tools/list` shaping cannot ship without choosing batch size, cache key shape, and invalidation triggers.
- CEL host ABI `spicedb.bulk_check` ([reference-host-abi.md](../identity/reference-host-abi.md)) cannot be wired without a defined backing cache contract.
- `tools/call` cannot reuse list-shaping results — duplicate PDP load on every invocation after a list.
- Multi-instance gateway HA cannot share session-scoped freshness without a documented key namespace tied to `mcp-session-id`.

**Current state (branch snapshot):**

- `trogon-mcp-gateway::spicedb` uses `CheckBulkPermissions` for single-item `tools/call` but keeps only a process-global `Mutex<Option<String>>` for the last `checked_at` token.
- No per-session ZedToken store, no permission result cache, no list-shaping hook.
- CEL `spicedb.bulk_check` and `cache.get`/`cache.set` are stubs in `cel_builtins/`.

Until ADR 0014 is accepted, treat bulk+cache as **provisional** in implementation PRs.

## Decision

When the gateway authorizes a `*/list` response (e.g. `tools/list`), it batches per-item permission checks into one SpiceDB `BulkCheckPermission` call. The returned ZedToken is cached keyed by `{session_id, principal, permission}` with TTL = `min(zed_expiration, configured_max_ttl)` (default **5 s**). Cache is invalidated on `notifications/tools/list_changed` and on JWT renewal.

### Decision breakdown

| Element | Choice |
|---------|--------|
| Batch API | SpiceDB `CheckBulkPermissions` (product name: BulkCheckPermission) — one RPC per list response for all cache-miss items |
| Cache scope | In-process LRU per gateway worker; not replicated to NATS KV |
| Cache key | `{session_id, principal, permission}` where `principal` is the normalized SpiceDB subject (`{subject_type}:{subject_id}`, default `trogon/principal:{caller_sub}`) |
| Entry TTL | `min(zed_expiration, configured_max_ttl)`; `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS` default **5**, hard ceiling configurable per bundle |
| Invalidation (primary) | `notifications/tools/list_changed` for the affected `server_id`; JWT renewal (new mesh/bootstrap token on the session) |
| Invalidation (safety net) | Per-entry TTL expiry; LRU eviction under memory cap |
| Disable switch | Per-rule or per-tenant `spicedb.cache_ttl_secs: 0` disables caching (always cold PDP path) |

The ZedToken cache holds the **latest snapshot cursor** for the `(session, principal, permission)` tuple. Permission allow/deny results from the bulk response are stored alongside the token under resource-scoped sub-keys (see Implementation notes). Lookups on `tools/call` consult the same session cache before issuing a single-item bulk check.

## Consequences

### Positive

- **Latency floor for `*/list`:** One PDP round-trip per list response (cold cache) regardless of catalogue size N; repeat lists within TTL cost 0 RPCs when the graph is unchanged.
- **SpiceDB load proportional to write rate, not read rate:** Relationship changes and explicit invalidation drive freshness; agent polling and catalogue size do not multiply RPC count.
- **Cache invalidation tied to MCP notifications:** `notifications/tools/list_changed` aligns PDP freshness with backend catalogue mutations — the same signal [tools-list-filtering.md](../identity/tools-list-filtering.md) uses for `schema_cache` invalidation.
- **`tools/call` reuse:** Tools evaluated during the last list bulk check hit the cache on invocation — amortizing list shaping across the session's call volume.
- **Operator tuning:** Short default TTL (5 s) limits staleness window; regulated tenants can set TTL=0 for always-fresh checks without code changes.

### Negative

- **Cache key cardinality** is bounded by `sessions × principals × permissions` (plus resource sub-keys for per-tool results). Busy multi-tenant clusters need per-tenant LRU caps (proposed: 10k entries/tenant) to prevent unbounded memory growth.
- **Eviction policy is LRU:** Under memory pressure, oldest entries are dropped; evicted entries cause PDP misses — correct but higher latency. No cross-worker cache coherence: queue-group members may duplicate bulk checks until tokens converge via session KV.
- **Stale-by-design within TTL window:** A revoke that does not emit `list_changed` and occurs between bulk checks may not be visible until TTL expires or JWT renews. This is an accepted trade-off for the default 5 s window; security-sensitive bundles should lower TTL or disable cache.

### Neutral

- **Per-rule operators can disable caching** by setting TTL=0 — every check goes to SpiceDB; behavior matches Phase 1 single-check path with no regression in fail-closed semantics.
- **Deny results are cached** as well as allows — repeated calls to forbidden tools avoid PDP load; same staleness window applies.
- **WatchService invalidation** (SpiceDB relationship watch) remains a future optional enhancement — not required for Block E acceptance.

## Alternatives considered

### Per-item CheckPermission (rejected)

Issue one unary check (or single-item bulk) per tool in `result.tools[]`.

| Assessment | |
|------------|---|
| RPC count | N per list — linear in catalogue size |
| Latency | Unacceptable at N ≈ 50–200; dominates gateway P99 |
| SpiceDB load | Proportional to agents × poll rate × N |

**Rejected:** Latency and load scale linearly with catalogue size. Cannot meet Block E operator budgets.

### Unbounded in-memory cache (rejected)

Cache all `(session, principal, permission, resource)` results without TTL cap or LRU eviction.

| Assessment | |
|------------|---|
| Memory | Grows with sessions × tools × principals — unbounded in long-running gateways |
| Staleness | Revoked permissions may persist indefinitely without invalidation |
| Security | Violates "no allow cache past trust" invariant from [failure-mode-matrix.md](../identity/failure-mode-matrix.md) |

**Rejected:** Memory risk and unbounded staleness after relationship revokes.

### Persist cache to NATS KV (rejected)

Replicate permission results and ZedTokens to JetStream KV (`mcp-sessions` or dedicated bucket) for cross-instance sharing.

| Assessment | |
|------------|---|
| Consistency | Cross-instance staleness amplification — worker A caches allow, worker B serves deny before KV propagates |
| Write churn | High cardinality tuple cache generates KV write storms on every list |
| Complexity | Requires watch/broadcast invalidation across queue group |

**Rejected:** Cross-instance staleness amplification outweighs hit-rate benefit. Session KV stores only `latest_zed_token` for consistency headers; tuple results stay in-process ([bulk-check-permission.md](../identity/bulk-check-permission.md) §10 lean).

### SpiceDB WatchService as primary invalidation (deferred)

Subscribe to relationship updates and invalidate matching cache entries eagerly.

| Assessment | |
|------------|---|
| Staleness | Lowest after writes |
| Cost | Long-lived gRPC per replica; fan-out on busy graphs |

**Deferred:** Optional Phase 3 enhancement (`spicedb.watch_invalidation: true`). Token bump + TTL + MCP notifications suffice for Block E.

## Implementation notes

### In-process LRU cache

Introduce `ZedTokenCache` (Moka or equivalent) in `trogon-mcp-gateway::spicedb`:

- One LRU map per gateway process.
- Per-tenant entry cap (default 10 000) with `spicedb_cache_eviction_total{reason="lru"}` metric.
- Thread-safe async get/insert; no cross-process replication.

Deprecate the global `check_zed_token_cache` mutex after session-scoped cache is wired.

### Cache key shape

**ZedToken cursor key** (decision lock):

```text
v1|zed|{session_id}|{principal}|{permission}
```

| Segment | Source |
|---------|--------|
| `session_id` | `mcp-session-id` header / session KV from MCP `initialize` |
| `principal` | `{subject_type}:{subject_id}` — default `trogon/principal:{normalized_caller_sub}` |
| `permission` | Bundle-defined permission string — default `invoke` on `trogon/mcp_tool` |

**Permission result sub-key** (per-tool allow/deny from bulk response):

```text
v1|result|{session_id}|{principal}|{permission}|{resource_type}|{resource_id}|{checked_at_token}
```

Including `checked_at_token` in result keys prevents serving a result evaluated under an older snapshot when the session token advances ([bulk-check-permission.md](../identity/bulk-check-permission.md) §4.3).

Optional `caveat_context_hash` (BLAKE3) appended when contextualized caveats are populated on bulk items.

### TTL clamp from ZedToken expiration

On insert after a successful bulk response:

```text
entry_ttl = min(
  zed_expiration,                          # derived from SpiceDB token metadata when available
  MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS,  # default 5
  bundle.spicedb.cache_ttl_secs            # per-rule override; 0 = disable
)
```

If `checked_at` is absent from the response, **do not cache** — use `minimize_latency` only and increment `spicedb_zed_token_missing_total`.

Hard ceiling: env `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_MAX_SECS` clamps operator misconfiguration (proposed max 60 s aligned with session token freshness elsewhere).

### `*/list` shaping flow

1. Gateway forwards `tools/list` upstream; receives full `result.tools[]`.
2. Load cached ZedToken for `(session_id, principal, invoke)`.
3. For each tool, probe result cache; partition into hits and misses.
4. If misses non-empty: one `CheckBulkPermissions` with `items = misses`, `consistency = at_least_as_fresh(cached_token)` or `minimize_latency` if none.
5. Insert allows/denies; update ZedToken cache with response `checked_at`.
6. Filter list to allowed tools; apply CEL/redaction per [tools-list-filtering.md](../identity/tools-list-filtering.md).

**RPC count:** 1 cold; 0 when all N tools hit with current token.

### `tools/call` fast path

When CEL requires SpiceDB:

1. Build result cache key for `(server_id|tool_name, invoke, subject)`.
2. Hit + allowed → skip SpiceDB, forward to backend.
3. Hit + denied → `-32100` `policy_deny`.
4. Miss → single-item bulk check (existing path), insert cache, proceed.

JWT / mesh / STS gates run **before** cache lookup (unchanged order in `gateway.rs`).

### Invalidation handler for `notifications/tools/list_changed`

Wire into existing schema cache invalidation path ([schema_cache/invalidate.rs](../../rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/invalidate.rs)):

| Event | Cache action |
|-------|--------------|
| `notifications/tools/list_changed` for `server_id` | Drop all ZedToken and result entries for sessions bound to that `server_id`; drop filtered-list cache keys per [tools-list-filtering.md](../identity/tools-list-filtering.md) §6.3 |
| JWT renewal on session | Drop all cache entries for `(session_id, *)` — new principal or scope may change outcomes |
| Session terminate / TTL expiry | Drop session-scoped entries on KV delete |

Optional control-plane broadcast (`mcp.control.catalog.invalidate.{server_id}`) for queue-group fan-out when invalidation originates on a worker that did not serve the list.

### CEL `spicedb.bulk_check` host function

Expose bulk checks to Tier 2 CEL per [reference-host-abi.md](../identity/reference-host-abi.md):

```cel
spicedb.bulk_check(subject, permission, resources) -> map<string, bool>
```

Implementation delegates to the same `ZedTokenCache` and bulk RPC helper as native list shaping — not a separate cache namespace. Capability-gated in policy bundles; **not** allowed in `tools_list_filter` program class (pure predicates only).

Default timeout: 500 ms + 50 ms per 100 items; clamp to request deadline from `mcp-deadline-unix-ms`.

### Metrics (normative)

| Metric | Labels |
|--------|--------|
| `spicedb_cache_hit_total` | `tenant`, `method`, `permission` |
| `spicedb_cache_miss_total` | same |
| `spicedb_bulk_check_latency_ms` | `tenant`, `item_count_bucket`, `method` |
| `spicedb_cache_eviction_total` | `tenant`, `reason` (`ttl`, `lru`, `invalidation`) |

Audit envelope extensions: `spicedb.cache_hit`, `spicedb.bulk_items`, `spicedb.zedtoken` per [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §7.

### Files affected

| File | Change |
|------|--------|
| `rsworkspace/crates/trogon-mcp-gateway/src/spicedb.rs` | `ZedTokenCache`, shared bulk helper, deprecate global mutex |
| `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs` | Post-backend `shape_tools_list` hook |
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/spicedb.rs` | Wire `spicedb.bulk_check` |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/invalidate.rs` | Extend invalidation for permission cache |

## Status of supporting work

| Work item | Status |
|-----------|--------|
| Design spec [bulk-check-permission.md](../identity/bulk-check-permission.md) | **Done** (2026-05-28) |
| Design spec [tools-list-filtering.md](../identity/tools-list-filtering.md) | **Done** (2026-05-28) |
| Host ABI [reference-host-abi.md](../identity/reference-host-abi.md) `spicedb.bulk_check` | **Done** (spec); implementation **pending** |
| ADR 0014 (this document) | **Done** |
| `ZedTokenCache` + session keying in `spicedb.rs` | **Pending** |
| `shape_tools_list` gateway hook | **Pending** |
| `list_changed` invalidation extension | **Pending** |
| CEL `spicedb.bulk_check` builtin | **Pending** (stub today) |
| Session KV `latest_zed_token` field | **Pending** ([mcp-session-model.md](../identity/mcp-session-model.md)) |
| Metrics §9 from bulk-check spec | **Pending** |
| SpiceDB WatchService optional invalidation | **Deferred** (Phase 3) |

---

*Distilled from [bulk-check-permission.md](../identity/bulk-check-permission.md). Review target: gateway + identity maintainers before `spicedb.rs` refactor lands.*

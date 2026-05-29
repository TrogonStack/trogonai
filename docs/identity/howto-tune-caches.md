# How to tune schema, ZedToken, and filtered-list caches under load

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform operators running `trogon-mcp-gateway` on a Trogon mesh (co-deployed NATS, SpiceDB, JetStream KV). Not for standalone gateway installs.

**Related:** [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Failure mode matrix](failure-mode-matrix.md) · [BulkCheckPermission + ZedToken cache](bulk-check-permission.md) · [Tools/list filtering](tools-list-filtering.md) · [ADR 0023](../adr/0023-schema-cache-invalidation.md) · [ADR 0014](../adr/0014-bulk-check-zedtoken-cache.md) · [ADR 0015](../adr/0015-tools-list-filtering.md) · [ADR 0031](../adr/0031-latency-budget-benchmarking.md)

| Placeholder | Example value |
|---|---|
| Gateway deployment | `gw-prod-1` (queue group `mcp-gateway`) |
| Hot `server_id` | `github` |
| NATS prefix | `mcp` (`MCP_PREFIX`) |
| Catalogue size (bench fixture) | 50 tools ([ADR 0031](../adr/0031-latency-budget-benchmarking.md) `list-shape` workload) |

Replace placeholders before running commands.

---

## Goal

Observe cache pressure on gateway replicas and adjust schema, ZedToken, and filtered-list cache settings so warm-path latency stays inside the Block G budget ([ADR 0031](../adr/0031-latency-budget-benchmarking.md)) without exceeding per-process memory caps.

When you finish, you have a documented baseline (hit ratios, entry counts, p99 added latency on `tools/call` and `tools/list`), tuned env/KV knobs with rollback values recorded, and confirmation that `notifications/tools/list_changed` still clears all three caches on the affected `server_id`.

---

## When to use this

Use this guide when:

- P99 gateway span or client RTT grows while backend MCP latency is flat ([operator overview §6](mcp-gateway-operator-overview.md#6-latency-and-cold-path-work)).
- `tools/list` or gated `tools/call` spikes after deploy, bundle promotion, or catalogue churn.
- SpiceDB QPS scales with agent poll rate × catalogue size instead of relationship writes.
- `mcp_schema_cache_size` or proposed cache gauges approach configured caps.

**When not to use this:**

- First-time cluster bring-up — use [Bootstrap / day-zero](bootstrap-day-zero.md).
- Backend timeouts (`-32102`) or SpiceDB total outage (`-32107`) — see [failure-mode matrix](failure-mode-matrix.md) rows 1 and 8 first.
- Multi-region WAN latency — [ADR 0016](../adr/0016-multi-region-topology.md); this guide assumes single-broker localhost-style tuning methodology from [ADR 0031](../adr/0031-latency-budget-benchmarking.md).

---

## Prerequisites

1. **Gateway Block E caches wired or in canary.** Schema sniff, ZedToken session cache, and list shaping may still be `#[ignore]` in tests; tuning assumes ADR contracts are live on the target build. Confirm with your release notes before treating metrics as authoritative.

2. **Observability path.** Prometheus scrape of gateway metrics (Block G `mcp_*` series per `tests/metric_labels.rs`) or log access to structured invalidation lines ([ADR 0023](../adr/0023-schema-cache-invalidation.md) observability table).

3. **SpiceDB and NATS healthy.** `MCP_GATEWAY_SPICEDB_ENDPOINT` set for gated paths; queue group `mcp-gateway` consuming `{prefix}.gateway.request.>` ([operator overview §9.2](mcp-gateway-operator-overview.md#92-gateway-data-plane)).

4. **Operator tooling:** `kubectl`/`systemctl` (or your supervisor) to rolling-restart gateway pods; optional `agctl` when admin cache stats land ([ADR 0030](../adr/0030-gateway-ctl-cli-surface.md) `cache stats`).

5. **Baseline workload identity.** Record `server_id`, approximate tool count, and whether clients issue `tools/list` before `tools/call` bursts — cold schema and ZedToken paths depend on that ordering ([ADR 0023](../adr/0023-schema-cache-invalidation.md), [ADR 0014](../adr/0014-bulk-check-zedtoken-cache.md)).

---

## Steps

### 1. Map the three caches

Each cache is **in-process per gateway replica** (not NATS KV). Queue-group members do not share entries; invalidation fan-out keeps staleness bounded ([operator overview §3](mcp-gateway-operator-overview.md#3-deployment-topology)).

| Cache | Holds | Populated by | Primary consumer |
|---|---|---|---|
| **Schema** ([ADR 0023](../adr/0023-schema-cache-invalidation.md)) | Canonical `inputSchema` JSON per `(server_id, schema_hash)` | Sniff upstream `tools/list`; miss fetch on `tools/call` | Redaction, validation, audit metadata |
| **ZedToken + bulk results** ([ADR 0014](../adr/0014-bulk-check-zedtoken-cache.md), [bulk-check-permission.md](bulk-check-permission.md)) | Session ZedToken cursor + per-tool allow/deny from `CheckBulkPermissions` | First `tools/list` bulk; warm `tools/call` | Catalog shaping ([ADR 0015](../adr/0015-tools-list-filtering.md)) |
| **Filtered list** ([tools-list-filtering.md §6](tools-list-filtering.md#6-caching-reference)) | Post-filter JSON-RPC `tools/list` body per `ToolsListCacheKey` | After CEL filter + redaction on upstream path | Repeat `tools/list` within TTL / same `policy_version` |

**Cross-cache rule:** `notifications/tools/list_changed` triggers one handler that drops schema rows, ZedToken entries, and filtered-list keys for that `server_id`, then publishes `{prefix}.control.cache.invalidate.{server_id}` so every replica clears local state ([ADR 0023](../adr/0023-schema-cache-invalidation.md) invalidation handler sketch).

---

### 2. Establish a pressure baseline

Pick a 15–30 minute window with representative traffic on the hot `server_id`.

**2.1 Prometheus queries (metric names from ADRs; minting may lag implementation)**

```promql
# Schema cache — ADR 0023
sum(rate(mcp_schema_cache_hits_total{outcome="hit"}[5m]))
  / sum(rate(mcp_schema_cache_hits_total[5m]))
mcp_schema_cache_size

# ZedToken / bulk — ADR 0014 / bulk-check-permission.md §9 (normative names on disk today)
sum(rate(spicedb_cache_hit_total{method="tools/list"}[5m]))
  / (sum(rate(spicedb_cache_hit_total{method="tools/list"}[5m]))
   + sum(rate(spicedb_cache_miss_total{method="tools/list"}[5m])))
histogram_quantile(0.99, sum(rate(spicedb_bulk_check_latency_ms_bucket[5m])) by (le))

# Filtered list — (proposed) until Block G mints mcp_tools_list_cache_* 
# Use audit list_cache_hit or agctl cache stats when available
```

**2.2 CLI snapshot (proposed — ADR 0030)**

```bash
agctl mcp cache stats --output json
# (proposed -- ADR 0030) Expect schema_cache, zed_token_cache, filtered_list_cache sections
```

**2.3 Latency spot-check (warm path)**

```bash
# (proposed -- ADR 0031) E2e added-latency bench; until landed, use production trace:
# gateway span duration p99 for tools/call vs tools/list on same server_id
cargo bench -p trogon-mcp-gateway --bench latency_baseline
```

Record:

| Signal | Healthy warm band (indicative) | Pressure |
|---|---|---|
| Schema hit ratio | High after steady `tools/list` | Low + rising `outcome=miss` / `expired` |
| `mcp_schema_cache_size` | Stable below `MCP_SCHEMA_CACHE_MAX_ENTRIES` | Pinning at cap, churning invalidations |
| ZedToken hit ratio on `tools/list` | High between `list_changed` events | Near zero; bulk RPC every list |
| `spicedb_bulk_check_latency_ms` p99 | Within operator SpiceDB SLO | Scales with catalogue N on every list |
| Filtered-list reuse | `list_cache_hit=true` in audit **(proposed field)** | Every list re-runs full CEL loop |

Target warm added latency ceilings (proposed until baseline run): `tools/call` **redact** row ≤ 15 ms p99 added; `tools/list` **list-shape** ≤ 20 ms p99 added ([ADR 0031](../adr/0031-latency-budget-benchmarking.md)).

---

### 3. Tune the schema cache

Authority: [ADR 0023](../adr/0023-schema-cache-invalidation.md) configuration table. Numbers below are operational guidance, not ADR amendments.

| Knob | Default (ADR) | Effect |
|---|---|---|
| `MCP_SCHEMA_CACHE_MAX_ENTRIES` | `10000` | LRU cap per process; lower to limit RSS, higher to reduce miss storms on diverse catalogues |
| `MCP_SCHEMA_CACHE_TTL_SECS` | `120` | Entry freshness; lower = more sniff/backend fetch, higher = longer stale window if `list_changed` is lost |
| `schema_cache_enabled` | `true` | Master switch; `false` = direct fetch every `tools/call` ([ADR 0023 rollback](../adr/0023-schema-cache-invalidation.md#rollback-plan)) |

**Apply (example — adjust per your supervisor):**

```bash
export MCP_SCHEMA_CACHE_MAX_ENTRIES=15000
export MCP_SCHEMA_CACHE_TTL_SECS=90
# schema_cache_enabled via KV feature_flags or env per ADR 0030 effective config — confirm loader for your build
```

Rolling restart one canary replica; compare `mcp_schema_cache_hits_total` and `tools/call` p99 for 10 minutes before fleet-wide rollout.

**Trade-off matrix (schema)**

| Change | Latency | Freshness | Memory |
|---|---|---|---|
| Raise `MAX_ENTRIES` | Fewer LRU evictions | Unchanged | Higher RSS |
| Lower `TTL_SECS` | More misses / fetches | Safer after silent catalogue drift | Lower average residency |
| Lower `MAX_ENTRIES` | More misses after churn | Unchanged | Lower RSS |
| `schema_cache_enabled=false` | Miss path every call | Strongest (no stale schema cache) | Minimal cache footprint |

---

### 4. Tune the ZedToken and bulk-result cache

Authority: [ADR 0014](../adr/0014-bulk-check-zedtoken-cache.md), [bulk-check-permission.md §5–§6](bulk-check-permission.md#5-cache-value-reference).

| Knob | Default (ADR) | Effect |
|---|---|---|
| `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS` | `5` | Upper bound with `min(zed_expiration, …)`; raise slightly for stable catalogues, lower for regulated tenants |
| `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_MAX_SECS` | (proposed) max `60` | Clamps operator typos |
| Per-tenant LRU cap | `10000` entries **(ADR 0014)** | Eviction drives `spicedb_cache_eviction_total{reason="lru"}` |
| Bundle `spicedb.cache_ttl_secs: 0` | — | Disables cache for that rule (always cold PDP) |

**Apply:**

```bash
export MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS=10
# Restart gateway replicas in the queue group
```

**Symptoms → action**

| Symptom | Likely cause | Tuning |
|---|---|---|
| SpiceDB QPS ≈ agents × lists/min × N tools | TTL too low or constant `list_changed` | Raise TTL only if security review accepts staleness window; fix notification storms |
| `-32107` after policy writes | Stale ZedToken ([failure-mode matrix row 2](failure-mode-matrix.md#failure-mode-matrix)) | Shorten TTL or rely on invalidation; do not disable fail-closed |
| Hit ratio high but wrong tools visible | Stale bulk **allow** within TTL | Lower TTL; verify `list_changed` invalidation wiring |

Metrics: use `spicedb_cache_hit_total` / `spicedb_cache_miss_total` / `spicedb_bulk_check_latency_ms` per [bulk-check-permission.md §9](bulk-check-permission.md#9-metrics-reference). Prefix `mcp_bulk_check_*` is **(proposed)** until Block G converges naming with `tests/metric_labels.rs` ([ADR 0031 open question #1](../adr/0031-latency-budget-benchmarking.md#open-questions)).

---

### 5. Tune the filtered-list cache

Authority: [tools-list-filtering.md §6](tools-list-filtering.md#6-caching-reference), [§9.1](tools-list-filtering.md#91-per-tool-ttl-on-the-filtered-list-cache).

There is **no** separate env var on disk today. Provisional TTL **120 s** aligns with schema cache default; primary invalidation is `policy_version` bump and `list_changed` (not TTL alone).

| Knob | Source | Effect |
|---|---|---|
| `policy_version` / bundle promotion | KV `mcp-gateway-config` | Invalidates all list keys for new revision |
| `schema_hash` in `ToolsListCacheKey` | Sniffed catalogue bytes | New backend catalogue → miss |
| Provisional list TTL | §9.1 **120 s** | Safety net if notifications are lost |
| Catalogue size N | Backend | Larger N increases miss cost even with filtered-list hit (first list still runs bulk + CEL) |

**Apply:** Prefer invalidation hygiene over long TTL:

```bash
# After bundle promotion, confirm policy_version in audit matches KV pointer
nats kv get mcp-gateway-config bundle/active

# (proposed) Force peer invalidation if a replica serves stale shaped lists
nats pub "mcp.control.cache.invalidate.github" \
  '{"server_id":"github","reason":"operator_flush","emitted_at":"'"$(date -u +%Y-%m-%dT%H:%M:%SZ)"'"}'
# Payload shape proposed -- ADR 0023
```

Metrics `mcp_tools_list_cache_hits_total` / `mcp_tools_list_cache_size` are **(proposed)**; until minted, use `catalog_tools_filtered` / `list_cache_hit` on audit ([tools-list-filtering.md §8](tools-list-filtering.md#8-observability-reference)) or `agctl mcp cache stats` **(proposed -- ADR 0030)**.

---

### 6. Re-verify under load

Repeat Step 2 after changes. Compare:

- Warm `list-shape` and `redact` added latency vs [ADR 0031](../adr/0031-latency-budget-benchmarking.md) proposed ceilings.
- Schema hit ratio after a controlled `tools/list` burst on the hot server.
- SpiceDB bulk RPC rate (should drop when ZedToken hits rise).

If memory RSS grows without hit-ratio gain, **lower** caps before raising TTL.

---

## Verify

Short checklist after tuning:

- [ ] `mcp_schema_cache_hits_total{outcome="hit"}` dominates on repeated `tools/call` to the same tool after a warming `tools/list`.
- [ ] `mcp_schema_cache_size` stays below `MCP_SCHEMA_CACHE_MAX_ENTRIES` without sustained `singleflight` wait growth **(metric per ADR 0023)**.
- [ ] `spicedb_cache_hit_total` rises on repeat `tools/list` for the same `mcp-session-id` within TTL ([tests/bulk_check_zedtoken_cache.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/bulk_check_zedtoken_cache.rs)).
- [ ] Publishing `notifications/tools/list_changed` (or control invalidate) drops all three caches for `server_id` — next list invokes bulk check, next call may schema-miss until re-sniff ([tests/schema_cache_invalidation.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs)).
- [ ] Audit on shaped `tools/list` shows `catalog_tools_filtered` consistent with policy; no spike in `-32100` on tools that remain listed.
- [ ] Gateway-added p99 within operator budget ([ADR 0031](../adr/0031-latency-budget-benchmarking.md)); backend latency unchanged.

---

## Rollback / undo

| Lever | Effect | When |
|---|---|---|
| `schema_cache_enabled=false` | Schema direct-fetch; eliminates stale-schema class ([ADR 0023 rollback](../adr/0023-schema-cache-invalidation.md#rollback-plan)) | Suspected wrong redaction/schema |
| Restore prior `MCP_SCHEMA_CACHE_*` env | Previous LRU/TTL behavior | OOM or miss storm after cap change |
| Lower `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS` to `5` or bundle TTL `0` | Fresher PDP; higher SpiceDB load | Stale allows after revoke |
| `{prefix}.control.cache.invalidate.{server_id}` | Peer-wide flush without disabling caches | Post-incident hygiene **(proposed payload)** |
| Revert bundle / `policy_version` | Clears filtered-list keys | Bad list-filter program |

Canonical failure semantics: [failure-mode matrix](failure-mode-matrix.md) rows 1–2 (SpiceDB / ZedToken). Rolling restart clears **all** in-process caches — expect cold-path latency spike until clients re-issue `tools/list`.

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| Hit ratio near zero after deploy | Cold replicas; no warming `tools/list` | Canary warmup: one `tools/list` + representative `tools/call` per `server_id` ([operator overview §6.2](mcp-gateway-operator-overview.md#62-cold-path-work-first-request-cache-miss-or-periodic-refresh)) |
| `mcp_schema_cache_size` at cap, miss storm | Catalogue has many distinct `schema_hash` values | Raise `MCP_SCHEMA_CACHE_MAX_ENTRIES` or reduce catalogue diversity; check LRU eviction rate |
| Every `tools/list` costs one bulk RPC | ZedToken TTL too short or `list_changed` spam | Inspect notification rate; tune `MCP_GATEWAY_SPICEDB_ZEDTOKEN_CACHE_TTL_SECS`; fix backend churn |
| Shaped list stale on one replica only | Queue-group cache divergence | Publish `mcp.control.cache.invalidate.{server_id}`; shorten filtered-list provisional TTL |
| `tools/call` latency up, schema hits high, SpiceDB flat | Schema OK; ZedToken miss on call | Ensure prior list bulk included tool; check session id header continuity |
| Added latency over ADR 0031 ceiling with flat hit ratios | CPU-bound CEL/redaction, not cache | Scale gateway replicas; profile `list-shape` workload — cache knobs will not help |

---

## Related

| Document | Relevance |
|---|---|
| [ADR 0023: Schema cache](../adr/0023-schema-cache-invalidation.md) | Schema knobs, metrics, invalidation, `schema_cache_enabled` |
| [ADR 0014: BulkCheck + ZedToken cache](../adr/0014-bulk-check-zedtoken-cache.md) | TTL, LRU cap, `list_changed` invalidation |
| [ADR 0015: Tools/list filtering](../adr/0015-tools-list-filtering.md) | Filtered-list key shape, audit fields |
| [ADR 0031: Latency budget](../adr/0031-latency-budget-benchmarking.md) | Warm p99 targets, `list-shape` / `redact` workloads |
| [bulk-check-permission.md](bulk-check-permission.md) | Cache keys, `spicedb_*` metrics |
| [tools-list-filtering.md](tools-list-filtering.md) | `ToolsListCacheKey`, §6.3 invalidation |
| [failure-mode-matrix.md](failure-mode-matrix.md) | ZedToken stale, SpiceDB unreachable |
| [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) | Deployment topology, cold path, control invalidate subject |
| [How to integrate a third-party MCP server](howto-integrate-third-party-mcp.md) | When a new `server_id` enters the mesh |
| [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) | Block E cache contracts and test scaffolds |

---

*Document type: Diátaxis **how-to**. For cache design rationale, read the ADRs above. For NATS subject grammar and mesh tokens, start at [overview.md](overview.md).*

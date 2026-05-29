# ADR 0031: Latency Budget and Benchmarking Methodology

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block G item 1 (latency baseline: P50/P99 added by the gateway vs direct `mcp-nats`); unblocks Phase 2+ operator SLO dashboards, CI regression gates, and numeric tightening of hot-path budgets in [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) §6.1 |
| **Related** | [ADR 0014](0014-bulk-check-zedtoken-cache.md) (ZedToken cache — warm/cold PDP paths); [ADR 0015](0015-tools-list-filtering.md) (`list-shape` workload); [ADR 0023](0023-schema-cache-invalidation.md) (schema cache — warm/cold on `tools/call`); [ADR 0016](0016-multi-region-topology.md) (multi-region WAN latency **out of scope** for this ADR); [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) §6; [otel-wiring.md](../identity/otel-wiring.md) §8 (metric cardinality); `rsworkspace/crates/trogon-mcp-gateway/tests/e2e_nats_forward.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/metric_labels.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/bulk_check_zedtoken_cache.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/tools_list_filter.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/redaction_rules.rs`; `rsworkspace/crates/trogon-mcp-gateway/benches/latency_baseline.rs` |

## Context

Phase 1 shipped without measured P50/P99 baselines. `MCP_GATEWAY_PLAN.md` Block G item 1 defers the numeric baseline until Phase 2+ policy work (CEL shaping, SpiceDB catalog filtering, schema cache, redaction) lands and operators need a shared definition of "good" before tightening fleet SLOs.

Without a written budget and methodology:

| Gap | Impact |
|-----|--------|
| No agreed **added-latency** metric | Teams optimize end-to-end RTT (dominated by backend tool execution) and miss gateway regressions. |
| No **direct `mcp-nats` baseline** | Comparisons across docs use incompatible harnesses (unit tests vs live NATS vs production traces). |
| ADR 0014 / 0023 cache wins unmeasured | ZedToken and schema cache reduce p99 in design docs but lack acceptance thresholds. |
| Block G metrics scaffold untested | `tests/metric_labels.rs` pins `mcp_*` series and histogram buckets with no link to bench artifacts. |
| No regression gate | Micro-bench file `benches/latency_baseline.rs` exists (Criterion, in-memory fakes) but e2e added-latency is undefined. |

**Operator design targets already on paper (not yet measured on this branch):**

- [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) §6.1: per-request gateway overhead target **under ~10 ms** for typical hot-path steps (JWT, CEL, cached SpiceDB, subject rewrite).
- [ADR 0016](0016-multi-region-topology.md): gateway-added policy work **p95 &lt; 5 ms** without cross-WAN hops on the hot path (regional topology only).

This ADR defines **how** to measure added latency, **what workloads** to run, **where** artifacts live, and **when** CI blocks — without committing final microsecond ceilings until Block G implementation records the first baseline run. Numeric SLO cells marked **(proposed)** below are placeholders gated on that run.

**Current branch snapshot:**

- `benches/latency_baseline.rs` — Criterion micro-benches (JWT verify, subject rewrite, `AllowAllPermissionChecker`); **no NATS, no SpiceDB**.
- `tests/e2e_nats_forward.rs` — live NATS harness for gateway forward (`tools/list`); pattern to reuse for e2e benches.
- Integration scaffolds for full-policy paths (`bulk_check_zedtoken_cache.rs`, `schema_cache_invalidation.rs`, `tools_list_filter.rs`, `redaction_rules.rs`) are `#[ignore]` until Block E wiring lands.
- No e2e latency bench binary or committed baseline JSON artifact on disk today.

Multi-region WAN latency, cross-cluster mirror lag, and supercluster RTT are explicitly **out of scope** — [ADR 0016](0016-multi-region-topology.md) owns regional topology SLOs. Benchmarks run against a **single local NATS** broker (`NATS_URL`, default `nats://127.0.0.1:4222`) on the same host as the gateway under test.

---

## Decision

Adopt **added latency vs direct `mcp-nats`** as the primary SLO metric, five named **workloads**, a **two-tier measurement stack** (Criterion micro + HDR Histogram e2e), JSON benchmark artifacts under `rsworkspace/crates/trogon-mcp-gateway/benches/` **(proposed layout)**, and a **15% p99 regression gate** **(proposed)** on CI-safe workloads. Full-policy workloads run on nightly / manual SpiceDB fixtures. Production `mcp_*` / `mcp_gateway_*` Prometheus histograms are observability inputs, not the regression source of truth.

### Added latency (normative definition)

**Added latency** for one request:

```text
T_added = T_gateway - T_direct
```

| Term | Measurement |
|------|-------------|
| `T_gateway` | Wall time from client `request_with_headers` on `{prefix}.gateway.request.{server_id}.{method_lane}` until JSON-RPC reply payload received (same client connection as harness). |
| `T_direct` | Wall time from client `request_with_headers` on `{prefix}.server.{server_id}.{method_lane}` until JSON-RPC reply received — **same payload shape, same backend stub, no gateway process in path**. |

Both paths use the **`mcp-nats` subject grammar** and client API (`mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsConfig`, `request_with_headers`) as exercised in `tests/e2e_nats_forward.rs`. The gateway path additionally runs `trogon_mcp_gateway::run` with configured `GatewaySettings`, policy bundle, and authz checker per workload.

**Direct `mcp-nats` baseline** means: client publishes to the **backend server lane** (`*.server.{server_id}.*`) with a backend subscriber that echoes a fixed JSON-RPC success body. It isolates gateway policy work; NATS RTT and JSON parse cost appear in **both** arms and cancel in the delta only if backend stub behavior is identical.

Report **p50** and **p99** of `T_added` per workload. Optional p95/p999 in artifacts; gates use **p99** unless noted.

### Hot-path SLO budget (proposed ceilings — gated on first baseline run)

Ceilings apply to **warm** added latency on **localhost NATS**, **JWT validation off or warm JWKS**, backend stub replying in &lt; 1 ms. Values are **(proposed)** until Block G item 1 records measured baselines on the reference runner profile **`linux-amd64-nats-local`** **(proposed)**.

| Workload | Method | Warm p50 added | Warm p99 added | Notes |
|----------|--------|----------------|----------------|-------|
| **shadow** | `tools/call` | ≤ 2 ms **(proposed)** | ≤ 8 ms **(proposed)** | Empty policy — see workloads table |
| **permit** | `tools/call` | ≤ 3 ms **(proposed)** | ≤ 10 ms **(proposed)** | CEL allow-all; no SpiceDB |
| **lookup** | `tools/call` | ≤ 4 ms **(proposed)** | ≤ 12 ms **(proposed)** | One warm SpiceDB check (cached ZedToken per ADR 0014) |
| **redact** | `tools/call` | ≤ 5 ms **(proposed)** | ≤ 15 ms **(proposed)** | Full policy: CEL + warm SpiceDB + 4-field schema redaction |
| **list-shape** | `tools/list` | ≤ 6 ms **(proposed)** | ≤ 20 ms **(proposed)** | Catalogue filter + one bulk PDP (ADR 0014/0015); catalogue size **50 tools (proposed fixture)** |

**Full-policy `tools/call`** for operator communication maps to the **redact** row (CEL + SpiceDB + redaction). **Empty-policy** maps to **shadow**.

**Cold-path reporting (separate track, not PR-gated):**

| Cold event | Workloads affected | Reporting |
|------------|-------------------|-----------|
| First request after `trogon_mcp_gateway::run` spawn | All | Tag `cache_state=cold_start` **(proposed)** |
| First SpiceDB check after process start or `zedtoken_cache` flush | `lookup`, `redact`, `list-shape` | Tag `cache_state=spicedb_cold` **(proposed)** |
| First schema resolve after `tools/list` sniff miss | `redact` | Tag `cache_state=schema_cold` **(proposed)** per ADR 0023 |
| JWKS fetch window | Any JWT-on workload | Exclude samples where JWKS refresh span fires **(proposed)** |

Warm and cold **must both** be exercised in acceptance runs for ADR 0014 and ADR 0023; only **warm** samples feed the PR regression gate.

### Benchmark workloads (normative)

| Workload id | JSON-RPC | Policy / authz | Backend stub | Exercises |
|-------------|----------|----------------|--------------|-----------|
| **shadow** | `tools/call` | `JwtValidator::disabled()`; `AllowAllPermissionChecker`; no bundle / Tier-0 forward | Fixed 4-field success result **(proposed fixture)** | Pure forward overhead |
| **permit** | `tools/call` | Bundle: single CEL rule always `true`; no SpiceDB host calls | Same stub | CEL eval on hot path |
| **lookup** | `tools/call` | Gated method; **one** `CheckBulkPermissions` item; SpiceDB test double or local SpiceDB | Same stub | ADR 0014 warm ZedToken path |
| **redact** | `tools/call` | CEL permit + SpiceDB allow + ruleset on **4 fields** **(proposed)**: `$.params.token`, `$.params.note`, `$.result.secret`, `$.result.meta` per `tests/redaction_rules.rs` | Stub returns matching 4-field body | Schema cache warm + redaction |
| **list-shape** | `tools/list` | ADR 0015 per-item filter + ADR 0014 bulk check; **50-tool** catalogue **(proposed)**; half denied | Static `tools/list` JSON | List shaping p99 |

**Fixture invariants:**

- Payload sizes fixed per workload (document byte length in artifact metadata).
- Backend subscriber uses the same `McpPrefix` token and queue discipline as `e2e_nats_forward.rs` (`Uuid::now_v7()` prefix segment, isolated queue group).
- SpiceDB and schema state **pre-warmed** for warm runs: one throwaway `tools/list` + one throwaway gated `tools/call` before measurement window, unless workload tag is `*_cold`.

### Measurement methodology

Two tiers — different tools by design:

| Tier | Scope | Tool | Rationale |
|------|-------|------|-----------|
| **Micro** | In-process: JWT, subject rewrite, CEL, redaction CPU on JSON fixture | **Criterion** (`benches/latency_baseline.rs`, existing) | Stable, fast, no broker; catches algorithm regressions |
| **E2e added-latency** | Live NATS + gateway task + backend stub | **HDR Histogram** via `hdrhistogram` crate **(proposed dev-dependency)** + async Tokio driver **(proposed)** — **not** Criterion | Criterion loops fight async setup/teardown and shared broker state; HDR records streaming samples with stable p99 |

**E2e procedure (normative):**

1. Start NATS (external or test container); single broker, no leafnodes.
2. Spawn backend echo subscriber on `{prefix}.server.{server_id}.{lane}`.
3. Spawn gateway via `trogon_mcp_gateway::run` with workload-specific settings.
4. **Warmup:** discard **50** **(proposed)** paired `(T_gateway, T_direct)` samples per workload.
5. **Record:** **500** **(proposed)** paired samples; compute `T_added` each pair; record into HDR Histogram (3 significant digits **(proposed)**).
6. **Cold-start exclusion for warm track:** omit first **10** samples after gateway spawn from recorded window; omit samples flagged by optional tracing hooks for JWKS/PDP cold events.
7. Shutdown gateway; persist artifact (below).

**Pairing:** For sample *i*, run direct and gateway requests sequentially in the same iteration (direct first **(proposed)**) to reduce broker jitter correlation; alternate order every 100 samples **(proposed)** to detect ordering bias.

**Reproducibility:** Document `NATS_URL`, CPU governor state **(proposed: performance mode)**, Rust toolchain from `rust-toolchain.toml`, and `git rev-parse HEAD` in artifact. Third party with repo + local NATS reruns:

```bash
cargo bench -p trogon-mcp-gateway --bench e2e_latency -- --ignored
```

**(proposed bench name and flags)** — until landed, use `cargo test -p trogon-mcp-gateway -- --ignored` on the dedicated module.

Micro tier:

```bash
cargo bench -p trogon-mcp-gateway --bench latency_baseline
```

### Reporting and artifact layout

| Path | Purpose |
|------|---------|
| `rsworkspace/crates/trogon-mcp-gateway/benches/latency_baseline.rs` | Existing Criterion micro suite |
| `rsworkspace/crates/trogon-mcp-gateway/benches/e2e_latency.rs` **(proposed)** | E2e added-latency driver + workload matrix |
| `rsworkspace/crates/trogon-mcp-gateway/benches/fixtures/` **(proposed)** | Static JSON-RPC bodies and bundle snippets |
| `.trogonai/benchmarks/gateway/` **(proposed)** | Committed or CI-uploaded JSON baselines per branch |

**Artifact schema** `trogon.mcp.benchmark.gateway/v1` **(proposed)**:

```json
{
  "schema": "trogon.mcp.benchmark.gateway/v1",
  "git_sha": "...",
  "recorded_at": "2026-05-29T12:00:00Z",
  "runner_profile": "linux-amd64-nats-local",
  "nats_url": "nats://127.0.0.1:4222",
  "workloads": {
    "shadow": {
      "cache_state": "warm",
      "added_latency_ms": { "p50": 0.0, "p99": 0.0, "samples": 500 },
      "payload_bytes": 0
    }
  }
}
```

Replace `0.0` placeholders after first baseline run. Store `main` branch artifact as **`baseline.json`** **(proposed)** for regression diff.

Human-readable summary emitted to stderr after each run (table of workloads × p50/p99 added).

### Regression gate cadence

| Gate | Trigger | Workloads | Threshold | Action |
|------|---------|-----------|-----------|--------|
| **PR required** | Pull request touching `trogon-mcp-gateway/**` | `shadow`, `permit` | p99 added latency **&gt; 15%** **(proposed)** vs `main` `baseline.json` on same runner profile | Fail check; no merge until fix or intentional baseline bump PR |
| **Nightly** | Scheduled CI | All five workloads, warm + cold tags | Soft alert **&gt; 15%** p99; hard alert **&gt; 25%** **(proposed)** | Notify `#mcp-gateway-perf` **(proposed)**; file issue if hard |
| **Release** | Tag `trogon-mcp-gateway-v*` | All warm workloads | Must meet **(proposed)** SLO table or documented exception in release notes | Blocker for Phase 2+ budget sign-off |

**Baseline bump process:** intentional regression requires a PR that updates `baseline.json` with decider sign-off in commit message body (exception to conventional-commit no-body rule for baseline bumps only).

Micro Criterion benches: **warn-only** on PR **(proposed)** until noise profile stable; e2e added-latency is the authoritative gate.

SpiceDB-dependent workloads (`lookup`, `redact`, `list-shape`) are **not** required on default PR runners without SpiceDB; nightly supplies the test double or embedded SpiceDB **(proposed — Open question)**.

### Prometheus metric cardinality budget (cross-reference)

Benchmarks and production SLOs share label discipline but **different metric names** until Block G converges naming:

| Source | Metric prefix | Canonical for |
|--------|---------------|---------------|
| `tests/metric_labels.rs` | `mcp_*` (`mcp_request_duration_seconds`, etc.) | Block G acceptance tests / pinned histogram buckets |
| [otel-wiring.md](../identity/otel-wiring.md) §4 | `mcp_gateway_*` | OTel export catalog |

**Cardinality budget (normative for gateway-emitted request metrics):**

- **Allowed label keys on request counters/histograms:** `tenant`, `method_root` (or `method` per otel-wiring — reconcile in implementation), closed `decision` / `outcome` enums only — per `tests/metric_labels.rs` and [otel-wiring.md §8.2](../identity/otel-wiring.md#82-allowed-metric-label-keys-exhaustive-v1).
- **Forbidden on `mcp_*` / `mcp_gateway_*` request series:** `tool`, `tool_name`, `server_id`, `session_id`, `agent_id`, `subject`, `request_id`, `trace_id` — per `tests/metric_labels.rs` `cardinality` module and otel-wiring §8.
- **Estimated upper bound (50 tenants, 15 method roots, 3 outcomes):** ~2,250 counter series + ~750 histograms + authz decision series — per otel-wiring §8.4. Benchmark artifacts **must not** introduce per-tool or per-session Prometheus labels to capture workload detail; use workload id in JSON artifacts only.
- **Histogram buckets:** `mcp_request_duration_seconds` uses pinned `BUCKET_UPPER_BOUNDS` in `metric_labels.rs` (500 µs … 10 s). Production p99 SLO burn uses these buckets; bench JSON reports milliseconds separately — do not conflate without conversion.

`mcp_policy_rule_fired_total{rule_id}` is bounded by bundle rule count (operator-controlled, typically &lt; 100 rules/bundle); not part of added-latency gate.

---

## Consequences

### Positive

- **Comparable baselines:** Added latency vs direct `mcp-nats` isolates gateway work from backend tool execution and WAN RTT.
- **Cache-aware acceptance:** Warm and cold tracks make ADR 0014 / 0023 performance claims testable before operators rely on them.
- **Layered regression detection:** Criterion catches CPU regressions cheaply; e2e HDR catches integration regressions (NATS subscribe, gateway task, policy wiring).
- **CI proportionality:** PR gate limited to SpiceDB-free workloads keeps contributor friction low; full matrix runs nightly.
- **Cardinality safety:** Bench artifacts stay JSON-side; Prometheus label rules from `metric_labels.rs` / otel-wiring remain enforceable.

### Negative

- **Local NATS only:** Baselines exclude supercluster, TLS, and auth-callout overhead — production p99 will be higher.
  - **Mitigation:** Document runner profile; optional `tls_edge` workload in nightly using `tests/tls_edge.rs` harness **(proposed)**; multi-region remains ADR 0016.
- **Dual metric naming (`mcp_*` vs `mcp_gateway_*`)** confuses dashboard wiring until Block G lands.
  - **Mitigation:** Open question to pick one prefix; tests and otel doc update in same PR.
- **Proposed SLO numbers may change** after first measurement — early releases could churn baselines.
  - **Mitigation:** Explicit `(proposed)` markers; first baseline PR only sets `baseline.json`, not fleet alerts.
- **HDR + async harness maintenance cost** vs pure Criterion.
  - **Mitigation:** Reuse `e2e_nats_forward.rs` types; single `e2e_latency.rs` driver shared by CI and local dev.
- **15% p99 gate noise** on shared CI runners.
  - **Mitigation:** Pin runner profile; rerun on failure; require 2 consecutive passes for baseline bump merges.

### Neutral

- Production traces (`mcp_gateway.handle_ingress` span duration) complement but do not replace bench artifacts — different sampling and backend mix.
- `trogon-sts-probe` P99 threshold (40 ms) remains STS-owned; gateway egress mint cache miss adds separate span — not included in `tools/call` workloads unless egress mint enabled in fixture.
- Audit and anomaly publish paths stay out of added-latency measurement (async side channel per operator overview §6.3).

---

## Rejected alternatives

### End-to-end RTT as the only SLO (no direct baseline)

Measure client-to-client latency through gateway only; skip direct `mcp-nats` arm.

| Assessment | |
|------------|---|
| **Pros** | Single measurement; matches naive operator intuition. |
| **Cons** | Backend stub changes and NATS jitter mask gateway regressions; incompatible with Block G item 1 wording ("vs direct `mcp-nats`"). |
| **Verdict** | **Rejected.** Added latency delta is the primary metric; end-to-end RTT optional in artifact metadata only. |

### Criterion for e2e NATS benchmarks

Run the full gateway+NATS matrix inside `criterion_group!` like micro-benches.

| Assessment | |
|------------|---|
| **Pros** | One bench framework; familiar HTML reports. |
| **Cons** | Async broker lifecycle, shared gateway task, and 500+ iteration broker load produce high variance; Criterion outlier handling hides paired-sample correlation. |
| **Verdict** | **Rejected** for e2e tier. **Accepted** for micro tier only (`latency_baseline.rs`). |

### Production Prometheus histograms as regression gate

Compare `mcp_request_duration_seconds` or `mcp_gateway_request_duration_ms` scrape deltas in CI.

| Assessment | |
|------------|---|
| **Pros** | Exercises real export path and bucket boundaries. |
| **Cons** | CI lacks representative tenant/method mix; scrape timing noise; conflates backend slowness with gateway added work unless direct baseline arm exists. |
| **Verdict** | **Rejected** as gate source. **Accepted** as post-deploy SLO burn (otel-wiring §8.6 alerting hints). |

### No PR regression gate (manual baseline only)

Publish methodology; operators run benches ad hoc.

| Assessment | |
|------------|---|
| **Pros** | Zero CI flake; no runner pinning. |
| **Cons** | Regressions merge silently; Phase 2+ cache/policy work high regression risk. |
| **Verdict** | **Rejected.** 15% p99 gate on `shadow` + `permit` is required once `e2e_latency` **(proposed)** lands. |

---

## Open questions

1. **Metric prefix convergence:** Block G implementation must pick `mcp_*` (test scaffold) or `mcp_gateway_*` (otel-wiring) and update the other doc + tests in one change set.
2. **SpiceDB in nightly CI:** Embedded SpiceDB container vs in-process test double vs mocked gRPC — affects `lookup` / `redact` / `list-shape` fidelity.
3. **Reference runner profile:** Pin GitHub-hosted label vs self-hosted bare metal for `baseline.json` — variance trade-off.
4. **Catalogue size for `list-shape`:** 50 vs 200 tools changes bulk PDP payload; pick one fixture size for baseline comparability.
5. **JWT-on shadow workload:** Add optional `shadow+jwt` workload for HS256 verify path or keep JWT in micro-bench only.
6. **WASM / Tier-3 policy in bench matrix:** Deferred until Block F WASM lands; may add `wasm-permit` workload in follow-on ADR.
7. **Final numeric SLO cells:** Replace **(proposed)** ms ceilings after first green baseline on reference runner — owner: platform / gateway maintainers.

---

## Rollback plan

| Control | Behavior |
|---------|----------|
| **`MCP_GATEWAY_BENCH_REGRESSION=0`** **(proposed env)** | CI skips p99 compare; still runs benches in warn-only mode. |
| **Remove gate job** | Delete or disable workflow step referencing `baseline.json`; methodology doc remains valid for manual runs. |
| **Revert baseline file** | Restore prior `baseline.json` on main if erroneous bump merged — no runtime gateway behavior change (paper + CI only). |
| **Disable e2e bench binary** | Cargo feature `bench-e2e` **(proposed)** default off in `--all-features` dev builds if NATS unavailable breaks default `cargo test`. |

Rollback of this ADR does **not** change gateway runtime latency — only measurement and merge policy.

---

## Implementation notes

### Code surfaces (when implementation lands)

| Surface | Responsibility |
|---------|----------------|
| `rsworkspace/crates/trogon-mcp-gateway/benches/e2e_latency.rs` **(proposed)** | Workload matrix, paired sampling, HDR recording, JSON artifact write |
| `rsworkspace/crates/trogon-mcp-gateway/benches/fixtures/` **(proposed)** | Static JSON-RPC + bundle YAML for five workloads |
| `rsworkspace/crates/trogon-mcp-gateway/benches/latency_baseline.rs` | Extend with CEL/redaction micro functions when modules stabilize |
| `rsworkspace/crates/trogon-mcp-gateway/tests/e2e_nats_forward.rs` | Extract shared harness helpers **(proposed)** for prefix spawn, backend subscriber, gateway lifecycle |
| `rsworkspace/crates/trogon-mcp-gateway/tests/bulk_check_zedtoken_cache.rs` | Warm/cold PDP assertions feed `lookup` / `list-shape` acceptance |
| `rsworkspace/crates/trogon-mcp-gateway/tests/schema_cache_invalidation.rs` | Warm/cold schema path for `redact` acceptance |
| `rsworkspace/crates/trogon-mcp-gateway/tests/tools_list_filter.rs` | `list-shape` catalogue assertions |
| `rsworkspace/crates/trogon-mcp-gateway/tests/redaction_rules.rs` | 4-field redaction fixture contract |
| `rsworkspace/crates/trogon-mcp-gateway/tests/metric_labels.rs` | Cardinality + bucket stability after bench runs populate metrics |
| `.github/workflows/` **(proposed)** | PR job: NATS service + `shadow`/`permit` gate; nightly full matrix |
| `.trogonai/benchmarks/gateway/baseline.json` **(proposed)** | Committed main-branch reference |

### Acceptance mapping

| Workload | Primary test scaffolds |
|----------|------------------------|
| shadow | `e2e_nats_forward.rs` (forward path) |
| permit | `policy_eval.rs`, `cel_authz_gate.rs` |
| lookup | `bulk_check_zedtoken_cache.rs` (cold + warm modules) |
| redact | `redaction_rules.rs`, `schema_cache_invalidation.rs` |
| list-shape | `tools_list_filter.rs`, `bulk_check_zedtoken_cache.rs` |

### First baseline run checklist (Block G item 1)

1. Implement `e2e_latency` **(proposed)** driver reusing `e2e_nats_forward.rs` harness types.
2. Run warm matrix locally; fill `baseline.json` with measured p50/p99 (replace **(proposed)** SLO cells if data supports tighter bounds).
3. Wire PR gate on `shadow` + `permit` only.
4. Document runner profile in crate `README` **(proposed)** or Block H operator doc — not in this ADR.

---

## Status of supporting work

| Item | Status |
|------|--------|
| ADR 0031 (this document) | **Done** |
| Criterion micro benches | **Done** (`latency_baseline.rs`) |
| E2e added-latency bench | **Pending** |
| `baseline.json` on main | **Pending** |
| PR regression workflow | **Pending** |
| Numeric SLO ceilings (non-proposed) | **Pending** first baseline |

---

*Contract sources: [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block G item 1; [mcp-gateway-operator-overview.md](../identity/mcp-gateway-operator-overview.md) §6; [otel-wiring.md](../identity/otel-wiring.md) §4, §8; [ADR 0014](0014-bulk-check-zedtoken-cache.md); [ADR 0015](0015-tools-list-filtering.md); [ADR 0023](0023-schema-cache-invalidation.md); [ADR 0016](0016-multi-region-topology.md); `rsworkspace/crates/trogon-mcp-gateway/tests/e2e_nats_forward.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/metric_labels.rs`; `rsworkspace/crates/trogon-mcp-gateway/benches/latency_baseline.rs`; `rsworkspace/crates/trogon-mcp-gateway/Cargo.toml` (`[[bench]] latency_baseline`, `criterion` dev-dependency).*

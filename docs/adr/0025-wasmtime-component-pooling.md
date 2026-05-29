# ADR 0025: Wasmtime Component Pooling Strategy

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block F item 2 (Wasmtime integration with component pooling per bundle version); unblocks Block F item 3 (tracing across WASM boundary), Block F item 5 (bundle loader warm path), Block F item 6 (first-party `mcp-pack` WASM components), and CEL `wasm.call(...)` **(proposed)** wiring in Block E |
| **Related** | [ADR 0010](0010-bundle-format.md), [ADR 0011](0011-nats-auth-callout.md), [ADR 0024](0024-failure-mode-matrix.md), [ADR 0026](0026-bundle-hot-swap.md) **(proposed)**, [reference-host-abi.md](../identity/reference-host-abi.md), [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md), [reference-error-codes.md](../identity/reference-error-codes.md), [wasm-bundle-format.md](../identity/wasm-bundle-format.md), `rsworkspace/crates/trogon-mcp-gateway/tests/wasm_host_abi.rs`, `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs` |

## Context

Phase 3 of `trogon-mcp-gateway` loads **signed policy bundles** ([ADR 0010](0010-bundle-format.md)) containing one or more WebAssembly **Component Model** artifacts under `components/*.wasm`, linked against WIT world `policy-bundle` at `trogon:mcp-policy@0.1.0` pinned to **WASI 0.3** ([mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md), `MCP_GATEWAY_PLAN.md` Block F item 1).

Every request that reaches Tier 3 policy evaluation — whether via the guest export `evaluate` on the authoritative WASM tier or via the CEL host builtin `wasm.call(component_id, input_json)` **(proposed)** — needs a **linked, initialized component instance**. Wasmtime component **instantiation and `init()`** on the hot path would dominate P50 latency; `MCP_GATEWAY_PLAN.md` § Discipline and Block F item 2 explicitly require **component pooling per bundle version**.

Without a written pooling strategy, implementation will fork on incompatible axes:

| Axis | Risk if undecided |
|------|-------------------|
| Lifecycle | Per-request instantiate vs pool checkout — order-of-magnitude latency spread |
| Pool identity | Wrong key causes cross-tenant or cross-version instance reuse |
| Concurrency | Shared single instance serializes the queue group; per-worker sticky instances waste memory |
| Hot-swap | Old digest instances leak after bundle promotion ([ADR 0010](0010-bundle-format.md) compile/pool/activate) |
| Resource limits | Unbounded pools × 16 MiB linear memory exhaust gateway workers |
| Failures | Trap vs fuel vs OOM mapped inconsistently to JSON-RPC ([ADR 0024](0024-failure-mode-matrix.md) row 3) |

**Current branch snapshot:**

- No Wasmtime code in `trogon-mcp-gateway` yet; workspace pins `wasmtime = "=45.0.0"` in `rsworkspace/Cargo.toml` (current wasmtime release at time of writing — ADR does not pin a semver for Phase 3 implementation beyond "current workspace pin").
- Acceptance scaffolds exist: `tests/wasm_host_abi.rs` (host imports, traps → `-32101`), `tests/bundle_load_hot_reload.rs` (in-flight drain on prior digest).
- Resource and error defaults are sketched in [mcp-policy-wit-sketch.md § Resource limits](../identity/mcp-policy-wit-sketch.md#resource-limits) and [wasm-bundle-format.md §8](../identity/wasm-bundle-format.md#8-loading); this ADR makes them **normative for pooling**.

**Relationship to bundle hot-swap:** Pool activation, drain, and eviction semantics align with [ADR 0026](0026-bundle-hot-swap.md) **(proposed)** (this wave). ADR 0025 owns **instance pool mechanics**; ADR 0026 owns **control-plane pointer flip and promotion workflow**.

---

## Decision

**Adopt a pre-warmed, checkout-based component pool** backed by a **process-wide shared `Engine`**, with **lazy expansion** up to configured caps, **one instance per concurrent evaluation** (no concurrent sharing of a single instance), and **digest-scoped eviction** after hot-swap drain.

### 1. Lifecycle model

| Phase | When | Behavior |
|-------|------|----------|
| **Compile** | After bundle verify, before activation | Wasmtime compiles each `components/*.wasm` to a cached `Component` keyed by pool key (§2). No guest execution. |
| **Pre-warm** | On activation (or prefetch — [wasm-bundle-format.md §8.4](../identity/wasm-bundle-format.md#84-pre-fetch-and-promotion-)) | Create `MCP_WASM_POOL_PREWARM` **(proposed)** instances (default **4**) per pool key; link; call guest `init()` once each. Failed `init()` discards that slot; if zero instances succeed, bundle activation fails. |
| **Checkout** | Per CEL `wasm.call(...)` **(proposed)** or Tier-3 `evaluate` | Borrow idle instance from pool (async-aware mutex queue **(proposed)**). If pool at `max_instances` and all busy, wait with timeout or fail `-32105` **(proposed)** `rate_limited` when wait exceeds `MCP_WASM_POOL_ACQUIRE_TIMEOUT_MS` **(proposed)** (default **50 ms**). |
| **Evaluate** | During checkout | Reset guest linear memory to post-`init` snapshot **(proposed)** OR instantiate from shared `Component` with cheap reset path; set fuel/epoch budget (§5); invoke export (`evaluate` or `wasm.call` target). |
| **Return** | After evaluate completes or traps | Return instance to pool if still healthy; **poison** slot on trap/OOM (discard instance, optionally spawn replacement up to cap). |
| **Lazy grow** | Under sustained load | When checkout waits occur, add instances up to `max_instances` (default **32** per pool key — matches [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md) concurrent cap). |
| **Evict** | Hot-swap or shutdown | Stop scheduling new checkouts on old digest; drain in-flight; destroy pool after quiescence or `MCP_WASM_POOL_DRAIN_TIMEOUT_SECS` **(proposed)** (default **120 s**). |

**Rejected for hot path:** per-request fresh `instantiate()` without pool (acceptable only in `agctl` offline verify **(proposed)**).

### 2. Pool key

Each pool is a distinct **`ComponentPool`** **(proposed)** value in an **`InstancePoolRegistry`** **(proposed)** map.

```
PoolKey = {
  tenant_id,           // from bundle manifest name prefix per ADR 0001
  bundle_name,         // manifest `name` slug
  manifest_digest,     // sha256 canonical manifest — authoritative version pin (ADR 0010)
  component_id,        // manifest `components[].id`
  target_wit,          // e.g. trogon:mcp-policy@0.1.0 — ABI / linker pin
}
```

| Field | Source | Notes |
|-------|--------|-------|
| `manifest_digest` | ADR 0010 signed manifest bytes | **Not** semver alone; hot-swap may change digest with same semver |
| `component_id` | `manifest.toml` `components[]` | Multiple WASM components in one bundle → multiple pools |
| `target_wit` | `manifest.toml` `target_wit` | Linker selection; incompatible WIT → load fails before pool creation |

Semver (`manifest.version`) and guest export `version()` are **audit-only** ([mcp-policy-wit-sketch.md § Versioning](../identity/mcp-policy-wit-sketch.md#versioning-and-naming)).

**CEL `wasm.call("schema-learner", ...)`** **(proposed)** resolves `component_id` against the **active** manifest for the request tenant; unknown id → `-32101` `policy_fault` **(proposed)** `unknown_wasm_component`.

### 3. Concurrency model

```
┌─────────────────────────────────────────────────────────────┐
│  Gateway worker (tokio task per NATS message)               │
│    checkout(pool_key) → evaluate → return/poison            │
└───────────────────────────┬─────────────────────────────────┘
                            │
         ┌──────────────────┼──────────────────┐
         ▼                  ▼                  ▼
   Instance A           Instance B         Instance C
   (idle/busy)          (idle/busy)        (idle/busy)
         └──────────────────┴──────────────────┘
                    same PoolKey
```

| Model | Verdict |
|-------|---------|
| **Single instance, serialized calls** | **Rejected** — head-of-line blocking across queue-group concurrency |
| **Sticky instance per worker thread** | **Rejected** — poor utilization; N×memory with uneven load |
| **Checkout per call (pool)** | **Accepted** — matches agentgateway pooling intent (`MCP_GATEWAY_PLAN.md` § Discipline); one instance serves one evaluation at a time |

Host imports (`spicedb-check`, etc.) run on the **same tokio worker** as the checkout holder; blocking PDP calls use the gateway's existing async SpiceDB client with timeout clamped to request deadline ([reference-host-abi.md §2](../identity/reference-host-abi.md)).

### 4. Engine and linker reuse

| Artifact | Scope | Reuse rule |
|----------|-------|------------|
| **`Engine`** | One per gateway process **(proposed `WasmEngineHandle`)** | Created at startup; config enables component model, fuel, epoch interruption, memory limits (§5). **Shared across all tenants and bundles.** |
| **`Component` (compiled)** | One per `PoolKey` | Cached in **`ComponentCache`** **(proposed)**; invalidated when `manifest_digest` or `component_id` changes |
| **`Linker`** | One per distinct `target_wit` | Host import implementations bound once; reused for all pools at that WIT version |
| **`Store` + instance** | One per pooled slot | Not shared across concurrent evaluations |

Initial heap: configure Wasmtime **static memory maximum** to pool cap (§5) at link time so instances do not pay repeated `memory.grow` on hot path **(proposed)** — tune with Block G latency baseline.

Core wasm modules are **not** loaded on the policy path; loader rejects non-component artifacts ([ADR 0010](0010-bundle-format.md)).

### 5. Memory, fuel, and epoch interruption

Per-instance limits inherit [mcp-policy-wit-sketch.md § Resource limits](../identity/mcp-policy-wit-sketch.md#resource-limits) unless overridden in bundle manifest `[wasm.limits]` **(proposed)** or tenant KV overlay.

| Limit | Default | Scope |
|-------|---------|-------|
| Linear memory max | **16 MiB** (256 pages) | Per instance |
| Fuel per `evaluate` / `wasm.call` | **5_000_000** | Per checkout |
| Fuel per `init` | **500_000** | Once per instance life |
| Max stack | **512 KiB** | Per instance |
| Host import calls per evaluation | **64** | Enforced by host wrapper |
| **`max_instances` per `PoolKey`** | **32** | Pool slot cap |
| **`max_pools` per tenant** | **8** | Distinct `component_id` count |
| **Process-wide pooled instance cap** | **`MCP_WASM_POOL_GLOBAL_MAX_INSTANCES` (proposed)** default **256** | Sum across all pool keys; activation fails closed when exceeded |

**Fuel:** Set on each checkout via Wasmtime fuel API; exhaustion → trap → §6.

**Epoch interruption **(proposed):** Gateway tokio deadline for the NATS message arms an **epoch increment** on the shared `Engine` when `remaining_deadline < MCP_WASM_EPOCH_ARM_MS` **(proposed)** (default **10 ms**), allowing Wasmtime to interrupt runaway guest code even if fuel accounting lags. Epoch deadline exceeded maps same as fuel exhaustion.

**Pool memory budget:** `estimated_bytes = active_instances × 16 MiB` (plus compile cache); gateway `/ready` **(proposed)** reports `wasm_pool_bytes`; exceed **80%** of `MCP_WASM_POOL_MEMORY_BUDGET_MB` **(proposed)** (default **2048**) → refuse new pool expansion (existing pools continue).

### 6. Failure handling → JSON-RPC

Normative mapping aligns [ADR 0024](0024-failure-mode-matrix.md) row 3, [reference-error-codes.md §3](../identity/reference-error-codes.md), and [mcp-policy-wit-sketch.md § Error mapping](../identity/mcp-policy-wit-sketch.md#error-mapping).

| Condition | JSON-RPC | Symbol | Audit `decision_reason` **(proposed)** |
|-----------|----------|--------|----------------------------------------|
| Guest trap (panic, unreachable, OOB) | `-32101` | `policy_fault` | `wasm_trap` |
| Fuel / epoch exhausted | `-32101` | `policy_fault` | `wasm_fuel_exhausted` |
| Linear memory cap / OOM trap | `-32101` | `policy_fault` | `wasm_memory_limit` |
| Host import limit exceeded | `-32101` | `policy_fault` | `wasm_import_limit` |
| `spicedb-check` host error `spicedb_unavailable` | `-32107` | `authz_unreachable` | PDP unreachable |
| Guest `deny(reason)` / `policy-decision::deny` | `-32100` | `policy_deny` | guest reason |
| Guest `challenge(reason)` | `-32107` | `approval_required` | challenge path |
| Oversize `policy-input` / `wasm.call` JSON | `-32101` | `policy_fault` | `wasm_input_oversize` |
| Pool acquire timeout (saturation) | `-32105` | `rate_limited` | `wasm_pool_exhausted` |
| No active bundle / pool missing | `-32108` | `no_policy` | `no_policy` |
| `init()` failure at activation | N/A request path; prior bundle serves | — | `wasm_init_failed` on control audit **(proposed)** |

**`error.data` shape (WASM paths):**

```json
{
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "tier": "wasm",
  "error": "wasm_fuel_exhausted",
  "policy_bundle_digest": "sha256:…",
  "component_id": "schema-learner",
  "wit_version": "0.1.0"
}
```

Constants `-32101` and `-32108` are **(proposed wire)** — not yet in `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs` ([ADR 0024](0024-failure-mode-matrix.md)).

**Side-effect journal:** Trap or `-32101` before decision finalization drops the per-evaluation effect journal ([reference-host-abi.md §13](../identity/reference-host-abi.md#13-side-effect-ordering)); `tests/wasm_host_abi.rs` `wasm_trap_drops_side_effect_journal` covers this.

**Poison policy:** After trap, instance is **not** returned to the idle pool; a replacement is created asynchronously if below `max_instances`.

### 7. Hot-swap and eviction (cross ADR 0026)

On manifest digest promotion ([ADR 0010](0010-bundle-format.md) loader, [ADR 0026](0026-bundle-hot-swap.md) **(proposed)**):

```
1. Prefetch: compile + pre-warm pools for D_new (optional mcp.control.bundle.prefetch)
2. Flip: InstancePoolRegistry.active_digest = D_new for tenant/bundle_name
3. New requests: checkout only from D_new pools
4. In-flight: continue on D_old checkouts until evaluate returns
5. Drain: D_old pools stop accepting checkout; refcount → 0
6. Evict: drop D_old ComponentCache + all instances; no leak across versions
7. Hard cap: after MCP_WASM_POOL_DRAIN_TIMEOUT_SECS, force-drop remaining D_old (audit wasm_pool_force_evicted)
```

| Rule | Detail |
|------|--------|
| **No cross-digest reuse** | `PoolKey.manifest_digest` must match active pointer for checkout |
| **Failed promotion** | Retain D_old pools; D_new warm artifacts discarded |
| **Rollback** | Repoint active digest to D_prev; warm/evict symmetrically |
| **Tenant isolation** | Eviction scoped per `{tenant_id, bundle_name}`; no global flush |

Audit: `bundle_loaded` includes `policy_bundle_digest`; request errors include same field ([ADR 0010](0010-bundle-format.md)).

### 8. CEL integration: `wasm.call` **(proposed)**

Tier 2 CEL programs invoke named components without exporting the full Tier-3 pipeline slot:

```cel
wasm.call("schema-learner", {
  "method": mcp.method,
  "params": request.params,
  "tenant": jwt.tenant
}) == "allow"
```

| Property | Contract |
|----------|----------|
| Registry | Host resolves `component_id` against active manifest `components[]` |
| Input | JSON object serialized to string; mapped to `policy-input` or component-specific guest entry **(proposed export `call-json`)** — if only `evaluate` exists, host wraps input per [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md) |
| Pool | Same checkout path as Tier-3 `evaluate` |
| Capability | Manifest `capabilities.cel_host` must list `wasm.call` **(proposed)** and declare target `component_id` |

Authoritative Tier-3 bundles that export `policy-guest.evaluate` may run without CEL `wasm.call`; both paths share one pool implementation **(proposed `WasmPool::run`)**.

---

## Consequences

### Positive

- **Predictable hot-path latency** — Pre-warmed pools avoid instantiate+link on every `tools/call` gated by WASM.
- **Safe concurrency** — Checkout model prevents data races in guest linear memory without serializing the entire gateway.
- **Version safety** — Digest-keyed pools prevent silent reuse of instances across bundle promotions.
- **Operational alignment** — Pool drain semantics match existing bundle hot-reload test scaffold (`bundle_load_hot_reload.rs`).
- **Single Engine** — Amortizes Wasmtime JIT/code cache cost across tenants; compiled components cached by digest.

### Negative

- **Memory footprint** — Worst case `256 instances × 16 MiB` theoretical if every pool grows to cap; **Mitigation:** process-wide instance cap, `/ready` memory pressure, default `max_instances=32` per component.
- **Pool acquire latency under burst** — Wait or `-32105` when all instances busy; **Mitigation:** lazy grow, metrics `wasm_pool_wait_ms`, operator tuning of prewarm/max.
- **Poisoned instance churn** — Traps trigger re-instantiation; **Mitigation:** circuit-breaker on repeated traps from same digest → refuse activation **(proposed)** `mcp.control.bundle.runtime_unhealthy`.
- **Blocking host imports** — Sync WIT imports may hold pool slots during PDP I/O; **Mitigation:** strict import timeouts; future async component model revisit ([mcp-policy-wit-sketch.md open question 3](../identity/mcp-policy-wit-sketch.md#open-questions)).
- **Implementation complexity** — Reset-vs-recreate instance strategy must be validated against Wasmtime component API; **Mitigation:** prefer recreate-from-`Component` until reset proven stable on current wasmtime release.

### Neutral

- **Multiple components per bundle** — Each `components[].id` gets an independent pool; memory scales linearly with component count.
- **Prefetch path** — Optional; cold promotion may mark instance `/ready=false` until pre-warm completes ([wasm-bundle-format.md §8.2](../identity/wasm-bundle-format.md#82-cold-load-latency-budget-proposed)).
- **CEL vs Tier-3 entry** — Two call surfaces, one pool backend; authors choose based on bundle tier, not pooling behavior.

---

## Rejected alternatives

### Per-request fresh instantiation (no pool)

| | |
|-|-|
| **Pros** | Simplest lifecycle; no poison/drain logic; lowest idle memory |
| **Cons** | Wasmtime link + `init()` per request violates Block F latency budget; compile cache alone insufficient |
| **Verdict** | **Rejected** for gateway hot path. Allowed for offline `agctl mcp policies verify --component` **(proposed)** only |

### Single global instance per component (serialized mutex)

| | |
|-|-|
| **Pros** | Minimal memory; no checkout queue |
| **Cons** | Serializes all concurrent MCP requests through one guest; P99 explodes under queue-group load |
| **Verdict** | **Rejected** |

### Sticky pool per OS thread / tokio worker

| | |
|-|-|
| **Pros** | Avoids cross-thread `Store` moves; zero checkout contention on hot worker |
| **Cons** | Uneven load leaves most instances idle while hot workers queue; memory ≈ `workers × components × 16 MiB` |
| **Verdict** | **Rejected** — checkout pool with async wait matches gateway concurrency model better |

### LRU cache of instances across pool keys

| | |
|-|-|
| **Pros** | Reuses instances when many bundle/component combinations exist |
| **Cons** | Cross-key reuse impossible (different `Component` / linker); LRU across digests risks serving wrong version if keying bug |
| **Verdict** | **Rejected** — LRU applies **within** a `PoolKey` free-list only, not across keys |

---

## Open questions

1. **Instance reset mechanism** — Does current wasmtime release support cheap `Store` reset for component instances post-`evaluate`, or must each checkout create a new `Store` from cached `Component`? Implementation chooses after spike; ADR prefers recreate until measured.

2. **`wasm.call` guest entrypoint** — Single export `evaluate` only (host wraps) vs dedicated `call-json` export for auxiliary components ([mcp-pack schema-learner](../identity/howto-write-bundle.md)). WIT minor bump **(proposed 0.2.0)** if separate export required.

3. **Async host imports** — WASI 0.3 async component model vs blocking `spicedb-check` under pool contention; defer until Block G P99 baseline.

4. **Cross-region pool warming** — Whether leaf gateways prefetch bundles from regional OCI mirror before KV flip ([ADR 0016](0016-multi-region-topology.md)); pooling semantics unchanged, fetch source not.

5. **Regulated tenant lower caps** — Per-tenant KV override for `max_instances` and memory budget; syntax **(proposed)** `{tenant}/wasm.pool.max_instances`.

6. **Relationship to `a2a-gateway` Wasmtime** — `a2a-gateway` uses `WasmtimeSubstrate` + `a2a-redaction` for Tier-3 redaction today; extraction to shared `trogon-policy-wasm` **(proposed crate)** is Block F item 8 — out of scope for pool semantics but affects code reuse.

7. **ADR 0026 pointer** — Exact control subjects and KV keys for prefetch/flip owned by [ADR 0026](0026-bundle-hot-swap.md) **(proposed)**; this ADR assumes `active_digest` flip event exists.

---

## Rollback plan

| Control | Effect |
|---------|--------|
| **`MCP_WASM_POOL_ENABLED=0` **(proposed)** | Disable Tier-3 evaluation and CEL `wasm.call`; CEL-only and SpiceDB path unchanged (Phase 2 behavior) |
| **`MCP_WASM_POOL_PREWARM=0` **(proposed)** | Lazy first-request instantiate (higher latency; not recommended production) |
| **Bundle manifest `wasm.mode=off` **(proposed)** | Skip pool creation for that bundle; no WASM checkout |
| **Active digest rollback** | Repoint to prior manifest digest ([ADR 0010](0010-bundle-format.md)); registry evicts new digest pools per §7 |
| **Reduce caps** | Lower `MCP_WASM_POOL_GLOBAL_MAX_INSTANCES` via env reload; refuse new expansions, existing instances drain naturally |

Rollback does **not** require gateway restart when implemented via config watcher; in-flight evaluations complete on their checked-out digest.

---

## Implementation notes

### Code surfaces **(proposed layout)**

| Path | Responsibility |
|------|----------------|
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/engine.rs` | Process-wide `Engine` config (fuel, epoch, memory, component model) |
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/linker.rs` | WIT `host` imports per `target_wit`; capability gating from manifest |
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/component_cache.rs` | Compiled `Component` by `PoolKey` |
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/pool.rs` | `ComponentPool`, checkout/return, poison, metrics |
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/registry.rs` | `InstancePoolRegistry`, active digest, eviction |
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/eval.rs` | Map `policy-input` / traps → JSON-RPC + audit |
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/wasm.rs` | CEL `wasm.call` registration **(proposed)** |
| `rsworkspace/crates/trogon-mcp-gateway/src/bundle/loader.rs` | Verify → compile → pre-warm → activate ([ADR 0010](0010-bundle-format.md)) |

### Configuration **(proposed env vars)**

| Variable | Default | Purpose |
|----------|---------|---------|
| `MCP_WASM_POOL_ENABLED` | `1` in Phase 3 | Master gate |
| `MCP_WASM_POOL_PREWARM` | `4` | Instances per pool at activation |
| `MCP_WASM_POOL_MAX_INSTANCES` | `32` | Per `PoolKey` cap |
| `MCP_WASM_POOL_GLOBAL_MAX_INSTANCES` | `256` | Process cap |
| `MCP_WASM_POOL_ACQUIRE_TIMEOUT_MS` | `50` | Checkout wait before `-32105` |
| `MCP_WASM_POOL_DRAIN_TIMEOUT_SECS` | `120` | Force evict old digest |
| `MCP_WASM_POOL_MEMORY_BUDGET_MB` | `2048` | Soft limit for expansion |
| `MCP_WASM_FUEL_EVALUATE` | `5000000` | Fuel per checkout |
| `MCP_WASM_FUEL_INIT` | `500000` | Fuel per `init` |

### Acceptance tests

| Test file | Scenario |
|-----------|----------|
| `tests/wasm_host_abi.rs` | Traps → `-32101`; ABI/version gate; effect journal drop |
| `tests/bundle_load_hot_reload.rs` | In-flight drain on prior digest; readiness during warm |
| `tests/jsonrpc_error_codes.rs` | **(proposed)** extend with `policy_fault` / `no_policy` WASM fixtures |
| **(proposed)** `tests/wasm_pool_concurrency.rs` | N parallel checkouts ≤ `max_instances`; no data race |
| **(proposed)** `tests/wasm_pool_eviction.rs` | Hot-swap evicts old digest after drain |

### Metrics **(proposed)**

| Metric | Labels |
|--------|--------|
| `wasm_pool_checkout_wait_ms` | `tenant`, `component_id`, `digest` |
| `wasm_pool_active_instances` | `pool_key` |
| `wasm_pool_poison_total` | `reason` |
| `wasm_evaluate_duration_ms` | `component_id`, `outcome` |

### Dependency note

Phase 3 implementation adds `wasmtime` (and WASI 0.3 component bindings) to `trogon-mcp-gateway` using the **existing workspace pin** — no new top-level dependency version choice in this ADR.

---

*Contract sources: [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F item 2, § Discipline, § Tier 3; [ADR 0010](0010-bundle-format.md); [ADR 0024](0024-failure-mode-matrix.md); [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md); [wasm-bundle-format.md](../identity/wasm-bundle-format.md) §8; [reference-error-codes.md](../identity/reference-error-codes.md); [reference-host-abi.md](../identity/reference-host-abi.md); `rsworkspace/crates/trogon-mcp-gateway/tests/wasm_host_abi.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`; `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs`.*

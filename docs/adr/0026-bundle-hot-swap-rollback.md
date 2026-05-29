# ADR 0026: Bundle Hot-Swap, Versioning, and Atomic Rollback

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block F item 5 (bundle loader from NATS KV with hot-swap and rollback); unblocks Wasmtime pool activation wiring (Block F item 2), first-party `mcp-pack` promotion (Block F item 6), and acceptance tests in `bundle_load_hot_reload.rs` |
| **Related** | [ADR 0010](0010-bundle-format.md), [ADR 0021](0021-bootstrap-day-zero.md), [ADR 0024](0024-failure-mode-matrix.md), ADR 0025 *(wasmtime component pooling — sibling Block F item 2 ADR, this wave)*, [ADR 0011](0011-nats-auth-callout.md), [wasm-bundle-format.md](../identity/wasm-bundle-format.md), [howto-write-bundle.md](../identity/howto-write-bundle.md), [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md), [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md), `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`, `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs`, `rsworkspace/crates/trogon-mcp-gateway/tests/wasm_host_abi.rs`, `rsworkspace/crates/trogon-mcp-gateway/tests/config_hot_reload.rs` |

## Context

Signed policy bundles are the gateway control plane's highest-risk mutable surface: a bad promotion can change authorization, redaction, and WASM guest behavior fleet-wide without a process restart. [ADR 0010](0010-bundle-format.md) defines bundle bytes, NKey verification, and OCI/KV distribution. [ADR 0021](0021-bootstrap-day-zero.md) pins cold-start and pointer-delete behavior. [ADR 0024](0024-failure-mode-matrix.md) rows 3–6 define fail-closed defaults when load or evaluation fails.

What remains undecided — and blocks Block F item 5 — is the **runtime contract** for swapping bundles under queue-group concurrency:

| Gap | Why it blocks implementation |
|-----|------------------------------|
| KV watch vs. control subject vs. admin API precedence | Loader cannot subscribe or debounce without a single source-of-truth ordering |
| Generation identity across replicas | Operators cannot reason about split-brain windows or rollback targets |
| In-flight request fencing | CEL/WASM evaluators need a pin rule; ambiguous pins cause torn policy or use-after-free on drained pools |
| Atomic load pipeline | Partial compile or verify must not publish a half-built bundle |
| Rollback signal paths | Incident response needs deterministic revert without pod restart |
| `trogon:mcp-policy@0.1.0` drift | Forward-incompatible bundles must not brick the gateway |
| Telemetry and audit on swap | SIEM and on-call cannot correlate promotions without stable metric names |

`MCP_GATEWAY_PLAN.md` Block F item 5 requires a loader that fetches signed bundles, verifies NKey signatures (non-negotiable per ADR 0010), hot-swaps in place, and rolls back via pointer flip. The plan's Hot-swap section states: *atomic version pointer; in-flight messages finish on the old version, new ones start on the new; rollback is a pointer flip.* Control-plane subjects include `mcp.control.bundle.reload` as belt-and-braces with KV watchers as primary (`MCP_GATEWAY_PLAN.md` § Control subjects).

The acceptance contract lives in `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`: cold start, parse-error retention, fs-watcher hot swap with in-flight drain, `/readyz` digest, atomic pointer flip, operator rollback, NKey signature gate, and load-time CEL compilation. The test `in_flight_request_completes_with_old_bundle_after_reload` explicitly requires in-flight work to complete on the pre-reload digest while subsequent requests observe the new digest.

[wasm-bundle-format.md §8](../identity/wasm-bundle-format.md#8-loading) documents the load pipeline stages (Fetch → Verify → Compile → Pool → Activate → Evaluate) and §8.5 hot-swap semantics. [hierarchical-policy-merge.md §6](../identity/hierarchical-policy-merge.md#6-storage) pins `mcp-gateway-config/bundle/active` as the active manifest pointer. Binary artifact bytes may live in the **proposed** bucket `mcp-policy-bundles` (ADR 0010) or OCI; the implemented config bucket remains `mcp-gateway-config` per [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md) naming note.

Hot-swap must be safe under **queue-group concurrency**: each gateway replica watches KV and swaps independently. There is no JetStream-managed leader election in scope for bundle promotion; the design must tolerate brief split-brain windows where replicas evaluate different digests until KV revisions converge.

---

## Decision

Adopt a **snapshot-based loader** with **request-scoped bundle pins**, **monotonic local generation**, **KV-revision-ordered promotion**, and a **single atomic activation gate** after the full verify-compile-pool pipeline succeeds. NKey signature verification remains authoritative per ADR 0010; this ADR does not redefine signing.

### 1. Source of truth and watch semantics

| Layer | Canonical name | Role |
|-------|----------------|------|
| Active pointer | `mcp-gateway-config/bundle/active` (hard tenancy) or `{tenant}/bundle/active` (soft tenancy) | JSON manifest pointer: `source`, `digest`, `version`, optional `oci_ref`, `target_wit` |
| Binary blobs | `mcp-policy-bundles` **(proposed)** KV bucket keyed by full `sha256:…` digest; or OCI when `source=oci` | Immutable artifact bytes; gateway read-only |
| Trust roots | `mcp-gateway-config/trusted_signers` | NKey allowlist; watch invalidates verify cache |
| Belt-and-braces signal | `mcp.control.bundle.reload` | Forces immediate re-read of active pointer; does not carry bundle bytes |

**Watch behavior (normative):**

1. On gateway start, subscribe to KV watch on the tenant's active pointer key(s) and `trusted_signers`.
2. On revision change, enqueue a load job keyed by `(kv_revision, digest)` from the pointer JSON.
3. **Debounce** rapid pointer writes: coalesce events within `MCP_GATEWAY_BUNDLE_WATCH_DEBOUNCE_MS` **(proposed)** default `250` ms (acceptance: `watcher_debounce_coalesces_rapid_writes_to_single_reload` in `bundle_load_hot_reload.rs`).
4. **Idempotency:** if the requested `digest` equals the currently active digest and verify metadata is unchanged, skip activation (metric `outcome=noop` **(proposed)**).
5. **Primary path is KV watch.** `mcp.control.bundle.reload` and `POST /admin/reload` **(proposed)** re-trigger step 2 without waiting for a watch edge. Operator CLI `nats kv put … bundle/active` remains the authoritative promotion action ([howto-write-bundle.md](../identity/howto-write-bundle.md)).

Local filesystem bundle paths (dev / CI) use the same pipeline via fs notify; production default is KV + optional OCI.

### 2. Generation numbering and ordering

Two identifiers serve different consumers:

| Identifier | Scope | Semantics |
|------------|-------|-----------|
| **`digest`** | Global, content-addressed | SHA-256 of canonical signed `manifest.toml` bytes (ADR 0010). Authoritative for audit (`policy_bundle_digest`), rollback target, and cross-replica comparison. |
| **`kv_revision`** | Global, per pointer key | JetStream KV revision on `bundle/active`. Operator ordering key: higher revision wins for promotion intent. |
| **`generation`** | **Per gateway replica** | Monotonic `u64` **(proposed type `BundleGeneration`)** incremented exactly once per successful activation on this process. Exposed as gauge `mcp_bundle_active_generation` **(proposed)**. |

**Cross-replica ordering guarantees:**

- There is **no cluster-wide generation leader.** Replicas may activate digest `D_new` at different wall-clock times after KV mirror latency ([ADR 0016](0016-multi-region-topology.md) eventual consistency).
- Split-brain window is **expected and tolerated**: replica A may serve `D_old` while replica B serves `D_new` for seconds. Mitigation is digest in every audit envelope and operator promotion via single KV pointer write, not replica coordination.
- Operators compare fleet state via `/readyz` `bundle.sha256` and `mcp_bundle_active_generation` **(proposed)** per instance plus KV revision history (`nats kv history mcp-gateway-config bundle/active`).
- Hierarchical merge may include `bundle_generation` in predicate cache fingerprint ([hierarchical-policy-merge.md §6](../identity/hierarchical-policy-merge.md#6-storage)); that value is the **local** generation, not a global sequence.

### 3. In-flight request fencing (pinned decision)

**Rule: each request pins the active `BundleSnapshot` at handler entry and retains that snapshot for the entire evaluation, including CEL compile results and WASM pool borrows.**

| Phase | Bundle seen |
|-------|-------------|
| Request started before activation flip | **Prior** snapshot (old digest) |
| Request started after activation flip | **New** snapshot (new digest) |
| Concurrent requests during swap window | Each sees exactly one full snapshot; never mixed manifest fields |

Rationale:

- Matches `bundle_load_hot_reload.rs` contract (`in_flight_request_completes_with_old_bundle_after_reload`, `concurrent_requests_never_see_partial_manifest`).
- Avoids mid-request policy flip that could allow a gated method after SpiceDB check under old rules but before backend publish under new rules.
- Enables safe drain: prior snapshot stays in memory until its `Arc` reference count reaches zero (in-flight complete + pool instances returned).

Implementation sketch **(proposed)**:

```text
BundleStore:
  active: ArcSwap<BundleSnapshot>
  draining: HashMap<Digest, Arc<BundleSnapshot>>  // prior snapshots awaiting drain

pin() -> Arc<BundleSnapshot>:
  return active.load_full()   // cheap Arc clone at request entry

activate(candidate):
  PRECONDITION: candidate fully loaded (see §4)
  old ← active.swap(candidate)
  draining.insert(old.digest, old)
  generation += 1
  schedule_evict_when_refcount_zero(old)
```

New requests MUST NOT block on drain completion. Drain is asynchronous and bounded by `MCP_GATEWAY_BUNDLE_MAX_DRAINING_AGE_SECS` **(proposed)** default `300` s; exceeded age logs WARN and forces pool teardown (in-flight requests hold `Arc` and remain safe).

### 4. Atomicity: load, validate, swap as one operation

The load pipeline is **all-or-nothing** relative to the active pointer. Stages run on a **candidate** snapshot off the hot path:

| Stage | Failure effect |
|-------|----------------|
| Fetch bytes (OCI / `mcp-policy-bundles` **(proposed)** / disk) | Reject; retain active; audit `bundle_rejected` / control `mcp.control.bundle.load_failed` **(proposed)** |
| NKey verify manifest digest (ADR 0010) | Reject; retain active ([ADR 0024](0024-failure-mode-matrix.md) row 4, invariant 8) |
| Parse `manifest.toml`; schema validation | Reject; retain active |
| `target_wit` / `min_gateway_version` / host ABI gate | Reject; retain active (§6 below) |
| CEL: compile **every** `policies/*.cel` at load | Reject; retain active ([ADR 0024](0024-failure-mode-matrix.md) row 6) |
| WASM: Wasmtime compile + pool warm + guest `init()` (ADR 0025 pooling) | Reject; retain active |
| **Activate** — `ArcSwap` pointer flip | Only after all prior stages succeed |

**Rejected bundles MUST NOT:**

- Partially update the active pointer
- Expose torn manifest fields to any request
- Replace compiled CEL programs incrementally on the active snapshot

Acceptance: `reload_publishes_new_pointer_only_after_full_compile`, `failed_mid_pipeline_reload_leaves_prior_pointer` (`bundle_load_hot_reload.rs` `atomic_swap` module).

Cold start with no prior snapshot: failed first load leaves gateway not ready (`/readyz` 503, `-32108` `no_policy` on gated methods per ADR 0021).

Optional **prefetch** path ([wasm-bundle-format.md §8.4](../identity/wasm-bundle-format.md#84-pre-fetch-and-promotion-proposed)): compile and pool candidate without activation; operator then updates pointer. Prefetch failure never changes `active`.

### 5. Rollback API and operator signals

Rollback means **re-activating a previously verified digest** without process restart. Three equivalent signal sources, single effect:

| Signal | Mechanism | Notes |
|--------|-----------|-------|
| **KV pointer revert** (primary) | Operator writes prior JSON to `bundle/active` or restores KV revision | Global; all replicas converge via watch |
| **Control subject** | Publish empty payload to `mcp.control.bundle.reload` | Forces re-read; does not select digest — pointer JSON is truth |
| **Admin HTTP** **(proposed)** | `POST /admin/bundle/rollback` per `admin_api.rs` | Local orchestration helper; MUST ultimately respect KV pointer or explicit `digest` query param **(proposed)**; emits `admin.bundle_rollback` audit **(proposed)** |

**Not rollback triggers:**

- `/readyz` or `/ready` probe failure — reflects state only; does **not** auto-revert pointer (prevents flapping on transient compile slowness).
- WASM panic on request path (row 3) — denies request; operator rollback is manual via pointer revert + optional `mcp.control.bundle.reload`.
- Health probe panic — out of scope; no automatic bundle revert.

**Local activation history **(proposed)**: keep ring buffer of last `N=5` successfully activated digests per process for admin rollback when KV history is unavailable (break-glass only; KV pointer remains authoritative for fleet-wide revert).

Rollback audit MUST emit `bundle_loaded` with `previous_digest` and `restored_digest` (acceptance: `rollback_emits_bundle_loaded_audit_with_old_and_new_ids`).

Explicit pointer **delete** is not rollback — it triggers day-zero / bundle-absent semantics per ADR 0021 (distinct from failed reload retaining last good).

### 6. Coexistence with `trogon:mcp-policy@0.1.0` ABI drift

Compatibility checks run in the candidate pipeline **before** activation (extends ADR 0010 WIT gate):

| Check | On failure |
|-------|------------|
| `manifest.toml` `target_wit` equals host allowlist (`trogon:mcp-policy@0.1.0` for Phase 3 MVP) | Reject; audit `policy_bundle_wit_incompatible` **(proposed)**; retain prior bundle |
| `min_gateway_version` > running gateway semver | Reject; `gateway_too_old` **(proposed)** |
| Guest `host_abi` > gateway supported ABI (`wasm_host_abi.rs`) | Reject at link/load |
| Forward-incompatible WIT major or missing host import | Reject; retain prior |

Forward-compatible minor WIT within the same major MAY be allowed only via explicit host forward-compat table **(proposed)** in gateway config; default is **closed**.

Gateway binary upgrade without bundle change: existing digest remains active if still compatible. Gateway downgrade with newer bundle pinned: load fails closed; prior bundle serves if compatible, else `-32108`.

CEL-only Phase 2 bundles without WASM skip pool stages; `target_wit` check applies when `components/` non-empty.

### 7. Telemetry and audit on swap

| Instrument | Type | Labels / fields | When |
|------------|------|-----------------|------|
| `mcp_bundle_load_total` **(proposed)** | Counter | `outcome` = `success` \| `rejected` \| `rollback` \| `prefetch_ok` \| `prefetch_failed` \| `noop`; `reason` on reject | End of each load pipeline attempt |
| `mcp_bundle_active_generation` **(proposed)** | Gauge | `tenant` (soft tenancy) | After successful activation |
| `mcp_bundle_draining_snapshots` **(proposed)** | Gauge | — | Count of snapshots awaiting drain |
| `bundle.parse_error` **(proposed)** | Counter | `path`, `reason` | Parse/compile failures (acceptance: `bundle_parse_error_metric_emitted_with_path_and_reason`) |

Audit (JetStream `mcp.audit.>` / control subjects per ADR 0010):

| Event | Subject / kind | Key fields |
|-------|----------------|------------|
| Successful activation | `bundle_loaded` | `policy_bundle_digest`, `previous_digest`, `version`, `target_wit`, `kv_revision`, `generation` **(proposed)**, `signer_nkey` |
| Failed promotion | `bundle_rejected` / `mcp.control.bundle.load_failed` **(proposed)** | `attempted_digest`, `decision_reason`, `kv_revision` |
| Operator rollback | `bundle_loaded` + `admin.bundle_rollback` **(proposed)** | `restored_digest`, `superseded_digest` |

Request-path audits continue to stamp `policy_bundle_digest` from the pinned snapshot.

---

## Consequences

### Positive

- **Zero-downtime policy promotion** — new digest serves new requests without dropping NATS queue-group membership or restarting Wasmtime engine ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Hot-swap).
- **Fail-closed load path** — invalid bundles never partially activate; aligns with ADR 0024 invariants 6–8.
- **Deterministic in-flight behavior** — request pin eliminates torn-policy bugs and matches acceptance scaffolds.
- **Operator-native rollback** — KV pointer revert reuses the same loader; no special-case code path beyond digest selection.
- **Replica independence** — no leader election; horizontal scale unchanged.

### Negative

- **Split-brain evaluation window** — replicas may disagree on digest for KV propagation delay.
  - **Mitigation:** digest in audit; monitor max revision lag; optional canary instance before global pointer flip ([wasm-bundle-format.md §8.4](../identity/wasm-bundle-format.md#84-pre-fetch-and-promotion-proposed)).
- **Memory retention during drain** — old WASM pools and compiled CEL stay resident until in-flight completes.
  - **Mitigation:** cap draining snapshots (`MCP_GATEWAY_BUNDLE_MAX_DRAINING_SNAPSHOTS` **(proposed)** default `2`); age-based WARN; pool sizing per ADR 0025.
- **CEL compile latency on promotion** — large bundles block activation until all programs compile.
  - **Mitigation:** prefetch path; `/readyz` false until warm complete when `MCP_GATEWAY_BUNDLE_READY_ON_WARM=1` **(proposed)**.
- **Metric cardinality from `reason` label** — unbounded compile error strings could explode cardinality.
  - **Mitigation:** normalize `reason` to enum (`signature_invalid`, `cel_compile`, `wit_incompatible`, …); raw detail in audit only.

### Neutral

- Local `generation` differs per replica; operators use `digest` + `kv_revision` for fleet comparisons.
- Filesystem watcher path remains for dev; production uses KV.
- Dual bucket names (`mcp-gateway-config` pointer vs. `mcp-policy-bundles` **(proposed)** blobs) persist until a future rename ADR.

---

## Rejected alternatives

### Global generation leader via JetStream

| | |
|-|-|
| **Pros** | Fleet-wide monotonic generation; replicas activate in lockstep; simpler SIEM "generation" story. |
| **Cons** | Requires leader election or CRDT not in scope; adds failure mode on JS unavailability; conflicts with queue-group stateless design; promotion latency tied to consensus. |
| **Verdict** | **Rejected.** Digest + KV revision suffice; split-brain tolerated with audit correlation. Listed as open question only if product later mandates strict fleet uniformity. |

### Block new requests until drain completes (stop-the-world swap)

| | |
|-|-|
| **Pros** | No concurrent old/new evaluation; single digest fleet-wide moment. |
| **Cons** | Violates "without dropping in-flight requests"; introduces latency spike and NATS consumer stall under load; unnecessary given Arc pin model. |
| **Verdict** | **Rejected.** Drain is asynchronous; only pointer flip is synchronous. |

### Mid-request bundle upgrade (evaluate start on old, finish on new)

| | |
|-|-|
| **Pros** | Slightly faster visibility of new rules for long-running requests. |
| **Cons** | Violates acceptance tests; creates inconsistent authorization within one JSON-RPC transaction; complicates WASM pool lifetime. |
| **Verdict** | **Rejected.** Request-scoped pin to entry snapshot is mandatory. |

### Auto-rollback on `/readyz` failure after failed load

| | |
|-|-|
| **Pros** | Self-healing if operator publishes bad pointer. |
| **Cons** | Races with intentional canary; hides operator error; could revert a deliberate partial fleet test; conflates load failure (retains last good) with pointer delete (day-zero). |
| **Verdict** | **Rejected.** Failed load retains last good automatically; rollback requires explicit operator pointer revert. |

---

## Open questions

1. **JetStream Object Store vs. `mcp-policy-bundles` KV for WASM bytes** — [wasm-bundle-format.md §6.2](../identity/wasm-bundle-format.md#62-path-b--nats-kv-import-offline--air-gapped-fallback) lists both; loader blob backend ADR deferred until Object Store provisioning exists.
2. **Prefetch control subject** — `mcp.control.bundle.prefetch` **(proposed)** in wasm-bundle-format §8.4; confirm subject grammar and payload schema in a future ADR or extend [reference-subject-grammar.md](../identity/reference-subject-grammar.md).
3. **Strict fleet digest enforcement** — whether an operator-facing controller should alert when replica `/readyz` digests diverge > N seconds (monitoring only vs. active quarantine).
4. **Soft-tenancy pointer key layout** — single watch on `{tenant}/bundle/active` vs. multiplexed watcher; follows ADR 0001 tenancy finalization.
5. **Admin rollback without KV write** — break-glass local digest pop vs. requiring KV authority for all fleet rollback (security vs. availability tradeoff).
6. **ADR 0025 pool eviction interaction** — exact Wasmtime pool teardown hooks when draining snapshot ages out; implementation detail owned by sibling ADR.

---

## Rollback plan

If snapshot-based hot-swap proves wrong in production:

1. **Config disable **(proposed)**: set `MCP_GATEWAY_BUNDLE_HOT_SWAP=0` — gateway loads initial pointer at startup only; ignores KV watch until restart (degraded ops).
2. **Pin digest in config** **(proposed)**: `MCP_GATEWAY_BUNDLE_PINNED_DIGEST=sha256:…` overrides watch promotions; used for incident freeze.
3. **Revert this ADR's behavior in code** — restore prior behavior: process restart on bundle change only (Block G runbook: rolling restart with pinned OCI digest).
4. **KV revert** — `nats kv put mcp-gateway-config bundle/active @last-known-good.json` remains the operator path regardless of code version ([bootstrap-day-zero.md §7](../identity/bootstrap-day-zero.md)).
5. **Documentation revert** — mark ADR 0026 superseded; re-point Block F item 5 to prior interim model.

Feature flags **(proposed)** live in `mcp-gateway-config/feature_flags` JSON watched alongside bundle pointer.

---

## Implementation notes

| Surface | Responsibility |
|---------|----------------|
| `rsworkspace/crates/trogon-mcp-gateway/src/bundle/` **(proposed module tree)** | `loader.rs`, `store.rs`, `snapshot.rs`, `watch.rs`, `verify.rs` (NKey delegate to ADR 0010), `activate.rs` |
| `rsworkspace/crates/trogon-mcp-gateway/src/policy/cel/` **(proposed)** | Load-time compile; reject partial activation |
| `rsworkspace/crates/trogon-mcp-gateway/src/policy/wasm/` **(proposed)** | Pool per digest; integrate ADR 0025 pooling |
| `rsworkspace/crates/trogon-mcp-gateway/src/http/readyz.rs` **(proposed)** | Expose `bundle.sha256`, local `generation` **(proposed)** |
| `rsworkspace/crates/trogon-mcp-gateway/src/admin/` **(proposed)** | `POST /admin/bundle/rollback`, `POST /admin/reload` |
| `rsworkspace/crates/trogon-mcp-gateway/src/telemetry/metrics.rs` | Counters/gauges in §7 |
| NATS subscriptions | KV watch on `mcp-gateway-config`; optional `mcp.control.bundle.reload` consumer |

**Acceptance tests (un-ignore when implemented):**

| Module in `bundle_load_hot_reload.rs` | ADR section |
|---------------------------------------|-------------|
| `cold_start` | §4 cold path |
| `parse_error`, `signature`, `cel_compile` | §4 pipeline failures |
| `hot_reload`, `atomic_swap` | §3–§4 |
| `sha256` | §7 `/readyz` |
| `rollback` | §5 |

Cross-test: `admin_api.rs` rollback/reload; `wasm_host_abi.rs` ABI rejection at activation; `config_hot_reload.rs` trust-bundle reload remains separate from policy bundle (do not conflate SIGHUP paths).

**Config/env summary **(proposed)**:**

| Name | Default | Purpose |
|------|---------|---------|
| `MCP_GATEWAY_BUNDLE_WATCH_DEBOUNCE_MS` | `250` | Coalesce KV/fs events |
| `MCP_GATEWAY_BUNDLE_MAX_DRAINING_SNAPSHOTS` | `2` | Memory cap |
| `MCP_GATEWAY_BUNDLE_MAX_DRAINING_AGE_SECS` | `300` | Force pool teardown |
| `MCP_GATEWAY_BUNDLE_READY_ON_WARM` | `1` | `/readyz` false until pool warm |
| `MCP_GATEWAY_BUNDLE_HOT_SWAP` | `1` | Disable watch when `0` |

---

*Contract sources: [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F item 5, § Hot-swap, § Control subjects; [docs/adr/0010-bundle-format.md](0010-bundle-format.md); [docs/adr/0021-bootstrap-day-zero.md](0021-bootstrap-day-zero.md); [docs/adr/0024-failure-mode-matrix.md](0024-failure-mode-matrix.md); [docs/identity/wasm-bundle-format.md](../identity/wasm-bundle-format.md) §8; [docs/identity/howto-write-bundle.md](../identity/howto-write-bundle.md); [docs/identity/hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md) §6; [docs/identity/bootstrap-day-zero.md](../identity/bootstrap-day-zero.md); `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/admin_api.rs`.*

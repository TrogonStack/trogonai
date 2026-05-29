# ADR 0012: Rate-Limit State Placement

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Rate-limit state placement; unblocks rate-limit wiring with the chosen state placement |
| **Related** | [rate-limiting.md](../identity/rate-limiting.md), [reference-rate-defaults.md](../identity/reference-rate-defaults.md), [reference-error-codes.md](../identity/reference-error-codes.md), [reference-host-abi.md](../identity/reference-host-abi.md), [reference-rate-defaults.md](../identity/reference-rate-defaults.md) |

## Context

The MCP gateway runs as a NATS queue group: multiple replicas dequeue from `mcp.gateway.request.>` and serve traffic concurrently. Rate limiting on that path serves two distinct operational goals that pull in opposite directions on where state must live.

**Inflight caps (backpressure)** bound how many requests a gateway instance holds open toward a backend `server_id` or on behalf of a tenant at once. Saturation here means the worker is already doing expensive work -- JWT verify, policy evaluation, backend forward, audit publish. Rejecting an over-cap request must be **fast** (sub-microsecond on the hot path) so the gateway does not amplify overload by spending CPU on requests it cannot admit. Inflight is about **concurrency**, not requests-per-second fairness across the fleet.

**Cluster-wide rate budgets (fairness)** bound how often an identity or tool may invoke gated RPCs **across all replicas**. A token-bucket or windowed budget keyed by `jwt.sub`, `(jwt.sub, tool)`, or a CEL-authored bucket id must be **accurate cluster-wide**; otherwise an attacker or noisy neighbor spreads load across *N* queue-group members and consumes *N* times the intended budget. That accuracy requires shared state with atomic compare-and-swap, which implies a NATS round-trip on each authoritative check.

One storage mechanism cannot satisfy both constraints:

| Requirement | Inflight backpressure | Cluster rate budget |
|---|---|---|
| Latency budget | Sub-microsecond; every RPC | ~1--3 ms p99 acceptable when backend cost dominates |
| Consistency | Per-instance acceptable (sized for replica count) | Cluster-wide mandatory |
| Typical trigger | 256 concurrent calls to one `server_id`; 4096 tenant-wide inflight | 100 req / 10 s per caller; opt-in per-tool caps |
| Failure symptom if wrong store | Gateway CPU saturation, head-of-line blocking | Cross-replica budget drift; unfair tenant/agent abuse |

The normative design spec [rate-limiting.md](../identity/rate-limiting.md) analyzed ingress, post-auth, and per-tool placement layers and concluded a **both-and** model: in-process semaphores for inflight, JetStream KV for cluster-authoritative budgets accessed from CEL. Block C item 5 in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) left the backing-store choice open until this ADR.

**What breaks if this stays undecided:**

- Block E cannot wire platform defaults from Wire-Format Pin 9 without forking implementations (pure local vs pure KV).
- Bundle authors cannot rely on a stable `rate.acquire` contract or predictable `-32105` semantics per scope.
- Operators cannot size caps knowing whether inflight is per-replica or cluster-aggregate.
- Observability (`rate_limited_total`, audit `outcome=rate_limited`) lacks a single placement story to label by store type.

**Current state (branch snapshot):**

- `trogon-mcp-gateway::throttle` implements a sliding-window context throttler (adaptive-access risk path); migration to the pinned token-bucket algorithm is Block E work tracked in the spec.
- `cel_builtins/rate.rs` registers `rate.acquire` but returns `NotImplemented` today.
- Platform inflight semaphores (Pin 9 defaults) are **not wired** on the gateway hot path in Phase 1.

## Decision

**Adopt a two-tier rate-limit state model.** Tier 1 (in-process semaphores) handles inflight caps per `server_id` (default **256**) and per tenant (default **4096**) -- fast path, no NATS round-trip, instance-local. Tier 2 (JetStream KV atomic increment) handles cluster-wide rate budgets accessed via CEL `rate.acquire(scope, key, budget, window)` -- slower but accurate. Per-rule choice of tier and scope. Platform defaults follow Wire-Format Pin 9 in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md#9-per-target-inflight-cap-and-rate-limit-defaults).

### Tier assignment (normative)

| Limit kind | Default scope (`data.scope`) | Tier | Backing store | Pin 9 default |
|---|---|---|---|---|
| Per `server_id` inflight | `server` | **Tier 1** | In-process semaphore per gateway instance | 256 concurrent |
| Per tenant inflight | `tenant` | **Tier 1** | In-process semaphore; cap **value** overridable via KV config | 4096 concurrent |
| Per caller `jwt.sub` rate | `caller` | **Tier 2** (hybrid soft-local + KV hard sync per spec) | JetStream KV authoritative; optional local soft cache | 100 req / 10 s |
| Per `(jwt.sub, tool)` rate | `tool` (typical) | **Tier 2** | JetStream KV via `rate.acquire` | unset; opt-in in bundle |

Tier 1 limits compose **in series** with Tier 2 limits on the hot path. A request may pass `rate.acquire` and still receive `-32105` with `scope=server` or `scope=tenant` when a semaphore is saturated ([reference-rate-defaults.md §1.2](../identity/reference-rate-defaults.md)).

### `rate.acquire` contract (Tier 2 entry point)

Policy authors invoke the host builtin defined in [reference-host-abi.md §6.1](../identity/reference-host-abi.md):

```cel
rate.acquire(scope, key, budget, window) -> bool
```

| Argument | Semantics |
|---|---|
| `scope` | `"local"` -- in-memory per replica (CEL-only, not Pin 9 platform defaults); `"cluster"` -- JetStream KV authoritative |
| `key` | Bucket id suffix; host prefixes tenant per [rate-limiting.md §3](../identity/rate-limiting.md) |
| `budget` | Maximum events per window (> 0) |
| `window` | CEL `duration` sliding window length |

Return `true` when one unit is consumed; `false` when rate-limited (rule maps to deny). Explicit deny path emits JSON-RPC `-32105` with `data.scope` derived from scope and key prefix.

Idiomatic pattern:

```cel
mcp.method == "tools/call"
  && mcp.tool.name == "run_llm_inference"
  && rate.acquire("cluster", "tool/" + jwt.tenant + "/" + mcp.tool.name, 10, duration("1m"))
```

Bundle authors choose `"local"` vs `"cluster"` per rule based on whether cross-replica accuracy is worth the KV latency cost.

## Consequences

### Positive

- **Fast inflight path:** Semaphore acquire/release stays in-process; no NATS RTT on every RPC for the Pin 9 backpressure defaults that fire most often under saturation.
- **Accurate cluster budgets when needed:** KV CAS gives one global budget for caller and opt-in tool rules; hybrid soft-local sync (every *N* acquires, default *N* = 10 per spec) amortizes KV cost for default caller rate without abandoning hard caps.
- **Per-rule cost transparency:** `"local"` vs `"cluster"` in `rate.acquire` makes the latency/fairness trade-off explicit in bundle source; platform defaults document which Pin 9 rows are Tier 1 vs Tier 2 without surprise.
- **Unified client surface:** All exceed paths return `-32105 rate_limited` with stable `data.scope` and `retry_after_ms` per [reference-error-codes.md §3](../identity/reference-error-codes.md) and Pin 6.

### Negative

- **Two implementations to maintain:** Semaphore maps (`server_id`, `tenant`) plus KV token-bucket state, hybrid sync, and CEL host dispatch -- separate test matrices and failure modes ([rate-limiting.md §7](../identity/rate-limiting.md)).
- **Bundle authors must understand the trade-off:** Choosing `"local"` in `rate.acquire` multiplies effective budget by replica count; choosing `"cluster"` adds ~1--3 ms p99 per check -- inappropriate for cheap methods unless backend cost dominates.
- **Replica-aware sizing:** Tier 1 inflight defaults are **per gateway instance**; operators running *R* queue-group members must size against approximate `R x cap` cluster-wide concurrency unless backends enforce their own limits ([reference-rate-defaults.md §9](../identity/reference-rate-defaults.md)).

### Neutral

- **Defaults are sensible:** Pin 9 values (256 / 4096 / 100 per 10 s / tool opt-in) match typical MCP backend and gateway fleet shapes without mandating KV on every `ping`.
- **Operators rarely override:** Emergency unlock and global kill-switch keys in the spec apply to Tier 2 and post-auth paths; Tier 1 inflight overrides via bundle/KV are incident-driven, not day-one configuration.

## Alternatives considered

### Pure in-process (all limits local)

Every rate bucket and inflight counter lives in gateway process memory.

| Assessment | |
|---|---|
| **Pros** | Simplest code; lowest latency on all checks. |
| **Cons** | Queue-group scaling breaks fairness: *N* replicas admit *N* times the caller/tool budget; authenticated abuse spreads across members. |
| **Verdict** | **Rejected** for cluster-wide rate budgets. Acceptable **only** for Tier 1 inflight caps where per-replica semantics are documented and sized explicitly. |

### Pure JetStream KV (all limits authoritative in KV)

Every acquire, including inflight semaphores and ingress checks, performs a KV CAS.

| Assessment | |
|---|---|
| **Pros** | Single consistency model; exact cluster-wide inflight if implemented as distributed counter. |
| **Cons** | Adds 1--5 ms p99 to **every** RPC including cheap methods; amplifies hot-key CAS contention under flood; wastes KV on backpressure that must reject before JWT verify on ingress paths. |
| **Verdict** | **Rejected** as the default platform path. KV remains authoritative for Tier 2 and for `"cluster"` scope in CEL. |

### Redis (or other external counter store)

Central Redis cluster with INCR / sliding-window libraries.

| Assessment | |
|---|---|
| **Pros** | Mature atomic increment; sub-millisecond in LAN deployments. |
| **Cons** | Foreign dependency alongside NATS; separate HA, ACL, and audit story; violates mesh-native ops (JetStream KV already provisioned for config, sessions, JWKS). |
| **Verdict** | **Rejected.** JetStream KV is the cluster-wide store; operators already run NATS for transport and config. |

## Implementation notes

### Tier 1 -- in-process semaphores

| Component | Detail |
|---|---|
| **Location** | `trogon-mcp-gateway` gateway worker hot path (Block E); before backend forward, after JWT verify for tenant scope |
| **Keying** | `server_id` for per-backend inflight; `jwt.tenant` (validated claim) for tenant inflight |
| **Acquire** | Atomic counter increment; on exceed, reject without dequeuing additional backend work |
| **Release** | Decrement on reply completion, timeout, or client cancel |
| **`retry_after_ms` (`scope=server`)** | Derived from oldest inflight start timestamp on **this instance** (Pin 9); clamp per [reference-rate-defaults.md §3.1](../identity/reference-rate-defaults.md) |
| **`retry_after_ms` (`scope=tenant`)** | Pinned constant hint (proposed **1000 ms** in reference doc) to avoid cross-replica oldest-tracking divergence |
| **Config** | Bundle `targets.<server_id>.inflight_max`; KV `mcp-gateway-config` keys `inflight.server.*`, `{tenant}/inflight.tenant` |

Each queue-group member maintains its own semaphore map. Worst-case cluster concurrent calls to one `server_id` is approximately `R x inflight_max` for *R* replicas unless the backend enforces its own cap.

### Tier 2 -- JetStream KV

| Component | Detail |
|---|---|
| **Bucket name** | **`mcp-rate-state`** (proposed; consolidate with `mcp-rate-limits` naming in [rate-limiting.md §5](../identity/rate-limiting.md) during Block E implementation) |
| **Key layout** | `{tenant}/{bucket_id}` under soft tenancy; full bucket id string per [rate-limiting.md §3](../identity/rate-limiting.md) |
| **Value** | JSON token-bucket state: `tokens`, `last_refill_ms`, `capacity`, `refill_per_sec` |
| **Atomicity** | JetStream KV compare-and-swap on read-modify-write; retry policy per spec (3x then fail-closed) |
| **Hybrid sync** | Post-auth caller default: in-memory soft bucket + KV hard sync every 10 successful acquires (spec default *N* = 10) |
| **TTL** | `2 x (capacity / refill_per_sec)` seconds, refreshed on write |
| **CEL path** | `cel_builtins/rate.rs` -- implement `rate.acquire(scope, key, budget, window)`; `"cluster"` scope uses `kv-io` side-effect class per host ABI |
| **KV failure** | Fail-closed for KV-backed rules; Tier 1 ingress/inflight semaphores remain effective ([rate-limiting.md §7](../identity/rate-limiting.md)) |

### Wire response on exceed

All tiers surface the same JSON-RPC contract:

```json
{
  "code": -32105,
  "message": "rate_limited",
  "data": {
    "trace_id": "<32 hex>",
    "scope": "server",
    "retry_after_ms": 1200
  }
}
```

| Field | Source |
|---|---|
| `code` | `rpc_codes::RATE_LIMITED` (`-32105`) |
| `data.scope` | Closed enum: `server`, `tenant`, `caller`, `tool`, plus orthogonal `ingress` / `purpose` from other layers |
| `data.retry_after_ms` | Scope-specific computation per [reference-rate-defaults.md §3](../identity/reference-rate-defaults.md) |
| NATS reply headers | Mirror `retry-after-ms` and `mcp-rate-limit-scope` (proposed Block E) |

### Audit

| Property | Value |
|---|---|
| **Audit `outcome`** | `deny` |
| **Subject** | `{prefix}.audit.deny.request.{method_root}` |
| **`decision_reason`** | `rate_limited` |
| **Additive fields** | `rate_limited: true`, `rate_limit_scope`, `bucket_id` (when applicable), `retry_after_ms` |

Correlation with JSON-RPC: clients and SIEM key on `error.data.trace_id` + `error.code` + `decision_reason=rate_limited` ([reference-error-codes.md §7](../identity/reference-error-codes.md)).

### Observability (Block E)

| Metric | Labels | When |
|---|---|---|
| `rate_limited_total` | `tenant`, `agent`, `tool`, `scope` | Each `-32105` denial |
| `rate_limit_check_latency_ms` | `layer`, `store` (`memory`, `kv`) | Acquire path timing |
| `mcp_gateway_inflight_current` | `scope`, `key` | Tier 1 gauge (proposed) |

### Request lifecycle (where tiers attach)

```text
NATS ingress
     |
     v
[optional ingress rate -- Tier 1 memory]
     |
     v
JWT verify
     |
     v
[post-auth caller rate -- Tier 2 hybrid]
     |
     v
[tenant inflight semaphore -- Tier 1]
     |
     v
[server inflight semaphore -- Tier 1]
     |
     v
Policy / CEL rate.acquire per rule -- Tier 2 when scope=cluster
     |
     v
Backend forward
```

## Status of supporting work

| Work item | Status | Owner / anchor |
|---|---|---|
| Block C rate-limit state placement ADR | **Done** (this document) | `docs/adr/0012-rate-limit-state.md` |
| Normative design spec | **Done** | [rate-limiting.md](../identity/rate-limiting.md) |
| Pin 9 defaults reference | **Done** | [reference-rate-defaults.md](../identity/reference-rate-defaults.md) |
| Host ABI `rate.acquire` signature | **Documented** | [reference-host-abi.md §6](../identity/reference-host-abi.md); reconcile 3-arg stub in `rate.rs` in Block E |
| Implement `cel_builtins::rate::acquire` | **Pending** (Block E) | `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/rate.rs` |
| Wire Tier 1 inflight semaphores | **Pending** (Block E) | Gateway worker; defaults 256 / 4096 |
| Provision KV bucket `mcp-rate-state` | **Pending** (Block E) | NATS JetStream KV; merge naming with spec |
| Hybrid soft-local + KV sync | **Pending** (Block E) | Caller default 100 / 10 s |
| `-32105` + audit `outcome=rate_limited` | **Partial** | Code constant exists; full wire + audit fields Block E |
| Migrate `throttle.rs` to token bucket | **Pending** (Block E) | Adaptive-access path; orthogonal scope `purpose` |
| Metrics + NATS header echo | **Pending** (Block E) | [rate-limiting.md §8](../identity/rate-limiting.md), [reference-rate-defaults.md §4](../identity/reference-rate-defaults.md) |
| Close `MCP_GATEWAY_PLAN.md` Block C item 5 checkbox | **Pending** | Editorial after ADR acceptance |

---

*Design context: [rate-limiting.md](../identity/rate-limiting.md). This ADR records the durable placement choice and trade-offs; implementation details remain in the spec and reference docs.*

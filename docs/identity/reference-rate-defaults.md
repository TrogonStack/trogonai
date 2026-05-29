# Per-target inflight and rate-limit defaults reference

**Diátaxis:** reference (lookup tables, config keys, wire shapes, operator knobs).

**Status banner:** Phase 1 contract. Defaults are pinned; per-rule overrides are configurable in the bundle / KV config.

**Canonical source:** Wire-Format Pin 9 — "Per-target inflight cap and rate-limit defaults". This document is the normative source.

**Related:**

| Document | Relevance |
|---|---|
| [rate-limiting.md](rate-limiting.md) | Enforcement modes, token bucket, hybrid KV, ingress layers |
| [reference-error-codes.md](reference-error-codes.md) | `-32105` `rate_limited` JSON-RPC contract (forward reference if not merged) |
| [otel-wiring.md](otel-wiring.md) | Metrics, span events, audit correlation |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Row 10 gateway saturation; fail-closed semantics |
| [hierarchical-policy-merge.md](hierarchical-policy-merge.md) | `mcp-gateway-config` KV key patterns for inflight overlays |

**Implementation anchors (today):**

| Component | Location | Notes |
|---|---|---|
| JSON-RPC code | `rpc_codes::RATE_LIMITED` (`-32_105`) | `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs` |
| Adaptive-access throttle | `throttle.rs` | Sliding window; emits `retry_after_s` on risk path (**today**) |
| CEL `rate.acquire` | `cel_builtins/rate.rs` | **Not implemented** (`NotImplemented`) — contract in [rate-limiting.md](rate-limiting.md) |
| Plan §9 inflight semaphores | Block E | **Not wired** on gateway hot path in Phase 1 worker |

---

## 1. Pinned default limits (Wire-Format Pin 9)

Reproduced verbatim:

| Scope | Default cap | Configurable in | On exceed |
|---|---|---|---|
| Per `server_id` inflight | 256 | bundle / KV config | `-32105 rate_limited`, scope `server`, `retry_after_ms` set from oldest inflight age |
| Per tenant inflight | 4096 | KV config | `-32105 rate_limited`, scope `tenant` |
| Per caller `jwt.sub` rate | 100 req / 10s | bundle | `-32105 rate_limited`, scope `caller` |
| Per `(jwt.sub, tool)` rate | (unset; opt-in via CEL `rate.acquire`) | bundle | per rule |

### 1.1 Scope vocabulary

| `data.scope` (JSON-RPC) | Limit type | Default backing store |
|---|---|---|
| `server` | Inflight semaphore (concurrent requests to one backend target) | In-process per gateway instance |
| `tenant` | Inflight semaphore (concurrent requests attributed to one tenant) | In-process per gateway instance; cap value from KV overlay |
| `caller` | Token-bucket rate budget on `jwt.sub` | Bundle rule + hybrid local/KV sync ([rate-limiting.md §5](rate-limiting.md#5-backing-store-hybrid-model)) |
| `tool` | Token-bucket rate budget on `(jwt.sub, tool)` or rule-defined key | JetStream KV via `rate.acquire` (opt-in) |

Ingress IP limits (`data.scope = "ingress"`) and adaptive-access purpose throttle (`scope = "purpose"`) are **orthogonal** defaults documented in [rate-limiting.md](rate-limiting.md) and [adaptive-access.md](adaptive-access.md). They also surface as `-32105` but are outside Pin 9's four-row table.

### 1.2 Interaction between limits

Limits compose **in series** on the hot path — passing one gate does not skip the others:

```text
ingress (optional) → post-auth caller rate → tenant inflight → server inflight → CEL rate.acquire (per rule) → backend
```

A request may satisfy `rate.acquire` yet still receive `-32105` with `scope=server` or `scope=tenant` when an inflight semaphore is saturated ([rate-limiting.md §6](rate-limiting.md#6-cel-rateacquire-contract)).

---

## 2. Enforcement modes

Pin 9 defines two mechanisms. Full placement rationale, hybrid sync, and failure modes are in [rate-limiting.md](rate-limiting.md) (sections 2–7).

### 2.1 In-process semaphore (inflight caps)

| Property | Value |
|---|---|
| **Use for** | Per `server_id` inflight, per tenant inflight |
| **Scope** | `server`, `tenant` |
| **Consistency** | Per gateway **instance** — no NATS round-trip on acquire/release |
| **Latency** | Sub-microsecond acquire on hot path (atomic counter + optional oldest-entry timestamp for `retry_after_ms`) |
| **Trade-off** | Fleet of *N* replicas may admit up to *N × cap* concurrent requests cluster-wide unless caps are sized per replica |

Each queue-group member maintains its own semaphore map keyed by `server_id` or `tenant`. Operators size per-server cap against **replica count** (see §9).

### 2.2 JetStream KV atomic increment (`rate.acquire`)

| Property | Value |
|---|---|
| **Use for** | Cluster-wide rate budgets; per `(jwt.sub, tool)` when opted in via CEL |
| **Scope** | `caller` (default bundle budget), `tool`, custom scopes in rule expressions |
| **Consistency** | Cluster-wide (subject to KV CAS latency and hybrid soft-cache drift) |
| **Latency** | ~1–3 ms p99 per check (NATS RTT + KV compare-and-swap) — acceptable when backend cost dominates |
| **Bucket** | `mcp-rate-limits` ([rate-limiting.md §5](rate-limiting.md#5-backing-store-hybrid-model)); keys `{tenant}/{bucket_id}` under soft tenancy |

CEL host builtin (pinned arity follows `cel_builtins/rate.rs`, not the four-argument sketch in plan §8):

```cel
rate.acquire(key, capacity, refill_per_sec)  // returns false when denied
```

Idiomatic deny: `!rate.acquire("tool:" + jwt.sub + ":" + mcp.tool.name, 10, 1.0) ? deny("tool rate limited")`.

---

## 3. `retry_after_ms` computation

[reference-error-codes.md](reference-error-codes.md) pins stable `data` fields for `-32105`: `{ trace_id, scope, retry_after_ms }`. Block E standardizes **milliseconds** in JSON-RPC `data` for all rate-limit paths ([rate-limiting.md §8](rate-limiting.md#8-observability)); adaptive-access risk throttle still emits `retry_after_s` **(today)** — convergence tracked in Block E.

| `data.scope` | How `retry_after_ms` is derived | Grounding |
|---|---|---|
| `server` | `now_ms - oldest_inflight_started_at_ms`, clamped to `[1, cap_wait_max]` | Pin 9: "from oldest inflight age" — time until the longest-held slot would free a slot if FIFO ordering |
| `tenant` | **Pinned constant** | **proposed: 1000 ms** — Pin 9 does not specify tenant inflight hint; fixed hint avoids per-replica oldest-tracking divergence across instances |
| `caller` | Remaining window: `(window_ms - elapsed_in_window_ms)`, minimum 1 | Token-bucket / sliding-window remainder for `100 req / 10s` default |
| `tool` | Time-to-next-token × 1000 from KV bucket state | Same algorithm as [rate-limiting.md §4](rate-limiting.md#4-token-bucket-algorithm) |
| `ingress` | Time-to-next-token × 1000 (ingress bucket) | [rate-limiting.md](rate-limiting.md) ingress layer |
| `purpose` | `retry_after_s × 1000` from `ContextThrottler` | [adaptive-access.md](adaptive-access.md) — migration to `retry_after_ms` only |

### 3.1 `server` scope detail

When the per-`server_id` semaphore is at cap **256**, the gateway tracks the enqueue timestamp of the **oldest** in-flight request for that `server_id` on **this instance**:

```text
retry_after_ms = max(1, min(estimated_backend_p99_ms, now_ms - oldest_started_ms))
```

If backend latency is unknown, use `oldest_started_ms` only (Pin 9 does not require backend RTT in the hint). Clients SHOULD honor `retry_after_ms` with jittered backoff; they MUST NOT treat it as a lease grant.

### 3.2 `caller` scope detail

Default budget **100 requests per 10_000 ms** window per `jwt.sub`:

```text
retry_after_ms = max(1, window_ms - (now_ms mod window_ms))   // fixed window remainder
```

Hybrid local/KV sync may admit briefly over budget; `retry_after_ms` reflects the **authoritative** KV state once deny is returned ([rate-limiting.md §7](rate-limiting.md#7-failure-modes)).

---

## 4. Header echo on `-32105`

### 4.1 Pinned behavior (Phase 1 contract)

When the gateway returns JSON-RPC `-32105` on the NATS reply path, it **SHOULD** also set a NATS message header so transport-level clients (non-JSON-RPC subscribers, bridges, observability taps) can backoff without parsing the payload.

| Header | Type | Value | When |
|---|---|---|---|
| `retry-after-ms` | integer string | Same integer as `data.retry_after_ms` | Every `-32105` where `retry_after_ms` is computed |
| `mcp-rate-limit-scope` | string | Same as `data.scope` | Optional but **recommended** for symmetric observability |

**Rationale:** JSON-RPC `data` is authoritative for MCP clients; headers mirror the hint for NATS-native tooling and align with HTTP `Retry-After` semantics without claiming HTTP compatibility.

**Implementation status:** Header echo is **proposed** for Block E — not set in `trogon-mcp-gateway` reply publish paths today. Ingress hardening rules do not include these headers on ingress from clients; gateway sets them only on **egress reply** to the client inbox.

### 4.2 Example reply headers

```text
retry-after-ms: 847
mcp-rate-limit-scope: caller
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
mcp-schema: trogon.mcp/v1
```

---

## 5. Configuration surface

Where each cap is set. Paths below marked **(proposed)** are the operator-facing bundle/KV shapes from this reference task; paths marked **(documented)** appear in repo docs but are not yet validated in Rust loaders.

### 5.1 Summary table

| Limit | Default | Primary config surface | KV / alternate | Loader status |
|---|---|---|---|---|
| Per `server_id` inflight | 256 | Bundle `targets.<server_id>.inflight_max` **(proposed)** | `mcp-gateway-config` key `inflight.server.{server_id}` **(documented)** | Block E |
| Per tenant inflight | 4096 | KV bucket `mcp-tenant-config`, key `{tenant}.inflight_max` **(proposed)** | `mcp-gateway-config` key `{tenant}/inflight.tenant` or `inflight.tenant` **(documented)** | Block E |
| Per caller rate | 100 / 10s | Bundle `policies.<rule>.caller_rate.{budget, window}` **(proposed)** | `mcp-gateway-config/{tenant}/rate_limit/caller` **(proposed)** | Block E |
| Per tool rate | unset | CEL `rate.acquire(...)` in bundle rule | `mcp-rate-limits` bucket **(documented)** | Block E |

**Bucket naming note:** Production docs standardize on **`mcp-gateway-config`** for policy and inflight overlays ([hierarchical-policy-merge.md §6](hierarchical-policy-merge.md#6-storage), [bootstrap-day-zero.md](bootstrap-day-zero.md)). The `mcp-tenant-config` bucket name in the task spec is **proposed** as a tenant-scoped alias — prefer `mcp-gateway-config` keys until a dedicated tenant bucket ships.

### 5.2 Per `server_id` inflight

| Field | Location |
|---|---|
| Bundle **(proposed)** | `targets.<server_id>.inflight_max` — integer, concurrent slots for that backend target on each gateway instance |
| KV **(documented)** | `mcp-gateway-config` → `inflight.server.{server_id}` (hard tenancy: no `{tenant}/` prefix; soft tenancy: `{tenant}/inflight.server.{server_id}`) |

Omit key → default **256**.

### 5.3 Per tenant inflight

| Field | Location |
|---|---|
| KV **(proposed task shape)** | Bucket `mcp-tenant-config`, key `{tenant}.inflight_max` |
| KV **(documented repo shape)** | `mcp-gateway-config` → `inflight.tenant` or `{tenant}/inflight.tenant` |

Omit key → default **4096** per Pin 9.

### 5.4 Per caller `jwt.sub` rate

| Field | Location |
|---|---|
| Bundle **(proposed)** | `policies.<rule_id>.caller_rate.budget` (integer requests), `policies.<rule_id>.caller_rate.window` (duration string, e.g. `10s`) |
| Applies when | Rule is active for the method/target; post-auth gate before expensive policy |

Omit override → default **100 req / 10s** per Pin 9.

### 5.5 Per `(jwt.sub, tool)` rate

No platform default. Opt in per bundle rule:

```cel
!rate.acquire("pair:" + jwt.tenant + ":" + jwt.sub + ":" + mcp.tool.name, 5, 0.2)
  ? deny("pair rate limited")
```

On exceed: `-32105`, `data.scope = "tool"` (or rule-specific scope string — prefer closed enum values in §1.1).

---

## 6. Bundle and KV override examples

Examples use YAML; JSON bundles are equivalent. Keys marked **(proposed)** follow §5 — validate against `agctl` / bundle loader before production.

### 6.1 Raise per-server inflight for a heavy backend

```yaml
# bundle fragment — targets overlay (proposed schema)
targets:
  github:
    inflight_max: 512
```

KV equivalent **(documented)**:

```bash
nats kv put mcp-gateway-config "inflight.server.github" '512'
# soft tenancy:
nats kv put mcp-gateway-config "acme/inflight.server.github" '512'
```

### 6.2 Lower tenant inflight during incident

```yaml
# KV value — proposed mcp-tenant-config layout
# key: acme.inflight_max
# value: 1024
```

```bash
# documented mcp-gateway-config layout
nats kv put mcp-gateway-config "acme/inflight.tenant" '1024'
```

### 6.3 Tighten caller rate on a sensitive rule

```yaml
policies:
  tools-call-production:
    match:
      method: tools/call
      server_id: production-mcp
    caller_rate:
      budget: 30
      window: 10s
```

### 6.4 Opt-in per-tool cluster-wide cap

```yaml
policies:
  deny-expensive-deploy:
    match:
      method: tools/call
      tool: deploy_production
    expression: |
      !rate.acquire("tool:" + jwt.tenant + ":deploy_production", 2, 0.05)
        ? deny("deploy_production rate limited")
```

---

## 7. Telemetry and audit

### 7.1 Metrics

| Metric | Type | Labels | Description | Status |
|---|---|---|---|---|
| `mcp_gateway_inflight_current` | Gauge | `scope`, `key` | Current inflight count; `key` is `server_id` or `tenant` slug | **proposed** (Block E) |
| `rate_limited_total` | Counter | `tenant`, `agent`, `tool`, `scope` | Denials where cause is rate limit | **proposed** — see [rate-limiting.md §8](rate-limiting.md#8-observability) |
| `mcp_gateway_authz_decision_total{reason="rate_limited"}` | Counter | `tenant`, `decision` | Aggregated deny path | [otel-wiring.md §4.3](otel-wiring.md#43-mcp_gateway_authz_decision_total) |

**`mcp_gateway_inflight_current` examples:**

```text
mcp_gateway_inflight_current{scope="server",key="github"} 241
mcp_gateway_inflight_current{scope="tenant",key="acme"} 1204
```

Alert when `current / configured_max > 0.85` sustained for 5m — saturation precedes hard deny.

### 7.2 Spans and events

On `-32105`, child span event `gateway.rate_limited` with attributes `scope`, `retry_after_ms` ([otel-wiring.md §3.7](otel-wiring.md#37-span-status-and-events)).

### 7.3 Audit envelope

| Field | When | Notes |
|---|---|---|
| Outcome | `deny` / normalized `rate_limited` | [reference-audit-envelope.md §2.1](reference-audit-envelope.md#21-header-field-reference) |
| `rate_limited` | `true` | **proposed** additive field |
| `rate_limit_scope` | `server` \| `tenant` \| `caller` \| `tool` | Matches `data.scope` |
| `retry_after_ms` | integer | Mirrors JSON-RPC `data` |
| `decision_reason` | `rate_limited` | Closed enum — [otel-wiring.md](otel-wiring.md) `reason=rate_limited` |

Subject: `{prefix}.audit.deny.request.{method_root}` **(proposed)** per [failure-mode-matrix.md](failure-mode-matrix.md) row 10.

---

## 8. JSON-RPC and error catalog cross-reference

Stable client contract for Pin 9 denials:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32105,
    "message": "rate_limited",
    "data": {
      "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
      "scope": "server",
      "retry_after_ms": 1200
    }
  }
}
```

Full code table: [reference-error-codes.md](reference-error-codes.md).

| Code | Symbol | Pin 9 relevance |
|---|---|---|
| `-32105` | `rate_limited` | Inflight cap or rate budget exceeded |
| `-32101` | `policy_fault` | CEL/`rate.acquire` not implemented or KV fail-closed ambiguity **(today)** |

---

## 9. Operator tuning guide

**Per-server inflight (default 256):** Size per **gateway replica**, not per cluster. If you run *R* queue-group members, worst-case concurrent calls to one `server_id` is approximately `R × inflight_max` unless backends enforce their own limits. Start at 256 when backend p99 &lt; 2s; divide cap by *R* when moving from single-replica dev to HA production, or accept higher aggregate pressure on shared MCP servers.

Lower `inflight_max` when:

- Backend queue depth or MCP server CPU climbs before gateway deny metrics fire.
- A single `server_id` dominates fleet inflight ([`mcp_gateway_inflight_current`](otel-wiring.md) **proposed**).
- Head-of-line blocking affects unrelated tenants on the same replica.

**Per-tenant inflight (default 4096):** Protects fairness across tenants on shared gateway fleets. Lower toward **512–1024** when a tenant runs high fan-out automation (`tools/call` storms) or shares replicas with latency-sensitive neighbors. Use KV overlay for incident throttle; revert via watch without redeploy.

**Per-caller rate (default 100 / 10s):** Tune in bundle when legitimate power users hit `-32105` with `scope=caller` but tenant and server gauges stay healthy — indicates human or agent `jwt.sub` burst, not backend saturation.

**Per-tool `rate.acquire`:** Add only for tools whose backend cost exceeds KV check latency (~1–3 ms). Do not mirror inflight caps in KV — use semaphores for concurrency, `rate.acquire` for time-window budgets ([rate-limiting.md §2](rate-limiting.md#2-where-in-the-request-lifecycle)).

---

## 10. Quick lookup — exceed behavior

| Scope | Default | Exceed code | `data.scope` | Config |
|---|---|---|---|---|
| `server_id` inflight | 256 | `-32105` | `server` | bundle / KV |
| tenant inflight | 4096 | `-32105` | `tenant` | KV |
| `jwt.sub` rate | 100 / 10s | `-32105` | `caller` | bundle |
| `(jwt.sub, tool)` | unset | `-32105` | `tool` (typical) | CEL / bundle |

---

## 11. Cross-references (index)

| Link | Section |
|---|---|
| [rate-limiting.md](rate-limiting.md) | Full mode discussion, hybrid store, ingress layers, `rate.acquire` |
| [reference-error-codes.md](reference-error-codes.md) | JSON-RPC `-32100`…`-32199` catalog |
| [otel-wiring.md](otel-wiring.md) | `gateway.rate_limited` span event; `reason=rate_limited` |
| [hierarchical-policy-merge.md §6](hierarchical-policy-merge.md#6-storage) | `inflight.server.*`, `inflight.tenant` KV keys |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Row 10 saturation |

---

## Changelog

| Date | Change |
|---|---|
| 2026-05-28 | Initial reference — Wire-Format Pin 9; Block H deliverable |

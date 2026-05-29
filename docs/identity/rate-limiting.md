# Rate limiting — placement, dimensions, and backing store

**Status:** Normative design spec (Block C paper; Block E implementation gate). Satisfies [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block C “Rate-limit state placement” and unblocks Block E “Rate limiting wired with chosen state-placement decision”.

**Diátaxis:** Explanation (sections 1–5, narrative in 6–10) + reference (tables, contracts, operator knobs).

**Related:** [Identity overview](overview.md), [Failure-mode matrix](failure-mode-matrix.md), [Adaptive access](adaptive-access.md), [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Blocks C and E, §9 backpressure defaults.

**Implementation note:** `trogon-mcp-gateway::throttle` (`throttle.rs`) implements a **sliding-window** context throttler keyed by `(tenant, agent_id, purpose)` for adaptive-access risk paths. The CEL host builtin `rate.acquire` in `cel_builtins/rate.rs` is **not yet implemented** (`NotImplemented`). This document is the contract both paths must converge on in Block E; code changes to `throttle.rs` and `rate.rs` are explicitly out of scope for this paper.

---

## 1. Why rate-limit at all

Rate limiting on the MCP gateway is not a substitute for authorization. SpiceDB answers *whether* a caller may invoke a tool; rate limiting answers *how often* they may invoke it without degrading the mesh for everyone else. Three operational goals drive the design:

| Goal | What breaks without limits | Gateway symptom |
|---|---|---|
| **Abuse prevention** | Credential theft, runaway automation, or malicious clients hammer gated RPCs | CPU saturation on gateway workers, NATS fan-out storms, audit stream backpressure |
| **Fairness across tenants** | One tenant’s burst consumes shared gateway replicas, SpiceDB QPS, or backend MCP slots | P99 latency spikes for unrelated tenants on the same fleet |
| **Protection of expensive upstreams** | Each `tools/call` may trigger SpiceDB checks, STS mint, LLM inference, or external SaaS APIs | Upstream quota exhaustion, cost overruns, provider-side throttling with opaque errors |

### Threat vectors (motivating scenarios)

Three concrete attack or failure patterns justify layered limits rather than a single global cap:

**1. Credential stuffing and token replay at ingress**

An attacker obtains or guesses bootstrap or mesh JWT material and issues high-frequency JSON-RPC on `mcp.gateway.request.>` before policy can distinguish tenants. Without an ingress ceiling keyed by source (IP, NATS connection id, or unauthenticated connection rate), the gateway pays JWT parse, trace allocation, and queue-group dequeue cost on every message even when later gates would deny.

**2. Noisy-neighbor tenant or agent**

A legitimate tenant deploys a buggy agent loop (`while true: tools/call`). Post-auth limits keyed by `jwt.tenant` and `jwt.agent_id` prevent one automation from starving others in the same NATS account or soft-tenancy partition. Fairness requires identity-aware keys; ingress IP limits alone are insufficient when many agents share a NATS egress.

**3. Expensive-tool amplification**

Even authorized callers can destabilize backends: a `tools/call` into an LLM-backed MCP server or a SpiceDB-heavy policy path costs orders of magnitude more than `tools/list`. Per-tool and per-(agent, tool) budgets stop “allowed but catastrophic” call patterns without denying the tool outright.

Cross-reference: [overview.md § Failure modes](overview.md#failure-modes) maps context throttle to `-32105 rate_limited`; [failure-mode-matrix.md](failure-mode-matrix.md) row 10 (gateway worker saturated) covers inflight caps — a sibling backpressure mechanism, not a substitute for rate budgets.

---

## 2. Where in the request lifecycle

Rate limits can attach at three distinct points on the gateway worker path. Each placement sees different identity and costs different work per rejected request.

### Request lifecycle (simplified)

```
  NATS ingress
       │
       ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ A. INGRESS — before JWT verify                               │
  │    Key: source IP / connection id / unauthenticated rate     │
  │    Cost to reject: ~µs (no JWT, no CEL, no SpiceDB)          │
  └──────────────────────────┬──────────────────────────────────┘
                             ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ B. POST-AUTH — after JWT verify, before policy bundle        │
  │    Key: jwt.tenant, jwt.sub, jwt.agent_id, jwt.purpose       │
  │    Cost to reject: JWT crypto + claim parse; no SpiceDB yet  │
  └──────────────────────────┬──────────────────────────────────┘
                             ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ Policy: CEL / WASM / SpiceDB gate (method-dependent)         │
  └──────────────────────────┬──────────────────────────────────┘
                             ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ C. PER-TOOL — after policy decision (allow path) or inside   │
  │    CEL via rate.acquire before expensive builtins            │
  │    Key: (agent_id, tool_name), tool-specific budgets         │
  │    Cost to reject: full policy eval; saves backend/SpiceDB   │
  └──────────────────────────┬──────────────────────────────────┘
                             ▼
  Backend forward / egress mint / audit publish
```

### Candidate placements compared

| Placement | Identity available | Typical key | Reject cost (order) | Blind spots |
|---|---|---|---|---|
| **A. Ingress** (pre-JWT) | Transport only | `ingress:ip:{addr}`, `ingress:conn:{nats_conn_id}` | Tens of µs | Cannot distinguish tenants behind NAT; shared egress IP |
| **B. Post-auth** (post-JWT, pre-policy) | Full JWT claims | `tenant:{tenant}`, `agent:{tenant}:{agent_id}`, `caller:{jwt.sub}` | Hundreds of µs | Does not know tool cost until params parsed |
| **C. Per-tool** (post-policy / in-CEL) | JWT + `mcp.tool.name` + policy context | `tool:{tenant}:{tool}`, `pair:{tenant}:{agent_id}:{tool}` | Milliseconds (policy already ran) | Too late for cheap denial if policy itself is expensive |

### Recommended combined placement

**Adopt all three layers with different budgets and backing stores**, not a single winner-takes-all choice.

| Layer | Purpose | Default budget (starting point) | Backing store | JSON-RPC on exceed |
|---|---|---|---|---|
| **Ingress** | DoS absorption, connection abuse | 500 req / 10 s per source IP (burst 100) | In-memory per replica | `-32105 rate_limited`, `data.scope = "ingress"` |
| **Post-auth** | Tenant/agent fairness | 100 req / 10 s per `jwt.sub` (plan §9 default); 4096 inflight per tenant (separate semaphore) | Hybrid: in-memory soft + KV hard sync every 10 requests | `-32105`, `data.scope = "caller"` or `"tenant"` |
| **Per-tool** | Expensive upstream protection | Opt-in per bundle rule via `rate.acquire` | JetStream KV (cluster-wide) | `-32105`, `data.scope = "tool"` |

**Quantified rationale:**

1. **Ingress** rejects ~99% of raw flood traffic at &lt;50 µs per message (no JWT verify). At 10k rps attack, saving JWT verify alone preserves ~5–10% CPU on a 4-core gateway pod (internal estimate; validate in Block G benchmarks).

2. **Post-auth** aligns with [MCP_GATEWAY_PLAN.md §9](../../MCP_GATEWAY_PLAN.md) defaults (`100 req / 10s` per caller, `4096` tenant inflight). Enforcement here prevents authenticated abuse without waiting for CEL evaluation on every method.

3. **Per-tool** limits are **opt-in** via CEL because tool cost variance is tenant-specific. A cluster-wide KV check adds ~1–3 ms p99 (NATS RTT + KV compare-and-swap); acceptable only for tools where backend cost &gt;&gt; 3 ms (LLM, provisioning, bulk SpiceDB).

**Relationship to adaptive-access throttling:** [adaptive-access.md](adaptive-access.md) describes `ContextThrottler` keyed by `(tenant, agent_id, purpose)` on the risk path (`policy::run_with_risk`). That is a **post-auth, purpose-scoped** sliding window, surfaced as `-32105` when risk score triggers throttle — orthogonal to ingress IP limits but overlapping with post-auth fairness. Block E should register both under the same observability labels (`scope=purpose` vs `scope=caller`) and converge algorithms (see §4).

---

## 3. Dimensions

Rate limits are keyed by one or more **dimensions**. The gateway composes a canonical **bucket id** string from dimensions plus a **scope prefix** that encodes placement layer (ingress / tenant / tool).

### Dimension matrix

| Dimension | Key fragment example | Mandatory? | Layer(s) | Notes |
|---|---|---|---|---|
| **Per-tenant** | `tenant:acme` | **Mandatory** for post-auth fairness | B | Derived from `jwt.tenant` or NATS account mapping ([ADR 0001](../adr/0001-tenancy-model.md)) |
| **Per-agent** | `agent:acme/oncall-responder` | **Mandatory** when `jwt.agent_id` present | B, C | Prefer `agent_id` over raw `sub` for registered agents |
| **Per-caller (`jwt.sub`)** | `caller:user:alice@acme.com` | **Mandatory** default post-auth cap (plan §9) | B | Distinct from agent when human drives session |
| **Per-tool** | `tool:deploy_production` | Optional (opt-in rules) | C | From `mcp.tool.name` on `tools/call` |
| **Per-method** | `method:tools/call` | Optional | B | Coarse guard for expensive method roots |
| **Per-(agent, tool) pair** | `pair:acme/oncall-responder:delete_repository` | Optional (recommended for sensitive tools) | C | Primary key for expensive-tool budgets |
| **Per-purpose** | `purpose:incident_response` | Optional | B | Used by adaptive-access throttle today |
| **Per-source IP** | `ingress:ip:203.0.113.42` | **Mandatory** at ingress layer | A | Only dimension available pre-JWT |

### Bucket id composition (normative)

Implementations MUST construct bucket ids as:

```
{bucket_scope}/{tenant_or_global}/{dimension_segments...}
```

Rules:

1. **`bucket_scope`** — one of `ingress`, `tenant`, `caller`, `agent`, `tool`, `pair`, `purpose`, `method`.
2. **`tenant_or_global`** — literal tenant id, or `_global` for ingress keys with no tenant yet.
3. **`dimension_segments`** — `/`-joined, URL-safe, lowercased; no raw JWT blobs.

Examples:

| Policy intent | Bucket id |
|---|---|
| Ingress IP limit | `ingress/_global/ip/203.0.113.42` |
| Tenant fairness | `tenant/acme/_aggregate` |
| Caller default (plan §9) | `caller/acme/user:alice@acme.com` |
| Agent + tool | `pair/acme/oncall-responder/delete_repository` |
| Adaptive-access purpose throttle | `purpose/acme/oncall-responder/incident_response` |

**Mandatory vs optional summary:**

- **Mandatory (platform defaults):** ingress source IP (or connection id fallback), per-tenant aggregate post-auth ceiling, per-`jwt.sub` post-auth rate (when JWT ingress active).
- **Optional (bundle / CEL):** per-tool, per-(agent, tool), per-method, per-purpose — enabled by operator config or explicit `rate.acquire` in policy.

**Collision avoidance across tenants:** Never use a bucket id without the tenant segment except ingress `_global` keys. Policy authors passing custom keys to `rate.acquire` MUST prefix with a tenant-scoped fragment, e.g. `rate.acquire("caller/acme/" + jwt.sub, 100, 0.167)` — the host does not auto-prefix (caller responsibility), but bundled defaults MUST auto-prefix to prevent cross-tenant collision.

---

## 4. Algorithm

### Decision: token bucket (single algorithm for Block E)

Block E standardizes on the **token bucket** for all new rate-limit paths: CEL `rate.acquire`, JetStream KV backing, and the migration target for `throttle.rs`.

| Algorithm | Burst handling | Memory per key | Distributed sync | Fit for MCP gateway |
|---|---|---|---|---|
| **Token bucket** | Absorbs bursts up to `capacity`; steady rate via `refill_per_sec` | O(1) state: `{tokens, last_refill_ts}` | CAS on `{tokens, last_refill_ts}` in KV | **Selected** — matches agent bursty tool usage |
| Leaky bucket | Smooth output; penalizes legitimate bursts | O(1) | Same | Poor fit for batch agents |
| Sliding-window counter | Accurate window count; sharp reset edges | O(window events) or approximate counters | Heavy in KV | Current `throttle.rs` implementation; migrate off |

**Justification:** MCP clients legitimately burst (initialize, `tools/list`, several `tools/call` in one orchestration step). Token bucket allows burst up to `capacity` while bounding sustained rate via `refill_per_sec`. SpiceDB and LLM backends tolerate short bursts better than sustained peg at limit.

**Parameters:**

| Parameter | CEL / config name | Type | Meaning |
|---|---|---|---|
| Capacity | 2nd arg to `rate.acquire` | `int` | Maximum tokens (burst size) |
| Refill rate | 3rd arg to `rate.acquire` | `double` | Tokens added per second (fractional allowed) |

Refill formula on check at time `t`:

```
elapsed = t - last_refill_ts
tokens = min(capacity, tokens + elapsed * refill_per_sec)
if tokens >= 1:
    tokens -= 1
    return allow
else:
    return deny
```

**Migration note:** `ContextThrottler` in `throttle.rs` uses sliding-window (`window_secs`, `max_requests`). Block E should either (a) reimplement on token bucket with equivalent steady rate, or (b) document a compatibility shim mapping `max_requests / window_secs` → `refill_per_sec` with `capacity = max_requests`. Prefer (a) for one algorithm site-wide.

---

## 5. Backing store

Three storage options were evaluated for token-bucket state.

### Options compared

| Store | Consistency | Typical latency | Attack multiplier across N replicas | Ops burden |
|---|---|---|---|---|
| **In-memory per replica** | Per-process only | &lt;1 µs | Attacker can consume **N × budget** by spreading across queue-group members | None |
| **JetStream KV with TTL** | Cluster-wide (CAS) | 1–5 ms p99 | **1× budget** globally | Bucket TTL, compaction, watch for hot keys |
| **Hybrid** | Soft local + periodic hard sync | &lt;1 µs amortized; 1–5 ms every N checks | Bounded drift: **≤ N × soft_budget + hard cap** | Medium |

### Recommendation: hybrid (in-memory soft + KV hard)

**Use in-memory token buckets for ingress and post-auth soft limits**, plus a **JetStream KV hard bucket** synchronized every **N = 10** successful acquires (or on deny when local tokens = 0).

| Layer | Soft (local) | Hard (KV) | Sync policy |
|---|---|---|---|
| Ingress | Yes — full enforcement local | No — per-replica acceptable for DoS | Optional KV only if anycast ingress behind single IP |
| Post-auth tenant/caller | Yes — fast path | Yes — `mcp-rate-limits` KV bucket | Every 10 acquires: CAS merge; on KV miss, seed from local |
| Per-tool (CEL) | Optional local cache | **Yes — authoritative** | Every acquire for expensive tools; local cache TTL 100 ms |

**KV bucket layout (proposed):**

| Field | Value |
|---|---|
| Bucket name | `mcp-rate-limits` (tenant-prefixed keys under soft tenancy: `{tenant}/{bucket_id}`) |
| Key | Full bucket id string (§3) |
| Value | JSON `{"tokens": f64, "last_refill_ms": i64, "capacity": f64, "refill_per_sec": f64}` |
| TTL | `2 × (capacity / refill_per_sec)` seconds, refreshed on write |

**Why not pure in-memory:** Queue-group gateways horizontally scale; pure local state breaks fairness guarantees in [overview.md](overview.md) multi-tenant posture.

**Why not pure KV:** Ingress and default caller checks happen on every RPC; pure KV would add 1–5 ms to all traffic including cheap `ping`/`initialize`.

**Drift bound:** With sync every 10 requests and soft capacity C, worst-case overshoot before hard cap catches up is approximately `10 × replica_count` tokens per bucket in the worst scheduling scenario — tune hard KV `capacity` to `soft_capacity + slack` where `slack = 10 × max_replicas`.

Cross-reference: [failure-mode-matrix.md](failure-mode-matrix.md) row 12 (JetStream KV unavailable) — rate-limit KV failure behaviour in §7.

---

## 6. CEL integration

Policy authors interact with rate limits through the host builtin registered in `cel_builtins::rate` (`rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/rate.rs`).

### Builtin contract (canonical)

| Property | Value |
|---|---|
| **Namespace** | `rate` (CEL variable bound to namespace marker) |
| **Function** | `rate.acquire` |
| **Arity** | 3 |
| **Signature** | `rate.acquire(key: string, capacity: int, refill_per_sec: double) -> bool` |
| **Return** | `true` if one token consumed; `false` if rate limited (no token consumed) |
| **Errors** | Wrong type/arity → CEL execution error (`policy_fault` path). Host not implemented → `-32101` until Block E lands. |

**Argument semantics:**

| Arg | Type | Description |
|---|---|---|
| `key` | `string` | Bucket id suffix or full bucket id (§3). Must be tenant-scoped by policy author. |
| `capacity` | `int` | Maximum bucket size (burst). Must be &gt; 0. |
| `refill_per_sec` | `double` | Steady refill rate. Must be &gt; 0. |

**Example rules:**

```cel
// Deny expensive tool when per-agent+tool budget exhausted
mcp.method == "tools/call"
  && mcp.tool.name == "run_llm_inference"
  && !rate.acquire(
       "pair/" + jwt.tenant + "/" + jwt.agent_id + "/" + mcp.tool.name,
       10,
       0.05   // ~3 calls/min sustained, burst 10
     )
  ? deny("tool rate limited")

// Rate-limit by act-chain originator (not immediate caller)
mcp.method == "tools/call"
  && !rate.acquire(
       "caller/" + jwt.tenant + "/" + chain.originator().sub,
       100,
       0.0278  // ~100/hour sustained
     )
  ? deny("originator rate limited")
```

**Note on stale docs:** [MCP_GATEWAY_PLAN.md §8](../../MCP_GATEWAY_PLAN.md) lists a four-argument `rate.acquire(scope, key, budget, window)` shape. The wired host ABI in `cel_builtins/rate.rs` uses the three-argument token-bucket form above. Block E implementation and this spec follow **`rate.rs`**; plan table update is a separate editorial change.

### Contexts where `rate.acquire` may be called

| Evaluation phase | Allowed? | Rationale |
|---|---|---|
| Request-phase CEL (pre-forward) | **Yes** | Primary use — deny before backend |
| Response-phase CEL | **Yes, read-only** | May read bucket for metrics; **must not** consume tokens on response-only rules unless idempotent key includes `request.id` |
| Ingress hardcoded gate (`requires_spicedb_for_method`) | No | Use platform post-auth limits instead |
| WASM Tier-3 component | **Future** — same WIT shape as host ABI | Phase 3 port |

**Interaction with `deny()` / rule outcome:** Idiomatic pattern: `!rate.acquire(...) ? deny("rate limited")`. Returning `false` does not auto-deny; the rule must map to deny (failClosed default for `tools/call`).

**Interaction with inflight semaphores:** Inflight caps (plan §9) are separate counters — not token buckets. A request may pass `rate.acquire` yet hit `-32105` on inflight `scope=server|tenant`.

---

## 7. Failure modes

| Failure | Default behaviour | JSON-RPC | Audit | Operator mitigation |
|---|---|---|---|---|
| **KV unavailable** (post-auth hard / per-tool) | **CLOSED** for KV-backed rules; ingress + soft local still enforce | `-32105` if limit unknown and rule requires KV; `-32101` `policy_fault` if CEL cannot distinguish | `{prefix}.audit.error.request.{method_root}` | Fail-closed per [failure-mode-matrix.md](failure-mode-matrix.md) row 12 pattern; optional tenant knob `rate_limit.kv_fail_open: false` (default false) |
| **Clock skew** between replicas | Token refill drift; may over-admit briefly | N/A | Log `rate_limit.clock_skew_ms` gauge | NTP on gateway hosts; store `last_refill_ms` from KV writer; ignore skew &lt; 500 ms |
| **Bucket key collision across tenants** | Tenant A consumes Tenant B budget | Wrong tenant denied or over-admitted | `bucket_id` in audit (§8) | Enforce §3 tenant segment; validate bundle keys in CI |
| **Refill drift** (hybrid sync lag) | Temporary over-admission bounded by §5 drift | `-32105` once hard KV catches up | `rate_limited: true` | Lower sync interval N; tighten hard capacity slack |
| **Hot key in KV** | Single bucket CAS contention | Latency spike | `rate_limit_check_latency_ms` tail | Shard key with hash suffix for extreme tools; local cache 100 ms |
| **`rate.acquire` NotImplemented** | CEL host error | `-32101` **(today)** | error audit | Block E implements builtin |

**Fail-closed default:** Rate limiting protects shared resources; when authoritative store is unreachable, **deny** rather than allow unlimited traffic. Exception: **ingress soft limit** remains in-memory only — still effective during KV outage.

**Relationship to adaptive-access:** Context throttle in [adaptive-access.md](adaptive-access.md) uses in-memory sliding window only; KV outage does not disable it.

---

## 8. Observability

### Metrics (OpenTelemetry / Prometheus naming)

| Metric | Type | Labels | Description |
|---|---|---|---|
| `rate_limited_total` | Counter | `tenant`, `agent`, `tool`, `scope` | Incremented on each `-32105` denial where cause is rate limit |
| `rate_limit_check_latency_ms` | Histogram | `layer` (`ingress`, `post_auth`, `cel`), `store` (`memory`, `kv`) | Time spent in acquire path |
| `rate_limit_kv_sync_total` | Counter | `tenant`, `result` (`ok`, `conflict`, `error`) | Hybrid sync attempts |
| `rate_limit_tokens_remaining` | Gauge (optional debug) | `bucket_id` | Low-cardinality export only via agctl debug |

Label cardinality rules: `tool` label omitted (empty string) for non-`tools/call` methods. `agent` from `jwt.agent_id` or empty.

### Audit envelope additions (proposed fields)

Published on gateway audit JSON; additive to `trogon.mcp.audit/v1`:

| Field | Type | When present |
|---|---|---|
| `rate_limited` | `bool` | Always on deny path when limit triggered; `false` on allow |
| `bucket_id` | `string` | When `rate_limited == true` or `rate.acquire` evaluated |
| `rate_limit_scope` | `string` | `ingress` \| `tenant` \| `caller` \| `agent` \| `tool` \| `pair` \| `purpose` |
| `retry_after_ms` | `int` | Optional client hint (token bucket time-to-next-token × 1000) |

Example fragment on deny:

```json
{
  "decision": "deny",
  "error": { "code": -32105, "message": "rate_limited" },
  "rate_limited": true,
  "bucket_id": "pair/acme/oncall-responder/delete_repository",
  "rate_limit_scope": "tool",
  "retry_after_ms": 4200
}
```

Wire subject unchanged: `{prefix}.audit.deny.request.{method_root}` per [failure-mode-matrix.md](failure-mode-matrix.md).

### Client JSON-RPC error shape

On exceed, return `-32105` (`rpc_codes::RATE_LIMITED`) with:

```json
{
  "code": -32105,
  "message": "rate_limited",
  "data": {
    "scope": "tool",
    "bucket_id": "pair/acme/oncall-responder/delete_repository",
    "retry_after_ms": 4200,
    "trace_id": "…"
  }
}
```

Aligns with adaptive-access `retry_after_s` — Block E should standardize on **`retry_after_ms`** in JSON-RPC `data` for all rate-limit paths.

---

## 9. Operator overrides

| Override | Mechanism | Scope | Effect |
|---|---|---|---|
| **Per-tenant emergency unlock** | KV key `mcp-gateway-config/{tenant}/rate_limit/emergency_unlock` = `true` | Single tenant | Bypass post-auth and CEL KV checks; ingress IP limit remains |
| **Global kill switch** | KV key `mcp-gateway-config/_global/rate_limit/enabled` = `false` | Fleet | Disables all rate limits including ingress (incident response only) |
| **Per-tool override** | Bundle YAML or KV `rate_limit/tools/{tool_name}` → `{capacity, refill_per_sec, enabled}` | Tool | Raises or disables specific tool bucket |
| **Adaptive-access throttle off** | `MeshGatewayConfig.throttle.enabled = false` | Risk path | Disables purpose throttle only ([adaptive-access.md](adaptive-access.md)) |
| **Shadow metrics** | `rate_limit.mode = shadow` **(proposed)** | Tenant | Log `would_rate_limit` without deny — rollout aid |

**Precedence (highest wins):**

1. Global kill switch (`enabled = false`)
2. Per-tenant emergency unlock
3. Per-tool override
4. Bundle / CEL `rate.acquire`
5. Platform defaults (ingress + post-auth)

Changes via NATS KV watch — no gateway restart required. Audit every override activation to `{prefix}.audit.allow.control.rate_limit_override` **(proposed subject)**.

**agctl (Block G):** `agctl mcp rate-limit status --tenant acme` reads KV + local gauges **(future)**.

---

## 10. Open questions

Track in Block E / Block C closure; do not block initial implementation of mandatory layers.

| # | Question | Options | Default if unresolved |
|---|---|---|---|
| 1 | **Per-organisation aggregate caps** | Sum across tenants in org vs independent | Independent tenants only; org cap via SIEM offline |
| 2 | **Cost-weighted limits** | One expensive tool = N tokens | Uniform 1 token per acquire; document pattern `rate.acquire(key, capacity/N, …)` manually |
| 3 | **Refund on policy-deny** | Return token when SpiceDB denies after acquire | **No refund** (simpler); acquire after authz passes |
| 4 | **Sync interval N** for hybrid | Fixed 10 vs adaptive by load | N = 10 |
| 5 | **Ingress key behind carrier NAT** | IP vs NATS connection id vs mTLS fingerprint | Prefer `conn_id`; fall back IP /24 |
| 6 | **Unified algorithm for `throttle.rs`** | Rewrite sliding window vs shim | Rewrite to token bucket |
| 7 | **KV CAS conflict policy** | Retry vs fail-closed | Retry 3× then fail-closed |

---

## Reference — platform default limits (from plan §9)

| Scope | Default | Config surface | On exceed |
|---|---|---|---|
| Per `server_id` inflight | 256 | bundle / KV | `-32105`, scope `server` |
| Per tenant inflight | 4096 | KV | `-32105`, scope `tenant` |
| Per `jwt.sub` rate | 100 req / 10 s | bundle | `-32105`, scope `caller` |
| Per `(jwt.sub, tool)` | unset (opt-in CEL) | bundle | per rule |

Inflight caps = semaphores (in-process). Rate budgets = token bucket (§4) with hybrid store (§5).

---

## Reference — implementation checklist (Block E)

| Item | Owner crate | Depends on |
|---|---|---|
| Implement `cel_builtins::rate::acquire` | `trogon-mcp-gateway` | KV bucket `mcp-rate-limits`, hybrid sync |
| Wire ingress IP limiter | `trogon-mcp-gateway::ingress` | In-memory token bucket |
| Wire post-auth defaults | `trogon-mcp-gateway::gateway` | JWT active |
| Migrate `throttle.rs` to token bucket | `trogon-mcp-gateway::throttle` | This spec §4 |
| Metrics + audit fields | `trogon-mcp-gateway::audit`, telemetry | OTel crate conventions |
| KV operator keys | `mcp-gateway-config` loader | Existing config watch |

---

## Reference — cross-document index

| Document | Relevance |
|---|---|
| [overview.md](overview.md) | Identity claims for bucket dimensions; `-32105` in failure table |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Row 10 saturation; row 12 KV unavailable; CLOSED semantics |
| [adaptive-access.md](adaptive-access.md) | Purpose throttle; `-32105` on risk path |
| [act-chain.md](act-chain.md) | Originator-scoped `rate.acquire` examples (update arity to 3-arg) |
| `cel_builtins/rate.rs` | `ACQUIRE_NAME = "rate.acquire"`, arity 3 |
| `throttle.rs` | Current sliding-window adaptive throttle (migration target) |

---

## Changelog

| Date | Change |
|---|---|
| 2026-05-28 | Initial Block C spec — placement, dimensions, token bucket, hybrid KV, CEL contract |

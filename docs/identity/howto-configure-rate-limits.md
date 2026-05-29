# How to configure per-tenant rate limits

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform operators tuning MCP gateway rate limits and inflight caps on a co-deployed Trogon mesh ([ADR 0017](../adr/0017-product-positioning.md) Option A).

**Related:** [Rate limiting](rate-limiting.md) · [Reference rate defaults (Pin 9)](reference-rate-defaults.md) · [Hierarchical policy merge](hierarchical-policy-merge.md) · [ADR 0012 rate-limit state](../adr/0012-rate-limit-state.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Failure mode matrix](failure-mode-matrix.md) · [How to write a bundle](howto-write-bundle.md)

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| MCP `server_id` | `github` |
| Tool (sensitive) | `deploy_production` |
| NATS subject prefix | `mcp` (`MCP_PREFIX`) |
| KV config bucket | `mcp-gateway-config` |
| KV rate-state bucket | `mcp-rate-state` **(proposed — ADR 0012)** |

Replace these with your deployment values before running commands.

---

## Goal

Configure per-tenant, per-caller, and per-tool rate limits (plus inflight backpressure caps) so the gateway enforces the intended budgets cluster-wide, surfaces `-32105 rate_limited` with actionable hints, and picks up KV or bundle changes without restarting replicas.

---

## When to use this

Use this guide when you need to:

- Raise or lower rate budgets for one tenant without republishing the org-wide bundle.
- Add a per-tool cap on an expensive MCP tool (`tools/call`).
- Investigate `-32105 rate_limited` denials or saturation before backends fail.
- Tune inflight caps (`scope=server` or `scope=tenant`) during an incident.

**When not to use this:**

- Authoring a new CEL policy bundle from scratch — see [How to write a bundle](howto-write-bundle.md).
- Adaptive-access purpose throttle (`scope=purpose`) — see [Adaptive access](adaptive-access.md).
- Ingress IP flood absorption — see [Rate limiting §2](rate-limiting.md#2-where-in-the-request-lifecycle).
- NATS or MCP basics — see [Identity overview](overview.md).

---

## Prerequisites

Confirm the following before Step 1.

1. **Gateway fleet running.** `trogon-mcp-gateway` replicas subscribe to `mcp.gateway.request.>` under queue group `mcp-gateway` ([operator overview § Day-2 checklist](mcp-gateway-operator-overview.md#92-gateway-data-plane)).

2. **JetStream KV reachable.** Buckets `mcp-gateway-config` (policy + cap overlays) and **`mcp-rate-state`** **(proposed — ADR 0012)** for Tier-2 cluster budgets exist or will be provisioned in Block E.

3. **At least one active policy bundle.** Production posture is default deny ([ADR 0021](../adr/0021-bootstrap-day-zero.md), [ADR 0013](../adr/0013-hierarchical-policy-merge.md)); rate limits attach to bundle rules or KV overlays, not to an allow-all baseline.

4. **Operator tooling:** `nats` CLI, `agctl` (`cargo build -p agctl --release`), and read access to gateway metrics or audit subjects.

5. **Environment defaults:**

   ```bash
   export NATS_URL=nats://127.0.0.1:4222
   export MCP_PREFIX=mcp
   export TENANT=acme
   export SERVER_ID=github
   ```

6. **Know the cap hierarchy.** Limits compose **in series** on the hot path ([reference-rate-defaults §1.2](reference-rate-defaults.md)):

   ```text
   ingress (optional) → post-auth caller rate → tenant inflight → server inflight → CEL rate.acquire (per rule) → backend
   ```

   **Effective cap precedence (narrow-only merge):** organisation default → tenant overlay → server overlay → bundle rule → caller/tool override. More-specific layers may **tighten** limits; they must not widen past the intersection of less-specific caps ([hierarchical-policy-merge §2](hierarchical-policy-merge.md#2-merge-semantics), Example D).

---

## Steps

### 1. Map limits to tiers and stores

[ADR 0012](../adr/0012-rate-limit-state.md) assigns each Pin 9 default to a tier. Use this table when choosing KV vs bundle vs CEL.

| Limit | Pin 9 default | `data.scope` | Tier | State store |
|---|---|---|---|---|
| Per `server_id` inflight | 256 concurrent | `server` | **Tier 1** | In-process semaphore per gateway replica |
| Per tenant inflight | 4096 concurrent | `tenant` | **Tier 1** | In-process semaphore; cap value from KV overlay |
| Per caller `jwt.sub` rate | 100 req / 10 s | `caller` | **Tier 2** | JetStream KV authoritative; hybrid soft-local sync every 10 acquires **(ADR 0012)** |
| Per `(jwt.sub, tool)` rate | unset (opt-in) | `tool` | **Tier 2** | JetStream KV via CEL `rate.acquire` |

**Observable check:** `agctl mcp rate-limit status --tenant "${TENANT}" --output json` **(proposed — Block G)** lists active caps by scope. Until it ships, read KV keys in Steps 2–3.

### 2. Set tenant-wide defaults via KV overlay

Bump tenant inflight or caller rate for **one tenant** without forking the org bundle. Writes to `mcp-gateway-config` propagate via KV watch — no gateway restart ([hierarchical-policy-merge §6](hierarchical-policy-merge.md#6-storage)).

**2a — Raise tenant inflight cap**

```bash
# Soft tenancy key shape (documented in hierarchical-policy-merge §6)
nats kv put mcp-gateway-config "${TENANT}/inflight.tenant" '8192'
```

Expected: next request wave uses cap **8192** per replica for tenant `${TENANT}`; omit key restores Pin 9 default **4096**.

**2b — Raise per-caller rate for the tenant**

```bash
cat > /tmp/caller-rate.json <<EOF
{
  "budget": 200,
  "window": "10s"
}
EOF

nats kv put mcp-gateway-config "${TENANT}/rate_limit/caller" @/tmp/caller-rate.json
```

Expected: post-auth caller budget becomes **200 req / 10 s** for all `jwt.sub` under `${TENANT}` unless a more-specific bundle rule tightens further.

**2c — Per-tool KV override (operator knob, no CEL edit)**

```bash
cat > /tmp/tool-rate.json <<EOF
{
  "enabled": true,
  "budget": 20,
  "window": "60s"
}
EOF

nats kv put mcp-gateway-config "${TENANT}/rate_limit/tools/${TOOL:-deploy_production}" @/tmp/tool-rate.json
```

Expected: gateway applies tool override with precedence above platform defaults, below global kill switch ([rate-limiting §9](rate-limiting.md#9-operator-overrides)).

| Override mechanism | KV key pattern | Precedence vs bundle |
|---|---|---|
| Tenant inflight | `{tenant}/inflight.tenant` | Narrows org default only |
| Tenant caller rate | `{tenant}/rate_limit/caller` | Overrides platform 100/10s default |
| Per-tool | `{tenant}/rate_limit/tools/{tool_name}` | Above bundle defaults; below emergency unlock |

### 3. Add or tighten per-tool limits in the bundle (CEL call site)

Policy authors enforce per-tool budgets inside **request-phase CEL** via the host builtin. Operators do not hand-edit CEL in production; publish a signed bundle revision or a tenant-scoped overlay bundle at `mcp-gateway-config/policy/tenant/{tenant}`.

**Where enforcement runs:** after JWT verify and Tier-1 inflight gates, inside the merged policy predicate, **before** backend forward ([ADR 0012 § Request lifecycle](../adr/0012-rate-limit-state.md#request-lifecycle-where-tiers-attach)). The bundle manifest must declare `host.rate-acquire` under `capabilities.imports` ([howto-write-bundle § Step 2](howto-write-bundle.md#step-2--write-the-manifest)).

**Authoritative builtin shape** ([reference-host-abi §6.1](reference-host-abi.md#61-rateacquire), [ADR 0012](../adr/0012-rate-limit-state.md)):

```cel
rate.acquire(scope, key, budget, window) -> bool
```

| Argument | Values | Meaning |
|---|---|---|
| `scope` | `"local"` \| `"cluster"` | `"local"` = per-replica (budget × replica count); `"cluster"` = JetStream KV authoritative |
| `key` | tenant-scoped bucket id | Host prefixes tenant; see [rate-limiting §3](rate-limiting.md#3-dimensions) |
| `budget` | int > 0 | Max events per window (burst ceiling within the window) |
| `window` | CEL `duration` | Sliding window length |

Example fragment (tenant tool cap — cluster-wide):

```cel
mcp.method == "tools/call"
  && mcp.tool.name == "deploy_production"
  && rate.acquire(
       "cluster",
       "pair/" + jwt.tenant + "/" + jwt.sub + "/" + mcp.tool.name,
       5,
       duration("1m")
     )
```

**Burst vs sustained semantics:**

| Mechanism | Burst | Sustained rate | Notes |
|---|---|---|---|
| Tier-2 `rate.acquire(..., budget, window)` | Up to `budget` events within `window` | ≈ `budget / window` | Sliding window per [reference-host-abi §6.1](reference-host-abi.md#61-rateacquire) |
| Platform caller default (100 / 10 s) | 100 requests | 10 req/s average over 10 s window | Fixed-window remainder drives `retry_after_ms` ([reference-rate-defaults §3.2](reference-rate-defaults.md#32-caller-scope-detail)) |
| Tier-1 inflight (`server`, `tenant`) | Admits up to cap **concurrent** requests | Not a token bucket — release on reply/timeout | Saturated when all slots held |

Idiomatic deny: combine with `&&` or `? deny(...)`; `false` from `rate.acquire` does not auto-deny ([rate-limiting §6](rate-limiting.md#6-cel-integration)).

Publish tenant overlay without replacing org bundle:

```bash
# Proposed pointer — validate against bundle loader when Block E lands
nats kv put mcp-gateway-config "policy/tenant/${TENANT}" @./bundles/acme-rate-overlays.yaml
nats pub mcp.control.bundle.reload "{\"tenant\":\"${TENANT}\",\"reason\":\"rate limit tune\"}"  # (proposed)
```

Expected: gateway hot-swaps merged policy within target **< 5 s P99** KV propagation ([howto-integrate-third-party-mcp §4.3](howto-integrate-third-party-mcp.md#43-publish-to-kv)).

### 4. Adjust per-server inflight (backend backpressure)

When `-32105` carries `scope=server`, tune concurrent slots toward one MCP backend.

**Bundle overlay (proposed schema):**

```yaml
targets:
  github:
    inflight_max: 512
```

**KV equivalent (documented):**

```bash
nats kv put mcp-gateway-config "${TENANT}/inflight.server.${SERVER_ID}" '512'
# hard tenancy omit tenant prefix:
# nats kv put mcp-gateway-config "inflight.server.${SERVER_ID}" '512'
```

Expected: each gateway replica admits up to **512** concurrent forwards to `${SERVER_ID}` on that instance. Cluster-wide concurrency ≈ **R × inflight_max** for *R* queue-group members ([reference-rate-defaults §9](reference-rate-defaults.md#9-operator-tuning-guide)).

### 5. Use emergency overrides only during incidents

| Knob | KV key | Effect |
|---|---|---|
| Per-tenant emergency unlock | `mcp-gateway-config/{tenant}/rate_limit/emergency_unlock` = `true` | Bypasses Tier-2 and CEL KV checks; Tier-1 ingress IP limit remains |
| Global kill switch | `mcp-gateway-config/_global/rate_limit/enabled` = `false` | Disables **all** rate limits including ingress **(incident only)** |

```bash
nats kv put mcp-gateway-config "${TENANT}/rate_limit/emergency_unlock" 'true'
```

Expected: audit event on `{prefix}.audit.allow.control.rate_limit_override` **(proposed subject)**. Revert key to `false` when stable.

---

## Verify

Run this checklist after any cap change.

1. **KV revision visible**

   ```bash
   nats kv history mcp-gateway-config "${TENANT}/rate_limit/caller" | tail -3
   ```

   Expected: your write appears with a new revision number.

2. **Synthetic deny (staging)**

   Exhaust a test budget (e.g. 4 rapid `tools/list` when caller cap is 3 req / 1 s in staging — see `tests/rate_limit_caps.rs`). Expected JSON-RPC:

   ```json
   {
     "error": {
       "code": -32105,
       "message": "rate_limited",
       "data": {
         "scope": "caller",
         "retry_after_ms": 847
       }
     }
   }
   ```

3. **NATS reply headers (proposed — Block E)**

   ```bash
   # Inspect reply headers on a rate-limited NATS response
   # Expected: retry-after-ms: <same as data.retry_after_ms>
   # Expected: mcp-rate-limit-scope: caller
   ```

   Header names from [reference-rate-defaults §4](reference-rate-defaults.md#4-header-echo-on--32105) (Pin 1 egress reply contract).

4. **Metrics** — `rate_limited_total{scope=…}` and `mcp_gateway_inflight_current` **(proposed — Block E)**; `mcp_gateway_authz_decision_total{reason="rate_limited"}` ([otel-wiring.md](otel-wiring.md)).

5. **Audit**

   ```bash
   nats sub "${MCP_PREFIX}.audit.deny.request.tools" --count 1
   ```

   Expected envelope fields: `decision_reason=rate_limited`, `rate_limited=true`, `rate_limit_scope` matching JSON-RPC `data.scope`, `retry_after_ms` mirroring `data` ([ADR 0012 § Audit](../adr/0012-rate-limit-state.md#audit)).

---

## Rollback / undo

1. **Record current values** before change: `nats kv get mcp-gateway-config "${TENANT}/rate_limit/caller"` (and any tool or inflight keys touched).

2. **Revert KV overlay** — repoint to previous revision or delete key to restore Pin 9 default:

   ```bash
   nats kv put mcp-gateway-config "${TENANT}/rate_limit/caller" @/tmp/previous-caller-rate.json
   # or delete to fall back to platform default:
   nats kv del mcp-gateway-config "${TENANT}/rate_limit/caller"
   ```

3. **Revert bundle** — repoint `policy/tenant/${TENANT}` or org active digest per [How to write a bundle § Rollback](howto-write-bundle.md#rollback).

4. **Clear emergency unlock** — set `emergency_unlock` to `false`; never leave kill switch disabled in production.

5. **Confirm recovery** — repeat Verify checklist; clients should stop seeing `-32105` under normal load.

Saturation and KV-unavailable behaviour: [failure-mode-matrix row 10](failure-mode-matrix.md#failure-mode-matrix) (CLOSED, `-32105`); row 12 pattern for KV fail-closed on Tier-2 rules.

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| `-32105` `scope=caller` but tenant/server gauges healthy | `jwt.sub` burst exceeds caller budget (default 100/10s) | Raise `{tenant}/rate_limit/caller` or bundle `caller_rate`; confirm distinct `jwt.sub` keys do not share budget ([rate_limit_caps.rs](../rsworkspace/crates/trogon-mcp-gateway/tests/rate_limit_caps.rs)) |
| `-32105` `scope=server`, backend latency normal | Per-replica inflight saturated; cluster load = R × cap | Lower concurrent automation or raise `inflight.server.{server_id}`; size per replica count |
| `-32105` `scope=tool` after bundle deploy | CEL `rate.acquire("cluster", ...)` budget/window too tight | Increase `budget` or `window`; verify `scope="cluster"` when cross-replica accuracy is required |
| `-32101 policy_fault` instead of `-32105` | `rate.acquire` not implemented yet, or KV unreachable fail-closed | Block E wiring pending ([cel_builtins/rate.rs](../rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/rate.rs)); restore JetStream KV |
| Limits unchanged after KV write | Wrong tenancy key prefix or bundle loader not watching key | Match hard vs soft tenancy key shapes ([hierarchical-policy-merge §6](hierarchical-policy-merge.md#6-storage)); check gateway logs for bundle generation |
| Effective budget higher than configured on multi-replica fleet | Used `rate.acquire("local", ...)` or Tier-1 inflight only | Switch tool/caller rules to `"cluster"` scope; divide inflight caps by replica count |

**Open question:** Reconcile 3-arg token-bucket stub in `cel_builtins/rate.rs` with 4-arg `(scope, key, budget, window)` ABI in [reference-host-abi §6.1](reference-host-abi.md#61-rateacquire) during Block E — operators should follow the ADR / host ABI until loader release notes say otherwise.

---

## Related

| Document | Relevance |
|---|---|
| [ADR 0012](../adr/0012-rate-limit-state.md) | Accepted two-tier state placement (Tier 1 inflight, Tier 2 KV) |
| [ADR 0013](../adr/0013-hierarchical-policy-merge.md) | Tenant overlay merge without forking org bundle |
| [rate-limiting.md](rate-limiting.md) | Full placement model, hybrid sync, failure modes |
| [reference-rate-defaults.md](reference-rate-defaults.md) | Pin 9 defaults, `retry_after_ms`, header echo |
| [reference-host-abi.md](reference-host-abi.md) | `rate.acquire` signature and side-effect class |
| [hierarchical-policy-merge.md](hierarchical-policy-merge.md) | KV key patterns, narrow-only rate merge |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Row 10 saturation; `-32105` audit subject |
| [howto-write-bundle.md](howto-write-bundle.md) | CEL `rate.acquire` in bundle programs |
| [tests/rate_limit_caps.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/rate_limit_caps.rs) | Block E verification contract |

---

*Document type: Diátaxis **how-to**. For algorithm and store theory, read [rate-limiting.md](rate-limiting.md). For default lookup tables, read [reference-rate-defaults.md](reference-rate-defaults.md).*

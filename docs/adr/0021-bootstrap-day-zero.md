# ADR 0021: Bootstrap / Day-Zero Behavior

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Bootstrap / day-zero behavior with empty bundle; unblocks readiness evaluator, mixed method classifier, and `/readyz` bootstrap fields |
| **Related** | [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md), [failure-mode-matrix.md](../identity/failure-mode-matrix.md), [overview.md](../identity/overview.md), [registry-operations.md](../identity/registry-operations.md), [sts-exchange.md](../identity/sts-exchange.md), [ADR 0001](0001-tenancy-model.md), [ADR 0003](0003-bootstrap-vs-mesh-tokens.md), [ADR 0010](0010-bundle-format.md), [ADR 0013](0013-hierarchical-policy-merge.md), `rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs` |

## Context

When the MCP gateway boots — or when a tenant's control-plane artifacts are absent — the data plane must choose a posture before any signed policy bundle is loaded. **Day zero** is the interval after JetStream/KV buckets are provisioned but before the identity and policy control plane is seeded: the gateway process may be running, clients may hold bootstrap NATS User JWTs from auth callout (ADR 0011), and backends may be reachable, yet **no successful gated MCP call should be assumed safe** until the bootstrap sequence completes.

The tension is familiar:

| Posture | Benefit | Cost |
|---|---|---|
| **Default-deny** | Strongest security; no ungoverned `tools/call` or `resources/read` | Blocks the same wire path operators use for `initialize` / `ping` smoke tests; raises adoption friction on first deploy |
| **Permissive fall-through** | Ergonomic local dev; operators see end-to-end traffic immediately | Unacceptable in production — unauthenticated policy bypass when SpiceDB endpoint is unset (Phase 1 allow-all) or bundle is missing |
| **Mixed (audit fall-through for probes only)** | Denies authorization-bearing methods; allows session/catalog probes with auditable flag | Operators must understand two method classes; SIEM must alert on sustained bootstrap traffic |

Without pinning this decision, implementation forks on three independent axes:

1. **First boot** — does the gateway process start and accept NATS traffic with an empty `mcp-gateway-config/bundle/active` pointer, or refuse to bind the request consumer?
2. **Mid-session regression** — when the active bundle pointer is deleted, signature validation fails, or KV watch delivers an empty revision, do in-flight requests retain the last good bundle while new requests fail closed, or does the entire tenant fall open?
3. **Operator awareness** — how does a platform team know a production tenant is still in day-zero posture without enabling permissive mode?

The normative design spec [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md) (Block C paper) defines the **`tenant_initialised`** predicate, method classification, audit contracts, bootstrap sequence, and failure recovery. [failure-mode-matrix.md](../identity/failure-mode-matrix.md) row 5 (WASM bundle missing) and row 4 (signature invalid) depend on this ADR for request-path semantics. [ADR 0013](0013-hierarchical-policy-merge.md) states default deny when no bundles exist in production enforce mode; this ADR specifies **which methods** are gated during day zero and **how** operators escape safely.

**Current branch snapshot (Phase 1 gap):**

- `trogon-mcp-gateway` allow-all on SpiceDB-gated methods when `MCP_GATEWAY_SPICEDB_ENDPOINT` is unset — dev-only; must not define production day-zero posture.
- Policy bundle load from KV and readiness evaluator are not wired; `/readyz` scaffold expects 503 until bundle loaded ([health_probes.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/health_probes.rs)).
- `bundle_load_hot_reload.rs` scaffolds cold-start, parse-error retention, and rollback tests against ADR 0010 — day-zero behavior extends those tests with empty-pointer and mid-session unload cases.

**What stays undecided downstream if we do not pin this:**

- Block D cannot implement the mixed method classifier or `-32108` with `data.reason: tenant_uninitialised`.
- Block G `/readyz` JSON shape lacks bootstrap banner fields for kubelet and operator dashboards.
- Per-tenant bootstrap overrides (enforce-from-day-zero tenants) have no config surface.
- SIEM rules cannot key on stable `bootstrap_mode` audit fields vs. ad-hoc log grep.

---

## Decision

**Adopt the paper spec's Mixed posture as the production default at first boot and after bundle loss.** Gated data-plane methods deny before backend publish; session/catalog methods forward with auditable `bootstrap_mode: true`. The gateway **starts and serves NATS traffic** in default configuration — it does **not** refuse process startup when the bundle pointer is absent.

Configuration is exposed as **`bootstrap_mode`** with values **`audit`** (default), **`deny`**, and **`fail-start`**. Environment alias **`MCP_GATEWAY_BOOTSTRAP_POSTURE`** maps to the same enum (`mixed` = `audit`, `strict` = `deny`, `permissive` rejected in enforce mode — see Implementation notes).

### Tenant readiness predicate (pinned)

Define **`tenant_initialised`** as the conjunction of all checks below. Until true, the tenant is in **day-zero posture** regardless of gateway uptime:

| Check | Artifact | KV key / store |
|---|---|---|
| **Trust** | Valid SPIFFE bundle PEM for tenant trust domain | `mcp-trust-bundles/{trust_domain}` |
| **Policy** | Signature-valid bundle revision loaded via active pointer | `mcp-gateway-config/bundle/active` |
| **Registry** | At least one agent at `@latest` with `lifecycle_state = active` | `mcp-agent-registry/{agent_id}/@latest` (required when `MCP_GATEWAY_AGENT_IDENTITY=enforce`) |
| **Authorization graph** | SpiceDB schema includes MCP definitions and minimum tuples for smoke principal | SpiceDB datastore |
| **Backend target** | MCP `server_id` listed in gateway config / discovery | `mcp-gateway-config` backend entry or discovery announcement |

Gateway re-evaluates `tenant_initialised` every **30 s** and on KV watch events for `mcp-gateway-config`, `mcp-trust-bundles`, and `mcp-agent-registry`. Negative results cache **5 s**; positive results hold until any watched key revision changes ([bootstrap-day-zero.md §4.4](../identity/bootstrap-day-zero.md)).

### First boot default (`bootstrap_mode = audit`)

When the gateway starts with **no** `bundle/active` entry, empty PEM trust bundle, empty registry, or any failed readiness check:

| Method class | JSON-RPC / subject suffix | Behavior |
|---|---|---|
| **Gated data plane** | `tools.call`, `resources.read`; side-effect callbacks (`sampling.createMessage`, `elicitation.create`) | **Deny** before backend publish |
| **Session / catalog** | `initialize`, `ping`, `tools.list`, `resources.list`, `prompts.list`, `prompts.get` (read-only) | **Allow** with `bootstrap_mode: true` audit; optional egress header `mcp-bootstrap-incomplete: true` |
| **Notifications** | `notifications.*` | **Deny** (treat as gated) |
| **Control plane** | `mcp.control.>` | **Unchanged** — not handled by gateway request consumer |

This is the paper's **Mixed** posture: fail-closed for authorization-bearing work, limited allow for connectivity and catalog probes on the same NATS subject tree operators use post-bootstrap.

#### Gated deny contract

| Field | Value |
|---|---|
| JSON-RPC code | `-32108` `no_policy` |
| JSON-RPC `data.reason` | `tenant_uninitialised` |
| JSON-RPC `data.missing` | Array of failed checks, e.g. `["policy_bundle","registry","spicedb_tuples"]` |
| Audit subject | `{prefix}.audit.deny.request.{method_root}` |
| Audit `decision` | `deny` |
| Audit `rules_fired` | `["tenant-readiness"]` |
| Audit `extra.reason` | `tenant_uninitialised` |
| Audit `extra.readiness` | Per-check boolean object |
| Backend publish | **None** on deny |

#### Session/catalog allow contract

| Field | Value |
|---|---|
| JSON-RPC | Forward when backend registered and reachable; else `-32103` `backend_unreachable` |
| Audit `extra.bootstrap_mode` | `true` |
| Audit `extra.tenant_initialised` | `false` |
| Egress header **(proposed)** | `mcp-bootstrap-incomplete: true` |

Sustained `bootstrap_mode: true` on allow-path audit after cutover is a **misconfiguration alert** — not normal production steady state.

### Bundle unloaded mid-session (rollback to default)

When the active bundle pointer is **deleted**, **empty**, or **signature-invalid** after a previously loaded revision:

1. **Failed load does not advance** the in-memory active revision ([ADR 0010](0010-bundle-format.md), [bundle_load_hot_reload.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs) `parse_error` / `atomic_swap` modules).
2. If **no prior good bundle** exists, or the operator **explicitly removes** `bundle/active` (compromise hard reset per [bootstrap-day-zero.md §7.3](../identity/bootstrap-day-zero.md)), the gateway **immediately** treats the tenant as day-zero: **`tenant_initialised` = false** for new requests.
3. **In-flight** evaluations that started under a loaded bundle **complete on the prior digest** until drain; **subsequent** requests apply current bootstrap posture (gated deny under default `audit`).
4. Readiness cache invalidates on KV watch; no gateway restart required.
5. Audit on transition: `mcp.control.bundle.load_failed` or `bundle_rejected` with `extra.reason: bundle_signature_invalid` when applicable ([failure-mode-matrix.md](../identity/failure-mode-matrix.md) row 4).

Mid-session unload **never** falls through gated methods permanently. It **never** retains permissive allow-all from Phase 1 SpiceDB-unset behavior in production configuration.

### Operator signals (day-zero awareness)

Operators MUST be able to detect day-zero posture without enabling permissive mode. Three pinned surfaces:

#### 1. Log level and structured fields

| Event | Level | Fields |
|---|---|---|
| Gateway enters day-zero (startup or bundle loss) | **WARN** | `bootstrap_mode=audit|deny`, `tenant_initialised=false`, `readiness={...}` |
| Gated deny due to readiness | **INFO** (default) | `reason=tenant_uninitialised`, `missing=[...]` |
| Sustained bootstrap allows (> threshold, proposed 100/min/tenant) | **WARN** | `bootstrap_mode=true`, `alert=day_zero_traffic_after_cutover` |

Structured logs use stable keys matching audit `extra` for SIEM correlation.

#### 2. Metric

| Metric | Type | Labels | Value |
|---|---|---|---|
| `mcp_gateway_bootstrap_mode_active` | Gauge | `tenant`, `posture` (`audit` \| `deny`) | `1` when `tenant_initialised=false` and gateway serving; `0` after cutover |
| `mcp_gateway_tenant_readiness_check` | Gauge | `tenant`, `check` (`trust`, `policy_bundle`, `registry`, `spicedb`, `backend`) | `1` pass / `0` fail |

Increment `mcp_gateway_bootstrap_denies_total{tenant,method_root}` on gated `-32108` denies with `reason=tenant_uninitialised`.

#### 3. `/readyz` JSON banner

HTTP **`GET /readyz`** includes bootstrap state in the JSON body (extends [health_probes.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/health_probes.rs) contract):

```json
{
  "status": "ready",
  "checks": {
    "nats": "ok",
    "bundle": "absent",
    "spicedb": "ok",
    "tenant_initialised": false
  },
  "bootstrap": {
    "day_zero": true,
    "mode": "audit",
    "posture_effective": "mixed",
    "missing": ["policy_bundle", "registry"]
  },
  "warnings": [
    "day_zero_posture_active: gated MCP methods denied until bootstrap completes"
  ]
}
```

| `bootstrap_mode` | HTTP status when NATS ok | `bootstrap.day_zero` | Notes |
|---|---|---|---|
| `audit` (default) | **200** with `warnings` | `true` until `tenant_initialised` | Process ready for probes; operators see banner |
| `deny` | **200** with `warnings` or **503** (operator choice via `MCP_GATEWAY_READYZ_STRICT=1` **(proposed)**) | `true` | All MCP methods denied; stricter k8s signal optional |
| `fail-start` | **503** (process may exit before binding consumer — see below) | N/A at steady state | No request path until bundle valid |

When `tenant_initialised` transitions to `true`, `bootstrap.day_zero` becomes `false`, `warnings` clears, `checks.bundle` reports loaded digest per ADR 0010, and `mcp_gateway_bootstrap_mode_active` returns to `0`.

**Proposed readiness beacon:** gateway publishes `{ "tenant", "initialised", "checks", "ts" }` on `mcp.control.tenant.ready.{tenant}` when readiness flips false to true ([bootstrap-day-zero.md §6.2](../identity/bootstrap-day-zero.md)).

### Mode overrides (`bootstrap_mode` enum)

| `bootstrap_mode` | `MCP_GATEWAY_BOOTSTRAP_POSTURE` alias | Gated methods | Session/catalog | Process start without bundle |
|---|---|---|---|---|
| **`audit`** (default) | `mixed` or unset | Deny | Allow + audit `bootstrap_mode: true` | **Yes** |
| **`deny`** | `strict` | Deny | Deny | **Yes** |
| **`fail-start`** | *(no alias — explicit only)* | N/A | N/A | **No** — refuse to bind `mcp.gateway.request.>` consumer until signature-valid bundle loads |

**`fail-start`** implementation: gateway exits non-zero or remains in pre-ready state without dequeuing MCP requests; `/readyz` returns **503** with `checks.bundle: required`. Use in CI, compliance scans, and clusters that must not serve any MCP traffic until policy exists.

**Permissive mode** (`MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive`) is **not** a production `bootstrap_mode` value. It remains a dev-only escape documented in the paper; gateway startup **rejects** permissive when `MCP_GATEWAY_AGENT_IDENTITY=enforce`.

### End-to-end flow (default audit)

```text
Gateway boot (no bundle/active)
        |
        v
bootstrap_mode = audit (default)
tenant_initialised = false
        |
        +--> /readyz 200 + bootstrap.day_zero banner
        +--> WARN log + metric gauge = 1
        |
        v
NATS request on mcp.gateway.request.{server_id}.{method}
        |
        +-- gated method --> DENY -32108 tenant_uninitialised (no backend publish)
        |
        +-- session/catalog --> ALLOW + audit bootstrap_mode:true
        |
        v
Operator completes bootstrap sequence (trust, registry, bundle, SpiceDB, backend)
        |
        v
KV watch / 30s poll --> tenant_initialised = true
        |
        +--> /readyz: day_zero false, bundle digest present
        +--> metric gauge = 0
        +--> mcp.control.tenant.ready.{tenant} beacon
```

---

## Consequences

### Positive

- **Security where it matters.** `tools/call` and `resources/read` deny until policy, registry, trust, SpiceDB, and backend targets are present — closing the Phase 1 footgun where unset SpiceDB implies allow-all on gated methods.
- **Operability on the production wire path.** Operators smoke-test `initialize` and `ping` through the same gateway subjects used after cutover without flipping a global fail-open flag first.
- **Consistent fail-closed culture.** Aligns with ADR 0003 bootstrap vs. mesh separation, STS/registry fail-closed paths, and [failure-mode-matrix.md](../identity/failure-mode-matrix.md) row 5 (missing bundle closed).
- **Clear audit and observability story.** Gated denies and bootstrap allows emit distinguishable audit fields; `/readyz`, metrics, and WARN logs give operators three independent detection paths.
- **Safe mid-session regression.** Bundle pointer removal or invalid signature reverts to day-zero for new requests without silent permanent fall-through; in-flight work drains on prior digest per ADR 0010.
- **Explicit rollback knob.** `bootstrap_mode` lets operators tighten to `deny` or `fail-start` during incidents without code changes.

### Negative

- **Adoption friction on first gated call.** Teams must complete the bootstrap sequence ([bootstrap-day-zero.md §3](../identity/bootstrap-day-zero.md)) before `tools/call` succeeds; partial state yields `-32108` with `missing` array rather than opaque errors.
- **Two method classes to document.** Integrators must know session/catalog vs. gated classification; misclassified custom methods may deny unexpectedly until bundle defines them.
- **SIEM noise during bring-up.** Bootstrap allows emit `bootstrap_mode: true`; operators need alert thresholds to distinguish bring-up from post-cutover misconfiguration.
- **Readiness vs. liveness split.** Default `audit` mode returns `/readyz` 200 while `tenant_initialised=false` — kubelet must read `bootstrap.day_zero`, not HTTP status alone, for tenant readiness gates.

### Mitigations

| Risk | Mitigation |
|---|---|
| Adoption friction | Documented bootstrap sequence with idempotent steps; `data.missing` array on denies; proposed `mcp.gateway.bootstrap.{tenant}.readiness` probe subject |
| Method classification errors | Pin default table in gateway code; bundle may extend gated set, not shrink platform defaults without explicit config |
| SIEM noise | Alert on sustained bootstrap allows after `tenant_initialised` expected true; rate-limited WARN for deny storms |
| Kubelet misconfiguration | Document `bootstrap.day_zero` in Block G; optional `MCP_GATEWAY_READYZ_STRICT=1` maps day-zero to 503 for teams that conflate ready with tenant initialised |

---

## Alternatives considered

### Refuse to start without a bundle (`fail-start` as the only behavior)

Gateway process exits or never binds the MCP request consumer until `mcp-gateway-config/bundle/active` points at a signature-valid revision.

| Assessment | |
|---|---|
| **Pros** | Strongest guarantee — no MCP request is ever evaluated without policy; simplest mental model for security reviewers. |
| **Cons** | **Adoption friction:** Kubernetes rolls gateway pods before bundle CI publishes; NATS queue group never forms; operators cannot run `initialize`/`ping` through the edge path to validate connectivity; couples gateway availability to bundle pipeline ordering. |
| **Verdict** | **Rejected as production default.** Retained as explicit **`bootstrap_mode=fail-start`** for regulated subsets and CI. Default remains **`audit`** so the process starts and operators get `/readyz` banner + audit trail while completing bootstrap. |

### Permanent fall-through when bundle missing (permissive / allow-all)

All gateway requests forward to backend regardless of readiness; audit optionally tags `bootstrap_mode: true`.

| Assessment | |
|---|---|
| **Pros** | Fastest local dev; minimal operator steps before first `tools/call`. |
| **Cons** | **Unacceptable security posture** — ungoverned egress for authorization-bearing methods; equivalent to leaving `MCP_GATEWAY_SPICEDB_ENDPOINT` unset in production; defeats purpose of signed bundles (ADR 0010) and hierarchical merge default deny (ADR 0013). |
| **Verdict** | **Rejected.** Permissive remains dev-only with startup guard against enforce mode; never the default or rollback target for production tenants. |

### Strict fail-closed on every method (including `initialize` / `ping`)

All gateway MCP requests deny with `tenant_uninitialised` until full readiness.

| Assessment | |
|---|---|
| **Pros** | Simplest single rule; no method classifier; strongest deny story for compliance questionnaires. |
| **Cons** | Forces operators to enable permissive or bypass gateway for smoke tests; breaks the bootstrap sequence's step e verification through production subjects. |
| **Verdict** | **Rejected as default.** Available as **`bootstrap_mode=deny`** (`MCP_GATEWAY_BOOTSTRAP_POSTURE=strict`) for pen-test and compromise hard reset ([bootstrap-day-zero.md §7.3](../identity/bootstrap-day-zero.md)). |

### Defer decision to per-deployment env chaos (no pinned default)

Each cluster chooses deny vs. allow-all via undocumented env combinations.

| Assessment | |
|---|---|
| **Pros** | No central ADR needed. |
| **Cons** | Phase 1 `MCP_GATEWAY_SPICEDB_ENDPOINT` unset behavior becomes accidental production config; failure-mode matrix row 5 cannot close; cross-region fleets diverge. |
| **Verdict** | **Rejected.** This ADR pins **`audit`** as default and documents the enum. |

---

## Implementation notes

### Configuration surface

| Variable | Values | Default | Meaning |
|---|---|---|---|
| **`bootstrap_mode`** **(proposed primary)** | `audit` \| `deny` \| `fail-start` | `audit` | Day-zero posture enum (this ADR) |
| `MCP_GATEWAY_BOOTSTRAP_POSTURE` | `mixed` \| `strict` \| `permissive` | unset → `audit` | Alias: `mixed`=audit, `strict`=deny; `permissive` dev-only |
| `MCP_GATEWAY_READYZ_STRICT` | `0` \| `1` | `0` | When `1`, `/readyz` returns 503 if `tenant_initialised=false` |
| `MCP_GATEWAY_AGENT_IDENTITY` | `off` \| `shadow` \| `enforce` | deployment-specific | `enforce` + `permissive` → startup error |

Precedence: explicit `bootstrap_mode` wins over `MCP_GATEWAY_BOOTSTRAP_POSTURE` when both set; conflicting values → startup error.

### Code and test surfaces

| Surface | Change |
|---|---|
| Gateway readiness evaluator | Implement `tenant_initialised` predicate + KV watches |
| Method classifier | Gated vs. session/catalog table pinned in ingress |
| Audit envelope | `extra.bootstrap_mode`, `extra.tenant_initialised`, `extra.readiness`, `extra.missing` |
| `rpc_codes` / JSON-RPC | `-32108` + `data.reason: tenant_uninitialised` |
| `/readyz` handler | `bootstrap` object + `warnings` array |
| Metrics | `mcp_gateway_bootstrap_mode_active`, readiness gauges |
| Tests | Extend `bundle_load_hot_reload.rs` with empty pointer at boot, pointer delete mid-session, `/readyz` banner fields |

### Relationship to bundle hot reload (ADR 0010)

| Scenario | Active in-memory bundle | New request posture |
|---|---|---|
| Cold start, no pointer | None | Day-zero `audit` — gated deny |
| Invalid v2, v1 good | v1 retained | v1 policy for evaluation; readiness may still fail other checks |
| Pointer deleted, v1 was good | v1 until explicit unload policy **(proposed: drop to none on pointer delete)** | Day-zero `audit` for new requests |
| First valid load | vN activated | Readiness recheck; gated allow when all checks pass |

**Pinned:** on **`bundle/active` key delete**, in-memory bundle is **cleared** for authorization purposes (not retained). In-flight requests on v1 still drain; this matches compromise hard reset semantics in [bootstrap-day-zero.md §7.3](../identity/bootstrap-day-zero.md). Failed parse/signature reload **retains** last good bundle per ADR 0010 — distinct from operator pointer deletion.

### Control-plane paths that remain available on day zero

Per [bootstrap-day-zero.md §1.4](../identity/bootstrap-day-zero.md):

- NATS KV PUT/GET for bootstrap artifacts
- Auth callout at CONNECT (bootstrap User JWT)
- `mcp.sts.exchange` (structured errors until registry + attestation succeed)
- `mcp.registry.agent.lookup` (miss until seeded)
- `mcp.control.discovery.register.{server_id}`
- JetStream audit consumers on `mcp.audit.>`

---

## Open questions

1. **Per-tenant bootstrap mode.** Some tenants may require **`bootstrap_mode=deny`** or **`fail-start`** from day one (enforce-from-day-zero) while the platform default remains `audit`. Deferred: tenant-scoped override in `mcp-gateway-config/{tenant}/bootstrap_mode` vs. fleet-wide env only ([ADR 0001](0001-tenancy-model.md) soft vs. hard tenancy key layout).

2. **`/readyz` HTTP status vs. banner-only warning.** Default pins 200 + JSON banner for `audit`; some operators may require 503 until `tenant_initialised`. `MCP_GATEWAY_READYZ_STRICT` proposed — finalize in Block G k8s doc.

3. **Exact backend config key shape.** Paper proposes `mcp-gateway-config/backend/{server_id}` — confirm in bundle loader ADR follow-on.

4. **Bootstrap probe subject.** `mcp.gateway.bootstrap.{tenant}.readiness` request/reply — implement in Block D or defer to Block G?

5. **Permissive mode lifecycle.** Sunset path for `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` and Phase 1 SpiceDB-unset allow-all — target removal milestone in Block E.

6. **Multi-tenant gateway instance.** Single process serving multiple tenants: per-tenant readiness map vs. global worst-case posture for `/readyz` aggregate.

---

## Rollback plan

If Mixed default proves too friction-heavy or too permissive in production:

1. **Immediate posture tighten (no redeploy logic change):** set **`bootstrap_mode=deny`** fleet-wide — all MCP methods deny until `tenant_initialised`; session/catalog smoke tests require temporary strict override removal or direct backend access.

2. **Hard stop:** set **`bootstrap_mode=fail-start`** — new pods refuse to serve MCP until bundle present; use during incident when any day-zero allow is unacceptable.

3. **Temporary loosen (dev/staging only):** `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` with `MCP_GATEWAY_AGENT_IDENTITY=off|shadow` — **not** permitted with enforce; audit every allow with `bootstrap_mode: true`.

4. **Config rollback without binary change:** all modes are env/KV-driven; no migration. Document runbook: flip `bootstrap_mode`, rolling restart gateway queue group, verify `/readyz` `bootstrap.mode` and metric `mcp_gateway_bootstrap_mode_active`.

5. **ADR supersession:** if default changes from `audit`, publish ADR 0021 amendment or successor; keep `tenant_initialised` predicate and audit field names stable for SIEM continuity.

6. **Feature gate (proposed):** `MCP_GATEWAY_DAY_ZERO_V2=1` guards readiness evaluator + mixed classifier until Block D ships; gate off reverts to Phase 1 behavior (documented dev-only hazard).

---

## Status of supporting work

| Work item | Status | Owner / anchor |
|---|---|---|
| Block C bootstrap/day-zero paper | **Done** | [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md) |
| Block C bootstrap/day-zero ADR | **Done** (this document) | `docs/adr/0021-bootstrap-day-zero.md` |
| Readiness evaluator + KV watches | **Pending** (Block D) | `trogon-mcp-gateway` |
| Mixed method classifier | **Pending** (Block D) | Ingress / policy gate |
| `-32108` + `tenant_uninitialised` | **Pending** (Block D) | `rpc_codes.rs` |
| `/readyz` bootstrap JSON fields | **Pending** (Block G) | [health_probes.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/health_probes.rs) |
| Audit `extra.bootstrap_mode` | **Pending** (Block D) | Audit publisher |
| `bootstrap_mode` config enum | **Pending** (Block D) | Gateway settings |
| Bundle unload → day-zero tests | **Pending** (Block D) | [bundle_load_hot_reload.rs](../../rsworkspace/crates/trogon-mcp-gateway/tests/bundle_load_hot_reload.rs) |

---

*Design context: [bootstrap-day-zero.md](../identity/bootstrap-day-zero.md). This ADR is the durable decision record for empty-bundle and partial-readiness posture; the paper spec remains the operator how-to and bootstrap sequence.*

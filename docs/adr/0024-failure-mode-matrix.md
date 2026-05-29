# ADR 0024: Failure-Mode Matrix

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block C item 4 (Failure-mode matrix); unblocks Block E failure-mode wiring, operator runbooks, and SIEM correlation for dependency outages |
| **Related** | [failure-mode-matrix.md](../identity/failure-mode-matrix.md); [reference-error-codes.md](../identity/reference-error-codes.md); [multi-region.md](../identity/multi-region.md); [nats-callout-plugin.md](../identity/nats-callout-plugin.md); [ADR 0011](0011-nats-auth-callout.md); [ADR 0012](0012-rate-limit-state.md); [ADR 0016](0016-multi-region-topology.md) |

## Context

The MCP gateway sits on a hot path between external clients and **N** backend MCP servers. Every request traverses a dependency chain that can fail independently:

| Dependency | Role on the hot path | Typical failure modes |
|------------|----------------------|------------------------|
| **NATS** (core + JetStream) | Ingress queue group, backend forward, audit publish, KV for sessions/config | Partition, subscription loss, broker slow, stream full |
| **SpiceDB** | PDP for gated methods (`tools/call`, `resources/read`, …) | gRPC unreachable, deadline exceeded, stale `ZedToken` |
| **IdP / OAuth issuer** | Perimeter auth via NATS auth-callout and HTTP ingress JWT verify | JWKS fetch failure, introspection timeout, issuer outage |
| **STS** (`trogon-sts`) | Mesh token mint on egress in enforce mode | `mcp.sts.exchange` timeout, signing key unavailable |
| **Bundle store** (`mcp-gateway-config` KV) | Active policy pointer, CEL/WASM bundle, trust anchors | Missing pointer, invalid signature on hot-swap, compile error at load |
| **Backend MCP servers** | Tool execution on `mcp.server.{server_id}.>` | Timeout, malformed JSON-RPC, empty queue group |
| **Gateway worker itself** | Policy evaluation, inflight semaphores, audit side channel | Saturation, WASM trap, CEL runtime fault |

Without a written, normative matrix, each crate and operator runbook improvises **fail-open** vs **fail-closed** per incident. That produces inconsistent security posture (silent allows on SpiceDB outage in one replica, hard denies in another), incompatible JSON-RPC codes for the same root cause, and audit gaps that break SIEM playbooks.

The design spec [failure-mode-matrix.md](../identity/failure-mode-matrix.md) (Block C paper) defines sixteen failure classes, three behaviour modes, JSON-RPC mappings, audit subjects, operator override knobs, and ten invariants. [reference-error-codes.md](../identity/reference-error-codes.md) pins the client-visible `-32100` … `-32199` contract. [ADR 0016](0016-multi-region-topology.md) adds regional partition semantics (local tenants continue; cross-region control-plane work degrades). This ADR records the **durable decision** to adopt that matrix as the canonical operator and implementation reference.

**What stays undecided and breaks downstream work if this ADR is not accepted:**

- Block E cannot wire consistent `-32107` / `-32102` / `-32105` responses per dependency class without forking per-handler behaviour.
- On-call runbooks lack a single table mapping trigger → mode → code → audit subject → config knob.
- Multi-region partition handling ([ADR 0016](0016-multi-region-topology.md)) cannot reference row-level readiness behaviour for gateway `/ready`.
- Per-method fail-open exceptions (`tools/list` vs `tools/call`) remain implicit in CEL comments instead of an explicit follow-on ADR scope.

**Current branch snapshot (Phase 1):**

- Rows 1, 8, 13 (STS mint path), 14, 15, 16 (library partial) are partially or fully implemented in `trogon-mcp-gateway`.
- Rows 3–7, 9–12, and full row 10 saturation wiring are Phase 2–3.
- When implementation diverges from the matrix, code and [failure-mode-matrix.md](../identity/failure-mode-matrix.md) must change in the same change set (Block C completion criterion).

---

## Decision

**Adopt [failure-mode-matrix.md](../identity/failure-mode-matrix.md) as the normative failure-mode matrix for the MCP gateway.** All gateway workers, auth-callout handlers, and regional STS replicas MUST classify dependency failures into one of three behaviour modes and apply the default row unless an documented operator override applies.

### Behaviour modes (pinned)

| Mode | Definition |
|------|------------|
| **CLOSED** | Request **denied** before side effects (no backend publish, no egress mint, no credential propagation). Client receives a Trogon JSON-RPC error (`-32100` … `-32199`). Audit envelope with outcome `deny` or `error` when the worker reached the request path. |
| **OPEN-with-audit** | Request **proceeds** while failure is recorded. Used only where availability is explicitly traded for observability — **never** for authorization on gated methods (`tools/call`, `resources/read`, egress mint in enforce mode). |
| **DEGRADED** | Gateway instance **stops accepting or completing** a class of work (subscription loss, partial fleet), or serves a reduced path (stale read model) without granting new privileged access. Operators treat as infra/outage, not per-request policy. |

Wire audit subjects use **`{prefix}.audit.{outcome}.{direction}.{method_root}`** (default prefix `mcp`). Control-plane events use `mcp.control.*` subjects as specified in the paper.

### Pinned per-dependency defaults (task-critical rows)

These rows are the minimum contract for Block C item 4; full matrix below includes all sixteen classes from the paper.

| Dependency / trigger | Default mode | Client / probe signal | Operator override |
|----------------------|--------------|----------------------|-------------------|
| **SpiceDB unreachable** | **CLOSED** | `-32107` `authz_unreachable` | **Default:** deny. **Per-tenant override (proposed):** `{tenant}/spicedb.unreachable_mode: allow_with_audit` in `mcp-gateway-config` KV — OPEN-with-audit for **non-gated** methods only; gated methods remain CLOSED regardless of override. Dev-only: unset `MCP_GATEWAY_SPICEDB_ENDPOINT` ⇒ Phase 1 allow-all **(today)** — not a production fail-open knob. |
| **WASM policy bundle panic** | **CLOSED** | `-32101` `policy_fault` **(proposed)** | Repoint active bundle in KV; `mcp.control.bundle.reload`; disable Tier-3 in manifest **(proposed)**. |
| **Bundle signature invalid (hot-swap / startup)** | **CLOSED** (no new bundle) | `/ready` false **(proposed)**; `-32108` `no_policy` if no prior bundle **(proposed)** | Trust anchor list in KV `trusted_signers`; **keep last good bundle pinned**; alert on `mcp.control.bundle.load_failed` **(proposed)**. |
| **Backend MCP server timeout** | **CLOSED** | `-32102` `backend_timeout` | `operation_timeout` via `mcp-nats` env; per-server deadline in bundle/KV **(proposed)**. |
| **Gateway worker saturated** | **CLOSED** | `-32105` `rate_limited` | Inflight caps per [ADR 0012](0012-rate-limit-state.md): bundle/KV `inflight.server`, `inflight.tenant`; adaptive-access throttle **(today)**. |
| **NATS partition** | **DEGRADED** (instance) | HTTP `/ready` → **503** for cross-region / supercluster-dependent work **(proposed)**; in-region tenants with last-known mirror continue per [ADR 0016](0016-multi-region-topology.md) | NATS reconnect policy; scale replicas across failure domains; no fail-open subscription. |
| **IdP unreachable** (auth-callout / HTTP ingress) | **CLOSED** on cache miss; **cached path** on hit | Callout: NATS authorization error; HTTP: `-32107` or `-32118` `auth_required` class | Per-token cache keyed `{tenant}:{auth_method}:{token_hash}`; TTL `min(token_exp - now, MCP_CALLOUT_CACHE_MAX_TTL_SECS)` per [ADR 0011](0011-nats-auth-callout.md); **bounded TTL** caps blast radius — no unbounded offline grace in production. |

### Full failure-mode matrix (normative)

Reproduced from [failure-mode-matrix.md](../identity/failure-mode-matrix.md). Implementation phase markers **(today)** / **(proposed)** preserved from the paper.

| # | Class | Failure trigger | Default behaviour | JSON-RPC code | Audit subject | Override knob |
|---|-------|-----------------|-------------------|---------------|---------------|---------------|
| 1 | SpiceDB unreachable | gRPC to `MCP_GATEWAY_SPICEDB_ENDPOINT` fails | **CLOSED** | `-32107` | `{prefix}.audit.error.request.{method_root}` | Per-tenant `allow_with_audit` for non-gated only **(proposed)**; dev unset endpoint |
| 2 | SpiceDB inconsistent ZedToken | Cached token rejected (`at_least_as_fresh`) | **CLOSED** (target); optional retry | `-32107` **(today)** | same | `spicedb.consistency_on_stale_token` in bundle **(proposed)** |
| 3 | WASM bundle panic | Tier-3 trap during evaluation | **CLOSED** | `-32101` **(proposed)** | `{prefix}.audit.error.request.{method_root}` | Bundle rollback; disable Tier-3 |
| 4 | WASM bundle signature invalid | Hot-swap NKey mismatch | **CLOSED**; keep last good | N/A request path; `/ready` false **(proposed)** | `mcp.control.bundle.load_failed` **(proposed)** | `trusted_signers`; pin last good |
| 5 | WASM bundle missing | No active bundle for tenant | **CLOSED** | `-32108` **(proposed)** | `{prefix}.audit.deny.request.{method_root}` | Bootstrap bundle in KV |
| 6 | CEL compile error (load) | `Program::compile` fails at reload | **CLOSED** (unhealthy instance) | N/A at request path **(proposed)** | `mcp.control.policy.compile_failed` **(proposed)** | Fix bundle; rollback pointer |
| 7 | CEL runtime error | Eval error on request path | **CLOSED** | `-32101` **(proposed)** | `{prefix}.audit.error.request.{method_root}` | Per-rule `on_error: deny \| log` **(proposed)**; default `deny` |
| 8 | Backend MCP timeout | No NATS reply within deadline | **CLOSED** | `-32102` | `{prefix}.audit.error.request.{method_root}` | Operation timeout env / bundle |
| 9 | Backend malformed JSON-RPC | Invalid reply shape | **CLOSED** (target) | `-32116` **(proposed)**; pass-through **(today)** | `{prefix}.audit.error.request.{method_root}` | `gateway.validate_backend_jsonrpc` **(proposed)** |
| 10 | Gateway saturated | Inflight / queue depth above cap | **CLOSED** | `-32105` | `{prefix}.audit.deny.request.{method_root}` **(proposed)** | Inflight caps [ADR 0012](0012-rate-limit-state.md) |
| 11 | NATS partition | Core/JS connectivity loss; queue-group drop | **DEGRADED** | N/A ingress | Last-known audit before partition **(proposed)** | Reconnect; multi-AZ replicas |
| 12 | JetStream KV unavailable | Session bucket timeout / denied | **CLOSED** for session RPCs | `-32101` or `-32107` **(proposed)** | `{prefix}.audit.error.request.{method_root}` | Tenancy key layout; session-affinity **(proposed Phase 3)** |
| 13 | STS unreachable / mesh JWKS stale | Mint timeout; JWKS fetch fail | **CLOSED** enforce; shadow ingress metrics | `-32107` mint; `-32106`/`-32110` JWT | `{prefix}.audit.error.*`; `mcp.audit.sts.error` | `MCP_GATEWAY_AGENT_IDENTITY=shadow`; trust bundle KV watch |
| 14 | Audit publisher backlog | JetStream publish fail | **OPEN-with-audit** (request proceeds) | N/A | None if publish fails **(today)** | Stream retention; `MCP_GATEWAY_AUDIT_REQUIRED=1` **(proposed)** for regulated tenants |
| 15 | Trust bundle missing | No PEM for SPIFFE trust domain | **CLOSED** | `-32107` / `-32110` | `mcp.audit.sts.deny` / `error` | Publish to `mcp-trust-bundles/{domain}` |
| 16 | Approval timeout | No decision before TTL | **CLOSED** | `-32107` `approval_required` | `{prefix}.audit.deny.request.{method_root}` **(proposed)** | `approval_ttl_secs`; risk thresholds |

### Invariants (non-negotiable)

From [failure-mode-matrix.md § Invariants](../identity/failure-mode-matrix.md#invariants):

1. **No allow without PDP** — Gated methods require successful SpiceDB check (or explicit Phase 1 dev bypass with endpoint unset).
2. **No allow cache past trust** — Cached allows must not outlive ZedToken, trust bundle, or approval freshness.
3. **No credential propagation on deny** — CLOSED paths must not publish to `mcp.server.*`, mint mesh tokens, or forward bearer credentials in enforce mode.
4. **Audit outcome matches decision** — `allow` only after all applicable gates succeed.
5. **Enforce mode is strict** — STS or trust failures are CLOSED, not shadow-allowed on egress.
6. **Approval is explicit** — Timeout is deny, not allow.
7. **Backend timeout is not allow** — Missing reply never becomes synthetic success.
8. **Signed policy only** — Invalid signature never replaces active bundle; last good bundle remains authoritative.
9. **CEL/WASM fault defaults closed** on mutating methods; OPEN-with-audit only for explicitly marked non-mutating rules (e.g. `*/list` shaping).
10. **Audit loss does not relax authz** — Row 14 OPEN-with-audit applies to audit side channel only.

### JSON-RPC quick reference

Authoritative table: [reference-error-codes.md §3](../identity/reference-error-codes.md). Constants in `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs`.

| Code | Symbol | Matrix rows |
|------|--------|-------------|
| `-32101` | `policy_fault` | 3, 6, 7, 12 **(proposed wire)** |
| `-32102` | `backend_timeout` | 8 |
| `-32105` | `rate_limited` | 10 |
| `-32107` | `authz_unreachable` / `approval_required` | 1, 2, 13, 16 — disambiguate via `error.data` |
| `-32108` | `no_policy` | 4, 5 **(proposed wire)** |
| `-32116` | `backend_malformed` | 9 **(proposed wire)** |

**Note on `-32107`:** Same code serves `authz_unreachable` and `approval_required`; clients MUST disambiguate via `error.message` and `error.data` ([failure-mode-matrix.md](../identity/failure-mode-matrix.md#behaviour-modes)).

### NATS partition and multi-region readiness

When a gateway instance loses NATS connectivity or supercluster mirror lag exceeds SLO ([multi-region.md](../identity/multi-region.md)):

- **In-region tenants** with last-known policy bundle, JWKS, and registry mirror **continue to be served** on local `mcp.gateway.request.>` — fail-closed relative to **new** revocations, not fail-open to anonymous access ([ADR 0016](0016-multi-region-topology.md)).
- **Cross-region work** (global registry write visibility, fresh mesh mint depending on global issuer, mirror-dependent config) is **DEGRADED**; HTTP **`/ready` returns 503** **(proposed)** so load balancers drain instances that cannot meet cross-region SLOs while regional MCP paths may still process with stamped config age.
- Per-request CLOSED applies once a worker accepts a message it cannot complete (e.g. session KV required but unavailable — row 12).

### IdP unreachable and auth-callout cache

Per [ADR 0011](0011-nats-auth-callout.md) and [nats-callout-plugin.md](../identity/nats-callout-plugin.md):

| Condition | Behaviour |
|-----------|-----------|
| Cache **hit** (valid entry, within TTL) | Skip IdP round-trip; mint User JWT; audit `cache_hit=true` |
| Cache **miss** + IdP unreachable | **CLOSED** — deny CONNECT / HTTP 401–503 class |
| OAuth token expired / revoked | **CLOSED** — deny regardless of cache |
| TTL formula | `min(token_exp - now, MCP_CALLOUT_CACHE_MAX_TTL_SECS)` — default max cache **60 s**; User JWT cap **300 s** |

Production MUST NOT enable unbounded offline grace (`MCP_CALLOUT_OIDC_OFFLINE_GRACE` dev only). Bounded TTL is the availability/security trade-off for IdP outages.

---

## Consequences

### Positive

- **Single operator table.** SRE and on-call can map any dependency outage to mode, JSON-RPC code, audit subject, and config knob without reading Rust sources.
- **Reduced incident handling time.** Runbooks align on `-32107` vs `-32102` vs `-32105` instead of ad-hoc HTTP status or opaque NATS errors.
- **Security consistency.** Gated methods fail CLOSED on PDP, WASM, and trust failures; OPEN-with-audit is explicit and auditable, not accidental.
- **Client contract stability.** [reference-error-codes.md](../identity/reference-error-codes.md) and the matrix share one source of truth for automation (retry rules, SIEM alerts).
- **Multi-region coherence.** Partition behaviour references [ADR 0016](0016-multi-region-topology.md) — local serve vs cross-region 503 — without inventing per-region fail-open authz.

### Negative

- **Operational complexity.** Sixteen rows × override knobs × phase markers require keeping paper spec, ADR, and code in sync.
- **Partial Phase 1 coverage.** Several **(proposed)** codes (`-32101`, `-32108`, `-32116`) and `/ready` semantics are not yet in `rpc_codes.rs`; clients must tolerate message-based disambiguation for `-32107`.
- **Per-tenant SpiceDB fail-open tension.** Regulated tenants expect default CLOSED; optional `allow_with_audit` must be guarded so gated methods never silently allow — misconfiguration is a security incident class.
- **Audit loss on row 14.** Default OPEN-with-audit on audit backlog means request may succeed without JetStream record unless `MCP_GATEWAY_AUDIT_REQUIRED=1` **(proposed)**.

### Mitigations

| Risk | Mitigation |
|------|------------|
| SpiceDB fail-open misconfig | Override ignored for gated methods; config validation in bundle linter **(proposed)**; audit `decision_reason=spicedb_unreachable_allow_with_audit` |
| Code / matrix drift | Block C criterion: same change set updates [failure-mode-matrix.md](../identity/failure-mode-matrix.md) and tests |
| `-32107` ambiguity | Structured `error.data.reason` distinguishes `authz_unreachable` vs `approval_required` |
| NATS partition blind spot | Heartbeat to `mcp.control.gateway.heartbeat.{instance_id}` **(proposed)**; mirror lag alarms per [multi-region.md](../identity/multi-region.md) |
| IdP cache staleness | Short TTL; denylist before cache promotion; `auth.revoke` **(proposed)** per ADR 0011 |

### Operator quick reference (SRE)

| Symptom | Likely row | First actions |
|---------|------------|---------------|
| Spike in `-32107` on `tools/call` | 1, 2 | Check SpiceDB replica health; verify gRPC endpoint; review ZedToken cache |
| `-32102` on one `server_id` | 8 | Increase timeout or fix backend; check queue group on `mcp.server.{id}.>` |
| `-32105` cluster-wide | 10 | Raise inflight caps temporarily; scale gateway replicas; check [ADR 0012](0012-rate-limit-state.md) KV |
| `/ready` 503, regional traffic OK | 11 | NATS partition or mirror lag; verify supercluster routes; drain bad instances |
| Bundle load alerts, old policy still active | 4 | Invalid signature rejected; confirm last good pinned; rotate trust anchors |
| CONNECT storms during IdP outage | IdP cache | Expect deny on cache miss; extend cache TTL only within `MCP_CALLOUT_CACHE_MAX_TTL_SECS` cap |

---

## Alternatives considered

### Fail-open globally

On any dependency failure (SpiceDB, STS, bundle, backend), the gateway would proceed with the request and log a warning.

| Assessment | |
|------------|---|
| **Rejected because** | Violates security posture for gated MCP methods: SpiceDB outage would allow arbitrary `tools/call`, WASM panic would skip tenant policy, and trust bundle absence would accept forged workload identity. Availability gains on read-only paths do not justify cluster-wide authorization bypass. OPEN-with-audit is permitted only per-row and per-method class, never as a global default. |

### Fail-closed globally

Every dependency failure, including audit publish backlog and non-security list shaping, would hard-deny the client request.

| Assessment | |
|------------|---|
| **Rejected because** | Over-denies availability on non-security-critical paths: audit JetStream slowness (row 14) would block all MCP traffic; IdP cache hits would be useless if any auxiliary check failed; `tools/list` shaping could not degrade gracefully when PDP is slow for non-mutating catalog reads. Regulated tenants may opt into audit fail-closed via `MCP_GATEWAY_AUDIT_REQUIRED=1` **(proposed)** without forcing that posture on all tenants. |

### Per-handler ad-hoc behaviour (no matrix)

Each gateway subsystem chooses fail mode independently at implementation time without a published spec.

| Assessment | |
|------------|---|
| **Rejected because** | Produced the inconsistent behaviour this ADR eliminates; blocks SIEM correlation, client SDK retry logic, and Block E wiring. The paper spec already exists; formalizing it is lower cost than continuing implicit defaults. |

### External circuit-breaker sidecar (Envoy / standalone)

Out-of-process breakers would wrap NATS and gRPC calls without gateway-native codes.

| Assessment | |
|------------|---|
| **Rejected as primary model** | Breakers do not emit Trogon JSON-RPC `-321xx` or audit envelopes; hybrid deployments may use L7 health checks ([on-bus-vs-hybrid.md](../identity/on-bus-vs-hybrid.md)) but the **authoritative** failure semantics remain in-gateway per this matrix. |

---

## Open questions

1. **Per-method overrides** — Should `tools/list` fail-open with audit while `tools/call` fails-closed when SpiceDB is unreachable, without a full tenant-level override? Bundle syntax for method-scoped `on_spicedb_unreachable` is **(proposed)**; default remains row 1 CLOSED until a follow-on ADR or bundle schema pins precedence (tenant override vs method override vs hierarchical merge [ADR 0013](0013-hierarchical-policy-merge.md)).

2. **SpiceDB per-tenant `allow_with_audit` scope** — Confirm whether non-gated methods include `ping`, `initialize`, and shadow ingress only, or also `tools/list` when CEL marks list shaping as non-authoritative.

3. **`/ready` vs `/live` split** — Row 4 and row 11 propose `/ready` false; liveness may remain true for process supervision. Exact probe contract is Block G operator doc work.

4. **Regulated tenant audit fail-closed** — Default row 14 is OPEN-with-audit; `MCP_GATEWAY_AUDIT_REQUIRED=1` **(proposed)** needs tenancy-scoped CRD projection alongside `MCPTenant` residency ([ADR 0016](0016-multi-region-topology.md)).

5. **Backend malformed JSON-RPC (row 9)** — Strict validation toggle vs pass-through **(today)**; migration timeline and client impact for `-32116`.

6. **Session KV hard dependency (row 12)** — Phase 3 session-affinity may reduce KV requirement; until then, which RPCs are session-bound vs stateless?

7. **Cross-region client retry** — When `/ready` is 503 for mirror lag, may clients fail over to another region per tenant policy? **(proposed)** in [multi-region.md](../identity/multi-region.md); not pinned here.

---

## Rollback plan

Each failure mode exposes an **independent config knob**; rollback is **per-mode**, not a single global flag. Operators revert one row without redeploying unrelated behaviour.

| Row / class | Rollback knob | Rollback action |
|-------------|---------------|-----------------|
| 1 SpiceDB unreachable | `MCP_GATEWAY_SPICEDB_ENDPOINT`; `{tenant}/spicedb.unreachable_mode` | Restore endpoint; set mode to `deny` (default); remove per-tenant `allow_with_audit` |
| 2 Stale ZedToken | `spicedb.consistency_on_stale_token` in bundle | Set to `deny`; rolling restart to clear process cache |
| 3 WASM panic | Active bundle pointer in KV | Repoint to last known good version; disable Tier-3 in manifest |
| 4 Invalid signature | `trusted_signers`; bundle version | Revert pointer; remove bad artifact; confirm last good pinned |
| 5 Missing bundle | Bootstrap bundle key | Install org base + tenant overlay per bootstrap-day-zero spec |
| 6–7 CEL fault | Bundle version; `on_error` per rule | Rollback bundle; set `on_error: deny` |
| 8 Backend timeout | `operation_timeout` env / bundle | Increase deadline temporarily; revert after backend fix |
| 9 Malformed backend | `gateway.validate_backend_jsonrpc` | Set `false` to restore pass-through **(today)** behaviour |
| 10 Saturation | `inflight.server`, `inflight.tenant`, rate KV | Raise caps per [ADR 0012](0012-rate-limit-state.md); scale replicas |
| 11 NATS partition | Reconnect / scale | No fail-open knob; heal connectivity; remove instance from LB via `/ready` |
| 12 Session KV | Tenancy bucket layout | Fail over to healthy JS domain; defer session RPCs until KV healthy |
| 13 STS / JWKS | `MCP_GATEWAY_AGENT_IDENTITY=shadow` (ingress only) | Temporary shadow for observability — **does not** rollback egress enforce mint CLOSED |
| 14 Audit backlog | Stream limits; `MCP_GATEWAY_AUDIT_REQUIRED` | Raise retention; disable audit-required fail-closed if wrongly enabled |
| 15 Trust bundle | `mcp-trust-bundles/{domain}` KV | Republish PEM; file bootstrap `MCP_STS_TRUST_BUNDLE_PATH` |
| 16 Approval | `approval_ttl_secs` | Extend TTL for incident window; revert after approval pipeline healthy |
| IdP cache | `MCP_CALLOUT_CACHE_MAX_TTL_SECS` | Lower TTL to reduce stale allow window; denylist promotion for revoked tokens |

**Feature-gate pattern:** New **(proposed)** behaviours ship behind bundle keys or env vars defaulting to matrix defaults in this ADR. Disabling a gate restores prior Phase 1 behaviour documented as **(today)** in [failure-mode-matrix.md](../identity/failure-mode-matrix.md).

**Deprecation:** Removing a row default requires a new ADR and a migration note in [reference-error-codes.md](../identity/reference-error-codes.md); clients depend on stable `-321xx` codes.

---

## Implementation notes

### Code and config surfaces

| Surface | Matrix responsibility |
|---------|----------------------|
| `trogon-mcp-gateway` ingress / worker | Rows 1–10, 12–13 gateway path; JSON-RPC mapping |
| `a2a-auth-callout` / embedded callout | IdP cache, deny on miss, bounded TTL (ADR 0011) |
| `trogon-sts` | Rows 13, 15; `mcp.audit.sts.*` |
| `mcp-gateway-config` KV | Bundle pointer, `spicedb.unreachable_mode`, inflight caps, `trusted_signers` |
| `rpc_codes.rs` | Add **(proposed)** constants `-32101`, `-32108`, `-32116` in Block E |
| Health endpoints | `/ready` reflects rows 4, 6, 11 **(proposed Block G)** |

### Implementation phases (from paper)

| Phase | Rows principally affected |
|-------|---------------------------|
| Phase 1 **(today)** | 1, 8, 13 (STS mint), 14, 15 (via STS), 16 (library partial); partial 7, 10 |
| Phase 2 | 2, 6, 7, 10, 12 |
| Phase 3 | 3, 4, 5, 9, bundle distribution |

### Status of supporting work

| Work item | Status | Owner / anchor |
|-----------|--------|----------------|
| Block C failure-mode matrix ADR | **Done** (this document) | `docs/adr/0024-failure-mode-matrix.md` |
| Normative design spec | **Done** | [failure-mode-matrix.md](../identity/failure-mode-matrix.md) |
| JSON-RPC reference | **Done** | [reference-error-codes.md](../identity/reference-error-codes.md) |
| Multi-region partition semantics | **Done** | [ADR 0016](0016-multi-region-topology.md) |
| Auth-callout IdP cache | **Accepted** | [ADR 0011](0011-nats-auth-callout.md) |
| Rate-limit saturation codes | **Accepted** | [ADR 0012](0012-rate-limit-state.md) |
| Wire proposed RPC constants | **Pending** (Block E) | `rpc_codes.rs` |
| `/ready` 503 on partition | **Pending** (Block G) | Gateway health handler |
| Per-tenant `spicedb.unreachable_mode` | **Pending** (Block E) | Bundle schema + KV projection |
| Close `MCP_GATEWAY_PLAN.md` Block C item 4 checkbox | **Pending** | Editorial after ADR acceptance |

---

*Design context: [failure-mode-matrix.md](../identity/failure-mode-matrix.md). This ADR is the durable decision record; the paper spec remains the living operator reference and must stay aligned with implementation.*

# How to investigate an audited `tools/call` denial

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** MCP gateway operators triaging "my agent cannot call tool X" reports using audit stream, structured logs, and traces only.

**Related:** [Audit envelope reference](reference-audit-envelope.md) · [JSON-RPC error codes](reference-error-codes.md) · [Failure mode matrix](failure-mode-matrix.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [How to write a policy bundle](howto-write-bundle.md)

This guide walks from a user-reported JSON-RPC failure to a root-cause class (policy deny, SpiceDB miss, hierarchical overlay, rate limit, identity/STS failure, or infra error) and the correct fix or escalation. It does not teach JSON-RPC, JetStream, or SpiceDB internals.

| Placeholder | Example value |
|---|---|
| JSON-RPC `id` | `99` or `"req-20260528-001"` |
| Incident time (UTC) | `2026-05-28T14:02:04Z` |
| Tenant | `acme` |
| `server_id` | `github` |
| Tool name | `delete_repository` |
| NATS subject prefix | `mcp` (`MCP_PREFIX`) |
| Audit stream | `MCP_AUDIT` (`MCP_GATEWAY_AUDIT_STREAM`) |

Replace placeholders before running commands.

---

## Goal

You will classify why a specific `tools/call` was denied, tie that classification to one JetStream audit record and (when present) an OpenTelemetry trace, and choose the right remediation: bundle change, cache/session reset, identity fix, or PDP escalation.

---

## When to use this

Use this runbook when:

- A client or SDK reports a Trogon JSON-RPC error on `tools/call` with code `-32100` through `-32109` (or implementation extensions `-32110` and above per [reference-error-codes.md §9](reference-error-codes.md#9-implementation-extensions-outside-pin-6)).
- You need to prove whether the gateway denied before backend publish (audit `outcome: "deny"` or `"error"`).
- SIEM or support tickets include only `error.data.trace_id` or JSON-RPC `id`, not full NATS subjects.

Do **not** use this guide for:

- CONNECT-time or bootstrap JWT failures — see [overview.md](overview.md) and [nats-callout-plugin.md](nats-callout-plugin.md).
- STS exchange denials on `mcp.sts.exchange` — see [sts-exchange.md](sts-exchange.md) and audit family `mcp.audit.sts.*` in [reference-audit-envelope.md §3.2](reference-audit-envelope.md#32-sts).
- Third-party server registration or sidecar wiring — see [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md).
- MCP protocol tutorials — see [overview.md](overview.md).

---

## Prerequisites

Confirm before Step 1:

1. **Incident identifiers from the reporter:** JSON-RPC `id`, approximate UTC timestamp, `server_id`, tool name (`params.name`), tenant or agent id if known, and `error.data.trace_id` when the client surfaced it ([reference-error-codes.md §2](reference-error-codes.md#2-trace_id-requirement)).

2. **NATS operator access** to the cluster where the gateway publishes audit (backend zone creds with JetStream read on `MCP_AUDIT`).

3. **Audit stream exists:**

   ```bash
   export NATS_URL="${NATS_URL:-nats://127.0.0.1:4222}"
   export MCP_PREFIX="${MCP_PREFIX:-mcp}"
   export MCP_GATEWAY_AUDIT_STREAM="${MCP_GATEWAY_AUDIT_STREAM:-MCP_AUDIT}"

   nats stream info "${MCP_GATEWAY_AUDIT_STREAM}"
   ```

   Expected: stream present, subject filter includes `{prefix}.audit.>` ([reference-audit-envelope.md §4](reference-audit-envelope.md#4-gateway-audit-envelope), [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md)).

4. **Environment defaults for the worked example:**

   ```bash
   export JSONRPC_ID=99
   export INCIDENT_TS="2026-05-28T14:02:04Z"
   export TENANT=acme
   export SERVER_ID=github
   export TOOL_NAME=delete_repository
   ```

---

## Steps

### 1. Capture the JSON-RPC correlation from the user

1. Ask for the exact JSON-RPC `id` from the error response (string or number — the audit field `request_id` echoes it per [reference-audit-envelope.md §4.1](reference-audit-envelope.md#41-payload-fields-wire-today)).
2. Record UTC time of the failure (client clock or support ticket).
3. Copy `error.code`, `error.message`, and full `error.data` (must include `trace_id` on gateway-emitted errors per [reference-error-codes.md §2](reference-error-codes.md#2-trace_id-requirement)).
4. Note `server_id`, `method` (`tools/call`), and `params.name` (tool).

**Self-check:** You have `JSONRPC_ID`, `INCIDENT_TS`, and at least one of `trace_id` or tool/server context.

---

### 2. Query the `MCP_AUDIT` JetStream stream

Gateway denials for `tools/call` publish on subjects `{prefix}.audit.{outcome}.request.tools` where `{outcome}` is `deny` or `error` ([reference-audit-envelope.md §3.1](reference-audit-envelope.md#31-gateway)).

1. Confirm stream health and retention:

   ```bash
   nats stream info "${MCP_GATEWAY_AUDIT_STREAM}"
   ```

   Expected: `Subjects` includes `mcp.audit.>` (or your `{prefix}.audit.>`).

2. Tail recent deny traffic on the tools method root (interim — until `trogon-gateway-ctl` audit query ships per [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md)):

   ```bash
   nats sub "${MCP_PREFIX}.audit.deny.request.tools" \
     --stream "${MCP_GATEWAY_AUDIT_STREAM}" \
     --since 30m
   ```

   In another terminal, filter captured JSON for your id:

   ```bash
   jq -c --arg id "${JSONRPC_ID}" \
     'select(.request_id == ($id | tonumber) or .request_id == $id)'
   ```

3. If the client reported `-32107` / backend errors, also tail `mcp.audit.error.request.tools` on the same stream.

**Self-check:** One envelope near `INCIDENT_TS` with `jsonrpc_method == "tools/call"` and `subject_in` matching `mcp.gateway.request.${SERVER_ID}`. No match ⇒ see [failure-mode-matrix.md](failure-mode-matrix.md) row 14 (audit backlog).

---

### 3. Read the audit envelope fields

Parse the payload against [reference-audit-envelope.md §4.1](reference-audit-envelope.md#41-payload-fields-wire-today). Wire fields today:

| Field | Use in investigation |
|---|---|
| `outcome` | `deny` = policy/authz class; `error` = dependency/backend class |
| `subject_in` | Ingress subject; confirms `server_id` and method dots |
| `subject_out` | Rewritten backend subject; empty or unchanged on deny paths that never forward |
| `jsonrpc_method` | Must be `tools/call` |
| `tenant` | Tenant slug from JWT |
| `caller_sub` | Principal denied |
| `identity_source` | `jwt`, `legacy_header`, or `anonymous` |
| `request_id` | Must match Step 1 `JSONRPC_ID` |
| `agent_id`, `wkl`, `purpose`, `act_chain` | Identity and delegation context when populated |

**Proposed** when present: `decision_reason`, `rules_fired`, `trace_id`, `policy_ids` ([reference-audit-envelope.md §2.3](reference-audit-envelope.md#23-well-known-decision_reason-values)). Denied calls must not forward to `mcp.server.${SERVER_ID}.*` ([failure-mode-matrix.md § Invariants](failure-mode-matrix.md#invariants)).

---

### 4. Branch on the client JSON-RPC error code

Map `error.code` using [reference-error-codes.md §3](reference-error-codes.md#3-full-code-table-wire-format-pin-6) and [failure-mode-matrix.md § JSON-RPC code quick reference](failure-mode-matrix.md#json-rpc-code-quick-reference).

| Code range | Symbol | Root-cause class | Next action |
|---|---|---|---|
| `-32100` | `policy_deny` | CEL/WASM deny or SpiceDB `PERMISSIONSHIP_DENIED` | §4a — inspect `error.data.rule_fired`, `reason`; audit `deny`; check SpiceDB tuples and bundle rules |
| `-32101` | `policy_fault` | CEL runtime error, bundle load fault | §4a — bundle rollback ([howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback)); matrix row 6–7 |
| `-32102` | `backend_timeout` | Backend lane slow | Not a policy deny — backend/SRE path; audit `error` |
| `-32103` | `backend_unreachable` | No consumer on `mcp.server.{server_id}.>` | Integration runbook; audit `error` |
| `-32104` | `schema_unknown` | Schema cache miss for redaction | §4c — schema cache / `tools/list` |
| `-32105` | `rate_limited` | Token bucket or inflight cap | §4d — [ADR 0012](../adr/0012-rate-limit-state.md), [rate-limiting.md](rate-limiting.md) |
| `-32106` | `auth_expired` | JWT/session expired | §4e — re-mint mesh token |
| `-32107` | `authz_unreachable` **or** `approval_required` | SpiceDB/PDP down **or** HITL pending | Disambiguate `error.data` ([failure-mode-matrix.md](failure-mode-matrix.md) note on `-32107`) |
| `-32108` | `no_policy` | No active bundle / default deny | [bootstrap-day-zero.md](bootstrap-day-zero.md), bundle promotion |
| `-32109` | `audience_mismatch` | JWT `aud` drift | [oauth-mcp-integration.md](oauth-mcp-integration.md), egress audience checks |

#### 4a. Policy deny (`-32100`) and SpiceDB miss

1. Read `error.data.rule_fired` and `error.data.reason` when present ([reference-error-codes.md §3.1](reference-error-codes.md#31-pin-6-data-field-reference)).
2. If `reason` is `spicedb_deny`, verify tuples for `trogon/mcp_tool:{server_id}|{tool_name}` and principal ([trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md)).
3. If CEL allowed but SpiceDB denied, Phase 1 still requires both — see [howto-integrate-third-party-mcp.md §4.2](howto-integrate-third-party-mcp.md#42-seed-spicedb-tuples-phase-1-pdp).
4. Check hierarchical overlay: tenant deny can block org allow ([ADR 0013](../adr/0013-hierarchical-policy-merge.md), [hierarchical-policy-merge.md §1](hierarchical-policy-merge.md#1-the-hierarchy)). **Proposed** audit field `policy_ids` lists contributors.

#### 4b. Identity / STS (not bundle policy)

For `-32106`, `-32110`, `-32117`, `-32118`, act-chain `-32113`–`-32115` ([reference-error-codes.md §9](reference-error-codes.md#9-implementation-extensions-outside-pin-6)): tail `mcp.audit.sts.*`, read STS `decision_reason` ([reference-audit-envelope.md §5](reference-audit-envelope.md#5-sts-audit-envelope)), then [ADR 0019](../adr/0019-oauth-mcp-integration.md) / [sts-exchange.md](sts-exchange.md). Callback denials: [ADR 0020](../adr/0020-bidirectional-enforcement.md). Do not amend bundles for token or registry failures.

#### 4c. Schema redaction (allow with rewrite)

Audit may show `outcome: "rewrite"` while JSON-RPC returns `result` ([reference-audit-envelope.md §2.1](reference-audit-envelope.md#21-header-field-reference), [ADR 0027](../adr/0027-schema-driven-redaction.md)). Flush schema cache via `mcp.control.cache.invalidate.{server_id}` **(proposed -- ADR 0023)**. **Open question:** gateway `decision_reason` for redaction-only rows.

#### 4d. Rate limit (`-32105`)

Use `error.data.scope` and `retry_after_ms`; map to [ADR 0012](../adr/0012-rate-limit-state.md) Tier 1 inflight vs Tier 2 `rate.acquire` ([rate-limiting.md](rate-limiting.md)). Tests: `tests/rate_limit_caps.rs`. Fix budgets or wait — not PDP.

#### 4e. `-32107` disambiguation

`approval_subject` in `error.data` ⇒ [adaptive-access.md](adaptive-access.md) HITL. Otherwise SpiceDB/PDP down ([failure-mode-matrix.md](failure-mode-matrix.md) row 1) — escalate platform, do not republish bundles.

---

### 5. Cross-reference `traceparent` to OpenTelemetry

1. Take `error.data.trace_id` (32 hex chars) from Step 1.
2. If the client still has the NATS message, parse W3C `traceparent` from ingress headers ([reference-nats-headers.md §1.1](reference-nats-headers.md#11-full-header-table)); trace id is the middle segment.
3. In your trace backend, search `service.name = trogon-mcp-gateway` and trace id ([otel-wiring.md §7.2](otel-wiring.md#72-correlation-without-duplication)).
4. Expect child spans `mcp.gateway.authz`, `mcp.gateway.spicedb.check`, `mcp.gateway.egress` on allow paths; deny paths should show authz/SpiceDB failure without backend egress ([otel-wiring.md §3](otel-wiring.md#3-spans-the-gateway-emits)).

Audit remains the legal record; traces are best-effort and may be unsampled ([otel-wiring.md §7.1](otel-wiring.md#71-why-two-pipelines)).

---

### 6. Reproduce the decision offline

Replay policy without hitting production backends:

```bash
# (proposed -- ADR 0030)
trogon-gateway-ctl trace request --id "${JSONRPC_ID}" \
  --gateway-admin-url "${MCP_GATEWAY_ADMIN_URL:-http://127.0.0.1:8080}" \
  --output json
```

Expected `data`: `subject_in`, `jsonrpc_method`, `cel_requires_spicedb`, `spicedb_allowed`, `caller_sub`, `identity_source` ([ADR 0030](../adr/0030-gateway-ctl-cli-surface.md)). For bundle regressions, `trogon-gateway-ctl bundle dry-run` **(proposed)** per [howto-write-bundle.md §5](howto-write-bundle.md#step-5--dry-run-against-fixture-traffic). Exit `4` if the id is absent on that replica — try another gateway member.

---

## Verify

After remediation, confirm:

- [ ] Client `tools/call` succeeds or returns the **expected** deny with updated `rule_fired` / tuple fix documented.
- [ ] JetStream shows `{prefix}.audit.allow.request.tools` for successful retry (or `deny` with new `caller_sub` if still correctly denied).
- [ ] `error.data.trace_id` on any remaining deny links to a trace showing the same authz decision.
- [ ] No publish to `mcp.server.${SERVER_ID}.*` on denied attempts ([failure-mode-matrix.md § Invariants](failure-mode-matrix.md#invariants)).
- [ ] Integration tests relevant to the class pass locally when you changed policy (`audit_envelope_shape`, `audit_delivery`, `rate_limit_caps` under `trogon-mcp-gateway/tests/`).

---

## Rollback / undo

| If you changed | Undo |
|---|---|
| Policy bundle promotion | Repoint `mcp-gateway-config` active digest — [howto-write-bundle.md § Rollback](howto-write-bundle.md#rollback), [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md) |
| SpiceDB tuples (test) | Revert `zed relationship delete` in dev; production via PDP change control |
| ZedToken / permission cache | Rolling restart gateway replicas; delete session KV entry `{tenant}/{session_id}` in `mcp-sessions` when bucket exists ([bulk-check-permission.md](bulk-check-permission.md), [failure-mode-matrix.md](failure-mode-matrix.md) row 2) |
| Rate limit KV keys | Wait window or adjust bundle `rate.acquire`; see [rate-limiting.md](rate-limiting.md) |
| Schema cache flush | `mcp.control.cache.invalidate.{server_id}` **(proposed)** per [ADR 0023](../adr/0023-schema-cache-invalidation.md) |

If investigation proved infra failure (SpiceDB, NATS, STS), roll back **operational** changes only; do not loosen bundles to mask outage ([failure-mode-matrix.md](failure-mode-matrix.md)).

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| Audit empty for known deny | Stream missing, `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT`, or row 14 backlog | `nats stream info MCP_AUDIT`; fix retention; see matrix row 14 |
| `request_id` mismatch | Client retried with new id | Re-query with id from the failing response body |
| `-32107` with `approval_subject` | HITL hold, not SpiceDB | Wait for `mcp.approvals.{request_id}` decision ([reference-audit-envelope.md §7](reference-audit-envelope.md#7-approvals)) |
| `-32100` but tuple looks correct | Wrong resource id (`\|` vs `/`), stale ZedToken | Reseed tuple; restart replicas or invalidate session token ([ADR 0014](../adr/0014-bulk-check-zedtoken-cache.md)) |
| Surprise deny after bundle promotion | Hierarchical deny-wins overlay | Compare tenant vs org bundles ([hierarchical-policy-merge.md](hierarchical-policy-merge.md)) |
| Trace missing, audit present | Sampling or OTel not wired | Use audit as source of truth ([otel-wiring.md §6.4](otel-wiring.md#64-audit-stream-vs-trace-sampling)) |

---

## Related

- [reference-audit-envelope.md](reference-audit-envelope.md) (Pin 7) · [reference-error-codes.md](reference-error-codes.md) (Pin 6) · [failure-mode-matrix.md](failure-mode-matrix.md) (ADR 0024)
- ADRs [0012](../adr/0012-rate-limit-state.md), [0013](../adr/0013-hierarchical-policy-merge.md), [0019](../adr/0019-oauth-mcp-integration.md), [0020](../adr/0020-bidirectional-enforcement.md), [0030](../adr/0030-gateway-ctl-cli-surface.md)
- [otel-wiring.md](otel-wiring.md) · [howto-write-bundle.md](howto-write-bundle.md) · [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md) · [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md)
- Tests: `trogon-mcp-gateway/tests/audit_delivery.rs`, `audit_envelope_shape.rs`, `rate_limit_caps.rs`

---

*Document type: Diátaxis **how-to**. For exhaustive error and audit field lists, use [reference-error-codes.md](reference-error-codes.md) and [reference-audit-envelope.md](reference-audit-envelope.md).*

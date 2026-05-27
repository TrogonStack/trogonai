# Adaptive access — approvals, step-up, throttling, anomaly features

**Status:** Gateway library contract (Block 5). Ingress wiring lands in a follow-up track.

---

## Overview

When a gateway request crosses a configurable risk threshold, the MCP gateway can:

1. **Step-up auth** — return JSON-RPC `-32107 approval_required` with `approval_subject = mcp.approvals.step-up.{request_id}`.
2. **Human approval (HITL)** — park the call until an approver publishes a decision on `mcp.approvals.{request_id}`.
3. **Context throttling** — sliding-window limits keyed by `(tenant, agent_id, purpose)`; surfaces as `-32105 rate_limited`.
4. **Anomaly features** — best-effort publish to `mcp.metrics.anomaly.features` for offline scoring.

Policy entrypoint for ingress adopters: `policy::run_with_risk(...)`.

---

## Approval subjects

| Flow | NATS subject | JSON-RPC `approval_subject` field |
|---|---|---|
| HITL approval | `mcp.approvals.{request_id}` | same |
| Step-up auth | `mcp.approvals.step-up.{request_id}` | same |

`{request_id}` is the gateway-correlated id for the parked JSON-RPC call (same value as `request_id` in the error data).

Approvers (UI, governance service, or automation) **publish** a decision message to the subject. The gateway **subscribes** and resumes or denies the parked call.

---

## Decision message contract

Publish JSON to the approval subject:

```json
{
  "decision": "approve",
  "approver": "human:alice@acme",
  "expires_at": 1716840000
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `decision` | `"approve"` \| `"deny"` | yes | Lowercase string |
| `approver` | string | yes | Stable principal id (DID, email, service id) |
| `expires_at` | Unix seconds | yes | Decision validity; gateway also caps cache TTL |

On **`approve`**: gateway caches approval for `(request_id, hash(args))` until `min(expires_at, ttl_seconds)`.

On **`deny`** or **timeout**: gateway returns `-32107 approval_required` with `reason` describing denial/expiry.

Malformed payloads are ignored; the awaiter keeps listening until TTL.

---

## Client-visible `-32107 approval_required` envelope

Error code: **`-32107`**. Message: **`approval_required`**.

`error.data` object:

```json
{
  "approval_url": "https://console.trogon.local/approvals/{request_id}",
  "approval_subject": "mcp.approvals.{request_id}",
  "request_id": "{request_id}",
  "ttl_seconds": 300,
  "reason": "risk_score=85"
}
```

Step-up uses `approval_subject = "mcp.approvals.step-up.{request_id}"` and `reason` set to the required scope (e.g. `"purpose:admin"`).

Built by `approvals::build_approval_required(...)` and `approvals::build_approval_required_step_up(...)`.

---

## Context throttling

Keyed by **`(tenant, agent_id, purpose)`** — not `jwt.sub` alone.

| Config field (`MeshGatewayConfig.throttle`) | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Master switch |
| `window_secs` | `300` | Sliding window length |
| `max_requests` | `100` | Max admissions per key per window |

When exceeded: `RiskDecision::Throttle { retry_after_s }` → JSON-RPC `-32105 rate_limited` with `data.retry_after_s`.

---

## Anomaly feature stream

Subject: **`mcp.metrics.anomaly.features`**

Payload (JSON, one message per gateway request, best-effort async):

```json
{
  "ts": 1716836400,
  "tenant": "acme",
  "agent_id": "agent/oncall",
  "purpose": "incident_response",
  "target_aud": "urn:trogon:mcp:backend:acme:github",
  "scope_fingerprint": "tool:deploy",
  "outcome": "allow",
  "latency_ms": 42,
  "recent_denials_60s": 1
}
```

Publish failures must not block the request path.

---

## Risk evaluator (`policy::evaluate_risk`)

Inputs (`CallContext`): tenant, agent_id, purpose, target_aud, scope_fingerprint, jsonrpc_method, recent_denials_60s, args, request_id.

Outputs (`RiskDecision`):

| Variant | Gateway action |
|---|---|
| `Allow` | Continue |
| `Deny { reason }` | `-32100 policy_deny` |
| `StepUp { scope }` | `-32107` step-up envelope |
| `RequireApproval { reason, ttl_s }` | `-32107` HITL envelope; await subject |
| `Throttle { retry_after_s }` | `-32105 rate_limited` |

Thresholds live in `MeshGatewayConfig.risk` (approval/deny score, step-up purpose list, denial burst threshold).

---

## Implementing an approval client

1. Receive `-32107` from gateway/SDK; read `approval_subject` and `request_id`.
2. Present UI to operator; collect approve/deny.
3. Publish decision JSON to `approval_subject` (core NATS publish, no request/reply required).
4. Caller retries or gateway resumes parked call when decision arrives before `ttl_seconds`.

For step-up, subject is `mcp.approvals.step-up.{request_id}`; OIDC re-auth UI is out of scope for the gateway library — only the envelope is emitted.

---

## References

- [Identity overview](overview.md)
- [A2A SDK error model](sdk.md) — `ApprovalRequired` variant
- `trogon-mcp-gateway::approvals` — envelope builders, `ApprovalClient`, cache
- `trogon-mcp-gateway::policy::run_with_risk` — risk + throttle hook for ingress

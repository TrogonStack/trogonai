# Gateway-emitted JSON-RPC error codes

**Diátaxis:** reference (strict, exhaustive for Phase 1 client contract).

**Phase 1 contract.** Codes and `data` shapes are stable; message text is not.

**Authority:** Wire-Format Pin 6 (Gateway-emitted JSON-RPC error codes). This page is the normative source.

**Related:** [failure-mode-matrix.md](failure-mode-matrix.md), [reference-audit-envelope.md](reference-audit-envelope.md), [rate-limiting.md](rate-limiting.md), [otel-wiring.md](otel-wiring.md) (trace correlation).

**Implementation:** `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs` (subset of constants today); gateway reply helpers in `gateway.rs`, `approvals/envelope.rs`.

---

## 1. Code range

Trogon application errors occupy **`-32100` through `-32199`** (within the JSON-RPC application-error range).

| Range | Owner | Rationale |
|---|---|---|
| `-32700` … `-32600` | JSON-RPC protocol | Parse error, invalid request, method not found, invalid params, internal error — reserved by [JSON-RPC 2.0](https://www.jsonrpc.org/specification#error_object). Clients must treat these as transport/protocol faults, not Trogon policy. |
| `-32002` | MCP specification | MCP-defined server error slot. Distinct namespace so MCP SDKs do not collide with Trogon gateway semantics. |
| **`-32100` … `-32199`** | **Trogon MCP gateway** | Application-level denials, policy faults, backend faults, auth, and rate limits emitted at the gateway termination point before or instead of backend JSON-RPC. |
| Other negative codes | Backends / third parties | Opaque to the gateway contract unless explicitly remapped (not done in Phase 1). |

Only codes and structured `error.data` fields in §3 are stable for automation. Human-readable `error.message` strings may change between releases.

---

## 2. `trace_id` requirement

Every gateway-emitted Trogon error **MUST** include `trace_id` in `error.data`.

| Property | Contract |
|---|---|
| Format | 32 lowercase hex characters (W3C `trace-id` portion of `traceparent`) |
| Source | Same value as the inbound `traceparent` (or gateway-generated root trace when the client omitted tracing) |
| Purpose | Correlate JSON-RPC responses, OpenTelemetry traces ([otel-wiring.md](otel-wiring.md)), and audit envelopes ([reference-audit-envelope.md](reference-audit-envelope.md)) without parsing `message` text |

**Why in every `data` object:** Policy denials, rate limits, and backend timeouts are diagnosed in three places — client SDK, operator trace UI, and JetStream audit. A single stable id avoids joining on JSON-RPC `id` (client-local, not unique cluster-wide) or on mutable `message` strings. Dashboards and SIEM rules should key on `error.code` + `data.trace_id` + `data` extensions, never on `message`.

---

## 3. Full code table (Wire-Format Pin 6)

Reproduced. Each row adds a JSON `data` example and a typical trigger. Symbols match the plan column **Symbol**.

| Code | Symbol | Meaning | `data` shape (JSON example) | Example trigger |
|---|---|---|---|---|
| `-32100` | `policy_deny` | Authorization rule denied the request. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "rule_fired": "tool-call-authz", "reason": "spicedb_deny" }` | SpiceDB `CheckBulkPermissions` returns denied for `tools/call`; CEL/WASM guest returns `deny(reason)`; hierarchical merge deny-wins ([failure-mode-matrix.md](failure-mode-matrix.md) row 1 deny path). |
| `-32101` | `policy_fault` | CEL evaluation error, WASM trap, bundle missing. failClosed result. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "tier": "cel", "error": "division by zero in rule 'redact-github-pat'" }` | CEL runtime error on gated method; Tier-3 WASM panic ([failure-mode-matrix.md](failure-mode-matrix.md) rows 3, 7). **(proposed wire)** — constant not yet in `rpc_codes.rs`. |
| `-32102` | `backend_timeout` | Backend MCP server did not reply within the request deadline. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "server_id": "github", "elapsed_ms": 15000 }` | No NATS reply on `mcp.server.{server_id}.>` within operation timeout after forward ([failure-mode-matrix.md](failure-mode-matrix.md) row 8). |
| `-32103` | `backend_unreachable` | No backend matched the target or queue-group has no consumers. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "server_id": "github" }` | Unknown `server_id`, disabled backend, or empty queue group ([failure-mode-matrix.md](failure-mode-matrix.md) row 9 class). |
| `-32104` | `schema_unknown` | inputSchema not in cache and could not be fetched; redaction cannot validate. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "server_id": "github", "tool": "create_issue" }` | Redaction engine requires schema for `$.params.*` paths and cache miss + fetch failure. **(proposed wire)** — constant not yet in `rpc_codes.rs`. |
| `-32105` | `rate_limited` | Inflight cap or rate budget exceeded. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "scope": "tenant", "retry_after_ms": 2500 }` | Ingress/post-auth/per-tool budget exceeded ([rate-limiting.md](rate-limiting.md)); adaptive-access context throttle; worker inflight cap ([failure-mode-matrix.md](failure-mode-matrix.md) row 10). |
| `-32106` | `auth_expired` | JWT expired mid-session, or session revoked. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c" }` | Bearer `exp` in the past; session KV revocation; mesh token no longer valid at post-auth gate ([failure-mode-matrix.md](failure-mode-matrix.md) row 13 JWT path). |
| `-32107` | `authz_unreachable` | SpiceDB (or chosen PDP) did not respond. failClosed result. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "elapsed_ms": 3000 }` | SpiceDB gRPC transport failure, deadline exceeded, or HTTP 5xx ([failure-mode-matrix.md](failure-mode-matrix.md) row 1). **Note:** `-32107` is also used for `approval_required` in implementation — disambiguate via `error.data` (see [failure-mode-matrix.md § Behaviour modes](failure-mode-matrix.md#behaviour-modes)). |
| `-32108` | `no_policy` | Bundle not loaded, default-deny configured. | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c" }` | No active bundle pointer for tenant; day-zero default deny ([failure-mode-matrix.md](failure-mode-matrix.md) row 5). **(proposed wire)** — constant not yet in `rpc_codes.rs`. |
| `-32109` | `audience_mismatch` | JWT `aud` does not match the expected gateway or backend URI (enforce mode). | `{ "trace_id": "0af7651916cd43dd8448eb211c80319c", "expected_aud": "urn:trogon:mcp:gateway:acme", "actual_aud": "urn:trogon:mcp:server:wrong" }` | Egress mint or ingress validation detects `aud` drift in enforce mode ([oauth-mcp-integration.md](oauth-mcp-integration.md), shadow metrics on separate subject). |
| `TODO(enforce-mode-hardening)` | `agent_identity_required` **(RESERVED)** | Reserved for enforce-mode identity checks (missing `wkl`, SPIFFE/`agent_id` mismatch, act-chain depth reject, etc.) not yet pinned. | TBD | **Do not implement clients against this row.** Code value TBD; plan notes `-32108` as a *likely* candidate but `-32108` is already allocated to `no_policy` in the same pin — resolution is tracked under `TODO(enforce-mode-hardening)`. |

### 3.1 Pin 6 `data` field reference

| Symbol | Stable `data` fields (in addition to `trace_id`) |
|---|---|
| `policy_deny` | `rule_fired` (string), `reason` (string) |
| `policy_fault` | `tier` (string: `cel`, `wasm`, `bundle`, …), `error` (string) |
| `backend_timeout` | `server_id` (string), `elapsed_ms` (integer) |
| `backend_unreachable` | `server_id` (string) |
| `schema_unknown` | `server_id` (string), `tool` (string) |
| `rate_limited` | `scope` (string), `retry_after_ms` (integer) |
| `auth_expired` | (none beyond `trace_id`) |
| `authz_unreachable` | `elapsed_ms` (integer) |
| `no_policy` | (none beyond `trace_id`) |
| `audience_mismatch` | `expected_aud` (string), `actual_aud` (string) |

---

## 4. Error-shape pseudo-schema

Gateway JSON-RPC error responses follow JSON-RPC 2.0 with a Trogon-allocated `code` and structured `data`:

```json
{
  "jsonrpc": "2.0",
  "id": "<request id>",
  "error": {
    "code": -32100,
    "message": "<human readable>",
    "data": {
      "trace_id": "0af7651916cd43dd8448eb211c80319c",
      "...": "code-specific fields per §3"
    }
  }
}
```

| Field | Stability | Notes |
|---|---|---|
| `jsonrpc` | Stable | Always `"2.0"`. |
| `id` | Stable | Echoes request `id`; may be `null` for notifications. |
| `error.code` | **Stable** | Trogon allocation `-32100` … `-32199` (§1). |
| `error.message` | **Unstable** | Human-readable; suitable for logs and UI, not for dispatch logic (§9). |
| `error.data` | **Stable shape** | Object; `trace_id` required (§2); additional keys per §3. |

Optional future fields (e.g. `error.data.policy_bundle_digest`) MUST be additive; clients MUST ignore unknown keys.

---

## 5. Decision matrix (fail-closed vs fail-open)

Cross-reference: [failure-mode-matrix.md](failure-mode-matrix.md) behaviour modes (**CLOSED**, **OPEN-with-audit**, **DEGRADED**).

| Code | Symbol | Default gateway mode | Fail-closed? | Fail-open exception |
|---|---|---|---|---|
| `-32100` | `policy_deny` | **CLOSED** | Yes | None on gated methods. |
| `-32101` | `policy_fault` | **CLOSED** | Yes | Non-mutating paths only when bundle marks `on_error: log` **(proposed)** per matrix row 7. |
| `-32102` | `backend_timeout` | **CLOSED** | Yes (no synthetic success) | None. |
| `-32103` | `backend_unreachable` | **CLOSED** | Yes | None. |
| `-32104` | `schema_unknown` | **CLOSED** (redaction required) | Yes in enforce | **Shadow / observability-only** redaction may proceed without schema validation **(proposed)** — treat as fail-open with audit, not as authorization bypass. |
| `-32105` | `rate_limited` | **CLOSED** | Yes | None; exceeding budget is not an authz bypass. |
| `-32106` | `auth_expired` | **CLOSED** | Yes | None in enforce; re-authenticate. |
| `-32107` | `authz_unreachable` | **CLOSED** | Yes in production PDP configs | Dev-only: unset `MCP_GATEWAY_SPICEDB_ENDPOINT` allow-all **(today)** — not fail-open at request level, config bypass. |
| `-32108` | `no_policy` | **CLOSED** | Yes | None when default-deny bundle posture applies. |
| `-32109` | `audience_mismatch` | **CLOSED** in enforce | Yes in enforce | Shadow ingress may record metric without hard deny ([reference-audit-envelope.md](reference-audit-envelope.md) §11.3). |
| *(reserved)* | `agent_identity_required` | **CLOSED** (intended) | Yes (intended) | Shadow identity mode documented in [overview.md](overview.md) — not yet pinned to a code. |

**Invariant:** CLOSED codes MUST NOT publish to `mcp.server.*`, mint egress mesh tokens, or forward bearer credentials on deny paths ([failure-mode-matrix.md § Invariants](failure-mode-matrix.md#invariants)).

---

## 6. Retry semantics

| Code | Symbol | Retryable? | Backoff | Idempotency token |
|---|---|---|---|---|
| `-32100` | `policy_deny` | **No** | — | Retrying without policy/tuple change reproduces deny. |
| `-32101` | `policy_fault` | **No** (client) | — | Operator fixes bundle/CEL; client retry unsafe until config healthy. |
| `-32102` | `backend_timeout` | **Yes** | Exponential backoff with jitter; respect `elapsed_ms` as hint | Safe for idempotent tools when client supplies idempotency key **(proposed)**; mutating tools require human judgment. |
| `-32103` | `backend_unreachable` | **Yes** | Short backoff then longer; fail over to another gateway replica | Same as timeout; check `server_id` registration first. |
| `-32104` | `schema_unknown` | **Yes** (limited) | Single immediate retry after `tools/list` cache warm | Not a substitute for fixing schema publication. |
| `-32105` | `rate_limited` | **Yes** | Wait at least `data.retry_after_ms` | Do not spin; honor `scope` for metrics ([rate-limiting.md](rate-limiting.md)). |
| `-32106` | `auth_expired` | **No** | — | Obtain fresh token / re-`initialize` session. |
| `-32107` | `authz_unreachable` | **Cautious** | Bounded retries (e.g. 2–3) with backoff | Only when side effects are none; gated mutating calls should not retry blindly. |
| `-32108` | `no_policy` | **No** | — | Operator loads bundle; client retry useless. |
| `-32109` | `audience_mismatch` | **No** | — | Fix token `aud` or gateway/backend URI configuration. |
| *(reserved)* | `agent_identity_required` | TBD | TBD | TBD |

**Approval path (`-32107` with approval data):** Not a transport retry — subscribe to `data.approval_subject`, wait for human decision within `ttl_seconds` ([reference-audit-envelope.md](reference-audit-envelope.md) §7).

---

## 7. Audit envelope correlation

Gateway JetStream audit uses subject pattern `{prefix}.audit.{outcome}.{direction}.{method_root}` ([reference-audit-envelope.md](reference-audit-envelope.md) §3.1). Map JSON-RPC codes to audit **`outcome`** (wire vocabulary: `allow`, `deny`, `error`).

| Code | Symbol | Audit `outcome` | Example subject suffix | Proposed `decision_reason` |
|---|---|---|---|---|
| `-32100` | `policy_deny` | `deny` | `.deny.request.tools` | `policy_deny` |
| `-32101` | `policy_fault` | `error` | `.error.request.tools` | `policy_fault` |
| `-32102` | `backend_timeout` | `error` | `.error.request.tools` | `backend_timeout` |
| `-32103` | `backend_unreachable` | `error` | `.error.request.tools` | `backend_unreachable` |
| `-32104` | `schema_unknown` | `deny` or `error` | `.deny.request.tools` **(proposed)** | `schema_unknown` |
| `-32105` | `rate_limited` | `deny` | `.deny.request.tools` | `rate_limited` (STS uses `rate_limit` outcome on separate family) |
| `-32106` | `auth_expired` | `deny` | `.deny.request.tools` | `auth_expired` |
| `-32107` | `authz_unreachable` | `error` | `.error.request.tools` | `authz_unreachable` |
| `-32108` | `no_policy` | `deny` | `.deny.request.tools` | `no_policy` |
| `-32109` | `audience_mismatch` | `deny` | `.deny.request.tools` | `audience_mismatch` |

**Correlation workflow**

1. Client records `error.data.trace_id` from JSON-RPC.
2. Operator searches traces by that id ([otel-wiring.md](otel-wiring.md) §10.3).
3. SIEM consumes `mcp.audit.*` with matching `trace_id` when the common header lands on wire ([reference-audit-envelope.md](reference-audit-envelope.md) §2.1).

`allow` audit envelopes MUST NOT be emitted for rows where the client received a Trogon error above ([failure-mode-matrix.md](failure-mode-matrix.md) invariant 4).

---

## 8. SDK author guidance

### 8.1 Typed enum mapping

Define a single enum (names illustrative) mapped from `error.code`:

```rust
pub enum TrogonGatewayError {
    PolicyDeny(PolicyDenyData),
    PolicyFault(PolicyFaultData),
    BackendTimeout(BackendTimeoutData),
    BackendUnreachable(BackendUnreachableData),
    SchemaUnknown(SchemaUnknownData),
    RateLimited(RateLimitedData),
    AuthExpired,
    AuthzUnreachable(AuthzUnreachableData),
    NoPolicy,
    AudienceMismatch(AudienceMismatchData),
    Unknown(i32),
}
```

Deserialize `error.data` into per-variant structs; treat missing optional fields as `None`.

### 8.2 Dispatch rules

| Do | Do not |
|---|---|
| Branch on `error.code` and parse `error.data` | Match `error.message` strings |
| Surface `trace_id` to application logs and support tickets | Assume English `message` stability across versions |
| Map `-32105` to retry-after using `retry_after_ms` | Retry `-32100` / `-32109` without policy or token change |
| Disambiguate `-32107` via `data.approval_subject` vs `elapsed_ms` | Treat all `-32107` as SpiceDB down |

### 8.3 HTTP / NATS clients

Ingress may arrive over NATS queue groups, not HTTP. JSON-RPC shape is identical; propagate W3C `traceparent` on publish so `trace_id` in errors aligns with telemetry.

---

## 9. Implementation extensions (outside Pin 6)

The following constants exist in `rpc_codes.rs` on the implementation branch but are **not** part of Wire-Format Pin 6. SDKs MAY handle them defensively; do not treat them as Phase 1 contract until promoted into this reference.

| Code | Constant | Typical meaning |
|---|---|---|
| `-32110` | `INVALID_TOKEN` | Malformed or invalid Bearer under enforce |
| `-32113` | `ACT_CHAIN_DEPTH_EXCEEDED` | Delegation chain too deep |
| `-32114` | `ACT_CHAIN_LOOP_DETECTED` | Cycle in act chain |
| `-32115` | `ACT_CHAIN_UNRESOLVED` | Unresolved hop in chain |
| `-32117` | `AGENT_IDENTITY_REQUIRED` | Enforce identity gate (overlaps reserved Pin row) |
| `-32118` | `AUTH_REQUIRED` | Auth required when gate applies |

See [failure-mode-matrix.md § JSON-RPC code quick reference](failure-mode-matrix.md#json-rpc-code-quick-reference) for plan-vs-code gaps (`policy_fault`, `no_policy`, `schema_unknown`).

---

## 10. Complete response examples

### 10.1 Policy deny

```json
{
  "jsonrpc": "2.0",
  "id": 99,
  "error": {
    "code": -32100,
    "message": "policy_deny",
    "data": {
      "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
      "rule_fired": "tool-call-authz",
      "reason": "spicedb_deny"
    }
  }
}
```

### 10.2 Rate limited

```json
{
  "jsonrpc": "2.0",
  "id": 100,
  "error": {
    "code": -32105,
    "message": "rate_limited",
    "data": {
      "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
      "scope": "tenant",
      "retry_after_ms": 2500
    }
  }
}
```

### 10.3 Authz unreachable

```json
{
  "jsonrpc": "2.0",
  "id": 101,
  "error": {
    "code": -32107,
    "message": "authz_unreachable",
    "data": {
      "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
      "elapsed_ms": 3000
    }
  }
}
```

---

## 11. Cross-references

| Document | Relevance |
|---|---|
| [failure-mode-matrix.md](failure-mode-matrix.md) | CLOSED / OPEN / DEGRADED defaults per failure class |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit `outcome`, subjects, `decision_reason` |
| [rate-limiting.md](rate-limiting.md) | `-32105` scopes, `retry_after_ms`, placement layers |
| [otel-wiring.md](otel-wiring.md) | `traceparent` injection and trace lookup |
| [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md) | Operator-facing deny example |

---

## 12. Changelog (reference doc)

| Date | Notes |
|---|---|
| 2026-05-28 | Initial Diátaxis reference (Block H Wire-Format Pin 6). |

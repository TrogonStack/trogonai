# Audit envelope schema reference

**Status:** Reference (Diátaxis). Canonical field-by-field contract for JetStream audit payloads emitted by the MCP gateway, STS, agent registry, and approval flows.

**Schema version (`envelope_version`):** `1.0.0` (this document).

**Related:** [agent-traffic.md](agent-traffic.md) (index projection and SIEM), [reference-subject-grammar.md](reference-subject-grammar.md) (NATS subject source of truth; forward reference if not yet merged), [integration-touchpoints.md](integration-touchpoints.md), [sts-exchange.md](sts-exchange.md), [registry.md](registry.md), [adaptive-access.md](adaptive-access.md).

**Implementation anchors:**

| Producer | Rust type | Publish path |
|---|---|---|
| MCP gateway | `trogon_mcp_gateway::audit::AuditEnvelope` | `rsworkspace/crates/trogon-mcp-gateway/src/audit.rs` |
| STS | `trogon_sts::audit::StsAuditEvent` | `rsworkspace/crates/trogon-sts/src/audit.rs` |
| Registry (runtime lookup) | `LookupAuditEvent`, `MutationAuditEvent` | `rsworkspace/crates/trogon-agent-registry/src/audit.rs` |
| Registry (controller) | `AuditEvent` | `rsworkspace/crates/trogon-agent-registry-controller/src/audit.rs` |
| Approvals (decision) | `ApprovalDecisionMessage` | `rsworkspace/crates/trogon-mcp-gateway/src/approvals/types.rs` |
| Traffic indexer (consumer) | `trogon_traffic_view::envelope::AuditEnvelope` | `rsworkspace/crates/trogon-traffic-view/src/envelope.rs` |
| OCSF exporter | `DefaultOcsfExporter` | `rsworkspace/crates/trogon-traffic-view/src/siem/ocsf.rs` |

Unless a field is marked **proposed**, it is grounded in the cited Rust source at the time of this document.

---

## 1. Scope

This reference covers audit payloads published to JetStream stream **`MCP_AUDIT`** (default) and core NATS subjects used by the approval awaiter. Subject names follow the grammar in [reference-subject-grammar.md](reference-subject-grammar.md); the tables below list the audit families implemented today.

### 1.1 Covered audit families

| Family | Subject pattern (default prefix `mcp`) | Producer | Indexed by traffic-view |
|---|---|---|---|
| Gateway MCP decisions | `{prefix}.audit.{outcome}.{direction}.{method_root}` | `trogon-mcp-gateway` | Yes (classifier in `normalize.rs`) |
| STS token exchange | `mcp.audit.sts.{outcome}` | `trogon-sts` | Yes |
| Registry runtime | `mcp.audit.registry.lookup.{found\|notfound\|revoked}`, `mcp.audit.registry.{put\|delete}` | `trogon-agent-registry` | Yes |
| Registry controller lifecycle | `mcp.audit.registry.{registered\|bumped\|deprecated\|revoked}` | `trogon-agent-registry-controller` | Yes (via `mcp.audit.registry.*` prefix) |
| Approvals (HITL / step-up) | `mcp.approvals.{request_id}`, `mcp.approvals.step-up.{request_id}` | External publisher; gateway subscribes | No (not under `mcp.audit.*`) |
| STS latency probe violation | `mcp.audit.sts.latency_violation` | `trogon-sts-probe` sidecar | No |
| Gateway chain resolution (log-only today) | N/A (structured log `event = mcp.audit.gateway.chain_unresolved`) | `trogon-mcp-gateway::ingress` | No |

### 1.2 Out of scope

- OAuth-specific audit subjects marked **proposed** in [oauth-mcp-integration.md](oauth-mcp-integration.md).
- A2A gateway audit envelopes (`a2a-nats/src/audit/envelope.rs`) — separate product surface; only noted where field names align with MCP forward-compat plans.
- Anomaly feature vectors on `mcp.metrics.anomaly.features` (`trogon-mcp-gateway/src/anomaly.rs`) — metrics, not legal audit.
- Raw JSON-RPC request/response bodies (never audit-payload fields on the gateway hot path today).

### 1.3 Consumer filter (traffic indexer)

Durable consumer filter: `mcp.audit.>` (`trogon-traffic-view/src/projector/consumer.rs:37`). Gateway-shaped subjects are normalized in `normalize.rs` but the consumer currently acks only STS and registry subjects (`consumer.rs:72`); gateway rows are classified for future widening.

---

## 2. Common header

Every audit family shares a **logical** header. Producers today emit different JSON shapes; the header below is the **target contract** for `envelope_version` `1.0.0`. Fields marked **wire today** appear on at least one producer without transformation. Fields marked **proposed** are not yet emitted by all producers but are required for new integrations (OCSF exporter, SIEM, traffic indexer).

### 2.1 Header field reference

| Field | Type | Null | Range / format | Wire today | Source |
|---|---|---|---|---|---|
| `envelope_version` | string (semver) | No | Semver of **this** reference doc; currently `1.0.0` | **proposed** | Operator contract; not in gateway/STS structs |
| `event_id` | string | No | Opaque unique id per envelope | Partial | Embedded in payload if present; else SHA-256 hex of `subject ‖ payload` (`trogon-traffic-view/src/envelope.rs:17-34`) |
| `event_time` | string (RFC3339) | No | UTC timestamp | Partial | STS uses derived time from `latency_ms` when `ts` absent (`normalize.rs:111-127`); gateway omits timestamp on wire |
| `producer` | string | No | Component name | Partial | Implied by NATS subject prefix / family (`mcp-gateway`, `trogon-sts`, `trogon-agent-registry`, `trogon-agent-registry-controller`) |
| `tenant_id` | string | Yes | Non-empty tenant slug | Partial | Gateway: `tenant` optional (`audit.rs:39`); STS: optional on minted claims; indexer defaults missing to `"default"` (`normalize.rs:39-41`, `75`, `96`) |
| `trace_id` | string | Yes | W3C trace id (32 hex) | **proposed** | Planned from `inject_trace_context` (`gateway.rs:417`); A2A envelope has optional `trace_id` (`a2a-nats/src/audit/envelope.rs:81`) |
| `span_id` | string | Yes | W3C span id (16 hex) | **proposed** | Same as `trace_id` |
| `correlation_id` | string | Yes | Opaque correlation (JSON-RPC `id`, approval `request_id`, STS `jti` hash) | Partial | Gateway: `request_id` (`audit.rs:45`); approvals: `request_id` in client envelope |
| `outcome` | enum | No | See §2.2 | Yes | Family-specific vocabulary normalized by indexer (`TrafficDecision::parse`, `event.rs:84-91`) |
| `decision_reason` | string | Yes | Free text; well-known values in §2.3 | Partial | STS: `decision_reason` required (`trogon-sts/src/audit.rs:13`); gateway: absent on wire |

**Normalization aliases used by the traffic indexer:**

| Wire `outcome` (producer) | Normalized `TrafficDecision` | Common header `outcome` |
|---|---|---|
| `allow`, `success`, `ok` | `allow` | `success` |
| `deny`, `err` | `deny` | `deny` |
| `error` (gateway upstream failure) | `deny` (parse quirk) | `error` |
| `rewrite`, `redact` | `rewrite` | `success` (with rewrite flag) |
| `rate_limit`, `rate_limited` | `rate_limited` | `deny` |

Gateway blocked paths set `audit_outcome` to `"deny"` or `"error"` (`gateway.rs:306`, `326`, `379`, `181`, `213`, `241`). Allowed forward paths use `"allow"` (`gateway.rs:532`, `447`). Backend timeout/unreachable after allow path uses `"error"` (`gateway.rs:448-449`).

### 2.2 Common `outcome` enum (header vocabulary)

Integrators SHOULD map producer-specific outcomes to this three-value header enum for dashboards that span services:

```text
success | deny | error
```

| Value | Meaning |
|---|---|
| `success` | Policy allowed the action; STS mint succeeded; registry lookup found active agent; approval granted |
| `deny` | Policy or validation rejected the action (including rate limits and authz denials) |
| `error` | Infrastructure or dependency failure (SpiceDB unreachable, signer down, backend timeout treated as error on gateway allow path) |

### 2.3 Well-known `decision_reason` values

#### STS (`trogon-sts`)

Emitted in `StsAuditEvent.decision_reason` (`audit.rs:13`). Built from `StsError::audit_reason()` (`error.rs:58-67`) and hard-coded success paths (`exchange.rs:94-95`, `111-112`).

| `decision_reason` | Typical `outcome` (wire) | When |
|---|---|---|
| `exchange_ok` | `success` | Token mint succeeded (`exchange.rs:95`) |
| `wkl_unattested` | `deny` | Shadow attestation policy: exchange succeeds but parallel deny audit emitted (`exchange.rs:111-112`) |
| `purpose_missing` | `deny` | `require_purpose` and empty purpose (`error.rs:65`) |
| `act_chain_entry_revoked` | `deny` | Chain walk found revoked/unknown agent (`error.rs:64`) |
| `registry_unavailable` | `error` or `deny` | Registry dependency down (`error.rs:60`) |
| `spicedb_unavailable` | `error` | SpiceDB check failed (`error.rs:61`) |
| `dependency_unavailable` | `error` | Generic dependency (`error.rs:62`) |
| `signer_unavailable` | `error` | JWT signing failed (`error.rs:63`) |
| `act_chain_depth_exceeded` | `deny` | Inbound chain at max depth (`error.rs:13-14`, audit via `error_description`) |
| `act_chain_loop_detected` | `deny` | Duplicate hop in chain (`error.rs:15-16`) |
| `invalid_request: …` | `deny` | Malformed wire request (`StsError::InvalidRequest`) |
| `invalid_grant: …` | `deny` | Token verification failure |
| `invalid_target: …` | `deny` | Audience/scope/target validation |
| `access_denied: …` | `deny` | Authorization failure |
| `rate_limited: …` | `rate_limit` | Rate limiter (`error.rs:21-22`, `audit_outcome` → `rate_limit` at `error.rs:73`) |

#### Gateway (`trogon-mcp-gateway`)

No `decision_reason` field on wire today (`audit.rs:33-58`). Operators SHOULD treat these as **proposed** canonical reasons when the header is unified:

| Proposed `decision_reason` | Trigger (code) |
|---|---|
| `policy_deny` | SpiceDB/CEL deny (`gateway.rs:306-308`, `jsonrpc_message: policy_deny`) |
| `authz_unreachable` | SpiceDB client error (`gateway.rs:326-328`) |
| `sts_unavailable` | Egress mint failure (`gateway.rs:367+`, tests in `egress_mint.rs`) |
| `act_chain_unresolved` | Ingress chain deny (`ingress.rs:96-100`) |
| `backend_timeout` | Request timeout (`gateway.rs:449`, `496+`) |
| `backend_unreachable` | Upstream NATS error (`gateway.rs:448`, `490-492`) |
| `allow` | Forward allowed (`gateway.rs:447`, `532`) |

Structured log event `mcp.audit.gateway.chain_unresolved` (`ingress.rs:89`, `149`) is not yet a JetStream envelope.

#### Registry

Runtime lookup events use `outcome` string on the payload (`LookupAuditEvent.outcome`, `audit.rs:18`) with values `found`, `notfound`, `revoked` implied by subject constant. Controller events use `event_type` (`controller/audit.rs:31`) — not `decision_reason`.

#### Approvals

Client-visible `reason` in approval-required JSON (`approvals/envelope.rs:56`) — e.g. `high_risk_tool`, `scope:admin`. Decision messages carry `decision` enum, not `decision_reason` (`types.rs:77-80`).

---

## 3. NATS subject grammar (audit)

Canonical grammar: [reference-subject-grammar.md](reference-subject-grammar.md). Operational summary:

### 3.1 Gateway

**Pattern:** `{prefix}.audit.{outcome}.{direction}.{method_root}`

Built by `audit_publish_subject` (`audit.rs:112-114`):

```text
format!("{prefix}.audit.{outcome}.{direction}.{method_root}")
```

| Segment | Values (wire) | Notes |
|---|---|---|
| `prefix` | Default `mcp` | `MCP_PREFIX` / `mcp_nats::Config` |
| `outcome` | `allow`, `deny`, `error` | Not the common-header `success` token |
| `direction` | `request` (primary) | `response`, `callback` reserved in [agent-traffic.md](agent-traffic.md) §2.1 |
| `method_root` | First path segment of JSON-RPC method, `.` → `_` | `jsonrpc_method_root` (`audit.rs:116-123`) |

**Examples:**

| JSON-RPC method | `method_root` | Example subject |
|---|---|---|
| `tools/call` | `tools` | `mcp.audit.allow.request.tools` |
| `resources/read` | `resources` | `mcp.audit.deny.request.resources` |
| `initialize` | `initialize` | `mcp.audit.error.request.initialize` |

Task shorthand **`mcp.audit.gateway.{decision}.{outcome}`** maps to **`outcome`** + **`direction`** segments above, not a literal `gateway` token ([agent-traffic.md](agent-traffic.md) §2.1).

### 3.2 STS

**Pattern:** `mcp.audit.sts.{outcome}` (`audit.rs:53-54`)

| `{outcome}` (wire) | Emitted when |
|---|---|
| `success` | Mint OK (`exchange.rs:94`) |
| `deny` | Validation/policy denial (`error.rs:79`) |
| `rate_limit` | Rate limited (`error.rs:73`) — note: docs often say `rate_limited`; wire uses `rate_limit` |
| `error` | Server/dependency errors (`error.rs:74-78`) |

Additional: `mcp.audit.sts.latency_violation` (probe sidecar, `trogon-sts/src/bin/probe.rs:13`).

### 3.3 Registry

**Runtime** (`trogon-agent-registry/src/audit.rs:7-11`):

| Subject | Event shape |
|---|---|
| `mcp.audit.registry.lookup.found` | `LookupAuditEvent` |
| `mcp.audit.registry.lookup.notfound` | `LookupAuditEvent` |
| `mcp.audit.registry.lookup.revoked` | `LookupAuditEvent` |
| `mcp.audit.registry.put` | `MutationAuditEvent` |
| `mcp.audit.registry.delete` | `MutationAuditEvent` |

**Controller** (`trogon-agent-registry-controller/src/audit.rs:5-8`):

| Subject | `event_type` in payload |
|---|---|
| `mcp.audit.registry.registered` | `registered` (via kind enum) |
| `mcp.audit.registry.bumped` | version bump |
| `mcp.audit.registry.deprecated` | deprecation |
| `mcp.audit.registry.revoked` | revocation |

### 3.4 Approvals

| Subject | Direction | Payload |
|---|---|---|
| `mcp.approvals.{request_id}` | Approver → gateway | `ApprovalDecisionMessage` |
| `mcp.approvals.step-up.{request_id}` | Approver → gateway | Same |

Built by `ApprovalSubject::for_request` / `for_step_up` (`approvals/types.rs:40-45`).

Client JSON-RPC `-32107` `approval_required` **`error.data`** (gateway → client) is documented in §5.4.

---

## 4. Gateway audit envelope

**Rust:** `AuditEnvelope` (`trogon-mcp-gateway/src/audit.rs:33-58`)

**Subject:** §3.1

**Stream:** JetStream `MCP_AUDIT`, filter `{prefix}.audit.>` (`audit.rs:125-139`)

### 4.1 Payload fields (wire today)

| Field | JSON type | Null | Description | Source |
|---|---|---|---|---|
| `subject_in` | string | No | NATS subject of inbound MCP message | `audit.rs:34`, set from `msg.subject` (`gateway.rs:556`) |
| `subject_out` | string | No | Rewritten backend subject | `audit.rs:35`, `gateway.rs:557` |
| `outcome` | string | No | `allow`, `deny`, or `error` | `audit.rs:36` |
| `direction` | string | No | `request` on all current publish sites | `audit.rs:37` |
| `jsonrpc_method` | string | No | Full method e.g. `tools/call` | `audit.rs:38` |
| `tenant` | string | Yes | Tenant slug from JWT / headers | `audit.rs:39` |
| `caller_sub` | string | Yes | Principal `sub` | `audit.rs:41`, omitted when null (`skip_serializing_if`) |
| `jwt_issuer` | string | Yes | Token `iss` | `audit.rs:43` |
| `identity_source` | string | No | `jwt`, `legacy_header`, or `anonymous` | `authz.rs:8-14`, serialized snake_case |
| `request_id` | string or number | Yes | JSON-RPC `id` | `audit.rs:45` |
| `agent_id` | string | Yes | From JWT claims when populated | `audit.rs:47`, via `IdentityFields` |
| `agent_version` | string | Yes | Agent semver | `audit.rs:49` |
| `wkl` | string | Yes | Workload SPIFFE URI | `audit.rs:51` |
| `purpose` | string | Yes | Intent string | `audit.rs:53` |
| `session_id` | string | Yes | MCP session | `audit.rs:55` |
| `act_chain` | array | Yes | Delegation hops; see §4.2 | `audit.rs:57` |

**Not on wire today (proposed extensions from [agent-traffic.md](agent-traffic.md) and operator docs):**

| Field | Status | Notes |
|---|---|---|
| `decision_reason` | **proposed** | §2.3 |
| `trace_id`, `span_id` | **proposed** | W3C from outbound headers |
| `event_id`, `event_time`, `envelope_version`, `producer` | **proposed** | Common header §2 |
| `jsonrpc_params_hash` / `params_fingerprint` | **proposed** | A2A gateway uses `params_fingerprint` (`a2a-nats/src/audit/envelope.rs:111`) |
| `allowed_tools` | **proposed** | Referenced in ingress tests (`ingress_chain.rs:15`); not serialized in gateway audit yet |
| `latency_us` | **proposed** | `AuditEnvelopeExtensions.latency_us` in `a2a-pack/src/audit.rs:79` |
| `rules_fired` | **proposed** | CEL policy audit builtin not implemented (`cel_builtins/audit.rs:14`) |
| `spicedb_allowed` | **proposed** | Available in `DecisionTrace` (`trace.rs`) but not in audit JSON |

### 4.2 `act_chain` entry

**Rust:** `AuditActChainEntry` (`audit.rs:14-19`)

| Field | Type | Null | Description |
|---|---|---|---|
| `sub` | string | No | Principal at hop |
| `agent_id` | string | No | Agent id at hop |
| `wkl` | string | No | Workload id |
| `iat` | integer | No | Unix seconds |

Populated via `apply_identity_fields` when gateway passes `IdentityFields` (`audit.rs:98-108`). **Wire today:** gateway publish sites pass `None` for identity (`gateway.rs:566`, `619`); fields are schema-ready but omitted until identity rollout completes.

### 4.3 `identity_source` enum

| Value | Meaning |
|---|---|
| `jwt` | Bearer JWT validated (`authz.rs:11`) |
| `legacy_header` | Legacy header attribution |
| `anonymous` | No identity |

---

## 5. STS audit envelope

**Rust:** `StsAuditEvent` (`trogon-sts/src/audit.rs:11-28`)

**Subject:** `mcp.audit.sts.{outcome}` (§3.2)

### 5.1 Top-level fields

| Field | Type | Null | Description | Source |
|---|---|---|---|---|
| `outcome` | string | No | `success`, `deny`, `rate_limit`, `error` | `audit.rs:12` |
| `decision_reason` | string | No | §2.3 | `audit.rs:13` |
| `latency_ms` | integer (u64) | No | Exchange duration ms | `audit.rs:14`, `audit.rs:156` |
| `source_ip` | string | Yes | Client IP if provided | `audit.rs:16` |
| `request` | object | No | Exchange request subset | `audit.rs:17`, §5.2 |
| `minted` | object | Yes | Summary of minted claims on success | `audit.rs:19`, §5.3 |
| `wkl` | string | Yes | Attested workload URI | `audit.rs:21` |
| `agent_id` | string | Yes | Target agent id | `audit.rs:23` |
| `offending_index` | integer | Yes | `act_chain` hop index on revoke deny | `audit.rs:25` |
| `offending_agent_id` | string | Yes | Agent id at offending hop | `audit.rs:27` |

**Not on wire today (documented elsewhere as illustrative / proposed):**

| Field | Status | Notes |
|---|---|---|
| `act_chain_len` | **proposed** | Use `minted.act_chain_depth` (`types.rs:66`) |
| `attestation_policy` | **proposed** | Runtime `AttestationPolicy` (`exchange.rs:34`, `186`); shadow vs enforce not serialized |
| `aud_mismatch_shadow` | **proposed** | Separate metric subject `mcp.metrics.gateway.aud_mismatch_shadow` (`egress/audience.rs:4`) — gateway egress, not STS audit |
| `trace_id`, `event_id`, `ts`, `schema` | **proposed** | [sts-exchange.md](sts-exchange.md) illustrative envelope |

### 5.2 `request` object (`StsAuditRequestFields`)

**Rust:** `audit.rs:31-38`

| Field | Type | Null | Description |
|---|---|---|---|
| `subject_token_type` | string | No | OAuth token type URI |
| `actor_token` | string | No | **Sensitive** — SVID/JWT material; MUST redact off-box (§8) |
| `audience` | string | No | Requested `aud` |
| `scope` | string | No | Requested scope |
| `purpose` | string | No | Intent |
| `requested_token_type` | string | No | Typically JWT URI |

Mapped from `StsExchangeRequest` (`types.rs:4-12`, `audit.rs:40-50`). **`subject_token` is not included** in audit (by design — see [sts-exchange.md](sts-exchange.md) § Audit emission).

### 5.3 `minted` object (`MintedClaimsSummary`)

**Rust:** `types.rs:56-67`

| Field | Type | Null | Description |
|---|---|---|---|
| `sub` | string | No | Minted subject |
| `iss` | string | No | Mesh issuer |
| `aud` | string | No | Audience |
| `exp` | integer | No | Expiry Unix s |
| `iat` | integer | No | Issued-at Unix s |
| `agent_id` | string | Yes | Agent id claim |
| `wkl` | string | Yes | Workload URI |
| `purpose` | string | Yes | Purpose claim |
| `scope` | string | No | Narrowed scope |
| `act_chain_depth` | integer | No | Length of `act_chain` after append (`exchange.rs:259`) |

Full `act_chain` array is **not** duplicated in audit summary (proposed for a future minor version).

### 5.4 Shadow attestation dual emit

When `attestation_policy.is_shadow()` (`exchange.rs:186`), a successful exchange emits:

1. `mcp.audit.sts.success` with `decision_reason: exchange_ok` and populated `minted`
2. `mcp.audit.sts.deny` with `decision_reason: wkl_unattested` and `minted: null` (`exchange.rs:107-123`)

Integrators counting denies MUST dedupe by correlation (same request, two subjects) or filter shadow denies in enforce mode.

### 5.5 STS latency violation (auxiliary)

**Subject:** `mcp.audit.sts.latency_violation` (`probe.rs:13`)

| Field | Type | Description |
|---|---|---|
| `schema` | string | `trogon.mcp.audit.sts.latency_violation/v1` |
| `p99_ms` | integer | Rolling P99 |
| `threshold_ms` | integer | Configured threshold |
| `p50_ms`, `p95_ms` | integer | Rolling percentiles |

Not a token-exchange decision envelope; operators wire alerting separately.

---

## 6. Registry audit envelopes

### 6.1 Runtime lookup (`trogon-agent-registry`)

**Rust:** `LookupAuditEvent` (`audit.rs:14-19`)

| Field | Type | Null | Description |
|---|---|---|---|
| `agent_id` | string | No | Queried agent |
| `tenant_hint` | string | Yes | Optional tenant context |
| `outcome` | string | No | Semantic outcome (paired with subject) |

Subjects encode outcome: `lookup.found`, `lookup.notfound`, `lookup.revoked` (`audit.rs:7-9`).

### 6.2 Runtime mutation (`MutationAuditEvent`)

**Rust:** `audit.rs:22-28`

| Field | Type | Null | Description |
|---|---|---|---|
| `event_type` | string | No | `put` or `delete` |
| `agent_id` | string | No | Agent id |
| `agent_version` | string | No | Version key |
| `kv_revision` | integer | Yes | NATS KV revision after write |

Subjects: `mcp.audit.registry.put`, `mcp.audit.registry.delete` (`audit.rs:10-11`).

### 6.3 Controller lifecycle (`trogon-agent-registry-controller`)

**Rust:** `AuditEvent` (`controller/audit.rs:30-41`)

| Field | Type | Null | Description |
|---|---|---|---|
| `event_type` | string | No | Mutation kind label |
| `agent_id` | string | No | Agent id |
| `agent_version` | string | No | Manifest version |
| `prior_lifecycle_state` | string | Yes | Previous state label |
| `new_lifecycle_state` | string | No | `active`, `deprecated`, `revoked` |
| `manifest_digest` | string | No | Content digest |
| `operator` | string | No | Controller operator id |
| `kv_revision` | integer | Yes | KV revision |
| `git_commit` | string | No | Source commit for manifest |

Subjects: §3.3 controller table.

---

## 7. Approvals

Approvals use **core NATS** (not JetStream audit stream) for the decision path.

### 7.1 Decision message (approver → gateway)

**Rust:** `ApprovalDecisionMessage` (`approvals/types.rs:76-81`)

**Subject:** `mcp.approvals.{request_id}` or `mcp.approvals.step-up.{request_id}`

| Field | Type | Null | Description |
|---|---|---|---|
| `decision` | string | No | `approve` or `deny` (serde lowercase) |
| `approver` | string | No | Approver principal e.g. `human:alice@acme` |
| `expires_at` | integer | No | Unix seconds |

Gateway awaiter: `approvals_nats.rs` integration test (`tests/approvals_nats.rs:42-46`).

### 7.2 Client approval-required envelope (gateway → MCP client)

**Rust:** `build_approval_required_with_subject` (`approvals/envelope.rs:39-57`)

Not published to `mcp.approvals.*`; returned in JSON-RPC error `data`:

| Field | Type | Description |
|---|---|---|
| `approval_url` | string | Console URL |
| `approval_subject` | string | NATS subject to publish decision to |
| `request_id` | string | Correlation id |
| `ttl_seconds` | integer | Wait TTL |
| `reason` | string | Policy reason string |

Wrapped by `jsonrpc_error_with_approval_data` (`envelope.rs:61-73`).

### 7.3 Args binding (cache key)

**Rust:** `ArgsHash` (`types.rs:54-67`) — SHA-256 of canonical JSON args; used to match approval to parked request, **not** included in decision message wire format.

---

## 8. OCSF mapping

Exporter: `DefaultOcsfExporter` (`trogon-traffic-view/src/siem/ocsf.rs:7-15`).

Normalized input: `TrafficEvent` (`event.rs:35-49`) produced by `normalize_envelope` (`normalize.rs:7-109`).

### 8.1 STS → Authorization Activity

| OCSF field | Value / source |
|---|---|
| `class_uid` | `3003` |
| `class_name` | `Authorization Activity` |
| `type_uid` | `300301` |
| `activity_name` | `Authorize` |
| `category_uid` | `3` (Identity & Access Management) |
| `service.name` | `trogon-sts` |
| `actor.user.uid` | `caller_sub` ← `agent_id` or minted `sub` |
| `src_endpoint.name` | `caller_wkl` |
| `status_id` | `1` Success if outcome `allow`; `2` Failure if `deny`/`rate_limited`; `99` Other |
| `time` | `event.ts` epoch ms |

**Unmapped (placed in `unmapped` object):**

- `trogon.event_id`
- `trogon.tenant`
- `trogon.audience` ← `target_aud`
- `trogon.purpose`
- `trogon.scope`
- `trogon.reason` ← `decision_reason`
- `trogon.session_id`

(`ocsf.rs:47-55`)

### 8.2 Gateway and registry → Application Activity

| OCSF field | Value / source |
|---|---|
| `class_uid` | `6002` |
| `class_name` | `Application Activity` |
| `type_uid` | `600201` |
| `activity_name` | `Access` |
| `category_uid` | `6` |
| `dst_endpoint.name` | `target_aud` ← gateway `subject_out` |
| `status_id` | Same rules as §8.1 |

**Unmapped:**

- `trogon.event_id`
- `trogon.tenant`
- `trogon.purpose`
- `trogon.session_id`
- `trogon.request_id`

(`ocsf.rs:88-94`)

**Note:** [agent-traffic.md](agent-traffic.md) §7 mentions class `3002` Authentication for STS in illustrative samples; the **implemented** exporter uses class **`3003`** Authorization Activity (`ocsf.rs:25-26`, test at `ocsf.rs:142`).

### 8.3 Approvals

No OCSF mapping in `trogon-traffic-view` today. **Proposed:** map grant/deny to OCSF `Authorization Activity` with `activity_id` for policy decision, using `approver` as `actor.user.uid`.

### 8.4 NDJSON export

`emit_ndjson` joins one OCSF JSON object per line (`ocsf.rs:106-112`).

---

## 9. Versioning policy

`envelope_version` follows semver attached to **this reference document**, not individual producer binaries.

| Bump | When | Consumer rule |
|---|---|---|
| **Patch** | Documentation clarifications; new well-known `decision_reason` string; new optional subject suffix | Consumers MUST ignore unknown fields (JSON forward compatibility) |
| **Minor** | New **optional** payload field on any family; new audit subject under existing prefix | Consumers MUST accept records with or without the new field |
| **Major** | Remove, rename, or change type of an existing field; change meaning of `outcome` enum; change subject grammar | Consumers MUST opt in via explicit `envelope_version` range or separate consumer |

Producers SHOULD emit `envelope_version` on all new code paths once the common header lands. Until then, indexers derive `event_id` and timestamp heuristically (`envelope.rs:17-34`, `normalize.rs:111-127`).

---

## 10. Sample envelopes

Samples below match **wire structs today** unless noted. Optional fields omitted when null per `skip_serializing_if`.

### 10.1 Gateway — allow (success)

**Subject:** `mcp.audit.allow.request.tools`

```json
{
  "subject_in": "mcp.gateway.request.fs.tools.call",
  "subject_out": "mcp.server.fs.tools.call",
  "outcome": "allow",
  "direction": "request",
  "jsonrpc_method": "tools/call",
  "tenant": "acme",
  "caller_sub": "agent:acme/oncall-agent",
  "jwt_issuer": "https://sts.trogon.ai/acme",
  "identity_source": "jwt",
  "request_id": "01JXYZ9ABCDEF0123456789AB"
}
```

### 10.2 Gateway — deny (policy)

**Subject:** `mcp.audit.deny.request.tools`

```json
{
  "subject_in": "mcp.gateway.request.fs.tools.call",
  "subject_out": "mcp.server.fs.tools.call",
  "outcome": "deny",
  "direction": "request",
  "jsonrpc_method": "tools/call",
  "tenant": "acme",
  "caller_sub": "agent:acme/oncall-agent",
  "jwt_issuer": "https://sts.trogon.ai/acme",
  "identity_source": "jwt",
  "request_id": "01JXYZ9ABCDEF0123456789AC"
}
```

**Proposed header overlay** (not on wire today):

```json
{
  "envelope_version": "1.0.0",
  "event_id": "01JXYZ9ABCDEF0123456789AC",
  "event_time": "2026-05-28T14:02:04Z",
  "producer": "mcp-gateway",
  "tenant_id": "acme",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "span_id": "b7ad6b7169203331",
  "correlation_id": "01JXYZ9ABCDEF0123456789AC",
  "outcome": "deny",
  "decision_reason": "policy_deny"
}
```

### 10.3 Gateway — error (backend timeout)

**Subject:** `mcp.audit.error.request.tools`

```json
{
  "subject_in": "mcp.gateway.request.fs.tools.call",
  "subject_out": "mcp.server.fs.tools.call",
  "outcome": "error",
  "direction": "request",
  "jsonrpc_method": "tools/call",
  "tenant": "acme",
  "caller_sub": "agent:acme/oncall-agent",
  "identity_source": "jwt",
  "request_id": 42
}
```

### 10.4 STS — success

**Subject:** `mcp.audit.sts.success`

```json
{
  "outcome": "success",
  "decision_reason": "exchange_ok",
  "latency_ms": 12,
  "source_ip": "10.0.0.4",
  "request": {
    "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
    "actor_token": "<REDACTED_SVID>",
    "audience": "urn:trogon:a2a:agent:acme:oncall-agent",
    "scope": "mcp:tools",
    "purpose": "incident.response",
    "requested_token_type": "urn:ietf:params:oauth:token-type:jwt"
  },
  "minted": {
    "sub": "agent:acme/triage-router",
    "iss": "https://sts.trogon.ai/acme",
    "aud": "urn:trogon:a2a:agent:acme:oncall-agent",
    "exp": 1748347320,
    "iat": 1748347200,
    "agent_id": "acme/oncall-agent",
    "wkl": "spiffe://acme.local/ns/prod/sa/oncall-agent",
    "purpose": "incident.response",
    "scope": "mcp:tools",
    "act_chain_depth": 2
  },
  "wkl": "spiffe://acme.local/ns/prod/sa/triage-router",
  "agent_id": "acme/triage-router"
}
```

### 10.5 STS — deny (act chain revoked)

**Subject:** `mcp.audit.sts.deny`

```json
{
  "outcome": "deny",
  "decision_reason": "act_chain_entry_revoked",
  "latency_ms": 8,
  "request": {
    "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
    "actor_token": "<REDACTED_SVID>",
    "audience": "urn:trogon:a2a:agent:acme:oncall-agent",
    "scope": "",
    "purpose": "incident.response",
    "requested_token_type": "urn:ietf:params:oauth:token-type:jwt"
  },
  "offending_index": 1,
  "offending_agent_id": "acme/legacy-bot"
}
```

### 10.6 Registry — lookup found

**Subject:** `mcp.audit.registry.lookup.found`

```json
{
  "agent_id": "acme/oncall-agent",
  "tenant_hint": "acme",
  "outcome": "found"
}
```

### 10.7 Registry controller — registered

**Subject:** `mcp.audit.registry.registered`

```json
{
  "event_type": "registered",
  "agent_id": "acme/oncall-agent",
  "agent_version": "1.0.0",
  "prior_lifecycle_state": null,
  "new_lifecycle_state": "active",
  "manifest_digest": "sha256:abc123…",
  "operator": "registry-controller",
  "kv_revision": 17,
  "git_commit": "deadbeef"
}
```

### 10.8 Approvals — grant message

**Subject:** `mcp.approvals.req-abc`

```json
{
  "decision": "approve",
  "approver": "human:alice@acme",
  "expires_at": 1748347500
}
```

---

## 11. Redaction and PII

Redaction module: `trogon-mcp-gateway/src/redaction/` (`mod.rs:1-11`). Engine applies JSONPath rules (`rule.rs:26-30`: `Mask`, `Hash`, `Drop`, `Replace`) before audit/anomaly/egress paths. YAML ruleset loading is **pending** (`ruleset.rs:22-24`); `redact()` is currently a no-op stub (`engine.rs:7-8`).

### 11.1 Fields that MAY contain PII or secrets

| Field / family | Risk | Handling |
|---|---|---|
| `caller_sub`, `sub`, `approver` | User identifiers | MAY leave trusted audit stream; hash/mask in SIEM export |
| `subject_in` / `subject_out` | May embed tenant/server ids | Low; keep for routing forensics |
| STS `request.actor_token` | **Secret** — SVID/JWT | MUST NOT ship to SIEM; replace with `hash:` fingerprint **proposed** |
| STS `request` (full) | Token material if extended | Exclude raw `subject_token` (already omitted) |
| `wkl` | Infrastructure identity | Usually not human PII; tenant-scoped |
| `purpose` | Business intent | May describe sensitive operations |
| `session_id` | Session correlation | Pseudonymous; treat as personal data in GDPR contexts |
| `source_ip` | Network PII | Mask last octet in exported copies **proposed** |
| JSON-RPC `request_id` | Client correlation | Low sensitivity |
| `jwt_issuer` | Metadata | Public |
| Approval `approval_url` | May contain tokens in query **proposed** | Strip query params in logs |

### 11.2 MUST redact before leaving trust boundary

Apply rules (order matters — `engine.rs:2-3`):

1. **`Drop`** raw tokens: `actor_token`, any future `subject_token` echo, Authorization headers.
2. **`Hash`** stable correlators when PII retention is disallowed: `caller_sub`, `approver` (SHA-256 hex).
3. **`Mask`** partial strings: `source_ip` → `10.0.0.0/24` representation.
4. **`Replace`** constant placeholders for removed objects: `"<REDACTED>"`.

Operators SHOULD configure rulesets per destination:

| Destination | Policy |
|---|---|
| JetStream `MCP_AUDIT` (legal) | Full envelope minus raw tokens; retain `decision_reason` |
| Postgres traffic index | Normalized columns only (`indexer/postgres.rs`); no raw tokens |
| OCSF NDJSON / SIEM | Use exporter output; unmapped fields may still include tenant/purpose — add SIEM-side masking |
| Object storage archive | Encrypt at rest; apply same rules as legal stream |

### 11.3 Gateway egress shadow metrics (not audit)

Audience mismatch shadow events publish to `mcp.metrics.gateway.aud_mismatch_shadow` (`egress/audience.rs:4-76`) — metrics schema, not audit envelope. Do not merge with STS audit in compliance reports.

### 11.4 Cross-reference

- [agent-traffic.md](agent-traffic.md) §2 — indexed columns vs raw envelope
- [sts-exchange.md](sts-exchange.md) § Audit emission — excluded JWT material
- [integration-touchpoints.md](integration-touchpoints.md) § Traffic / OCSF — consumer ownership
- `a2a-redaction` signed bundle verification (`a2a-redaction/src/signed_bundle/verify.rs`) — envelope version checks for a separate redaction packaging flow

---

## 12. Quick lookup — producer → subject → Rust type

| Subject example | Struct | File:line |
|---|---|---|
| `mcp.audit.allow.request.tools` | `AuditEnvelope` | `trogon-mcp-gateway/src/audit.rs:33` |
| `mcp.audit.sts.success` | `StsAuditEvent` | `trogon-sts/src/audit.rs:11` |
| `mcp.audit.registry.lookup.found` | `LookupAuditEvent` | `trogon-agent-registry/src/audit.rs:14` |
| `mcp.audit.registry.put` | `MutationAuditEvent` | `trogon-agent-registry/src/audit.rs:22` |
| `mcp.audit.registry.registered` | `AuditEvent` | `trogon-agent-registry-controller/src/audit.rs:30` |
| `mcp.approvals.req-1` | `ApprovalDecisionMessage` | `trogon-mcp-gateway/src/approvals/types.rs:77` |

---

## 13. Changelog (reference doc)

| Version | Date | Notes |
|---|---|---|
| `1.0.0` | 2026-05-28 | Initial Diátaxis reference (Block H). Documents wire shapes from `yordis/agentgateway` branch. |

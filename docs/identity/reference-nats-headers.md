# NATS message headers reference

**Diátaxis:** reference (strict, exhaustive).

**Status:** Phase 1 contract. Headers are authoritative for routing and policy; the JSON-RPC payload is for protocol semantics. Anything not in this list is allowed to change.

**Wire-format version:** `trogon.mcp/v1` (header `mcp-schema`).

**Related:** [reference-subject-grammar.md](reference-subject-grammar.md), [reference-audit-envelope.md](reference-audit-envelope.md), [act-chain.md](act-chain.md), [ADR 0002](../adr/0002-identity-layers.md), [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pin 1.

**Implementation anchors:**

| Concern | Crate / path |
|---|---|
| `mcp-act-chain` projection | `trogon-mcp-gateway/src/act_chain.rs` |
| `mcp-session-id` | `trogon-mcp-gateway/src/egress/cache.rs` |
| W3C trace inject | `trogon-nats/src/messaging.rs` (`inject_trace_context`) |
| Act-chain types | `trogon-identity-types` (`MAX_ACT_CHAIN_DEPTH = 8`) |

Unless marked **proposed**, fields below match [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pin 1.

---

## 1. Header catalog

Every gateway-handled NATS message in the edge zone (`{prefix}.gateway.request.>`) and backend zone (`{prefix}.server.*`, `{prefix}.client.*`) carries the pinned header set below. CEL rules read headers via `nats.headers` ([MCP_GATEWAY_PLAN.md §8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace)).

### 1.1 Full header table

| Header | Direction | Type | Source | Purpose | Default |
|---|---|---|---|---|---|
| `traceparent` | both | W3C string | client (or gateway if absent) | Distributed-tracing parent span ([W3C Trace Context](https://www.w3.org/TR/trace-context/)). | Absent on client ingress; gateway generates on egress when absent. |
| `tracestate` | both | W3C string | client / gateway | Vendor-specific trace state; pass-through. | Absent. |
| `mcp-schema` | both | string, e.g. `trogon.mcp/v1` | gateway sets on egress | Wire-format version. Major bumps break consumers. | `trogon.mcp/v1` on gateway egress. |
| `mcp-session-id` | both | opaque string | gateway issues at `initialize` | Stable across an MCP session. Routes to session KV. | Absent until `initialize` completes; required on post-init edge messages. |
| `mcp-caller-sub` | gateway → backend | string | gateway, from JWT `sub` | Authenticated principal. Backend may log or refuse. | Set by gateway on egress; never trusted from client ingress. |
| `mcp-tenant` | gateway → backend, audit | string | gateway, from JWT claim | Tenant identity mirror for log legibility — never trusted as authority. | Set by gateway on egress from validated JWT `tenant` claim. |
| `mcp-deadline-unix-ms` | client → gateway → backend | integer string | client (gateway clamps) | Absolute deadline, milliseconds since epoch. Gateway propagates after clamping to configured max. | Absent (no deadline). |
| `mcp-correlation-id` | both | string | client (optional) | Opaque client correlator surfaced in audit envelope. | Absent. |
| `mcp-instance-id` | gateway → backend | string | gateway | Which gateway instance handled the request. Reply-inbox routing and audit. | Gateway boot NUID (`mcp.control.gateway.heartbeat.{instance_id}`). |
| `mcp-act-chain` | client → gateway, gateway → backend | compact JSON string | gateway (egress); client may send but is stripped on ingress | Delegation lineage. See §5. | Absent on ingress; gateway sets on egress when agent identity mode is not `off`. |

**Direction legend:**

| Value | Meaning |
|---|---|
| both | Present on edge ingress (client → gateway) and egress (gateway → client or backend). |
| gateway → backend | Set or overwritten by gateway before publish to `{prefix}.server.*` or callback relay. |
| client → gateway → backend | Client may supply; gateway may clamp or strip before forwarding. |

---

## 2. Ingress hardening rule

On every message entering the gateway from the edge zone, the gateway **drops** any client-supplied value of the following headers and replaces them with values derived from the validated JWT and gateway state:

| Header dropped on ingress | Replacement source | Why |
|---|---|---|
| `mcp-caller-sub` | JWT `sub` after signature and claim validation | Clients cannot forge the authenticated principal. |
| `mcp-tenant` | JWT `tenant` claim (or derived tenant binding) | Tenant boundary is claim-bound, not header-bound. |
| `mcp-instance-id` | Gateway process instance id (boot NUID) | Instance identity is gateway-owned; clients cannot redirect reply routing. |
| `mcp-schema` | Gateway wire-format version (`trogon.mcp/v1`) | Schema version is operator-controlled, not client-selected. |

**Additional ingress rule (`mcp-act-chain`):** The gateway **drops** any client-supplied `mcp-act-chain` value. Delegation lineage comes from the validated JWT `act_chain` claim (when present) and gateway append logic on egress — not from client headers. See §5.

**Authority ordering:** When a validated egress JWT and `mcp-act-chain` disagree, the JWT is authoritative; the header is a convenience projection for backends that do not parse JWTs ([act-chain.md § Header projection](act-chain.md#header-projection)).

**Identity never from JSON-RPC alone:** Tenant and caller fields in the JSON-RPC payload may exist for protocol semantics but do not override hardened headers or JWT claims for policy.

---

## 3. Header lifecycle

Per header: who sets it, who clamps or transforms it, who strips it.

| Header | Set by | Clamp / transform | Strip |
|---|---|---|---|
| `traceparent` | Client on ingress if present; gateway via `inject_trace_context` when absent or on new child span | Invalid W3C value treated as absent; gateway starts new root trace ([otel-wiring.md §2.2](otel-wiring.md#22-json-rpc-_metatraceparent)) | Never stripped; gateway overwrites only when starting a new root |
| `tracestate` | Client; gateway may append vendor entries on inject | Pass-through; gateway MUST NOT remove vendor entries ([otel-wiring.md §2.1](otel-wiring.md#21-propagation-matrix)) | Never stripped |
| `mcp-schema` | Gateway on egress | Fixed to configured wire version | **Stripped on ingress** (client value dropped) |
| `mcp-session-id` | Gateway at `initialize`; client echoes on subsequent edge messages | Gateway validates session exists in KV before forward | Not stripped; invalid or missing session rejected by policy |
| `mcp-caller-sub` | Gateway from JWT `sub` on egress | None | **Stripped on ingress** (client value dropped) |
| `mcp-tenant` | Gateway from JWT `tenant` on egress | None | **Stripped on ingress** (client value dropped); also stripped on egress when legacy JWT ingress modes require it |
| `mcp-deadline-unix-ms` | Client (optional) | Gateway clamps to `min(client_value, now + configured_max_deadline_ms)` before backend forward | Not stripped; absent means no explicit deadline header |
| `mcp-correlation-id` | Client (optional) | None | Not stripped; propagated both directions when present |
| `mcp-instance-id` | Gateway (boot NUID) | None | **Stripped on ingress** (client value dropped) |
| `mcp-act-chain` | Gateway on egress from JWT `act_chain` + gateway hop append | Max depth 8; at capacity forward without append (§5) | **Stripped on ingress** (client value dropped) |

### 3.1 `mcp-deadline-unix-ms` clamping

| Stage | Behavior |
|---|---|
| Ingress | Parse as unsigned integer string (milliseconds since Unix epoch). Non-numeric or empty → treat as absent. |
| Clamp | `effective_deadline = min(client_deadline, gateway_now_ms + max_deadline_ms)` where `max_deadline_ms` is bundle/KV configurable. |
| Egress | Gateway sets clamped value on backend publish; CEL exposes `request.deadline` as timestamp. |
| Absent | No header on wire; backend timeout falls back to gateway default request timeout. |

---

## 4. Tracing headers

### 4.1 `traceparent`

**Format (W3C Trace Context):**

```text
{version}-{trace-id}-{parent-id}-{trace-flags}
```

| Field | Length / charset | Semantics |
|---|---|---|
| `version` | `00` (hex) | Version of the specification. |
| `trace-id` | 32 hex chars (16 bytes) | Trace identifier shared across services. |
| `parent-id` | 16 hex chars (8 bytes) | Span id of the incoming request in the sending service. |
| `trace-flags` | 2 hex chars | Bit 0 (`01`) = sampled. |

**Example:** `00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01`

| Direction | Setter |
|---|---|
| Client → gateway | Client when distributed tracing is enabled. |
| Gateway → backend | Gateway via `trogon_nats::inject_trace_context` — continues active span or creates root when ingress header absent. |
| Gateway → client (reply) | Gateway injects current span context on edge reply. |

JSON-RPC error `data.trace_id` matches the trace id from the active `traceparent` ([MCP_GATEWAY_PLAN.md §6](../../MCP_GATEWAY_PLAN.md#6-gateway-emitted-json-rpc-error-codes)).

### 4.2 `tracestate`

**Format:** Comma-separated list of `key=value` pairs per W3C Trace Context Level 2.

| Rule | Detail |
|---|---|
| Ingress | Client-supplied `tracestate` preserved. |
| Gateway | MAY append vendor-specific keys; MUST NOT delete client/vendor entries. |
| Absent | Gateway MAY omit on egress; backends tolerate absence. |

Trace context is **not** embedded in JWT claims ([otel-wiring.md §2.3](otel-wiring.md#23-jwt-claims-and-trace-context)).

---

## 5. `mcp-act-chain` deep dive

Compact JSON string header mirroring the JWT `act_chain` claim plus the gateway hop on egress.

**Related:** [act-chain.md](act-chain.md), [ADR 0002](../adr/0002-identity-layers.md), `trogon-mcp-gateway/src/act_chain.rs`.

### 5.1 Shape

JSON array of hop objects. Plain UTF-8 JSON — **not** base64url.

```json
[
  {"sub":"user:alice@acme.com","wkl":"human","iat":1748341200},
  {"sub":"agent:acme/oncall-responder","agent_id":"acme/oncall-responder","wkl":"spiffe://acme.local/ns/prod/sa/oncall-responder","iat":1748341203}
]
```

**Compact serialization on egress:** no insignificant whitespace; per-object key order `sub`, `agent_id`, `wkl`, `iat`; omit `agent_id` when absent.

### 5.2 Entry fields

| Field | Type | Required | Semantics |
|---|---|---|---|
| `sub` | string | yes | Principal at this hop (`user:…`, `agent:…`, `job:…`). |
| `agent_id` | string | no | Registry agent id when hop is agent-originated. Omitted for human, batch, and gateway hops without registered agent. |
| `wkl` | string | no on gateway hop | Workload identity (SPIFFE URI) or documented sentinel (`human`, `batch`). Gateway egress hop may omit when gateway is not a registered agent. |
| `iat` | number (integer) | yes | Unix timestamp **seconds** when this hop was appended. |

No other fields permitted in v1. Unknown fields → reject in enforce mode ([act-chain.md § Verification](act-chain.md#verification-rules)).

### 5.3 Ordering and depth

| Property | Value |
|---|---|
| Index 0 | Oldest hop (originator). |
| Last index | Immediate caller before gateway append (from JWT). |
| Max depth | **8** entries (`MAX_ACT_CHAIN_DEPTH` in `trogon-identity-types`). |
| Gateway hop | Appended on egress with `sub` from `MCP_GATEWAY_IDENTITY_SUB` (default `trogon-mcp-gateway`), `iat` = gateway clock. |

### 5.4 At capacity (`act_chain_too_deep`)

When the parsed chain (from JWT, after ingress header drop) already has length **≥ 8**:

| Behavior | Detail |
|---|---|
| Forward | Gateway forwards **without** appending its hop. |
| Log | Structured log `event = act_chain_too_deep` with `depth` and `max_depth = 8`. |
| Header | Existing chain re-serialized to `mcp-act-chain` when non-empty. |
| Enforce rejection | **Reserved** — future enforce mode may return JSON-RPC error instead of forward-only log. |

### 5.5 Ingress / egress flow

```text
Client ingress                Gateway                         Backend egress
─────────────                 ───────                         ──────────────
mcp-act-chain (DROP)    →     Parse JWT act_chain      →     mcp-act-chain = JWT chain + gateway hop
                              Append gateway hop              (unless too deep)
```

| Mode (`MCP_GATEWAY_AGENT_IDENTITY`) | Egress header |
|---|---|
| `off` | Not set (no-op). |
| `shadow` / `enforce` | Projected per rules above. |

---

## 6. Audit envelope mirror

Audit payloads publish to JetStream stream `MCP_AUDIT`. Header values map to envelope fields as follows.

### 6.1 Mirrored (header → audit)

| NATS header | Audit envelope field | Notes |
|---|---|---|
| `mcp-correlation-id` | `correlation_id` (common header) / `request_id` (gateway wire today) | Opaque client correlator; JSON-RPC `id` used when header absent. |
| `mcp-instance-id` | `instance_id` | Top-level in target schema ([MCP_GATEWAY_PLAN.md §7](../../MCP_GATEWAY_PLAN.md#7-audit-envelope-schema)). |
| `mcp-session-id` | `session_id` | Top-level and `caller` context when identity block populated. |
| `mcp-tenant` | `tenant` / `tenant_id` | Mirror for legibility; authority remains JWT claim. |
| `mcp-caller-sub` | `caller.sub` / `caller_sub` | Populated from JWT on publish; header mirrors JWT after hardening. |
| `mcp-act-chain` | `act_chain` / `caller.act_chain` | Full array; `caller.originator_sub` = `act_chain[0].sub`, `caller.chain_depth` = length ([act-chain.md § Audit](act-chain.md#audit-envelope-embedding)). |
| `traceparent` | `trace_id`, `span_id` | Parsed W3C ids; **proposed** on all producers ([reference-audit-envelope.md §2.1](reference-audit-envelope.md#21-header-field-reference)). |

Identity fields (`agent_id`, `wkl`, `purpose`) in audit originate from **JWT claims**, not from NATS headers directly, though egress headers mirror JWT for backends.

### 6.2 Wire-only (not mirrored to audit)

| NATS header | Reason |
|---|---|
| `tracestate` | Vendor trace baggage; OTel/SIEM use `traceparent` ids. |
| `mcp-schema` | Audit uses its own `schema` field (`trogon.mcp.audit/v1`), not MCP wire version. |
| `mcp-deadline-unix-ms` | Request timing; may appear in CEL `request.deadline` but not required in audit JSON today. |

### 6.3 Audit-only (no NATS header)

| Audit field | Source |
|---|---|
| `subject_in`, `subject_out` | NATS subject, not a header. |
| `jsonrpc_method`, `method_root`, `tool` | JSON-RPC payload parsing. |
| `decision`, `rules_fired`, `spicedb`, `latency_us` | Gateway policy engine. |
| `jwt_issuer`, `identity_source` | JWT validation metadata. |

---

## 7. Header forbidden list

Phase 1 explicitly disallows the following on gateway-handled messages:

| Category | Rule |
|---|---|
| Ad-hoc namespaces | Any `x-*` header. No custom vendor prefixes on the MCP wire in Phase 1. Use JWT claims or audit `extra` for extensibility. |
| Oversized values | Any single header value **> 4 KiB** (4096 bytes UTF-8). Oversize values are dropped or the message rejected (**proposed** enforce behavior; operators SHOULD size `mcp-act-chain` below this limit at depth 8). |
| Secrets in headers | Headers MUST NOT carry bearer tokens, API keys, passwords, or raw JWT material. Authenticate via validated JWT on the message or connection; use `Authorization` only at transport edges that define it, not as MCP policy headers. |
| Unpinned `mcp-*` headers | Any `mcp-*` name not in §1.1. |
| Identity forgery | Client-supplied `mcp-caller-sub`, `mcp-tenant`, `mcp-instance-id`, `mcp-schema`, `mcp-act-chain` — always dropped on ingress (§2). |

---

## 8. Versioning (`mcp-schema`)

| Header value | Meaning |
|---|---|
| `trogon.mcp/v1` | Phase 1 wire contract (this document). |

### 8.1 Change classes

| Change type | `mcp-schema` bump | Consumer rule |
|---|---|---|
| **Additive** | Same major (`trogon.mcp/v1`) | New optional headers MAY be added in a future minor doc revision without bumping `mcp-schema`. Existing consumers MUST ignore unknown headers. |
| **Additive header fields** | Same major | New optional semantics on existing headers (documented in reference) — tolerated when absent. |
| **Breaking** | New major (`trogon.mcp/v2`) | Rename, remove, or change type/requiredness of a pinned header; change ingress hardening set; change `mcp-act-chain` entry schema. Consumers MUST opt in explicitly. |

Major bumps break SDK parsers, CEL rules referencing `nats.headers`, and backend log pipelines. Coordinate with audit envelope `schema` bumps only when audit fields change — wire and audit versions are independent.

### 8.2 Deployment rule

Gateway sets `mcp-schema: trogon.mcp/v1` on every egress message. Backends SHOULD log unknown major values and refuse policy decisions that depend on unpinned headers.

---

## 9. Quick lookup — phase and zone

| Zone | Headers applied |
|---|---|
| Edge ingress (`{prefix}.gateway.request.>`) | All §1.1 headers client may send; §2 hardening on identity headers; deadline clamp §3.1 |
| Edge egress (reply to client) | `traceparent`, `tracestate`, `mcp-schema`, `mcp-session-id`, `mcp-correlation-id`; identity headers not forged to client unless protocol requires |
| Backend egress (`{prefix}.server.*`) | Full §1.1 gateway-set headers including `mcp-caller-sub`, `mcp-tenant`, `mcp-instance-id`, `mcp-act-chain` |
| Callback relay (`{prefix}.client.*`) | Same rules as backend egress for gateway-initiated backend → client traffic |

---

## 10. Cross-references

| Document | Relevance |
|---|---|
| [reference-subject-grammar.md](reference-subject-grammar.md) | NATS subjects carrying these headers |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit field mapping (§6) |
| [act-chain.md](act-chain.md) | JWT claim schema, verification, CEL `chain.*` helpers |
| [ADR 0002](../adr/0002-identity-layers.md) | Identity triplet (`wkl`, `agent_id`, originator) |
| [otel-wiring.md](otel-wiring.md) | W3C propagation, span attributes from headers |
| [mcp-session-model.md](mcp-session-model.md) | `mcp-session-id` lifecycle |
| [reply-correlation.md](reply-correlation.md) | `mcp-instance-id` and inbox routing |
| [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Wire-Format Pin 1 | Authoritative pin source |

---

## 11. Changelog (reference doc)

| Version | Date | Notes |
|---|---|---|
| `1.0.0` | 2026-05-28 | Initial Diátaxis reference (Block H, Wire-Format Pin 1). |

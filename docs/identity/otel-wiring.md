# OpenTelemetry wiring — MCP gateway observability spec

**Status:** Normative design spec (Block G paper). Satisfies [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block G “OpenTelemetry trace export from each phase + JetStream consumer for audit→SIEM”.

**Diátaxis:** Reference (span/metric catalogs, env tables, propagation matrix) + explanation (sections 1, 7, 8, 9 — why traces differ from audit, cardinality trade-offs, rollout phases).

**Related:** [integration-touchpoints.md](integration-touchpoints.md), [reference-audit-envelope.md](reference-audit-envelope.md), [agent-traffic.md](agent-traffic.md), [mcp-session-model.md](mcp-session-model.md), [overview.md](overview.md), [jwt-claim-schema.md](jwt-claim-schema.md).

**Implementation anchors (today):**

| Component | Crate / path | OTel today |
|---|---|---|
| Telemetry bootstrap | `rsworkspace/crates/trogon-telemetry` | Traces, metrics, logs via OTLP/HTTP JSON (`trace.rs`, `metric.rs`, `log.rs`); W3C propagator installed in `lib.rs` |
| NATS trace inject | `rsworkspace/crates/trogon-nats/src/messaging.rs` | `inject_trace_context` writes W3C headers on outbound messages |
| Gateway egress | `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs:417` | Calls `inject_trace_context` before backend fan-out |
| Gateway service name | `trogon-telemetry::ServiceName::TrogonMcpGateway` | `"trogon-mcp-gateway"` |
| Audit publish | `trogon-mcp-gateway/src/audit.rs` | JetStream publish; **no** trace headers or dedicated span yet |
| Redaction surface | `trogon-mcp-gateway/src/redaction/` | JSONPath rules before audit/anomaly/egress (engine stub today) |

Unless marked **proposed**, identifiers below are grounded in repo sources at the time of this document. Items marked **proposed** are design targets for Block G implementation; they do not imply shipped code.

Default MCP subject prefix is `mcp` (`mcp_nats::Config` / `MCP_PREFIX`). Tables use `{prefix}` where the prefix is configurable.

---

## 1. Goal

The MCP identity stack already emits **audit envelopes** (legal/compliance), **anomaly feature vectors** (risk pipeline), and **ad-hoc structured logs** (`tracing` JSON to stderr and optional file). Block G adds **OpenTelemetry (OTel)** so SREs can plug gateway, STS, registry, and traffic-indexer behavior into the existing Trogon observability stack (collector → backend APM/metrics/logs).

### 1.1 Three pillars

| Pillar | Purpose in this system | Primary consumer |
|---|---|---|
| **Traces** | End-to-end latency, dependency failures, per-hop gateway work (authz, SpiceDB, egress mint, backend NATS) | On-call debugging, SLO burn investigation |
| **Metrics** | Aggregated rates, histograms, saturation gauges — low-cardinality rollups | Dashboards, alerting, capacity planning |
| **Logs** | Human-readable detail, error stacks, startup/shutdown | Incident triage when span granularity is insufficient |

Traces answer “which hop added 400 ms?” Metrics answer “is deny rate climbing for tenant X?” Logs answer “what did SpiceDB return on this replica?”

### 1.2 Phase rollout (recommended)

| Phase | Ship | Defer | Rationale |
|---|---|---|---|
| **v1 (Block G)** | Traces + metrics from `trogon-mcp-gateway`, `trogon-sts`, `trogon-agent-registry`; trace context on NATS headers; span catalog in §3; metric catalog in §4 | Structured OTel **logs** export as first-class signal | `trogon-telemetry` already bridges `tracing` events to OTel logs, but gateway hot path still relies on audit + metrics for compliance; logs-as-OTel need redaction review (§7) |
| **v3+** | Cross-region trace correlation, tail-based sampling hooks, SIEM trace_id join (audit `trace_id` field) | — | Depends on multi-region Block G items |

**v1 success criteria:** An operator can open a trace for a `tools/call`, see gateway child spans (`mcp.gateway.authz`, `mcp.gateway.spicedb.check`, `mcp.gateway.egress`, `mcp.gateway.audit.publish`), and correlate `trace_id` with JSON-RPC error `data.trace_id` and (when present) audit envelope `trace_id` — without copying JWT material or tool arguments into span attributes.

### 1.3 Services in scope (v1)

| Binary | `service.name` (resource) | Trace role |
|---|---|---|
| `trogon-mcp-gateway` | `trogon-mcp-gateway` | Root `mcp.gateway.request` per ingress message |
| `trogon-sts` | **proposed** `trogon-sts` (add `ServiceName` variant) | Child spans under propagated context on `mcp.sts.exchange` |
| `trogon-agent-registry` | **proposed** `trogon-agent-registry` | Lookup/mutation spans on `mcp.registry.agent.lookup` |
| `trogon-traffic-view` projector | **proposed** `trogon-traffic-view` | Consumer span per JetStream message (`mcp.audit.>`) — v2 if v1 slips |

Out of v1 scope: `mcp-nats-server`, `mcp-nats-stdio` (they already init OTel for transport debugging; not gateway SLO path), `trogon-gateway` Slack ingress, A2A gateway (`a2a-gateway` has separate tier-3 redaction docs).

### 1.4 Non-goals

- Replacing JetStream audit as the legal record ([reference-audit-envelope.md](reference-audit-envelope.md)).
- Exporting anomaly feature payloads (`mcp.metrics.anomaly.features`) as span events — remain metrics/pub-sub ([integration-touchpoints.md §9](integration-touchpoints.md#9-gateway--anomaly-feature-pipeline)).
- Storing trace IDs in OAuth bootstrap JWT claims (§2.3).
- Auto-instrumenting every JSON-RPC field as a span attribute.

---

## 2. Trace propagation

Distributed tracing uses **W3C Trace Context** (`traceparent`, `tracestate`). Trogon already installs `TraceContextPropagator` in `trogon-telemetry::init_logger` and injects context via `trogon_nats::inject_trace_context`.

### 2.1 Propagation matrix

| Carrier | Header / field | Direction | Required v1 | Notes |
|---|---|---|---|---|
| NATS message headers | `tracestate` | both | Yes | Pass-through; gateway MUST NOT strip vendor entries |
| JSON-RPC request | `_meta.traceparent` | client → gateway | Yes (stdio/hybrid) | **Trogon convention** — see §2.2 |
| JSON-RPC request | `_meta.tracestate` | client → gateway | Optional | Mirror of W3C tracestate when `_meta` is the only carrier |
| Audit JetStream publish | NATS headers on `{prefix}.audit.*` | gateway → stream | Yes | Propagate active span context on publish (§3.5) |
| OAuth bootstrap JWT | — | — | **No** | Identity-bound, not trace-bound (§2.3) |
| Mesh egress JWT | — | — | **No** | Short-lived credential; trace stays in headers |

### 2.2 JSON-RPC `_meta.traceparent`

**Grep result (repo + MCP docs):** MCP specifies `_meta` as an extensible object on requests (e.g. `_meta.progressToken` in [mcp-session-model.md](mcp-session-model.md) and [bidirectional-enforcement.md](bidirectional-enforcement.md)). The MCP specification does **not** define `_meta.traceparent` as a standard field. Trogon adopts it as a **bridge convention** for transports without NATS headers (stdio, HTTP-to-NATS bridges, test harnesses).

**Ingress algorithm (normative):**

1. If NATS headers contain valid `traceparent`, extract parent context from headers (authoritative on NATS transport).
2. Else if JSON-RPC params or top-level request object contains `_meta.traceparent` (string, W3C format), extract parent context from `_meta`.
3. Else start a new root trace (gateway-generated trace id); still emit `traceparent` on outbound NATS and in JSON-RPC error `data.trace_id`.

**Egress / reply:** Gateway MUST NOT echo client `_meta` wholesale. Only trace fields listed above are forwarded. Strip unknown `_meta` keys unless a future MCP spec version standardizes them.

**Validation:** Invalid `traceparent` → treat as absent (new root) and increment metric `mcp_gateway_traceparent_parse_errors_total` (**proposed** counter, low cardinality, no labels).

### 2.3 OAuth bootstrap claims — no trace

Bootstrap JWTs ([jwt-claim-schema.md](jwt-claim-schema.md), [overview.md](overview.md)) prove NATS CONNECT identity. They are:

- Longer-lived than a single RPC trace.
- Verified once per connection, not per MCP method.
- Already carrying tenant, subject, and ACL-oriented claims.

**Decision:** Do **not** add `trace_id`, `traceparent`, or `tracestate` to bootstrap JWT claims. Trace context remains on per-message NATS headers and JSON-RPC `_meta`. STS mesh tokens ([sts-exchange.md](sts-exchange.md)) likewise exclude trace claims; correlation uses headers + audit envelope `trace_id` when unified ( [reference-audit-envelope.md §2.1](reference-audit-envelope.md#21-header-field-reference) ).

### 2.4 Subject lanes (where headers must flow)

| Subject pattern | Propagation obligation |
|---|---|
| `{prefix}.gateway.request.*` | Client → gateway: extract parent; gateway → self: child spans |
| `{prefix}.server.*` | Gateway → backend: `inject_trace_context` (implemented `gateway.rs:417`) |
| `{prefix}.audit.*` | Gateway / STS / registry → JetStream: inject on publish (**proposed** for audit path) |
| `mcp.sts.exchange` | Gateway client → STS: request headers carry trace (**proposed** on STS handler) |
| `mcp.registry.agent.lookup` | Gateway → registry: request headers (**proposed**) |

Backend MCP servers SHOULD start spans linked to extracted context when using `mcp-nats` (`headers_with_trace_context` helpers exist in `mcp-nats/src/nats/mod.rs`).

### 2.5 Header extraction (implementation sketch)

Use the same pattern as inject, inverted:

```rust
// Pseudocode — belongs in trogon-nats alongside inject_trace_context
struct HeaderMapExtractor<'a>(&'a HeaderMap);
impl Extractor for HeaderMapExtractor<'_> { /* get(key) */ }

let parent = global::get_text_map_propagator(|p| {
    p.extract(&HeaderMapExtractor(&headers))
});
let span = tracer.span_builder("mcp.gateway.request").with_parent_context(parent).start(&tracer);
```

JSON-RPC `_meta` extraction parses the W3C string and builds `SpanContext` without duplicating parser logic (single `traceparent` parse module in `trogon-nats` or `trogon-telemetry`).

### 2.6 `trace_id` in JSON-RPC errors

[Pinned contract](../../MCP_GATEWAY_PLAN.md#6-gateway-emitted-json-rpc-error-codes): every Trogon application error includes `data.trace_id` matching the active span's trace id (32 hex chars). This is the **client-visible correlator** when the client lacks OTel export. Implementation MUST derive `trace_id` from the active OTel span, not a separate UUID.

---

## 3. Spans the gateway emits

Span names follow `{domain}.{component}.{operation}` lowercase dot notation. Attributes use snake_case keys; MCP-specific keys use the `mcp.*` prefix where noted.

### 3.1 Span hierarchy (explanation)

```
mcp.gateway.request          (root — one per ingress NATS message)
├── mcp.gateway.authz        (CEL + policy bundle evaluation)
│   └── mcp.gateway.spicedb.check   (optional — when SpiceDB consulted)
├── mcp.gateway.egress       (STS mint + header rewrite + NATS to backend)
│   └── nats.request         (existing trogon-nats span — child)
└── mcp.gateway.audit.publish
```

Session KV operations ([mcp-session-model.md § Observability](mcp-session-model.md)) add sibling spans under the same trace (`mcp.session.*`), not under `mcp.gateway.request`, when session work happens outside a single RPC (e.g. background close).

### 3.2 `mcp.gateway.request` (root)

| Property | Value |
|---|---|
| **Span name** | `mcp.gateway.request` |
| **Kind** | `CONSUMER` (messaging semantic conventions) |
| **Parent** | Extracted W3C context (§2) or none |
| **Start** | First byte dequeued from `{prefix}.gateway.request.>` queue group |
| **End** | Reply sent to client inbox, or fire-and-forget publish complete, or early deny response |

**Required attributes:**

| Attribute | Type | Source | Example |
|---|---|---|---|
| `mcp.method` | string | JSON-RPC `method` | `tools/call` |
| `mcp.server_id` | string | Parsed from subject | `filesystem` |
| `tenant_id` | string | JWT `tenant` claim or `"default"` | `acme` |
| `agent_id` | string | JWT `agent_id` when present | `acme/oncall-agent` |

**Recommended attributes (bounded cardinality):**

| Attribute | Type | Notes |
|---|---|---|
| `messaging.system` | string | `nats` |
| `messaging.destination.name` | string | Full ingress subject |
| `mcp.session_id` | string | From `mcp-session-id` header when set |
| `mcp.direction` | string | `request` \| `callback` for client-originated notifications |
| `mcp.outcome` | string | `allow` \| `deny` \| `error` — mirrors audit outcome |
| `jsonrpc.error_code` | int | Set on deny/error paths only |

**Forbidden on span:** raw JWT, `Authorization`, JSON-RPC `params`, tool arguments, resource URIs with user content, full `act_chain` JSON (use `act_chain.depth` int instead if needed).

### 3.3 `mcp.gateway.authz`

| Property | Value |
|---|---|
| **Span name** | `mcp.gateway.authz` |
| **Parent** | `mcp.gateway.request` |
| **Start** | Before CEL/policy evaluation (`policy.rs`) |
| **End** | Policy decision known (allow, deny, fault) |

**Required attributes:**

| Attribute | Type | Source |
|---|---|---|
| `policy_id` | string | Active bundle id + version, e.g. `mcp-pack@1.2.3` |
| `decision` | string | `allow` \| `deny` \| `fault` |
| `cel_eval_ms` | int | Wall time for CEL evaluation only (excludes SpiceDB) |

**Notes:**

- When SpiceDB is not consulted (non-sensitive method), span still records `decision=allow` with `cel_eval_ms` if CEL ran; omit `mcp.gateway.spicedb.check` child.
- `decision=fault` maps to JSON-RPC `-32101` / `-32108` class errors.
- `policy_id` MUST be stable across replicas (loaded bundle pointer, not file path).

### 3.4 `mcp.gateway.spicedb.check`

| Property | Value |
|---|---|
| **Span name** | `mcp.gateway.spicedb.check` |
| **Parent** | `mcp.gateway.authz` |
| **Start** | Before gRPC `CheckBulkPermissions` |
| **End** | Response or error |

**Required attributes:**

| Attribute | Type | Source |
|---|---|---|
| `permission` | string | e.g. `call`, `read` — env-configured permission name |
| `resource` | string | SpiceDB resource id, e.g. `filesystem\|read_file` — **not** full params |
| `zed_token_age_ms` | int | Age of cached ZedToken used for `at_least_as_fresh`, or `0` if fresh read |
| `cache_hit` | bool | `true` when consistency used cached token (`spicedb.rs`) |

**Error handling:** gRPC failure → span status `ERROR`, gateway returns `-32107`; do not attach gRPC message text (may contain internal endpoints).

### 3.5 `mcp.gateway.egress`

| Property | Value |
|---|---|
| **Span name** | `mcp.gateway.egress` |
| **Parent** | `mcp.gateway.request` |
| **Start** | After allow decision, before STS mint / header rewrite |
| **End** | Backend NATS request completes or times out |

**Required attributes:**

| Attribute | Type | Source |
|---|---|---|
| `server_id` | string | Target MCP server id |
| `latency_ms` | int | End-to-end egress phase (mint + NATS round-trip) |

**Recommended attributes:**

| Attribute | Type |
|---|---|
| `mesh.minted` | bool — whether STS exchange ran |
| `mesh.cache_hit` | bool — `MeshEgressCache` hit |
| `mcp.agent_identity_mode` | string — `off` \| `shadow` \| `enforce` |

Existing `nats.request` span from `trogon-nats` remains a child covering wire time only.

### 3.6 `mcp.gateway.audit.publish`

| Property | Value |
|---|---|
| **Span name** | `mcp.gateway.audit.publish` |
| **Parent** | `mcp.gateway.request` |
| **Start** | Before JetStream publish to `{prefix}.audit.*` |
| **End** | Publish ack or failure logged |

**Required attributes:**

| Attribute | Type | Source |
|---|---|---|
| `subject` | string | Full audit subject, e.g. `mcp.audit.allow.request.tools` |
| `bytes` | int | Serialized JSON payload length |

**On failure:** Request path continues (best-effort audit per [integration-touchpoints.md §6](integration-touchpoints.md#6-gateway--nats-core-and-jetstream)); span status `ERROR`; increment `mcp_gateway_audit_publish_failures_total`.

**Trace headers on audit message:** Call `inject_trace_context` on JetStream publish headers so downstream SIEM/projector can extract the same trace (**proposed** — not in `audit.rs` today).

### 3.7 Span status and events

| Event | When | Attributes |
|---|---|---|
| `exception` | Panics / unexpected errors | `exception.type`, no message body if it may contain PII |
| `gateway.rate_limited` | `-32105` | `scope`, `retry_after_ms` |
| `gateway.backend_timeout` | `-32102` | `server_id`, `elapsed_ms` |

Do not emit span events containing audit envelope payloads.

### 3.8 STS and registry spans (v1 parity)

**STS (`trogon-sts`):** Span name **proposed** `mcp.sts.exchange` with attributes `outcome`, `audience`, `latency_ms`, `tenant_id`, `decision_reason` (enum string, not free text). Parent context from NATS headers on `mcp.sts.exchange`.

**Registry (`trogon-agent-registry`):** Span name **proposed** `mcp.registry.lookup` with `agent_id`, `outcome` (`found` \| `notfound` \| `revoked`), `latency_ms`.

These services MUST NOT duplicate gateway span names; they are downstream children when gateway initiates the call.

---

## 4. Metrics

All metric names use snake_case with `mcp_gateway_` prefix for gateway-emitted series. Units follow OTel conventions (`{request}`, `ms`, `{session}`).

Instrument types:

| Suffix / pattern | OTel instrument |
|---|---|
| `*_total` | Counter |
| `*_duration_ms` | Histogram (milliseconds) |
| `*_failures_total` | Counter |
| `active_*` | UpDownCounter or ObservableGauge |

### 4.1 `mcp_gateway_requests_total`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_requests_total` |
| **Type** | Counter |
| **Unit** | `{request}` |
| **Description** | Ingress JSON-RPC messages completed (allow, deny, or error) |

**Labels (allowed keys only):**

| Label | Values | Cardinality note |
|---|---|---|
| `method` | JSON-RPC method string | Bounded by MCP method set (~20) |
| `tenant` | Tenant slug | Bounded by tenant count |
| `outcome` | `allow` \| `deny` \| `error` | 3 |

**Example:** `mcp_gateway_requests_total{method="tools/call",tenant="acme",outcome="allow"}`

### 4.2 `mcp_gateway_request_duration_ms`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_request_duration_ms` |
| **Type** | Histogram |
| **Unit** | `ms` |
| **Description** | Wall time from ingress dequeue to client reply (or fire-and-forget complete) |

**Labels:**

| Label | Values |
|---|---|
| `method` | JSON-RPC method |
| `tenant` | Tenant slug |

Explicit histogram buckets (**proposed**): `5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000` ms — aligned with gateway SLO bands in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) latency baseline work.

### 4.3 `mcp_gateway_authz_decision_total`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_authz_decision_total` |
| **Type** | Counter |
| **Unit** | `{decision}` |

**Labels:**

| Label | Values | Notes |
|---|---|---|
| `decision` | `allow` \| `deny` \| `fault` | Same vocabulary as span |
| `reason` | Closed enum — see table below | **Not** free-text SpiceDB errors |
| `tenant` | Tenant slug | |

**Allowed `reason` values (v1):**

| `reason` | Meaning |
|---|---|
| `policy_allow` | CEL allow |
| `policy_deny` | CEL deny |
| `spicedb_deny` | PDP denied |
| `spicedb_unreachable` | `-32107` path |
| `policy_fault` | CEL/bundle error |
| `no_policy` | Default deny |
| `act_chain_deny` | Ingress chain unresolved |
| `rate_limited` | `-32105` |

New reasons require a doc update — do not emit dynamic SpiceDB error strings as label values.

### 4.4 `mcp_gateway_spicedb_cache_hit_total`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_spicedb_cache_hit_total` |
| **Type** | Counter |
| **Unit** | `{check}` |
| **Labels** | **None** (global counter) |

Increments when `CheckBulkPermissions` uses cached ZedToken (`cache_hit=true` on span). Separate from mesh egress cache metrics.

### 4.5 `mcp_gateway_audit_publish_failures_total`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_audit_publish_failures_total` |
| **Type** | Counter |
| **Unit** | `{publish}` |
| **Labels** | **None** |

Alert when rate &gt; 0 sustained — audit loss is compliance-impacting even though requests continue.

### 4.6 `mcp_gateway_active_sessions`

| Field | Value |
|---|---|
| **Name** | `mcp_gateway_active_sessions` |
| **Type** | UpDownCounter or ObservableGauge |
| **Unit** | `{session}` |
| **Description** | MCP sessions with KV record not closed (**proposed** — requires session KV) |

**Labels:**

| Label | Values |
|---|---|
| `tenant` | Tenant slug |

Per-replica gauge summed in dashboard — document as approximate under HA ([mcp-session-model.md](mcp-session-model.md)).

### 4.7 Existing metrics (do not rename)

| Metric / subject | Owner | Relation to OTel |
|---|---|---|
| `mcp.metrics.anomaly.features` pub | `trogon-mcp-gateway/src/anomaly.rs` | Parallel pipeline — not part of `mcp_gateway_*` namespace |
| `mcp.metrics.gateway.aud_mismatch_shadow` | `egress/audience.rs` | Shadow metric — keep separate |

### 4.8 Metric implementation location

Define instruments in `trogon-mcp-gateway/src/telemetry/metrics.rs` (**proposed** module per crate AGENTS.md). Register via `trogon_telemetry::meter("trogon-mcp-gateway")` at startup. Record in gateway hot path via thin helpers — no stringly label keys at call sites (use newtypes or enums converted to label values at the telemetry boundary).

---

## 5. Exporter

### 5.1 Transport and endpoint

| Setting | v1 target | Current `trogon-telemetry` |
|---|---|---|
| Protocol | **OTLP/gRPC** | OTLP/HTTP JSON (`SpanExporter::builder().with_http()`) |
| Endpoint | Configurable via env | Standard OTel env vars |
| TLS | Collector-managed | `reqwest-rustls` on HTTP exporter today |

**Migration note:** Block G implementation SHOULD switch gateway-family binaries to gRPC exporter (`with_tonic()`) or support both via `OTEL_EXPORTER_OTLP_PROTOCOL=grpc|http/protobuf`. Until then, operators may point HTTP exporter at port `4318` (collector default HTTP) vs `4317` (gRPC).

### 5.2 Environment variables

Standard OpenTelemetry SDK variables (supported by `opentelemetry-otlp` + `trogon-telemetry` init):

| Variable | Purpose | Default when unset |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | Collector base URL | `http://localhost:4318` (HTTP) / **proposed** `http://localhost:4317` (gRPC) |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | Trace-specific override | Falls back to `OTEL_EXPORTER_OTLP_ENDPOINT` |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | Metrics override | Same |
| `OTEL_EXPORTER_OTLP_HEADERS` | Auth headers `key=value,...` | Empty |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | Export timeout ms | `10000` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | **proposed** explicit protocol | `grpc` for v1 target |
| `OTEL_RESOURCE_ATTRIBUTES` | Extra resource attrs | None |
| `OTEL_SERVICE_VERSION` | `service.version` | Cargo package version at build time (**proposed** embed) |

**Service name:** Hardcoded per binary via `ServiceName` enum — **not** overridable by `OTEL_SERVICE_NAME` (matches `acp-nats-server` README pattern). Prevents accidental collision in shared-host dev environments.

**Gateway-specific resource attribute today:** `mcp.prefix` from `ResourceAttribute::mcp_prefix` (`resource_attribute.rs`). Do not duplicate prefix as a metric label on every series.

### 5.3 Resource attributes

| Attribute | Required | Cardinality | When to set |
|---|---|---|---|
| `service.name` | Yes | 1 per binary | Always |
| `service.version` | Yes | 1 per deploy | Release tag / crate version |
| `deployment.environment` | Recommended | env count | Via `OTEL_RESOURCE_ATTRIBUTES` |
| `mcp.prefix` | Yes (gateway) | 1 per process | NATS prefix config |
| `tenant.id` | **Conditional** | 1 per tenant | **Only** single-tenant deployments |

**`tenant.id` rule:** Set on resource **only** when the process serves exactly one tenant (dedicated NATS account / dedicated gateway replica). Multi-tenant fleet gateways MUST NOT set `tenant.id` on the resource — use span attribute `tenant_id` and metric label `tenant` instead. Putting `tenant.id` on resource in a shared fleet creates N duplicate metric streams per tenant × replica (cardinality explosion).

**Forbidden resource attributes:** `request_id`, `session_id`, `agent_id`, `trace_id`.

### 5.4 Shutdown

All binaries MUST call `trogon_telemetry::shutdown_otel()` on exit (already in `trogon-mcp-gateway/src/main.rs:151`). Flush batch span/metric exporters before process termination.

### 5.5 Collector expectations

Reference deployment (explanation): gateway → OTLP gRPC → OpenTelemetry Collector → vendor backend (Datadog, Tempo, Grafana Cloud, etc.). Collector handles tail sampling (v3), PII scrubbing for logs (v2), and attribute hash replacement for SIEM-bound exports.

---

## 6. Sampling

### 6.1 Head-based sampling (v1)

| Variable | Purpose | Default |
|---|---|---|
| `OTEL_TRACES_SAMPLER` | Sampler type | `parentbased_always_on` in dev; `parentbased_traceidratio` in prod |
| `OTEL_TRACES_SAMPLER_ARG` | Ratio for traceidratio | `1.0` dev; **proposed** `0.1` prod initial |

**Trace-context-aware (required):** Use `ParentBased` root sampler so upstream `traceparent` with sampled flag = 0 still records gateway spans when an upstream system forces collection, and respects upstream drop decisions when parent is not sampled.

**Implementation:** OpenTelemetry SDK default parent-based samplers satisfy this when extracting parent from W3C headers (§2.5). Gateway MUST NOT implement a custom sampler that ignores parent flags.

### 6.2 Deny/error bias (**proposed** v2)

Optional `mcp.gateway.sampling.always_record_deny=true` env — always record traces when `outcome=deny` or `jsonrpc.error_code` set, even when ratio sampler would drop. Requires custom sampler wrapper; not v1.

### 6.3 Metrics sampling

Metrics are **not** trace-sampled — all counters/histograms aggregate every request. Low-cardinality labels (§8) keep cost bounded.

### 6.4 Audit stream vs trace sampling

Audit envelopes publish **independently** of trace sampling. A dropped trace MUST NOT skip audit publish. Compliance path is orthogonal (§7).

---

## 7. Audit vs trace separation

### 7.1 Why two pipelines

| Dimension | Audit (JetStream `MCP_AUDIT`) | Trace (OTLP) |
|---|---|---|
| **Purpose** | Security, compliance, legal record | Performance, debugging, SLO |
| **Retention** | Long, immutable, hash-chained (**planned**) | Short (days–weeks) |
| **Audience** | GRC, SIEM, traffic indexer | SRE, platform engineering |
| **Payload richness** | Identity, outcome, subjects, decision context | Timing, dependency structure, error class |
| **Failure mode** | Best-effort publish; alert on failure | Best-effort export; drop under load |

[MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md): “OpenTelemetry traces emit alongside but the audit stream is the legal record.”

### 7.2 Correlation without duplication

Join keys (safe on both sides):

| Field | Audit | Trace |
|---|---|---|
| `trace_id` | **proposed** header field ([reference-audit-envelope.md](reference-audit-envelope.md)) | W3C trace id |
| `span_id` | **proposed** | Root or request span id |
| `tenant` / `tenant_id` | Envelope + header mirror | Span attribute |
| `request_id` | JSON-RPC id in envelope | **Not** a metric label; optional span attribute |
| `subject_in` / `subject_out` | Yes | Use `messaging.destination.name` only |

Operators correlate in the backend by `trace_id`. They MUST NOT expect full audit JSON in span attributes.

### 7.3 Redaction surface (traces and logs)

Apply the same **trust boundary** as [reference-audit-envelope.md §11](reference-audit-envelope.md#11-redaction-and-pii) and the gateway redaction module:

| Data class | Audit stream | Trace spans | OTel logs (v2) |
|---|---|---|---|
| Raw JWT / bearer | Must not appear | **Forbidden** | **Forbidden** |
| `actor_token` (STS) | Fingerprint only | **Forbidden** | **Forbidden** |
| `caller_sub`, `approver` | Allowed (legal) | Pseudonymize or omit | Hash |
| JSON-RPC `params` / results | Not on wire today | **Forbidden** | **Forbidden** |
| Tool names | Yes | OK (`mcp.method`, resource id) | OK |
| Full resource URI content | Partial in subjects | Normalize to server_id + method | Drop query strings |
| `act_chain` full JSON | Yes | `act_chain.depth` only | Omit |
| SpiceDB error text | No | Status only | Status code |
| Client IP | **proposed** audit field | Mask / omit | Mask |

**Redaction module:** [`trogon-mcp-gateway/src/redaction/`](../../rsworkspace/crates/trogon-mcp-gateway/src/redaction/) — `RedactionAction`: `Mask`, `Hash`, `Drop`, `Replace` ([tools-list-filtering.md §5](tools-list-filtering.md#5-schema-redaction-within-a-kept-tool-reference)). Traces MUST pass through the same denylist paths before setting span attributes when copying from JWT or JSON-RPC (**proposed** helper `telemetry::redact_attribute`).

**A2A tier-3 redaction** (`a2a-redaction`, [tier3-redaction.md](../a2a/reference/policy/tier3-redaction.md)) is a separate WASM boundary for A2A gateway — MCP gateway v1 uses JSONPath redaction module above.

### 7.4 What may appear on traces

| Allowed | Forbidden |
|---|---|
| Method names, server ids, outcome enums | Passwords, API keys, OAuth codes |
| Tenant slug, agent_id (registered id) | Free-form `purpose` text if high sensitivity (**proposed** truncate/hash) |
| Latency, cache hit booleans | Full JWT claims dump |
| JSON-RPC error **codes** | Error **messages** with user data |
| SpiceDB permission name | SpiceDB free-text denial |

When in doubt, drop the attribute. Traces are not a compliance archive.

---

## 8. Cost model and cardinality budget

### 8.1 Principle

Metric labels and span attributes are a **cardinality budget**. Prometheus-style backends explode when labels carry unbounded values. Gateway code MUST use typed enums at the telemetry boundary ([rsworkspace/crates/AGENTS.md](../../rsworkspace/crates/AGENTS.md)).

### 8.2 Allowed metric label keys (exhaustive v1)

| Label key | Used on |
|---|---|
| `method` | `mcp_gateway_requests_total`, `mcp_gateway_request_duration_ms` |
| `tenant` | All tenant-scoped counters/histograms/gauges above |
| `outcome` | `mcp_gateway_requests_total` |
| `decision` | `mcp_gateway_authz_decision_total` |
| `reason` | `mcp_gateway_authz_decision_total` (closed enum §4.3) |

**No other label keys in v1.** In particular:

| Forbidden label | Why |
|---|---|
| `request_id` | Unbounded per request |
| `session_id` | Unbounded |
| `server_id` | High but unbounded — use span attribute, not metric label |
| `tool` | Unbounded tool catalog |
| `trace_id` | Never a metric label |
| `agent_id` | Unbounded — use tenant + method for rollups |
| `subject` | NATS subject strings are high cardinality |

### 8.3 Span attribute budget

Low-cardinality span attributes (OK): §3 tables “Required” and “Recommended”.

High-cardinality (span only, not metrics): `messaging.destination.name`, `mcp.session_id`, JSON-RPC `request_id` as string attribute — acceptable at trace sample rate; still omit params.

### 8.4 Estimated series count (explanation)

Example fleet: 50 tenants, 15 methods, 3 outcomes, 8 deny reasons.

- `mcp_gateway_requests_total`: `50 × 15 × 3 = 2,250` series
- `mcp_gateway_request_duration_ms`: `50 × 15 = 750` histograms
- `mcp_gateway_authz_decision_total`: `50 × 3 × 8 = 1,200` series

This fits typical SaaS monitoring budgets. Adding `server_id` would multiply by backend count (often 100+) — rejected for metrics.

### 8.5 Trace volume

With `parentbased_traceidratio` at 0.1 and 1k RPS ingress, expect ~100 traces/s per gateway replica before child spans. Batch export (`SdkTracerProvider` with batch exporter) amortizes collector cost. Audit publish rate remains 1 envelope per decision regardless of sampling.

### 8.6 Alerting hints (non-normative)

| Signal | Alert |
|---|---|
| `mcp_gateway_audit_publish_failures_total` rate &gt; 0 | Page — compliance gap |
| `mcp_gateway_authz_decision_total{decision="fault"}` spike | Bundle/CEL misconfig |
| `mcp_gateway_request_duration_ms` p99 | SLO burn |
| Trace export errors (collector logs) | Telemetry pipeline degraded |

---

## 9. Cross-references and Block G checklist

### 9.1 Identity docs

| Document | Relevance |
|---|---|
| [integration-touchpoints.md](integration-touchpoints.md) | NATS subjects, audit stream, STS/registry wire paths for span linking |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit schema, `trace_id` header **proposed**, PII table §11 |
| [agent-traffic.md](agent-traffic.md) | Traffic indexer consumes audit; `trace_id` column for join |
| [mcp-session-model.md](mcp-session-model.md) | Session spans, `traceparent` propagation note |
| [sts-exchange.md](sts-exchange.md) | STS audit subjects; no trace in JWT |
| [jwt-claim-schema.md](jwt-claim-schema.md) | Claims safe vs forbidden on spans |
| [tools-list-filtering.md §5](tools-list-filtering.md#5-schema-redaction-within-a-kept-tool-reference) | Redaction engine paths |
| [overview.md](overview.md) | Identity layers — do not conflate bootstrap JWT with trace |

### 9.2 Redaction module paths

| Path | Role |
|---|---|
| [`trogon-mcp-gateway/src/redaction/mod.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/redaction/mod.rs) | Module entry |
| [`redaction/rule.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/redaction/rule.rs) | `RedactionAction`, `JsonPath` |
| [`redaction/engine.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/redaction/engine.rs) | Apply rules (stub today) |
| [`redaction/ruleset.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/redaction/ruleset.rs) | YAML loading **pending** |

### 9.3 MCP_GATEWAY_PLAN.md Block G mapping

| Block G item | This spec section |
|---|---|
| OpenTelemetry trace export from each phase | §1.2, §3, §5, §6 |
| JetStream consumer for audit→SIEM | §7 (orthogonal pipeline); consumer spans §1.2 v2 |
| Latency baseline | §4.2 histogram buckets |
| CLI trace requests | Uses same `trace_id` join §7.2 (**future** `agctl`) |

### 9.4 Implementation checklist (Block G code)

- [ ] Extract W3C context on gateway ingress (headers + `_meta.traceparent`)
- [ ] Emit span catalog §3 with typed attributes
- [ ] Register metrics §4 in `telemetry/metrics.rs`
- [ ] `inject_trace_context` on audit JetStream publish
- [ ] Switch or dual-support OTLP/gRPC per §5.1
- [ ] Wire `trogon-sts` and `trogon-agent-registry` with `ServiceName` + extract/inject
- [ ] Unit tests: forbidden attributes never set from JWT fixture
- [ ] Document operator env in gateway README (mirror `acp-nats-server` OTel table)

### 9.5 Wire-format pins (trace-related)

From [MCP_GATEWAY_PLAN.md § Wire-Format Pins](../../MCP_GATEWAY_PLAN.md#1-nats-message-headers):

| Header | OTel interaction |
|---|---|
| `traceparent` | Extract/inject W3C context |
| `tracestate` | Pass-through |
| `mcp-correlation-id` | Audit envelope only — not OTel baggage |
| `mcp-session-id` | Span attribute; links to session spans |

---

## 10. Examples

### 10.1 Allow path trace (conceptual)

```
trace_id=4bf92f3577b34da6a3ce929d0e0e4736
  mcp.gateway.request [200ms] method=tools/call server_id=github tenant_id=acme agent_id=acme/bot
    mcp.gateway.authz [2ms] decision=allow cel_eval_ms=1 policy_id=mcp-pack@0.1.0
      mcp.gateway.spicedb.check [8ms] permission=call resource=github|create_issue cache_hit=true zed_token_age_ms=0
    mcp.gateway.egress [180ms] server_id=github latency_ms=180 mesh.minted=true
      nats.request [175ms] destination=mcp.server.github.tools.call
    mcp.gateway.audit.publish [3ms] subject=mcp.audit.allow.request.tools bytes=842
```

### 10.2 Deny path (policy)

Root span ends early; egress child absent; audit publish still runs:

```
mcp.gateway.request [12ms] outcome=deny
  mcp.gateway.authz [10ms] decision=deny cel_eval_ms=9
    mcp.gateway.spicedb.check [9ms] permission=call resource=github|delete_repo cache_hit=false
mcp.gateway.audit.publish [2ms] subject=mcp.audit.deny.request.tools
```

Metric increments:

```
mcp_gateway_requests_total{method="tools/call",tenant="acme",outcome="deny"} 1
mcp_gateway_authz_decision_total{decision="deny",reason="spicedb_deny",tenant="acme"} 1
```

### 10.3 JSON-RPC error correlation

Client receives:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32100,
    "message": "policy denied",
    "data": {
      "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
      "rule_fired": "tools/call.deny",
      "reason": "spicedb_deny"
    }
  }
}
```

Operator searches traces by `trace_id`; SIEM searches audit by the same id when envelope field lands.

---

## 11. Open questions (deferred)

1. **OTLP/gRPC vs HTTP** — dual protocol support in `trogon-telemetry` or breaking switch?
2. **`tenant.id` resource** — auto-detect single-tenant from env `MCP_GATEWAY_SINGLE_TENANT=acme`?
3. **Audit header injection** — JetStream publish API header support across NATS server versions in prod.
4. **Baggage** — whether `mcp-correlation-id` should map to OTel Baggage (likely no — audit only).
5. **Tail sampling** — collector-side only vs gateway `always_record_deny` (§6.2).

---

## 12. Acceptance criteria (Block G paper)

- [ ] Operators can configure export via standard `OTEL_*` env vars (§5).
- [ ] Implementer can code spans/metrics without inventing new names outside §3–§4.
- [ ] Cardinality budget §8 is enforceable in code review (closed label sets).
- [ ] Audit/trace separation §7 is explicit — no PII duplication path.
- [ ] Cross-references resolve to [integration-touchpoints.md](integration-touchpoints.md), [reference-audit-envelope.md](reference-audit-envelope.md), redaction module, and [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block G.

---

## References

| Source | Topic |
|---|---|
| [W3C Trace Context](https://www.w3.org/TR/trace-context/) | `traceparent` / `tracestate` |
| [OpenTelemetry Semantic Conventions — Messaging](https://opentelemetry.io/docs/specs/semconv/messaging/) | NATS span kind and attributes |
| [integration-touchpoints.md](integration-touchpoints.md) | Integration ownership |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Audit schema and redaction |
| `rsworkspace/crates/trogon-telemetry` | Current OTel bootstrap |
| `rsworkspace/crates/trogon-nats/src/messaging.rs` | Trace inject |

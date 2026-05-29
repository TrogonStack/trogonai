# ADR 0032: Tracing Across the WASM Component Boundary

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block F item 3 (Tracing across the WASM boundary; span context as part of `request-ctx`); unblocks Phase 3 Wasmtime linker work ([ADR 0025](0025-wasmtime-component-pooling.md)), redaction span attributes ([ADR 0027](0027-redaction-span-attributes.md)), and acceptance tests in `tests/otel_span_shape.rs` / `tests/traceparent_propagation.rs` |
| **Related** | [ADR 0024](0024-failure-mode-matrix.md) (WASM trap row 3); [ADR 0025](0025-wasmtime-component-pooling.md); [ADR 0027](0027-redaction-span-attributes.md); [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md); [reference-host-abi.md](../identity/reference-host-abi.md); [otel-wiring.md](../identity/otel-wiring.md); `rsworkspace/crates/trogon-mcp-gateway/tests/otel_span_shape.rs`; `tests/traceparent_propagation.rs`; `tests/wasm_host_abi.rs` |

## Context

Phase 3 introduces Tier-3 WASM policy evaluation inside the gateway hot path. A single `tools/call` request already spans ingress authz (CEL), optional NATS-callout plugins, SpiceDB, egress mint, and audit publish. Without an explicit tracing contract at the Wasmtime boundary, WASM evaluation becomes a **trace hole**: operators see latency in `mcp.gateway.authz` but cannot attribute time to guest logic, host imports, or traps.

`MCP_GATEWAY_PLAN.md` Block F item 3 requires **tracing across the WASM boundary** with span context carried as part of **`request-ctx`**. The Block B sign-off WIT ([mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md)) supersedes the plan's illustrative `request-ctx` record with **`policy-input`** + **`attributes-json`**, which today carries `trace_id` (32-hex) but not the full W3C carrier. [otel-wiring.md](../identity/otel-wiring.md) defers WASM boundary spans to v2 (Block F) while pinning W3C propagation on NATS headers and parent-based sampling on the host.

Downstream constraints:

| Constraint | Source |
|------------|--------|
| WIT package `trogon:mcp-policy@0.1.0`, WASI **0.3**, Component Model | Block F item 1; [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md) |
| Host owns OTel spans; guest has capability imports only | [reference-host-abi.md](../identity/reference-host-abi.md); trust boundary |
| No arbitrary OTLP types in guest ABI — W3C `traceparent` + flat string KV only | User task; aligns with otel-wiring redaction posture |
| Tracing optional for bundles that ignore observability imports; always available from host | Block F operational requirement |
| WASM trap → `-32101` `policy_fault`, span ERROR | [ADR 0024](0024-failure-mode-matrix.md) row 3; `tests/wasm_host_abi.rs` `wasm_traps` module |

Acceptance scaffolds already exist:

- `tests/traceparent_propagation.rs` — ingress `traceparent` → audit `trace_id` / egress child continuity; sampling flag preservation (`sampling` module).
- `tests/otel_span_shape.rs` — root/authz child hierarchy, `policy.rule_fired` events, ERROR status on deny/fault, OTLP `trace_id` correlation with JSON-RPC `data.trace_id`.
- `tests/wasm_host_abi.rs` — trap mapping, `data.tier == "wasm"`, effect journal drop on trap.

This ADR records the **normative propagation and span-ownership model** so implementers do not fork ad hoc Wasmtime store hacks or duplicate sampling logic inside guest code.

---

## Decision

Adopt a **host-owned span tree** with **dual trace-context channels**: (1) a read-only W3C snapshot in `policy-input.attributes-json`, and (2) authoritative OpenTelemetry state in the Wasmtime **component-instance store** user data. Guests MAY attach flat string attributes via a new host import; guests MUST NOT create spans, parse OTLP, or influence sampling.

### 1. Trace context carriers

| Channel | Location | Contents | Writer | Reader |
|---------|----------|----------|--------|--------|
| **Ingress snapshot** | `policy-input.attributes-json` | `trace_id` (32 hex, existing), **`traceparent`** (full W3C string) **(proposed)**, **`tracestate`** (optional pass-through) **(proposed)** | Gateway when building `policy-input` from NATS headers / JSON-RPC `_meta.traceparent` per [otel-wiring.md §2](../identity/otel-wiring.md#2-trace-propagation) | Guest for audit/determinism only; not authoritative for OTel |
| **Active evaluation context** | Wasmtime store user data: `WasmEvalContext` **(proposed)** | Handle to host `tracing` span (`mcp.gateway.wasm.evaluate`), parent `SpanContext`, **`trace_sampled: bool`** (derived once at entry), bundle/instance ids, import-call budget | Host before `evaluate()` | Host import shims; never exposed to linear memory |

**Mapping from plan `request-ctx`:** Block F "span context as part of `request-ctx`" is realized as **`attributes-json.traceparent`** on the signed `policy-input` record. No separate WIT `request-ctx` type is added.

Example non-normative `attributes-json` fragment:

```json
{
  "tenant": "acme",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
  "tracestate": "vendor=value",
  "nats": { "subject": "mcp.gateway.request.github.tools.call" },
  "direction": "request"
}
```

### 2. Span hierarchy and ownership

Normative span tree for a gated request that reaches Tier-3 WASM (extends [otel-wiring.md §3.1](../identity/otel-wiring.md#31-span-hierarchy-explanation)):

```
mcp.gateway.request
└── mcp.gateway.authz
    ├── mcp.gateway.cel.eval          (when CEL tier runs — existing Block G)
    ├── mcp.gateway.plugin.call       (when NATS-callout tier runs)
    └── mcp.gateway.wasm.evaluate     (host: one per evaluate() invocation)
        ├── mcp.gateway.wasm.import.spicedb-check   (per import call)
        ├── mcp.gateway.wasm.import.cache-get
        ├── mcp.gateway.wasm.import.audit-emit
        └── mcp.gateway.wasm.import.log
```

| Responsibility | Owner |
|----------------|-------|
| Create `mcp.gateway.wasm.evaluate` child under active `mcp.gateway.authz` | **Host**, immediately before `policy-guest.evaluate` |
| Set required WASM span attributes (table below) | **Host**, at span start and on completion |
| Attach guest-authored attributes | **Guest** via `span-attribute-set` import **(proposed)**; host validates keys |
| Create per-import child spans | **Host**, inside each `host.*` import shim |
| End `mcp.gateway.wasm.evaluate` | **Host**, after `evaluate` returns or trap is caught |
| Propagate `traceparent` on NATS egress / audit | **Host**, unchanged from [otel-wiring.md §2.4](../identity/otel-wiring.md#24-subject-lanes-where-headers-must-flow) |

Guest exports (`init`, `version`, `evaluate`) do **not** receive a WIT span handle. Observability is entirely host-side.

### 3. Host ABI additions (`trogon:mcp-policy@0.1.0` minor bump)

Add to the `host` interface in [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md). **Additive minor** (`0.1.1` **(proposed)** or documented as `0.1.0` + forward-compatible optional import at link time). No new WIT primitives beyond `func` / `string` / `result`.

```wit
/// Set a string attribute on the host-owned `mcp.gateway.wasm.evaluate` span.
/// Keys MUST start with `wasm.`; max key 64 bytes; max value 256 bytes.
/// Forbidden keys: raw JWT, params-json slices, free-text deny reasons with PII.
span-attribute-set: func(key: string, value: string) -> result<_, host-error>;

/// Record a named event on the WASM evaluation span.
/// `attributes-json` MUST be a JSON object string; same size caps as audit-emit fields.
/// Event names MUST start with `wasm.`.
span-event: func(name: string, attributes-json: string) -> result<_, host-error>;
```

| Import | Capability manifest | When optional |
|--------|---------------------|---------------|
| `span-attribute-set` | `host.span-attribute-set` | Bundle may omit; host span still emitted |
| `span-event` | `host.span-event` | Bundle may omit; CEL `policy.rule_fired` parity uses host-side mapping from `audit-emit` category `policy` **(proposed)** |

Existing `log` import behaviour (forward to gateway tracing with bundle attribution) is unchanged; `log` does **not** replace span attributes.

**Explicitly excluded from WIT:** `span-start`, `span-end`, OTLP export, `traceparent` mutation, sampling APIs, baggage.

### 4. Sampling

| Rule | Detail |
|------|--------|
| Sampling decisions | **Host only**, via OpenTelemetry `ParentBased` sampler configured in `trogon-telemetry` ([otel-wiring.md §6](../identity/otel-wiring.md#6-sampling)) |
| Guest visibility | Guest receives **`trace_sampled`** only as an opaque boolean inside `WasmEvalContext` **(proposed)** for future metrics; it is **not** a WIT import and MUST NOT branch authorization logic |
| Upstream unsampled parent | Host still creates `mcp.gateway.wasm.evaluate` for correlation; export may drop per SDK rules — same as ingress (`tests/traceparent_propagation.rs` `sampling` module) |
| Guest MUST NOT | Call sampling APIs, override `traceparent` flags, or force sample via host imports |

### 5. Host-attached span attributes (automatic)

Applied by the host without guest cooperation:

| Attribute | When set | Example / source |
|-----------|----------|------------------|
| `policy.tier` | Span start | `wasm` |
| `policy_id` | Span start | `{bundle_id}@{bundle_version}` — aligns with `mcp.gateway.authz` |
| `wasm.wit_version` | Span start | `0.1.0` |
| `wasm.component` | Span start | Manifest component name |
| `wasm.pool_instance_id` | Span start | Pool slot id **(proposed)** — ties to [ADR 0025](0025-wasmtime-component-pooling.md) |
| `wasm.bundle_generation` | Span start | Hot-swap generation counter **(proposed)** |
| `tenant_id` | Span start | From eval context |
| `mcp.method` | Span start | From `policy-input.method` |
| `decision` | Span end | `allow` \| `deny` \| `fault` |
| `wasm.eval_ms` | Span end | Wall time of `evaluate()` only |
| `wasm.fuel_consumed` | Span end | Fuel meter delta **(proposed)** |
| `wasm.import_calls` | Span end | Count of host imports invoked |

**Guest-authored attributes** (via `span-attribute-set`) MUST use the `wasm.*` prefix. Host rejects keys outside allowlist or matching [otel-wiring.md §7.4](../identity/otel-wiring.md#74-what-may-appear-on-traces) forbidden data classes → `host-error.code = span_attribute_rejected` **(proposed)**.

**Redaction alignment:** Attribute values copied from JWT, `params-json`, or trap messages pass through `telemetry::redact_attribute` **(proposed)** per [ADR 0027](0027-redaction-span-attributes.md).

### 6. Error and trap path

When Wasmtime returns a trap or host maps fuel/memory/import-limit failure ([mcp-policy-wit-sketch.md § Error mapping](../identity/mcp-policy-wit-sketch.md#error-mapping)):

1. Host sets `mcp.gateway.wasm.evaluate` status to **ERROR**.
2. Host sets attributes:

| Attribute | Value |
|-----------|-------|
| `decision` | `fault` |
| `wasm.error.kind` | Closed enum: `trap`, `fuel_exhausted`, `memory_limit`, `import_limit`, `init_failed` |
| `wasm.error.code` | Wasmtime trap code / `host-error.code` when applicable |
| `wasm.error.message` | First line of trap message, **max 256 UTF-8 bytes**, no JSON-RPC params |
| `wasm.stack_excerpt` | Wasmtime stack trace excerpt, **max 512 bytes**, redacted **(proposed)** |

3. JSON-RPC response uses `-32101` `policy_fault` with `data.tier = "wasm"`, `data.trace_id` from **active root span** (same as [otel-wiring.md §2.6](../identity/otel-wiring.md#26-trace_id-in-json-rpc-errors)).
4. Effect journal dropped per [reference-host-abi.md §13](../identity/reference-host-abi.md#13-side-effect-ordering) — span still records fault (observability ≠ side effects).
5. Parent `mcp.gateway.authz` span receives `decision=fault` and `jsonrpc.error_code=-32101` when evaluation aborts.

Import-level errors (`spicedb_unavailable`, etc.) set the **import child span** to ERROR; the WASM evaluate span records `wasm.import_error` **(proposed)** unless the guest catches via `result` (SpiceDB deny is not ERROR — boolean `false`).

### 7. Pipeline ordering and trace continuity

Per `MCP_GATEWAY_PLAN.md` policy pipeline (CEL → NATS-callout → WASM):

| Phase | Span | Trace parent |
|-------|------|--------------|
| Ingress | `mcp.gateway.request` | Extracted W3C or new root |
| CEL | `mcp.gateway.cel.eval` | Child of `authz` |
| WASM | `mcp.gateway.wasm.evaluate` | Child of `authz` (sibling to CEL when both run) |
| Audit | `mcp.gateway.audit.publish` | Child of `request`; `trace_id` unchanged |

A single `tools/call` trace MUST contain **at most one** `mcp.gateway.wasm.evaluate` span per bundle hook invocation. Response-phase WASM (`evaluate-response`, future) gets **`mcp.gateway.wasm.evaluate-response`** **(proposed)** as a separate sibling under `authz`.

### 8. Configuration

| Knob | Default | Effect |
|------|---------|--------|
| `MCP_GATEWAY_WASM_TRACING` **(proposed)** | `on` | `off`: skip WASM span creation and import child spans; still populate `attributes-json.traceparent` and audit `trace_id` |
| `MCP_GATEWAY_WASM_SPAN_IMPORTS` **(proposed)** | `on` | `off`: collapse import calls into evaluate span events only (latency debugging trade-off) |

Bundled defaults; no tenant-level sampling override at WASM layer.

---

## Consequences

### Positive

- **End-to-end visibility.** Operators can expand `mcp.gateway.authz` to see WASM evaluate time, per-import latency, and trap class without custom guest code.
- **Single sampling authority.** Parent-based head sampling from ingress propagates through pooled instances; no guest bypass.
- **Contract stability.** Flat KV host imports survive WIT 0.3 / WASI 0.3 without OTLP in the sandbox.
- **Test alignment.** Scaffolds in `otel_span_shape.rs`, `traceparent_propagation.rs`, and `wasm_host_abi.rs` gain explicit expected span names and attributes.
- **Audit correlation.** `attributes-json.trace_id` / `traceparent` match JSON-RPC `data.trace_id` and audit envelope fields already pinned in Wire-Format Pins.

### Negative

- **Store user data per pooled instance.** `WasmEvalContext` must be reset on pool checkout; bugs could leak span parent across requests.
  - **Mitigation:** Reset context in pool acquire hook ([ADR 0025](0025-wasmtime-component-pooling.md)); add leak test in `tests/wasm_host_abi.rs` **(proposed)**.
- **Minor WIT bump for new imports.** Bundles using observability must declare new capabilities.
  - **Mitigation:** Imports optional at link time; bundles that omit them behave as today with host-only spans.
- **Cardinality from guest `span-attribute-set`.**
  - **Mitigation:** Prefix enforcement (`wasm.*`), key/value caps, reject dynamic tool names as attribute keys; follow [otel-wiring.md §8](../identity/otel-wiring.md#8-cost-model-and-cardinality-budget).
- **Stack excerpts may contain tenant paths in trap messages.**
  - **Mitigation:** Redaction helper + max length; omit stack on production when `MCP_GATEWAY_WASM_STACK_TRACES=off` **(proposed)**.

### Neutral

- CEL and NATS-callout tiers keep their own spans; WASM does not subsume them.
- `TraceStore` in `trogon-mcp-gateway/src/trace.rs` remains the Phase-1 decision debug aid; OTel spans are parallel, not a replacement.
- Component bundles that never call `span-attribute-set` still incur one host span per evaluation (acceptable overhead vs policy latency budget).

---

## Rejected alternatives

### Guest-owned spans via WIT `span` resource type

Add a WIT `resource span` with guest `span.end()` and OTLP-ish fields.

| | |
|-|-|
| **Pros** | Familiar to authors from other SDKs; guest controls span lifetime. |
| **Cons** | Requires new WIT resource lifecycle; guest could force sample or hold spans across pooled reuse; violates "host owns OTel" and redaction boundary. |
| **Verdict** | **Rejected.** Host-owned span with KV imports only. |

### Propagate trace context only through Wasmtime `Store` (no `attributes-json` field)

Keep `traceparent` out of `policy-input`; rely entirely on store user data.

| | |
|-|-|
| **Pros** | Smaller guest input; no duplicate trace id fields. |
| **Cons** | Breaks `MCP_GATEWAY_PLAN.md` Block F item 3 (`request-ctx` / policy-input visibility); audit replay and bundle dry-run (`agctl`) cannot see carrier without host-only tooling; conflicts with [mcp-policy-wit-sketch.md Appendix B](../identity/mcp-policy-wit-sketch.md) worked example. |
| **Verdict** | **Rejected.** Dual channel: snapshot in JSON + authoritative store. |

### Encode trace context as custom Wasmtime host `get-state` syscall outside WIT

Bypass Component Model imports for tracing.

| | |
|-|-|
| **Pros** | No WIT semver bump. |
| **Cons** | Invisible to manifest capability gating; breaks "host ABI is permanent" parity with CEL builtins; untestable via `wasm_host_abi.rs` pattern. |
| **Verdict** | **Rejected.** Tracing uses the same `host` import surface as `audit-emit`. |

### Map `policy.rule_fired` only to tracing `log` import

Use existing `log` for all guest observability.

| | |
|-|-|
| **Pros** | Zero WIT changes. |
| **Cons** | Logs are not structured span events; `tests/otel_span_shape.rs` `policy_event` module expects `policy.rule_fired` events on authz/wasm spans; SIEM cannot join rule firings to latency spans. |
| **Verdict** | **Rejected** as sole mechanism; add `span-event` **(proposed)** with host fallback from `audit-emit(category=policy)`. |

---

## Open questions

1. **WIT minor version tag** — Ship `span-attribute-set` / `span-event` as `0.1.0` optional imports at link time, or require manifest `wit.version: 0.1.1` **(proposed)**? Linker behaviour TBD in bundle loader ADR scope.

2. **`policy.rule_fired` event placement** — Emit on `mcp.gateway.wasm.evaluate` vs parent `mcp.gateway.authz` when WASM is the only tier firing rules. Tests in `otel_span_shape.rs` allow either; pick one in Block G implementation PR.

3. **Async host imports (WASI 0.3)** — When `spicedb-check` yields across async component model, does the import child span cover yield time or only host work? Affects P99 attribution.

4. **Response-phase WASM** — Separate span name `mcp.gateway.wasm.evaluate-response` vs reuse `evaluate` with `wasm.phase=response` attribute **(proposed)** when `evaluate-response` lands in WIT 0.2.0.

5. **Cross-bundle traces** — Multiple WASM components in one request (policy + redaction) need distinct `wasm.component` attributes; ordering relative to [ADR 0027](0027-redaction-span-attributes.md) redaction spans TBD.

6. **In-memory OTLP test harness** — Confirm `trogon-telemetry` test exporter is available in `otel_span_shape.rs` exporter module or add gateway-local recorder **(proposed)**.

---

## Rollback plan

| Knob | Rollback action | Residual behaviour |
|------|-----------------|-------------------|
| `MCP_GATEWAY_WASM_TRACING=off` **(proposed)** | Disable WASM span creation at runtime | Ingress `traceparent`, audit `trace_id`, JSON-RPC `data.trace_id` unchanged; WASM latency invisible in traces |
| `MCP_GATEWAY_WASM_SPAN_IMPORTS=off` **(proposed)** | Keep evaluate span only | Coarser import attribution |
| Bundle manifest | Remove `host.span-attribute-set` / `host.span-event` from capabilities | Guest observability calls fail closed at link or return `undeclared_host_capability` |
| WIT linker pin | Stay on `0.1.0` linker without optional observability imports | Host-only automatic attributes on evaluate span |

Rollback does **not** disable global OTel export or ingress propagation ([otel-wiring.md](../identity/otel-wiring.md)). Audit stream remains legal record independent of trace sampling.

---

## Implementation notes

### File-level surfaces (when code lands)

| Path | Responsibility |
|------|----------------|
| `rsworkspace/crates/trogon-mcp-gateway/src/wasm/` **(proposed)** | Wasmtime embedder, pool checkout, `WasmEvalContext` in store |
| `src/wasm/host/mod.rs` **(proposed)** | Import shims: span child per import, trap mapping |
| `src/wasm/policy_input.rs` **(proposed)** | Build `policy-input`; inject `traceparent` into `attributes-json` |
| `src/telemetry/spans.rs` **(proposed)** | Span builders: `mcp.gateway.wasm.evaluate`, import children |
| `src/telemetry/redact_attribute.rs` **(proposed)** | Attribute sanitizer per ADR 0027 |
| `wit/trogon-mcp-policy/` **(proposed)** | WIT source for minor bump |
| `docs/identity/mcp-policy-wit-sketch.md` | Update import table when WIT lands (follow-on doc PR, not this ADR commit) |

### Integration points

| Caller | Action |
|--------|--------|
| Gateway ingress handler | Extract W3C → root/authz spans ([otel-wiring.md §2.5](../identity/otel-wiring.md#25-header-extraction-implementation-sketch)) |
| Policy pipeline orchestrator | Invoke WASM tier; pass active `tracing::Span` into pool worker |
| `audit-emit` host import | Merge audit + optional `span-event` mirror for `category=policy` **(proposed)** |
| JSON-RPC error builder | `data.trace_id` from root span trace id |

### Acceptance tests to extend

| File | New / updated scenarios **(proposed)** |
|------|----------------------------------------|
| `tests/traceparent_propagation.rs` | `wasm_evaluate_span_trace_id_matches_ingress`; sampling flags unchanged across WASM path |
| `tests/otel_span_shape.rs` | Child `mcp.gateway.wasm.evaluate` under authz; `policy.rule_fired` from WASM; ERROR on trap with `wasm.error.kind` |
| `tests/wasm_host_abi.rs` | Module `wasm_tracing`: `span-attribute-set` allowlist; trap sets span ERROR + stack excerpt cap; `MCP_GATEWAY_WASM_TRACING=off` |

### Dependency on sibling ADRs

| ADR | Need |
|-----|------|
| [0025](0025-wasmtime-component-pooling.md) | Pool acquire/release resets `WasmEvalContext` |
| [0027](0027-redaction-span-attributes.md) | Shared redaction denylist for span attribute values |
| [0024](0024-failure-mode-matrix.md) | Trap → `-32101` wire + audit outcome |

---

*Contract sources: `MCP_GATEWAY_PLAN.md` Block F items 1–3; `docs/identity/mcp-policy-wit-sketch.md`; `docs/identity/reference-host-abi.md`; `docs/identity/otel-wiring.md`; `docs/identity/reference-nats-headers.md`; `rsworkspace/crates/trogon-mcp-gateway/tests/otel_span_shape.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/traceparent_propagation.rs`; `rsworkspace/crates/trogon-mcp-gateway/tests/wasm_host_abi.rs`; `rsworkspace/crates/trogon-nats/src/messaging.rs` (`inject_trace_context`); `rsworkspace/crates/trogon-mcp-gateway/src/trace.rs`*

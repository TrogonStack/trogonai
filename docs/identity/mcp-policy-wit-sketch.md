# MCP policy WIT host ABI sketch (`trogon:mcp-policy@0.1.0`)

**Status:** Block B paper artifact (2026-05-28). Irreversible host ABI sketch for Phase 3 WASM policy bundles. No Wasmtime code ships from this document alone.

**Document type:** Diátaxis **reference** (stable identifiers, WIT surface, error mapping) with short **explanation** prose where design intent matters.

**Related:** [Identity overview](overview.md), [Adaptive access](adaptive-access.md), [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Block B (Host ABI surface) and Block F (Phase 3 WASM), [Failure-mode matrix](failure-mode-matrix.md), [Act chain](act-chain.md).

**Implementation target:** `rsworkspace/crates/trogon-mcp-gateway` Phase 3; CEL builtins in Phase 2 are shaped to desugar into this surface ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block E).

---

## Goal

A **WASM policy bundle** is a signed WebAssembly Component (Component Model) that the MCP gateway loads from a bundle manifest and invokes once per policy evaluation on the request path (and optionally on the response path in later revisions). The bundle expresses tenant-authored authorization, risk scoring, catalog shaping hints, and audit enrichment in a sandbox with **capability-based host imports** only.

We want bundles to express: SpiceDB-backed allow/deny decisions; time- and cache-sensitive rules (for example business-hours gates, session-scoped memoization of expensive checks); structured audit emission that merges into the gateway audit envelope; and adaptive-access outcomes (`deny`, `challenge` for step-up or human approval) aligned with [adaptive-access.md](adaptive-access.md). Bundles receive a normalized **policy input** record derived from validated JWT claims, NATS headers, and JSON-RPC fields — including `act-chain-json` and `attributes-json` for mesh identity ([overview.md](overview.md), [act-chain.md](act-chain.md)).

We **explicitly do not** expose syscalls, raw networking, filesystem access, environment variables, or direct NATS publish/subscribe to guest code. Guests cannot open sockets, read `/etc`, spawn processes, or call SpiceDB gRPC directly. Every external effect flows through the host `import host` interface listed below. The gateway host links only the imports declared in the bundle manifest; undeclared host capabilities are unavailable at link time.

This sketch pins the **guest export surface** (`evaluate`, `init`, `version`) and the **host import surface** so Phase 2 CEL builtins and Phase 3 Wasmtime integration share one permanent ABI. Downstream tasks (component pooling, bundle hot-swap, tracing across the WASM boundary) depend on this contract; they must not redefine it ad hoc.

---

## Versioning and naming

| Field | Value |
|---|---|
| WIT package name | `trogon:mcp-policy` |
| Initial package version | `0.1.0` |
| World name | `policy-bundle` |
| Pin target (Phase 3) | WASI 0.3 + Component Model async host calls **(per [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F)** |

### Semver guarantees

- **Minor version bumps** (`0.1.x`, `0.2.0` within major `0`) **add** interfaces, functions, record fields (with defaults at the WIT/ABI layer), or enum variants. They **do not remove**, **rename**, or **change the signature** of any symbol present in a prior minor release within the same major.
- **Major version bumps** (`1.0.0`, …) **may break** compatibility: remove imports, alter function signatures, or rename exported functions. Gateways may load multiple major versions side by side during migration **(proposed)** via bundle manifest `wit_major`.
- **`version()` export** returns the **bundle author's** semver string (manifest `version` field) for audit only. It is **not** the WIT package version and **not** authoritative for compatibility; the gateway selects the host linker by manifest-declared `trogon:mcp-policy` package version.

### Naming conventions

- WIT packages use kebab-case interface names (`host`, `policy-types`).
- Host functions use kebab-case (`spicedb-check`, `cache-get`).
- JSON carried as `string` fields (`params-json`, `fields` in `audit-emit`) must be UTF-8 JSON text; the host validates parseability only when merging audit or forwarding to SpiceDB tuple derivation — not on every read.

---

## World definition

The block below is the **normative sketch** for Block B sign-off. Field order is stable for code generation; additive fields in future minors append at record ends with WIT `@deprecated` annotations where needed **(future process)**.

```wit
package trogon:mcp-policy@0.1.0;

/// Host-provided logging levels for `log`.
enum log-level {
  trace,
  debug,
  info,
  warn,
  error,
}

/// Structured host failure returned from fallible imports and `init`.
record host-error {
  /// Stable machine code, e.g. `spicedb_unavailable`, `cache_key_too_long`.
  code: string,
  /// Human-readable detail; not a stable contract.
  message: string,
}

/// Normalized evaluation input — one JSON-RPC request on the gateway path.
record policy-input {
  /// Gateway-correlated id (JSON-RPC `id` when present, else synthetic).
  request-id: string,
  /// Current hop logical actor (`jwt.sub` after validation).
  actor-id: string,
  /// SpiceDB / policy subject string for this evaluation (often `user:{sub}` or `agent:{tenant}/{name}`).
  subject-id: string,
  /// MCP method with slash, e.g. `tools/call`, `resources/read`.
  method: string,
  /// JSON object string of JSON-RPC `params` (may be large — see Open questions).
  params-json: string,
  /// JSON array string of act-chain entries per act-chain.md (may be `[]`).
  act-chain-json: string,
  /// JSON object string of auxiliary attributes: tenant, session_id, agent_id, wkl, purpose, nats.subject, trace_id, etc.
  attributes-json: string,
}

/// Guest decision for a single evaluation.
variant policy-decision {
  /// Proceed; gateway continues pipeline (SpiceDB may still run per bundle manifest tier ordering).
  allow,
  /// Hard deny; maps to JSON-RPC `policy_deny`.
  deny(string),
  /// Adaptive challenge: step-up or HITL; maps to JSON-RPC `approval_required` when wired ([adaptive-access.md](adaptive-access.md)).
  challenge(string),
}

/// Capability surface the gateway implements for guests.
interface host {
  /// SpiceDB CheckPermission-style boolean check. Host maps strings to Authzed API; no raw gRPC in guest.
  spicedb-check: func(
    resource: string,
    permission: string,
    subject: string,
  ) -> result<bool, host-error>;

  /// Opaque byte cache read. Host may scope keys by tenant + bundle version prefix.
  cache-get: func(key: string) -> option<list<u8>>;

  /// Opaque byte cache write with TTL seconds. Host may cap value size (see Resource limits).
  cache-set: func(key: string, value: list<u8>, ttl-secs: u32) -> result<_, host-error>;

  /// Merge structured audit fields into the in-flight audit envelope `extra` / extension bucket.
  /// `fields` must be a JSON object string. Host adds `ts`, `request_id`, and trace correlation.
  audit-emit: func(category: string, fields: string) -> result<_, host-error>;

  /// Wall-clock milliseconds since Unix epoch from host clock.
  time-now-ms: func() -> u64;

  /// Structured log line attributed to bundle id + instance id in host telemetry.
  log: func(level: log-level, message: string) -> result<_, host-error>;
}

/// Guest exports implemented by the policy component.
interface policy-guest {
  /// Called once after instantiation before any `evaluate`.
  init: func() -> result<_, host-error>;

  /// Author semver for audit (`bundle_version` field) — not used for linker selection.
  version: func() -> string;

  /// Primary authorization / risk decision entrypoint.
  evaluate: func(input: policy-input) -> policy-decision;
}

world policy-bundle {
  import host;
  export policy-guest;
}
```

### Import reference table

| Import | Inputs | Output | Host responsibility |
|---|---|---|---|
| `spicedb-check` | `resource`, `permission`, `subject` strings | `result<bool, host-error>` | Map to gateway SpiceDB client; apply session ZedToken from attributes/session KV **(Phase 2+)**; fail with `host-error.code = spicedb_unavailable` when PDP unreachable ([failure-mode-matrix.md](failure-mode-matrix.md) row 1). |
| `cache-get` | `key` | `option<list<u8>>` | Tenant-scoped key prefix; no cross-tenant reads. |
| `cache-set` | `key`, `value`, `ttl-secs` | `result<_, host-error>` | Enforce max value bytes; TTL clamped to host maximum. |
| `audit-emit` | `category`, `fields` (JSON object string) | `result<_, host-error>` | Merge into audit envelope; invalid JSON → `host-error.code = audit_invalid_json`. |
| `time-now-ms` | — | `u64` | Real-time clock; see Determinism rules. |
| `log` | `level`, `message` | `result<_, host-error>` | Forward to gateway tracing/logging with WASM span child **(Phase 3)**. |

### Export reference table

| Export | When invoked | Success | Failure |
|---|---|---|---|
| `init` | Once per pooled component instance after link, before traffic | `ok` | Instance discarded; gateway may fail bundle activation **(proposed)** |
| `version` | Any time (cheap); logged at load | semver string | — |
| `evaluate` | Each policy hook invocation | `policy-decision` | Trap → see Error mapping |

### Mapping to CEL builtins (Phase 2 alignment)

Block B lists host ABI names that Phase 2 CEL should mirror ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block B, Block E):

| CEL / plan name | WIT import | Notes |
|---|---|---|
| `spicedb.check(...)` | `spicedb-check` | Same triple ordering: subject, permission, resource in CEL; WIT argument order matches gateway SpiceDB helper today (`resource`, `permission`, `subject`) — CEL wrapper may reorder; WASM uses WIT order. |
| `cache.get` / `cache.set` | `cache-get` / `cache-set` | CEL returns JSON values; WASM uses opaque bytes — host serializes at boundary **(proposed)**. |
| `audit.emit(...)` | `audit-emit` | CEL merges map; WASM passes JSON string + category. |
| `time.now` | `time-now-ms` | CEL timestamp type; WASM milliseconds. |
| `rate.acquire` | *(not in v0.1.0 WIT)* | Stays host-native in gateway for Phase 2; **proposed** `rate-acquire` import in `0.2.0` minor. |
| `jsonpath.*` | *(not in v0.1.0 WIT)* | Request/response redaction uses separate component in plan; out of scope for `policy-bundle` v0.1.0. |
| `kv.fetch`, `nats.request` | **Excluded** | Listed in plan Block B as future platform hooks; intentionally absent from v0.1.0 to preserve trust boundary. |

### `policy-input` field derivation

| Field | Source (gateway ingress) |
|---|---|
| `request-id` | JSON-RPC `id` stringified, or gateway synthetic id for notifications |
| `actor-id` | Validated JWT `sub` ([overview.md](overview.md)) |
| `subject-id` | SpiceDB subject string chosen by bundle manifest / tuple derivation table ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § SpiceDB Integration Model) |
| `method` | Parsed MCP method from NATS subject + JSON-RPC |
| `params-json` | Serialized `params` object; `{}` when absent |
| `act-chain-json` | JWT `act_chain` or reconstructed from `mcp-act-chain` header after gateway append ([act-chain.md](act-chain.md)) |
| `attributes-json` | Stable JSON object: at minimum `tenant`, `session_id`, `agent_id`, `wkl`, `purpose`, `trace_id`, `nats.subject`, `method_root`, `direction` |

### `policy-decision` gateway actions

| Variant | Gateway action | JSON-RPC | Audit outcome |
|---|---|---|---|
| `allow` | Continue pipeline (later tiers may still deny) | — (success path) | `allow` when final |
| `deny(reason)` | Short-circuit; no backend publish on gated methods | `-32100` `POLICY_DENY` | `deny` |
| `challenge(reason)` | Park or emit approval envelope per [adaptive-access.md](adaptive-access.md) | `-32107` `APPROVAL_REQUIRED` | `deny` or parked state **(proposed:** `challenge`) |

---

## Determinism rules

Policy replay and SIEM correlation require explicit handling of **non-deterministic host inputs**. A bundle evaluation is **deterministic** iff, given identical `policy-input` and identical return values for all host imports invoked during that evaluation, the guest returns the same `policy-decision`.

### Non-deterministic imports

| Import | Why non-deterministic | Replay requirement |
|---|---|---|
| `time-now-ms` | Wall clock advances between calls and replays | If `evaluate` calls `time-now-ms` and the decision depends on it (directly or via guest branch), guest **must** call `audit-emit` with category `determinism:time` and JSON fields including `{ "time_now_ms": <value> }` before returning the decision. |
| `cache-get` | Another evaluation or gateway instance may `cache-set` between runs | If decision depends on cache hit/miss or cached bytes, guest **must** `audit-emit` category `determinism:cache` with `{ "key": "...", "hit": true\|false, "value_b64": "..." }` **(proposed field names)**. |
| `spicedb-check` | SpiceDB graph may change between replay time and original time | Not treated as non-deterministic for **authorization truth** — replay uses recorded SpiceDB checks from audit envelope (`spicedb` block in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §7). Guest should still emit audit when check result is surprising **(optional)**. |
| `log` | Side effect only | Does not affect decision determinism for replay classification. |

### Host obligations

1. **Audit merge order:** `audit-emit` calls append to an in-memory list attached to the evaluation context; the gateway serializes them into the audit envelope `rules_fired` / extension fields before publish.
2. **Replay mode **(proposed):** Offline replay tooling (`agctl` bundle dry-run) supplies recorded import stubs; missing `determinism:*` audit entries when imports were used is a validation warning.
3. **Clock injection **(proposed):** Test harness may fix `time-now-ms` via host config; production uses real time.
4. **Cache isolation:** Keys are prefixed `{tenant}/{bundle_version}/{key}` to prevent cross-tenant leakage; cache content does not affect deny/allow unless guest reads it — then determinism audit applies.

### Deterministic pure paths

Guests that implement only `spicedb-check` + `audit-emit` with fixed logic on `policy-input` fields (no time/cache) produce replayable decisions without extra determinism audit entries.

---

## Resource limits

Concrete limits apply per **`evaluate` invocation** (and per `init` where noted). Values are initial proposals for Wasmtime 21+ on amd64/linux; tune with Block G latency baseline **(proposed)**.

| Limit | Value | Justification |
|---|---|---|
| **Fuel per `evaluate` call** | `5_000_000` instructions | ~50 ms CPU on warm pooled instance at observed Wasmtime fuel rates in dev **(proposed benchmark)**; enough for chained SpiceDB checks + JSON inspection; prevents infinite loops. |
| **Fuel per `init` call** | `500_000` | One-time setup; smaller ceiling. |
| **Linear memory pages** | `256` pages (16 MiB) | Sufficient for large `params-json` copies; bounded to prevent memory exhaustion across pool size × concurrent requests. |
| **Max stack depth** | `512` KB | Wasmtime default stack guard tuned down from unlimited guest recursion; traps → `policy_fault`. |
| **`params-json` / `attributes-json` max bytes** | `1_048_576` (1 MiB) each | Host rejects oversize input before guest entry with trap-equivalent error **(proposed)** `-32101` tier `wasm_input_oversize`. |
| **`cache-set` value max bytes** | `65_536` (64 KiB) | Prevents cache poisoning with huge blobs; ZedToken-sized payloads fit. |
| **`cache-set` TTL max** | `86_400` seconds (24 h) | Clamped by host even if guest requests longer. |
| **`audit-emit` fields max bytes** | `16_384` | Keeps JetStream messages bounded. |
| **Host import call count per `evaluate`** | `64` | Prevents host DoS via import spam; exceeding → trap. |
| **Concurrent pooled instances per bundle version** | `32` per gateway process **(proposed)** | Matches inflight cap philosophy ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §9). |

### Fuel exhaustion behavior

When fuel is exhausted mid-`evaluate`, Wasmtime traps. The gateway maps trap to `-32101` `policy_fault` **(proposed)** with audit `decision_reason = wasm_fuel_exhausted` and fail-closed on gated methods ([failure-mode-matrix.md](failure-mode-matrix.md) row 3).

### Memory growth

Guests may not grow memory beyond the configured maximum pages; host denies `memory.grow` beyond cap **(Component Model default)**.

---

## Error mapping

Guest traps and host import failures become client-visible JSON-RPC errors and audit fields. Constants cite `rsworkspace/crates/trogon-mcp-gateway/src/rpc_codes.rs` where present.

### Trap and host error → JSON-RPC → audit

| Guest / host condition | Gateway JSON-RPC code | Symbol (`rpc_codes.rs`) | Audit `decision` | Audit `decision_reason` / notes |
|---|---|---|---|---|
| Guest returns `deny(reason)` | `-32100` | `POLICY_DENY` | `deny` | `reason` string from guest |
| Guest returns `challenge(reason)` | `-32107` | `APPROVAL_REQUIRED` | `deny` **(today)** / `challenge` **(proposed)** | Maps to [adaptive-access.md](adaptive-access.md) envelope with `approval_subject` |
| Guest returns `allow` | — | — | `allow` (if no later deny) | — |
| Wasm trap (panic, unreachable, div-by-zero) | `-32101` | **proposed** `POLICY_FAULT` | `error` | `wasm_trap` + trap message |
| Fuel exhausted | `-32101` | **proposed** `POLICY_FAULT` | `error` | `wasm_fuel_exhausted` |
| Import call limit exceeded | `-32101` | **proposed** `POLICY_FAULT` | `error` | `wasm_import_limit` |
| Linear memory cap / OOM trap | `-32101` | **proposed** `POLICY_FAULT` | `error` | `wasm_memory_limit` |
| `init` returns `err(host-error)` | Bundle not activated **(proposed)**; in-flight uses prior bundle | N/A at request **(proposed)** `-32108` if no active bundle | `error` on control subject **(proposed)** | `host_error.code` from guest |
| `init` trap | Same as init err | **proposed** `NO_POLICY` / unhealthy instance | `error` | `wasm_init_trap` |
| `spicedb-check` → `err` (`spicedb_unavailable`) | `-32107` | `AUTHZ_UNREACHABLE` | `error` | PDP unreachable ([failure-mode-matrix.md](failure-mode-matrix.md) row 1) |
| `spicedb-check` → `ok(false)` | `-32100` | `POLICY_DENY` | `deny` | `spicedb_check_denied` **(proposed)** |
| `audit-emit` → `err` | `-32101` | **proposed** `POLICY_FAULT` | `error` | `audit_emit_failed` |
| `cache-set` → `err` | `-32101` | **proposed** `POLICY_FAULT` | `error` | `cache_set_failed` |
| Oversize `policy-input` JSON fields | `-32101` | **proposed** `POLICY_FAULT` | `error` | `wasm_input_oversize` |
| Bundle unsigned / bad signature at load | N/A (request path) | **proposed** `NO_POLICY` (`-32108`) if no prior bundle | — | See Trust boundary |
| No bundle loaded (day zero) | `-32108` | **proposed** `NO_POLICY` | `deny` | `no_policy` ([failure-mode-matrix.md](failure-mode-matrix.md) row 5) |

### `host-error.code` registry (initial)

| Code | Meaning | Typical JSON-RPC |
|---|---|---|
| `spicedb_unavailable` | gRPC/transport failure to PDP | `-32107` |
| `spicedb_invalid_argument` | Malformed resource/permission/subject | `-32101` |
| `cache_key_too_long` | Key byte length > 256 | `-32101` |
| `cache_value_too_large` | Value > 64 KiB | `-32101` |
| `audit_invalid_json` | `fields` not a JSON object | `-32101` |
| `audit_payload_too_large` | `fields` > 16 KiB | `-32101` |
| `import_limit_exceeded` | > 64 imports in one evaluate | `-32101` |

### JSON-RPC `error.data` shape (WASM paths)

Aligned with [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §6:

```json
{
  "trace_id": "…",
  "tier": "wasm",
  "error": "wasm_trap: unreachable",
  "bundle_id": "acme/mcp-pack",
  "bundle_version": "1.2.3",
  "wit_version": "0.1.0"
}
```

For `POLICY_DENY`:

```json
{
  "trace_id": "…",
  "rule_fired": "wasm:evaluate",
  "reason": "…"
}
```

For `APPROVAL_REQUIRED` (challenge path), see [adaptive-access.md](adaptive-access.md) § Client-visible envelope.

### Relationship to existing constants

Verified in `rpc_codes.rs` today:

```rust
pub const POLICY_DENY: i32 = -32_100;
pub const BACKEND_TIMEOUT: i32 = -32_102;
pub const BACKEND_UNREACHABLE: i32 = -32_103;
pub const RATE_LIMITED: i32 = -32_105;
pub const AUTH_EXPIRED: i32 = -32_106;
pub const AUTHZ_UNREACHABLE: i32 = -32_107;
pub const APPROVAL_REQUIRED: i32 = -32_107;
pub const AUDIENCE_MISMATCH: i32 = -32_109;
pub const INVALID_TOKEN: i32 = -32_110;
pub const ACT_CHAIN_DEPTH_EXCEEDED: i32 = -32_113;
pub const ACT_CHAIN_LOOP_DETECTED: i32 = -32_114;
pub const ACT_CHAIN_UNRESOLVED: i32 = -32_115;
pub const AGENT_IDENTITY_REQUIRED: i32 = -32_117;
pub const AUTH_REQUIRED: i32 = -32_118;
```

**Proposed additions** (plan §6, not yet in `rpc_codes.rs`): `-32101` `POLICY_FAULT`, `-32104` `SCHEMA_UNKNOWN`, `-32108` `NO_POLICY`.

---

## Trust boundary

WASM policy bundles are **untrusted tenant code**. They run sandboxed with only declared imports, but they can exfiltrate any data passed in `policy-input` through `audit-emit` and `log`. **Supply-chain integrity** therefore matters as much as sandboxing.

### Signing requirement

Production bundles **MUST** be signed before activation. The plan specifies **NKey signature over manifest digest** ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Bundles, Block F). Cosign / Sigstore-style signatures are acceptable at the **OCI packaging layer** **(proposed)** as long as the gateway verifies an allowed signer set before load.

**Crypto algorithm:** TBD; see future signing-policy doc **(explicitly out of scope here)**.

### Verification surface

| Stage | Where verification happens | Behavior on failure |
|---|---|---|
| **Build-time / CI** | Bundle pipeline signs manifest; rejects unsigned artifacts **(proposed)** | Artifact never published to OCI / Object Store |
| **Load-time (gateway)** | Bundle loader verifies NKey (or configured sig) over manifest digest before Wasmtime link | Previous signed bundle remains active; instance `/ready=false` **(proposed)**; control event `mcp.control.bundle.load_failed` **(proposed)** ([failure-mode-matrix.md](failure-mode-matrix.md) row 4) |
| **Hot-swap** | Atomic pointer flip only after new bundle verifies | In-flight evaluations finish on old version |
| **Runtime** | No guest callable "verify-signature" import in v0.1.0 | Integrity is entirely host-side |

There is **no** WIT import for cryptography in v0.1.0. Guests cannot verify their own signature.

### Signer trust

Trusted signer NKeys (or cosign key refs) live in NATS KV `mcp-gateway-config/trusted_signers` **(proposed key)** per [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Bundles. Unknown signer → load rejected.

### Capability manifest

Bundle manifest declares required host imports (`host.spicedb-check`, `host.cache-get`, …). Gateway refuses link if manifest requests undeclared capabilities or omits required imports for the bundle tier.

---

## Lifecycle (explanation)

This section describes how the WIT world fits the gateway pipeline — reference ordering for implementers.

### Instance pooling

1. Bundle loader verifies signature and downloads WASM artifact from JetStream Object Store **(Phase 3)**.
2. Wasmtime **component pool** pre-instantiates `N` linked components per `{bundle_id, wit_version}`.
3. Each instance: `init()` once; on failure, discard instance.
4. Per request: borrow instance from pool → `evaluate(input)` → return instance.

### Pipeline position

Per [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Policy Engine, evaluation order is **CEL → NATS-callout plugin → WASM** (increasing power). WASM `evaluate` runs in **PostRouting** phase for methods selected by bundle manifest. A WASM `allow` does not skip mandatory SpiceDB when manifest declares SpiceDB as authoritative **(bundle config)** — v0.1.0 sketch allows either "WASM is sole PDP" or "WASM is advisory" via manifest flag **(proposed:** `wasm.mode: authoritative | advisory`).

### Tracing **(Phase 3)**

Span context from `attributes-json.trace_id` / W3C headers is attached as a host span around `evaluate`; each import creates a child span **(Block F requirement)**.

---

## Open questions

1. **Streaming large `params-json`:** v0.1.0 passes full params as a single string (up to 1 MiB host cap). Tools with multi-megabyte arguments may need a **`policy-input-stream` import** or host-side JSONPath pre-slice **(proposed)**. Deferred until schema-cache redaction component defines overlap.

2. **Guest-side caching across evaluations:** `cache-get` / `cache-set` enable memoization (for example expensive derived keys). Should cache persist **across** `evaluate` calls on the same pooled instance (instance-local) vs **cluster-wide** (JetStream KV)? Instance-local is faster but inconsistent under HA; cluster-wide matches ZedToken cache semantics. **Default proposal:** instance-local for `ttl-secs < 60`, JetStream-backed for longer TTL **(proposed)** — needs Block C rate-limit / session KV decision.

3. **`spicedb-check` async vs sync:** SpiceDB gRPC is async on the gateway tokio runtime. WIT guest call is synchronous from the component's view. Host options: (a) block worker thread inside `spicedb-check` **(simple, risks pool starvation)**; (b) async component model with yield **(WASI 0.3 async)**; (c) bulk prefetch before `evaluate` **(not visible to guest)**. **Recommendation for Phase 3 MVP:** (a) with short PDP timeout matching Phase 1; revisit (b) when P99 > 40 ms **(proposed)**.

4. **Response-phase evaluation:** Plan shows `shape-response` in an earlier WIT draft ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Tier 3). v0.1.0 exports only request-phase `evaluate`. Add `evaluate-response` in `0.2.0` or separate `redaction-bundle` world **(open)**.

5. **`challenge` vs risk library:** [adaptive-access.md](adaptive-access.md) implements risk in native Rust today (`policy::run_with_risk`). Should WASM `challenge` delegate to the same approval subjects, or can guests emit custom approval subjects? **Proposal:** gateway ignores guest-suggested subjects; always uses `mcp.approvals.{request_id}`.

6. **WIT boundary one-world vs multi-protocol:** [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Open Questions §1 — generic `nats-policy.wit` vs MCP-aware world. This sketch commits to **MCP-aware** `policy-input` for Phase 3; extraction to `trogon-policy-core` in Block F is a refactor, not a second ABI.

7. **Fail-open on `*/list`:** Plan § SpiceDB Integration allows fail-open with log for list shaping. WASM v0.1.0 has no `log`-only decision variant — guest must return `allow` and rely on separate CEL tier, or we add `allow-with-audit` variant **(proposed)** in minor bump.

8. **Bundle rollback automation:** Signature verification failure is clear; **runtime** regression detection (error rate spike after swap) is not in v0.1.0 WIT — operational **(proposed)** via `mcp.control.bundle.rollback` and metrics.

---

## Cross-reference index

| Topic | Document |
|---|---|
| Mesh identity, JWT, STS | [overview.md](overview.md) |
| Approvals, `-32107`, throttling | [adaptive-access.md](adaptive-access.md) |
| Host ABI todo, Phase 3 checklist | [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block B, Block F |
| WASM panic, bundle signature failures | [failure-mode-matrix.md](failure-mode-matrix.md) rows 3–5 |
| `act_chain` in `policy-input` | [act-chain.md](act-chain.md) |
| Audit envelope shape | [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §7 |
| JSON-RPC codes | [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) §6, `rpc_codes.rs` |

---

## Revision history

| Date | Change |
|---|---|
| 2026-05-28 | Initial Block B sketch: `trogon:mcp-policy@0.1.0`, world `policy-bundle`, determinism + resource limits + error mapping |

---

## Appendix A — Example guest pseudocode (non-normative)

Illustrates determinism audit for time-based rule:

```text
func evaluate(input: policy-input) -> policy-decision {
  let now = time-now-ms();
  if hour_utc(now) < 6 || hour_utc(now) > 22 {
    audit-emit("determinism:time", json({ "time_now_ms": now }));
    audit-emit("policy", json({ "rule": "business_hours", "decision": "deny" }));
    return deny("outside business hours");
  }
  if !spicedb-check("tool:foo", "invoke", input.subject-id) {
    return deny("spicedb denied");
  }
  return allow;
}
```

---

## Appendix B — Example `attributes-json` (non-normative)

```json
{
  "tenant": "acme",
  "session_id": "sess_01H…",
  "agent_id": "acme/oncall-responder",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder",
  "purpose": "incident-response",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "nats": {
    "subject": "mcp.gateway.request.github.tools.call"
  },
  "direction": "request",
  "method_root": "tools"
}
```

---

## Appendix C — Manifest snippet (proposed)

```yaml
# bundle manifest fragment — not implemented
id: acme/custom-policy
version: 2.0.0
wit:
  package: trogon:mcp-policy
  version: 0.1.0
  world: policy-bundle
capabilities:
  imports:
    - host.spicedb-check
    - host.audit-emit
    - host.time-now-ms
  exports:
    - policy-guest.evaluate
    - policy-guest.init
    - policy-guest.version
signature:
  scheme: nkey
  pubkey: "…"
  digest: "sha256:…"
  sig: "…"
```

---

## Appendix D — Phase checklist mapping (Block F)

| Block F item | Satisfied by this doc |
|---|---|
| WIT interface (`trogon:mcp-policy@0.1.0`) finalized | World `policy-bundle` + imports/exports |
| Pin to WASI 0.3 | Stated in Versioning |
| Tracing across WASM boundary | Open question / lifecycle note; span context via `attributes-json` |
| Bundle format + NKey verification | Trust boundary (verification at load) |
| Component pooling | Lifecycle + resource limits |

---

## Appendix E — Failure-mode matrix row mapping

| Matrix row | WIT / mapping |
|---|---|
| Row 3 WASM panic | Trap → `-32101` **proposed** |
| Row 4 Invalid signature | Load-time; no WIT |
| Row 5 Missing bundle | `-32108` **proposed** |
| Row 7 CEL runtime error | Parallel path; WASM uses same `policy_fault` code |

---

## Appendix F — Glossary

| Term | Definition |
|---|---|
| **Bundle** | Signed manifest + WASM component artifacts loaded by gateway |
| **Guest** | WASM component implementing `policy-guest` exports |
| **Host** | Gateway Wasmtime embedder implementing `host` imports |
| **Policy input** | Normalized per-request record passed to `evaluate` |
| **Challenge** | Guest decision requesting step-up or HITL approval |

---

## Appendix G — Explicit non-goals (v0.1.0)

- Response body redaction (`shape-response`) — separate component/world later.
- `rate.acquire`, `jsonpath.*`, `kv.fetch`, `nats.request` imports.
- In-guest signature verification.
- Specifying cosign/OCI layout (reference plan § Bundles only).
- NATS subject constants for bundle control plane (only cite plan examples as **proposed** where not in repo).

---

## Appendix H — Security notes

1. **Do not pass raw JWT** in `policy-input`; only validated claims in `attributes-json`.
2. **`log` messages** may contain PII if guest logs `params-json`; operators should scrub at log sink.
3. **`cache-get` hits** must never cross tenant prefix.
4. **Challenge path** must not forward to backend until approval — same invariant as [adaptive-access.md](adaptive-access.md).

---

## Appendix I — Compatibility with earlier plan WIT fragment

[MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) § Tier 3 shows an earlier `world mcp-policy` with `authorize` / `shape-response` and split imports (`trogon:host/spicedb`, etc.). **This document supersedes that fragment for Block B sign-off** with a single `host` interface and `evaluate` export. Migration: first-party bundles rewrite to `policy-bundle` before Phase 3 code lands.

---

## Appendix J — Audit `audit-emit` categories (recommended)

| Category | Purpose |
|---|---|
| `policy` | Rule firings, human-readable policy name |
| `determinism:time` | Record `time-now-ms` when decision depends on it |
| `determinism:cache` | Record cache key/hit/value when decision depends on it |
| `spicedb` | Optional guest-level annotation; authoritative checks remain in gateway `spicedb` block |
| `risk` | Score features when WASM replaces partial risk eval **(proposed)** |

---

## Appendix K — Operator FAQ

**Q: Can a bundle call the network?**  
A: No. Only listed host imports.

**Q: Does `version()` change which WIT linker runs?**  
A: No. Manifest `wit.version` selects the linker.

**Q: What happens if SpiceDB is down inside `spicedb-check`?**  
A: Import returns `err`; gateway maps to `-32107` fail-closed.

**Q: Is WASM Phase 1?**  
A: No. Phase 3 per [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block F; Phase 1 proves NATS/SpiceDB/audit substrate.

**Q: How does this relate to Synadia Protect?**  
A: Protect operates at NATS subject level; this WIT is JSON-RPC-aware MCP policy inside the gateway — complementary, not a replacement ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Inspiration).

---

## Appendix L — Lineage to Block B checkbox

When this document merges, Block B item **Host ABI surface** can cite this file as the WIT sketch prerequisite for Phase 3. CEL builtin naming in Block E should track the import reference table in this doc.

---

## Appendix M — Test vectors (proposed)

| # | Input | Expected |
|---|---|---|
| 1 | `evaluate` returns `allow`, no imports | Audit `allow`, forward |
| 2 | `evaluate` returns `deny("x")` | `-32100`, audit `deny` |
| 3 | `evaluate` calls `time-now-ms`, no determinism audit | Replay warning **(proposed)** |
| 4 | Trap in guest | `-32101`, audit `error` |
| 5 | `init` fails | Instance not pooled |

---

## Appendix N — Related ADRs

| ADR | Relevance |
|---|---|
| [0001 Tenancy](../adr/0001-tenancy-model.md) | Cache key prefix, audit tenant field |
| [0002 Identity layers](../adr/0002-identity-layers.md) | `actor-id`, `attributes-json` |
| [0005 TTL and audience](../adr/0005-token-ttl-and-audience.md) | Not directly imported; egress mint remains host-native |

---

## Appendix O — Document metrics

Target length: 500–800 lines for Block B review depth. Identifiers marked **proposed** are not verified in production code paths yet.

---

*End of reference.*

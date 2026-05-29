# Host ABI surface reference (CEL builtins)

**Status:** Phase 1 contract for CEL builtins. WASM port (Phase 3) MUST preserve these signatures.

**Document type:** Diataxis **reference** (signatures, side-effect classes, error behavior, WIT preview).

**Related:** [MCP policy WIT host ABI sketch](mcp-policy-wit-sketch.md), [reference-cel-variables.md](reference-cel-variables.md), [wasm-bundle-format.md](wasm-bundle-format.md), [policy-dsl-choice.md](policy-dsl-choice.md), [MCP gateway plan Block B Host ABI surface](../../MCP_GATEWAY_PLAN.md) and [Wire-Format Pins for Phase 1](../../MCP_GATEWAY_PLAN.md#wire-format-pins-for-phase-1).

**Implementation target:** `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` (Block E); WIT linker in Phase 3 per [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md).

Identifiers marked **(proposed)** are not verified in production code paths yet.

---

## 1. Scope

This reference is the single lookup for every **host function** callable from CEL policy programs and the corresponding **WIT import** shape for Phase 3 WASM bundles. It covers:

- Namespace roots that expose functions: `spicedb.*`, `cache.*`, `audit.*`, `rate.*`
- Standalone builtins: `kv.fetch`, `nats.request`
- JSON and clock helpers: `jsonpath.*`, `time.*`

Host functions are **not** CEL variables. They are capability-gated imports registered at evaluator setup ([`cel_builtins::register`](../../rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/mod.rs)). Policy authors call them with namespace receiver syntax:

```cel
spicedb.check("user:alice", "invoke", "tool:github|create_issue")
```

WASM guests call the kebab-case WIT imports (`spicedb-check`, `cache-get`, ...) declared in the bundle manifest. CEL and WASM share semantics; only marshalling differs (CEL JSON values vs WIT strings / byte lists).

**Out of scope:** CEL variable roots (`mcp.*`, `jwt.*`, `nats.*`, `request.*`, `response.*`, `chain.*`) -- see [reference-cel-variables.md](reference-cel-variables.md) and [MCP_GATEWAY_PLAN.md SS8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace).

---

## 2. Surface inventory

Side-effect class definitions:

| Class | Meaning |
|---|---|
| `pure` | No host I/O; result depends only on arguments and evaluation bindings |
| `cache` | Reads or writes gateway memoization (may affect later calls in the same evaluation) |
| `audit` | Appends fields to the in-flight audit envelope (committed only on successful decision emit; see SS13) |
| `nats-io` | NATS request/reply to an allowed subject prefix |
| `kv-io` | JetStream KV read on an allowed bucket prefix |

Default host timeouts apply when the builtin performs I/O. Authors may pass an explicit timeout only where noted (`nats.request`). All timeouts are clamped to the request deadline from header `mcp-deadline-unix-ms`.

| Host function | CEL signature | Return type | Side-effect class | Error behavior | Default timeout |
|---|---|---|---|---|---|
| `spicedb.check` | `(subject: string, permission: string, resource: string) -> bool` | `bool` | `nats-io` (gRPC to PDP) | PDP unreachable: CEL abort -> `-32107` `authz_unreachable`. Malformed args: `-32101` `policy_fault`. Deny is `false`, not an error. | **500 ms** (proposed; clamp to remaining request deadline) |
| `spicedb.bulk_check` | `(subject: string, permission: string, resources: list<string>) -> map<string,bool>` | `map<string,bool>` | `nats-io` | Same as `check`. Partial item errors: fail-closed for shaping; omit failed keys from map and treat as deny. | **500 ms** + **50 ms** per 100 items (proposed) |
| `cache.get` | `(key: string) -> any` | `any` | `cache` | Missing key: CEL null. Key too long / tenant scope violation: `-32101`. | n/a (in-process) |
| `cache.set` | `(key: string, value: any, ttl: duration) -> bool` | `bool` | `cache` | Value too large / TTL out of bounds: `-32101`. Success: `true`; rejected write: `false`. | n/a |
| `audit.emit` | `(extra_fields: map<string,any>) -> bool` | `bool` | `audit` | Reserved top-level envelope keys: `-32101`. Invalid map type: `-32101`. Success: `true`. | n/a |
| `rate.acquire` | `(scope: string, key: string, budget: int, window: duration) -> bool` | `bool` | `kv-io` when `scope` is cluster-wide; else `cache` | KV unreachable for cluster scope: `-32105` or `-32101` per [rate-limiting.md](rate-limiting.md). Limited: `false` (not an error). | **2000 ms** for KV path (proposed) |
| `kv.fetch` | `(bucket: string, key: string) -> any` | `any` | `kv-io` | Bucket not allowlisted: `-32101`. Key missing: null. KV timeout: `-32101`. | **1000 ms** (proposed) |
| `nats.request` | `(subject: string, payload: bytes, timeout: duration) -> bytes` | `bytes` | `nats-io` | Subject not allowlisted: `-32101`. No reply / timeout: `-32101` or plugin failClosed deny. | **caller `timeout`**, max **5000 ms** (proposed) |
| `jsonpath.query` | `(json: any, path: string) -> list<any>` | `list<any>` | `pure` | Invalid path / document: `-32101`. Zero matches: empty list. | n/a |
| `jsonpath.extract` | `(json: any, path: string) -> any` | `any` | `pure` | Zero matches: `-32101`. Multiple matches: `-32101`. | n/a |
| `time.now` | `() -> timestamp` | `timestamp` | `pure` (non-deterministic across replays) | n/a | n/a |
| `time.hour_utc` | `() -> int` | `int` (0-23) | `pure` (non-deterministic across replays) | n/a | n/a |
| `time.weekday` | `() -> int` | `int` (0-6, Sunday=0) | `pure` (non-deterministic across replays) | n/a | n/a |

### WIT column (proposed imports for `trogon:mcp-policy@0.2.0`)

| CEL function | WIT import (proposed) | WIT signature |
|---|---|---|
| `spicedb.check` | `spicedb-check` | `(subject: string, permission: string, resource: string) -> result<bool, host-error>` |
| `spicedb.bulk_check` | `spicedb-bulk-check` | `(subject: string, permission: string, resources: list<string>) -> result<list<tuple<string, bool>>, host-error>` |
| `cache.get` | `cache-get` | `(key: string) -> option<list<u8>>` |
| `cache.set` | `cache-set` | `(key: string, value: list<u8>, ttl-secs: u32) -> result<_, host-error>` |
| `audit.emit` | `audit-emit` | `(fields-json: string) -> result<_, host-error>` |
| `rate.acquire` | `rate-acquire` | `(scope: string, key: string, budget: u32, window-secs: u32) -> result<bool, host-error>` |
| `kv.fetch` | `kv-fetch` | `(bucket: string, key: string) -> result<option<list<u8>>, host-error>` |
| `nats.request` | `nats-request` | `(subject: string, payload: list<u8>, timeout-ms: u32) -> result<list<u8>, host-error>` |
| `jsonpath.query` | `jsonpath-query` | `(document-json: string, path: string) -> result<list<string>, host-error>` |
| `jsonpath.extract` | `jsonpath-extract` | `(document-json: string, path: string) -> result<string, host-error>` |
| `time.now` | `time-now-ms` | `() -> u64` |
| `time.hour_utc` | `time-hour-utc` | `() -> u8` |
| `time.weekday` | `time-weekday` | `() -> u8` |

v0.1.0 WIT in [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) covers a subset (`spicedb-check`, `cache-get/set`, `audit-emit`, `time-now-ms`). This reference extends the ABI for Block B sign-off; Phase 3 minors add imports without changing existing signatures.

### Implementation drift (today)

| Item | This reference | Current stub (`cel_builtins/`) |
|---|---|---|
| `spicedb.check` arg order | `(subject, permission, resource)` | `(resource, permission, subject)` -- reconcile in Block E |
| `audit.emit` | 1-arg map | 2-arg `(category, fields)` -- reconcile in Block E |
| `rate.acquire` | 4-arg windowed counter | 3-arg token bucket -- reconcile in Block E |
| `jsonpath.*` | `query`, `extract` | `get`, `set`, `delete` -- `get` aliases `extract`; `set`/`delete` reserved for rewrite tier **(proposed)** |
| `kv.fetch`, `nats.request` | specified here | not registered yet |

---

## 3. `spicedb.*`

SpiceDB host functions delegate to the gateway PDP client (`spicedb-rs-client`). They never expose raw gRPC to policy code.

### 3.1 `spicedb.check`

**CEL:**

```cel
spicedb.check(subject, permission, resource) -> bool
```

| Argument | Type | Description |
|---|---|---|
| `subject` | `string` | SpiceDB subject reference, e.g. `user:alice`, `agent:acme/oncall-responder` |
| `permission` | `string` | Permission name on the resource type, e.g. `invoke`, `read`, `list` |

**Return:** `true` if Authzed `CHECK_PERMISSIONSHIP_HAS_PERMISSION`; `false` if denied or resource not found in graph for that check (see SS3.4).

**Consistency:** Gateway sends `Consistency::at_least_as_fresh` with session-scoped `ZedToken` when present in `mcp-sessions` KV ([bulk-check-permission.md](bulk-check-permission.md)). Fully consistent mode is used when no token exists.

**Side-effect class:** `nats-io` (PDP gRPC).

**Errors:**

| Condition | CEL / client outcome |
|---|---|
| PDP unreachable, deadline exceeded | Evaluation error -> `-32107` `authz_unreachable` |
| Malformed subject/resource strings | `-32101` `policy_fault` |
| Denied permission | `false` (normal boolean) |

**Default timeout:** **500 ms** (proposed), min(remaining request deadline, configured `MCP_GATEWAY_SPICEDB_TIMEOUT_MS` **(proposed)**).

### 3.2 `spicedb.bulk_check`

**CEL:**

```cel
spicedb.bulk_check(subject, permission, resources) -> map<string, bool>
```

| Argument | Type | Description |
|---|---|---|
| `subject` | `string` | Same as `check` |
| `permission` | `string` | Same as `check` |
| `resources` | `list<string>` | Resource id strings; map keys in the result echo input strings |

**Return:** Map from each input resource string to allow (`true`) or deny (`false`). Used for `tools/list` catalog shaping ([tools-list-filtering.md](tools-list-filtering.md), [bulk-check-permission.md](bulk-check-permission.md)).

**Implementation note:** Backed by Authzed `CheckBulkPermissions` (one RPC per call). Gateway pairs results by echoed request item, not positional index.

### 3.3 ZedToken propagation

| Stage | Behavior |
|---|---|
| Session establish | `initialize` (or first gated call) stores latest `checked_at` ZedToken in session KV keyed by `mcp-session-id` |
| Subsequent `check` / `bulk_check` | Host attaches token as `at_least_as_fresh` cursor |
| After successful bulk | Host updates session token from response `checked_at` |

**Cache hit semantics:** When a `(subject, permission, resource)` result is served from the gateway check cache without a new RPC, audit records `spicedb.cache_hit: true`. Cache entries are invalidated when session ZedToken advances or TTL expires ([bulk-check-permission.md SS4](bulk-check-permission.md)).

### 3.4 Not found vs denied

| PDP outcome | `check` return | `bulk_check` value | Notes |
|---|---|---|---|
| `HAS_PERMISSION` | `true` | `true` | Allow |
| `NO_PERMISSION` | `false` | `false` | Explicit deny |
| Resource/object missing | `false` | `false` | Indistinguishable from deny in CEL; audit may include `spicedb.permissionship` **(proposed)** |
| Caveated / conditional | `false` until caveat satisfied | `false` | Caveats not evaluated in Phase 1 host ABI **(proposed Phase 2+)** |
| gRPC / item `Error` | CEL error (fail-closed) | Key omitted or entire call fails | List shaping: drop item on partial error |

Policy authors should assume **`false` means "must not proceed"** on gated methods unless a bundle explicitly implements fail-open for list paths.

---

## 4. `cache.*`

In-process memoization for expensive host reads (ZedToken-scoped check results, derived JSON slices, session-scoped policy flags). Distinct from JetStream KV (`kv.fetch`) and from the schema cache crate.

### 4.1 `cache.get`

**CEL:** `cache.get(key) -> any`

| Property | Value |
|---|---|
| Key scope | Host prefixes `{tenant_id}/{bundle_revision}/{key}` automatically |
| Max key length | **256** bytes UTF-8 (proposed; matches WIT sketch) |
| Miss | CEL null |
| Hit | Deserialized JSON value last stored via `cache.set` |

**Side-effect class:** `cache` (read).

### 4.2 `cache.set`

**CEL:** `cache.set(key, value, ttl) -> bool`

| Property | Value |
|---|---|
| `ttl` | CEL `duration`; converted to seconds, clamped to **[1, 86400]** |
| Max value size | **65536** bytes serialized JSON (proposed; aligns with WIT `cache-set` cap) |
| Backing store | **(proposed)** In-process LRU per gateway replica with optional async mirror to JetStream KV bucket `mcp-gateway-config` subkey `cache/{tenant}/{key}` for TTL > 60s |
| Return | `true` if accepted; `false` if rejected (oversize, invalid ttl) without aborting evaluation |

**Side-effect class:** `cache` (write).

**Determinism:** Per-evaluation memo only unless documented otherwise. Cross-request cache content may differ between replicas; policies that branch on cache must emit determinism audit when replay matters ([mcp-policy-wit-sketch.md SS Determinism rules](mcp-policy-wit-sketch.md#determinism-rules)).

---

## 5. `audit.*`

### 5.1 `audit.emit`

**CEL:**

```cel
audit.emit(extra_fields) -> bool
```

| Argument | Type | Description |
|---|---|---|
| `extra_fields` | `map<string, any>` | JSON-serializable fields merged into the audit envelope `extra` object |

**Merge rule:** Shallow merge into envelope `extra`. Host adds correlation (`request_id`, `trace_id`, evaluation timestamp) before JetStream publish. Multiple calls in one evaluation merge in call order.

**Disallowed keys:** Keys that would shadow top-level audit envelope fields are rejected with `-32101`:

```text
schema, ts, trace_id, span_id, instance_id, tenant, session_id,
caller, subject_in, subject_out, direction, method, method_root,
tool, decision, rules_fired, rewrites, spicedb, error, latency_us,
agent_id, agent_version, wkl, purpose, act_chain
```

Authors SHOULD nest policy-specific data under a stable prefix, e.g. `extra_fields.policy_rule = "business_hours"`.

**Side-effect class:** `audit` (lazy commit; see SS13).

**WIT note:** v0.1.0 sketch uses `audit-emit(category, fields-json)`. CEL passes a single map; host may derive `category` from required field `_category` inside the map if present **(proposed)**.

---

## 6. `rate.*`

### 6.1 `rate.acquire`

**CEL:**

```cel
rate.acquire(scope, key, budget, window) -> bool
```

| Argument | Type | Description |
|---|---|---|
| `scope` | `string` | `local` (in-memory per replica) or `cluster` (JetStream KV authoritative) |
| `key` | `string` | Bucket id suffix; host prefixes tenant per [rate-limiting.md SS3](rate-limiting.md) |
| `budget` | `int` | Maximum events per window (> 0) |
| `window` | `duration` | Sliding window length |

**Return:** `true` if one unit consumed; `false` if rate-limited (evaluation continues; rule typically combines with `&&` and maps to deny).

**Backing store:**

| `scope` | Store | Side-effect class |
|---|---|---|
| `local` | In-memory token/window counter on gateway replica | `cache` |
| `cluster` | JetStream KV bucket `mcp-rate-limits` ([rate-limiting.md SS5](rate-limiting.md)) | `kv-io` |

**Idiomatic rule:**

```cel
mcp.method == "tools/call"
  && mcp.tool.name == "run_llm_inference"
  && rate.acquire("cluster", "tool/" + jwt.tenant + "/" + mcp.tool.name, 10, duration("1m"))
```

When `false`, gateway JSON-RPC on explicit deny path uses `-32105` `rate_limited` with `data.scope` derived from `scope` and key prefix.

**Errors:** KV unreachable for `cluster` scope -> fail-closed per [rate-limiting.md SS7](rate-limiting.md). Wrong arity/types -> `-32101`.

**Default timeout:** **2000 ms** for KV CAS path (proposed).

---

## 7. `kv.fetch`

**CEL:**

```cel
kv.fetch(bucket, key) -> any
```

Read-only JetStream KV access for bundle-configured lookups (feature flags, emergency knobs, operator-maintained allowlists). Not a general KV browser.

### 7.1 Allowlisted bucket prefix

Only buckets whose names match **`mcp-*`** (regex `^mcp-[a-z0-9-]+$`) are accessible. Examples aligned with repo constants and proposed buckets:

| Bucket | Typical use | Source |
|---|---|---|
| `mcp-gateway-config` | Policy overlays, trusted signers | [reference-subject-grammar.md SS7](reference-subject-grammar.md) |
| `mcp-sessions` | **(proposed)** read-only session metadata slices | [mcp-session-model.md](mcp-session-model.md) |
| `mcp-rate-limits` | **(proposed)** inspect-only; prefer `rate.acquire` for writes | [rate-limiting.md](rate-limiting.md) |
| `mcp-trust-bundles` | SPIFFE trust bundle PEM | `trogon-sts` |
| `mcp-jwks` | Mesh JWKS document | `trogon-jwks-publisher` |
| `mcp-agent-registry` | **(proposed)** registry snapshot reads | [registry.md](registry.md) |

Arbitrary bucket names (`app-config`, `secrets`) -> `-32101` `policy_fault`.

### 7.2 Key and value limits

| Limit | Value |
|---|---|
| Max key length | **512** bytes (proposed) |
| Max value bytes returned | **65536** (proposed); larger values truncated with error |
| Missing key | CEL null |

**Side-effect class:** `kv-io`.

**Default timeout:** **1000 ms** (proposed).

**Capability:** `host.kv-fetch` must appear in bundle manifest `capabilities.imports`.

---

## 8. `nats.request`

**CEL:**

```cel
nats.request(subject, payload, timeout) -> bytes
```

Synchronous NATS request/reply for Tier 2.5 ext-proc-style plugins. Required when policy must consult an out-of-process modifier before allow/deny.

| Argument | Type | Description |
|---|---|---|
| `subject` | `string` | Must begin with `{MCP_PREFIX}.plugin.` (default `mcp.plugin.`) |
| `payload` | `bytes` | Opaque request envelope JSON (UTF-8) **(proposed schema in [nats-callout-plugin.md](nats-callout-plugin.md))** |
| `timeout` | `duration` | Max wait for reply; clamped to **[100 ms, 5000 ms]** and remaining request deadline |

**Return:** Reply payload bytes on success.

**Subject allowlist:**

```text
{prefix}.plugin.{plugin_name}
```

Examples: `mcp.plugin.redaction`, `mcp.plugin.risk-scorer`. Wildcards and non-plugin subjects (`mcp.server.*`, `mcp.gateway.*`, `$SYS.*`) -> `-32101`.

**Queue group:** Gateway publishes to the subject; plugins subscribe with queue group `mcp-plugin-{plugin_name}` per [MCP_GATEWAY_PLAN.md SS3](../../MCP_GATEWAY_PLAN.md#3-queue-group-strategy).

**Failure behavior (failClosed default):** No responder or timeout -> CEL evaluation error -> deny path `-32100` or `-32101` depending on bundle `plugin.fail_mode` **(proposed)**; default **closed**.

**Side-effect class:** `nats-io`.

**Capability:** `host.nats-request`.

---

## 9. `jsonpath.*`

JSONPath access for params/result inspection and rewrite policies. Dialect: **RFC 9535 strict** (proposed library pin in Block E). Paths MUST start with `$`.

### 9.1 `jsonpath.query`

**CEL:** `jsonpath.query(json, path) -> list<any>`

| Outcome | Result |
|---|---|
| 0 matches | `[]` |
| 1+ matches | List of values in document order |
| Invalid path | `-32101` |

**Side-effect class:** `pure`.

### 9.2 `jsonpath.extract`

**CEL:** `jsonpath.extract(json, path) -> any`

| Outcome | Result |
|---|---|
| Exactly 1 match | The matched value |
| 0 matches | `-32101` |
| 2+ matches | `-32101` (`jsonpath_ambiguous`) |

Use `extract` when the policy requires a scalar; use `query` for filters and comprehensions.

### 9.3 Legacy aliases **(proposed migration)**

| Legacy (stub today) | Maps to |
|---|---|
| `jsonpath.get(doc, path)` | `jsonpath.extract(doc, path)` |
| `jsonpath.set` / `jsonpath.delete` | Rewrite tier only; not in list-filter programs ([tools-list-filtering.md SS4.4](tools-list-filtering.md)) |

---

## 10. `time.*`

Host wall clock for time-bounded rules. Values are UTC unless noted.

| Function | CEL signature | Return | Range |
|---|---|---|---|
| `time.now` | `() -> timestamp` | Current instant | Unix timestamp |
| `time.hour_utc` | `() -> int` | Hour of day UTC | 0-23 |
| `time.weekday` | `() -> int` | Day of week UTC | 0-6 (Sunday=0) |

**Side-effect class:** `pure` for classification; **non-deterministic across audit replay** when decisions branch on these values. List-filter programs MUST NOT call `time.*` ([tools-list-filtering.md](tools-list-filtering.md)).

**Plan alignment:** [MCP_GATEWAY_PLAN.md SS8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace) lists `time.now`, `time.hour_utc`, `time.weekday` as host-evaluated bindings; this reference exposes them as functions for uniform namespace syntax with other host roots.

**WIT:** `time-now-ms` exists in v0.1.0 sketch; `time-hour-utc` and `time-weekday` ship in **0.2.0** minor (proposed).

---

## 11. WIT preview

Normative sketch for Phase 3 host interface extending [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md). Package version bump adds imports without breaking v0.1.0 symbols.

```wit
package trogon:mcp-policy@0.2.0;

interface host-ext {
  /// CEL: spicedb.check(subject, permission, resource)
  spicedb-check: func(
    subject: string,
    permission: string,
    resource: string,
  ) -> result<bool, host-error>;

  /// CEL: spicedb.bulk_check(subject, permission, resources)
  spicedb-bulk-check: func(
    subject: string,
    permission: string,
    resources: list<string>,
  ) -> result<list<tuple<string, bool>>, host-error>;

  cache-get: func(key: string) -> option<list<u8>>;
  cache-set: func(key: string, value: list<u8>, ttl-secs: u32) -> result<_, host-error>;

  /// CEL: audit.emit(map) serialized to JSON object string
  audit-emit: func(fields-json: string) -> result<_, host-error>;

  /// CEL: rate.acquire(scope, key, budget, window)
  rate-acquire: func(
    scope: string,
    key: string,
    budget: u32,
    window-secs: u32,
  ) -> result<bool, host-error>;

  /// CEL: kv.fetch(bucket, key) — bucket must match ^mcp-
  kv-fetch: func(bucket: string, key: string) -> result<option<list<u8>>, host-error>;

  /// CEL: nats.request(subject, payload, timeout)
  nats-request: func(
    subject: string,
    payload: list<u8>,
    timeout-ms: u32,
  ) -> result<list<u8>, host-error>;

  jsonpath-query: func(document-json: string, path: string) -> result<list<string>, host-error>;
  jsonpath-extract: func(document-json: string, path: string) -> result<string, host-error>;

  time-now-ms: func() -> u64;
  time-hour-utc: func() -> u8;
  time-weekday: func() -> u8;
}

/// v0.1.0 `host` interface remains; `host-ext` merges at link time in 0.2.0 world (proposed).
world policy-bundle-v0-2 {
  import host;      // from mcp-policy-wit-sketch.md
  import host-ext;
  export policy-guest;
}
```

Cross-reference mapping table: [mcp-policy-wit-sketch.md SS Mapping to CEL builtins](mcp-policy-wit-sketch.md#mapping-to-cel-builtins).

---

## 12. Capability gating

Every host function is **off by default** until the bundle manifest declares it.

### 12.1 CEL bundle manifest **(proposed until Phase 3 unified bundle loader)**

```yaml
capabilities:
  cel_host:
    - spicedb.check
    - spicedb.bulk_check
    - cache.get
    - cache.set
    - audit.emit
    - rate.acquire
    - jsonpath.query
    - jsonpath.extract
    - time.now
    - time.hour_utc
    - time.weekday
    # optional high-trust:
    # - kv.fetch
    # - nats.request
```

### 12.2 WASM bundle manifest

Per [wasm-bundle-format.md SS3.1](wasm-bundle-format.md):

```toml
[capabilities.imports]
hosts = [
  "host.spicedb-check",
  "host.cache-get",
  "host.audit-emit",
]
```

### 12.3 Gateway enforcement

| Check | When | On violation |
|---|---|---|
| Static scan | Bundle activation / hot-swap | Reject bundle; keep prior revision |
| Runtime | Host builtin dispatch | `-32101` `policy_fault` `undeclared_host_capability` |
| Link time (WASM) | Wasmtime component link | Instance not pooled; load fails |

Programs MUST declare the **superset** of host functions reachable from any rule in the bundle, including `tools/list` filter programs and ingress gates.

---

## 13. Side-effect ordering

Side-effecting builtins (`audit.emit`, `rate.acquire` with `cluster` scope, `cache.set`, `nats.request`) participate in a per-evaluation **effect journal**.

```
1. CEL / WASM evaluation runs; host calls append to journal (not yet visible externally).
2. If evaluation completes with a policy decision AND no fault:
     a. Apply rate / cache / nats effects in journal order.
     b. Merge audit.emit payloads into envelope.
     c. Publish audit JetStream message once.
3. If evaluation faults (CEL error, WASM trap, timeout):
     Drop entire journal — no KV writes, no NATS plugin side effects, no audit.emit merge.
4. If decision is deny before backend forward:
     Effects still commit if evaluation succeeded (author may have emitted audit context for deny).
```

**Rationale:** Prevents partial external state when `-32101` `policy_fault` fires mid-rule. Matches failClosed posture in [failure-mode-matrix.md](failure-mode-matrix.md).

**Exception:** Platform post-auth rate limits and inflight semaphores outside CEL are not rolled back by this journal.

---

## 14. Cross-references

| Topic | Document |
|---|---|
| WIT sketch (v0.1.0 subset) | [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) |
| WASM bundle packaging + capabilities | [wasm-bundle-format.md](wasm-bundle-format.md) |
| DSL choice + compile contract | [policy-dsl-choice.md](policy-dsl-choice.md) |
| ZedToken + bulk check cache | [bulk-check-permission.md](bulk-check-permission.md) |
| Rate limit placement | [rate-limiting.md](rate-limiting.md) |
| Audit envelope `extra` field | [reference-audit-envelope.md](reference-audit-envelope.md) |
| Failure modes | [failure-mode-matrix.md](failure-mode-matrix.md) |

---

## Revision history

| Date | Change |
|---|---|
| 2026-05-28 | Initial Block B reference: full CEL host inventory, WIT preview, capability gating, side-effect ordering |

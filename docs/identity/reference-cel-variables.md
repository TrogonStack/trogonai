# CEL variable namespace reference

**Diátaxis:** reference (strict lookup, worked examples).

**Status:** Phase 1 contract. Roots are pinned; fields under a root may be added without a version bump, never renamed or moved.

**Related:** [Policy DSL choice](policy-dsl-choice.md) · [MCP policy WIT host ABI sketch](mcp-policy-wit-sketch.md) · [Actor chain (`act_chain`)](act-chain.md) · [BulkCheckPermission and ZedToken cache](bulk-check-permission.md) · [MCP gateway plan § Wire-Format Pin 8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace)

**Implementation anchors:** `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` (variable binding, `chain.*` helpers), `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` (host functions). Block E stubs return `NotImplemented` until wired; signatures in this document match the wired host ABI where noted.

---

## 1. Roots overview

Reproduced verbatim from [MCP gateway plan § Wire-Format Pin 8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace):

| Root | Phase | Variables | Notes |
|---|---|---|---|
| `mcp.*` | both | `mcp.method` (string), `mcp.method_root` (string), `mcp.tool.name` (string), `mcp.tool.target` (string), `mcp.resource.uri` (string), `mcp.prompt.name` (string), `mcp.params` (JSON map), `mcp.result` (JSON map; response phase only) | Protocol surface. |
| `jwt.*` | both | `jwt.sub` (string), `jwt.tenant` (string), `jwt.roles` (list\<string\>), `jwt.iss` (string), `jwt.aud` (string), `jwt.agent_id` (string, optional — registered agent identity, ADR 0002), `jwt.agent_version` (string, optional), `jwt.wkl` (string — attested workload: SPIFFE URI or `sentinel:*`), `jwt.purpose` (string, optional intent claim), `jwt.session_id` (string, optional), `jwt.act_chain` (list\<map\> — see [act-chain.md](act-chain.md)), `jwt.<custom>` | Claims from validated JWT. Custom claims are dynamic. |
| `chain.*` | both | *(functions, bound to `jwt.act_chain`)* — `chain.contains(agent_id) -> bool`, `chain.originator() -> map`, `chain.depth() -> int` | Delegation lineage helpers (ADR 0002). |
| `nats.*` | both | `nats.subject` (string), `nats.headers` (map\<string,string\>), `nats.account` (string) | Transport context. |
| `request.*` | both | `request.id` (string), `request.deadline` (timestamp), `request.session_id` (string) | Request-level context. |
| `response.*` | response only | `response.is_error` (bool), `response.list_filter_index` (int) | The list-filter index is set when re-evaluating the same rule per-item during `*/list` shaping. |
| `time.*` | both | `time.now` (timestamp), `time.hour_utc` (int), `time.weekday` (int 0–6) | Host-evaluated. |
| `spicedb.*` | both | *(functions, not variables)* — `spicedb.check(subject, perm, resource) -> bool`, `spicedb.bulk_check(subject, perm, [resources]) -> map<string,bool>` | Capability-gated host import. |
| `cache.*` | both | *(functions)* — `cache.get(key) -> any`, `cache.set(key, value, ttl) -> bool` | ZedToken caching, schema caching. |
| `audit.*` | both | *(functions)* — `audit.emit(extra_fields)` | Merged into the audit envelope's `extra` field. |
| `rate.*` | both | *(functions)* — `rate.acquire(scope, key, budget, window) -> bool` | Returns false when rate-limited; rule typically denies on false. |

The `mcp.params` and `mcp.result` JSON-map roots support indexing (`mcp.params.name`, `mcp.params["complex-key"]`) and traversal with the standard CEL operators. Deep traversal into untyped JSON uses the `jsonpath.*` host functions for ergonomic access.

### 1.1 Evaluation phases

| Phase | When | Available roots | Typical use |
|---|---|---|---|
| **Request** | Before backend forward (ingress) | All roots except `mcp.result`, `response.*` | Allow/deny, rate limits, SpiceDB gate, audit enrichment |
| **Response** | After backend reply (egress shaping) | All roots including `mcp.result`, `response.*` | Redaction, `tools/list` filtering, response mutation |

Rules compiled for `tools_list_filter` program class restrict impure builtins on both phases; see [tools-list-filtering.md](tools-list-filtering.md) §4.4.

---

## 2. `mcp.*` — protocol surface

### 2.1 Variable list

| Variable | Type | Request | Response | Source |
|---|---|---|---|---|
| `mcp.method` | string | yes | yes | JSON-RPC method with slash, e.g. `tools/call` |
| `mcp.method_root` | string | yes | yes | First segment: `tools`, `resources`, `prompts`, … |
| `mcp.tool.name` | string | yes | yes | Tool name after virtual-MCP split (`github::create_issue` → `create_issue`) |
| `mcp.tool.target` | string | yes | yes | Virtual server id (`github`) or backend `server_id` |
| `mcp.resource.uri` | string | yes | yes | MCP resource URI on `resources/read` |
| `mcp.prompt.name` | string | yes | yes | Prompt name on `prompts/get` |
| `mcp.params` | map (JSON) | yes | yes | JSON-RPC `params` object |
| `mcp.result` | map (JSON) | **no** | yes | JSON-RPC `result` object |

Empty strings when a field does not apply to the current method (e.g. `mcp.tool.name` on `ping`).

### 2.2 Example expressions

```cel
// Gate mutating tool calls only
mcp.method == "tools/call" && mcp.tool.target == "github"

// Require params field on a specific tool
mcp.method == "tools/call"
  && mcp.tool.name == "run_query"
  && has(mcp.params.query)

// Response-phase redaction guard
mcp.method == "tools/call"
  && response.is_error == false
  && has(mcp.result.content)
```

### 2.3 Phase notes

Request-phase rules see `mcp.params` from the client payload. Response-phase rules additionally see `mcp.result` after the backend returns. Do not reference `mcp.result` in request-only bundle programs — compilation may succeed but evaluation binds an empty map.

---

## 3. `jwt.*` — validated claims

### 3.1 Variable list

| Variable | Type | Optional | Semantics |
|---|---|---|---|
| `jwt.sub` | string | no | Authenticated principal (`user:alice`, `agent:acme/oncall`) |
| `jwt.tenant` | string | no | Tenant slug; authoritative from JWT, not headers |
| `jwt.roles` | list\<string\> | yes | Coarse RBAC roles from issuer |
| `jwt.iss` | string | no | Token issuer URL |
| `jwt.aud` | string | no | Audience string (first element if claim is array) |
| `jwt.agent_id` | string | yes | Registered agent id (ADR 0002); null/absent for human callers |
| `jwt.agent_version` | string | yes | Agent semver from registry |
| `jwt.wkl` | string | no | Workload identity (SPIFFE URI or sentinel such as `human`) |
| `jwt.purpose` | string | yes | Intent claim for adaptive access |
| `jwt.session_id` | string | yes | Gateway-issued MCP session id |
| `jwt.act_chain` | list\<map\> | yes | Delegation chain; see [act-chain.md](act-chain.md) |
| `jwt.<custom>` | dynamic | varies | Any additional validated claim |
| `has(jwt.<claim>)` | bool | — | Presence test for optional claims |

See [jwt-claim-schema.md](jwt-claim-schema.md) for claim validation and ingress hardening.

### 3.2 Example expressions

```cel
// Deny if caller lacks engineering role
!"engineer" in jwt.roles

// Require attested agent for mesh traffic
jwt.wkl.startsWith("spiffe://")
  && has(jwt.agent_id)
  && jwt.agent_id != ""

// Scope-sensitive tool access via custom claim
mcp.method == "tools/call"
  && has(jwt.allowed_tools)
  && mcp.tool.name in jwt.allowed_tools
```

### 3.3 Phase notes

JWT bindings are identical on request and response phases for a single JSON-RPC transaction. Token refresh mid-session is out of band; rules evaluate the token validated at ingress.

---

## 4. `chain.*` — delegation helpers

Bound to `jwt.act_chain`. The gateway also exposes a top-level `chain` list mirroring `jwt.act_chain` so helpers dispatch as `chain.contains(...)`, `chain.depth()`, `chain.originator()`. See [ADR 0002](../adr/0002-identity-layers.md) and [act-chain.md](act-chain.md).

### 4.1 Functions

| Function | Signature | Return | Semantics |
|---|---|---|---|
| `chain.contains` | `(agent_id: string) -> bool` | bool | True if any hop's `agent_id` equals the argument |
| `chain.originator` | `() -> map` | map | First chain entry (index 0); null if chain empty |
| `chain.depth` | `() -> int` | int | `size(jwt.act_chain)` |

### 4.2 Example expressions

```cel
// Reject deep delegation for destructive tools
chain.depth() > 4 && mcp.tool.name == "delete_repository"

// Require on-call agent in lineage
!chain.contains("acme/oncall-responder")
  ? deny("missing on-call agent in act_chain")

// Attribute rate budget to human originator, not immediate agent
chain.originator().sub
```

### 4.3 Phase notes

Available on both phases. `chain.contains` matches `agent_id` fields only, not `sub` — human hops without `agent_id` are invisible to `contains`.

---

## 5. `nats.*` — transport context

### 5.1 Variable list

| Variable | Type | Source |
|---|---|---|
| `nats.subject` | string | Full ingress or egress NATS subject |
| `nats.headers` | map\<string,string\> | NATS message headers (gateway-sanitized on ingress) |
| `nats.account` | string | NATS account name for the connection |

### 5.2 Example expressions

```cel
// Allow only gateway ingress lane
nats.subject.startsWith("mcp.gateway.request.")

// Read correlation header surfaced in audit
nats.headers["mcp-correlation-id"]

// Tenant-scoped subject prefix (soft tenancy)
nats.subject.contains("." + jwt.tenant + ".")
```

### 5.3 Phase notes

Both phases. Header values are strings; parse numeric headers explicitly (`int(nats.headers["mcp-deadline-unix-ms"])` when present).

---

## 6. `request.*` — request context

### 6.1 Variable list

| Variable | Type | Source |
|---|---|---|
| `request.id` | string | JSON-RPC `id` when present; synthetic otherwise |
| `request.deadline` | timestamp | From `mcp-deadline-unix-ms` header (clamped) |
| `request.session_id` | string | `mcp-session-id` header / JWT `session_id` |

### 6.2 Example expressions

```cel
// Deny when deadline already passed
request.deadline < time.now

// Session-scoped cache key component
"zed/" + jwt.tenant + "/" + request.session_id

// Idempotent rate key including JSON-RPC id
rate.acquire("req", jwt.sub + "/" + string(request.id), 1, duration("10s"))
```

### 6.3 Phase notes

Both phases for the same in-flight request. `request.deadline` is absent when the client omits the header — use `has()` before compare.

---

## 7. `response.*` — response context

### 7.1 Variable list

| Variable | Type | Response only | Semantics |
|---|---|---|---|
| `response.is_error` | bool | yes | True when JSON-RPC `error` is set |
| `response.list_filter_index` | int | yes | Zero-based index of current `tools/list` candidate |

### 7.2 Example expressions

```cel
// Skip shaping on backend error
response.is_error == true

// Debug predicate: only evaluate first three tools (testing)
response.list_filter_index < 3

// Combine with per-tool name (list filter class)
mcp.method == "tools/list" && mcp.tool.name != ""
```

### 7.3 Phase notes

**Response phase only.** Referencing `response.*` in request-phase rules is a bundle authoring error; the gateway rejects or treats bindings as unset depending on program class.

---

## 8. `time.*` — host clock

### 8.1 Variable list (wire pin)

| Name | Type | Semantics |
|---|---|---|
| `time.now` | timestamp | Current UTC instant |
| `time.hour_utc` | int | Hour 0–23 UTC |
| `time.weekday` | int | Day of week 0–6 (Sunday = 0) |

**Wired host ABI today:** `time.now()` is registered as a zero-argument host function on the `time` namespace marker (`cel_builtins/time.rs`). `time.hour_utc` and `time.weekday` are specified in the wire pin but not yet wired — derive from `time.now()` or compare timestamps until Block E completes.

### 8.2 Example expressions

```cel
// Business-hours gate (UTC)
time.hour_utc >= 9 && time.hour_utc < 17 && time.weekday >= 1 && time.weekday <= 5

// Emergency break-glass before epoch (testing)
time.now < timestamp("2026-01-01T00:00:00Z")

// Short-lived deny during maintenance window
time.now >= timestamp("2026-06-01T02:00:00Z")
  && time.now < timestamp("2026-06-01T04:00:00Z")
```

### 8.3 Phase notes

`time.now` is impure. List-filter program class rejects it at compile time ([tools-list-filtering.md](tools-list-filtering.md)). Request and response decision policies may use it.

---

## 9. JSON traversal (`mcp.params`, `mcp.result`, `jsonpath.*`)

### 9.1 Map indexing

CEL treats `mcp.params` and `mcp.result` as dynamic maps decoded from JSON.

| Pattern | Example | Result type |
|---|---|---|
| Dot access (identifier keys) | `mcp.params.owner` | dynamic |
| Bracket access (any key) | `mcp.params["complex-key"]` | dynamic |
| Nested access | `mcp.params.repo.name` | dynamic |
| Presence | `has(mcp.params.token)` | bool |
| Default | `mcp.params.get("limit", 100)` | dynamic |

```cel
// Allow call when repo is in an approved list
mcp.params.repo in ["platform", "docs"]

// Bracket form for keys with dots or spaces
mcp.params["issue.title"] != ""

// Guard optional nested field
has(mcp.params.assignee) && mcp.params.assignee.email.endsWith("@acme.com")
```

### 9.2 `jsonpath.*` host functions

For deep or computed paths, use host functions (not in the roots table but referenced by Pin 8):

| Function | Signature | Side effect | Purpose |
|---|---|---|---|
| `jsonpath.get` | `(doc: map, path: string) -> any` | pure (read) | Read via JSONPath, e.g. `"$.items[0].id"` |
| `jsonpath.set` | `(doc: map, path: string, value: any) -> map` | pure (returns new map) | Response redaction / shaping |
| `jsonpath.delete` | `(doc: map, path: string) -> map` | pure | Remove sensitive branch |

```cel
// Read deeply nested param without long dot chains
jsonpath.get(mcp.params, "$.config.database.host") == "prod-db.internal"

// Redact token from result on response phase
jsonpath.set(mcp.result, "$.secrets.api_key", "[REDACTED]")

// Drop PII branch before client sees list output
jsonpath.delete(mcp.result, "$.tools[*].internal_metadata")
```

List-filter predicates may use `jsonpath.get` only; `set` and `delete` are forbidden in that program class.

---

## 10. Custom JWT claims (`jwt.<custom>`)

Any claim present in the validated JWT is exposed as `jwt.<claim_name>` without a namespace version bump. Use `has(jwt.<claim>)` before access when the claim is optional.

### 10.1 Dynamic access

```cel
has(jwt.cost_center) && jwt.cost_center == "eng-platform"

has(jwt.allowed_tools) && mcp.tool.name in jwt.allowed_tools
```

### 10.2 Declaring expected claim shape (**proposed**)

Bundle authors need a machine-readable contract so CI can reject rules that reference claims the issuer never mints. **Proposed:** a `claims:` schema block in the bundle manifest (YAML), parallel to JSON Schema for tool params:

```yaml
# proposed — not yet in bundle loader
claims:
  allowed_tools:
    type: array
    items: { type: string }
  cost_center:
    type: string
    enum: [eng-platform, eng-product, security]
```

The gateway would (proposed):

1. Validate manifest at bundle load.
2. Warn when a CEL program references `jwt.foo` without a `claims.foo` entry.
3. Coerce wire types (string vs list) before evaluation.

Until the manifest block lands, document expected claims in bundle README and enforce via STS issuance policy ([jwt-claim-schema.md](jwt-claim-schema.md)).

---

## 11. Host functions vs variables

These roots expose **functions only** — the root name binds to an internal namespace marker, not a user-readable map. Calling them as variables (`spicedb == "..."`) is a type error.

### 11.1 `spicedb.*`

| Function | Wire pin signature | Side effect | Notes |
|---|---|---|---|
| `spicedb.check` | `(subject, perm, resource) -> bool` | **effectful** (PDP network) | Single permission check |
| `spicedb.bulk_check` | `(subject, perm, [resources]) -> map<string,bool>` | **effectful** | Batch check; see [bulk-check-permission.md](bulk-check-permission.md) |

Typical tuple derivation for MCP tools:

```cel
spicedb.check(
  "user:" + jwt.sub,
  "invoke",
  jwt.tenant + "|" + mcp.tool.target + "|" + mcp.tool.name
)
```

**Wired stub:** `cel_builtins/spicedb.rs` registers `spicedb.check(resource, permission, subject)` today — argument order follows the implementation until Block E aligns with the wire pin. `bulk_check` is wire-pinned but not yet registered.

On PDP failure the gateway returns `-32107` `authz_unreachable` (fail-closed), not a CEL false.

### 11.2 `cache.*`

| Function | Signature | Side effect | Purpose |
|---|---|---|---|
| `cache.get` | `(key: string) -> any` | effectful (read) | Session-scoped memoization |
| `cache.set` | `(key: string, value: any, ttl: int) -> bool` | effectful (write) | Store ZedToken, schema hash, materialised attributes |

Keys must be tenant-prefixed by policy authors (`jwt.tenant + "/zed/" + request.session_id`). Pure list-filter programs must not call `cache.*`.

### 11.3 `audit.*`

| Function | Wire pin | Wired host ABI | Side effect |
|---|---|---|---|
| `audit.emit` | `(extra_fields: map) -> null` | `audit.emit(category: string, fields: map) -> null` | effectful (audit merge) |

Merged into the audit envelope `extra` map under `category`. Use for rule-firing metadata, not as a substitute for mandatory envelope fields ([reference-audit-envelope.md](reference-audit-envelope.md)).

```cel
audit.emit("policy", {"chain_depth": chain.depth(), "tool": mcp.tool.name})
```

### 11.4 `rate.*`

| Function | Wire pin | Wired host ABI | Side effect |
|---|---|---|---|
| `rate.acquire` | `(scope, key, budget, window) -> bool` | `(key, capacity, refill_per_sec) -> bool` | effectful (KV / local bucket) |

Returns `true` when a token is consumed, `false` when rate-limited. Idiomatic deny: `!rate.acquire(...) ? deny("rate limited")`. See [rate-limiting.md](rate-limiting.md) for token-bucket semantics and KV layout.

**Pure vs effectful summary**

| Root | Pure | Effectful |
|---|---|---|
| `spicedb.*` | — | all functions |
| `cache.*` | — | all functions |
| `audit.*` | — | all functions |
| `rate.*` | — | all functions |
| `jsonpath.get` | yes | — |
| `jsonpath.set`, `jsonpath.delete` | returns new value; mutates eval context when assigned | shaping |
| `chain.*`, `time.*` | `chain.*` pure; `time.now` impure | clock reads |

---

## 12. Sample rule library

Complete expressions illustrating common bundle patterns. Adapt resource ids and roles to your SpiceDB schema.

### 12.1 Allow `tools/call` when SpiceDB check passes

```cel
mcp.method == "tools/call"
  && spicedb.check(
       "user:" + jwt.sub,
       "invoke",
       jwt.tenant + "|" + mcp.tool.target + "|" + mcp.tool.name
     )
```

### 12.2 Deny if caller is not in `engineering` role

```cel
mcp.method == "tools/call"
  && !("engineering" in jwt.roles || "admin" in jwt.roles)
  ? deny("requires engineering or admin role")
```

### 12.3 Rate-limit per `(jwt.sub, tool)` to 30/min

Wire pin four-argument form:

```cel
mcp.method == "tools/call"
  && !rate.acquire(
       "tool",
       jwt.tenant + "/" + jwt.sub + "/" + mcp.tool.name,
       30,
       duration("1m")
     )
  ? deny("tool rate limited")
```

Wired token-bucket form ([rate-limiting.md](rate-limiting.md)):

```cel
mcp.method == "tools/call"
  && !rate.acquire(
       jwt.tenant + "/pair/" + jwt.sub + "/" + mcp.tool.name,
       30,
       0.5
     )
  ? deny("tool rate limited")
```

### 12.4 Emit audit extra field tagging chain depth

```cel
mcp.method == "tools/call"
  && audit.emit("delegation", {
       "chain_depth": chain.depth(),
       "originator": chain.originator().sub,
       "agent_id": jwt.agent_id
     })
  && true
```

### 12.5 Filter `tools/list` items via SpiceDB bulk check

Response-phase list shaping (conceptual — bulk materialisation preferred on hot path):

```cel
mcp.method == "tools/list"
  && spicedb.check(
       "user:" + jwt.sub,
       "invoke",
       jwt.tenant + "|" + mcp.tool.target + "|" + mcp.tool.name
     )
```

For large catalogs, precompute with `spicedb.bulk_check` in the gateway host and expose results via `actor.attributes` in list-filter class ([tools-list-filtering.md](tools-list-filtering.md), [bulk-check-permission.md](bulk-check-permission.md)).

### 12.6 Require purpose claim for high-risk tools

```cel
mcp.method == "tools/call"
  && mcp.tool.name in ["delete_repository", "rotate_secret"]
  && (!has(jwt.purpose) || jwt.purpose == "")
  ? deny("purpose claim required for destructive tools")
```

---

## 13. Type-error pitfalls

### 13.1 Common mistakes

| Mistake | Symptom | Fix |
|---|---|---|
| Treat `jwt.roles` as string | `no such overload: in` | Use `"role" in jwt.roles` (list membership) |
| Compare optional claim without `has()` | Runtime null dereference | `has(jwt.agent_id) && jwt.agent_id == "..."` |
| Use `mcp.result` on request phase | Empty map / wrong decision | Move rule to response phase or use `mcp.params` |
| Call `chain.originator().sub` on empty chain | Null access error | Guard with `chain.depth() > 0` |
| Pass list where string expected | `WrongType` from builtin | `string(jwt.sub)`, explicit constructors |
| Treat `jwt.act_chain` entry as string | Map accessor fails | Use `entry.sub`, `entry.agent_id` on maps |
| Use `spicedb` as variable | `WrongType: expected spicedb namespace receiver` | Call `spicedb.check(...)` only |
| Wrong `rate.acquire` arity | `expected N argument(s), got M` | Match wired signature (3-arg) or wire pin (4-arg) per deployment docs |

### 13.2 How errors surface

| Stage | Error class | Client / operator outcome |
|---|---|---|
| **Bundle load / compile** | CEL parse or type check | Bundle rejected; last good `policy_version` retained ([policy-dsl-choice.md](policy-dsl-choice.md)) |
| **Program class purity check** | Forbidden builtin in list filter | Compile fail with impure builtin name |
| **Evaluation — CEL type** | `ExecutionError` from interpreter | JSON-RPC `-32101` `policy_fault`, `data.tier = "cel"`, `data.error` message |
| **Evaluation — host builtin** | `CelBuiltinsError::WrongType` / `WrongArity` | Same `-32101` path; message includes builtin name |
| **Evaluation — host not implemented** | `cel builtin not implemented: spicedb.check` | `-32101` until Block E wires the builtin |
| **Evaluation — rule returned non-bool** | `PolicyError: must yield bool` | `-32101` |
| **Explicit deny** | Rule returned deny outcome | `-32100` `policy_deny` with `rule_fired` |

Example compile-time message (interpreter):

```text
ERROR: <input>:1:17: Undefined field 'role' in jwt.roles
```

Example runtime host error:

```text
rate.acquire: expected 3 argument(s), got 2
```

Runtime failures map to JSON-RPC `-32101` `policy_fault` with `data.tier = "cel"` and `data.error` carrying the interpreter or host message ([MCP gateway plan §6](../../MCP_GATEWAY_PLAN.md#6-gateway-emitted-json-rpc-error-codes)).

---

## 14. Cross-references

| Topic | Document |
|---|---|
| Why CEL over Expr, determinism, escape hatches | [policy-dsl-choice.md](policy-dsl-choice.md) |
| WIT host imports mirroring CEL builtins | [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) |
| `act_chain` schema, verification, CEL examples | [act-chain.md](act-chain.md) |
| Bulk PDP checks and ZedToken caching | [bulk-check-permission.md](bulk-check-permission.md) |
| List-filter program class and purity rules | [tools-list-filtering.md](tools-list-filtering.md) |
| Rate limit buckets and KV failure modes | [rate-limiting.md](rate-limiting.md) |
| JWT claim registry | [jwt-claim-schema.md](jwt-claim-schema.md) |
| Audit envelope fields merged with `audit.emit` | [reference-audit-envelope.md](reference-audit-envelope.md) |
| NATS subject grammar | [reference-subject-grammar.md](reference-subject-grammar.md) |
| Authoritative wire pin table | [MCP_GATEWAY_PLAN.md § Wire-Format Pin 8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace) |
| Identity layers ADR | [ADR 0002](../adr/0002-identity-layers.md) |

# Policy DSL choice (CEL)

**Status:** Block B paper artifact (2026-05-28). Irreversible decision record for the MCP gateway policy expression language.

**Document type:** Diátaxis **decision** (why CEL, what we rejected, when we would leave) + **reference** (evaluation contract, versioning, builtins, migration hooks).

**Related:** [MCP policy WIT host ABI sketch](mcp-policy-wit-sketch.md) · [Hierarchical policy merge](hierarchical-policy-merge.md) · [CEL-based `tools/list` filtering](tools-list-filtering.md) · [MCP gateway plan](../../MCP_GATEWAY_PLAN.md) Block B (DSL choice) · [MCP gateway plan § CEL variable namespace](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace) · [Identity overview](overview.md)

**Implementation today:** `cel-interpreter = 0.10.0` in `rsworkspace/Cargo.toml`; host builtins in `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/`; Phase 1 gate in `policy.rs`.

---

## Decision summary

| Question | Answer |
|---|---|
| Expression language | **CEL** (Common Expression Language, Google) |
| Rust evaluator crate | **`cel-interpreter`** (crates.io), pinned `=0.10.0` |
| Author audience | Platform engineers shipping gateway bundles, not application developers |
| Alternatives rejected | OPA/Rego, Cedar, custom DSL, Lua, Starlark (see §2) |
| Escape hatch | Blessed host builtins (`spicedb.check`, `jsonpath.*`, …) — not arbitrary Rust in policy |
| If CEL fails later | Recompile bundles; WIT host ABI shields JSON-RPC callers ([§7](#7-migration-path-if-we-leave-cel)) |

Block B in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) lists **DSL choice** beside **Host ABI surface** and **reply correlation**. This document closes the DSL item; host ABI is [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md).

---

## 1. Requirements

The gateway policy engine must satisfy constraints that survive Phase 2 (CEL hardening), Phase 3 (WASM bundles that embed the same DSL), and operator-facing bundle authoring. Requirements are **normative** for any future language reconsideration.

### 1.1 Sandboxed

Policy expressions must not grant:

- Host filesystem read/write
- Raw network sockets (TCP, HTTP, gRPC to SpiceDB from guest policy code)
- Process spawn or environment variable access
- Unbounded reflection into gateway process memory

All external effects flow through **capability-style host builtins** registered at evaluator setup time ([`cel_builtins::register`](../../rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/mod.rs)). Phase 3 WASM uses the same boundary via WIT `import host` ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)).

**Rationale:** The gateway is a trusted policy enforcement point (PEP). Tenant-authored policy is untrusted input. Sandboxing is non-negotiable before bundle hot-swap and third-party pack distribution (Block F).

### 1.2 Deterministic

For a fixed evaluation input snapshot (bindings + compiled program + host clock injection policy), repeated evaluation must yield the same boolean (or stable record/string where the program class allows non-bool).

| Allowed nondeterminism source | Mitigation |
|---|---|
| Wall clock | `time.now` is a host builtin; list-filter programs **reject** it at compile time ([tools-list-filtering.md §4.4](tools-list-filtering.md)) |
| SpiceDB replication lag | Gateway SpiceDB client uses **fully consistent** reads for authz paths (see `cel_builtins/spicedb.rs` module comment) |
| Cache races | Documented in [failure-mode-matrix.md](failure-mode-matrix.md); policy must not assume cross-request cache coherence without explicit keys |

**Rationale:** Audit replay, hierarchical merge compilation, and `tools/list` predicate caching all assume reproducible outcomes. Nondeterministic languages (unseeded `random()`, unconstrained `time.now` in pure programs) break those invariants.

### 1.3 Sub-millisecond evaluation (warm cache)

Hot-path targets (per request, after compile-once):

| Program class | Target | Notes |
|---|---|---|
| Ingress allow/deny predicate | P99 < 1 ms | Single bool eval on compiled `Program` |
| `tools/list` per-tool predicate | P99 < 1 ms x visible tools | Dominated by tool count; cache compiled programs per bundle revision |
| Risk scoring adjunct | Same process | Today partially native in `policy.rs`; future may move into CEL |

Cold path (bundle load, AST walk, first compile) may take milliseconds; that is acceptable on KV watch / hot-swap.

**Rationale:** The gateway sits on every JSON-RPC request. Interpreter startup cost must be paid once per bundle revision, not per NATS message.

### 1.4 Embeddable in Rust without a JIT

Constraints:

- Pure Rust dependency acceptable in `trogon-mcp-gateway` and future `trogon-policy-cel` crate
- No JVM, no Node embed, no mandatory `opa` sidecar
- No JIT requirement (rules out some WASM-only language runtimes as the **primary** Phase 2 path)

`cel-interpreter` interprets compiled programs with host function dispatch. That matches gateway deployment (static binary, musl-friendly).

### 1.5 First-class boolean, record, and list types

Policy rules are predominantly:

- Boolean guards: `mcp.method == "tools/call" && !tool.name.startsWith("delete_")`
- Record field access: `jwt.tenant`, `actor.attributes.role`
- List membership: `tool.name in actor.attributes.allowed_tools`

The DSL must type-check these without embedding a general-purpose language standard library (no arbitrary I/O, no package imports).

CEL's protobuf-aligned type system matches JSON-shaped MCP and JWT payloads.

### 1.6 Author audience: platform engineers

Authors are operators and identity engineers who:

- Ship YAML/JSON policy bundles to NATS KV
- Understand SpiceDB tuples, JWT claims, and MCP method names
- Do **not** need Turing-complete scripts for typical allow/deny

Implications:

- Prefer declarative expressions over procedural modules
- Fail closed on compile errors at bundle activation
- Document a **closed** builtin surface (§4, §6)
- Provide worked examples in sibling specs ([hierarchical-policy-merge.md](hierarchical-policy-merge.md), [tools-list-filtering.md](tools-list-filtering.md))

We explicitly do **not** optimize for arbitrary tenant developers writing policy as application code.

---

## 2. Candidates

Each alternative was evaluated against §1. **Chosen:** CEL. Others are rejected for the MCP gateway **primary** policy DSL unless a future ADR revisits this record.

### 2.1 CEL (Common Expression Language) — chosen

**What it is:** A small expression language designed for IAM, APIs, and Kubernetes admission. Syntax resembles C++/Java expressions; types include bool, int, uint, double, string, bytes, list, map, duration, timestamp.

**Pros:** Google ecosystem (IAM, K8s admission); `cel-interpreter` already in repo (Phase 1 `policy.rs`); sandbox via host functions only; agentgateway alignment; WASM desugar path in [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md); hierarchical merge compiles to one CEL program.

**Cons:** Pin `cel_version` (§5); no graph queries in-language; deep JSON needs `jsonpath.*`; less familiar than Rego in some ops teams.

**Why not rejected:** Best fit for §1 given code in tree and Block E/F plans.

### 2.2 OPA / Rego — rejected

**What it is:** Open Policy Agent with Rego, a Datalog-inspired declarative language over JSON documents.

**Pros:** Mature ecosystem; rich set operations; partial evaluation tooling.

**Cons:** Heavy runtime (sidecar or large WASM); no Trogon hot-path implementation; Rego rule blocks vs our CEL-shaped hybrid merge ([hierarchical-policy-merge.md](hierarchical-policy-merge.md)); per-tool list filtering likely exceeds warm-path budget.

**Why rejected:** Delays Block E; duplicates `cel-interpreter` work. OPA may still govern org-wide NATS provisioning outside the gateway.

### 2.3 Cedar (AWS) — rejected

**What it is:** AWS Cedar policy language with entity-oriented types and `permit`/`forbid` policies.

**Pros:** Strong static typing; formal verification story.

**Cons:** Few production Rust embed crates; awkward fit for dynamic JSON-RPC `params` and JWT bags; no Trogon or agentgateway precedent.

**Why rejected:** Higher integration risk vs CEL + SpiceDB for relationships.

### 2.4 Custom DSL — rejected

**What it is:** Trogon-specific YAML-to-AST language (e.g. only `allow_if` / `deny_if` keys).

**Pros:** Minimal surface; perfect bundle-schema alignment if narrowly designed.

**Cons:** Permanent lexer/parser/docs tax; every new operator needs a language release; no third-party ecosystem.

**Why rejected:** CEL is already an industry-maintained constrained DSL.

### 2.5 Lua / Starlark — rejected

**What they are:** General-purpose embedded scripting languages (Lua widely used; Starlark in Bazel).

**Pros:** Familiar syntax; mature Rust embed crates.

**Cons:** Hard to prove sandbox (especially Lua); weaker determinism and JSON type alignment than CEL.

**Why rejected:** PEP needs bounded expressions, not general-purpose scripts.

### 2.6 Expr (Google) — noted, not primary

[MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Open Questions mention **Expr** (Go-native, Synadia Protect). Expr is syntactically similar to CEL.

| Aspect | Decision |
|---|---|
| Status | **Not chosen** as primary DSL; CEL already integrated |
| Relationship | If `cel-interpreter` stalls, Expr-to-CEL transpile or shared AST **(proposed future)** is easier than adopting Rego |

No Trogon crate vendors Expr today; do not document Expr builtins as supported.

### 2.7 Candidate decision log

| Candidate | Decision | Primary rejection reason |
|---|---|---|
| CEL | **Adopt** | Meets §1; code in tree; agentgateway alignment |
| OPA/Rego | Reject | Embed weight + no implementation |
| Cedar | Reject | Immature Rust path + MCP JSON friction |
| Custom DSL | Reject | Permanent language tax |
| Lua / Starlark | Reject | Sandbox and determinism risk |
| Expr | Defer | Protect uses it; we already committed CEL in Phase 1 |

---

## 3. Recommendation (CEL) — justified

### 3.1 Alignment with irreversible downstream work

| Downstream item | How CEL supports it |
|---|---|
| Compile-to-bytecode caching | `cel_interpreter::Program::compile` once per bundle rule; reuse `execute` ([`SpicedbGatePolicy`](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs)) |
| Phase 3 WASM bundles | Guest evaluates same expressions or calls WIT imports mirroring CEL builtins ([mcp-policy-wit-sketch.md § Mapping to CEL builtins](mcp-policy-wit-sketch.md)) |
| Public policy authoring | Google CEL spec + Trogon reference tables (§4) |
| Hierarchical merge | Compile each tier to CEL; merge to single `Program` ([hierarchical-policy-merge.md](hierarchical-policy-merge.md)) |
| `tools/list` filtering | Per-tool bool predicates ([tools-list-filtering.md](tools-list-filtering.md)) |

Changing DSL after Block E would invalidate bundle corpuses and operator training. CEL is the correct lock **now** because Phase 1 already executes CEL.

### 3.2 Division of responsibility: CEL vs SpiceDB

| Concern | Layer |
|---|---|
| JWT claim matching, method gates, string prefixes | CEL |
| Relationship graph, zed tokens, bulk checks | `spicedb.check` / `spicedb.bulk_check` host builtins → gateway SpiceDB client |
| Rate limits, session counters | `rate.acquire`, native `throttle` ([rate-limiting.md](rate-limiting.md)) |
| Risk scoring (today) | Native `policy.rs`; may migrate into CEL later |

CEL is the **glue** language; SpiceDB is the **relationship** engine. Do not encode graph traversal in CEL.

### 3.3 Eval library choice

| Option | Status |
|---|---|
| `cel-interpreter` on crates.io | **Selected**, workspace pin `=0.10.0` |
| `cel-rust` upstream fork | **Not selected** unless upstream gap blocks Block E |
| Protect Expr | **Out of scope** for Trogon gateway crate |

Pin rationale: reproducible builds across `trogon-mcp-gateway` and future `trogon-policy-cel`; bundle `cel_version` must match workspace pin (§5).

### 3.4 Risk acceptance

| Risk | Mitigation |
|---|---|
| `cel-interpreter` maintenance | Pin version; fork only if security/bug critical |
| Language limit (no graphs) | Blessed builtins + SpiceDB |
| Plan/doc drift (`rate.acquire` arity) | Spec follows `cel_builtins/rate.rs` (3-arg token bucket) per [rate-limiting.md](rate-limiting.md) |

---

## 4. Compatibility plan (surface contract)

The gateway vendors a **closed** host surface under `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/`. Policy authors depend on this contract; Rust internals may change if semantics stay stable.

### 4.1 Evaluation inputs

Bindings are injected into `cel_interpreter::Context` before `Program::execute`. Names below are **normative for bundle authoring**. Implementation may also expose roots from [MCP_GATEWAY_PLAN.md §8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace) (`mcp.*`, `jwt.*`, `nats.*`, …) as those land in code.

#### Top-level roots (authoring model)

| Root | Type | Description |
|---|---|---|
| `request` | map | JSON-RPC + transport context: `id`, `method`, `params`, `session_id`, `deadline` **(proposed fields where not yet wired)** |
| `actor` | map | Caller identity: `sub`, `tenant`, `agent_id`, `wkl`, `purpose`, `scope`, `act_chain`, `attributes` |
| `subject` | string | SpiceDB subject string for this evaluation (e.g. `user:{sub}`, `agent:{tenant}/{id}`) |
| `attributes` | map | Auxiliary bag: `trace_id`, `nats.subject`, `direction`, `method_root`, deployment labels |

**`request`:** `id`, `method`, `params`, `session_id` **(proposed)** from JSON-RPC and session KV.

**`actor`:** `sub`, `tenant`, `agent_id`, `attributes` (registry + claims per [tools-list-filtering.md §3.4](tools-list-filtering.md)), `act_chain`. Phase 1 binds `jwt` / `chain` in [`policy.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs); **(proposed)** desugar `actor.*` → `jwt.*` until rename lands.

**`subject`:** SpiceDB subject string; aligns with WIT `subject-id` ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)). **`attributes`:** `trace_id`, `nats.subject`, `direction`, etc., merged into audit `extra`.

**List-only overlays:** `tool`, `server` per [tools-list-filtering.md §3.3](tools-list-filtering.md).

### 4.2 Evaluation outputs

| Program class | Expected result | On wrong type |
|---|---|---|
| Ingress gate / allow-deny | `bool` | `PolicyEvalError` / deny **(proposed)** |
| Audit enrichment expr | `map` or `string` | Reject at compile time **(proposed)** |
| Policy id metadata | Host-side only | Not a CEL return value |

Phase 1 `requires_spicedb_for_method` requires `bool` ([`policy.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs)).

### 4.3 Host builtins (vendored `cel_builtins/`)

Registration: [`cel_builtins::register`](../../rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/mod.rs).

Namespace variables are marker strings (`__trogon.cel_builtins.{name}`); calls use overloaded names `check`, `get`, `set`, `delete`, `emit`, `now`, `acquire`.

| Namespace | Function | Arity | Stable name | Status |
|---|---|---|---|---|
| `spicedb` | `check` | 3 | `spicedb.check` | Stub `NotImplemented` |
| `cache` | `get` | 1 | `cache.get` | Stub |
| `cache` | `set` | 3 | `cache.set` | Stub |
| `jsonpath` | `get` | 2 | `jsonpath.get` | Stub |
| `jsonpath` | `set` | 3 | `jsonpath.set` | Stub |
| `jsonpath` | `delete` | 2 | `jsonpath.delete` | Stub |
| `audit` | `emit` | 2 | `audit.emit` | Stub |
| `time` | `now` | 0 | `time.now` | Stub |
| `rate` | `acquire` | 3 | `rate.acquire` | Stub |

**Argument order (normative for new code):**

```cel
spicedb.check(resource, permission, subject)   // bool
cache.get(key)                                  // dyn
cache.set(key, value, ttl_secs)                 // bool
jsonpath.get(document, path)                    // dyn
jsonpath.set(document, path, value)             // dyn
jsonpath.delete(document, path)                 // dyn
audit.emit(category, fields_map)                // bool
time.now()                                      // timestamp
rate.acquire(key, capacity, refill_per_sec)     // bool
```

WIT mapping: [mcp-policy-wit-sketch.md § Mapping to CEL builtins](mcp-policy-wit-sketch.md).

### 4.4 Standard library (CEL, no host import)

Available without Trogon namespaces: boolean logic, comparisons, `size`, `startsWith`, `endsWith`, `contains`, `matches`, list/map literals, comprehensions enabled by interpreter feature flags.

**tools_list_filter** program class restricts to pure stdlib + `jsonpath.get` only ([tools-list-filtering.md §4.4](tools-list-filtering.md)).

### 4.5 Plan namespace vs implementation

[MCP_GATEWAY_PLAN.md §8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace) documents `mcp.*`, `jwt.*`, `chain.*`, `nats.*`, `spicedb.check(subject, perm, resource)` ordering. Implementation builtins use `(resource, permission, subject)` in `spicedb.rs` — authors should follow **this document and `cel_builtins/`**; plan table update is editorial.

`chain.contains`, `chain.depth`, `chain.originator` are registered on the `chain` value in [`policy.rs`](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs) (not separate roots).

### 4.6 Contract stability rules

| Change type | Bundle impact |
|---|---|
| Add builtin | Minor gateway release; document in changelog |
| Add optional `request` field | Non-breaking |
| Rename builtin | Major bundle `policy_bundle_version` bump |
| Change arity | Reject compile at load; fail closed on hot-swap |

---

## 5. Versioning

### 5.1 Author declares `cel_version`

Bundle manifest fragment **(proposed schema until Phase 3 bundle loader lands):**

```yaml
policy_bundle_version: "2026-05-28T12:00:00Z"
cel_version: "0.10"          # language + stdlib pin authors target
wit_version: "0.1.0"         # Phase 3 only; see mcp-policy-wit-sketch.md
programs:
  - id: tenant/acme-ingress
    class: ingress_gate
    expr: |
      mcp.method == "tools/call" || mcp.method == "resources/read"
```

| Field | Meaning |
|---|---|
| `cel_version` | Major.minor of CEL language + stdlib authors wrote against (`0.10` today) |
| `policy_bundle_version` | Opaque revision for KV cache invalidation ([tools-list-filtering.md](tools-list-filtering.md)) |
| `wit_version` | Independent; only when WASM components present |

Authors must set `cel_version` to the value in [rsworkspace/Cargo.toml](../../rsworkspace/Cargo.toml) `cel-interpreter` pin (currently `0.10.0` → declare `"0.10"`).

### 5.2 Gateway evaluator selection

| Step | Behavior |
|---|---|
| Load bundle | Read `cel_version` |
| Match | If `cel_version` major.minor equals gateway supported set, compile with pinned `cel-interpreter` |
| Mismatch | Fail activation; keep previous bundle revision (fail closed) |
| Multi-rule bundle | All programs in one bundle share one `cel_version` **(proposed)** |

Gateway maintains an internal table **(proposed):**

| Supported `cel_version` | Workspace pin | Notes |
|---|---|---|
| `0.10` | `cel-interpreter =0.10.0` | Current |

### 5.3 Multiple CEL minor versions in the fleet

| Scenario | Policy |
|---|---|
| Bundle `0.10`, gateway `0.10` | Allowed |
| Bundle `0.11`, gateway `0.10` | Reject at load until gateway upgraded |
| Gateway supports `0.10` and `0.11` concurrently **(proposed)** | Two interpreter pins in workspace during migration window only |
| Mixed bundles in one tenant KV | Each bundle carries its own `cel_version`; gateway picks evaluator per bundle metadata |

Operational rule: **never** hot-swap gateway binary without checking KV bundles still target a supported `cel_version`.

### 5.4 Compiled program cache key

**Proposed** cache key components:

```
(tenant, policy_bundle_version, program_id, cel_version, expr_hash)
```

Bump `policy_bundle_version` when any `expr` changes; bump `cel_version` when gateway adopts new interpreter.

---

## 6. Escape hatches

When CEL cannot express a policy cleanly (deep JSON mutation, graph reachability, cluster-wide rate state), authors **must not** embed Rust or call the network from CEL. They delegate to a **blessed host builtin** whose implementation hides complexity and enforces caps.

### 6.1 When CEL is insufficient

| Symptom | Wrong approach | Right approach |
|---|---|---|
| Traverse authz graph | Nested loops in CEL | `spicedb.check` or `spicedb.bulk_check` **(bulk proposed in plan §8)** |
| Deep optional JSON in `params` | 20-line field guards | `jsonpath.get` / `jsonpath.set` for redaction paths |
| Session-scoped memoization | Global CEL variable | `cache.get` / `cache.set` with tenant-prefixed keys |
| Time windows | Manual epoch math | `time.now()` + compare to duration literals |
| Cluster rate limit | Per-process counter in CEL | `rate.acquire` → JetStream KV ([rate-limiting.md](rate-limiting.md)) |
| Audit forensics | String concat in deny message | `audit.emit` with structured `fields` map |

### 6.2 Blessed builtins for delegation

| Builtin | Blessed for | Not blessed for |
|---|---|---|
| `spicedb.check` | Tuple-style allow/deny, resource-permission checks | Bulk catalog filtering ([tools-list-filtering.md](tools-list-filtering.md) forbids in list class) |
| `spicedb.bulk_check` | **(proposed)** Many resources in one call | List filter hot loop |
| `jsonpath.get` | Read-only schema/params inspection | — |
| `jsonpath.set` / `jsonpath.delete` | Response redaction, in-place shaping | `tools_list_filter` class |
| `cache.get` / `cache.set` | ZedToken cache, schema cache, materialised attributes | Pure list predicates |
| `audit.emit` | Operator-facing rule firing metadata | Replacing mandatory audit envelope |
| `time.now` | Business-hours rules on ingress | `tools_list_filter` class |
| `rate.acquire` | Token bucket / cluster budget | Synchronous spin-wait loops in CEL |

### 6.3 Adding a new blessed builtin

Requirements **(proposed process):**

1. Document in this file and [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) (WIT import in next minor if WASM-visible).
2. Implement in `cel_builtins/` with `CelBuiltinsError` typed errors (no raw strings across module boundary per crate AGENTS.md).
3. Classify pure vs impure; update `tools_list_filter` denylist if impure.
4. No new NATS subjects, KV bucket names, or claim names without explicit **proposed** label and justification elsewhere.

### 6.4 Anti-patterns

| Anti-pattern | Why blocked |
|---|---|
| `cache.set` in list filter | Breaks determinism and cache keys |
| Repeated `spicedb.check` per list item | Latency explosion; materialise `actor.attributes` once |
| Custom string encoding of graphs in CEL | Fragile; use SpiceDB |
| Requesting `kv.fetch` / `nats.request` in CEL | Excluded from WIT v0.1.0 trust boundary ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)) |

---

## 7. Migration path if we leave CEL

Leaving CEL is **unlikely** but planned for because Block B decisions are irreversible on paper, not in physics.

### 7.1 What callers see

| Caller | Shield |
|---|---|
| MCP clients | JSON-RPC errors (`POLICY_DENY`, `APPROVAL_REQUIRED`) unchanged |
| Operators | Bundle manifest `programs[].expr` changes; `policy_bundle_version` bumps |
| Audit consumers | Envelope schema stable; `rules_fired` may cite new language id **(proposed)** |

The [WIT host ABI](mcp-policy-wit-sketch.md) shields Phase 3 guests: policy semantics move inside `evaluate` or host imports, not client-visible method names.

### 7.2 Bundle recompilation

| Asset | Migration action |
|---|---|
| CEL `expr` strings | Transpile or rewrite to successor DSL |
| `cel_version` field | Replace with `policy_language_version` **(proposed)** |
| WASM components | Rebuild against new WIT if guest logic embedded CEL runtime |
| KV keys | Unchanged if bundle ids stable; content hash changes |

Tooling **(proposed):** `trogon-gateway-ctl` dry-run fixture traffic validates semantic equivalence before hot-swap.

### 7.3 Semantic preservation checklist

| Invariant | Verification |
|---|---|
| Fail closed on compile error | Load-time reject |
| `tools/list` pure predicates | No impure builtins in list class |
| Hierarchical merge semantics | Same deny/allow OR-tree ([hierarchical-policy-merge.md](hierarchical-policy-merge.md)) |
| Host builtin semantics | Golden tests per builtin |

### 7.4 Dual-run and successor languages **(proposed)**

A successor (Rego, Cedar, …) would add a parallel evaluator crate while keeping WIT import names stable; SpiceDB/JWT unchanged. During migration the gateway may load mixed `cel_version` and `policy_language_version` bundles, but the merge compiler must not combine languages in one `Program`.

---

## 8. Cross-references

| Document | Relevance |
|---|---|
| [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) | Host ABI for WASM; CEL builtin ↔ WIT import table |
| [hierarchical-policy-merge.md](hierarchical-policy-merge.md) | All tiers compile to CEL; merge semantics |
| [tools-list-filtering.md](tools-list-filtering.md) | `tools_list_filter` program class; pure builtin rules |
| [rate-limiting.md](rate-limiting.md) | `rate.acquire` contract |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Fail-closed defaults |
| [bootstrap-day-zero.md](bootstrap-day-zero.md) | Empty bundle behavior |

### 8.1 Code map

| Path | Role |
|---|---|
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` | Host builtin implementations |
| `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` | Phase 1 CEL gate + JWT/`chain` context |
| `rsworkspace/Cargo.toml` | `cel-interpreter` workspace pin |

### 8.2 Block B closure

When this document merges, [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block B item **DSL choice** should reference `docs/identity/policy-dsl-choice.md` as the signed decision. Eval library item resolves to **`cel-interpreter` pinned 0.10.x**, not an Expr fork, unless a future ADR amends.

### 8.3 Program classes and examples (reference)

| Class | Output | Builtin set |
|---|---|---|
| `ingress_gate` | bool | Full §4.3 (tier may restrict) |
| `tools_list_filter` | bool | stdlib + `jsonpath.get` only ([tools-list-filtering.md §4.4](tools-list-filtering.md)) |
| `risk_adjunct` | bool or map **(proposed)** | `time.now`, `cache.*`; no `audit.emit` on hot path |
| `audit_enrichment` | map **(proposed)** | `audit.emit` allowed |

Phase 1 gate (shipped): `mcp.method == "tools/call" || mcp.method == "resources/read"`. Future deny overlay example: `!tool.name.startsWith("delete_") && spicedb.check(resource, permission, subject)` with normalised `tool.name` binding **(Block E)**.

### 8.4 Security and open items

- Bind validated JWT claims only; never raw JWT strings ([mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md)).
- Treat `actor.attributes` as untrusted until registry-sourced.
- Builtins are `NotImplemented` until Block E; hot-swap compile failures keep the previous bundle.
- **(proposed):** `jwt` → `actor` alias; cache `Program` handles before bytecode API; `spicedb.bulk_check` when bulk path ships.

---

## Appendix — Document metrics

Target length: 400–600 lines. Identifiers marked **proposed** are not verified in production yet.

| Metric | Value |
|---|---|
| Decision date | 2026-05-28 |
| CEL evaluator pin | `cel-interpreter =0.10.0` |
| Primary crate | `trogon-mcp-gateway` |

---

*End of decision record.*

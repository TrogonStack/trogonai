# ADR 0008: Policy DSL Choice (CEL)

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Policy DSL choice; unblocks CEL builtins, WASM policy bundles, hierarchical merge, and `tools/list` filtering |
| **Related** | `docs/identity/policy-dsl-choice.md`, `docs/identity/reference-cel-variables.md`, `docs/identity/reference-host-abi.md`, `docs/identity/mcp-policy-wit-sketch.md`, `docs/identity/reference-cel-variables.md` (CEL variable namespace) |

## Context

The MCP gateway policy engine sits on every JSON-RPC request. Tier 1 policy is native Rust (method gates, risk scoring hooks). **Tier 2** is tenant-authored policy shipped in NATS KV bundles — the expression language for that tier must be chosen before Block E (host builtins), Block F (WASM bundles), hierarchical merge compilation, and operator bundle authoring can proceed without rework.

Design context is captured in [`docs/identity/policy-dsl-choice.md`](../identity/policy-dsl-choice.md). That spec evaluated sandboxing, determinism, sub-millisecond warm-path evaluation, Rust embeddability without a JIT, and boolean/record/list typing against MCP and JWT payloads. Phase 1 already executes CEL in `trogon-mcp-gateway` (`policy.rs`, `cel_builtins/`).

**Prior art and ecosystem:**

| Signal | Implication |
|--------|-------------|
| **agentgateway** | CEL validated on the proxy hot path; Trogon inherits the same language family for gateway policy |
| **Google CEL** | IAM, Kubernetes admission, and `cel.dev` tooling; typed evaluation over JSON-shaped inputs |
| **Synadia Protect** | Uses **Expr** (Go-native, syntactically similar to CEL) — not portable to our Rust gateway without a shim |
| **Phase 1 branch** | `cel-interpreter` pinned in workspace; ingress gate compiles and executes CEL today |

**What stays undecided if this ADR is not accepted:**

- Bundle manifest fields (`cel_version`, program classes) fork across implementers.
- Phase 3 WASM host imports cannot be pinned; CEL builtins and WIT imports diverge.
- Hierarchical merge and `tools/list` filtering cannot assume a single compile target.
- Block B Host ABI surface (item 2) lacks a stable guest language to desugar from.
- Operator training and third-party pack distribution (Block F) ship without a canonical authoring surface.

**Author audience:** Platform engineers and identity operators who ship YAML/JSON bundles — not arbitrary tenant application developers. The DSL must be learnable quickly from public CEL documentation plus Trogon reference tables; bundles encode policy semantics in a language SDK implementers already encounter in cloud-native stacks.

**Requirements the chosen DSL must satisfy (normative):**

1. **Sandboxed** — no filesystem, raw network, or process spawn from policy code; external effects only through host builtins ([`reference-host-abi.md`](../identity/reference-host-abi.md)).
2. **Deterministic** — fixed input snapshot yields stable bool (or stable record where allowed); impure builtins (`time.now`, `spicedb.check`, `rate.acquire`) are explicit and classifiable.
3. **Warm-path budget** — P99 &lt; 1 ms per compiled program eval after compile-once per bundle revision.
4. **Rust-embeddable** — pure Rust dependency, static binary friendly, no JVM/Node/OPA sidecar on the hot path.
5. **JSON-aligned types** — bool, string, list, map over JWT claims and JSON-RPC `params`/`result`.

## Decision

**CEL (Common Expression Language) is the Tier 2 policy DSL. Eval library: `cel-rust` upstream (no fork). CEL becomes the canonical surface for all gateway-emitted bundles in Phase 1+.**

Concretely:

| Item | Choice |
|------|--------|
| Expression language | CEL (Google Common Expression Language) |
| Rust evaluator | **`cel-rust` upstream** — consume published crates from the upstream project; do **not** maintain a Trogon fork unless a future ADR documents a security-critical upstream gap |
| Workspace dependency today | `cel-interpreter = 0.10.0` (crates.io artifact from the `cel-rust` lineage) |
| Bundle authoring surface | CEL `expr` strings in policy bundle manifests; `cel_version` major.minor pins language/stdlib expectations |
| Phase scope | Canonical for gateway-emitted bundles Phase 1 (interpreted CEL) through Phase 3 (WASM guests that embed or desugar the same expressions) |
| Division of labor | CEL for claim matching, method gates, string/list logic; SpiceDB graph checks via `spicedb.check` / `spicedb.bulk_check` host builtins — not in-language graph traversal |

Host extensions (SpiceDB, cache, rate limits, audit, jsonpath) are **blessed builtins** registered at evaluator setup, not arbitrary Rust in policy. Phase 3 WASM bundles call the same semantics through WIT imports ([`mcp-policy-wit-sketch.md`](../identity/mcp-policy-wit-sketch.md)).

## Consequences

### Positive

- **Typed evaluation** — CEL type-checks bool guards, record field access, and list membership against protobuf-aligned types that map cleanly to JWT and JSON-RPC payloads.
- **Ecosystem** — Google IAM and Kubernetes admission precedents; `cel.dev` spec and community tooling; operators can reuse existing CEL literacy.
- **agentgateway prior art** — Proxy path already validated CEL for similar hot-path constraints; reduces unknowns for Trogon gateway PEP design.
- **Code in tree** — Phase 1 `policy.rs` gate and `cel_builtins/` scaffold already assume CEL; choosing another language would invalidate shipped paths.
- **Downstream compile targets** — Hierarchical merge compiles each tier to CEL and merges to one `Program`; `tools/list` filtering uses per-tool bool predicates in the same language ([`tools-list-filtering.md`](../identity/tools-list-filtering.md)).
- **`cel.dev` tooling** — Bundle authors can dry-run expressions against fixture bindings; future `trogon-gateway-ctl validate` can wrap upstream compile APIs.
- **Stable host ABI path** — Block E implements builtins in Rust; Block F maps them to WIT without changing MCP client-visible behavior.

### Negative

- **WASM tier coupling (Phase 3)** — WASM policy bundles must call host functions with **identical semantics** to CEL builtins. The WIT interface must **shadow CEL builtin signatures** (argument order, return types, error mapping). Guest code cannot invent parallel import names; [`reference-host-abi.md`](../identity/reference-host-abi.md) is the single lookup, with WIT kebab-case mirrors in [`mcp-policy-wit-sketch.md`](../identity/mcp-policy-wit-sketch.md).
- **No in-language graph queries** — Relationship traversal stays in SpiceDB via host builtins; deep JSON paths need `jsonpath.*` rather than arbitrary reflection.
- **Version pin discipline** — Bundle `cel_version` must match gateway-supported interpreter pin; mismatch fails bundle activation (fail closed).
- **Upstream dependency risk** — Security or correctness bugs in `cel-rust` require upstream fix or emergency pin bump; fork is explicitly out of scope unless amended.
- **Operator familiarity gap** — Teams steeped in OPA/Rego must learn CEL; mitigated by constrained surface and worked examples in sibling specs.

### Neutral

- **Expr remains available for ad-hoc policies** — Synadia Protect and other Go stacks may continue to use Expr locally; it is **not** the canonical Phase 1+ bundle surface for Trogon gateway-emitted packs. A future Expr-to-CEL transpile for migration is easier than adopting Expr as primary (similar syntax, different ecosystem).
- **Tier 1 native policy unchanged** — Method allowlists, throttle counters, and risk hooks may remain native Rust; migration into CEL is optional per program class.
- **OPA/Rego and Cedar** — May still govern org-wide NATS provisioning or external control planes; rejected as **primary** gateway DSL ([`policy-dsl-choice.md`](../identity/policy-dsl-choice.md) §2).

## Alternatives considered

### Expr (Google — Synadia Protect)

| Aspect | Assessment |
|--------|------------|
| Syntax | Superficially similar to CEL; attractive if Protect bundles were portable |
| Ecosystem | **Go-native** — Protect embeds Expr in Go services; no production Rust embed path in Trogon today |
| Integration cost | Would require a Go shim, FFI boundary, or full reimplementation — violates Rust-embeddable requirement |
| Precedent | Useful reference for Protect operators; not Trogon gateway canonical surface |

**Rejected:** Go-only ecosystem; would require shim to embed in `trogon-mcp-gateway` and future `trogon-policy-cel` extraction. CEL is already integrated in Phase 1.

### Homegrown Trogon DSL

| Aspect | Assessment |
|--------|------------|
| Surface | YAML `allow_if` / `deny_if` keys mapped to a minimal AST |
| Pros | Perfect alignment with bundle schema if narrowly scoped |
| Cons | Permanent lexer/parser/docs tax; every new operator needs a language release; zero third-party ecosystem |

**Rejected:** Not a strategic differentiator. CEL is an industry-maintained constrained DSL; maintaining a bespoke language duplicates Google’s investment without improving sandbox or determinism guarantees.

### OPA / Rego

**Rejected:** Heavy runtime (sidecar or large WASM); no Trogon hot-path implementation; Rego rule blocks do not align with CEL-shaped hierarchical merge compilation. May remain outside the gateway for infrastructure policy.

### Cedar (AWS)

**Rejected:** Immature Rust embed story; awkward fit for dynamic JSON-RPC `params` and JWT claim bags; no agentgateway or Trogon precedent.

### Lua / Starlark

**Rejected:** Sandbox and determinism risk versus bounded PEP expressions; weaker JSON type alignment than CEL.

### `cel-rust` fork (`cel-fork`, `celx`, Trogon-specific)

**Rejected for Phase 1+:** [`MCP_GATEWAY_PLAN.md`](../../MCP_GATEWAY_PLAN.md) notes proxy forks extended CEL with proxy-specific builtins. Trogon adds host functions **externally** via `cel_builtins/` and WIT imports — no fork required. Fork only reconsidered if upstream blocks Block E with no timely fix.

## Implementation notes

### Eval library and workspace pin

| Layer | Action |
|-------|--------|
| Dependency | Depend on **`cel-rust` upstream** crates (no Trogon fork); workspace pin `cel-interpreter = 0.10.0` in `rsworkspace/Cargo.toml` |
| Bundle field | Authors declare `cel_version: "0.10"` matching gateway-supported major.minor |
| Activation | Gateway rejects bundles whose `cel_version` is unsupported; keeps previous revision (fail closed) |
| Cache key | `(tenant, policy_bundle_version, program_id, cel_version, expr_hash)` for compiled `Program` reuse |

Host functions are registered at evaluator setup via `cel_builtins::register` — not by patching the interpreter.

### Host ABI

All external effects flow through the closed builtin surface documented in [`reference-host-abi.md`](../identity/reference-host-abi.md):

| Namespace | Examples | Side-effect class |
|-----------|----------|-------------------|
| `spicedb.*` | `check`, `bulk_check` | NATS-io / gRPC to PDP |
| `cache.*` | `get`, `set` | In-process memoization |
| `rate.*` | `acquire` | KV-io or cache (scope-dependent) |
| `audit.*` | `emit` | Audit envelope merge |
| `jsonpath.*` | `query`, `extract` | Pure (document inspection) |
| `time.*` | `now`, `hour_utc`, `weekday` | Pure at eval time (non-deterministic across replays) |

Implementation target: `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` (Block E). Stubs may return `NotImplemented` until wired; signatures in the reference doc are normative for new code.

WIT mapping for Phase 3: each CEL builtin maps to a kebab-case import (`spicedb-check`, `cache-get`, …) with matching arity and fail-closed error codes ([`mcp-policy-wit-sketch.md`](../identity/mcp-policy-wit-sketch.md)).

### Variable namespace (Wire-Format Pin 8)

CEL evaluation bindings follow [`reference-cel-variables.md`](../identity/reference-cel-variables.md) and [`MCP_GATEWAY_PLAN.md` Wire-Format Pin 8](../../MCP_GATEWAY_PLAN.md#8-cel-variable-namespace):

| Root | Purpose |
|------|---------|
| `mcp.*` | Protocol surface: `method`, `tool.name`, `params`, `result` (response phase) |
| `jwt.*` | Validated claims: `sub`, `tenant`, `agent_id`, `wkl`, `act_chain`, custom claims |
| `chain.*` | Functions on `jwt.act_chain`: `contains`, `originator`, `depth` |
| `nats.*` | Transport: `subject`, `headers`, `account` |
| `request.*` / `response.*` | Correlation, deadline, session, list-filter index |
| `time.*` | Host-evaluated clock helpers |

Phase 1 binds `jwt` and `chain` in `policy.rs`; full Pin 8 roots land incrementally without renaming existing variables (additive only).

### Program classes

| Class | CEL output | Builtin restrictions |
|-------|------------|----------------------|
| `ingress_gate` | `bool` | Full host surface (tier may restrict) |
| `tools_list_filter` | `bool` | Stdlib + `jsonpath.*` pure paths only — no `spicedb.check`, `time.now`, or `cache.*` in list hot loop |
| `risk_adjunct` | `bool` or `map` | May use `time.now`, `cache.*`; no `audit.emit` on hot path |
| `audit_enrichment` | `map` | `audit.emit` allowed |

Ingress example (Phase 1 shipped pattern):

```cel
mcp.method == "tools/call" || mcp.method == "resources/read"
```

Future deny overlay (Block E):

```cel
mcp.method == "tools/call"
  && !mcp.tool.name.startsWith("delete_")
  && spicedb.check(subject, permission, resource)
```

### Bundle manifest fragment (illustrative)

```yaml
policy_bundle_version: "2026-05-28T12:00:00Z"
cel_version: "0.10"
programs:
  - id: tenant/acme-ingress
    class: ingress_gate
    expr: |
      mcp.method == "tools/call" || mcp.method == "resources/read"
```

`wit_version` is independent and applies only when WASM components are present (Phase 3).

### Code map

| Path | Role |
|------|------|
| `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` | Phase 1 CEL gate, JWT/`chain` context binding |
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` | Host builtin implementations |
| `rsworkspace/Cargo.toml` | `cel-interpreter` workspace pin |
| Future `trogon-policy-cel` crate | Block F engine extraction; same upstream dependency |

### Security constraints

- Bind **validated** JWT claims only; never raw JWT strings in CEL.
- Treat `jwt`-derived attribute bags as untrusted until registry-sourced.
- Compile errors at bundle load fail closed; hot-swap keeps previous revision.
- New builtins require reference doc update, WIT sketch update if WASM-visible, and program-class purity classification.

## Status of supporting work

| Item | Status |
|------|--------|
| ADR 0008 (this document) | **Accepted** — closes Block B DSL choice |
| [`policy-dsl-choice.md`](../identity/policy-dsl-choice.md) | Design context and candidate analysis — **complete** (paper artifact) |
| Phase 1 CEL ingress gate | **Shipped** in `policy.rs` |
| `cel_builtins/` host surface | **Scaffolded** — many builtins stub `NotImplemented` until Block E |
| [`reference-cel-variables.md`](../identity/reference-cel-variables.md) | **Phase 1 contract** — Pin 8 roots documented |
| [`reference-host-abi.md`](../identity/reference-host-abi.md) | **Phase 1 contract** — signatures pinned for CEL and WIT |
| [`mcp-policy-wit-sketch.md`](../identity/mcp-policy-wit-sketch.md) | **Block B paper** — WIT world sketch for Phase 3 |
| Hierarchical merge → single CEL `Program` | **Pending** — spec in [`hierarchical-policy-merge.md`](../identity/hierarchical-policy-merge.md) |
| `tools/list` predicate filtering | **Pending** — spec in [`tools-list-filtering.md`](../identity/tools-list-filtering.md) |
| Block E — wire all builtins | **Pending** — unblocked by this ADR |
| Block F — WASM bundle loader + Wasmtime | **Pending** — depends on host ABI + this DSL choice |
| `trogon-gateway-ctl bundle validate` | **Pending** — proposed dry-run against pinned `cel_version` |
| `MCP_GATEWAY_PLAN.md` Block B checkbox | **Pending editorial** — reference this ADR and mark DSL choice done |

### Migration if CEL is ever replaced

Unlikely but planned: MCP clients see stable JSON-RPC errors (`POLICY_DENY`, `APPROVAL_REQUIRED`); WIT host ABI shields Phase 3 guests; operators recompile bundle `expr` strings and bump `policy_bundle_version`. SpiceDB and JWT layers unchanged. See [`policy-dsl-choice.md` §7](../identity/policy-dsl-choice.md) for semantic preservation checklist.

---

*Distills [`docs/identity/policy-dsl-choice.md`](../identity/policy-dsl-choice.md) into the durable Tier 2 policy DSL decision for the MCP gateway.*

# ADR 0029: Policy Engine Crate Extraction

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Engine extraction; unblocks multi-protocol reuse (ACP / A2A), standalone policy-SKU packaging ([ADR 0017](0017-product-positioning.md)), and OSS boundary for `trogon-policy-*` without blocking Phase 2 gateway delivery |
| **Related** | [ADR 0008](0008-policy-dsl.md) (CEL DSL); [ADR 0013](0013-hierarchical-policy-merge.md) (merge → single program); [ADR 0015](0015-tools-list-filtering.md); [ADR 0017](0017-product-positioning.md); [policy-dsl-choice.md](../identity/policy-dsl-choice.md); [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md); [reference-host-abi.md](../identity/reference-host-abi.md); [mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md); `rsworkspace/crates/trogon-mcp-gateway/tests/policy_eval.rs`, `cel_authz_gate.rs`, `redaction_rules.rs`, `hierarchical_policy_merge.rs`, `tools_list_filter.rs` |

## Context

`MCP_GATEWAY_PLAN.md` Block F item 8 schedules extraction of the policy engine into `trogon-policy-core` and `trogon-policy-cel` so ACP, A2A, and future JSON-RPC-over-NATS protocols can reuse the same evaluation stack without duplicating CEL, merge, and host-builtin machinery. The plan's agentgateway translation table (`MCP_GATEWAY_PLAN.md` §11) maps agentgateway `core` → `trogon-policy-core` and `cel-fork` / `celx` → `trogon-policy-cel`.

Today the engine lives **inside** `trogon-mcp-gateway`:

| Module / path | Role on disk today |
|---------------|-------------------|
| `src/policy.rs` | Phase 1 CEL ingress gate (`SpicedbGatePolicy`), JWT/`chain` CEL binding (`configure_policy_cel_context`, `new_policy_cel_context`), risk adjunct (`CallContext`, `RiskDecision`, `PolicyOutcome`) |
| `src/cel_builtins/` | Host ABI namespaces (`spicedb`, `cache`, `jsonpath`, `audit`, `time`, `rate`) — many stubs return `NotImplemented` until Block E |
| `src/redaction/` | Schema-driven `RedactionRule`, `RedactionRuleset`, `redact` engine (Block E item 5) |
| `src/gateway.rs` | Orchestrates ingress, authz, policy gate, egress — NATS- and MCP-specific |

Block E (`MCP_GATEWAY_PLAN.md` Block E items 1–6) is actively wiring CEL builtins, `tools/list` filtering ([ADR 0015](0015-tools-list-filtering.md)), schema cache, redaction, hierarchical merge ([ADR 0013](0013-hierarchical-policy-merge.md)), and rate limits **in place** inside the gateway crate. ADR 0013 already names a future `trogon-policy-core` merge crate but explicitly allows Phase 1 hardcoded CEL to remain until Block E lands.

**Why decide now (paper only):**

- Without a recorded split boundary, Block E implementers may embed MCP/NATS types into merge and builtin code that block later extraction.
- Block F item 8 has no trigger criteria — teams could extract prematurely (platform trap per plan Take item 2) or never extract (ACP/A2A duplication).
- [ADR 0017](0017-product-positioning.md) ties extraction timing to product shape: TrogonStack **feature** defers extraction; **standalone security product** accelerates it — positioning remains Proposed, but engineering needs a default path that does not wait on leadership.

**Forcing-function summary from plan Take item 4:** one protocol (MCP) does not justify a generic policy platform; a second DSL or second protocol consumer is the natural extraction signal.

This ADR does **not** authorize code moves, new workspace members, or dependency changes. It records **when** to split, **what** each crate owns, and **how** the gateway depends on them so Block E can proceed monolithically without contradiction.

---

## Decision

**Defer physical crate extraction until a documented forcing function fires.** Until then, implement and harden the policy engine **inside** `trogon-mcp-gateway` using module boundaries that mirror the future crates. Block E ships on the monolith; Block F item 8 executes the mechanical split only after the triggers below.

Normative status: **Accepted (defer)** — extraction is **planned and bounded**, not cancelled.

### Extraction triggers (any one is sufficient)

| Priority | Trigger | Rationale |
|----------|---------|-----------|
| **P0** | A **second policy DSL** is accepted for Tier 2+ (e.g. Rego, CUE, Cedar) per a future ADR amending [ADR 0008](0008-policy-dsl.md) | Immediate justification for `trogon-policy-core` DSL-agnostic types + pluggable `ExpressionBackend` trait; CEL becomes one adapter in `trogon-policy-cel` |
| **P0** | A **second protocol crate** (ACP, A2A, or `trogon-mcp-protocol` consumer) must embed the same merge + CEL + builtin surface without depending on the gateway binary | Avoid copy-paste of `policy.rs` / `cel_builtins/`; extraction precedes that crate's policy hot path |
| **P1** | [ADR 0017](0017-product-positioning.md) is **Accepted** as **standalone security product** (Option B) | Commercial packaging requires independently versioned `trogon-policy-*` artifacts; accelerates Block F item 8 even if only MCP exists |
| **P1** | An **external OSS or downstream consumer** needs policy evaluation crates published without `trogon-mcp-gateway` | License and API stability boundary ([ADR 0017](0017-product-positioning.md) OSS envelope) |
| **P2** | Block E **and** Block F items 1–7 (WIT, Wasmtime, bundle loader) are **complete** on the monolith | Mechanical split with stable types and tests; lowest migration risk |

**Non-triggers (do not extract for these alone):**

- Desire to mirror agentgateway crate layout aesthetically (`MCP_GATEWAY_PLAN.md` §11).
- Block E starting or mid-flight — see migration risk below.
- WASM Tier 3 landing — WASM host ABI stays in gateway + WIT sketch until core traits exist; extraction follows stable builtin contracts ([reference-host-abi.md](../identity/reference-host-abi.md)).

### Crate boundaries when extraction executes

#### `trogon-policy-core` (DSL-agnostic)

Owns types and orchestration with **no** dependency on `cel-interpreter`.

| Surface | Contents | On-disk lineage |
|---------|----------|-----------------|
| **Evaluation snapshot** | `PolicyContext` **(proposed)** — immutable per-request binding bag (tenant, method, JWT claims handle, NATS metadata, session ids, list-filter index); distinct from today's `CallContext` (risk-only) | Compose from [reference-cel-variables.md](../identity/reference-cel-variables.md) Pin 8 roots + gateway identity types |
| **Decision algebra** | `PolicyDecision` **(proposed)** — normative enum covering plan decision verbs: `Allow`, `Deny`, `Suspend`, `Log`, `Error`, `Rewrite`, `Shape` | Plan § Policy Engine; audit subjects `allow\|deny\|rewrite\|error`; today split across `RiskDecision`, `PolicyOutcome`, JSON-RPC codes |
| **Redaction** | `RedactionRule`, `RedactionAction`, `JsonPath`, `RedactionRuleset`, `RedactionError` | `src/redaction/` today |
| **Merge** | Hierarchical merge AST, `MergeEngine`, expression hash `H(M)`, program-class metadata | [ADR 0013](0013-hierarchical-policy-merge.md), [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md) |
| **Program taxonomy** | `ProgramClass` (`ingress_gate`, `tools_list_filter`, `risk_adjunct`, `audit_enrichment`) | [ADR 0008](0008-policy-dsl.md) § Program classes |
| **Host effect model** | `BuiltinEffect` classification (pure / io / audit), `HostAbiError` | [reference-host-abi.md](../identity/reference-host-abi.md) |
| **Traits** | See trait table below | New at extraction |

**Explicitly not in `-core`:** CEL `Program` compile/eval, `cel_builtins::register`, SpiceDB gRPC client, NATS publish, Wasmtime — those are adapters or gateway infrastructure.

#### `trogon-policy-cel` (CEL backend)

Owns everything that touches `cel-interpreter` (workspace pin `0.10.0` per [ADR 0008](0008-policy-dsl.md)).

| Surface | Contents | On-disk lineage |
|---------|----------|-----------------|
| CEL compile cache | `(tenant, bundle_revision, program_id, cel_version, expr_hash) → Program` | ADR 0008 implementation notes |
| Context binding | `configure_policy_cel_context`, `new_policy_cel_context`, act-chain helpers | `policy.rs` today |
| Ingress / list eval | `SpicedbGatePolicy`, merged predicate execution, per-item list re-eval ([ADR 0015](0015-tools-list-filtering.md)) | `policy.rs` + Block E |
| Builtins | `cel_builtins/` modules + `register` | `src/cel_builtins/` |
| Merge → CEL | Canonical CEL builder emitting single `Program` for merged bundle | ADR 0013 pending impl |

**Dependency edge:** `trogon-policy-cel` → `trogon-policy-core` only. No reverse edge.

#### `trogon-mcp-gateway` (orchestrator)

| Responsibility | Depends on |
|----------------|------------|
| NATS ingress/egress, session KV, audit JetStream publish, SpiceDB client wiring, Wasmtime pool | `trogon-policy-cel`, `trogon-policy-core`, existing `mcp-nats`, `trogon-nats`, `trogon-sts`, … |
| MCP-specific subject grammar, JSON-RPC error mapping (`rpc_codes.rs`), gateway settings | Stays in gateway |
| Builtin **implementations** that perform I/O | Gateway provides `BuiltinProvider` **(proposed)** impls injected into CEL registrar (SpiceDB checks, rate KV, audit emit) |

### Trait and dependency contract **(proposed)**

| Trait | Crate | Implementor | Purpose |
|-------|-------|-------------|---------|
| `ExpressionBackend` | `trogon-policy-core` | `CelBackend` in `trogon-policy-cel` | Compile + eval boolean / record programs for a `ProgramClass`; future `RegoBackend` etc. |
| `PolicyMerger` | `trogon-policy-core` | `CelMergeBuilder` in `trogon-policy-cel` (or co-located merge module) | Deterministic hierarchical merge per ADR 0013 |
| `BuiltinProvider` | `trogon-policy-core` | `trogon-mcp-gateway` (production), test doubles in `tests/` | Close over NATS/SpiceDB handles; CEL registrar calls trait methods, not gateway modules |
| `RedactionEngine` | `trogon-policy-core` | Default impl in `-core` | Pure JSONPath transforms; gateway invokes before audit/egress |

Gateway dependency graph after extraction:

```text
trogon-mcp-gateway
  ├── trogon-policy-cel
  │     └── trogon-policy-core
  ├── mcp-nats, trogon-nats, trogon-sts, …
  └── (implements BuiltinProvider, owns I/O)
```

Phase 1–2 **before** extraction: gateway **is** the orchestrator; modules `policy`, `cel_builtins`, `redaction` act as logical `-cel` / `-core` boundaries (no new crates).

### Block E non-blocking guarantee

Block E work **must not** wait on crate extraction. Ordering:

1. Land Block E features in `trogon-mcp-gateway` modules listed above.
2. Keep public test surface stable (`trogon_mcp_gateway::policy::*`, `::redaction::*`, `::cel_builtins::*`) until extraction PR provides re-exports or thin facade.
3. Run extraction as a **single mechanical PR** only when a P0/P1 trigger fires **or** P2 completion is declared in release notes.

Mid-Block-E extraction is **forbidden** unless a P0 trigger (second DSL or second protocol crate) forces it — in that case, freeze Block E merge wiring until split lands to avoid double churn.

### Open-source vs internal split

Until [ADR 0017](0017-product-positioning.md) resolves:

| Artifact | Default (Option A — feature) | If Option B — standalone |
|----------|------------------------------|---------------------------|
| `trogon-policy-core` | Monorepo crate; Apache-2.0 **(assumed, TBD with 0017)** | Publish to crates.io; semver independent of gateway |
| `trogon-policy-cel` | Same repo; couples to `cel-interpreter` pin | Same; documented compatibility matrix with gateway |
| `trogon-mcp-gateway` | TrogonStack-internal binary | May still be open core; commercial SKU is support + bundles |

Extraction does **not** imply a separate git repository; it implies **workspace members** under `rsworkspace/crates/` per `MCP_GATEWAY_PLAN.md` §11. Repo split is an [ADR 0017](0017-product-positioning.md) follow-on, not Block F item 8.

---

## Consequences

### Positive

- **Block E velocity preserved** — no workspace churn, `Cargo.lock` fan-out, or cross-crate pub API debates while builtins and merge are still moving.
- **Clear module contract** — implementers treat `policy`, `cel_builtins`, and `redaction` as extraction boundaries; fewer MCP imports leak into merge logic.
- **Honest platform timing** — aligns with plan Take item 4; avoids premature `trogon-policy-*` crates with one consumer.
- **Deterministic trigger list** — second DSL or protocol forces the split; leadership can accelerate via ADR 0017 without re-opening architecture.
- **Test stability** — acceptance scaffolds under `tests/policy_eval.rs`, `cel_authz_gate.rs`, `redaction_rules.rs`, `hierarchical_policy_merge.rs`, `tools_list_filter.rs` keep importing `trogon_mcp_gateway::*` until extraction PR updates paths once.

### Negative

- **Monolith coupling** — ACP/A2A teams cannot depend on published policy crates until a trigger fires.
  - **Mitigation:** If ACP/A2A work starts before triggers, elevate **P0 second protocol** and schedule extraction ahead of that crate's policy hot path; do not fork `policy.rs`.
- **Type duplication risk** — `RiskDecision` / `PolicyOutcome` today are not yet unified as `PolicyDecision` **(proposed)**.
  - **Mitigation:** Block E refactors toward proposed names **inside** `policy.rs` without moving files; document mapping in extraction PR checklist.
- **Version churn when split lands** — one-time blast radius across gateway, tests, and any external branch importing `trogon_mcp_gateway::policy`.
  - **Mitigation:** Single mechanical PR; re-export deprecated paths for one release cycle **(proposed)**.
- **ADR 0013 names `trogon-policy-core` for merge** — deferred crate name creates expectation debt.
  - **Mitigation:** This ADR subsumes timing; merge code lands in `trogon-mcp-gateway/src/policy/` (or `policy/merge.rs`) until extraction.

### Neutral

- **`cel-interpreter` pin unchanged** — extraction moves the dependency to `trogon-policy-cel`; workspace pin in `rsworkspace/Cargo.toml` stays single-sourced per ADR 0008.
- **WASM / WIT** — Phase 3 guests still target host ABI; crate split does not change `trogon:mcp-policy@0.1.0` sketch ([mcp-policy-wit-sketch.md](../identity/mcp-policy-wit-sketch.md)).
- **Research docs referencing `trogon-policy`** — Microsoft AGT gap analysis predates this split; future crate names are `trogon-policy-core` / `trogon-policy-cel`, not a single `trogon-policy` crate.

---

## Rejected alternatives

### Extract now (Block F item 8 before Block E completes)

| | |
|---|---|
| **Pros** | Early crates.io story; forces clean boundaries immediately; matches agentgateway layout early |
| **Cons** | High churn across in-flight Block E PRs; builtins still stubbed; merge algorithm not on disk; doubles CI matrix; violates plan Take item 2 (engine before product) |
| **Verdict** | **Rejected** — migration risk on active Block E work outweighs reuse benefits with a single protocol consumer |

### Never extract (permanent monolith)

| | |
|---|---|
| **Pros** | Simplest dependency graph; one test target; no semver for policy crates |
| **Cons** | ACP/A2A copy-paste or gateway dependency inversion; cannot ship standalone policy SKU ([ADR 0017](0017-product-positioning.md) Option B); second DSL requires fork of `cel-interpreter` usage inside gateway |
| **Verdict** | **Rejected** — plan Block F item 8 and multi-protocol ambition remain valid; defer only, not cancel |

### Single crate `trogon-policy` (no `-core` / `-cel` split)

| | |
|---|---|
| **Pros** | One version line; simpler docs; matches older research doc name |
| **Cons** | `cel-interpreter` becomes mandatory for consumers needing only redaction/merge types; second DSL still pulls CEL into unrelated binaries |
| **Verdict** | **Rejected** — `-core` / `-cel` split is the minimum separation for DSL plugins ([ADR 0008](0008-policy-dsl.md) already anticipates `trogon-policy-cel`) |

### Extract `-cel` only, leave merge/redaction in gateway

| | |
|---|---|
| **Pros** | Smaller first PR; CEL builtins isolated |
| **Cons** | ACP/A2A still cannot reuse merge; `trogon-policy-core` purpose diluted; two partial extractions mean double migration |
| **Verdict** | **Rejected** — when extraction runs, move **both** DSL-agnostic surfaces and CEL together per trait table above |

---

## Open questions

1. **Exact `PolicyContext` / `PolicyDecision` fields** — Block E should converge risk, ingress, and audit outcomes into proposed types inside the monolith; field list TBD in implementation PR, not this ADR.
2. **`BuiltinProvider` async shape** — sync CEL host functions vs `async` SpiceDB: trait design must match `cel-rust` registration constraints (TBD at extraction).
3. **ADR 0017 verdict** — Option B may move extraction from P2 to P1; update trigger table in a one-line amendment to this ADR when 0017 accepts.
4. **Published crate names** — `trogon-policy-core` vs `trogon_policy_core` on crates.io; trademark / neutral naming if standalone (TBD with 0017).
5. **Whether redaction stays in `-core` or `trogon-mcp-gateway` only** — JSONPath redaction is protocol-agnostic; default is `-core`. If A2A uses a different ruleset shape, split `RedactionRuleset` versioning (future ADR).
6. **NATS-callout Tier 2.5 plugins** (`MCP_GATEWAY_PLAN.md` Block F item 7) — plugin bus may live in gateway while eval stays in `-cel`; integration ADR TBD.
7. **Deprecation period for `trogon_mcp_gateway::policy` re-exports** — one release vs immediate break (TBD at extraction PR).

---

## Rollback plan

This ADR is paper-only; rollback is **process**, not a feature flag.

| Situation | Action |
|-----------|--------|
| Extraction landed but causes Block E/F regression | Revert the mechanical split PR; restore single `trogon-mcp-gateway` crate; keep module boundaries. No data migration — policy state is KV/bundle external. |
| Premature extraction before trigger | Same revert; re-apply this ADR's defer triggers in team checklist; do not cherry-pick `-cel` without `-core`. |
| Second DSL chosen then abandoned | Keep `-core` traits; remove unused backend crate; CEL remains sole `ExpressionBackend` impl — avoid re-merging into gateway unless maintenance cost forces it (new ADR). |
| ADR 0017 stays Option A indefinitely | Extraction remains deferred per P0/P2 triggers; cancel scheduled split milestones in release planning. |

Runtime rollback of policy behavior is unchanged: bundle KV pointer rollback and fail-closed defaults per [ADR 0024](0024-failure-mode-matrix.md) — crate layout does not alter bundle semantics.

---

## Implementation notes

### Monolith phase (now through Block E / pre-trigger)

| Path | Action |
|------|--------|
| `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` | Block E merge wiring, expand CEL context toward `PolicyContext` **(proposed)**; unify decision types incrementally |
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` | Implement host ABI per Block E item 1; no new crate |
| `rsworkspace/crates/trogon-mcp-gateway/src/redaction/` | Block E item 5 schema-driven rules; keep free of `cel-interpreter` |
| `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs` | Continues to orchestrate; inject dependencies into policy modules via structs, not global statics — prepares `BuiltinProvider` **(proposed)** |
| `rsworkspace/crates/trogon-mcp-gateway/Cargo.toml` | Retains `cel-interpreter`; no `trogon-policy-*` path deps until extraction PR |

### Extraction phase (when trigger fires)

| New path | Migrates from |
|----------|---------------|
| `rsworkspace/crates/trogon-policy-core/Cargo.toml` | New workspace member; no `cel-interpreter` |
| `rsworkspace/crates/trogon-policy-core/src/lib.rs` | `redaction/`, merge types, traits, `PolicyDecision` **(proposed)** |
| `rsworkspace/crates/trogon-policy-cel/Cargo.toml` | Depends on `trogon-policy-core` + workspace `cel-interpreter` |
| `rsworkspace/crates/trogon-policy-cel/src/lib.rs` | `policy.rs` CEL surfaces, `cel_builtins/` |
| `rsworkspace/crates/trogon-mcp-gateway/Cargo.toml` | Replace direct `cel-interpreter` with `trogon-policy-cel` |
| `rsworkspace/Cargo.toml` | Add workspace members |

### Acceptance test anchors (unchanged until extraction PR)

| Test file | Validates |
|-----------|-----------|
| `tests/policy_eval.rs` | CEL context + act-chain helpers (`new_policy_cel_context`) |
| `tests/cel_authz_gate.rs` | `SpicedbGatePolicy` / SpiceDB gate selection (Block D/E) |
| `tests/redaction_rules.rs` | `RedactionRule`, `redact` scaffold (Block E item 5) |
| `tests/hierarchical_policy_merge.rs` | Merge precedence / `H(M)` **(proposed)** |
| `tests/tools_list_filter.rs` | Per-item CEL filtering ([ADR 0015](0015-tools-list-filtering.md)) |
| `tests/bundle_load_hot_reload.rs` | Bundle compile failure keeps last good generation (extraction must not change semantics) |

### Documentation follow-ups (non-blocking)

- `MCP_GATEWAY_PLAN.md` Block F item 8 checkbox — reference this ADR when editorially closing the item as "deferred with triggers."
- [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md) checklist `trogon-policy-core` merge crate — implement in monolith first, move at extraction.

---

*Contract sources: `MCP_GATEWAY_PLAN.md` (Take §4, Block E, Block F item 8, §11 crate translation); `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs`, `cel_builtins/`, `redaction/`, `gateway.rs`, `Cargo.toml`; `docs/adr/0008-policy-dsl.md`, `docs/adr/0013-hierarchical-policy-merge.md`, `docs/adr/0015-tools-list-filtering.md`, `docs/adr/0017-product-positioning.md`; `docs/identity/policy-dsl-choice.md`, `hierarchical-policy-merge.md`, `reference-host-abi.md`, `reference-cel-variables.md`, `mcp-policy-wit-sketch.md`, `wasm-bundle-format.md`.*

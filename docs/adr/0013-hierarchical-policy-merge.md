# ADR 0013: Hierarchical Policy Merge Precedence

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block E item 6 — hierarchical policy merge across subject-pattern specificity |
| **Related** | [Hierarchical policy merge spec](../identity/hierarchical-policy-merge.md) · [Policy DSL choice (CEL)](../identity/policy-dsl-choice.md) · [CEL variable namespace](../identity/reference-cel-variables.md) · [MCP gateway plan Block E](../../MCP_GATEWAY_PLAN.md) |

## Context

MCP gateway authorization is not a single bundle. Operators attach policy at multiple scopes simultaneously: an organisation baseline, tenant compliance narrows, a server-group (project or backend cohort) grants team access, per-server overrides tighten redaction, and method-scoped rules gate `tools/call` vs `resources/read`. Without a **pinned precedence rule**, two gateways loading identical bundle sets can reach different allow/deny outcomes depending on KV notification order, cache generation, or which code path evaluates policy first (ingress gate, `tools/list` shaping, callback authorization, adaptive-access risk).

**Bundles must be composable.** Platform teams need tenant-wide posture without re-authoring every server bundle. Tenant operators need per-server tightening without forking the org default. Emergency break-glass at one backend must not require republishing org-wide YAML. Composability only holds when merge semantics are deterministic and documented.

**What breaks if this stays undecided:**

- Block E cannot wire hierarchical merge into `policy.rs` — Phase 1 hardcoded CEL remains the only gate.
- `tools/list` filtering and `tools/call` authorization can disagree on the same tool ([tools-list-filtering.md](../identity/tools-list-filtering.md)).
- Audit envelopes cannot reproduce `-32100 policy_deny` from stored fields alone ([failure-mode-matrix.md](../identity/failure-mode-matrix.md) invariant 4).
- Operator dry-run and bundle review tooling have no stable "effective policy" to compare against production.

The normative design context lives in [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md). That document defines hybrid deny accumulation, CEL composition, KV key patterns, and extended identity scopes (agent-instance, agent-class, project). This ADR records the **durable precedence choice** and conflict rules that implementation must not reinterpret.

### Subject-pattern specificity

Policy bundles bind to NATS subject patterns and JWT dimensions, not to evaluation order of arrival. Specificity increases down the hierarchy:

| Rank (general → specific) | Scope | Typical selector |
|---|---|---|
| 1 | **Organisation** | Deployment-wide default; org root in control plane |
| 2 | **Tenant** | `jwt.tenant` (required in mesh tokens) |
| 3 | **Server group** | Project membership, backend cohort, or bundle `targets` group |
| 4 | **Server** | `server_id` from `mcp.gateway.request.{server_id}.…` |
| 5 | **Method** | `mcp.method` and tool/resource action class within the MCP RPC |

The spec doc adds **agent-instance**, **agent-class**, and **project** levels with the same merge semantics; they participate **between server group and server** when present (most-specific first). Empty levels are skipped — not implicit allow.

Under hard tenancy (NATS account per tenant, [ADR 0001](0001-tenancy-model.md)), tenant-scoped keys may omit the `tenant/{id}` prefix inside the account bucket. Under soft tenancy, keys include the tenant segment. Merge behavior is identical; only KV path layout differs.

### Relationship to CEL and bundles

Block B locked **CEL** as the policy DSL ([policy-dsl-choice.md](../identity/policy-dsl-choice.md)). Each rule compiles to a predicate; hierarchical merge produces **one merged expression** evaluated by the same interpreter on the hot path. Host builtins (`spicedb.check`, `rate.acquire`, `time.now`, …) bind per [reference-cel-variables.md](../identity/reference-cel-variables.md). Merge must not fork evaluator implementations per layer.

## Decision

Policy resolution walks the hierarchy **`org → tenant → server-group → server → method`** with **shallow field-level merge** (later layers override earlier). **`deny` decisions are sticky** — any layer emitting a matching deny short-circuits to deny. **`allow` requires explicit allow** at a contributing layer in the merged allow side; absence of rules is not allow. **Default: deny** when no allow side is satisfied (`-32108 no_policy` when no bundles exist at any level in production enforce mode).

Formally, for authorization predicates compiled from rule `effect` fields:

```text
ALLOW ⇔ (A_org ∨ A_tenant ∨ … ∨ A_method) ∧ ¬(D_org ∨ D_tenant ∨ … ∨ D_method)
```

Each `A_*` / `D_*` is the OR of matching allow/deny CEL predicates at that level after same-level conflict resolution. Missing level ⇒ that disjunct is false. **Config overlays** (rate caps, redaction paths, inflight limits) use shallow merge: more-specific layers may **narrow** values but must not widen beyond the effective value from less-specific layers (take the minimum rate cap, union of redaction paths, etc.).

Same-level conflicts: **deny wins** over allow when both match; among rules of the same effect, sort by **`priority` descending**, then **`policy_id` ascending** for stable audit ordering. Merge output is **order-independent** of KV publish sequence ([hierarchical-policy-merge.md § Order independence](../identity/hierarchical-policy-merge.md#4-order-independence)).

## Consequences

### Positive

- **Composable bundles.** Org, tenant, and server authors publish independently; merged outcome is predictable from bundle contents and request context, not from deployment order.
- **Predictable resolution.** Canonical sort by hierarchy level, priority, and `policy_id` yields stable expression hash `H(M)` for caching and offline replay.
- **Deny-sticky is fail-safe.** A tenant deny cannot be accidentally widened by a less-specific org allow; security review can treat deny rules as authoritative without deleting upstream allows.
- **Single evaluator.** One merged CEL program serves ingress, list filtering, and callback paths — Block E catalog shaping reuses the same predicate per item.
- **Audit reproducibility.** Envelopes carry `policy_merge.expression_hash` and contributing `policy_ids`; reviewers re-merge offline and verify hash match.

### Negative

- **Authoring literacy.** Bundle authors must understand the hierarchy and that org allows do not override tenant denies; misconfigured allows at multiple levels can confuse operators who expect "last file wins."
- **Debugging cost.** A deny requires tracing the resolution chain across levels, same-level priority ties, and CEL match results; `rules_fired` and dry-run tooling become mandatory for support, not optional niceties.
- **No deep merge for rules.** Shallow field merge means nested YAML maps at org level are replaced entirely by tenant maps when keys collide — authors cannot rely on deep inheritance of nested structures without explicit duplication or code generation.
- **Default deny surprises.** Empty allow side denies even when no deny matched; day-zero deployments must seed org or tenant bundles before `tenant_initialised` ([bootstrap-day-zero.md](../identity/bootstrap-day-zero.md)).

### Neutral

- **Shallow, not deep.** Merge does not accumulate per-field allow predicates across layers for config objects; only authorization CEL uses OR-of-layers semantics. Structural bundle fields override wholesale at the more-specific layer.
- **Extended scopes are additive.** Agent-instance, agent-class, and project levels from the spec doc add resolution steps but do not change deny-sticky or default-deny rules.
- **Phase 1 gate unchanged until wired.** Hardcoded Phase 1 CEL in `policy.rs` remains until Block E merge crate lands; this ADR does not require a flag-day cutover in this commit.

## Alternatives considered

### Deep merge

**Description:** Recursively merge nested policy maps so tenant YAML inherits org nested keys and only overrides leaf fields.

**Why rejected:** Ambiguous semantics for lists (replace vs append), for conflicting `effect` on the same logical rule, and for CEL fragments that must compose as boolean logic not structural inheritance. Reviewers cannot predict effective policy without a full tree walk. Deep merge also breaks order independence when sibling keys publish in different orders.

### First-match-wins

**Description:** Evaluate bundles in discovery order; stop at the first matching allow or deny rule.

**Why rejected:** Outcome depends on KV notification order and filesystem ordering — identical bundles produce different results on different gateways. Less composable: operators cannot publish a tenant deny after an org allow and rely on deny-sticky without re-publishing org bundle. Violates audit reproducibility goals in [hierarchical-policy-merge.md § Counter-example](../identity/hierarchical-policy-merge.md#counter-example-order-dependent-merge-violation).

### Allow-sticky (deny requires explicit deny everywhere)

**Description:** Any layer allow suffices unless every layer explicitly denies; default allow when no rule matches.

**Why rejected:** Dangerous default for a gateway PEP. A missing tenant bundle would inherit org allow and widen access; publish-order bugs become privilege escalation. Conflicts with fail-closed posture in [failure-mode-matrix.md](../identity/failure-mode-matrix.md) and production `-32108 no_policy` when no bundles exist.

### Pure override (most-specific layer only)

**Description:** Only the most-specific matching level applies; less-specific layers ignored entirely.

**Why rejected:** Breaks composable org baselines — tenant bundle must restate entire org allow set to avoid accidental total deny. Documented in spec as failing security review when org allow and tenant deny coexist ([hierarchical-policy-merge.md § Why not pure Override](../identity/hierarchical-policy-merge.md#why-not-pure-override)).

### Pure layered (more-specific may only narrow allows)

**Description:** More-specific layers can only restrict less-specific allows, never add new allow paths.

**Why rejected:** Cannot express legitimate agent-instance break-glass allow when tenant published a broad deny for unrelated tools; hybrid model needed ([hierarchical-policy-merge.md § Why not pure Layered](../identity/hierarchical-policy-merge.md#why-not-pure-layered)).

## Implementation notes

### Subject-pattern specificity scoring

Implementations assign each bundle a **level rank** (org=1 … method=5; extended agent/project ranks between server-group and server per spec). At reload, collect applicable bundles by matching request context (`jwt.*`, `nats.subject`, `mcp.method`) — never by KV watch arrival time. Optional **context fingerprint** cache dimension: `(tenant, server_group, server_id, method, agent_id, bundle_generation)`.

### Conflict detection in dry-run

Block E follow-up: `trogon-mcp-gateway policy explain` (or equivalent) accepts synthetic context, runs canonical merge, returns merged CEL, `H(M)`, verdict, and **conflict hints** (same-level allow+deny both match, duplicate `policy_id`, priority collisions). Dry-run must use the same algorithm as production; no side effects on backends or `MCP_AUDIT` unless `--emit-audit`. Conflicts are not merge failures — outcome is deny when deny matches — but dry-run surfaces them for bundle authors before publish.

### CEL re-evaluation per layer

**Compile time:** Each level's rules compile to `A_L` / `D_L`; merge builds single expression `M`.

**Request path:** Evaluate `M` once for ingress authorization. For **`tools/list`**, **`prompts/list`**, and **`resources/list`**, re-evaluate `M` per catalog item with list-scoped bindings (`response.list_filter_index`, tool name, resource URI) — same merged program, different binding snapshot per item ([MCP_GATEWAY_PLAN.md Block E](../../MCP_GATEWAY_PLAN.md)).

**Optimization:** Evaluator may short-circuit on first matching deny disjunct for latency once correctness tests prove equivalence to full `M` evaluation.

### Audit `rules_fired`

Every audit envelope for gated methods MUST include:

| Field | Requirement |
|---|---|
| `policy_merge.expression_hash` | SHA-256 of canonical merged CEL (`H(M)`) |
| `policy_merge.policy_ids` | All contributing rules `{ id, revision, level }` in merge tree |
| `policy_merge.merge_model` | `"hierarchical/v1"` (or successor version if algorithm changes) |
| `rules_fired` | **Every rule that contributed** to `M` at merge time — all `policy_id`s whose predicates were compiled into the merged expression, not only the subset that matched this request |

On deny, `error.reason` cites the highest-priority matching deny `policy_id`. Matching subset is derivable by re-evaluating each compiled predicate against the envelope context for support dashboards; `rules_fired` remains the full contribution list for bundle-generation audits.

Example fragment:

```json
{
  "decision": "deny",
  "rules_fired": [
    "org/default-employees",
    "tenant/acme-block-secrets",
    "server/github-allowlist"
  ],
  "policy_merge": {
    "expression_hash": "sha256:…",
    "merge_model": "hierarchical/v1",
    "policy_ids": [
      { "id": "org/default-employees", "revision": 3, "level": "org" },
      { "id": "tenant/acme-block-secrets", "revision": 17, "level": "tenant" },
      { "id": "server/github-allowlist", "revision": 42, "level": "server" }
    ]
  }
}
```

### Storage and hot path

Policy bundles in NATS KV bucket **`mcp-gateway-config`** under `policy/{level}/…` keys ([hierarchical-policy-merge.md § Storage](../identity/hierarchical-policy-merge.md#6-storage)). Background watcher triggers merge+compile; request path reads cache keyed by `H(M)`. CEL compile failure for a generation **refuses** adoption — keep last good generation (`-32101 policy_fault` if no cache). Missing level **skips** (degraded merge).

### Code touchpoints (follow-up PRs)

| Area | Change |
|---|---|
| `trogon-policy-core` (new) | Canonical merge + CEL builder |
| `trogon-mcp-gateway/src/policy.rs` | Wire merged predicate before SpiceDB gate |
| KV watcher | Per-level keys under `mcp-gateway-config` |
| Audit publisher | `policy_merge` block + expanded `rules_fired` |
| Metrics | `policy_merge_cache_hit`, `policy_merge_compile_ms` |

## Status of supporting work

| Item | Status |
|---|---|
| [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md) | **Done** — normative merge algorithm, examples, KV layout, failure modes |
| [policy-dsl-choice.md](../identity/policy-dsl-choice.md) | **Done** — CEL locked; merge compiles to one program |
| [reference-cel-variables.md](../identity/reference-cel-variables.md) | **Done** — binding namespace for merged evaluation |
| `MCP_GATEWAY_PLAN.md` Block E item 6 | **Unblocked by this ADR** — implementation still pending |
| Phase 1 hardcoded CEL gate in `policy.rs` | **Shipped** — placeholder until merge wired |
| CEL host builtins (`spicedb.check`, `rate.acquire`, …) | **Pending** — stubs return `NotImplemented` |
| `trogon-policy-core` merge crate | **Pending** |
| KV per-level policy keys + watcher | **Pending** |
| Audit `policy_merge` + full `rules_fired` | **Pending** |
| `policy explain` dry-run CLI | **Pending** — spec recommends Block E follow-up |
| `tools/list` per-item re-evaluation | **Pending** — depends on merge + list filter programs |

---

*Design context: [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md). This ADR is the acceptance record for precedence and conflict rules; implementation tracks Block E.*

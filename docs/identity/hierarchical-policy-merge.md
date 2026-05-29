# Hierarchical policy merge

**Status:** Normative reference (Block E, paper). Resolves the open item in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block E — *Hierarchical policy merge across subject-pattern specificity*.

**Diátaxis:** [Explanation](#explanation) (why merge order matters and how hybrid semantics protect tenants) + [Reference](#reference) (levels, merge rules, CEL composition, storage keys, audit fields, failure modes).

**Related:** [Identity overview](overview.md), [Adaptive access](adaptive-access.md), [Failure-mode matrix](failure-mode-matrix.md), [Bootstrap / day-zero](bootstrap-day-zero.md).

---

## Explanation

### Problem statement

MCP gateway authorization is not a single bundle. Policies attach at multiple scopes simultaneously:

- An **organisation** ships a baseline deny/allow posture for every tenant.
- A **tenant** narrows tool access for compliance.
- A **project** grants a team access to a subset of servers.
- A **server** override may tighten redaction for one backend.
- An **agent class** (e.g. `oncall-responder`) carries role-shaped defaults.
- An **agent instance** (e.g. `acme/oncall-responder-7`) may carry an emergency break-glass allow or deny.

Without a **deterministic merge order** and **conflict-resolution rule**, two code paths — ingress CEL evaluation, `tools/list` re-evaluation, risk scoring ([adaptive-access.md](adaptive-access.md)), callback authorization ([bidirectional-enforcement.md](bidirectional-enforcement.md)) — can disagree on whether the same call is allowed. Reviewers cannot reproduce a `-32100 policy_deny` from audit alone.

This document pins the merge semantics so implementation can follow in a later track (`policy.rs` is intentionally untouched here).

### Design goals

1. **Security-first defaults.** Less-specific layers must not widen access that a more-specific layer removed; organisation-wide allows must not override tenant denies.
2. **Order independence.** The merged predicate must be identical regardless of the order bundles were published to KV.
3. **Single evaluator.** Every tier compiles to CEL; the merged result is one CEL expression evaluated by the same interpreter on the hot path.
4. **Auditable denies.** A deny must cite every contributing `policy_id` and a stable hash of the merged expression so offline replay reproduces the decision ([failure-mode-matrix.md](failure-mode-matrix.md) invariant 4).
5. **KV-native distribution.** Policy bundles live in NATS KV; merge runs at bundle reload and on cache miss, not per NATS round-trip.

### Relationship to agentgateway

agentgateway merges policies **backend-group → target** with **most-specific-wins per field** (shallow field merge). Trogon adopts the same *specificity intuition* but expresses scopes as **identity and routing dimensions** (organisation, tenant, project, server, agent class, agent instance) rather than YAML backend hierarchy. Merge semantics here are **hybrid** (see §2), not pure override, because security policy requires deny accumulation across levels.

---

## Reference

### 1. The hierarchy

Policies are evaluated from **most-specific to least-specific**. Each level may contribute zero or more policy bundles. A level with no bundle is **skipped** (not an implicit allow).

| Rank | Level | Selector (how the gateway resolves applicable bundles) | Example `policy_id` prefix |
|---|---|---|---|
| 1 | **Agent-instance** | `jwt.agent_id` exact match **and** optional instance suffix in KV key | `agent-instance/acme/oncall-responder-7` |
| 2 | **Agent-class** | Registry `agent_class` or `agent_id` namespace prefix (everything before the last `-` segment when instance numbering is used; otherwise full `agent_id` when no instance suffix exists) | `agent-class/oncall-responder` |
| 3 | **Server** | `server_id` parsed from `nats.subject` (`mcp.gateway.request.{server_id}.…`) or virtual federation member after `::` split | `server/github` |
| 4 | **Project** | `jwt.project` claim when present; else KV index `project/{project_id}/members` listing agents and servers | `project/incident-response` |
| 5 | **Tenant** | `jwt.tenant` (required in mesh tokens per [overview.md](overview.md)) | `tenant/acme` |
| 6 | **Organisation default** | Deployment-wide bundle pointer not scoped to a tenant key (single-org deployments) or org root in multi-tenant control plane | `org/default` |

**Specificity rule:** When multiple levels match, all matching levels participate in merge (§2). No level "shadows" another except where hybrid semantics explicitly ignore a less-specific **allow**.

#### Fallback when no policies are present

If **no bundle exists at any level** for the request context:

| Deployment mode | Fallback behavior | JSON-RPC | Audit |
|---|---|---|---|
| Production enforce | **CLOSED** — treat as `no_policy` | `-32108` `no_policy` (proposed; see [failure-mode-matrix.md](failure-mode-matrix.md) row 5) | `{prefix}.audit.deny.request.{method_root}` |
| Dev / Phase 1 bypass | SpiceDB endpoint unset ⇒ Phase 1 hardcoded CEL gate only ([policy.rs](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs)) | Allow path continues to SpiceDB hook when expression matches | Partial — no hierarchical `policy_ids` |
| Day-zero partial | Organisation default missing but tenant bundle present | Merge uses only populated levels | `policy_ids` lists only contributing levels |

The **normative production fallback is deny** (`-32108`). Operators must seed at least an organisation-default or tenant bundle before marking `tenant_initialised` ([bootstrap-day-zero.md](bootstrap-day-zero.md)).

#### Worked example: level resolution

Request context:

- `jwt.tenant = "acme"`
- `jwt.agent_id = "acme/oncall-responder-7"`
- `jwt.project = "incident-response"`
- `nats.subject = "mcp.gateway.request.github.tools.call"`

Resolution order (fetch bundles for each; empty levels skipped):

1. `mcp-gateway-config/policy/agent-instance/acme/oncall-responder-7`
2. `mcp-gateway-config/policy/agent-class/oncall-responder`
3. `mcp-gateway-config/policy/server/github`
4. `mcp-gateway-config/policy/project/incident-response`
5. `mcp-gateway-config/policy/tenant/acme`
6. `mcp-gateway-config/policy/org/default`

Under **hard tenancy** (NATS account per tenant), keys 5–6 may live in the tenant account's KV bucket with shortened paths (`policy/agent-instance/…` without `tenant/acme` prefix). Under **soft tenancy**, keys include the tenant segment as shown.

---

### 2. Merge semantics

**Chosen model: Hybrid.**

| Rule kind | Cross-level behavior |
|---|---|
| **Deny rules** | **Accumulate (OR).** If any level emits a matching deny, the merged decision is deny unless a more-specific level explicitly marks the deny as `supersedable: false` (reserved; default is non-supersedable). |
| **Allow rules** | **Most-specific wins (override).** An allow at a more-specific level replaces less-specific allow predicates for the same `(method, tool, resource)` action class, but **cannot** override an accumulated deny from any level. |
| **Shape / rate / redaction config** | **Layered narrow-only.** More-specific levels may tighten limits (lower rate, additional redaction paths) but must not widen beyond the intersection of less-specific limits. |

#### Why not pure Override?

Pure override (most-specific wins; less-specific ignored) fails security review:

```yaml
# org/default — allow all tools for employees
- policy_id: org/employee-tools
  cel: jwt.roles.exists(r, r == "employee") && mcp.method == "tools/call"

# tenant/acme — deny destructive tools
- policy_id: tenant/acme-no-delete
  cel: mcp.tool.name.startsWith("delete_")  # effect: deny
```

With override semantics, if the tenant bundle is considered "more specific" and only listed the deny rule, an implementer might **drop** the org allow entirely and accidentally **deny all non-delete tools** — or, if merge order is wrong, the org allow could **widen** past the tenant deny. Hybrid semantics make the intent explicit: **deny OR-tree + allow AND-tree**.

#### Why not pure Layered?

Pure layered (more-specific may only narrow allows) cannot express **break-glass allow** at agent-instance level that overrides a tenant deny for one emergency tool — a legitimate operational need. Hybrid allows a **specific allow** at agent-instance to coexist with broader tenant denies only when no deny rule matches that call (deny still wins when matched).

#### Hybrid merge algorithm (normative)

For each level *L* from most-specific (1) to least-specific (6):

1. Compile all **deny** rules at *L* into `D_L` = OR of their CEL predicates (after same-level conflict resolution, §3).
2. Compile all **allow** rules at *L* into `A_L` = OR of their CEL predicates.

Merged predicate for authorization gate:

```text
ALLOW ⇔ (A_1 ∨ A_2 ∨ … ∨ A_6) ∧ ¬(D_1 ∨ D_2 ∨ … ∨ D_6)
```

Where `A_n` is absent (no allow rules at that level) ⇒ treat as `false` for that disjunct. **Default deny:** if the allow side is entirely false, result is deny regardless of denies.

For **list shaping** (`tools/list`, `prompts/list`, `resources/list`), the same merged predicate is re-evaluated per catalog item with `response.list_filter_index` set.

For **risk scoring** ([adaptive-access.md](adaptive-access.md)), `policy::evaluate_risk` runs **after** the merged allow gate passes; risk thresholds remain in gateway config, not in hierarchical bundles, until Block E wires bundle-sourced risk rules.

#### Examples

**Example A — tenant deny blocks org allow**

| Level | Rule | Effect |
|---|---|---|
| org/default | `jwt.roles.exists(r, r == "engineer")` | allow |
| tenant/acme | `mcp.tool.name == "prod_deploy"` | deny |

Call: engineer invoking `prod_deploy` ⇒ `A = true`, `D = true` ⇒ **deny** (`-32100`).

**Example B — server-level allow does not bypass tenant deny**

| Level | Rule | Effect |
|---|---|---|
| tenant/acme | `mcp.tool.name.startsWith("secret_")` | deny |
| server/github | `mcp.tool.name == "secret_scan"` | allow |

Call: `secret_scan` ⇒ tenant deny matches ⇒ **deny** even though server allow matches.

**Example C — agent-instance allow for on-call tool**

| Level | Rule | Effect |
|---|---|---|
| tenant/acme | `mcp.method == "tools/call"` (broad allow) | allow |
| agent-class/oncall | *(none)* | — |
| agent-instance/responder-7 | `mcp.tool.name == "page_duty"` | allow |

Call: `page_duty` ⇒ **allow** (allow disjunction satisfied, no deny).

**Example D — layered rate narrow-only**

| Level | Config | `max_requests` |
|---|---|---|
| org/default | context throttle | 100 / 300s |
| tenant/acme | context throttle | 50 / 300s |
| server/github | context throttle | 20 / 300s |

Effective limit: **min(100, 50, 20) = 20** per ([adaptive-access.md](adaptive-access.md) throttle key `(tenant, agent_id, purpose)`). Server level must not raise the cap above tenant.

---

### 3. Conflict resolution (same level)

When **multiple policies at the same hierarchy level** disagree (one allow, one deny on the same call), the gateway applies **two-phase resolution**:

#### Phase 1 — Effect precedence: deny-wins

If any **matching** rule at the level has `effect: deny`, the level contributes to the **deny** side (`D_L`) regardless of matching allow rules at that level. Matching allow rules at the same level are ignored for authorization when a matching deny exists.

**Justification:** Same-level conflicts are usually authoring mistakes or emergency overlays. Deny-wins is the only convention that does not require operators to delete an allow before a deny takes effect. It matches [failure-mode-matrix.md](failure-mode-matrix.md) fail-closed posture and A2A gateway tier-1 practice (higher-priority deny rules in `a2a-pack`).

#### Phase 2 — Ordering tie-break: explicit `priority`, then lexicographic `policy_id`

For **multiple denies** (or multiple allows when no deny matches) at the same level:

1. Sort rules by **`priority` descending** (integer, default `0`). Higher priority is evaluated first for audit `rules_fired` ordering.
2. Tie-break equal priorities by **`policy_id` ascending** (UTF-8 lexicographic).

The merged CEL OR/AND tree uses **sorted order** so the composite expression is stable (§4).

**Justification over lexicographic-only:** `policy_id` alone punishes operators who cannot rename policies without changing merge behavior. **`priority`** gives explicit control for "break-glass deny at priority 1000" without renaming `policy_id`.

**Justification over priority-only:** Without lexicographic tie-break, two rules at priority `100` have undefined order — bad for caching and audit diffs.

#### Same-level conflict table

| Situation | Winner | Notes |
|---|---|---|
| Allow + deny both match | **Deny** | Phase 1 |
| Two allows match, no deny | **Allow** (OR) | Either allow is sufficient |
| Two denies match | **Deny** (OR) | Both predicates in `D_L`; audit lists all matching `policy_id`s by priority order |
| Equal priority, different effects, both match | **Deny** | Phase 1 before priority sort matters for outcome |
| Duplicate `policy_id` at same level | **Newest KV revision wins** | Same key overwritten in KV; not a merge conflict |

#### Bundle manifest fields (per rule)

```yaml
rules:
  - policy_id: tenant/acme-no-delete
    priority: 100
    effect: deny   # deny | allow
    cel: |
      mcp.method == "tools/call" &&
      mcp.tool.name.startsWith("delete_")
  - policy_id: tenant/acme-engineers
    priority: 10
    effect: allow
    cel: |
      jwt.roles.exists(r, r == "engineer")
```

---

### 4. Order independence

**Invariant (normative):** For a fixed set of policy bundles `{B_1…B_n}` at fixed KV revisions, the merged CEL expression `M` and its canonical hash `H(M)` are **independent of the order** those bundles were written or observed by the KV watcher.

#### Canonical construction procedure

1. **Collect** applicable bundles by hierarchy level (§1), not by arrival time.
2. **Within each level**, sort rules by `(effect_rank, -priority, policy_id)` where `effect_rank = 0` for deny, `1` for allow (deny listed first in OR tree for stable pretty-print only; outcome is unchanged because deny-wins).
3. **Combine levels** in fixed order `L1 → L6` building `D = ⋁ D_L` and `A = ⋁ A_L` as in §2.
4. **Canonicalize** the expression tree (sort OR/AND children by sub-expression hash) before SHA-256.

Implementations MUST NOT fold bundles in KV notification order.

#### Counter-example: order-dependent merge (violation)

Suppose an incorrect implementation merges by **first-seen wins** at tenant level:

| Publish order | Rule A | Rule B |
|---|---|---|
| 1st | `policy_id: z-allow`, allow `tool == "deploy"` | |
| 2nd | `policy_id: a-deny`, deny `tool == "deploy"` | |

If the implementation stops at the first matching allow and never consults the deny published second, **`deploy` is allowed**.

If publish order is reversed:

| Publish order | Rule A | Rule B |
|---|---|---|
| 1st | `policy_id: a-deny`, deny | |
| 2nd | `policy_id: z-allow`, allow | |

First-match allow might still fire if the engine evaluates allows before denies without deny-wins.

**Fix:** Hybrid + deny-wins + sorted canonical merge ⇒ `deploy` is **always deny** when the deny rule exists, regardless of publish order.

#### Cache key stability

The predicate cache key is:

```text
merge_key = SHA256( canonical_cel(M) || sorted(policy_id:revision pairs) )
```

KV revision numbers are included so hot reload invalidates cache even when CEL text is unchanged but SpiceDB template references changed.

---

### 5. CEL composition

Each policy rule compiles to a CEL predicate `P`. The merged predicate `M` is itself valid CEL, evaluated by the same `cel_interpreter::Program` as Phase 1 hardcoded gate ([policy.rs](../../rsworkspace/crates/trogon-mcp-gateway/src/policy.rs)).

#### Three-policy example

**Context:** `tenant/acme`, server `github`, agent class `oncall-responder`. Three rules:

| policy_id | effect | CEL source |
|---|---|---|
| `org/default-employees` | allow | `jwt.roles.exists(r, r == "employee")` |
| `tenant/acme-block-secrets` | deny | `mcp.tool.name.startsWith("secret_")` |
| `server/github-allowlist` | allow | `mcp.tool.name in ["create_issue", "list_prs"]` |

**Step 1 — classify by level**

- `org/default-employees` → `A_6`
- `tenant/acme-block-secrets` → `D_5`
- `server/github-allowlist` → `A_3`

**Step 2 — combine**

```cel
(
  // Allow side (A_3 ∨ A_6) — only two levels contributed
  (mcp.tool.name in ["create_issue", "list_prs"]) ||
  (jwt.roles.exists(r, r == "employee"))
) &&
!(
  // Deny side (D_5)
  (mcp.tool.name.startsWith("secret_"))
)
```

**Step 3 — evaluation**

| Call | `A` | `D` | Result |
|---|---|---|---|
| `create_issue`, employee | true | false | **allow** |
| `secret_leak`, employee | true | true | **deny** |
| `random_tool`, intern | false | false | **deny** (default deny) |

#### Host builtins in merged expressions

Merged CEL may call host builtins: `spicedb.check`, `rate.acquire`, `time.now`, etc. **Compilation** resolves builtins at merge time; **evaluation** remains on the request hot path.

Rules that invoke `spicedb.check` compile to the same AST shape; the merge tree does not short-circuit SpiceDB calls across rules — the evaluator may apply internal short-circuit on deny for latency once Phase 2 optimizes.

#### Expression hashing for caching

After canonicalization (§4):

```text
canonical_cel = normalize_whitespace(sort_or_and_children(M))
H(M) = SHA256("trogon.mcp.policy-merge/v1\0" || UTF8(canonical_cel))
```

| Field | Purpose |
|---|---|
| `H(M)` | Key in process-local `HashMap<MergeKey, CompiledProgram>` |
| `policy_ids[]` | Ordered list of contributing rule ids + KV revisions for audit |
| TTL | Until any watched KV key revision changes or `mcp.control.bundle.reload` signal |

Example audit fragment:

```json
{
  "policy_merge": {
    "expression_hash": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    "policy_ids": [
      { "id": "server/github-allowlist", "revision": 42, "level": "server" },
      { "id": "tenant/acme-block-secrets", "revision": 17, "level": "tenant" },
      { "id": "org/default-employees", "revision": 3, "level": "org" }
    ],
    "merge_model": "hybrid/v1"
  }
}
```

---

### 6. Storage

Policy bundles distribute through NATS JetStream KV. Binary WASM artifacts (Phase 3) use JetStream Object Store.

#### Primary bucket: `mcp-gateway-config`

| Key pattern | Value | Watcher |
|---|---|---|
| `bundle/active` | Manifest pointer `{ version, digest, signed_at }` | Gateway process — loads full bundle |
| `policy/org/default` | Tier-1 YAML + Tier-2 `.cel` fragments or embedded rules array | Per-level lazy load |
| `policy/tenant/{tenant}` | Tenant override bundle | Filtered by `jwt.tenant` |
| `policy/project/{project_id}` | Project-scoped rules | Filtered by `jwt.project` |
| `policy/server/{server_id}` | Server-scoped rules | Parsed from subject |
| `policy/agent-class/{class}` | Agent class rules | Registry metadata |
| `policy/agent-instance/{agent_id}` | Instance rules | Exact `jwt.agent_id` |
| `trusted_signers` | NKey public keys for bundle signature verification | Startup + watch |
| `inflight.server.{server_id}` | Optional per-server inflight cap | Config overlay |
| `inflight.tenant` | Optional per-tenant inflight cap | Config overlay |

**Hard tenancy:** Each NATS account holds its own `mcp-gateway-config` bucket; tenant segment omitted from keys. **Soft tenancy:** Single cluster bucket; every key above is prefixed `{tenant}/` when the key pattern includes `{tenant}`.

#### Related existing buckets (read-only context)

| Bucket | Used for | Not used for policy merge |
|---|---|---|
| `mcp-sessions` | Session state, ZedToken cache ([failure-mode-matrix.md](failure-mode-matrix.md) row 12) | No |
| `mcp-trust-bundles` | SPIFFE trust anchors ([overview.md](overview.md)) | No |
| `mcp-jwks` | Mesh signing JWKS ([trogon-sts](../../rsworkspace/crates/trogon-sts/src/lib.rs)) | No |
| `mcp-agent-registry` | Agent metadata for class resolution ([registry.md](registry.md)) | Indirect — class lookup only |

#### Fetch pattern on reload

```text
On KV watch / startup / mcp.control.bundle.reload:
  1. Read bundle/active manifest; verify NKey signature (row 4, failure-mode matrix).
  2. For each hierarchy level, GET policy/{level}/… keys matching instance context
     OR preload all tenant keys into level-indexed map (implementation choice).
  3. Merge → compile → insert into predicate cache keyed by H(M) and context fingerprint.
```

**Context fingerprint** (optional secondary cache dimension): `(tenant, project, server_id, agent_id, bundle_generation)` to avoid recomputing merge for every request when only `mcp.params` differ.

#### Registry-assisted resolution

Agent class (level 2) requires `agent_id → agent_class` mapping from `mcp-agent-registry` when not present in JWT. Registry miss ⇒ skip level 2; merge continues with remaining levels ([failure-mode-matrix.md](failure-mode-matrix.md) row 5/6 — not a hard fail unless enforce requires agent identity).

---

### 7. Hot path

#### Warm predicate cache

When `H(M)` hits the process-local cache:

| Operation | Order of magnitude | Notes |
|---|---|---|
| HashMap lookup | **~50–200 ns** | `MergeKey` → `Arc<CompiledProgram>` |
| CEL evaluation (typical rule set) | **~5–50 µs** | Dominated by JWT claim access + string ops |

**Target:** merged policy evaluation (CEL only, no SpiceDB) adds **< 100 µs** P99 on warm cache vs Phase 1 hardcoded gate.

#### Cold path (cache miss or reload)

| Operation | Order of magnitude | Notes |
|---|---|---|
| KV fetch all levels (6 keys) | **~1–5 ms** | Parallel GET; depends on JetStream RTT |
| CEL compile per rule | **~0.5–2 ms** | `Program::compile` per fragment |
| Merge + canonicalize | **~0.1–1 ms** | String/builder work |
| Insert cache | **~1 µs** | |

**Cold merge + compile (10 rules):** roughly **5–25 ms** once per `(context fingerprint, bundle_generation)` — acceptable on reload or first request after deploy; must **not** run synchronously on every request.

#### Recommended implementation shape

1. **Background task:** KV watcher triggers merge+compile; swaps cache generation atomically.
2. **Request path:** read current generation + lookup; on miss, await in-flight compile (singleflight) with deadline — on timeout, **CLOSED** `-32101 policy_fault` rather than stale allow.

---

### 8. Audit

Every audit envelope for gated methods MUST include sufficient policy-merge context to reproduce denies.

#### Required fields (extension to `trogon.mcp.audit/v1`)

| Field | Type | When |
|---|---|---|
| `policy_merge.expression_hash` | string (`sha256:…`) | Always when hierarchical merge is active |
| `policy_merge.policy_ids` | array of `{ id, revision, level }` | Always — lists **all contributing rules** whose predicates were part of `M`, not only the deny that fired |
| `policy_merge.merge_model` | string | `"hybrid/v1"` |
| `rules_fired` | array of string | Subset of `policy_id` that matched this request |

On **deny** (`decision: deny`, `-32100 policy_deny`):

```json
{
  "schema": "trogon.mcp.audit/v1",
  "decision": "deny",
  "method": "tools/call",
  "tool": "secret_leak",
  "rules_fired": ["tenant/acme-block-secrets"],
  "policy_merge": {
    "expression_hash": "sha256:…",
    "merge_model": "hybrid/v1",
    "policy_ids": [
      { "id": "tenant/acme-block-secrets", "revision": 17, "level": "tenant" },
      { "id": "org/default-employees", "revision": 3, "level": "org" }
    ]
  },
  "error": {
    "code": -32100,
    "reason": "policy_deny: tenant/acme-block-secrets"
  }
}
```

**Reproduction procedure for reviewers:**

1. Fetch listed `policy_id` + `revision` from KV history (JetStream revision store).
2. Re-run canonical merge algorithm (§4) offline.
3. Verify `H(M)` matches `expression_hash`.
4. Evaluate `M` with the call context from the envelope (`jwt`, `mcp.*`, `nats.subject`).

Forward compatibility: consumers MUST tolerate unknown `policy_merge` subfields.

---

### 9. Failure modes

| # | Condition | Default behavior | Merge proceeds? | JSON-RPC | Operator action |
|---|---|---|---|---|---|
| F1 | **Policy missing at a level** | Skip level; merge with remaining | Yes (partial) | Deny only if no allow side and default deny | Publish missing bundle or accept narrower posture |
| F2 | **All levels empty** | CLOSED | N/A | `-32108 no_policy` | Seed org or tenant bundle ([bootstrap-day-zero.md](bootstrap-day-zero.md)) |
| F3 | **Conflicting `priority` values** (same effect) | Both kept; sort by priority then `policy_id` | Yes | N/A at merge time | Fix authoring if surprised by audit order |
| F4 | **Same `priority`, allow vs deny both match** | Deny-wins (§3) | Yes | `-32100` if deny matches | Intentional — raise deny priority for clarity |
| F5 | **CEL compile failure at one level** | **Refuse merge for that bundle generation** | **No** — entire generation rejected | Instance unhealthy; requests: `-32101 policy_fault` if no prior generation | Fix CEL; rollback KV revision ([failure-mode-matrix.md](failure-mode-matrix.md) row 6) |
| F6 | **CEL runtime error evaluating `M`** | CLOSED | N/A (eval time) | `-32101 policy_fault` | Same as row 7, failure-mode matrix |
| F7 | **Invalid bundle signature** | Keep **last good** generation | No adoption of bad bundle | Prior generation continues | Republish signed bundle |
| F8 | **KV unavailable during reload** | Continue with **last good** cache | Stale until TTL/health fail | If no cache: `-32101` or `-32108` | Restore JetStream |
| F9 | **Registry miss for agent-class** | Skip level 2 | Yes (degraded) | Depends on remaining rules | Register agent class |
| F10 | **Merge singleflight timeout** | CLOSED | No fresh cache | `-32101 policy_fault` | Scale gateway; reduce rule count |

**Degraded vs refuse summary:**

- **Missing level ⇒ degraded skip** (merge proceeds without that level).
- **Compile failure ⇒ refuse** (do not serve partially compiled policy — prevents half-applied denies).
- **Runtime fault ⇒ refuse request** (failClosed).

Aligns with [failure-mode-matrix.md](failure-mode-matrix.md) invariants 1, 8, 9.

---

### 10. Open questions

#### 10.1 Negative inheritance (organisation FORCE deny)

**Question:** Should organisation default be able to publish **non-overridable** denies that tenant/project/server cannot relax?

**Options:**

| Option | Pros | Cons |
|---|---|---|
| A. No force — hybrid only | Simple; tenants retain autonomy | Regulated industries may require org-wide blocks |
| B. `effect: deny, force: true` flag | Explicit audit trail | Complicates merge — must short-circuit all allows |
| C. Separate `org/immovable` level merged after all allows | Clear precedence story | Another level to document |

**Tentative recommendation:** defer to Phase 3; use SpiceDB org-wide tuples for non-overridable denies until flag semantics are proven in production.

#### 10.2 Time-bounded overrides

**Question:** How do temporary allows/denies (`valid_until`) interact with merge cache?

**Considerations:**

- Rules with `valid_until` must participate in `H(M)` — cache key includes expiry bucket or rules split into static + dynamic eval.
- Expired rules removed from merge on next periodic refresh (60s) even without KV change.
- Audit must record `valid_until` on fired rules.

**Tentative recommendation:** store time bounds in rule metadata; evaluator checks `time.now` inside CEL (`time.now < timestamp("2026-06-01T00:00:00Z")`) so merge tree stays static; document clock skew tolerance (±30s).

#### 10.3 Dry-run mode

**Question:** Should operators preview effective policy for a synthetic context without executing the call?

**Shape:**

- CLI: `trogon-mcp-gateway policy explain --tenant acme --agent acme/bot --server github --tool deploy`
- Returns merged CEL (pretty), `H(M)`, matching `rules_fired`, and allow/deny verdict.
- Never publishes to backend; no audit to `MCP_AUDIT` unless `--emit-audit`.

**Tentative recommendation:** Block E implementation follow-up; paper spec ensures merge is deterministic so dry-run is reproducible.

#### 10.4 Project claim trust

**Question:** Is `jwt.project` self-asserted or STS-issued from registry membership?

**Risk:** Self-asserted project claim could widen access if project-level allows exist.

**Tentative recommendation:** enforce mode requires project claim minted by STS from registry membership graph; shadow mode logs `project_unverified`.

#### 10.5 Virtual MCP federation

**Question:** For `virtual-default::github::create_issue`, is server level `virtual-default` or `github`?

**Tentative recommendation:** **member server** (`github`) for policy merge; virtual id affects routing only.

---

## Implementation checklist (follow-up track)

This document is paper-only. Code changes belong in a separate PR:

- [ ] `trogon-policy-core` merge crate with canonical CEL builder
- [ ] KV watcher for per-level keys under `mcp-gateway-config`
- [ ] Extend audit publisher with `policy_merge` block
- [ ] Wire merged predicate into `policy::run_with_risk` before SpiceDB gate
- [ ] `tools/list` re-evaluation uses same `M`
- [ ] Metrics: `policy_merge_cache_hit`, `policy_merge_compile_ms`

---

## Summary table

| Topic | Decision |
|---|---|
| Hierarchy (specific → general) | Agent-instance → Agent-class → Server → Project → Tenant → Organisation default |
| Empty fallback | **Deny** (`-32108 no_policy`) in production |
| Merge model | **Hybrid** — denies OR across levels; allows OR with most-specific contributing; default deny |
| Same-level conflict | **Deny-wins**; then **priority desc**, then **`policy_id` asc** |
| Order independence | Canonical sort by level + priority + `policy_id`; hash includes revisions |
| CEL | Single merged expression `M`; cached by `H(M)` |
| Storage | **`mcp-gateway-config`** KV; related: `mcp-sessions`, `mcp-agent-registry`, `mcp-trust-bundles`, `mcp-jwks` |
| Hot path | Warm: **~50–200 ns** lookup + **~5–50 µs** eval; cold compile: **~5–25 ms** |
| Audit | **`policy_ids` + `expression_hash`** required |
| Compile failure | **Refuse** generation; keep last good |
| Missing level | **Skip** (degraded merge) |

# ADR 0027: Schema-Driven Redaction Strategy

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | Schema-driven redaction; unblocks native Tier-1 redaction on `tools/call` / `resources/read` forward and callback paths, audit `rewrites` attestation (audit envelope schema), and acceptance tests in `redaction_rules.rs` |
| **Related** | ADR 0023 (schema cache source) · ADR 0013 (hierarchical policy merge) · ADR 0015 (`tools/list` descriptor redaction) · ADR 0020 (bidirectional enforcement) · [reference-audit-envelope.md](../identity/reference-audit-envelope.md) · [tools-list-filtering.md](../identity/tools-list-filtering.md) §5 · [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md) · `rsworkspace/crates/trogon-mcp-gateway/tests/redaction_rules.rs` · `rsworkspace/crates/trogon-mcp-gateway/src/redaction/` |

## Context

`trogon-mcp-gateway` must rewrite JSON-RPC `params` (inbound) and `result` (outbound) before those payloads cross trust boundaries — toward backend servers, toward edge clients, and into JetStream audit consumers. Generic regex or LLM-shaped guardrails (agentgateway-style) cannot express MCP-native sensitivity: field names, nesting, and array shapes come from each tool's `inputSchema` / `outputSchema`, not from free-text heuristics.

Block E item 5 pins the first implementation as **native Rust**, not WASM, with rules **attached to schemas** (`MCP_GATEWAY_PLAN.md` Block E item 5; § Redaction). ADR 0023 supplies the canonical schema bytes via `SchemaCacheKey { server_id, schema_hash }` populated from `tools/list` sniff and miss-path fetch. ADR 0015 already applies descriptor redaction on kept catalogue entries after CEL filtering; this ADR closes the **`tools/call` / response / notification** contract that the integration scaffold in `tests/redaction_rules.rs` expects.

**Why this decision is needed now**

| Gap without ADR | Impact |
|-----------------|--------|
| No primary rule format | Bundle authors cannot ship redaction; `RedactionRuleset::from_yaml` stays a stub |
| No pipeline slot | Redaction might run inside CEL (forbidden — CEL is policy, not transformation) or after audit (PII leak) |
| No verb semantics | `hash` vs `mask` vs `drop` behavior is ambiguous for replay tests and SIEM |
| No fail-closed rule | Schema cache miss could forward plaintext secrets (`-32104` tests block on this) |
| No audit attestation | Pin 7 `rewrites` cannot be specified; compliance cannot prove what was redacted |

**Current branch snapshot**

- `trogon_mcp_gateway::redaction` exports `JsonPath`, `RedactionAction` (`Mask`, `Hash`, `Drop`, `Replace(String)`), `RedactionRule`, `RedactionRuleset`, and `redact()` — engine is a no-op stub (`engine.rs`).
- `tests/redaction_rules.rs` defines the acceptance matrix (`hash`, `drop`, `mask`, bidirectional paths, nested JSONPath, deterministic order, `-32104 schema_unknown`, audit `rewrites`) — all `#[ignore]`.
- `docs/identity/reference-audit-envelope.md` §11 documents redaction module placement; `rewrites` on wire is **proposed** in plan samples, not yet in `AuditEnvelope` struct.

---

## Decision

**Adopt sidecar JSONPath rule lists in policy bundles as the primary redaction rule format**, validated against cached MCP JSON Schemas from ADR 0023. **Redaction is a dedicated transformation stage after CEL authorization and SpiceDB checks and before backend/client publish and audit emission.** Rules merge hierarchically under ADR 0013 narrow-only semantics. Five verbs apply at the JSON value level: `drop`, `mask`, `hash`, `tokenize`, `passthrough`. When a tool has configured redaction rules and the schema cache cannot supply a validating schema, the gateway **fail-closes** with JSON-RPC `-32104` / `schema_unknown`. Audit envelopes record **field paths and operations only** — never pre-redaction plaintext — plus optional deterministic digests for `hash` rules.

**Redaction does not run in CEL.** CEL and SpiceDB answer allow/deny; the redaction engine mutates JSON documents. Host builtins such as `jsonpath.set` in CEL are shaping helpers for policy authors, not the hot-path redactor (`docs/identity/policy-dsl-choice.md`). Bundle `redaction:` YAML is interpreted by `trogon_mcp_gateway::redaction`, not compiled into CEL expressions.

### Primary rule format: sidecar JSONPath in bundle YAML

Rules live in the policy bundle under a top-level `redaction:` section (distinct from CEL `rules:`). Each rule is a JSONPath string plus an action verb. Paths are evaluated against the **full JSON-RPC body fragment** being rewritten (`params` object on requests, `result` on responses, notification `params` on either direction).

```yaml
redaction:
  global:
    rules:
      - path: "$.params.connection_string"
        action: hash
  tools:
    db_query:
      request:
        - path: "$.params.connection_string"
          action: hash
      response:
        - path: "$.result.rows[*].ssn"
          action: mask
    search_repo:
      request:
        - path: "$.params.token"
          action: hash
```

Wire format for `action` in YAML matches integration tests: scalar string (`hash`, `mask`, `drop`, `tokenize`, `passthrough`) or structured form `{ mask: {} }` / `{ hash: {} }` once `RedactionRuleset::from_yaml` lands (`ruleset.rs`).

JSON Schema **`x-trogon-redact`** annotations on cached schema properties are **not** the primary authoring surface for Block E. They may be compiled into sidecar rules at bundle load time in a future pass (Open questions).

### Rule resolution and hierarchical merge

Redaction config merges across the same hierarchy as authorization ([ADR 0013](0013-hierarchical-policy-merge.md), [hierarchical-policy-merge.md](../identity/hierarchical-policy-merge.md)) with **narrow-only** semantics for config overlays:

| Layer | Bundle key **(proposed)** | Effect |
|-------|---------------------------|--------|
| Organisation | `redaction.global` | Baseline paths for all tools on all servers in scope |
| Tenant | `redaction.tenants.{tenant_id}` | Additional paths; cannot remove org-required redactions |
| Server group | `redaction.groups.{group_id}` | Cohort-specific paths |
| Server | `redaction.servers.{server_id}` | Backend-specific tightening |
| Tool | `redaction.tools.{tool_name}.request` / `.response` | Per-tool paths (native tool name after federated resolution) |
| Notification | `redaction.notifications.{method_path}` **(proposed)** | e.g. `notifications.progress` message fields |

**Effective ruleset algorithm (normative)**

```text
collect applicable layers L_org … L_tool in specificity order (general → specific)
for each layer L:
    append L.rules to accumulator (never delete upstream paths)
dedupe by path: more-specific layer wins on same JSONPath string
final order: stable sort by (layer_rank asc, yaml_declaration_order asc)
```

Properties:

- More-specific layers **may add** paths and **may override the verb** on the same path (last wins in walk order).
- More-specific layers **must not remove** a path present at a less-specific layer (narrow-only — cannot widen egress by deleting upstream hash rules).
- Per-tool `request` and `response` rule lists are independent; both may fire on one `tools/call` round-trip (`tests/redaction_rules.rs` `bidirectional_redaction` module).
- Catalogue descriptor redaction ([tools-list-filtering.md](../identity/tools-list-filtering.md) §5.3) uses `global` + `tools.{name}` on the **tool descriptor JSON**, not `request`/`response` keys — same engine, different attachment surface.

Tenant overlay example: org hashes `$.params.token`; tenant adds `$.params.internal_note` drop — both apply. Tenant cannot change org's `hash` on `$.params.token` to `passthrough`.

### Redaction verbs (JSON value semantics)

All verbs mutate the document **in place** via `redact(&mut serde_json::Value, &RedactionRuleset)` (`engine.rs`). Application is **deterministic**: same input JSON bytes + same effective ruleset + same tenant salt → identical output bytes (canonical JSON serialization pinned at implementation time for replay tests).

| Verb | Applies to | JSON effect | Absent path in payload | Audit `op` |
|------|------------|-------------|------------------------|------------|
| **`drop`** | Object fields only | Remove matched key from parent object; key **must not** appear as `null` | No-op; rule not listed in `rewrites` | `drop` |
| **`mask`** | String scalars (and string-coercible **(proposed)**) | Replace value with literal `"***"` | No-op if path unmatched | `mask` |
| **`hash`** | String / number / bool scalars | Replace with `sha256:` + 64 lowercase hex of `HMAC-SHA256(tenant_salt, canonical_scalar_bytes)` **(proposed)** | No-op if path unmatched | `hash` |
| **`tokenize`** | String scalars | Replace with `tok:` + opaque id; maintain tenant-scoped reversible map in gateway KV **(proposed)** `mcp.redaction.tokens.{tenant}` | No-op if path unmatched | `tokenize` |
| **`passthrough`** | Any matched type | Leave value unchanged; records intent in audit only | No-op if path unmatched | `passthrough` |

Additional rules:

- **`drop`** on array elements: remove element from array when path selects `[*]` matches — preserve array order for remaining elements.
- **`mask`** on non-string scalars: **(proposed)** stringify then mask, or skip with bundle-load warning — default skip at load time.
- **`hash`** salt: `tenant_id` + bundle `policy_version` + static deployment pepper from env `MCP_REDACTION_HASH_PEPPER` **(proposed)** — never a global salt across tenants.
- **`tokenize`** keys: scoped to `(tenant_id, server_id)`; token map entries TTL-bound **(proposed)** `MCP_REDACTION_TOKEN_TTL_SECS`; never shared across tenants.
- **`Replace(String)`** in existing `RedactionAction` enum: alias for `mask` with custom literal **(proposed)** — prefer explicit `mask` + optional `pattern` field in YAML v2.

Multi-rule order: rules run in **effective ruleset slice order** (`RedactionRuleset::rules()`); overlapping paths use declaration order documented in bundle YAML (`tests/redaction_rules.rs` `rule_application_order` module).

### Schema binding and validation

Every redaction path for a tool **must be validated** against the cached JSON Schema for the active direction:

| Direction | Schema source | Validated fragment |
|-----------|-----------------|-------------------|
| Request (`tools/call`, `resources/read`, callback ingress) | `inputSchema` from `SchemaCache` | `params` shape (tool arguments object) |
| Response | `outputSchema` when present; else infer from cached tool definition **(proposed)** | `result` shape |
| `tools/list` descriptor | Cached `inputSchema` on tool object | Tool descriptor subtree |

Validation steps on `tools/call`:

1. Resolve `server_id`, native tool name, and `schema_hash` hint from call params + catalogue.
2. `SchemaCache::get(SchemaCacheKey { server_id, schema_hash })` per ADR 0023.
3. If tool has **any** configured redaction rule for this direction and step 2 misses (including fetch failure): return `-32104` / `schema_unknown` with `{ trace_id, server_id, tool }` — do **not** forward to backend (`tests/redaction_rules.rs` `schema_cache_gate`).
4. If schema present: verify each rule path resolves to a **declared** location in the schema graph (property exists in `properties`, `items`, or `additionalProperties` path). Path not in schema → **fail closed** at **bundle load** when schema snapshot is embedded; at **runtime** if schema drifted → `-32104` or **(proposed)** `-32118` `redaction_path_invalid` with audit deny.
5. If path is in schema but **absent from payload** (optional field omitted): skip rule — no rewrite, no audit entry (`audit_rewrites_omits_rules_that_did_not_match`).

Tools with **no** redaction rules for a direction may forward without schema cache on that direction (redaction optional); schema cache still required when any rule exists.

### Pipeline placement (relative to CEL, SpiceDB, audit)

Redaction runs **after** authorization succeeds and **before** publish on each egress leg. CEL + SpiceDB never mutate payloads.

#### Request direction (`mcp.gateway.request.*`)

```text
1. parse JSON-RPC
2. resolve identity / session
3. derive resource tuple
4. CEL gate + SpiceDB CheckPermission          ← policy only; no redaction
5. resolve schema (SchemaCache, ADR 0023)
6. validate effective RedactionRuleset vs schema
7. redact request params (inbound / toward backend)
8. publish mcp.server.{server_id}.{method}
9. await backend reply
10. redact response result (outbound / toward client)
11. optional tools/list CEL shape (ADR 0015)   ← if */list; before or after output redact per list path
12. emit audit envelope (Pin 7) with rewrites
13. reply to client inbox
```

Matches `MCP_GATEWAY_PLAN.md` lifecycle steps 4–5 (input) and 9–10 (output + audit), with explicit schema resolution inserted before step 7.

#### Callback direction (`mcp.client.*` → `mcp.gateway.callback.*`, ADR 0020)

| Leg | Redaction target | When |
|-----|------------------|------|
| Server → gateway (ingress) | Callback `params` | After callback CEL + SpiceDB allow; before edge publish |
| Client → gateway (reply) | Callback `result` | After client reply received; before server inbox relay |

`notifications/progress` and `notifications/cancelled`: treat `params` as request-direction payload; apply `redaction.notifications.{method}` rules when configured; audit with `direction: "callback"` and `is_notification: true`.

#### Denied requests

When step 4 denies: **skip** redaction steps 5–7; emit audit without `rewrites`; do not forward.

#### List catalogue path (ADR 0015)

For each **kept** tool after CEL filter: run descriptor redaction (`global` + per-tool rules on tool JSON) before appending to `result.tools`. Authorization CEL runs first; redaction never substitutes for a deny.

### Failure modes

| Condition | Behavior | Client error | Audit |
|-----------|----------|--------------|-------|
| Schema cache miss + tool has redaction rules | Fail closed; no backend forward | `-32104` `schema_unknown` | `outcome: deny`, **(proposed)** `decision_reason: schema_unknown` |
| Rule path not in cached schema | Fail closed on call; reject at bundle load if detectable offline | **(proposed)** `-32118` `redaction_path_invalid` | Deny + operator alert |
| Redaction engine internal error | Fail closed | `-32101` `policy_fault` **(proposed)** tier `redaction` | Error envelope |
| Tool has no redaction rules | Passthrough; schema optional | — | No `rewrites` |
| `schema_cache_enabled=false` (ADR 0023 rollback) + rules present | Direct schema fetch on miss; same fail-closed if fetch fails | `-32104` when fetch fails | Same |

**Rejected for Block E:** silent skip of undeclared paths when rules are configured (would violate deterministic audit story); fail-open forward on schema miss (tests require block).

### Audit envelope attestation (Pin 7)

Every **allowed** forward that runs redaction includes a `rewrites` array on the audit envelope published to `{prefix}.audit.{outcome}.{direction}.{method_root}` (`MCP_GATEWAY_PLAN.md` § Audit, Wire-Format Pin 7).

| Field | Requirement |
|-------|-------------|
| `rewrites[]` | Ordered list of `{ "path": "<jsonpath>", "op": "<verb>" }` for rules whose paths matched in the document |
| Pre-redaction values | **MUST NOT** appear in audit JSON |
| `hash` attestation | **(proposed)** optional `{ "path", "op": "hash", "digest": "sha256:…" }` — digest only, not reversible secret |
| `tokenize` attestation | `{ "path", "op": "tokenize" }` only — token map stays in gateway KV, not audit |
| `rules_fired` | **(proposed)** include stable rule set ids e.g. `redact-db-query` alongside CEL policy ids |
| Merge with route rewrites | Subject rewrite metadata and redaction `rewrites` coexist in one envelope (`audit_rewrites_merge_with_subject_route_rewrites`) |
| Masked plaintext | Audit body after redaction pass for nested payload copies — masked fields show `"***"` only if payload snapshot included **(proposed)** `payload_fingerprint` hash of redacted body, not raw |

Denied `-32104` responses: emit deny audit with `decision_reason: schema_unknown`; `rewrites` empty.

Separate sampling subject `mcp.audit.rewrite.callback.sampling` (`MCP_GATEWAY_PLAN.md` control-plane table) is **optional** for rewrite-only telemetry — default single envelope per message (Open questions).

---

## Consequences

### Positive

- **MCP-native differentiator:** Redaction keys off real `inputSchema` / `outputSchema` shapes cached by ADR 0023 — not regex tables — matching the product story vs agentgateway.
- **Deterministic replay:** Fixed verb semantics and rule order enable byte-stable regression tests and audit hash chains.
- **Separation of concerns:** Policy (CEL/SpiceDB) and transformation (redaction) are independently testable; CEL authors cannot accidentally exfiltrate via `audit.emit` on raw secrets before redaction.
- **Composable operator posture:** Hierarchical narrow-only merge lets org baseline hash tokens while tenants add drops without forking bundles.
- **Audit-safe by default:** `rewrites` attest paths and ops without storing secrets; aligns with [reference-audit-envelope.md](../identity/reference-audit-envelope.md) §11 trust boundary.

### Negative

- **Schema cache hard dependency:** Any tool with redaction rules blocks on cache miss — extra `-32104` noise until warm `tools/list` or pre-warm lands (ADR 0023 Open questions).
  - **Mitigation:** Encourage `tools/list` before call bursts; monitor `mcp_schema_cache_hits_total`; operator pre-warm flag.
- **Bundle authoring burden:** JSONPath rules must align with schema graphs; drift after `list_changed` invalidates paths until bundles update.
  - **Mitigation:** Bundle load validation against pinned schema snapshots; dry-run CLI (Block G); invalidate on ADR 0023 control broadcast.
- **Tokenize operational cost:** Reversible tokenization requires tenant-scoped KV and TTL janitor — more moving parts than hash/mask.
  - **Mitigation:** Block E ships `hash`/`mask`/`drop` first; `tokenize` gated behind `redaction_tokenize_enabled` **(proposed)** default false.
- **Dual attachment surfaces:** Catalogue descriptor redaction (ADR 0015) and call-path `request`/`response` rules share one engine but different YAML keys — authors must learn both.
  - **Mitigation:** Document in bundle examples; `trogon-gateway-ctl validate` **(proposed)** cross-checks paths against schema cache export.

### Neutral

- **WASM Tier 3 deferred:** Block E native Rust matches plan "first pass as native code"; custom classifiers remain Phase 3 (`MCP_GATEWAY_PLAN.md` § Tier 3).
- **`Replace` enum variant:** Existing scaffold keeps `Replace(String)`; YAML v1 uses `mask` for literal stars per tests.
- **outputSchema:** Response redaction accepts missing `outputSchema` with inferred paths **(proposed)** — strict mode may require explicit schema later.

---

## Rejected alternatives

### JSON Schema `x-trogon-redact` annotations as primary authoring format

Embed redaction verbs directly on schema properties:

```json
"token": { "type": "string", "x-trogon-redact": "hash" }
```

| Assessment | |
|--------------|---|
| **Pros** | Colocated with field definitions; survives schema export; natural for MCP vendors publishing schemas. |
| **Cons** | MCP spec does not standardize `x-trogon-redact`; operators cannot tighten redaction without mutating backend-published schemas; merge across org/tenant/server layers is awkward inside schema blobs; bundle diff review mixes policy with schema content. |
| **Verdict** | **Rejected as primary** for Block E. Sidecar YAML is authoritative; annotations may compile to rules later (Open questions). |

### Regex / PII classifier guardrails (agentgateway-style)

Apply pattern tables independent of MCP schemas.

| Assessment | |
|--------------|---|
| **Pros** | Works without schema cache; catches unknown fields. |
| **Cons** | High false positive rate on structured tool args; not tied to `inputSchema`; duplicates agentgateway without MCP differentiation; non-deterministic ML classifiers conflict with replay tests. |
| **Verdict** | **Rejected** as primary. Optional WASM Tier 3 classifier may supplement schema paths in Phase 3. |

### Redaction inside CEL (`jsonpath.set` on hot path)

Evaluate redaction as CEL side effects during policy match.

| Assessment | |
|--------------|---|
| **Pros** | Single language for operators; conditional redaction in same expression as allow. |
| **Cons** | Violates separation of policy vs transformation; impure CEL breaks list-filter purity guarantees (ADR 0015); audit ordering ambiguous; harder to prove determinism. |
| **Verdict** | **Rejected.** CEL may *reference* redaction outcomes in future read-only builtins; mutation stays in `redaction::redact`. |

### Fail-open on schema cache miss

Forward plaintext when schema unavailable; log warning only.

| Assessment | |
|--------------|---|
| **Pros** | Higher availability; fewer `-32104` errors. |
| **Cons** | Violates fail-closed security posture; integration tests explicitly require block; secrets reach backend unredacted. |
| **Verdict** | **Rejected.** `-32104` fail-closed when rules exist and schema missing. |

### Skip invalid paths silently at runtime

When a rule path is not in schema, omit that rule and continue.

| Assessment | |
|--------------|---|
| **Pros** | Resilient to minor schema drift. |
| **Cons** | Operator may believe field is redacted when it is not; audit `rewrites` omits silent skips — compliance gap. |
| **Verdict** | **Rejected** for configured rules on validated schema. Optional fields **absent from payload** still skip (not the same as invalid path). |

---

## Open questions

1. **Compile `x-trogon-redact` at bundle load?** Should bundle compiler walk cached schemas and emit sidecar rules from annotations — deduping with explicit YAML? Deferred ADR topic.
2. **`outputSchema` mandatory for response rules?** Strict enforce mode may require `outputSchema` on every tool with response redaction vs infer-from-example **(proposed)**.
3. **Separate audit subject for rewrite-only events?** `mcp.audit.rewrite.callback.sampling` vs single envelope — sampling volume may need split stream.
4. **`-32118 redaction_path_invalid` code allocation** — register in `reference-error-codes.md` and `rpc_codes.rs` or fold into `-32104`.
5. **Tokenize KV backend** — JetStream KV vs in-memory with replication; cross-replica token resolve on queue-group stickiness.
6. **Progress notification high-volume redaction** — shed rules under load or sample audit `rewrites` for `notifications/progress` bursts (ADR 0020 latency note).
7. **WASM redaction component** — when to migrate verbose rule tables to Tier 3 (`MCP_GATEWAY_PLAN.md` Phase 3) without changing verb semantics.
8. **Shadow redaction mode** — `reference-error-codes.md` proposes observability-only redaction without schema validation; pin semantics in failure-mode ADR follow-up.
9. **Multi-tenant `SchemaCacheKey`** — tenant in cache key (ADR 0023 Q3) affects which schema validates redaction paths under soft tenancy.

---

## Rollback plan

| Control | Behavior |
|---------|----------|
| **`redaction_enabled=false` **(proposed)** | Gateway skips steps 6–7 and 10; forwards params/result verbatim; audit omits `rewrites`; CEL/SpiceDB unchanged. |
| **`schema_cache_enabled=false`** (ADR 0023) | Direct schema fetch on miss; redaction still enforced when rules exist — rollback of cache only, not redaction. |
| **Bundle pointer rollback** | Hot-swap to prior bundle `policy_version` with known-good `redaction:` section ([MCP_GATEWAY_PLAN.md] § Bundles). |
| **Verb-level disable** | **(proposed)** `redaction_disable_verbs: [tokenize]` without disabling hash/mask/drop. |
| **Emergency passthrough** | Per-tenant `redaction_enforcement: audit` **(proposed)** — compute `rewrites`, audit would-have-redacted, forward plaintext — break-glass with operator ticket; not default. |

Disabling redaction does not remove schema cache or CEL gates. Re-enable starts cold — no retroactive token map recovery for `tokenize` unless KV retained.

---

## Implementation notes

### Code surfaces

| Surface | Responsibility |
|---------|----------------|
| `src/redaction/engine.rs` | Implement `redact()` — JSONPath walk, verb application, `rewrites` trace return type **(proposed)** `RedactionOutcome` |
| `src/redaction/rule.rs` | Add `Tokenize`, `Passthrough` to `RedactionAction`; map YAML strings |
| `src/redaction/ruleset.rs` | `from_yaml` via serde_yaml; hierarchical merge helper **(proposed)** `EffectiveRuleset::resolve(context)` |
| `src/redaction/schema_validate.rs` **(proposed)** | Path-in-schema checker against `CachedSchema` |
| `src/schema_cache/` | ADR 0023 lookup before redaction (existing) |
| `src/gateway.rs` | Insert redaction at steps 7 and 10; map `-32104` on miss |
| `src/audit.rs` | Extend `AuditEnvelope` with `rewrites: Vec<RewriteEntry>` **(proposed)** |
| Policy bundle loader | Parse `redaction:` tree; merge per ADR 0013 |
| `tests/redaction_rules.rs` | Remove `#[ignore]` as behavior lands |

### Acceptance tests (from scaffold)

| Module | Behavior pinned |
|--------|-----------------|
| `hash_redaction` | Stable digest; audit `{path, op: hash}` |
| `drop_redaction` | Key absent, not null |
| `mask_redaction` | Literal `"***"`; no plaintext in audit |
| `bidirectional_redaction` | Request + response on one `tools/call` |
| `schema_cache_gate` | `-32104`; no backend forward |
| `nested_jsonpath` | Deep paths |
| `rule_application_order` | YAML order stable across requests |
| `audit_rewrites_envelope` | Order, wire format, merge with route metadata |

### Observability **(proposed)**

| Metric | Labels | When |
|--------|--------|------|
| `mcp_redaction_rules_applied_total` | `server_id`, `tool`, `direction`, `op` | Each matched rule |
| `mcp_redaction_schema_unknown_total` | `server_id`, `tool` | `-32104` |
| `mcp_redaction_duration_us` | `direction` | Engine wall time |

---

## Status of supporting work

| Item | Status |
|------|--------|
| ADR 0027 (this document) | **Done** |
| `redaction` scaffold types | **Done** (branch snapshot) |
| `redact()` engine | **Pending** (stub) |
| `from_yaml` | **Pending** (serde_yaml) |
| Schema validation gate | **Pending** |
| Gateway pipeline wiring | **Pending** (Block E) |
| Audit `rewrites` field | **Pending** |
| Integration tests | **Scaffold** (`#[ignore]`) |
| Close `MCP_GATEWAY_PLAN.md` Block E item 5 checkbox | **Pending** (editorial after implementation) |

---

*Contract sources: `MCP_GATEWAY_PLAN.md` Block E item 5, § Redaction, § Gateway lifecycle, Wire-Format Pin 7, JSON-RPC `-32104`; `rsworkspace/crates/trogon-mcp-gateway/tests/redaction_rules.rs`; `rsworkspace/crates/trogon-mcp-gateway/src/redaction/`; `docs/identity/reference-audit-envelope.md` §11; `docs/identity/tools-list-filtering.md` §5; ADR 0023, ADR 0013, ADR 0015, ADR 0020.*

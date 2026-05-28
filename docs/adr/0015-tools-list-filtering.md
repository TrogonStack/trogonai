# ADR 0015: Per-Item CEL Filtering for Catalog List Responses

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-28) |
| **Date** | 2026-05-28 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block E item 2 (`tools/list` filtering via CEL re-evaluation); downstream Block E catalog shaping for `prompts/list` and `resources/list` |
| **Related** | `docs/identity/tools-list-filtering.md` (design spec); `docs/identity/bulk-check-permission.md`; `docs/identity/reference-cel-variables.md`; `docs/identity/hierarchical-policy-merge.md`; ADR 0014 (ZedToken / BulkCheckPermission cache); `MCP_GATEWAY_PLAN.md` Block E item 2, § MCP Authorization |

## Context

MCP clients discover capabilities through catalog list methods: `tools/list`, `resources/list`, and `prompts/list`. Each successful JSON-RPC reply carries an array of descriptors (`tools`, `resources`, or `prompts`) that the client treats as the action space for subsequent calls.

Phase 1 of `trogon-mcp-gateway` forwards list responses from the backend **verbatim**. The gateway does not run SpiceDB or CEL shaping on `tools/list`; `policy.rs` excludes list methods from the gated-method set, and `dispatch_backend_response` publishes the backend payload unchanged. An agent authorized for three tools therefore receives the server's full catalogue -- often dozens of entries -- including names, descriptions, and JSON Schemas for tools it cannot invoke.

**Why the unfiltered catalogue is unacceptable**

| Risk | Mechanism |
|------|-----------|
| **Information leak** | Tool names, descriptions, and `inputSchema` reveal internal APIs, data models, and privileged operations. Least-privilege requires that unauthorized entries not appear in the catalogue at all. |
| **Agent confusion** | LLM clients treat list output as the complete action space. Irrelevant entries increase mistaken `tools/call` attempts, wasted tokens, and false confidence about capabilities. |
| **Policy inconsistency** | Block E requires the **same authorization intent** that gates `tools/call` to decide visibility in `tools/list`. Today list and call are decoupled; agents discover denied tools only through call-time failures. |
| **Audit blind spot** | Phase 1 audit records method and outcome but not how many catalogue entries were hidden. Operators cannot prove least-privilege catalogue shaping. |

**agentgateway pattern**

agentgateway's `mcpAuthorization` policy is the reference design (`MCP_GATEWAY_PLAN.md` § MCP Authorization). A single `rules:` list of CEL expressions with **OR semantics** gates mutating methods. The same rule set **automatically filters `list_tools` responses**: the engine re-evaluates each rule against every candidate item with `mcp.tool.name` bound to that item, and drops items that do not match. The Trogon gateway adopts this pattern directly rather than a separate `decision: shape` verb or parallel list-only policy language.

**Why per-item CEL re-evaluation**

Alternatives exist (pre-computed catalogues, client-side filtering, static ACL lists -- see Alternatives considered). CEL re-evaluation per candidate item is the most consistent way to keep **filtering decisions identical to call-time decisions** when both paths share one merged authorization rule. The evaluation context is augmented for the response phase so operators write one rule and the gateway applies it at request time (allow/deny the list RPC itself, if gated) and at list-shaping time (allow/deny each catalogue entry).

**Design context**

The algorithm, CEL variable bindings, purity constraints, upstream vs downstream placement, caching keys, error semantics, and observability fields are specified in `docs/identity/tools-list-filtering.md`. This ADR records the durable choice, rationale, and trade-offs; implementation follows that spec unless explicitly overridden here.

**What breaks if this stays undecided**

- Block E item 2 cannot land: list and call remain decoupled, violating the agentgateway adoption goal in `MCP_GATEWAY_PLAN.md`.
- ADR 0014 (BulkCheckPermission + ZedToken cache) lacks a defined consumer for list shaping; without per-item semantics, bulk PDP integration has no target contract.
- Operator bundles cannot rely on a single rule set for authorization and catalogue visibility; drift between list predicates and call predicates becomes likely.
- Hierarchical policy merge (`docs/identity/hierarchical-policy-merge.md`) cannot specify how merged rules apply during `*/list` shaping.

**Constraints carried forward from the spec**

1. **Upstream filtering first** -- hook between backend response and `dispatch_backend_response` in `gateway.rs`; downstream synthesis from `schema_cache` is an optimization, not the Block E prerequisite.
2. **Pure list predicates** -- `tools_list_filter` program class rejects impure host builtins (`spicedb.check`, `cache.*`, `audit.emit`, `time.now`, `rate.acquire`) at compile time; SpiceDB-backed rules materialize permissions via `BulkCheckPermission` outside the CEL loop (ADR 0014).
3. **Fail closed on compile errors** -- invalid bundle at load retains previous `policy_version`; hardened profiles may require valid list policy before exposing any catalogue.
4. **Per-tool runtime errors omit only that item** -- one malformed descriptor must not deny the entire list response.

## Decision

Catalog responses (`tools/list`, `resources/list`, `prompts/list`) are filtered by re-evaluating the **same authorization rule** per candidate item against an augmented CEL context (`response.list_filter_index` set, `mcp.tool.name` / `mcp.resource.uri` / `mcp.prompt.name` set to the candidate). Items where the rule denies are dropped from the response. The response is otherwise unchanged.

**Evaluation contract**

| List method | Candidate array key | Per-item binding |
|-------------|---------------------|------------------|
| `tools/list` | `result.tools[]` | `mcp.tool.name` = `Tool.name` |
| `resources/list` | `result.resources[]` | `mcp.resource.uri` = resource URI |
| `prompts/list` | `result.prompts[]` | `mcp.prompt.name` = prompt name |

For each index `i` in the candidate array:

1. Bind `response.list_filter_index = i` (zero-based).
2. Bind the method-appropriate `mcp.*` name/uri field to the candidate's identifier.
3. Re-run the merged authorization CEL rule (OR semantics across rule entries in the bundle -- any matching allow rule keeps the item).
4. If the rule evaluates to **allow**, retain the item (optionally apply schema redaction per `docs/identity/tools-list-filtering.md` §5 -- redaction is orthogonal to visibility).
5. If the rule evaluates to **deny**, or per-item evaluation fails (runtime CEL error), **omit** the item from the array.
6. Publish the modified JSON-RPC success body; do not alter JSON-RPC structure, error paths, or non-catalogue fields.

**Response shape invariant**

Clients receive the same JSON-RPC envelope they would from an unfiltered backend except that denied candidates are absent from the catalogue array. No synthetic entries, no placeholder stubs, no change to MCP method names or outer `result` keys. Empty arrays (`"tools": []`) are valid success responses.

**Opt-out**

Operators may disable list filtering per rule or bundle (e.g. intentional disclosure of tool names while call remains gated). When filtering is disabled for a scope, the gateway forwards the backend catalogue unchanged for that scope -- documented as an operator choice, not the default secure posture.

## Consequences

### Positive

- **Catalogue matches actual permissions** -- agents only see tools, resources, and prompts they can use; the action space shown to the LLM aligns with enforcement at call/read time.
- **No surprise denies at call time** -- denied entries never appear in the list, eliminating the common failure mode where the model attempts a tool it saw in `tools/list` but cannot invoke.
- **Single rule set, two evaluation contexts** -- operators do not maintain separate list-filter YAML; the agentgateway pattern reduces authoring burden and prevents list/call drift when bundles stay aligned.
- **ZedToken cache amortizes SpiceDB cost** -- when authorization rules delegate to SpiceDB, ADR 0014's `BulkCheckPermission` batch plus session-scoped ZedToken cache makes list re-evaluation cheap on repeat lists within the same MCP session; list results warm the cache for subsequent `tools/call` on the same tools.
- **Consistent with hierarchical merge** -- org → tenant → server → method merge produces one predicate; list shaping reuses that predicate per item with augmented bindings (`docs/identity/hierarchical-policy-merge.md`).
- **Auditability** -- per-list `filtered_count` (and related catalogue counters) prove least-privilege shaping to operators and SIEM consumers.

### Negative

- **Per-item CEL evaluation is O(items)** -- a catalogue of N tools incurs N predicate evaluations per list request on the uncached path. Large backends (50--200 tools) add measurable gateway CPU latency unless mitigated by filtered-list caching (`docs/identity/tools-list-filtering.md` §6).
- **SpiceDB load without batching** -- rules that invoke `spicedb.check` per item would multiply RPCs linearly; Block E routes list-time SpiceDB through **one** `BulkCheckPermission` request per list (materialized into evaluation context or a pre-pass) bundled with ADR 0014's cache, not N unary checks inside the CEL loop.
- **Gateway holds full catalogue transiently** -- upstream filtering requires parsing the backend reply before shaping; the gateway is a trusted PEP and may briefly retain the unfiltered list in memory. Downstream synthesis reduces backend load but depends on cache invalidation correctness.
- **Implementation surface** -- `gateway.rs` gains a response-phase hook; `policy.rs` gains list evaluation entry points; audit envelope extensions; purity checker for list program class; tests for OR semantics, per-tool omission on error, and compile-time impure rejection.

### Neutral

- **Opt-out per rule** -- operators who want intentional disclosure (e.g. advertise tool names while gating invocation separately) can disable list filtering for a scope without changing call-time authorization.
- **Redaction remains separate** -- filtering decides **whether** an item appears; JSONPath redaction decides **what** of a visible descriptor is shown. Both may apply to kept items.
- **`prompts/list` and `resources/list` parallel `tools/list`** -- same mechanism and CEL augmentation pattern; program classes may differ (`prompts_list_filter`, `resources_list_filter`) but evaluation semantics are identical.
- **Phase 1 passthrough until bundle loads** -- compatibility flag (`catalog.filter.enabled=false`) preserves verbatim forwarding until policy is ready; hardened deployments may set `MCP_GATEWAY_REQUIRE_LIST_POLICY=1` for deny-all without valid policy.

## Alternatives considered

### Pre-computed catalogue per principal

Maintain a materialized view of each principal's visible catalogue, updated on policy or registry change.

| Assessment | Detail |
|------------|--------|
| **Rejected** | Cache invalidation hell: every JWT claim change, registry update, SpiceDB graph mutation, and backend `tools/list_changed` notification must invalidate the correct principal x server x schema slice. Stale materialized catalogues are a security bug. Per-item re-evaluation with short-TTL filtered-list cache achieves similar performance without a separate invalidation graph. |

### Client-side filter

Return the full catalogue and rely on the MCP client or LLM to ignore unauthorized tools.

| Assessment | Detail |
|------------|--------|
| **Rejected** | Violates the trust boundary. Clients and models are not enforcement points; descriptors leak to any compromised or curious client. MCP spec does not require clients to filter; gateway must shape before egress. |

### Per-tool static ACL list

Configure explicit allow-lists of tool names per tenant or agent in gateway config, separate from CEL authorization.

| Assessment | Detail |
|------------|--------|
| **Rejected** | Not expressive enough for JWT claim matching, hierarchical merge, SpiceDB delegation, or agentgateway-style OR rules. Duplicates authorization logic and guarantees list/call drift. CEL re-evaluation reuses the same rule AST. |

### Separate list-only policy language

Distinct YAML section (`tools_list_filter`) with different semantics from call authorization.

| Assessment | Detail |
|------------|--------|
| **Rejected for authorization semantics** | The spec (`docs/identity/tools-list-filtering.md`) allows a dedicated `tools_list_filter` program class for **purity enforcement** (compile-time builtin restrictions), but the **decision** must mirror call authorization. agentgateway proved one rule set is strictly cleaner. Trogon adopts re-evaluation of the merged authorization rule; optional separate list programs exist only where purity classifiers require them, not as a second authorization language. |

### Live `spicedb.check` inside list CEL loop

Evaluate `spicedb.check(subject, perm, resource)` once per tool inside the CEL predicate.

| Assessment | Detail |
|------------|--------|
| **Rejected** | Side effects and N network round-trips inside a loop break purity, defeat filtered-list caching, and violate the hot-path budget in `docs/identity/bulk-check-permission.md`. SpiceDB checks batch via `BulkCheckPermission`; results feed the evaluation context or a host `spicedb.bulk_check` map populated once per list request. |

## Implementation notes

### CEL evaluator extension

Wire `response.list_filter_index` in the response-phase evaluation context (`docs/identity/reference-cel-variables.md` §7). The variable is **response phase only**; request-phase rules must not depend on it.

| Variable | Phase | Set during list filter |
|----------|-------|------------------------|
| `response.list_filter_index` | response | Zero-based index of current candidate |
| `mcp.tool.name` | both | Candidate tool name on `tools/list` |
| `mcp.resource.uri` | both | Candidate URI on `resources/list` |
| `mcp.prompt.name` | both | Candidate name on `prompts/list` |
| `mcp.method` | both | `"tools/list"`, `"resources/list"`, or `"prompts/list"` |

Top-level bindings (`jwt.*`, `nats.*`, `request.*`, `actor.*`, `server.*`) remain identical on every iteration; only the per-item `mcp.*` field and `response.list_filter_index` change.

### Gateway hook (upstream path)

```
handle_ingress_inner
  ...
  backend_result = request_with_headers(...)
  if is_catalog_list_method(method) && policy.list_filter_enabled() {
      payload = filter_catalog_list_response(payload, eval_ctx, policy, redaction)?;
      maybe_sniff_schema_cache(server_id, &payload);  // tools/list only
  }
  dispatch_backend_response(client, msg, payload)
```

Function names are illustrative; see `docs/identity/tools-list-filtering.md` §2.3 and Appendix A.

### Merge with BulkCheckPermission cache (ADR 0014)

When the merged authorization rule requires SpiceDB:

1. Collect all candidate resource ids from the backend catalogue array.
2. Issue **one** `CheckBulkPermissions` request with `items.len() == N` (or serve entirely from session-scoped ZedToken cache on cache hit).
3. Map `(object_type, object_id, permission, subject_id)` → allow/deny.
4. During per-item CEL re-evaluation, predicates read materialized results (e.g. via `actor.attributes` populated from bulk response, or `spicedb.bulk_check` host map when wired) -- **not** live per-item RPCs.

List shaping **warms** the same cache entries that `tools/call` consumes, so a tool visible after filtering should hit cache on the subsequent call path.

### Audit and observability

Extend `AuditEnvelope` when catalog filtering runs:

| Field | Description |
|-------|-------------|
| `catalog_tools_total` | Count from backend before filter (or analogue for resources/prompts) |
| `catalog_tools_visible` | Count after filter (+ redaction) |
| `catalog_tools_filtered` | `total - visible` (per-list **filtered_count**) |
| `catalog_filtered_name_hashes` | SHA-256 hex of **denied** tool names (no cleartext on JetStream by default) |
| `policy_version` | Bundle revision used |
| `list_cache_hit` | Whether response served from filtered-list cache |

**Proposed audit subject:** `mcp.audit.gateway.list_filter.{outcome}` (alongside existing `{prefix}.audit.{outcome}.request.tools_list` family in the spec).

Tracing spans on `mcp_gateway.handle_ingress` should record `gateway.catalog.filter.enabled`, `gateway.catalog.tools_total`, `gateway.catalog.tools_visible`, `gateway.catalog.tools_filtered`, and `gateway.catalog.cache_hit`.

### Policy bundle and purity

- List re-evaluation uses the **merged authorization rule** from hierarchical policy merge.
- Programs compiled under `tools_list_filter` (and sibling classes) enforce pure builtins only (`docs/identity/tools-list-filtering.md` §4.4).
- Compile-time rejection: `PolicyCompileError::ImpureBuiltin { name, program_class }` at bundle load; gateway retains last good `policy_version`.

### Error semantics (summary)

| Condition | Behavior |
|-----------|----------|
| Bundle compile error at load | Fail closed; retain previous bundle |
| Per-item CEL runtime error | Omit that item; continue siblings; log `warn` with tool name / server_id / agent_id |
| Interpreter fault (bug) | JSON-RPC `-32000` `policy_internal_error` for entire list |
| Backend timeout / unreachable | Unchanged Phase 1 errors; filtering does not run |

### Code and config affected

| Area | Change |
|------|--------|
| `rsworkspace/crates/trogon-mcp-gateway/src/gateway.rs` | Response-phase catalog filter before `dispatch_backend_response` |
| `rsworkspace/crates/trogon-mcp-gateway/src/policy.rs` | List evaluation entry point; merge with call authorization rule |
| `rsworkspace/crates/trogon-mcp-gateway/src/spicedb.rs` | Bulk list shaping integration per ADR 0014 |
| `rsworkspace/crates/trogon-mcp-gateway/src/audit.rs` | Catalogue counter fields |
| `rsworkspace/crates/trogon-mcp-gateway/src/cel_builtins/` | Response context binding for `response.list_filter_index` |
| `rsworkspace/crates/trogon-mcp-gateway/src/schema_cache/` | Sniff on upstream filtered `tools/list`; invalidation on `list_changed` |
| Gateway policy KV / bundle format | No separate list authorization DSL; optional purity class tags |

## Status of supporting work

| Item | Status |
|------|--------|
| Design spec `docs/identity/tools-list-filtering.md` | **Done** (paper spec) |
| CEL variable pin `response.list_filter_index` in `docs/identity/reference-cel-variables.md` | **Done** (spec) |
| BulkCheckPermission + ZedToken cache spec (`docs/identity/bulk-check-permission.md`) | **Done** (paper spec); ADR 0014 acceptance pending sibling task |
| `MCP_GATEWAY_PLAN.md` Block E item 2 checklist entry | **Pending** implementation |
| `filter_catalog_list_response` / `filter_tools_list_response` in `gateway.rs` | **Pending** |
| `PolicyBundle` list filter purity checker | **Pending** |
| `AuditEnvelope` catalogue_* fields | **Pending** |
| Filtered-list cache (`ToolsListCacheKey`) | **Pending** (spec §6) |
| Downstream synthesis from `schema_cache` | **Deferred** post-upstream + invalidation E2E |
| `prompts/list` / `resources/list` program classes | **Pending** (same semantics as this ADR) |
| Operator migration docs (Block H) | **Pending** |

**Acceptance criteria for Block E item 2 closeout**

- [ ] Same merged CEL rule gates `tools/call` and filters `tools/list` per item with augmented context.
- [ ] Denied tools absent from list; allowed tools present; JSON-RPC shape otherwise unchanged.
- [ ] One `BulkCheckPermission` per list when SpiceDB required; cache shared with call path (ADR 0014).
- [ ] Audit emits per-list filtered_count; proposed subject `mcp.audit.gateway.list_filter.{outcome}` documented in operator overview.
- [ ] Tests: OR rule semantics, per-item omit on eval error, impure builtin compile fail, empty catalogue success.

---

*Distilled from `docs/identity/tools-list-filtering.md`. Implementation MUST follow that spec for algorithmic detail unless this ADR explicitly overrides.*

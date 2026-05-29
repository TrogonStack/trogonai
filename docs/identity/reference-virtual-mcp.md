# Virtual-MCP federation name separator

**Diátaxis:** reference (field tables, splitting rules, routing, failure codes) with **explanation** prose where separator choice and federation semantics matter.

**Status banner:** Phase 1 contract. Default separator is `::`. Configurable per virtual server.

**Related:** [reference-subject-grammar.md](reference-subject-grammar.md), [reference-audit-envelope.md](reference-audit-envelope.md), [hierarchical-policy-merge.md](hierarchical-policy-merge.md), [tools-list-filtering.md](tools-list-filtering.md), [failure-mode-matrix.md](failure-mode-matrix.md), Virtual MCP in subjects).

**Implementation target:** `trogon-mcp-gateway` virtual-server federation layer (policy transform before backend fan-out). Backends remain unaware of prefixing.

**Authority:** Wire-Format Pin 4 is normative for Phase 1. This document is the normative source. Older prose that uses underscore (`_`) separators describes pre-pin agentgateway interop; new bundles MUST use `::` unless explicitly overridden.

---

## 1. Scope

This reference defines how a **virtual MCP server** (federated target) multiplexes multiple backend MCP servers behind one logical `server_id` on the edge zone (`virtual-{id}`). Clients see one MCP server; tool, resource, and prompt names carry a **target prefix** separated from the native name by a configurable delimiter.

### 1.1 In scope

| Surface | Namespaced? | Split applies on |
|---|---|---|
| `tools/list` merged catalogue | Yes (gateway adds prefix) | N/A (list is gateway-generated) |
| `tools/call` `params.name` | Yes (client sends prefixed name) | First separator occurrence |
| `resources/read` `params.uri` | Yes (recommended) | First separator occurrence |
| `prompts/get` `params.name` | Yes (recommended) | First separator occurrence |
| `resources/list`, `prompts/list` | Yes (gateway adds prefix on merge) | N/A (list is gateway-generated) |

### 1.2 Out of scope

Virtual server membership (bundle / KV config), hierarchical CEL merge ([hierarchical-policy-merge.md](hierarchical-policy-merge.md)), non-virtual targets (no prefix transform), and callback-direction subject routing.

### 1.3 Terminology

| Term | Definition |
|---|---|
| **Target prefix** | Alias of a federation member backend; becomes the left segment before the separator. Must resolve to a registered `server_id` on the backend lane. Example: `github`. |
| **Native name** | Tool / prompt / resource identifier as the member backend exposes it before federation. Example: `create_issue`. |
| **Federated name** | `{target_prefix}{separator}{native_name}`. Example: `github::create_issue`. |
| **Separator** | Delimiter string configured per virtual server. Phase 1 default: `::`. |

---

## 2. Separator syntax

### 2.1 Canonical form

Federated names use this shape:

```text
{target_prefix}{separator}{native_name}
```

Phase 1 default separator:

```text
{target_prefix}::{tool_name}
```

### 2.2 Concrete examples (default `::`)

| Federated name | `target_prefix` | `native_name` | Backend route |
|---|---|---|---|
| `github::create_issue` | `github` | `create_issue` | `mcp.server.github.tools.call` |
| `linear::create_ticket` | `linear` | `create_ticket` | `mcp.server.linear.tools.call` |
| `notion::search_pages` | `notion` | `search_pages` | `mcp.server.notion.tools.call` |

The separator is **not** part of `native_name`. The gateway strips `{target_prefix}` and `{separator}` before forwarding; the backend receives only `native_name` in JSON-RPC params.

### 2.3 Target prefix constraints

Target prefixes MUST satisfy the same constraints as `{server_id}` on the backend lane ([reference-subject-grammar.md §3.3](reference-subject-grammar.md#33-server_id--client_id-mcppeerid--natstoken)):

| Rule | Detail |
|---|---|
| Charset | ASCII; `[a-z0-9-]+` in plan examples |
| Max length | 128 characters (same as `McpPeerId`) |
| Forbidden | `.`, `*`, `>`, whitespace |
| Uniqueness | Each target prefix in a virtual server's member list MUST map to exactly one backend `server_id` |

The target prefix is typically identical to the member backend's `server_id`, but the bundle MAY alias (`gh` → backend `github`) **(proposed mapping table in bundle)**. Resolution uses the alias table; NATS egress always uses the resolved backend `server_id`.

### 2.4 Invalid federated names (structural)

| Federated name | Problem | Gateway behavior |
|---|---|---|
| `create_issue` (no separator) | Cannot resolve member | `-32103` `backend_unreachable` after virtual-server handler fails to match |
| `::create_issue` | Empty `target_prefix` | `-32103` `backend_unreachable` |
| `github::` | Empty `native_name` | `-32103` `backend_unreachable` |
| `unknown::create_issue` | Target not in member list | `-32103` `backend_unreachable` |

---

## 3. Why `::`

Wire-Format Pin 4 rationale:

> Rationale: the MCP spec restricts tool names to a pattern that excludes `:`, so `::` cannot collide with any legal native tool name. Single `:` would be ambiguous with URI schemes in resource references. The separator is configurable per virtual server in the bundle (`separator: "::"` default) for operators who need to interop with prior conventions.

### 3.1 Explanation: tool names vs resource URIs

MCP tool names are constrained identifiers on the wire. Because `:` is excluded from legal native tool names, doubling it (`::`) creates a delimiter that:

1. **Cannot appear inside a native tool name** — no accidental split inside backend-local names.
2. **Avoids URI scheme ambiguity** — resource URIs frequently contain `scheme:` (for example `file://`, custom `issue://`). A single-colon separator would make `github:issue://owner/repo/123` ambiguous: is `github` the target prefix or part of the URI?
3. **Reads unambiguously in logs and audit** — SIEM queries can key on `::` without colliding with URL syntax in the native segment.

### 3.2 Collision with native names containing `::`

Some backends may expose tool names that literally contain `::` (unusual but permitted by servers that do not enforce MCP name charset strictly). The **first-separator split** (§4) preserves such names: federated form `github::weird::tool` routes to backend `github` with native name `weird::tool`.

---

## 4. Splitting rule

On any federated method where the client supplies a namespaced identifier, the gateway splits on the **first** occurrence of the configured separator only.

```text
split_once(federated_name, separator) -> (target_prefix, native_name)
```

| Input | Separator | `target_prefix` | `native_name` |
|---|---|---|---|
| `github::create_issue` | `::` | `github` | `create_issue` |
| `github::weird::tool` | `::` | `github` | `weird::tool` |
| `linear::create_ticket` | `::` | `linear` | `create_ticket` |

**Applies to:**

| JSON-RPC method | Param field | Rewritten value sent to backend |
|---|---|---|
| `tools/call` | `params.name` | `native_name` |
| `resources/read` | `params.uri` | `native_name` (see §7) |
| `prompts/get` | `params.name` | `native_name` |

The gateway MUST NOT split on subsequent separator occurrences. Everything after the first separator is treated as the original identifier verbatim. Empty `target_prefix` or empty `native_name` after split yields `-32103` `backend_unreachable`.

---

## 5. Configurability

Per virtual server, the policy bundle declares the separator string.

### 5.1 Bundle default

```yaml
# **proposed** bundle fragment — virtual server section
virtual_servers:
  - id: default                    # edge server_id: virtual-default
    separator: "::"                # Wire-Format Pin 4 default
    members:
      - target: github
        server_id: github
      - target: linear
        server_id: linear
      - target: notion
        server_id: notion
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `separator` | string | `"::"` | MUST be non-empty. Phase 1 bundles omitting the field inherit `::`. |
| `members[].target` | string | same as `server_id` | Target prefix in federated names |
| `members[].server_id` | string | — | Resolved backend for NATS egress |

### 5.2 Override for legacy interop

Operators migrating from prior conventions (for example agentgateway-style `github_create_issue` with `_`, or legacy `github:create_issue` with `:`) MAY set:

```yaml
separator: "_"
# or
separator: ":"
```

**Recommendation:** override only during migration windows. Single `:` reintroduces URI ambiguity (§3). Underscore `_` is safe for tool names but conflicts with MCP resource URI schemes less visibly than `:` — still prefer `::` for new deployments.

### 5.3 Validation at bundle load

Bundle load rejects empty `separator`, member `target` values that contain the separator string, or duplicate `target` / `server_id` entries within one virtual server.

---

## 6. `tools/list` aggregation

When a client calls `tools/list` on a virtual server, the gateway performs parallel federation, not a single backend hop.

### 6.1 Sequence

```text
Client → mcp.gateway.request.virtual-default.tools.list
           │
           ├─ parallel initialize (if member not yet initialized for session)
           ├─ mcp.server.github.tools.list
           ├─ mcp.server.linear.tools.list
           └─ mcp.server.notion.tools.list
           │
           ▼ merge: prefix each tool.name with {target}{separator}
           ▼ optional CEL filter per tool ([tools-list-filtering.md](tools-list-filtering.md))
           ▼ single tools/list JSON-RPC result to client
```

Member initialization follows Wire-Format Pin 5: lazy per member, parallel before merge when `tools/list` runs.

### 6.2 Merge rules

| Field | Transform |
|---|---|
| `name` | `{target}{separator}{original_name}` — required |
| `inputSchema` | Untouched (copied verbatim from member) |
| `outputSchema` | Untouched if present |
| `title` | Unchanged by default |
| `description` | Unchanged by default |

### 6.3 Optional description augmentation (off by default)

Bundle flag `augment_descriptions: true` **(proposed)** MAY append a source hint:

```text
{original_description} [via {target}]
```

Default is **off** to keep client-visible text identical to backend semantics aside from the required `name` prefix.

### 6.4 Schema cache keying

After merge, the gateway schema cache keys redaction and policy by `{resolved_server_id, native_name, schema_hash}` — not by federated name. A `tools/call` for `github::create_issue` resolves to server `github`, tool `create_issue` before cache lookup.

### 6.5 Partial member failure

Phase 1 default: **`best_effort`** — return tools from members that succeed; audit per-member errors. Bundle MAY set `tools_list_mode: fail_closed` to fail the entire virtual `tools/list` if any member fails **(proposed flag)**.

---

## 7. Resource and prompt namespacing

**Pin:** the same separator and the same first-split rule apply to resources and prompts, not only tools. One cognitive model across MCP surfaces.

### 7.1 Resources

| Operation | Client sends | Gateway forwards |
|---|---|---|
| `resources/list` | (virtual server id on subject only) | Merged list with prefixed `uri` |
| `resources/read` | `params.uri = "github::issue://owner/repo/123"` | `params.uri = "issue://owner/repo/123"` on `mcp.server.github.resources.read` |

Example federated resource URIs (default `::`):

```text
github::issue://owner/repo/123
linear::ticket://TEAM-42
notion::page://abc-def-ghi
```

The native URI (`issue://…`) may contain `:`; only the **first** `::` (default separator) is federation metadata.

### 7.2 Prompts

| Operation | Client sends | Gateway forwards |
|---|---|---|
| `prompts/list` | (virtual server id on subject only) | Merged list with prefixed `name` |
| `prompts/get` | `params.name = "github::summary"` | `params.name = "summary"` on `mcp.server.github.prompts.get` |

### 7.3 Legacy single-colon separator

When `separator: ":"` is configured:

| Federated URI | Split result | Risk |
|---|---|---|
| `github:summary` | `github` / `summary` | Works for simple prompt names |
| `github:issue://owner/repo/123` | `github` / `issue://owner/repo/123` | Works if split is first `:` only |
| Ambiguous custom URIs with extra colons before scheme | First `:` wins | Operator must ensure targets do not use `:` in aliases |

**Pinned behavior:** identical `split_once` algorithm regardless of separator width. Gateway does not special-case URI schemes. Operators using `:` accept first-colon semantics documented above.

---

## 8. Subject routing

Federation affects JSON-RPC params and list merge; NATS subjects encode **which backend lane** receives the forwarded message after split.

### 8.1 Edge vs backend subjects

| Phase | Subject pattern | `server_id` segment |
|---|---|---|
| Client ingress | `{prefix}.gateway.request.{edge_server_id}.{method_suffix}` | Virtual id, e.g. `virtual-default` |
| Gateway egress (after split) | `{prefix}.server.{backend_server_id}.{method_suffix}` | Resolved member, e.g. `github` |

The virtual id appears only on ingress. Egress NEVER uses `virtual-*` as `server_id` after federation resolution.

Cross-reference: [reference-subject-grammar.md §2.2–2.3](reference-subject-grammar.md#22-gateway-ingress).

### 8.2 Worked trace: `tools/call`

| Step | Subject | JSON-RPC `params.name` |
|---|---|---|
| 1 Client publish | `mcp.gateway.request.virtual-default.tools.call` | `github::create_issue` |
| 2 Gateway split | — | `target=github`, `native=create_issue`, `backend=github` |
| 3 Gateway egress | `mcp.server.github.tools.call` | `create_issue` |
| 4 Audit | `mcp.audit.allow.request.tools` | envelope `tool` = native `create_issue`; `subject_out` = step 3 |

Policy evaluation for authorization uses the **member** `server_id` (`github`), not the virtual id ([hierarchical-policy-merge.md §10.5](hierarchical-policy-merge.md#105-virtual-mcp-federation)).

### 8.3 Worked trace: federated `tools/list`

| Step | Subject | Notes |
|---|---|---|
| 1 Client publish | `mcp.gateway.request.virtual-default.tools.list` | No name split |
| 2 Gateway fan-out | `mcp.server.github.tools.list`, `mcp.server.linear.tools.list`, … | Parallel |
| 3 Gateway reply | (client inbox) | Merged tools with prefixed names |

### 8.4 STS egress mint

Egress mesh token mint targets the **resolved backend** audience (`server_id` after split), not the virtual server id. STS `target` claim aligns with physical backend for SpiceDB tuples keyed by server scope.

---

## 9. Failure modes

Errors Trogon JSON-RPC allocation unless marked **proposed**.

### 9.1 Target not registered

| Condition | Code | Symbol | `data` shape |
|---|---|---|---|
| `target_prefix` not in virtual server's member list | `-32103` | `backend_unreachable` | `{ trace_id, server_id }` where `server_id` is the **resolved target** attempted (or `target_prefix` if unmapped) |
| Member backend has no active NATS consumer | `-32103` | `backend_unreachable` | `{ trace_id, server_id }` |
| Empty prefix or suffix after split | `-32103` | `backend_unreachable` | `{ trace_id, server_id }` |

Audit: `mcp.audit.error.request.tools` (or matching `method_root`) with `decision_reason` **`backend_unreachable`** ([reference-audit-envelope.md §2.3](reference-audit-envelope.md#23-well-known-decision_reason-values)).

### 9.2 Tool not in target's `tools/list` cache

| Condition | Code | Symbol | When |
|---|---|---|---|
| `inputSchema` missing for `{backend, native_name}` and fetch fails | `-32104` | `schema_unknown` | failClosed redaction path requires schema |
| Tool absent from cached list after successful prior `tools/list` | `-32100` or forward backend error | `policy_deny` / backend error | Policy may deny; backend may return MCP tool not found |

`schema_unknown` `data` shape: `{ trace_id, server_id, tool }` where `tool` is the **native** name (post-split), not the federated name.

Virtual `tools/list` merge populates cache per member. Stale cache after `notifications/tools/list_changed` from a member follows existing invalidation on `mcp.control.cache.invalidate.{server_id}`.

### 9.3 Ambiguous legacy separator (`:`)

| Scenario | Pinned behavior |
|---|---|
| Bundle sets `separator: ":"` | First `:` split; see §7.3 |
| Client sends name with no separator | `-32103` `backend_unreachable` |
| Client sends federated name using `::` while bundle expects `:` | Split fails or misroutes; treat as unreachable / wrong tool |
| Client sends `::` name while bundle expects `_` | Same — separator mismatch is operator error |

**Migration guidance:** run dual-list window **(proposed)** only at gateway (expose both prefixed forms) is NOT supported in Phase 1. Switch separator only with coordinated client updates.

### 9.4 Other federation failures

Unknown virtual server id, member timeout (`-32102`), fan-out rate limit (`-32105`), and policy deny on native tool + member scope (`-32100`). Full matrix: [failure-mode-matrix.md](failure-mode-matrix.md).

Audit envelopes SHOULD record ingress virtual id on `subject_in`, member backend on `subject_out`, and native `tool` post-split ([reference-audit-envelope.md](reference-audit-envelope.md)).

---

## 10. Cross-references

| Topic | Document / plan section |
|---|---|
| NATS subject patterns, `{server_id}` token rules | [reference-subject-grammar.md](reference-subject-grammar.md) |
| Policy merge uses member server, not virtual id | [hierarchical-policy-merge.md §10.5](hierarchical-policy-merge.md#105-virtual-mcp-federation) |
| `tools/list` CEL filtering after merge | [tools-list-filtering.md](tools-list-filtering.md) |
| Audit envelope fields | [reference-audit-envelope.md](reference-audit-envelope.md) |

### 10.1 Phase 1 compatibility

Default `::`, first-separator split, per-virtual-server configurability, and the same rule for tools/resources/prompts (§7) are stable Phase 1 contracts. Underscore examples in older plan sections are not wire pins. SDKs SHOULD read bundle `separator` rather than hard-code `::`.

---

*Wire-Format Pin 4 reference. Verify against gateway federation implementation when landed.*

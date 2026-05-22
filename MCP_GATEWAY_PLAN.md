# MCP Gateway Plan

A NATS-positioned policy enforcement point for MCP (Model Context Protocol) traffic — authorization, redaction, audit, rate limiting, and catalog shaping — built on the existing `mcp-nats` transport.

## Intent

Provide a single chokepoint where every MCP tool call, resource read, prompt fetch, sampling request, and elicitation crossing the bus is:

- **Authenticated** against a tenant identity (NATS auth callout → JWT claims).
- **Authorized** against a policy decision point (SpiceDB by default, pluggable).
- **Shaped** — `tools/list`, `resources/list`, `prompts/list` responses filtered to what the caller can use.
- **Redacted** — request `params` and response `result` rewritten according to schema-driven policies.
- **Audited** — every decision (allow / deny / rewrite / log / error) emitted to a JetStream stream.
- **Rate-limited and bounded** — per-tool, per-tenant, per-caller.

The gateway is generic at the transport level (NATS protocol + JSON-RPC) and MCP-aware via shippable policy bundles. The same engine should later cover ACP, A2A, or any JSON-RPC-over-NATS protocol.

## Inspiration

### [Synadia Protect](https://www.synadia.com/protect)

Transparent NATS-protocol proxy with signed versioned policy bundles, Expr-based rules, decision verbs (allow / deny / suspend / log / error), and audit streamed back to NATS in JSON/CEF.

What to adopt wholesale:

- **Bundle packaging** — NKey-signed, versioned, hot-swap, rollback, no gateway restart.
- **Decision verbs** — extend allow/deny with `rewrite` (redaction) and `shape` (catalog filtering).
- **Audit-to-NATS** — JetStream stream is the source of truth; SIEM consumers subscribe.
- **Transparent positioning** — clients keep their existing subjects; the gateway intercepts.

What Protect alone cannot do, and why we need more:

- It operates at NATS subject/payload level — cannot understand JSON-RPC method/params/result.
- It cannot shape a `tools/list` *response* by caller permission.
- It cannot map JSON-RPC `params` into a SpiceDB resource tuple.
- It cannot do schema-aware redaction tied to a tool's `inputSchema` / `outputSchema`.

So: build Protect-shaped but JSON-RPC-aware, with MCP semantics layered via bundles.

### [agentgateway](https://github.com/agentgateway/agentgateway) — [agentgateway.dev](https://agentgateway.dev)

Open-source Rust proxy positioned as "the next-generation agentic proxy for AI agents and MCP servers." Closest prior art at the *product* level. See **§ "agentgateway Deep Dive — Mapping onto NATS"** below for a concept-by-concept analysis of how their architecture maps onto our substrate; the short version:

- Borrow: CEL as the DSL, the four-phase pipeline shape, CEL-driven catalog filtering, target-prefix virtual MCP federation, hierarchical policy merge.
- Diverge: NATS transport instead of HTTP/gRPC/SSE; NATS KV + JetStream object store as the control plane instead of xDS + Gateway API CRDs; WASM Component Model as a sandboxed tier on top of an ext-proc-style callout (they have only the callout); schema-driven redaction tied to MCP `inputSchema`/`outputSchema` instead of regex/PII guardrails (theirs are LLM-shaped, not MCP-shaped).
- Reuse pattern: the same engine should cover ACP, A2A, and any JSON-RPC-over-NATS protocol because *bundles* encode protocol semantics, not engine code.

## agentgateway Deep Dive — Mapping onto NATS

This section unpacks how agentgateway is actually built — based on its docs, the crate layout (`agentgateway`, `cel-fork`, `celx`, `core`, `hbone`, `xds`, `pool`, `protos`, `xtask`, `controller`), and the policy/MCP doc pages — and shows the concrete NATS-substrate equivalent for each piece. The intent: pick the right things to copy, the right things to throw away, and the right things to *redesign in NATS-native form* rather than port.

### 1. The Four-Phase Pipeline

agentgateway processes every request through four sequential phases with shallow field-level merge across all applicable policies at each phase:

| Phase | What it does in agentgateway | NATS-substrate equivalent |
|---|---|---|
| **Frontend** | TCP/TLS termination, access logging, tracing, gateway-level concerns | NATS **auth callout** at CONNECT — identity resolution (OIDC / mTLS / API key) → scoped user JWT with subject ACL `mcp.gateway.{tenant}.>`. TLS is the NATS server's responsibility. Tracing context bound to the connection. |
| **PreRouting** | JWT / Basic / API-key auth, extAuth, extProc, transformations — runs *before* route selection | Gateway queue-group consumer receives the NATS message, decodes JSON-RPC, validates JWT claims, applies pre-routing transformations (e.g., header injection from claims). No subject mapping yet. |
| **PostRouting** | The bulk — CORS, authentication, authorization, rate limit (local + global), extProc, transformations, header modifiers, CSRF, direct response. 14 filter types. | **The MCP policy engine runs here.** CEL evaluation, SpiceDB checks, schema-driven redaction, audit emission, rate limiting (JetStream-backed for distributed), `tools/list` shaping, request rewriting. |
| **Backend** | TLS to upstream, backend auth, AI/MCP-specific policies, health checks | Publish to `mcp.server.{server_id}.<method>` via NATS request/reply. NATS account isolation replaces backend TLS. Health is NATS subscription liveness. |

The 4-phase shape is **directly portable**. We adopt it. Filter-order is one of the things they got right and we shouldn't re-invent.

### 2. Config Distribution (xDS → NATS KV + JetStream)

agentgateway runs a control plane that ships config to data-plane proxies via **xDS** (the Envoy protocol family). Standalone mode reads YAML/JSON; Kubernetes mode uses Gateway API CRDs (`AgentgatewayPolicy`, `AgentgatewayBackend`, `AgentgatewayParameters`) translated to xDS by their controller.

NATS equivalent — and arguably better operationally because NATS is already the bus:

- **Config source of truth** → NATS KV bucket `gateway-config-{tenant}` with revisions. Each policy / backend / route is a KV entry keyed by name.
- **Dynamic config push** → KV watchers. The gateway service subscribes; any KV update fires a hot reload. Revision number is the version pin.
- **Bundles (Tier 1 + Tier 2 + Tier 3 + manifest)** → NATS **JetStream Object Store** for the binary WASM artifacts; NKey signature in the manifest; KV holds the active version pointer.
- **Multi-source layering** → multiple KV buckets in priority order (org-base, tenant-override). The controller — if we even need one — is just a NATS service that watches some upstream (Git, OCI, K8s CRDs) and projects into KV.

This collapses agentgateway's three-tier "K8s CRD → controller → xDS → proxy" into "KV write → proxy watcher fires." No xDS protocol, no controller process required for the standalone case.

### 3. MCP Authorization (CEL + Automatic List Filtering)

agentgateway's `mcpAuthorization` policy is the single sharpest design they made. The mechanism:

- A `rules:` list of CEL expressions. **OR semantics** — any matching rule allows.
- Available CEL variables include `mcp.tool.name`, `mcp.tool.target`, `mcp.prompt.name`, `mcp.resource.name`, `jwt.sub`, `jwt.<claim>`, `has(jwt.<claim>)`.
- The same rule that gates `call_tools` **automatically filters `list_tools` responses** — the engine re-evaluates the rule against each candidate item with `mcp.tool.name` bound, and drops items that don't match.

```yaml
mcpAuthorization:
  rules:
  - 'jwt.sub == "alice" && mcp.tool.name == "add"'
  - 'mcp.tool.name == "echo"'           # public tool
  - '"admin" in jwt.roles && mcp.tool.name == "admin-tool"'
```

**Adopt this directly.** It is strictly cleaner than the `decision: shape` verb sketched earlier in this plan: one rule set, two evaluation contexts (request-time, list-time), and the user never writes "shaping" logic separately from authorization logic. The same approach extends to `prompts/list` and `resources/list` for free.

NATS-substrate addition: the CEL variable namespace gains `nats.*` (subject, headers, account) and `spicedb.check(subject, perm, resource)` as a host builtin so rules can defer to SpiceDB when something is more complex than JWT claim matching. The TrogonStack default `mcp-pack` bundle would ship CEL rules that delegate to SpiceDB for fine-grained tuples and use JWT claims directly for the coarse cases.

Replace the earlier `decision: shape` verb with this re-evaluation mechanism.

### 4. Hierarchical Policy Merge (Backend → Target)

agentgateway merges policies hierarchically: backend-group-level → target-level, with target overriding backend for the same field. Shallow field merge.

NATS equivalent uses subject-pattern specificity:

| Scope | Subject pattern | Example |
|---|---|---|
| Org-wide | `mcp.gateway.>` | Audit envelope shape, base rate limit |
| Tenant | `mcp.gateway.{tenant}.>` | Tenant-wide tool allowlist |
| Server group | `mcp.gateway.{tenant}.server.{group}.>` | Per-group SpiceDB resource template |
| Specific server | `mcp.gateway.{tenant}.server.{id}.>` | Override redaction for one server |
| Method | `mcp.gateway.{tenant}.server.{id}.tools.call` | Per-method rules |

Merge is most-specific-wins per field. Same model, expressed in subject patterns rather than YAML target/backend hierarchy.

### 5. Virtual MCP Federation (Target-Prefix Multiplexing)

agentgateway's "Virtual MCP" combines N backend MCP servers into one logical server by prefixing tool names with target name: `time_get_current_time`, `everything_echo`. Collision resolution is free because the prefix is unique per target.

NATS substrate: this is **trivial** because subjects already encode `mcp.server.{server_id}` — the prefix is the server id, no separate config needed.

- Federated `tools/list` becomes a fan-out: gateway receives `mcp.gateway.{tenant}.tools.list` → fans out to `mcp.server.*.tools.list` for each registered server in the tenant's view → collects replies → prefixes each tool's `name` with `{server_id}_` → applies CEL filtering → returns one merged list.
- `tools/call` reverses: gateway receives `mcp.gateway.{tenant}.tools.call` with `params.name = "github_create_issue"` → splits on first `_` → routes to `mcp.server.github.tools.call` with rewritten `params.name = "create_issue"`.

Federation is a pure gateway-layer transform. The backend MCP servers don't know they've been multiplexed.

### 6. Extensibility (ExtProc → NATS-callout + WASM)

agentgateway's escape hatch is **ExtProc** — Envoy's external-processing protocol over gRPC. The proxy ships headers/body to an external server, the server returns modifications, the proxy applies them. `failClosed` by default. They do **not** support WASM filters.

We have a strictly better story available because we're on NATS:

**Tier-2.5: NATS-callout plugins.** Instead of dialing out to a separate gRPC server, the gateway issues a NATS request on `mcp.policy.extproc.{plugin_name}` with the request envelope, and any subscriber (with the right entitlement) replies with the modification. Same out-of-process plugin model, same failClosed default, but:

- No second network transport — it's already on the bus.
- Plugins scale horizontally via queue groups for free.
- Audit, tracing, and rate limiting on plugin invocations come from NATS itself.
- Multi-language is automatic — anything that speaks NATS can be a plugin.

**Tier 3: WASM Component Model.** For in-proc, sandboxed, tenant-authored code that doesn't deserve a separate service — schema-driven redaction tables, custom PII classifiers, format translators. WIT interface, capability-based host imports (`spicedb.check`, `nats.request`, `cache.get/set`, `audit.emit`, `kv.fetch`). Component pooled per bundle version. This is the tier agentgateway *doesn't have* and the user-facing differentiator.

Pipeline order: CEL → NATS-callout → WASM, each tier strictly more powerful, all three available in the same bundle.

### 7. Guardrails / Redaction (Regex/PII → Schema-Driven)

agentgateway's guardrails are **LLM-prompt-shaped**: regex/PII detectors, OpenAI Moderation, Bedrock Guardrails, Google Model Armor, webhook callouts. Useful for filtering LLM prompts and completions — not designed for MCP `tools/call` payloads with arbitrary `params` and `result` shapes.

Our redaction is **schema-driven** and MCP-native:

- The gateway sniffs `tools/list` replies and caches each tool's `inputSchema` / `outputSchema` keyed by `{server_id, tool_name, schema_hash}`.
- Redaction policies attach to schemas via JSONPath (and via JSON Schema annotations once MCP standardizes a `x-sensitive` extension or similar): "for tool `db_query`, hash `$.params.connection_string`, mask `$.result.rows[*].ssn`."
- The redactor is a Tier-3 WASM component shipped in the `mcp-pack` bundle. Tenants extend with their own components for custom classifiers.
- Their regex/PII guardrails still useful as a layer: ship them as Tier-1 rules in the `llm-pack` bundle, separately.

This is one of the cleanest places we can be strictly better than agentgateway: **schemas exist for a reason; use them.**

### 8. Audit (Access Logging → JetStream Stream)

agentgateway has access logging and OpenTelemetry-by-default. Audit is structured logging out the gateway.

NATS equivalent is the natural one: audit envelope per decision → JetStream stream `MCP_AUDIT` with subject `mcp.audit.{tenant}.{outcome}.{method}`. Hash-chained envelopes (each envelope includes prior envelope's digest in the tenant's chain) for tamper-evidence. SIEM consumers subscribe via durable JetStream consumers. OpenTelemetry traces emit alongside but the audit stream is the legal record.

This is one place where NATS is *operationally simpler* than agentgateway — JetStream gives us durability, retention policy, replay, and SIEM connection out of the box.

### 9. What NATS Substrate Gives Us For Free

Things agentgateway has to build, that we get from NATS infrastructure:

| Concern | agentgateway approach | NATS substrate |
|---|---|---|
| Horizontal scale | Multiple proxy pods + xDS sync | Queue groups (subscribe with `--queue`) |
| Distributed rate limit | Custom global rate-limit service | JetStream KV with TTL + atomic increment |
| Service discovery | Gateway API `Backend` CRD + endpoint slices | Subject space — `mcp.server.*` *is* the registry |
| TLS to backend | `backendTLS` policy | NATS account / cluster TLS already in place |
| Auth to backend | `backendAuth` policy (API key, IAM, etc.) | NATS user JWT with subject permissions |
| Connection pooling | `pool` crate | NATS multiplexes everything over one connection |
| mTLS between hops | Explicit cert config | NATS server-to-server already mTLS |
| Multi-region routing | Locality-aware-routing policy | NATS clusters + leaf nodes + subject routing |
| Observability bus | OTel + custom exporters | NATS itself, plus OTel |

We lose almost nothing by being NATS-native and gain a lot of operational primitives.

### 10. What Agentgateway Has That We Will Need to Build

Honest list — these are real:

- **CEL forks (`cel-fork`, `celx`).** They have already done the work of extending CEL with proxy-specific builtins. We need our own — either fork `cel-rust` or use the upstream and add host functions externally. Either way, this is real engineering.
- **`xds` crate / Envoy ext_proc compatibility.** Useful for *interop* — being able to receive xDS config or be an ext_proc target — which would let TrogonStack gateways participate in Envoy meshes. Worth considering as a v2 feature, not v1.
- **`agctl` CLI + Admin UI.** Operator UX. Inspect config, trace requests, debug routes. We'll need an equivalent (likely a TUI / web UI subscribing to the gateway's `mcp.audit.>` and config-status subjects).
- **OAuth 2.0 MCP integration.** They have first-class OAuth flows for MCP clients (Keycloak, Auth0, Okta, OIDC). We need to either match this in the auth-callout service or document the recommended pattern.
- **Gateway API / Kubernetes ergonomics.** A meaningful population of users expects CRDs. A thin K8s controller that projects CRDs into our NATS KV is a reasonable v2.
- **LLM-specific features** (model aliases, virtual keys, prompt enrichment, cost tracking, token budgets). Out of scope for the MCP gateway itself — that belongs in a sibling `trogon-llm-gateway` if/when we want to compete on that surface.

### 11. Concrete Crate / Module Translation

Their crate → our crate (proposed):

| agentgateway | trogon equivalent |
|---|---|
| `agentgateway` (main) | `trogon-mcp-gateway` (binary) |
| `core` | `trogon-policy-core` (engine, decision verbs, phase pipeline) |
| `cel-fork`, `celx` | `trogon-policy-cel` (CEL + Trogon-specific builtins) |
| `xds` | (none — NATS KV watcher replaces it) |
| `controller` | `trogon-mcp-gateway-controller` (optional K8s/CRD projector, v2) |
| `hbone` | (none — NATS is the overlay) |
| `pool` | (none — NATS connection pool covers it) |
| `protos` | (none — JSON-RPC + NATS-subject schemas in `trogon-mcp-protocol`) |

`mcp-nats` and friends remain unchanged; the gateway is a *consumer* of that transport.

### 12. Net Picture

agentgateway is the strongest existing answer to "MCP needs a gateway" — and confirms the shape of the product (4-phase pipeline, CEL policies, MCP-aware authz with list filtering, virtual federation, ext-proc extensibility, control-plane/data-plane split). Our differentiation is not "do something they don't do for the sake of differentiation" but to **collapse their stack onto a substrate we already operate** — NATS — which removes whole categories of infrastructure they have to build (xDS, hbone, separate rate-limit service, separate config controller), and to add the **WASM Component Model tier** they don't have, which is the right escape hatch for tenant-authored policy logic.

The product wedge in one sentence: **agentgateway, but the control plane, data plane, audit, and plugin bus are all NATS, and tenant policy logic runs as signed WASM components.**

## Architecture

### Positioning on NATS

Two namespaces, separated by NATS authorization (issued via auth callout):

- `mcp.gateway.{tenant}.>` — only subject clients are permitted to publish to.
- `mcp.server.{id}.>` — only the gateway service is permitted to publish to.

Real MCP servers subscribe to `mcp.server.{id}.>` exactly as today (`rsworkspace/crates/mcp-nats`). Clients see only the gateway namespace. No code change to the existing transport.

### Components

1. **Auth callout service** — connect-time identity resolution. Maps external credentials (OIDC, mTLS, API key) to a scoped NATS user JWT with subject ACL covering `mcp.gateway.{tenant}.>`.
2. **Gateway service** — queue-group subscriber on `mcp.gateway.>`. Stateless per-message; correlation state lives in-flight only.
3. **Policy engine** — embedded in the gateway. Three-tier (see below). One execution model.
4. **Audit emitter** — every decision → JetStream stream `mcp.audit.{tenant}.>` with JSON envelope.
5. **Bundle loader** — fetches signed bundles from a configured source (NATS KV, OCI registry, HTTP), verifies NKey signature, hot-swaps in place.
6. **Schema cache** — learns each backend server's tool / resource / prompt schemas by inspecting `tools/list` etc. replies; keyed by server id + version; used by redaction policies.

### Request flow

```
client ──PUB──▶ mcp.gateway.{tenant}.tools.call
                        │
                        ▼
                ┌──────────────────┐
                │  Gateway service │
                │  (queue group)   │
                └──────────────────┘
                        │
        1. parse JSON-RPC (method, params, id)
        2. resolve identity from JWT claims
        3. derive resource tuple from method+params
        4. policy.authorize(req)            ──▶ SpiceDB CheckPermission
        5. policy.rewrite(req.params)       ──▶ input redaction
        6. publish mcp.server.{id}.tools.call
        7. await reply on inbox
        8. policy.shape(resp.result)        ──▶ catalog filter (if list)
        9. policy.rewrite(resp.result)      ──▶ output redaction
       10. publish audit envelope            ──▶ mcp.audit.{tenant}.*
       11. reply to client on original inbox
```

Bidirectional traffic (server → client `sampling/createMessage`, `roots/list`, `elicitation/create`) is handled symmetrically on `mcp.client.{client_id}.>` — same policy engine, same decision verbs, separate rule set.

## Policy Engine

Three tiers, strictly increasing in power. Lower tiers desugar into higher tiers. **One execution engine** under the hood (WASM).

### Tier 1 — Declarative config

YAML. Covers the boring 60%.

```yaml
rules:
  - name: deny-after-hours-writes
    when:
      subject: "mcp.gateway.*.tools.call"
      jwt.role: "intern"
      time.hour_utc_in: [0, 6]
    decision: deny
    reason: "interns cannot write outside business hours"

  - name: rate-limit-expensive-tools
    when:
      tool.name_in: ["run_query", "send_email"]
    decision: rate_limit
    limit: { per: caller, max: 10, window: 1m }
```

### Tier 2 — Expression DSL

CEL (preferred over Expr for ecosystem reach and typed evaluation). Predicates, projections, builtin host functions.

```cel
rule "tool-call-authz" {
  when = request.method == "tools/call"
  decision = spicedb.check(
    subject = "user:" + jwt.sub,
    permission = "invoke",
    resource = "tool:" + request.params.name
  ) ? "allow" : "deny"
}

rule "shape-tools-list" {
  when = response.method == "tools/list"
  action = "shape"
  filter = spicedb.bulk_check(
    subject = "user:" + jwt.sub,
    permission = "invoke",
    resources = response.result.tools.map(t -> "tool:" + t.name)
  )
}
```

Builtins (host-provided): `spicedb.check`, `spicedb.bulk_check`, `cache.get/set` (ZedToken caching), `jsonpath.get/set/delete`, `audit.emit`, `time.now`, `rate.acquire`, `http.get/post` (capability-gated).

### Tier 3 — WASM component

Full Component Model + WIT interface. Capability-based — only host functions the bundle declares are linked in. For schema-driven redaction, ML-based PII detection, custom audit shapes, format translation.

```wit
package trogon:mcp-policy@0.1.0;

interface policy {
  record request-ctx {
    tenant: string,
    caller: identity,
    subject: string,
    method: string,
    params: json,
    headers: list<tuple<string, string>>,
  }

  variant decision {
    allow,
    deny(reason),
    rewrite(json),
    shape(list<u32>),  // indices to keep
    error(error-info),
  }

  authorize: func(ctx: request-ctx) -> decision;
  shape-response: func(ctx: request-ctx, response: json) -> decision;
}

world mcp-policy {
  import trogon:host/spicedb;
  import trogon:host/cache;
  import trogon:host/audit;
  export policy;
}
```

### Discipline

- **The DSL compiles to WASM internally.** Tier 1 → Tier 2 desugaring → Tier 3 IR → wasmtime. One execution path, one audit story, one perf budget.
- **Host ABI is permanent.** Treat it like a public API. Pin to WASI 0.3 (component model + async) so we don't paint ourselves into a corner.
- **Component pooling.** Pool instances per bundle version. Instantiation per message will eat the latency budget.
- **Tracing across the WASM boundary.** Pass span context as part of `request-ctx` from day one; emit child spans from inside host functions.

## SpiceDB Integration Model

Resource tuples derived per method:

| MCP method                    | Subject               | Permission | Resource                                     |
|-------------------------------|-----------------------|------------|----------------------------------------------|
| `tools/list`                  | `user:{sub}`          | `list`     | `mcp_server:{server_id}`                     |
| `tools/call`                  | `user:{sub}`          | `invoke`   | `tool:{server_id}/{tool_name}`               |
| `resources/list`              | `user:{sub}`          | `list`     | `mcp_server:{server_id}`                     |
| `resources/read`              | `user:{sub}`          | `read`     | `resource:{uri}`                             |
| `prompts/get`                 | `user:{sub}`          | `read`     | `prompt:{server_id}/{prompt_name}`           |
| `sampling/createMessage`      | `mcp_server:{id}`     | `sample`   | `user:{sub}`                                 |
| `elicitation/create`          | `mcp_server:{id}`     | `elicit`   | `user:{sub}`                                 |

`BulkCheckPermission` is used for all `*/list` shaping. ZedToken consistency scoped to MCP session id (`initialize` issues, subsequent calls re-use); cached in the host `cache` interface.

Fail-closed vs fail-open is a per-rule policy decision, expressed in the bundle. Default: fail-closed for `*/call`, `*/read`, `*/write`; fail-open with `log` decision for `*/list`.

## Redaction

Two directions:

- **Inbound (request params)** — strip secrets, normalize PII before they hit the backend server.
- **Outbound (response result)** — redact sensitive fields, mask outputs, truncate large payloads.

Driven by the **tool's schema** (cached from `tools/list`) plus a redaction policy table:

```yaml
redaction:
  "tool:db_query":
    request:
      - jsonpath: "$.params.connection_string"
        action: hash
    response:
      - jsonpath: "$.result.rows[*].ssn"
        action: mask
        pattern: "***-**-####"
      - jsonpath: "$.result.rows[*].email"
        action: classify
        classifier: "pii.email"
```

Schema-driven redaction is a WASM component (Tier 3) shipped in the MCP bundle pack — too verbose to express in CEL.

## Audit

Every decision emits one envelope to `mcp.audit.{tenant}.{outcome}.{method}`:

```json
{
  "ts": "2026-05-22T10:00:00Z",
  "trace_id": "...",
  "span_id": "...",
  "tenant": "acme",
  "caller": { "sub": "user:alice", "via": "oidc:google" },
  "subject_in": "mcp.gateway.acme.tools.call",
  "subject_out": "mcp.server.fs.tools.call",
  "method": "tools/call",
  "tool": "db_query",
  "decision": "allow",
  "rules_fired": ["tool-call-authz", "redact-db-query"],
  "rewrites": [{ "path": "$.params.connection_string", "op": "hash" }],
  "spicedb": { "zedtoken": "...", "checks": 1, "cache_hit": true },
  "latency_us": 1820
}
```

Stream: `MCP_AUDIT` on JetStream, retention `limits` with per-tenant subject quota. SIEM connectors subscribe via durable consumers.

## Bundles

- **Format** — OCI artifact or tarball: `bundle.yaml` (Tier 1) + `*.cel` (Tier 2) + `*.wasm` components (Tier 3) + `manifest.json` (declared host capabilities, version, signer).
- **Signing** — NKey signature over the manifest digest. Gateway verifies on load, rejects unsigned or unknown-signer bundles.
- **Distribution** — bundle source is configurable: NATS KV bucket, OCI registry, or HTTP. Multiple sources can layer (org-wide base + tenant override).
- **Hot-swap** — atomic version pointer; in-flight messages finish on the old version, new ones start on the new. Rollback is a pointer flip.
- **The "MCP pack"** — first-party bundle from us: resource-tuple derivation for every MCP method, catalog shaping rules, schema-learner WASM component, default audit envelope. Tenants extend/override.

## Open Questions

1. **WIT interface boundary** — one MCP-aware contract (`mcp-policy.wit` with typed `request`/`response`) versus a generic NATS-policy contract (`nats-policy.wit` over raw bytes) with MCP semantics as a library. The first is friendlier; the second is the platform play.
2. **DSL choice** — CEL versus Expr. CEL has Google's ecosystem and typed evaluation; Expr is Go-native and what Protect uses.
3. **Bundle distribution** — OCI registry first (familiar tooling) versus NATS KV first (operationally consistent with the rest of the stack).
4. **Adopt Synadia Protect for the perimeter** and only build the JSON-RPC-aware layer behind it, or **build the whole proxy ourselves**. First ships faster; second is a TrogonStack product.
5. **Bidirectional enforcement scope** — do we authorize server-initiated `sampling/createMessage` and `elicitation/create` from day one, or defer until v2?
6. **Multi-tenancy model** — tenant per NATS account (hard isolation, more ops), versus tenant as JWT claim (soft isolation, simpler).

## Phased Delivery

A possible sequencing — not committed, for discussion:

- **Phase 0** — auth callout + subject ACL on existing `mcp-nats` transport. Identity binding only, no payload inspection. Smallest possible step that proves the perimeter model.
- **Phase 1** — gateway service with Tier 1 (declarative) policies, SpiceDB authz on `tools/call` and `resources/read`, audit to JetStream. No redaction, no shaping.
- **Phase 2** — Tier 2 (CEL) expressions, `BulkCheckPermission` catalog shaping on `*/list`, ZedToken cache.
- **Phase 3** — Tier 3 (WASM components), schema cache, schema-driven redaction, bundle distribution.
- **Phase 4** — bidirectional enforcement, rate limiting, multi-source bundle composition.

## Existing Code to Lean On

- `rsworkspace/crates/mcp-nats` — JSON-RPC over NATS transport, subject parsing, peer id model. The gateway is a consumer of this crate, not a replacement.
- `rsworkspace/crates/mcp-nats-server` — Streamable HTTP frontdoor; already has `allowed_host` guard. Natural place for coarse identity binding before NATS publish.
- `rsworkspace/crates/mcp-nats-stdio` — stdio bridge; will route through the gateway namespace once it lands.
- `trogon_nats` — auth config, connection management.

The gateway crate would live alongside as `rsworkspace/crates/mcp-gateway` (or `trogon-mcp-gateway`), with the bundle/policy engine as a separate `trogon-policy` crate so it can be reused for ACP / A2A.

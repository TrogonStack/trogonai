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

## The Take (Read This First)

Everything below describes the *destination* — the full design space, including the platform-shaped ambition (generic policy engine, signed WASM bundles, multi-protocol reuse, KV-distributed control plane). It is **not the first thing to build.** The honest synthesis of all the design exploration in this document:

1. **Do not build "Protect by itself."** A generic NATS-protocol policy proxy is months of platform work, competes directly with Synadia Protect (which already exists), and is the wrong wedge for TrogonStack. If a customer needs subject-level / connect-time policy on raw NATS, point them at Protect.
2. **Do not build the full three-tier policy tower up front.** Declarative config → CEL DSL → NATS-callout plugins → WASM Component Model → signed bundles → KV-distributed control plane is the destination, not the first step. Building the engine before the product is the classic premature-abstraction trap, and most of the bundle / WIT / WASM-host-ABI machinery in this plan is platform thinking dressed as product thinking.
3. **Do build an MCP-aware gateway as a NATS queue-group service.** Narrow, opinionated, MCP-specific. CEL with SpiceDB integration on `tools/call` and `resources/read`, schema-driven redaction (the actual differentiator vs agentgateway), automatic `tools/list` filtering via CEL re-evaluation (adopt directly from agentgateway), audit to JetStream. Treat it as a feature of TrogonStack's MCP story, not a standalone product. The vertical slice in **Phased Delivery → Phase 1** is the right starting shape.
4. **Defer the generic-protocol play until a second protocol pulls on it.** When ACP, A2A, or decider commands need the same policy layer, the extraction will be obvious — and you'll know what is genuinely generic versus MCP-specific. Today there is one protocol asking for this; one protocol does not justify a platform.
5. **WASM Component Model is the long-term wedge vs agentgateway, but Phase 3 — not Phase 1.** Build with CEL only first. Only add the WASM tier when tenants ask for it or when the schema-driven redaction story can't be expressed in CEL anymore.

### The Strategic Question That Flips This

The recommendation above assumes TrogonStack's pitch is **"an event-modeling / decider-driven agentic platform"** where MCP and ACP are surfaces, and the gateway is a feature of that platform. If the pitch is instead **"the security and governance layer for NATS-based agentic systems,"** the recommendation inverts: build the generic Protect-shaped product because *that is the company*. The technical design space is the same either way; the sequencing and scope are entirely different.

This is a product-positioning decision, not a technical one. It belongs in the open questions until answered.

### What "Next" Looks Like Concretely

In order, before any code:

1. Answer the product-positioning question above (feature vs. product).
2. Pin the four irreversible technical decisions (host ABI, DSL, tenancy model, reply correlation) on paper — see **What's Missing** in the latter half of the plan.
3. Decide on-bus vs. hybrid based on whether the gateway must front third-party MCP servers (GitHub, Notion, Linear, etc.) you do not control.

Then, and only then, the **Phase 1 vertical slice**: `trogon-mcp-gateway` binary, queue-group on `mcp.gateway.request.>`, hardcoded CEL rule + one SpiceDB check on `tools/call`, audit to JetStream, end-to-end against an existing `mcp-nats` server. Approximately 2–4 weeks. Throwaway-safe — its purpose is to validate the substrate, not the policy model.

## TODO

Concrete work items derived from **The Take** and **What's Missing**. Ordered so earlier items unblock later ones. Checkboxes are intentional — this list should be edited in place as items land.

### Block A — Strategic decisions (paper, no code)

- [ ] Resolve product positioning: **feature inside TrogonStack** vs. **standalone security product**. Every other decision branches on this.
- [ ] Resolve **on-bus vs. hybrid** deployment shape. Driven by: which MCP servers must this gateway front (TrogonStack-native only, or third-party ecosystem too).
- [ ] Resolve tenancy boundary: **NATS account per tenant** vs. **tenant as JWT claim**. Affects KV namespaces, audit envelope shape, subject ACLs, JetStream stream layout — everything downstream. (Tenancy is **not** a subject segment — see **§ NATS Subject Topology → Tenancy**.)

### Block B — Irreversible technical decisions (paper, before code)

- [ ] **DSL choice.** CEL (recommended — agentgateway already validated it, ecosystem exists) vs. Expr (what Protect uses) vs. our own. Decide on the eval library too (`cel-rust` upstream or fork).
- [ ] **Reply correlation mechanism.** Gateway terminates and holds the client inbox in memory vs. NATS subject-transform with inbox preservation. Different perf and complexity.
- [ ] **Host ABI surface** — even if WASM is Phase 3, sketch the WIT interface now so CEL builtins are designed in a shape that survives the WASM port. `spicedb.check`, `cache.get/set`, `audit.emit`, `kv.fetch`, `nats.request`, `jsonpath.*`, `time.*`, `rate.acquire`.

### Block C — Design specs to write (paper, before code)

- [x] **Subject grammar** — full layout in **§ NATS Subject Topology** below: `mcp.gateway.request.>`, `mcp.gateway.callback.>`, `mcp.server.{server_id}.>`, `mcp.client.{client_id}.>`, `mcp.audit.>`, `mcp.control.>`, `mcp.plugin.{plugin_name}`. Includes reply-inbox conventions and how the gateway rewrites subjects.
- [ ] **Session model** — where session state lives across queue-group members (JetStream KV vs. sticky routing vs. session-aware subject mapping). Required for `initialize` → operate → close to work under HA.
- [ ] **Schema cache + invalidation** — keyed by `{server_id, schema_hash}`; invalidated by `notifications/tools/list_changed`, TTL fallback, version-bump on server reconnect.
- [ ] **Failure-mode matrix** — stated default for: SpiceDB unreachable, WASM panic, bundle signature invalid on hot-swap, backend MCP server timeout, gateway saturation, NATS partition. Fail-closed vs. fail-open per decision class.
- [ ] **Rate-limit state placement** — per-gateway-instance memory (fast, per-instance) vs. JetStream KV with atomic increment (cluster-wide, slower). Likely both, depending on rule scope.
- [ ] **OAuth 2.0 MCP integration** — how the MCP OAuth flow composes with NATS auth callout. Probably callout calls the OAuth provider for token verification then issues a scoped NATS JWT.
- [ ] **Bidirectional enforcement** — server→client traffic (`sampling/createMessage`, `elicitation/create`, `roots/list`) on `mcp.client.{client_id}.>`. Same policy engine, separate rule set, separate SpiceDB resource shape.
- [ ] **Bootstrap / day-zero** behavior with empty bundle (default deny vs. fall-through with audit).
- [ ] **Integration touch-points** — how the gateway emits to / consumes from `trogon-decider` (audit-as-events is the natural intersection), how it co-exists with `trogon-gateway` (Slack/etc.), whether the gateway itself is a decider.

### Block D — Phase 1 vertical slice (2–4 weeks)

Goal: prove the substrate, not the policy model. Throwaway-safe.

**Phase 1 is complete:** every item below is done. Performance baselines (P50/P99 vs direct `mcp-nats`) are deferred to Block G when we budget Phase 2+ — not required to call Phase 1 shipped.

- [x] Scaffold `rsworkspace/crates/trogon-mcp-gateway` (binary + library split).
- [x] Queue-group consumer on `mcp.gateway.request.>`; optional tenant via message header `trogon-mcp-tenant` (JWT / account-scoped tenancy in envelope still per Block A/B).
- [x] JSON-RPC parser; metadata from message headers; **verified workload JWT** ingress (`MCP_GATEWAY_JWT_*`, JWKS / static RSA PEM / HS256; phased `off` / `validate` / `require` on SpiceDB-gated RPC; **`trogon-mcp-tenant` stripped on egress when JWT ingress is active**; SpiceDB subject prefers JWT `sub` over forgeable tenant header).
- [x] CEL gate (`mcp.method == "tools/call" || mcp.method == "resources/read"`) selecting when the SpiceDB hook runs on those methods.
- [x] SpiceDB client (`spicedb-rs-client`) + one `CheckPermission` per gated `tools/call` or `resources/read` when `MCP_GATEWAY_SPICEDB_ENDPOINT` is set (allow-all when unset).
- [x] Reply correlation: ingress `reply` inbox preserved; gateway issues `request_with_headers` to `mcp.server.{id}.{method}` with the same payload; ingress without reply is forwarded as core publish only.
- [x] Audit JSON envelope to JetStream (default stream `MCP_AUDIT`; subjects `{prefix}.audit.{outcome}.request.{method_root}`, tenant field in envelope when header present).
- [x] End-to-end NATS harness (ignored unless `cargo test -p trogon-mcp-gateway -- --ignored`; see `tests/e2e_nats_forward.rs`).
- [x] In-memory trace by JSON-RPC request id (`trace::TraceStore`) — stand-in until `agctl trace`-style export (KV/service) lands.

### Block E — Phase 2 (CEL hardening + catalog shaping + redaction)

- [ ] CEL builtins implemented per host-ABI sketch (`spicedb.check`, `cache.get/set`, `jsonpath.*`, `audit.emit`, `time.now`, `rate.acquire`).
- [ ] `tools/list` filtering via CEL re-evaluation against each candidate item (adopt the agentgateway pattern directly).
- [ ] `BulkCheckPermission` + ZedToken cache keyed to MCP session id.
- [ ] Schema cache populated by sniffing `tools/list` replies; invalidation wired.
- [ ] Schema-driven redaction — first pass as native code, not WASM, attached to schemas via JSONPath rules in YAML.
- [ ] Hierarchical policy merge across subject-pattern specificity (org → tenant → server group → server → method).
- [ ] Rate limiting wired with chosen state-placement decision.

### Block F — Phase 3 (WASM components + bundles + multi-protocol)

- [ ] WIT interface (`trogon:mcp-policy@0.1.0`) finalized; pinned to WASI 0.3.
- [ ] Wasmtime integration with component pooling per bundle version.
- [ ] Tracing across the WASM boundary; span context as part of `request-ctx`.
- [ ] Bundle format (manifest + CEL + WASM components); NKey signature verification.
- [ ] Bundle loader from NATS KV with hot-swap and rollback.
- [ ] First-party `mcp-pack` bundle: resource-tuple derivation, catalog shaping, schema-learner WASM component, default audit envelope.
- [ ] NATS-callout plugin tier (Tier 2.5 ext-proc-style) on `mcp.plugin.{plugin_name}`.
- [ ] Engine extraction: `trogon-policy-core` + `trogon-policy-cel` as their own crates so ACP / A2A can reuse.

### Block G — Operational tooling

- [ ] **Latency baseline** (optional, when tightening Phase 2+ budget): P50/P99 added by the gateway vs direct `mcp-nats`.
- [ ] CLI (`trogon-gateway-ctl` or similar): inspect config, trace requests, validate bundles offline, dry-run policy against fixture traffic.
- [ ] Optional K8s controller projecting Gateway API CRDs into NATS KV (v2; only if positioning demands it).
- [ ] xDS interop layer (v2+; only if positioning demands it).
- [ ] Multi-region story: leaf-node deployment patterns, audit-stream replication, SpiceDB topology.
- [ ] OpenTelemetry trace export from each phase + JetStream consumer for audit→SIEM.

### Block H — Docs and process

- [ ] One-page operator overview (Diátaxis-style explanation): on-bus vs. hybrid, deployment topology, what the MCP server does or doesn't change.
- [ ] How-to: "put a third-party MCP server behind the gateway" (using `mcp-nats-stdio` as the server-side bridge).
- [ ] How-to: "write a bundle pack" (once Phase 3 lands).
- [ ] Reference: subject grammar, CEL variables, host ABI, audit envelope schema.
- [ ] RFC per Block-A and Block-B decision, recorded under `.trogonai/decisions/` so the irreversible choices have a paper trail.

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
| **Frontend** | TCP/TLS termination, access logging, tracing, gateway-level concerns | NATS **auth callout** at CONNECT — identity resolution (OIDC / mTLS / API key) → scoped user JWT with subject ACL `mcp.gateway.request.>` (and `mcp.gateway.callback.{my_client_id}.>` for the caller's own callbacks). TLS is the NATS server's responsibility. Tracing context bound to the connection. |
| **PreRouting** | JWT / Basic / API-key auth, extAuth, extProc, transformations — runs *before* route selection | Gateway queue-group consumer receives the NATS message, decodes JSON-RPC, validates JWT claims, applies pre-routing transformations (e.g., header injection from claims). No subject mapping yet. |
| **PostRouting** | The bulk — CORS, authentication, authorization, rate limit (local + global), extProc, transformations, header modifiers, CSRF, direct response. 14 filter types. | **The MCP policy engine runs here.** CEL evaluation, SpiceDB checks, schema-driven redaction, audit emission, rate limiting (JetStream-backed for distributed), `tools/list` shaping, request rewriting. |
| **Backend** | TLS to upstream, backend auth, AI/MCP-specific policies, health checks | Publish to `mcp.server.{server_id}.<method>` via NATS request/reply. NATS account isolation replaces backend TLS. Health is NATS subscription liveness. |

The 4-phase shape is **directly portable**. We adopt it. Filter-order is one of the things they got right and we shouldn't re-invent.

### 2. Config Distribution (xDS → NATS KV + JetStream)

agentgateway runs a control plane that ships config to data-plane proxies via **xDS** (the Envoy protocol family). Standalone mode reads YAML/JSON; Kubernetes mode uses Gateway API CRDs (`AgentgatewayPolicy`, `AgentgatewayBackend`, `AgentgatewayParameters`) translated to xDS by their controller.

NATS equivalent — and arguably better operationally because NATS is already the bus:

- **Config source of truth** → NATS KV bucket `mcp-gateway-config` (one per account when tenancy = account; tenant-keyed entries when tenancy = JWT claim) with revisions. Each policy / backend / route is a KV entry keyed by name.
- **Dynamic config push** → KV watchers. The gateway service subscribes; any KV update fires a hot reload. Revision number is the version pin.
- **Bundles (Tier 1 + Tier 2 + Tier 3 + manifest)** → NATS **JetStream Object Store** for the binary WASM artifacts; NKey signature in the manifest; KV holds the active version pointer.
- **Multi-source layering** → multiple KV buckets in priority order (org-base, account-override). The controller — if we even need one — is just a NATS service that watches some upstream (Git, OCI, K8s CRDs) and projects into KV.

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
| Org-wide | `mcp.gateway.request.>` | Audit envelope shape, base rate limit |
| Server group | `mcp.gateway.request.{group}-*.>` (a CEL `match` predicate, since groups are not subject segments) | Per-group SpiceDB resource template |
| Specific server | `mcp.gateway.request.{server_id}.>` | Override redaction for one server |
| Method | `mcp.gateway.request.{server_id}.tools.call` | Per-method rules |
| Tenant | *(not a subject — applied via JWT-claim predicate or NATS account scope)* | Tenant-wide tool allowlist |

Merge is most-specific-wins per field. Same model, expressed in subject patterns rather than YAML target/backend hierarchy.

### 5. Virtual MCP Federation (Target-Prefix Multiplexing)

agentgateway's "Virtual MCP" combines N backend MCP servers into one logical server by prefixing tool names with target name: `time_get_current_time`, `everything_echo`. Collision resolution is free because the prefix is unique per target.

NATS substrate: this is **trivial** because subjects already encode `mcp.server.{server_id}` — the prefix is the server id, no separate config needed.

- Federated `tools/list` becomes a fan-out: gateway receives `mcp.gateway.request.virtual-default.tools.list` → fans out to `mcp.server.*.tools.list` for each registered server in the federation member list → collects replies → prefixes each tool's `name` with `{server_id}_` → applies CEL filtering → returns one merged list.
- `tools/call` reverses: gateway receives `mcp.gateway.request.virtual-default.tools.call` with `params.name = "github_create_issue"` → splits on first `_` → routes to `mcp.server.github.tools.call` with rewritten `params.name = "create_issue"`.

Federation is a pure gateway-layer transform. The backend MCP servers don't know they've been multiplexed.

### 6. Extensibility (ExtProc → NATS-callout + WASM)

agentgateway's escape hatch is **ExtProc** — Envoy's external-processing protocol over gRPC. The proxy ships headers/body to an external server, the server returns modifications, the proxy applies them. `failClosed` by default. They do **not** support WASM filters.

We have a strictly better story available because we're on NATS:

**Tier-2.5: NATS-callout plugins.** Instead of dialing out to a separate gRPC server, the gateway issues a NATS request on `mcp.plugin.{plugin_name}` with the request envelope, and any subscriber (with the right entitlement) replies with the modification. Same out-of-process plugin model, same failClosed default, but:

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

NATS equivalent is the natural one: audit envelope per decision → JetStream stream `MCP_AUDIT` with subject `mcp.audit.{outcome}.{direction}.{method_root}` (tenant identity lives in the envelope payload). Hash-chained envelopes (each envelope includes prior envelope's digest) for tamper-evidence. SIEM consumers subscribe via durable JetStream consumers. OpenTelemetry traces emit alongside but the audit stream is the legal record.

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

Two **zones**, separated by NATS authorization (subject ACL baked into JWT issued by the auth callout). Full subject grammar in **§ NATS Subject Topology** below; the short version:

- **Edge zone** — `mcp.gateway.request.>` (client → server) and `mcp.gateway.callback.>` (server → client) — client-facing. Clients (or edge bridges) publish on the request side; the gateway subscribes there. Gateway publishes on the callback side; the matching client subscribes there. Clients cannot publish anywhere else.
- **Backend zone** — `mcp.server.{server_id}.>` and `mcp.client.{client_id}.>` — gateway-private. Uses the existing `mcp-nats` subject convention unchanged. Only the gateway holds publish permission on `mcp.server.*.>`; only the gateway holds subscribe permission on `mcp.client.*.>` (it receives server-initiated traffic and relays).

Existing `mcp-nats` MCP servers run **unchanged**. The gateway is purely additive — it introduces a new edge zone and translates between zones. Clients and servers are isolated from each other by NATS subject permissions, not by convention.

### Components

1. **Auth callout service** — connect-time identity resolution. Maps external credentials (OIDC, mTLS, API key) to a scoped NATS user JWT with subject ACL covering `mcp.gateway.request.>` (plus the caller's own `mcp.gateway.callback.{client_id}.>` subtree for server-initiated traffic). Tenant identity is asserted as a JWT claim, not a subject segment.
2. **Gateway service** — queue-group subscriber on `mcp.gateway.request.>` and `mcp.client.>`. Stateless per-message; correlation state lives in-flight only.
3. **Policy engine** — embedded in the gateway. Three-tier (see below). One execution model.
4. **Audit emitter** — every decision → JetStream stream `MCP_AUDIT` on subject root `mcp.audit.>` with JSON envelope.
5. **Bundle loader** — fetches signed bundles from a configured source (NATS KV, OCI registry, HTTP), verifies NKey signature, hot-swaps in place.
6. **Schema cache** — learns each backend server's tool / resource / prompt schemas by inspecting `tools/list` etc. replies; keyed by server id + version; used by redaction policies.

### Request flow

```
client ──PUB──▶ mcp.gateway.request.{server_id}.tools.call
                        │
                        ▼
                ┌──────────────────┐
                │  Gateway service │
                │  (queue group)   │
                └──────────────────┘
                        │
        1. parse JSON-RPC (method, params, id)
        2. resolve identity from JWT claims (sub, tenant, roles)
        3. derive resource tuple from method+params
        4. policy.authorize(request)         ──▶ SpiceDB CheckPermission
        5. policy.rewrite(request.params)    ──▶ input redaction
        6. publish mcp.server.{server_id}.tools.call
        7. await reply on inbox
        8. policy.shape(response.result)     ──▶ catalog filter (if list)
        9. policy.rewrite(response.result)   ──▶ output redaction
       10. publish audit envelope            ──▶ mcp.audit.{outcome}.request.tools
       11. reply to client on original inbox
```

Bidirectional traffic (server → client `sampling/createMessage`, `roots/list`, `elicitation/create`) is handled symmetrically: gateway subscribes on `mcp.client.{client_id}.>`, applies the callback-direction policy set, and re-publishes on `mcp.gateway.callback.{client_id}.{method}`. Subject mechanics in **§ NATS Subject Topology**.

## NATS Subject Topology

The full subject layout. This is the design's most foundational artifact — every other decision (subject ACLs, federation, audit filtering, multi-region routing, queue-group sharding) is downstream of it. Goals:

1. **Subject permissions enforce isolation.** Clients literally cannot reach servers, and servers literally cannot reach clients, without the gateway in the middle. Enforced by NATS subject ACL, not convention.
2. **Backwards-compatible with `mcp-nats`.** Backend zone reuses the existing transport's subjects verbatim — no breaking changes to the transport crate.
3. **Direction, target, and method legible from the subject alone.** Routing, filtering, audit aggregation, and rate-limit keying never have to parse the payload.
4. **Short enough for throughput.** NATS subject parsing is O(segments); 4–6 segments is the sweet spot.
5. **Tenancy is not a subject segment.** Multi-tenancy is expressed via NATS accounts and/or per-deployment prefix overrides — not by adding a `{tenant}` token to every subject. See **§ Prefix convention** and **§ Tenancy** below.

### Prefix convention (aligned with existing crates)

`mcp-nats` and `acp-nats` both ship a configurable subject prefix today:

- `mcp-nats` reads `MCP_PREFIX` env var, default `"mcp"`. Subjects branch as `{MCP_PREFIX}.server.{id}.{method}` and `{MCP_PREFIX}.client.{client_id}.{method}`.
- `acp-nats` reads `ACP_PREFIX` env var, default `"acp"`. Subjects branch as `{ACP_PREFIX}.agent.ext.{method}` and `{ACP_PREFIX}.{session_id}.agent.*`.

The gateway **reuses the same `MCP_PREFIX`** — it does not introduce a new env var. Every subject is spelled out — no short tokens — so an operator reading a subject in a log line can tell at a glance what it is. All gateway subjects are sibling subtrees under the one prefix:

```
{MCP_PREFIX}.gateway.request.{server_id}.{method}            # edge zone, client → server
{MCP_PREFIX}.gateway.callback.{client_id}.{method}           # edge zone, server → client
{MCP_PREFIX}.server.{server_id}.{method}                     # backend zone (existing mcp-nats)
{MCP_PREFIX}.client.{client_id}.{method}                     # backend zone (existing mcp-nats)
{MCP_PREFIX}.audit.{outcome}.{direction}.{method_root}       # outcome = allow|deny|rewrite|error
{MCP_PREFIX}.control.<...>                                   # cache invalidation, discovery, heartbeats
{MCP_PREFIX}.plugin.{plugin_name}                            # NATS-callout policy plugins
```

Default `MCP_PREFIX=mcp` yields `mcp.gateway.request.github.tools.call`, `mcp.audit.deny.request.tools`, etc. An operator who wants extra partitioning (multiple MCP installs on one cluster, environment-per-prefix, customer-per-prefix in a single-tenant-per-deploy model) sets `MCP_PREFIX=trogon.mcp` or `MCP_PREFIX=acme.mcp` and every subject shifts together.

For the rest of this section, subjects are written with `mcp.` as the prefix for readability — understand it as `{MCP_PREFIX}.` throughout.

### Two zones

| Zone | Subject root | Who publishes | Who subscribes |
|---|---|---|---|
| **Edge** (gateway-facing) | `mcp.gateway.>` | clients, edge bridges, *and* gateway (for callbacks out) | gateway, and clients (for their own callback subjects) |
| **Backend** (gateway-private, existing `mcp-nats`) | `mcp.server.{server_id}.>` / `mcp.client.{client_id}.>` | gateway (server-bound), backend servers (callbacks) | backend servers (own subject), gateway (all client-bound) |

### Subject grammar

#### Edge zone — client → server (`request` direction)

```
mcp.gateway.request.{server_id}.{method_path}
```

| Segment | Values | Notes |
|---|---|---|
| `request` | literal | Direction marker — message is client-initiated, flowing toward a server. |
| `{server_id}` | `[a-z0-9-]+` or `virtual-{id}` | Logical server target. `virtual-*` indicates a federated/multiplexed target the gateway fans out. |
| `{method_path}` | `tools.list` \| `tools.call` \| `resources.list` \| `resources.read` \| `resources.subscribe` \| `prompts.list` \| `prompts.get` \| `completion.complete` \| `initialize` \| `ping` \| `logging.setLevel` \| `notifications.initialized` \| `notifications.cancelled` \| `notifications.roots.list_changed` \| ... | MCP method with `/` → `.`. |

Examples:

```
mcp.gateway.request.github.tools.call
mcp.gateway.request.github.tools.list
mcp.gateway.request.virtual-default.tools.list        # federated fan-out
mcp.gateway.request.github.notifications.initialized
```

#### Edge zone — server → client (`callback` direction)

```
mcp.gateway.callback.{client_id}.{method_path}
```

| Segment | Values | Notes |
|---|---|---|
| `callback` | literal | Direction marker — message is server-initiated, flowing back to a client. |
| `{client_id}` | `[a-z0-9-]+` | Logical client target. Gateway learns this from `initialize`. |
| `{method_path}` | `sampling.createMessage` \| `elicitation.create` \| `roots.list` \| `notifications.tools.list_changed` \| `notifications.resources.list_changed` \| `notifications.resources.updated` \| `notifications.prompts.list_changed` \| `notifications.progress` \| `notifications.message` | Server → client MCP methods. |

#### Backend zone — gateway → server (existing `mcp-nats`)

```
mcp.server.{server_id}.{method_path}
```

Verbatim from `rsworkspace/crates/mcp-nats`. The gateway is the only allowed publisher.

#### Backend zone — server → gateway callbacks (existing `mcp-nats`)

```
mcp.client.{client_id}.{method_path}
```

Backend servers publish here when they need a client callback. The gateway is the only allowed subscriber.

### Reply inboxes and correlation

NATS request/reply uses an auto-generated `_INBOX.{nuid}` per request, set as the `reply-to` header. The gateway **terminates** correlation: it does not pass the client's inbox down to the backend; it creates its own.

```
1. client publishes mcp.gateway.request.github.tools.call
                                       reply-to: _INBOX.client.{nuid_c}

2. gateway receives, applies policy, then publishes
   mcp.server.github.tools.call
                    reply-to: _INBOX.gateway.{nuid_g}

3. gateway holds map: nuid_g → { original_reply_to: _INBOX.client.{nuid_c},
                                  request_ctx, span_ctx, deadline }

4. backend server replies to _INBOX.gateway.{nuid_g}

5. gateway receives reply, looks up nuid_g → context, applies response
   policy (redaction, list filtering), then publishes to the original
   _INBOX.client.{nuid_c}.
```

In-flight correlation state is **in-memory per gateway instance**. If a gateway instance dies mid-call, the request fails (client times out and retries; queue group routes the retry to a survivor). No JetStream KV for in-flight — that buys reliability we don't need at the cost of latency we can't afford.

### Translation table

Operations the gateway performs on each subject class:

| Direction | Inbound subject (gateway subscribes) | Outbound subject (gateway publishes) | Operations |
|---|---|---|---|
| Client → Server, request | `mcp.gateway.request.{server_id}.{method}` | `mcp.server.{server_id}.{method}` | Authn, authz, input redact, audit, set own reply inbox |
| Server → Gateway, response | `_INBOX.gateway.{nuid_g}` | original `_INBOX.client.{nuid_c}` | Output redact, `*/list` shape, audit |
| Server → Client, callback | `mcp.client.{client_id}.{method}` | `mcp.gateway.callback.{client_id}.{method}` | Authz (callback direction), input redact, audit, set own reply inbox |
| Client → Gateway, callback response | `_INBOX.gateway.{nuid_g2}` | original server-side `_INBOX.{nuid_s}` | Output redact, audit |
| Notification, either direction | corresponding `request`/`callback` subject | corresponding backend subject | Authz, redact, audit; no reply path |

### Virtual MCP (federation) in subjects

Federated server id appears in subjects as `virtual-{id}`. The gateway expands the fan-out in its policy layer:

```
Client publishes:
  mcp.gateway.request.virtual-default.tools.list
                          │
                          ▼ gateway fans out to all members of "default"
  mcp.server.github.tools.list
  mcp.server.linear.tools.list
  mcp.server.notion.tools.list
                          │
                          ▼ gateway collects replies, prefixes tool names
                             with target id, runs CEL filter per-item
                             ("github_create_issue", "linear_create_issue", ...)
                          ▼
  one merged tools.list response back to the client
```

`tools/call` reverses the prefix: `params.name = "github_create_issue"` → split on first `_` → `mcp.server.github.tools.call` with `params.name = "create_issue"`.

The fan-out membership list lives in the gateway's config (NATS KV); not encoded in the subject.

### Subject ACL per principal

Permissions baked into the JWT issued by the auth callout. Listed as `publish` / `subscribe` allow sets:

| Principal | Publish | Subscribe |
|---|---|---|
| **Client / edge bridge** | `mcp.gateway.request.>` `_INBOX.client.>` | `mcp.gateway.callback.{my_client_id}.>` `_INBOX.client.>` |
| **Backend MCP server** | `mcp.client.>` `_INBOX.>` (own replies) | `mcp.server.{my_server_id}.>` |
| **Gateway service** | `mcp.server.>` `mcp.gateway.callback.>` `mcp.audit.>` `mcp.plugin.>` `mcp.control.>` `_INBOX.gateway.>` | `mcp.gateway.request.>` `mcp.client.>` `mcp.control.>` `_INBOX.gateway.>` |
| **Audit consumer (SIEM)** | *(none)* | `mcp.audit.>` (durable JetStream consumer) |
| **Policy plugin (ext-proc-style)** | `_INBOX.>` (replies) | `mcp.plugin.{my_plugin_name}.>` |

The combination of (a) clients can only publish on `mcp.gateway.request.>` and (b) only the gateway can publish on `mcp.server.>` means **a malicious or buggy client cannot reach a backend MCP server even if it learns the server's id**. The gateway is not a recommended chokepoint — it's the only path the math allows.

### Audit subjects

```
mcp.audit.{outcome}.{direction}.{method_root}
```

| Segment | Values |
|---|---|
| `{outcome}` | `allow` \| `deny` \| `rewrite` \| `error` |
| `{direction}` | `request` \| `callback` |
| `{method_root}` | `tools` \| `resources` \| `prompts` \| `sampling` \| `elicitation` \| `roots` \| `initialize` \| `notification` \| `other` |

Full method, target id, caller, tenant claim, rules-fired, rewrites, SpiceDB decision, latency, span ctx live in the **envelope payload**, not the subject. Subject is for filtering.

Examples:

```
mcp.audit.deny.request.tools           # SIEM subscriber for denials only
mcp.audit.rewrite.callback.sampling    # redactions on server→client callbacks
mcp.audit.allow.request.tools          # full allow trace (volume; sample at consumer)
```

JetStream stream name: `MCP_AUDIT` with subject filter `mcp.audit.>`. Retention quotas via stream limits. Hash-chained envelopes (each envelope carries prior digest) for tamper-evidence. When multiple deployments share a cluster, per-deployment `MCP_PREFIX` overrides naturally partition the audit stream.

### Control plane subjects

| Purpose | Subject |
|---|---|
| Schema-cache invalidation broadcast (when any gateway sees `notifications/tools/list_changed` from a server, all peers drop the cache for that server) | `mcp.control.cache.invalidate.{server_id}` |
| Server registration / discovery (servers announce on startup) | `mcp.control.discovery.register.{server_id}` |
| Server deregistration / shutdown | `mcp.control.discovery.deregister.{server_id}` |
| Gateway instance heartbeat | `mcp.control.gateway.heartbeat.{instance_id}` |
| Policy bundle reload signal (optional — KV watcher is primary path) | `mcp.control.bundle.reload` |

Config bundles themselves live in **NATS KV** (bucket `mcp-gateway-config`) plus **JetStream Object Store** for binary WASM artifacts. KV watchers are the hot-reload mechanism; the `mcp.control.bundle.reload` subject is a belt-and-braces signal for non-watcher implementations. When tenancy = NATS account, each account naturally gets its own KV bucket; when tenancy = JWT claim, a single bucket holds entries keyed by tenant.

### Plugin / ext-proc subjects

NATS-callout policy plugins (Tier 2.5):

```
mcp.plugin.{plugin_name}
```

Gateway publishes a request envelope here with a reply inbox. Any subscriber on `mcp.plugin.{plugin_name}` in a queue group (named after the plugin) picks it up, processes, and replies. Multiple plugins are different `{plugin_name}` segments. failClosed by default — if no reply within the configured deadline, the gateway treats it as `deny`.

### Notifications

MCP notifications are one-way (no reply path). Use the same subject grammar as requests but publish without a reply inbox:

```
mcp.gateway.request.github.notifications.initialized              # client → server, fire-and-forget
mcp.gateway.callback.client-1.notifications.tools.list_changed    # server → client, fire-and-forget
```

Gateway still applies policy and audits, but skips reply correlation.

### Session correlation

MCP `initialize` returns a session id used in subsequent requests via the `_meta` or `sessionId` header. Multi-instance gateway can't keep session state in memory per-instance, so:

- **JetStream KV** bucket `mcp-sessions`, keyed by session id, TTL on idle. Stores: client_id, tenant claim (if soft tenancy), server_id binding(s), session-scoped ZedToken, schema versions in use, rate-limit budgets. Under hard tenancy the bucket is per-account; under soft tenancy a single bucket is namespaced by tenant claim in the key.
- Any gateway instance handling a request looks up the session before policy eval.
- For high-throughput / latency-sensitive deployments, **session-affinity via NATS subject mapping** is an alternative: include the session id in a subject token (e.g., `mcp.gateway.request.{server_id}.{method}.{session_id}` with consistent-hash routing), so the same gateway instance handles all requests for one session. Tradeoff: extra subject segment, harder to express subject ACL.

The KV-based approach is the default; session-affinity is a Phase-3 perf optimization.

### Tenancy

Tenancy is not a subject segment. It is expressed by one (or a combination) of two mechanisms, both above the subject layer:

1. **NATS account per tenant** (hard isolation, recommended default for multi-customer deployments). Subjects are identical across accounts — `mcp.gateway.request.github.tools.call` exists in every tenant's namespace — but accounts cannot reach each other's subjects without explicit imports/exports. The gateway runs once per account, or one gateway uses account imports to span tenants. KV buckets, JetStream streams, and audit retention all scope naturally to the account.
2. **Per-deployment `MCP_PREFIX` override** (soft, simple partitioning). One operator wants `acme` and `globex` on the same NATS cluster without going through account configuration; set `MCP_PREFIX=acme.mcp` for one deployment and `MCP_PREFIX=globex.mcp` for the other. Subjects shift wholesale (`acme.mcp.gateway.request.github.tools.call`, `globex.mcp.gateway.request.github.tools.call`) and the gateway code is unaware. This matches the existing convention in `mcp-nats` and `acp-nats`.

A tenant **claim** in the JWT remains useful even under hard tenancy — for the audit envelope payload, for SpiceDB principal naming, and for cross-account aggregation at the SIEM layer. But it does not appear in the subject. The subject answers "what message is this and where is it going"; the tenant answers "who is the message from", and that belongs in identity (JWT, account, headers), not topology.

The default recommendation: **account-per-tenant in production, single account + tenant-claim-in-JWT for dev/single-customer deployments, and `MCP_PREFIX` override available as a partitioning escape hatch in either mode.**

### Subject length / throughput note

Typical edge-zone subject has ~5–7 segments. Example:

```
mcp.gateway.request.github.tools.call         # 6 segments, 37 chars
mcp.audit.deny.request.tools                  # 5 segments, 28 chars
mcp.control.cache.invalidate.github           # 5 segments, 35 chars
```

Well within NATS's comfortable range. Subject hashing and wildcard subscription are unaffected. For comparison, NATS itself uses subjects up to ~12 segments for internal `$SYS` traffic without issue.

### Migration / compatibility with existing `mcp-nats`

The existing transport in `rsworkspace/crates/mcp-nats` uses `MCP_PREFIX` (default `mcp`) and subjects `mcp.server.{server_id}.{method}` / `mcp.client.{client_id}.{method}`. The gateway design **does not require any change** to that crate — backend zone keeps those subjects verbatim. The gateway is purely additive at the subject level.

The one new requirement on the transport: clients that want to talk to a gateway-fronted deployment use a new `mcp-gateway-client` adapter that targets the `mcp.gateway.request.>` edge zone instead of the backend `mcp.server.{server_id}.>` directly. For the bridge crates (`mcp-nats-stdio`, `mcp-nats-server`) the same applies — a configuration toggle picks edge zone vs backend zone. `MCP_PREFIX` is unchanged across the boundary.

### Open decisions feeding back into the topology

The subject design above leaves a few things deliberately under-specified because they depend on Block-A / Block-B decisions in **TODO**:

- **Whether session id appears in the subject** for affinity routing. Current default: no, session in KV.
- **Whether `request` and `callback` directions stay as separate segments** or could be unified into a single shape with role inferred from method. Current default: separate, because separate makes subject ACLs trivial.
- **Whether `virtual-{id}` belongs in the subject** or whether federation is fully transparent (gateway looks up the real server id from a separate registry per call). Current default: subject-encoded, because it makes operator debugging easier.
- **Whether the auth callout emits a tenant claim** under hard account tenancy. Current default: yes, for audit-envelope legibility and SpiceDB principal naming, even though the subject itself does not carry it.

## Policy Engine

Three tiers, strictly increasing in power. Lower tiers desugar into higher tiers. **One execution engine** under the hood (WASM).

### Tier 1 — Declarative config

YAML. Covers the boring 60%.

```yaml
rules:
  - name: deny-after-hours-writes
    when:
      subject: "mcp.gateway.request.*.tools.call"
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
    tenant: string,                      // from JWT claim, not subject

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

Every decision emits one envelope to `mcp.audit.{outcome}.{direction}.{method_root}` (tenant identity lives in the envelope payload, not the subject):

```json
{
  "ts": "2026-05-22T10:00:00Z",
  "trace_id": "...",
  "span_id": "...",
  "tenant": "acme",
  "caller": { "sub": "user:alice", "via": "oidc:google" },
  "subject_in": "mcp.gateway.request.fs.tools.call",
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

Stream: `MCP_AUDIT` on JetStream, retention `limits`. Per-tenant quotas via per-account stream limits when tenancy = account, or by deploying separate `MCP_PREFIX` overrides per tenant. SIEM connectors subscribe via durable consumers filtered on `mcp.audit.>`.

## Bundles

- **Format** — OCI artifact or tarball: `bundle.yaml` (Tier 1) + `*.cel` (Tier 2) + `*.wasm` components (Tier 3) + `manifest.json` (declared host capabilities, version, signer).
- **Signing** — NKey signature over the manifest digest. Gateway verifies on load, rejects unsigned or unknown-signer bundles.
- **Distribution** — bundle source is configurable: NATS KV bucket, OCI registry, or HTTP. Multiple sources can layer (org-wide base + per-account override).
- **Hot-swap** — atomic version pointer; in-flight messages finish on the old version, new ones start on the new. Rollback is a pointer flip.
- **The "MCP pack"** — first-party bundle from us: resource-tuple derivation for every MCP method, catalog shaping rules, schema-learner WASM component, default audit envelope. Tenants extend/override.

## Wire-Format Pins for Phase 1

These are the on-the-wire contracts Phase 1 implements. Pinned now because each one bakes into headers, audit envelopes, JSON-RPC error responses, CEL rules, or the inbox subscription topology — changing any of them after consumers exist is a coordinated migration. Anything not listed here is allowed to change during Phase 1.

### 1. NATS message headers

Every gateway-handled message carries a fixed header set. Headers are authoritative for routing and policy; the JSON-RPC payload is for protocol semantics.

| Header | Direction | Type | Source | Purpose |
|---|---|---|---|---|
| `traceparent` | both | W3C string | client (or gateway if absent) | Distributed-tracing parent span (W3C Trace Context). |
| `tracestate` | both | W3C string | client / gateway | Vendor-specific trace state. |
| `mcp-schema` | both | string, e.g. `trogon.mcp/v1` | gateway sets on egress | Wire-format version. Major bumps break consumers. |
| `mcp-session-id` | both | opaque string | gateway issues at `initialize` | Stable across an MCP session. Routes to session KV. |
| `mcp-caller-sub` | gateway → backend | string | gateway, from JWT `sub` | The authenticated principal. Backend may log/refuse. |
| `mcp-tenant` | gateway → backend, audit | string | gateway, from JWT claim | Tenant identity mirror for log legibility — never trusted as authority. Stripped/overwritten on ingress; payload still carries it. |
| `mcp-deadline-unix-ms` | client → gateway → backend | integer string | client (gateway clamps) | Absolute deadline, milliseconds since epoch. Gateway propagates after clamping to configured max. |
| `mcp-correlation-id` | both | string | client (optional) | Opaque client correlator surfaced in audit envelope. |
| `mcp-instance-id` | gateway → backend | string | gateway | Which gateway instance handled the request. Used for reply-inbox routing and audit. |
| `mcp-act-chain` | client → gateway, gateway → backend | compact JSON string | gateway (egress); client may send but is stripped on ingress | Delegation lineage: JSON array of `{sub, agent_id?, wkl?, iat}` objects (plain UTF-8 JSON — not base64url). `sub` and `iat` (Unix seconds) required per entry; `agent_id` and `wkl` omitted when absent. Gateway **drops** any client-supplied value on ingress, parses the prior chain (if any), appends its own hop (`sub` from `MCP_GATEWAY_IDENTITY_SUB`, default `trogon-mcp-gateway`), and sets the header before forwarding. Max depth **8** — at capacity the gateway forwards without append and logs `act_chain_too_deep` (enforce-mode rejection reserved). Not authoritative over the validated egress JWT when both are present. See `docs/identity/act-chain.md`, ADR 0002. |

**Ingress hardening rule.** On every message entering the gateway from the edge zone, the gateway **drops** any client-supplied value of `mcp-caller-sub`, `mcp-tenant`, `mcp-instance-id`, and `mcp-schema` and replaces them with values derived from the JWT and its own state. Clients cannot forge identity by header injection.

### 2. Reply inbox naming

```
_INBOX.gateway.{instance_id}.{nuid}        # gateway → backend reply correlation
_INBOX.client.{nuid}                       # client → gateway reply correlation (client-chosen)
```

- `{instance_id}` is a NUID generated at gateway-process boot and exposed on `mcp.control.gateway.heartbeat.{instance_id}`.
- Each gateway instance subscribes **only** to `_INBOX.gateway.{my_instance_id}.>` — no cross-talk between instances.
- Client inbox shape is the client's choice; the gateway never subscribes there. The original `reply-to` from the client is held in the in-memory correlation map keyed by `{nuid}` of the gateway-side inbox.
- If `{instance_id}` collides on restart, the prior in-flight requests are already lost (process died); no recovery needed.

### 3. Queue group strategy

| Subscription | Queue group name | Reason |
|---|---|---|
| `mcp.gateway.request.>` | `mcp-gateway` | Single group; any healthy instance can serve any request. |
| `mcp.client.>` | `mcp-gateway-callbacks` | Same instances, separate group so request and callback fairness are independent. |
| `_INBOX.gateway.{my_instance_id}.>` | *(none — direct subscribe)* | Per-instance; queue-grouping would break correlation. |
| `mcp.plugin.{plugin_name}` | `mcp-plugin-{plugin_name}` | Plugin authors get queue-group scale automatically. |

**Backpressure.** Per-target inflight semaphore (default cap 256 per `server_id`) plus per-tenant inflight cap (default 4096). On saturation, gateway returns JSON-RPC error code `-32105` (see §6) with `data.retry_after_ms`. Caps are configurable per server in the bundle / KV config. This avoids per-method queue-group sprawl while bounding head-of-line blocking.

### 4. Virtual-MCP name separator

Federated tool names use `::` as the target-prefix separator:

```
github::create_issue
linear::create_ticket
notion::search_pages
```

Rationale: the MCP spec restricts tool names to a pattern that excludes `:`, so `::` cannot collide with any legal native tool name. Single `:` would be ambiguous with URI schemes in resource references. The separator is configurable per virtual server in the bundle (`separator: "::"` default) for operators who need to interop with prior conventions.

**Splitting rule:** on `tools/call`, the gateway splits on the **first** `::` only; anything after the first separator is treated as the original tool name verbatim. This permits tools whose native name contains `::` (unusual but legal in some servers).

### 5. `initialize` handshake handling

**Gateway-terminated by default.** The gateway answers `initialize` itself; the client sees the gateway as the MCP server.

- Gateway returns its own `serverInfo` (name `trogon-mcp-gateway`, version) and an aggregated `capabilities` object reflecting the union of features the federation supports — `tools`, `resources`, `prompts`, `logging`, `completions`, `sampling`, `elicitation`, etc.
- Client's `clientInfo` and `protocolVersion` are recorded in the session KV under the issued `mcp-session-id`.
- For **non-virtual** targets, the gateway lazily forwards an `initialize` to the chosen backend on first method call so the backend can do per-session setup. The backend response is recorded in the session KV; subsequent calls reuse it.
- For **virtual** targets, the gateway lazily initializes each federation member on first call to that member. Members not yet initialized when `tools/list` runs are initialized in parallel before the merge.

The lazy-forward keeps `initialize` cheap (no fan-out for clients that never call anything) while still giving backends a chance to do session setup before real work hits them.

### 6. Gateway-emitted JSON-RPC error codes

Trogon application errors occupy `-32100` to `-32199` (within the JSON-RPC application-error range, distinct from MCP's `-32002` and the protocol-reserved `-32700` … `-32600`).

| Code | Symbol | Meaning | `data` shape |
|---|---|---|---|
| `-32100` | `policy_deny` | Authorization rule denied the request. | `{ trace_id, rule_fired, reason }` |
| `-32101` | `policy_fault` | CEL evaluation error, WASM trap, bundle missing. failClosed result. | `{ trace_id, tier, error }` |
| `-32102` | `backend_timeout` | Backend MCP server did not reply within the request deadline. | `{ trace_id, server_id, elapsed_ms }` |
| `-32103` | `backend_unreachable` | No backend matched the target or queue-group has no consumers. | `{ trace_id, server_id }` |
| `-32104` | `schema_unknown` | inputSchema not in cache and could not be fetched; redaction cannot validate. | `{ trace_id, server_id, tool }` |
| `-32105` | `rate_limited` | Inflight cap or rate budget exceeded. | `{ trace_id, scope, retry_after_ms }` |
| `-32106` | `auth_expired` | JWT expired mid-session, or session revoked. | `{ trace_id }` |
| `-32107` | `authz_unreachable` | SpiceDB (or chosen PDP) did not respond. failClosed result. | `{ trace_id, elapsed_ms }` |
| `-32108` | `no_policy` | Bundle not loaded, default-deny configured. | `{ trace_id }` |
| `-32109` | `audience_mismatch` | JWT `aud` does not match the expected gateway or backend URI (enforce mode). | `{ trace_id, expected_aud, actual_aud }` |
| `TODO(enforce-mode-hardening)` | `agent_identity_required` *(code TBD; `-32108` is the likely candidate)* | Reserved for enforce-mode identity checks (missing `wkl`, SPIFFE/`agent_id` mismatch, act-chain depth reject, etc.) not yet pinned. | TBD |

`trace_id` is always present and matches the `traceparent` header for cross-system correlation. Error message text is human-readable but **not** part of the contract — only the code and `data` shape are stable.

### 7. Audit envelope schema

Every audit envelope is JSON with a fixed top-level schema field. Forward-compat rule: consumers MUST tolerate unknown fields; consumers MAY refuse unknown major versions.

```json
{
  "schema": "trogon.mcp.audit/v1",
  "ts": "2026-05-22T10:00:00Z",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "span_id": "b7ad6b7169203331",
  "instance_id": "NB7K…",
  "tenant": "acme",
  "session_id": "sess_…",
  "caller": { "sub": "user:alice", "via": "oidc:google", "roles": ["engineer"] },
  "subject_in": "mcp.gateway.request.github.tools.call",
  "subject_out": "mcp.server.github.tools.call",
  "direction": "request",
  "method": "tools/call",
  "method_root": "tools",
  "tool": "create_issue",
  "decision": "allow",
  "rules_fired": ["tool-call-authz", "redact-github-pat"],
  "rewrites": [{ "path": "$.params.token", "op": "hash" }],
  "spicedb": { "zedtoken": "…", "checks": 1, "cache_hit": true },
  "error": null,
  "latency_us": 1820,
  "agent_id": "acme/oncall-responder",
  "agent_version": "3.2.1",
  "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder",
  "purpose": "incident-response",
  "act_chain": [
    { "sub": "user:alice@acme.com", "wkl": "human", "iat": 1748341200 },
    { "sub": "agent:acme/oncall-responder", "agent_id": "acme/oncall-responder", "wkl": "spiffe://acme.local/ns/prod/sa/oncall-responder", "iat": 1748341203 }
  ]
}
```

**Optional identity fields (ADR 0002).** Top-level `agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`, and `act_chain` are omitted from the wire when unset (`skip_serializing_if = Option::is_none` in the gateway publisher). When present, `act_chain` is a JSON array of `{sub, agent_id, wkl, iat}` objects matching the `mcp-act-chain` header / JWT claim shape. Envelopes without identity context remain byte-identical to the pre-identity schema.

Stream-level metadata also tags `schema` so an offline reader can dispatch without unpacking a message. Bumps to `v2` are reserved for additive changes that consumers must opt into; renames or removals are `v2` and break old consumers.

### 8. CEL variable namespace

Roots are pinned; fields under a root may be added without a version bump, but never renamed or moved.

| Root | Phase | Variables | Notes |
|---|---|---|---|
| `mcp.*` | both | `mcp.method` (string), `mcp.method_root` (string), `mcp.tool.name` (string), `mcp.tool.target` (string), `mcp.resource.uri` (string), `mcp.prompt.name` (string), `mcp.params` (JSON map), `mcp.result` (JSON map; response phase only) | Protocol surface. |
| `jwt.*` | both | `jwt.sub` (string), `jwt.tenant` (string), `jwt.roles` (list<string>), `jwt.iss` (string), `jwt.aud` (string), `jwt.agent_id` (string, optional — registered agent identity, ADR 0002), `jwt.agent_version` (string, optional), `jwt.wkl` (string — attested workload: SPIFFE URI or `sentinel:*`), `jwt.purpose` (string, optional intent claim), `jwt.session_id` (string, optional), `jwt.act_chain` (list<map> — see `docs/identity/act-chain.md`), `jwt.<custom>` | Claims from validated JWT. Custom claims are dynamic. |
| `chain.*` | both | *(functions, bound to `jwt.act_chain`)* — `chain.contains(agent_id) -> bool`, `chain.originator() -> map`, `chain.depth() -> int` | Delegation lineage helpers (ADR 0002). |
| `nats.*` | both | `nats.subject` (string), `nats.headers` (map<string,string>), `nats.account` (string) | Transport context. |
| `request.*` | both | `request.id` (string), `request.deadline` (timestamp), `request.session_id` (string) | Request-level context. |
| `response.*` | response only | `response.is_error` (bool), `response.list_filter_index` (int) | The list-filter index is set when re-evaluating the same rule per-item during `*/list` shaping. |
| `time.*` | both | `time.now` (timestamp), `time.hour_utc` (int), `time.weekday` (int 0–6) | Host-evaluated. |
| `spicedb.*` | both | *(functions, not variables)* — `spicedb.check(subject, perm, resource) -> bool`, `spicedb.bulk_check(subject, perm, [resources]) -> map<string,bool>` | Capability-gated host import. |
| `cache.*` | both | *(functions)* — `cache.get(key) -> any`, `cache.set(key, value, ttl) -> bool` | ZedToken caching, schema caching. |
| `audit.*` | both | *(functions)* — `audit.emit(extra_fields)` | Merged into the audit envelope's `extra` field. |
| `rate.*` | both | *(functions)* — `rate.acquire(scope, key, budget, window) -> bool` | Returns false when rate-limited; rule typically denies on false. |

The `mcp.params` and `mcp.result` JSON-map roots support indexing (`mcp.params.name`, `mcp.params["complex-key"]`) and traversal with the standard CEL operators. Deep traversal into untyped JSON uses the `jsonpath.*` host functions for ergonomic access.

### 9. Per-target inflight cap and rate-limit defaults

| Scope | Default cap | Configurable in | On exceed |
|---|---|---|---|
| Per `server_id` inflight | 256 | bundle / KV config | `-32105 rate_limited`, scope `server`, retry_after_ms set from oldest inflight age |
| Per tenant inflight | 4096 | KV config | `-32105 rate_limited`, scope `tenant` |
| Per caller `jwt.sub` rate | 100 req / 10s | bundle | `-32105 rate_limited`, scope `caller` |
| Per `(jwt.sub, tool)` rate | (unset; opt-in via CEL `rate.acquire`) | bundle | per rule |

Inflight caps are in-process semaphores per gateway instance (fast path, no NATS round-trip). Rate-limit budgets that need to be cluster-wide use `rate.acquire` against JetStream KV with atomic increment — slower but accurate across instances. Operators choose per-rule which one they need.

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

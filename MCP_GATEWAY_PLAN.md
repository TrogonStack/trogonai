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

The full design space (generic policy engine, signed WASM bundles, multi-protocol reuse, KV-distributed control plane) is the destination, not the first build. The honest synthesis:

1. **Do not build "Protect by itself."** A generic NATS-protocol policy proxy is months of platform work and competes directly with Synadia Protect. Point customers at Protect for subject-level / connect-time policy on raw NATS.
2. **Do not build the full three-tier policy tower up front.** Declarative config → CEL DSL → NATS-callout plugins → WASM Component Model → signed bundles → KV-distributed control plane is the destination, not the first step.
3. **Do build an MCP-aware gateway as a NATS queue-group service.** Narrow, opinionated, MCP-specific. CEL with SpiceDB integration on `tools/call` and `resources/read`, schema-driven redaction (the differentiator vs agentgateway), automatic `tools/list` filtering, audit to JetStream.
4. **Defer the generic-protocol play until a second protocol pulls on it.** Today there is one protocol asking for this; one protocol does not justify a platform.
5. **WASM Component Model is the long-term wedge vs agentgateway, but Phase 3 — not Phase 1.** Build with CEL only first.

### The Strategic Question That Still Flips This

The recommendation above assumes TrogonStack's pitch is **"an event-modeling / decider-driven agentic platform"** where MCP and ACP are surfaces, and the gateway is a feature of that platform. If the pitch is instead **"the security and governance layer for NATS-based agentic systems,"** the recommendation inverts: build the generic Protect-shaped product because *that is the company*.

This is a product-positioning decision, not a technical one. **ADR 0017 (Proposed)** documents the question and the trade-offs; awaiting leadership verdict. Until resolved, planning continues assuming "feature inside TrogonStack."

### What "Next" Looks Like Concretely

Pre-code paper work is **done** (modulo the leadership decision above):

- Product positioning question — **ADR 0017 (Proposed)**.
- Four irreversible technical decisions — DSL (**ADR 0008**), reply correlation (**ADR 0009**), tenancy (**ADR 0001**), host ABI (`docs/identity/reference-host-abi.md`).
- On-bus vs. hybrid — **ADR 0007**.

Phase 1 vertical slice **shipped** — see Block D. Active code work is Block E (Phase 2 hot-path).

## TODO

Concrete work items. Block A/B/C/H are paper-complete. Block D shipped. Block E/F/G remain (mostly code, with test-scaffold contracts already in tree).

### Block A — Strategic decisions (paper)

- [ ] Resolve product positioning: **feature inside TrogonStack** vs. **standalone security product**. **ADR 0017 (Proposed)** documents the question; awaiting leadership verdict.
- [x] On-bus vs. hybrid → **ADR 0007** + `docs/identity/on-bus-vs-hybrid.md`.
- [x] Tenancy boundary → **ADR 0001** + `docs/identity/tenancy-boundary.md`.

### Block B — Irreversible technical decisions (paper)

- [x] DSL choice → **ADR 0008** + `docs/identity/policy-dsl-choice.md`.
- [x] Reply correlation mechanism → **ADR 0009** + `docs/identity/reply-correlation.md`.
- [x] Host ABI surface → `docs/identity/reference-host-abi.md` + `docs/identity/mcp-policy-wit-sketch.md`; contract: `tests/wasm_host_abi.rs`.

### Block C — Design specs (paper)

- [x] Subject grammar → `docs/identity/reference-subject-grammar.md` + reference-{reply-inboxes,queue-groups,virtual-mcp,nats-headers}.md.
- [x] Session model → **ADR 0018** + `docs/identity/mcp-session-model.md`.
- [x] Schema cache + invalidation → **ADR 0023** + contract `tests/schema_cache_invalidation.rs`.
- [x] Failure-mode matrix → **ADR 0024** + `docs/identity/failure-mode-matrix.md`.
- [x] Rate-limit state placement → **ADR 0012** + `docs/identity/rate-limiting.md`.
- [x] OAuth 2.0 MCP integration → **ADR 0019** + `docs/identity/oauth-mcp-integration.md`.
- [x] Bidirectional enforcement → **ADR 0020** + `docs/identity/bidirectional-enforcement.md`; contract: `tests/subscription_scoping.rs`.
- [x] Bootstrap / day-zero behavior → **ADR 0021** + `docs/identity/bootstrap-day-zero.md`.
- [x] Integration touch-points → **ADR 0022** + `docs/identity/integration-touchpoints.md`.

### Block D — Phase 1 vertical slice — SHIPPED

- [x] Scaffold `rsworkspace/crates/trogon-mcp-gateway` (binary + library split).
- [x] Queue-group consumer on `mcp.gateway.request.>`; optional tenant via message header `trogon-mcp-tenant`.
- [x] JSON-RPC parser; verified workload JWT ingress; SpiceDB subject prefers JWT `sub` over forgeable tenant header.
- [x] CEL gate selecting when the SpiceDB hook runs on `tools/call` / `resources/read`.
- [x] SpiceDB `CheckPermission` per gated request (allow-all when endpoint unset).
- [x] Reply correlation: ingress `reply` inbox preserved; gateway issues `request_with_headers`.
- [x] Audit JSON envelope to JetStream (default stream `MCP_AUDIT`).
- [x] End-to-end NATS harness — `tests/e2e_nats_forward.rs`.
- [x] In-memory trace by JSON-RPC request id (`trace::TraceStore`).

### Block E — Phase 2 (CEL hardening + catalog shaping + redaction) — CODE PENDING

Every item has a test-scaffold contract under `rsworkspace/crates/trogon-mcp-gateway/tests/`. Implementation pending.

- [ ] CEL builtins per host-ABI sketch (`spicedb.check`, `cache.get/set`, `jsonpath.*`, `audit.emit`, `time.now`, `rate.acquire`) — contract: `tests/cel_authz_gate.rs`.
- [ ] `tools/list` filtering via CEL re-evaluation → **ADR 0015**; contract: `tests/tools_list_filter.rs`.
- [ ] `BulkCheckPermission` + ZedToken cache → **ADR 0014** + `docs/identity/bulk-check-permission.md`; contract: `tests/bulk_check_zedtoken_cache.rs`.
- [ ] Schema cache populated by sniffing `tools/list` → **ADR 0023**; contract: `tests/schema_cache_invalidation.rs`.
- [ ] Schema-driven redaction — contract: `tests/redaction_rules.rs`.
- [ ] Hierarchical policy merge → **ADR 0013** + `docs/identity/hierarchical-policy-merge.md`; contract: `tests/hierarchical_policy_merge.rs`.
- [ ] Rate limiting wired with chosen state placement → **ADR 0012**; contract: `tests/rate_limit_caps.rs`.

### Block F — Phase 3 (WASM components + bundles + multi-protocol) — CODE PENDING

- [ ] WIT interface (`trogon:mcp-policy@0.1.0`) finalized; pinned to WASI 0.3 — sketch: `docs/identity/mcp-policy-wit-sketch.md`.
- [ ] Wasmtime integration with component pooling per bundle version.
- [ ] Tracing across the WASM boundary; span context as part of `request-ctx`.
- [ ] Bundle format (manifest + CEL + WASM components); NKey signature verification → **ADR 0010** + `docs/identity/wasm-bundle-format.md`; contract: `tests/bundle_load_hot_reload.rs`.
- [ ] Bundle loader from NATS KV with hot-swap and rollback.
- [ ] First-party `mcp-pack` bundle: resource-tuple derivation, catalog shaping, schema-learner WASM component, default audit envelope.
- [ ] NATS-callout plugin tier (Tier 2.5) on `mcp.plugin.{plugin_name}` → **ADR 0011** + `docs/identity/nats-callout-plugin.md`.
- [ ] Engine extraction: `trogon-policy-core` + `trogon-policy-cel` as separate crates.

### Block G — Operational tooling — CODE PENDING

- [ ] Latency baseline (P50/P99 vs direct `mcp-nats`).
- [ ] CLI (`trogon-gateway-ctl`): inspect config, trace requests, validate bundles, dry-run policy — contract: `tests/admin_api.rs`.
- [ ] K8s controller projecting Gateway API CRDs into NATS KV → `docs/identity/k8s-controller.md`.
- [ ] xDS interop layer → `docs/identity/xds-integration.md`.
- [ ] Multi-region story → **ADR 0016** + `docs/identity/multi-region.md`; contract: `tests/multi_region_failover.rs`.
- [ ] OTel trace export + JetStream consumer for audit→SIEM → `docs/identity/otel-wiring.md`; contract: `tests/otel_span_shape.rs`.

### Block H — Docs and process — SHIPPED

- [x] One-page operator overview → `docs/identity/mcp-gateway-operator-overview.md`.
- [x] How-to: third-party MCP behind the gateway → `docs/identity/howto-integrate-third-party-mcp.md`.
- [x] How-to: write a bundle pack → `docs/identity/howto-write-bundle.md`.
- [x] Reference: subject grammar, CEL variables, host ABI, audit envelope → `docs/identity/reference-*.md`.
- [x] RFC per Block-A and Block-B decision → ADRs 0001-0024 under `docs/adr/`.

## Reference Anchors

The original deep-dive sections (agentgateway mapping, NATS subject topology, policy engine tiers, SpiceDB integration, redaction, audit, bundles, wire-format pins) have been ratcheted into dedicated reference documents and ADRs:

| Topic | Authoritative source |
|-------|---------------------|
| Deployment & positioning | `docs/identity/mcp-gateway-operator-overview.md`; ADR 0007, ADR 0017 |
| Subject grammar | `docs/identity/reference-subject-grammar.md` |
| NATS headers (Pin 1) | `docs/identity/reference-nats-headers.md` |
| Reply inboxes (Pin 2) | `docs/identity/reference-reply-inboxes.md` |
| Queue groups (Pin 3) | `docs/identity/reference-queue-groups.md` |
| Virtual-MCP separator (Pin 4) | `docs/identity/reference-virtual-mcp.md` |
| `initialize` handshake (Pin 5) | `docs/identity/reference-initialize.md` |
| JSON-RPC error codes (Pin 6) | `docs/identity/reference-error-codes.md` |
| Audit envelope (Pin 7) | `docs/identity/reference-audit-envelope.md` |
| CEL variable namespace (Pin 8) | `docs/identity/reference-cel-variables.md` |
| Rate-limit defaults (Pin 9) | `docs/identity/reference-rate-defaults.md` |
| Host ABI | `docs/identity/reference-host-abi.md`; ADR 0024 |
| Policy DSL choice | ADR 0008 |
| Reply correlation | ADR 0009 |
| Bundle format | ADR 0010; `docs/identity/wasm-bundle-format.md`; `docs/identity/howto-write-bundle.md` |
| Auth callout | ADR 0011; `docs/identity/nats-callout-plugin.md` |
| Rate-limit state | ADR 0012 |
| Hierarchical policy merge | ADR 0013 |
| BulkCheckPermission + ZedToken | ADR 0014; `docs/identity/bulk-check-permission.md` |
| `tools/list` filtering | ADR 0015 |
| Multi-region | ADR 0016; `docs/identity/multi-region.md` |
| Session model | ADR 0018 |
| OAuth 2.0 MCP integration | ADR 0019 |
| Bidirectional enforcement | ADR 0020 |
| Bootstrap / day-zero | ADR 0021 |
| Integration touch-points | ADR 0022 |
| Schema cache + invalidation | ADR 0023 |
| Failure-mode matrix | ADR 0024; `docs/identity/failure-mode-matrix.md` |

## Inspiration (anchors)

- **[Synadia Protect](https://www.synadia.com/protect)** — transparent NATS-protocol proxy with signed bundles. Adopt: bundle packaging, decision verbs, audit-to-NATS, transparent positioning. Discard: subject-level scope (we need JSON-RPC-aware).
- **[agentgateway](https://github.com/agentgateway/agentgateway)** — closest prior art at the product level. Adopt: CEL as DSL, four-phase pipeline, CEL-driven catalog filtering, target-prefix virtual MCP, hierarchical merge. Diverge: NATS transport, NATS KV + JetStream control plane, WASM Component Model on top of an ext-proc-style callout, schema-driven redaction tied to MCP `inputSchema`/`outputSchema`.

## Phased Delivery

- **Phase 0** — auth callout + subject ACL → ADR 0011.
- **Phase 1** — gateway service with Tier 1 policies, SpiceDB on `tools/call`/`resources/read`, audit to JetStream. **SHIPPED.**
- **Phase 2** — CEL expressions, BulkCheck catalog shaping, ZedToken cache, schema cache, redaction, hierarchical merge, rate limiting. **Active**; Block E items pending; every item has a test-scaffold contract.
- **Phase 3** — WASM components, schema-driven redaction in WASM, bundle distribution. Paper-complete; code pending (Block F).
- **Phase 4** — bidirectional enforcement (ADR 0020), rate limiting (ADR 0012), multi-source bundle composition.

## Existing Code to Lean On

- `rsworkspace/crates/mcp-nats` — JSON-RPC over NATS transport, subject parsing, peer id model. The gateway is a consumer of this crate, not a replacement.
- `rsworkspace/crates/mcp-nats-server` — Streamable HTTP frontdoor; already has `allowed_host` guard. Natural place for coarse identity binding before NATS publish.
- `rsworkspace/crates/mcp-nats-stdio` — stdio bridge; routes through the gateway namespace.
- `trogon_nats` — auth config, connection management.
- `rsworkspace/crates/trogon-mcp-gateway` — the gateway itself; Phase 1 complete (Block D).
- `rsworkspace/crates/trogon-mcp-gateway/tests/` — 38 integration test files; Block E/F/G test-scaffold contracts wait for code to delete the `#[ignore]` tags.

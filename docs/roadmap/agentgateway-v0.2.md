# agentgateway v0.2 roadmap

This is the explicit, scoped backlog for the next agentgateway release.
Items here were intentionally deferred during v0.1 — they require either
architectural decisions, multi-week implementation, or block on
external work. Each item carries deferral rationale.

The v0.1 release tracker (`PENDING_TODO.md`) only lists items shipping
in the current cut.

---

## Gateway architecture

### Multi-region routing

Wire `multi_region/mod.rs` into `gateway::handle_ingress_inner`.

**Deferred because:** `RegionRouter<A: RegionAuditSink>` is generic over
the sink type; wiring requires converting to `Arc<dyn RegionAuditSink>`
or threading a generic parameter through `GatewaySettings`. The
multi-region runtime topology (per-region NATS clients, cross-region
audit fanout, failover policy) is unscoped. **Owner:** architecture
review before code.

### Distributed KV-sync rate limiter (ADR 0012 Tier 2)

**Deferred because:** ADR 0012 frames this as Tier 2 (multi-replica
state sharing via NATS KV). v0.1 ships the in-process limiter only;
the Helm chart documents the single-replica-per-tenant constraint in
the meantime (`charts/agentgateway/values.yaml`, `replicaCount: 1`).

---

## A2A identity propagation

### Per-request User JWT propagation + Subject ACL templates

`jwt_caller_identity.rs`.

**Deferred because:** `production_default()` already enforces
`trust_caller_headers: false`. The remaining work (per-request JWT
extraction from inbound A2A metadata, Subject ACL template
materialization) needs an explicit spec; v0.1 ships with the secure
default.

### Real NATS `$SYS` auth callout

`a2a-bridge/src/auth.rs` `StubAuthCalloutClient` → real implementation.

**Deferred because:** NATS decentralized auth-callout integration is
multi-week work (account JWT signing, callout service registration).
v0.1 ships with the stub returning a clear error; operators are
expected to deploy a bespoke auth-callout service alongside.

### `a2a-bridge` HTTPS ingress (disposition)

**Decision (2026-06-03):** `a2a-bridge` ships as a `0.0.0-skeleton`
crate in v0.1 — binary builds but transports default to the stub
auth-callout client. Production HTTPS ingress is **deferred to v0.2**
together with the `$SYS` auth callout work above; the two land
together because the bridge cannot do anything useful without real
auth-callout responses.

**Why now:** ADR 0017 Option A focuses v0.1 on the NATS substrate;
HTTPS bridging is purely an integration convenience for HTTP-bound
MCP/A2A clients. The Helm chart leaves the bridge sub-chart out of
the default install (PENDING_TODO §7.1) until v0.2.

---

## AAuth post-v0

v0 ships identity-based and PS-managed 3-party flows (see
[`docs/identity/aauth-handshake.md`](../identity/aauth-handshake.md) and
[`docs/identity/aauth-design.md`](../identity/aauth-design.md) — those are
the permanent homes for the shipped surface). The items below are
intentionally not in v0.

### 2-party flow (resource-managed consent)

Agent presents an `aa-auth+jwt` minted by its own Agent Provider, and the
resource enforces consent locally without a Person-Server exchange. v0
intentionally only ships PS-managed 3-party because it keeps the consent
substrate centralized while we iterate on policy.

**Deferred because:** requires a resource-side consent store and a second
challenge envelope path; not needed until we have non-Trogon Agent
Providers calling Trogon resources.

### 4-party federation

Separate Agent Provider and Person Server with cross-issuer trust
(`iss` chains and JWKS discovery between issuers).

**Deferred because:** v0 collapses AP and PS into `PersonCore`; federation
needs an issuer trust list, a discovery doc format, and rotation rules.
Treat as a follow-up once we have a real second issuer.

### Mission tokens

The `mission` claim is already serialized by `trogon-identity-types`, but
`PersonCore` does not broker mission approvers — exchanges with a
`mission` requirement currently fall through to the standard consent
policy.

**Deferred because:** mission-scoped delegation needs a per-mission
approver registry and a UX for staging mission grants; the on-wire
format is ready but the runtime is not.

### Interaction-relay UI

`PersonCore::exchange` already returns `ConsentDecision::Interaction` with
`{requirement: "interaction", url, code}` and the SDK surfaces it as
`SdkError::Interaction`. The browser-side redirect page that closes the
loop with the user is not shipped.

**Deferred because:** v0 audience is service-to-service; the consent
screen is a discrete frontend deliverable that does not block any
production flow today.

### NATS KV-backed `ReplayStore`

`InMemoryReplayStore` ships for single-process deployments. The trait +
KV layout are documented in
[`aauth-design.md` §D6 / §D9](../identity/aauth-design.md#d6-storage).

**Deferred because:** v0 deploys a single Person Server replica; KV
backing is required only for HA and is a straightforward port once we
own the KV layout in production.

### Python AAuth SDK

`pyworkspace/packages/a2a-sdk` currently speaks A2A only. The Rust SDK
(`trogon-aauth-sdk`) covers the bootstrap → sign → 401 → exchange → retry
loop end-to-end; a Python sibling is on the v0.2 deliverables list so
existing demo agents can adopt AAuth via a client switch.

**Deferred because:** the Rust SDK is the reference and the contract is
not frozen yet — we want to land the 2-party variant first so the Python
surface lands once rather than twice.

### A2A gateway AAuth coverage

`trogon-mcp-gateway::aauth` is wired; `a2a-gateway` enforces JWT-based
identity today but does not yet run `trogon-aauth-verify` on its ingress
path.

**Deferred because:** the AAuth ingress shape needs an A2A-side decision
on where token / PoP headers ride (callout vs. inline) — tracked
alongside the `$SYS` auth callout item above and lands together.

### Hardening pass

Key rotation cadence for the Person Server signing key, JWKS cache TTLs
+ negative caching, per-(`iss`, `sub`) rate limits on `/aauth/token`, and
a threat-model review (`docs/security/threat-model.md`).

**Deferred because:** v0 covers the protocol surface and the tests prove
the loop; the operational hardening items are a separate cut and depend
on production deployment data.

### Out of scope (won't ship in v0.x)

- Cross–Person-Server federation / trust chains.
- Hardware-bound agent keys (TPM / TEE attestation).
- Human-in-the-loop step-up auth beyond the basic consent screen.

---

## Optional crates

### `a2a-pack` version bump

**Deferred because:** Tier-2 CEL → WASM compile path is a non-goal
for v0.1; the crate stays at skeleton version `0.0.0-skeleton` and is
not published.

### `trogon-source-telegram`

**Decision:** Shipped as private library crate in v0.1 (no binary
target). Webhook adapter restoration moves to v0.2.

---

## MCP federation (Virtual MCP)

### Virtual MCP routing across heterogeneous backends

**Decision (2026-06-03):** Virtual MCP routing across heterogeneous
backends is **deferred to v0.2**.

Today `trogon-mcp-gateway::chain_resolver` exists and the
`chain_resolver_e2e.rs` integration test exercises homogeneous chains.
Cross-backend federation (one client sees N servers as one) is the
upstream `mcp/mergestream.rs` analog and is not exercised end-to-end
in v0.1. The `virtual_mcp_routing.rs` test stays `#[ignore]`'d as the
v0.2 contract pin.

### `tools/list` virtualization (rename / alias / hide)

**Decision (2026-06-03):** Per-client tool **renaming and aliasing**
is **deferred to v0.2**. The hide / filter dimension (denylist by
name) ships in v0.1 via `policy/list_filter`.

The `tools_list_filter.rs` integration test stays `#[ignore]`'d
because it currently asserts rename behavior. v0.1 release notes call
out the supported subset (filter-only).

---

## Test scaffolds

Approximately 67 `#[ignore]`d tests across the gateway suite. **These
are SPECS for undelivered behavior, not flaky tests.** Each is owned
by the feature it specs and will be un-ignored when the feature lands.

| Test file | Cases | Owned by |
|-----------|-------|----------|
| `tests/otel_span_shape.rs` | ~15 | OTel Pin 7 |
| `tests/traceparent_propagation.rs` | ~8 | OTel Pin 7 |
| `tests/admin_api.rs` | ~8 | Admin HTTP API |
| `tests/wasm_host_abi.rs` | ~10 | WASM host ABI (Phase 3, v0.3+) |
| `tests/bundle_load_hot_reload.rs` | ~5 | Hot reload |
| `tests/config_hot_reload.rs` | ~8 | Hot reload |
| `tests/audit_envelope_shape.rs` | ~6 | Wire-Format Pin 7 Phase 2 |
| `tests/e2e_nats_forward.rs` | 1 | Integration-tier (live NATS) |

`tests/health_probes.rs` (~6 cases) was promoted into v0.1 along with
the `/healthz`+`/readyz` listener.

---

## Open GitHub items

- **PR #190** — `chore(deps): Bump rand 0.8.5 → 0.8.6` (dependabot).
  Auto-merge once CI green. **Action:** enable automerge on the PR;
  no v0.2 code required.
- **PR #189** — `chore(deps): Bump rustls-webpki 0.103.10 → 0.103.13`
  (dependabot). Auto-merge once CI green.
- **Issue #122** — restore `server_info().max_payload` once
  async-nats race resolved. **Blocked on upstream.**
- **Issue #101** — TTL on `trogon-claims` object store bucket.
  Provisioning manifest now sets a 10-minute KV TTL
  (`devops/nats/kv/trogon-claims.json`); object-store variant tracked
  alongside if/when needed.
- **Issue #136** — local dev orchestration decision (Docker Compose
  vs. native process orchestration).

---

## Release sign-off

Tag a release candidate, smoke-test against staging NATS, run the
integration test suite (`e2e_nats_forward.rs` plus any v0.2-promoted
scaffolds). Requires staging NATS cluster.

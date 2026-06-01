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

# Pending — A2A & MCP agentgateway Release & Deploy

Outstanding work for the `yordis/agentgateway` release. Items are split into:

- **v0.1 release** — must land before the cut; tracked here as checkboxes.
- **v0.2 backlog (deferred)** — explicit scoping decisions; documented but
  out of scope for v0.1.

---

## v0.1 — Code: gateway wiring

- [x] `approvals/mod.rs` — wire `ApprovalGate::request_approval`; emit
      `-32107 approval_required`. _Done: 633b885d8 + merge 586b9d6fb._
- [x] `stepup/mod.rs` — invoke `StepUpPolicy::evaluate` after CEL.
      _Done: 762d26771 + merge 957ba1b1e._
- [x] `anomaly/mod.rs` — emit per-request feature vectors via
      `AnomalyEmitter::emit`. _Done: d0416b6ea + merge 6e7cc2aba._
- [x] `context_throttle/mod.rs` — call `ContextThrottle::acquire` keyed on
      `(tenant, agent_id, purpose)`. _Done: 4719f3df0 + merge a4e819903._

## v0.1 — Code: A2A gateway

- [x] `runtime.rs` — `caller_id` populated via
      `gateway_audit_caller_attribution`. _Verified._
- [x] `runtime.rs` — `AuditEmitter` published at 8 decision sites.
      _Verified._
- [x] `runtime.rs` — `unary_deadline_for_method` enforced. _Verified._

## v0.1 — Wire-Format consistency

Every audit / metrics subject must honor the operator-configured prefix
(`MCP_GATEWAY_MCP_PREFIX`, `MCP_STS_PREFIX`, `MCP_REGISTRY_PREFIX`).

- [x] `approvals/types.rs` + envelope builders + `risk_gate_response`.
      _Done: 3704e1cb5._
- [x] `anomaly/emitter.rs` — `subject_for_tenant(prefix, tenant_id)` and
      `AnomalyEmitter::new(prefix, client)`. _Done: 3704e1cb5._
- [x] `egress/audience.rs` + `audience_shadow/audit_sink.rs`.
      _Done: 5ad5ed973._
- [x] `trogon-sts/src/audit.rs`. _Done: f3e8e9cb8._
- [x] `trogon-agent-registry/src/{audit,consumer,store}.rs`.
      _Done: b31ffae6f._

## v0.1 — Release housekeeping

- [x] `rsworkspace/deny.toml` — cargo-deny advisories + licenses + bans.
      _Done in this branch._
- [x] `docs/runbook/agentgateway.md` — operator runbook (startup,
      env vars, audit subjects, approval workflow, rollback).
      _Done in this branch._
- [x] CI workflow — `.github/workflows/ci-rust.yml` extended with a
      `cargo deny check` job. _Done: 54c0c5e39._
- [x] release-please configuration — `.github/release-please-config.json`
      and manifest now cover the 14 shipping binary crates
      (acp-nats-stdio, a2a-bridge, a2a-gateway, a2a-auth-callout,
      mcp-nats-server, mcp-pack, trogon-agent-registry,
      trogon-agent-registry-controller, trogon-gateway-ctl,
      trogon-gateway-k8s, trogon-jwks-publisher, trogon-mcp-gateway,
      trogon-sts, agctl). _Done: 54c0c5e39._
- [x] Base Dockerfile — `devops/docker/Dockerfile` with `BIN` build-arg
      covers every shipping binary; image pipeline (build/push/sign) is
      v0.2. _Done: 54c0c5e39._
- [ ] Decide branch strategy for the 604-commit `yordis/agentgateway` →
      `main` merge (single PR vs. stacked).

---

## v0.2 backlog — Deferred (with rationale)

### v0.2 — Gateway architecture

- [ ] `multi_region/mod.rs` wiring into `gateway::handle_ingress_inner`.
      **Deferred:** `RegionRouter<A: RegionAuditSink>` is generic over the
      sink type; wiring requires either converting to `Arc<dyn RegionAuditSink>`
      or threading a generic parameter through `GatewaySettings`. The
      multi-region runtime topology (per-region NATS clients, cross-region
      audit fanout) is also unscoped. Tracked as a v0.2 architectural item.
- [ ] Distributed KV-sync rate limiter (ADR 0012 Tier 2).
      **Deferred:** ADR 0012 frames this as Tier 2 (multi-replica state
      sharing via NATS KV). v0.1 ships the in-process limiter only; the
      Helm chart will document the single-replica-per-tenant constraint
      in the meantime.

### v0.2 — A2A identity propagation

- [ ] Per-request User JWT propagation + Subject ACL templates
      (`jwt_caller_identity.rs`). **Deferred:** `production_default()`
      already enforces `trust_caller_headers: false`. The remaining work
      (per-request JWT extraction from inbound A2A metadata, Subject ACL
      template materialization) needs an explicit spec; v0.1 ships with
      the secure default.
- [ ] `a2a-bridge/src/auth.rs` — `StubAuthCalloutClient` → real NATS `$SYS`
      auth callout. **Deferred:** NATS decentralized auth-callout
      integration is multi-week work (account JWT signing, callout
      service registration). v0.1 ships with the stub returning a clear
      error; operators are expected to deploy a bespoke auth-callout
      service alongside.

### v0.2 — Optional crates

- [ ] `a2a-pack/Cargo.toml` — version `0.0.0-skeleton`.
      **Deferred:** Tier-2 CEL → WASM compile path is a non-goal for
      v0.1; the crate stays at skeleton version and is not published.
- [ ] `trogon-source-telegram` — `lib.rs` re-export only.
      **Decision:** Ship as private library crate in v0.1 (no binary
      target). Webhook adapter restoration deferred to v0.2.

### v0.2 — Test scaffolds

Approximately 67 `#[ignore]`d tests across the gateway suite. **These are
SPECS for undelivered behavior, not flaky tests.** Each is owned by the
feature it specs and will be un-ignored when the feature lands:

- [ ] `tests/otel_span_shape.rs` (~15 cases) — owned by OTel Pin 7 work.
- [ ] `tests/traceparent_propagation.rs` (~8 cases) — owned by OTel Pin 7.
- [ ] `tests/admin_api.rs` (~8 cases) — owned by **admin HTTP API (v0.2)**.
- [ ] `tests/health_probes.rs` (~6 cases) — owned by **`/healthz`+`/readyz`
      HTTP listener (v0.2)**. Helm chart probes block on this.
- [ ] `tests/wasm_host_abi.rs` (~10 cases) — owned by **WASM host ABI
      (Phase 3, v0.3+)**.
- [ ] `tests/bundle_load_hot_reload.rs` (~5 cases) — owned by hot reload.
- [ ] `tests/config_hot_reload.rs` (~8 cases) — owned by hot reload.
- [ ] `tests/audit_envelope_shape.rs` (~6 cases) — owned by Wire-Format
      Pin 7 Phase 2 envelope shape work.
- [ ] `tests/e2e_nats_forward.rs` (1 case) — integration-tier; runs
      against live NATS.

### v0.2 — Deploy artifacts

The repository ships binaries via `release-artifacts.yml` (Linux + macOS
targets) but has no container or k8s artifacts yet. v0.1 supports the
binary-only deployment path; v0.2 adds containerized + k8s.

- [x] **Dockerfile** — `devops/docker/Dockerfile` covers every shipping
      binary via `BIN` build-arg (distroless `cc-debian12:nonroot`).
      _Done: 54c0c5e39._
- [ ] **Container image build + publish pipeline** (registry, tagging,
      cosign signing).
- [ ] **Helm chart** (`charts/agentgateway/`). Blocked by health probes
      (`/healthz`+`/readyz`); deploy without probes is unsafe.
- [ ] **NATS provisioning** manifests (streams, KV buckets, account /
      auth-callout config). Tracked as issue #101 (`trogon-claims`
      object store TTL).
- [ ] **Local dev orchestration** decision — issue #136 (replace Docker
      Compose with native process orchestration vs. keep Compose).

### v0.2 — Open GitHub items

- [ ] PR #190 — `chore(deps): Bump rand 0.8.5 → 0.8.6` (dependabot).
      **Triage:** Auto-merge once CI green.
- [ ] PR #189 — `chore(deps): Bump rustls-webpki 0.103.10 → 0.103.13`
      (dependabot). **Triage:** Auto-merge once CI green.
- [ ] Issue #122 — restore `server_info().max_payload` once
      async-nats race resolved. **Blocked on upstream.**
- [ ] Issue #101 — TTL on `trogon-claims` object store bucket.
      Bundled into the v0.2 NATS provisioning manifests.

### v0.2 — Release sign-off

- [x] Wire `cargo deny check` into `ci-rust.yml`. _Done: 54c0c5e39._
- [ ] Tag a release candidate, smoke-test against staging NATS, run the
      integration test suite (`e2e_nats_forward.rs` plus v0.2-promoted
      scaffolds). _External: requires staging NATS cluster._

---

**Source of truth:** `yordis/agentgateway` HEAD on 2026-06-01;
ADRs 0001–0032 are all `Accepted`; `cargo check --workspace --all-targets`
passes.

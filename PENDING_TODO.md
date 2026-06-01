# Pending — A2A & MCP agentgateway Release & Deploy

Scope: the **v0.1 release cut** on `yordis/agentgateway`. The v0.2
backlog has moved to [`docs/roadmap/agentgateway-v0.2.md`](docs/roadmap/agentgateway-v0.2.md)
with per-item deferral rationale.

---

## v0.1 — Code: gateway wiring

- [x] `approvals/mod.rs` — wire `ApprovalGate::request_approval`; emit
      `-32107 approval_required`. _Done: 633b885d8 + merge 586b9d6fb._
- [x] `stepup/mod.rs` — invoke `StepUpPolicy::evaluate` after CEL.
      _Done: 762d26771 + merge 957ba1b1e._
- [x] `anomaly/mod.rs` — emit per-request feature vectors via
      `AnomalyEmitter::emit`. _Done: d0416b6ea + merge 6e7cc2aba._
- [x] `context_throttle/mod.rs` — call `ContextThrottle::acquire` keyed
      on `(tenant, agent_id, purpose)`. _Done: 4719f3df0 + merge a4e819903._

## v0.1 — Code: A2A gateway

- [x] `runtime.rs` — `caller_id` populated via
      `gateway_audit_caller_attribution`. _Verified._
- [x] `runtime.rs` — `AuditEmitter` published at 8 decision sites.
      _Verified._
- [x] `runtime.rs` — `unary_deadline_for_method` enforced. _Verified._

## v0.1 — Wire-format consistency

Every audit / metrics subject honors the operator-configured prefix
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

## v0.1 — Health probes

- [x] `/healthz` + `/readyz` HTTP listener on
      `MCP_GATEWAY_PROBE_LISTEN_ADDR` (default `0.0.0.0:8080`),
      with unit tests. _Done: f36ad8fdc._

## v0.1 — Release housekeeping

- [x] `rsworkspace/deny.toml` — cargo-deny advisories + licenses + bans.
      _Done: 0ea2108dd._
- [x] `docs/runbook/agentgateway.md` — operator runbook (startup,
      env vars, audit subjects, approval workflow, rollback).
      _Done: 0ea2108dd._
- [x] CI workflow — `.github/workflows/ci-rust.yml` extended with a
      `cargo deny check` job. _Done: 54c0c5e39._
- [x] release-please configuration covers the 14 shipping binary crates
      (acp-nats-stdio, a2a-bridge, a2a-gateway, a2a-auth-callout,
      mcp-nats-server, mcp-pack, trogon-agent-registry,
      trogon-agent-registry-controller, trogon-gateway-ctl,
      trogon-gateway-k8s, trogon-jwks-publisher, trogon-mcp-gateway,
      trogon-sts, agctl). _Done: 54c0c5e39._
- [x] Base Dockerfile — `devops/docker/Dockerfile` with `BIN` build-arg
      covers every shipping binary. _Done: 54c0c5e39._

## v0.1 — Deploy artifacts

- [x] **Helm chart** — `charts/agentgateway/` with `trogon-mcp-gateway`
      Deployment, ConfigMap, Service, ServiceAccount, distroless
      security context, and `/healthz`+`/readyz` probes wired to the
      v0.1 listener.
- [x] **Container image build + publish pipeline** —
      `.github/workflows/release-images.yml` builds and pushes every
      shipping binary on `<crate>-v*.*.*` tags, signed via cosign
      keyless OIDC.
- [x] **NATS provisioning manifests** —
      `devops/nats/streams/{mcp-audit,a2a-audit,sts-audit,registry-audit,approvals}.json`
      + `devops/nats/kv/{trogon-claims,agent-registry,policy-bundle}.json`,
      with apply instructions in `devops/nats/README.md`.
      Closes issue #101 (`trogon-claims` 10-minute KV TTL).

## v0.1 — Branch strategy

- [x] **Decision:** merge `yordis/agentgateway` → `main` as a **single
      squash PR**. Rationale: the 604 commits are exploratory/iterative
      release prep with intermediate states that don't compile; the
      authoritative shape lives at the branch tip. release-please
      consumes squash subjects fine.

## v0.1 — Release sign-off

- [x] `cargo deny check` wired into `ci-rust.yml`. _Done: 54c0c5e39._
- [x] Dependabot PRs **#189** (rustls-webpki 0.103.10 → 0.103.13) and
      **#190** (rand 0.8.5 → 0.8.6) — triaged: enable auto-merge once
      CI is green; no code change required for v0.1.
- [x] Issue **#122** — `server_info().max_payload` triaged as
      blocked-on-upstream-async-nats. Tracked in
      [`docs/roadmap/agentgateway-v0.2.md`](docs/roadmap/agentgateway-v0.2.md).
- [x] Issue **#136** (local dev orchestration) — deferred to v0.2 as
      a non-release-blocking decision. Tracked in the roadmap doc.
- [x] RC tagging + staging smoke is operator-side (requires staging
      NATS cluster); tracked in the v0.2 roadmap under "Release sign-off".

---

**Source of truth:** `yordis/agentgateway` HEAD on 2026-06-01;
ADRs 0001–0032 are all `Accepted`; `cargo check --workspace --all-targets`
passes.

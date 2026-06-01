# Operator Runbook — MCP & A2A Agentgateway

Reference for operating the `yordis/agentgateway` release in production. Covers
startup, health, telemetry, audit subjects, rate-limit / approval flows, and
rollback procedures.

## Components

| Binary                          | Crate                              | Responsibility                                                |
|--------------------------------|-------------------------------------|---------------------------------------------------------------|
| `trogon-mcp-gateway`           | `trogon-mcp-gateway`               | MCP request gateway over NATS request/reply.                  |
| `trogon-gateway-ctl`           | `trogon-gateway-ctl`               | Operator CLI (audit tail, config inspect).                    |
| `trogon-sts`                   | `trogon-sts`                       | RFC 8693 token exchange daemon (mesh JWT minting).            |
| `trogon-agent-registry`        | `trogon-agent-registry`            | KV-backed agent metadata registry + lookup consumer.          |
| `trogon-agent-registry-controller` | `trogon-agent-registry-controller` | Kubernetes controller for agent CRD → registry KV reconciliation. |
| `trogon-jwks-publisher`        | `trogon-jwks-publisher`            | Publishes mesh JWKS to NATS KV for verifier consumption.      |
| `a2a-gateway`                  | `a2a-gateway`                      | Agent-to-agent gateway over NATS.                             |
| `a2a-bridge`                   | `a2a-bridge`                       | HTTP <-> NATS bridge for A2A clients.                         |
| `trogon-gateway-k8s`           | `trogon-gateway-k8s`               | Kubernetes operator for declarative gateway provisioning.     |

## Required environment

Every binary expects:

- `NATS_URL` — comma-separated NATS server URLs.
- `OTEL_EXPORTER_OTLP_ENDPOINT` (optional) — OTLP gRPC endpoint for traces.
- `RUST_LOG` — tracing filter (default `info`).

Per-binary critical env vars (see each crate's `config.rs` for the full set):

### `trogon-mcp-gateway`

- `MCP_GATEWAY_MCP_PREFIX` — wire-format prefix (default `mcp`). MUST match
  every other component that publishes / consumes audit and registry subjects.
- `MCP_GATEWAY_JWT_INGRESS_*` — ingress JWT validator config (issuer, audience,
  JWKS URL / KV path, mode: `off` / `shadow` / `enforce`).
- `MCP_GATEWAY_SPICEDB_*` — SpiceDB endpoint, token, object types, permission names.
- `MCP_GATEWAY_RATE_LIMIT_*` — in-process limiter caps; distributed KV-sync limiter
  (ADR 0012 Tier 2) is **not wired** in this release.
- `MCP_GATEWAY_REGISTRY_SUBJECT` — overrides default `mcp.registry.agent.lookup`.
- `MCP_GATEWAY_STS_EXCHANGE_SUBJECT` — overrides default `mcp.sts.exchange`.

### `trogon-sts`

- `MCP_STS_PREFIX` — must match the gateway's `MCP_GATEWAY_MCP_PREFIX`.
- `MCP_STS_SIGNING_KEY_PATH`, `MCP_STS_SIGNING_KID`.
- `MCP_STS_MESH_ISSUER` (default `urn:trogon:sts:mesh`).
- `MCP_STS_BOOTSTRAP_JWKS_URL`, `MCP_STS_TRUST_BUNDLE_PATH`, `MCP_STS_TRUST_DOMAIN`.
- `MCP_STS_REGISTRY_SUBJECT`, `MCP_STS_QUEUE_GROUP`.

### `trogon-agent-registry`

- `MCP_REGISTRY_PREFIX` — must match the gateway's `MCP_GATEWAY_MCP_PREFIX`.
- `TROGON_REGISTRY_AUTOCREATE` — set in dev to auto-create the KV bucket.

## Health probes

> **Status: planned for v0.2.** The HTTP probe listener is not yet implemented;
> `tests/health_probes.rs` is `#[ignore]`d as a SPEC. Until then, use NATS
> `$SYS.REQ.SERVER.PING` to verify gateway connectivity from k8s startupProbes.

Once implemented (Phase 1+):

- `GET :8080/healthz` — liveness; 200 once the NATS connection is bound.
- `GET :8080/readyz` — readiness; 200 when bundle loaded, NATS up, SpiceDB
  reachable, and every server in the routing table has
  `min_ready_replicas` available.

## Audit subjects

Every component publishes to `<prefix>.audit.*`. The default `<prefix>` is
`mcp`; override per the env vars above.

| Subject                                                       | Producer            | Notes                                  |
|---------------------------------------------------------------|---------------------|----------------------------------------|
| `<prefix>.audit.allow.<direction>.<method_root>`              | `trogon-mcp-gateway`| Successful request.                    |
| `<prefix>.audit.deny.<direction>.<method_root>`               | `trogon-mcp-gateway`| Policy / SpiceDB deny.                 |
| `<prefix>.audit.rate_limited.<direction>.<method_root>`       | `trogon-mcp-gateway`| In-process rate limit fired.           |
| `<prefix>.audit.approval.<status>.<direction>.<method_root>`  | `trogon-mcp-gateway`| `-32107 approval_required` lifecycle.  |
| `<prefix>.audit.gateway.aud_mismatch`                         | `trogon-mcp-gateway`| Inbound mesh `aud` shadow-mismatch.    |
| `<prefix>.audit.sts.success` / `.deny` / `.error`             | `trogon-sts`        | Token exchange outcome.                |
| `<prefix>.audit.registry.lookup.{found,notfound,revoked}`     | `trogon-agent-registry` | Lookup outcomes.                   |
| `<prefix>.audit.registry.{put,delete}`                        | `trogon-agent-registry` | KV mutations.                      |

Tail with `trogon-gateway-ctl audit tail --since 5m --subject '<prefix>.audit.>'`.

## Approval workflow (`-32107`)

When the gateway requires a step-up approval, it replies with JSON-RPC error
code `-32107` and an `approval_request_id`. The client is expected to:

1. Subscribe to `<prefix>.approvals.<request_id>.grant`.
2. Drive the operator-facing approval surface (Slack, ServiceNow, etc.).
3. Once the operator approves, publish the grant token to the subject.
4. Retry the original request with the grant token attached.

The grant subject grammar is fixed at `<prefix>.approvals.<request_id>.grant`
(see `docs/identity/reference-subject-grammar.md`).

## Rate limit + approval observability

- `<prefix>.audit.rate_limited.>` — in-process per-`jwt.sub` limiter fired.
- `<prefix>.audit.approval.required.>` — gateway issued `-32107`.
- `<prefix>.audit.approval.granted.>` — operator-approved grant consumed.
- `<prefix>.audit.approval.expired.>` — grant TTL elapsed.

## Rollback procedure

1. Identify the last-known-good image tag (release-please publishes
   `<crate>@v<version>`).
2. `helm upgrade --install agentgateway charts/agentgateway -f values.yaml \
    --set image.tag=<previous>` (chart pending; see PENDING_TODO §7).
3. Confirm `<prefix>.audit.allow.>` traffic resumes within the policy bundle
   load timeout (~5s).
4. If the rollback is caused by a bundle regression, also roll back the policy
   KV revision via `trogon-gateway-ctl bundle revert --to <rev>`.

## Diagnostic checklist

If the gateway is unhealthy:

1. `nats sub '<prefix>.audit.deny.>'` — is every request getting denied?
   → SpiceDB outage or bundle regression. Check `MCP_GATEWAY_SPICEDB_*` config.
2. `nats sub '<prefix>.audit.rate_limited.>'` — are clients being throttled?
   → Raise `MCP_GATEWAY_RATE_LIMIT_*` caps or investigate runaway agent.
3. `nats sub '$SYS.SERVER.>'` — is NATS itself flapping?
4. `nats sub '<prefix>.audit.gateway.aud_mismatch'` — are tokens minted for the
   wrong gateway? Confirm STS `aud` mode and trust-bundle freshness.

## Known limitations (v0.1)

- No HTTP probe listener (`/healthz`, `/readyz`); planned for v0.2.
- No distributed KV-sync rate limiter; in-process only. Multi-replica deployments
  share NO limiter state — see ADR 0012 Tier 2.
- No multi-region routing wiring (`multi_region/RegionRouter` exists but is
  not called from `gateway::handle_ingress_inner`).
- `trogon-source-telegram` ships as a private library crate only; the webhook
  adapter is deferred to v0.2.
- `a2a-pack` is `0.0.0-skeleton`; the Tier-2 CEL → WASM compile path is a
  v0.2 non-goal.
- `StubAuthCalloutClient` is a stub; auth-callout via NATS `$SYS` is deferred
  to v0.2.

See `PENDING_TODO.md` at the repo root for the complete v0.2 backlog.

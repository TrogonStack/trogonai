# ADR 0034: v0.1 Upstream-Parity Decisions (clone-derived action items)

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-06-03) |
| **Date** | 2026-06-03 |
| **Deciders** | *(platform security / mcp gateway)* |
| **Blocks** | Unblocks the v0.1 release sign-off by resolving every clone-derived action item in [PENDING_TODO.md §15.8](../../PENDING_TODO.md). |
| **Related** | [ADR 0017](0017-product-positioning.md) (Option A — no LLM gateway, no inference routing); [ADR 0012](0012-distributed-rate-limiter.md) (Tier-1 in-process, Tier-2 KV-sync deferred); [ADR 0024](0024-failure-mode-matrix.md); [ADR 0026](0026-bundle-hot-swap-rollback.md). |

## Context

Reading the upstream `agentgateway` repository source (not the docs-only
`llms.txt` pass) surfaced 10 concrete features that the doc-level scan
missed. Each needed an explicit decision before v0.1 sign-off — either
"shipped under a different name", "deferred to v0.2 with a roadmap
pointer", or "non-goal under ADR 0017 Option A".

The 10 items were enumerated in
[`PENDING_TODO.md §15.8`](../../PENDING_TODO.md). This ADR records the
decision per item.

## Decisions

### 1. RFC 9728 OAuth Resource Metadata (§15.1 `mcp/auth.rs`)

**Superseded-in-part (2026-06-04).** Original status `❌` (Deferred to
v0.2). Current status `⚠️ types-shipped, wiring-pending`.

Commit `4d1f98eab feat(agentgateway): close Solo.io MCP token-exchange
spec gaps` on `yordis/agentgateway` introduced
`trogon-frontend-proxy::protected_resource` with full RFC 9728 +
RFC 8414 metadata types, axum router, and unit tests. The router is
**not** mounted on any binary, so deployed gateways still return 404
on `/.well-known/oauth-protected-resource/mcp/{backend}` and
`/.well-known/oauth-authorization-server/mcp/{backend}`. Remaining
wiring (mount router, thread `GatewayDiscovery` from env) is tracked
in [`PENDING_TODO.md §3.2`](../../PENDING_TODO.md). When that lands,
this row flips to `✅`.

Original deferral text follows for historical reference:

> MCP clients that expect `/.well-known/oauth-protected-resource/{path}`
> and `/.well-known/oauth-authorization-server/{path}` will not discover
> our gateway as an OAuth-protected resource in v0.1. Today the gateway
> validates JWTs from a configured JWKS but does not advertise the
> discovery surface upstream MCP clients use.
>
> v0.2 will add a `/.well-known/*` set served alongside `/healthz` and
> `/readyz` on `MCP_GATEWAY_PROBE_LISTEN_ADDR`, populated from the
> existing JWT configuration (issuer URL + JWKS URI). Tracked in
> `docs/roadmap/agentgateway-v0.2.md`.

### 2. `Mcp-Session-Id` header semantics (§15.2 CORS row)

**Aligned, no code change.** Status `✅`.

Our session correlation IDs already match the upstream-standardized
`Mcp-Session-Id` header semantics: opaque, stable per session, returned
on the first response and echoed back on subsequent requests. The NATS
substrate carries the session ID in the `Mcp-Session-Id` NATS header
when an HTTP bridge is in front. Documented in
[`docs/identity/reference-nats-headers.md`](../identity/reference-nats-headers.md)
and [`docs/identity/mcp-session-model.md`](../identity/mcp-session-model.md).

### 3. `/metrics` Prometheus exposure (§15.4 `statsAddr`)

**Deferred to v0.2.** Status `❌` for v0.1.

For v0.1 the gateway emits OTel metrics via OTLP; operators wanting
Prometheus deploy an OTel collector with a Prometheus exporter
configured in pull mode. A dedicated `/metrics` endpoint on the probe
listener is deferred to v0.2 alongside the catalogue work in
`docs/identity/reference-metrics.md` (Task §9.2). Adding a
`MCP_GATEWAY_STATS_LISTEN_ADDR` env is not required for v0.1.

### 4. App-level drain deadline (§15.4 `connectionTerminationDeadline`)

**No env knob.** Status `✅` aligned via implicit drain.

The gateway main loop in `gateway::run` processes ingress requests
sequentially with `tokio::select!` against the shutdown signal. Drain
is implicit — the current in-flight `handle_ingress` future completes
before the loop exits. Outer bound comes from Kubernetes
`terminationGracePeriodSeconds` (chart default: 30s). No
`MCP_GATEWAY_DRAIN_DEADLINE` env is added for v0.1.

If concurrent request handling lands in v0.2 (spawned per-request
tasks), this decision must be revisited and a real drain deadline
added.

### 5. `extAuthz` HTTP protocol (§15.2)

**Non-goal under Option A.** Status `⛔`.

Upstream's `extAuthz` HTTP callout (with CEL metadata extraction +
redirect + header passthrough) is replaced in our substrate by the
NATS callout plugin pattern documented in
[`docs/identity/nats-callout-plugin.md`](../identity/nats-callout-plugin.md).
The NATS subject `policy.extauthz.request` is the substrate equivalent
of upstream's ExtAuthz cluster; the contract is in
`trogon-mcp-gateway::plugin::dispatcher` (the `nats_callout_plugin.rs`
integration test is `#[ignore]`'d pending live-NATS harness, per
§8.3). No HTTP ExtAuthz adapter is planned — agents and resources
authenticate via JWT/SVID per ADR 0017 Option A.

### 6. Rate-limit `failureMode: failOpen|failClosed` (§15.2 `remoteRateLimit`)

**Default fail-closed, env-configurable.** Status `❌` for v0.1.

The throttle/limiter config surface is the Tier-1 in-process limiter
shipped per ADR 0012. The `failureMode` semantic only becomes
meaningful with the Tier-2 KV-sync limiter (the in-process limiter
cannot "fail" — it either has the budget or it doesn't).

Tier-2 ships in v0.2 (ADR 0012 Tier 2). At that time the config will
add `MCP_GATEWAY_RATE_LIMIT_FAILURE_MODE=fail_closed|fail_open`
defaulting to `fail_closed`. Tracked in
`docs/roadmap/agentgateway-v0.2.md`.

### 7. OpenAPI → MCP tools (§15.1 `mcp/upstream/openapi/`)

**Non-goal under Option A.** Status `⛔`.

Upstream's `mcp/upstream/openapi/` auto-generates MCP tools from an
OpenAPI spec at config load. Under ADR 0017 Option A we keep the
position that "agents bring their own MCP tools" — the gateway is a
PEP for traffic, not a code-generator for adapters. If the OpenAPI →
MCP path is needed later, it lives in a sibling crate
(`mcp-openapi-adapter` style), not in `trogon-mcp-gateway`.

### 8. Backend re-auth / token exchange (`backendAuth`, §15.2)

**Already covered by STS.** Status `✅` partial.

The upstream `backendAuth` row (re-sign or pass-through credentials
to upstream backends) is the STS token-exchange path documented in
[`docs/identity/sts-exchange.md`](../identity/sts-exchange.md) and
implemented in `trogon-sts` + `trogon-sts-client`. The gateway's
egress mint path (`trogon-mcp-gateway::egress::mint`) is the per-call
re-sign equivalent. Multi-hop STS chaining (A2A → STS → upstream A2A
with the same caller identity) is a v0.2 deliverable
(`docs/a2a/explanation/auth-callout-design.md` §multi-hop).

### 9. Grafana dashboards (§15.5)

**Deferred to v0.2.** Status `❌` for v0.1.

No Grafana dashboard manifest ships with `charts/agentgateway/` in
v0.1. Operators wanting dashboards build them from the OTel metrics
documented in `docs/identity/otel-wiring.md`. v0.2 will ship at least
one reference dashboard JSON under `devops/grafana/` and reference it
from the chart values.

### 9b. `AuthOnly` token-exchange mode at the STS HTTP layer

**Decided (2026-06-05).** Status `✅`. The STS rejects
`mode=AuthOnly` requests with HTTP 400 `invalid_request`.

`AuthOnly` is a frontend-proxy concern only. The proxy short-circuits
the mint path on `AuthOnly` routes (no token-exchange call) and
forwards the inbound request to the backend without an upstream mesh
credential — see
`trogon-frontend-proxy::proxy::dispatch` and PENDING_TODO §3.4. An
`AuthOnly` request reaching the STS therefore signals a
misconfiguration (or a client bypassing the proxy contract) and is
rejected at `validate_wire_request` rather than silently minted as a
degenerate identity token.

The other three Solo.io modes (`Delegation`, `ExchangeOnly`,
`ElicitationOnly`) continue to be accepted, with `ExchangeOnly` and
`ElicitationOnly` suppressing the RFC 8693 `act`/`act_chain` claims
per `ExchangeMode::suppresses_act_claim`.

### 10. Reference-deployment examples tree (§15.7)

**Deferred to v0.2.** Status `❌` for v0.1.

Upstream ships 19 example folders under `examples/`. We have **zero**
equivalent reference-deployment tree for v0.1. The
`devops/docker/compose/compose.a2a.smoke.yml` and `compose.a2a.full.yml`
files cover the smoke path; the runbook covers single-binary
deployment. A `docs/examples/` or `examples/` tree mirroring upstream's
user-facing samples is a v0.2 documentation deliverable
(`docs/get-started/` already exists as the Diátaxis Tutorial root).

## Consequences

**Positive**

- Every clone-derived action item in §15.8 has an Accepted decision.
- v0.1 release sign-off (PENDING_TODO §12) is unblocked on these 10
  items.
- v0.2 roadmap inherits 6 concrete deferred items with crisp scope:
  RFC 9728, `/metrics`, `failureMode`, multi-hop STS, Grafana, examples.

**Negative**

- v0.1 MCP clients that hard-require RFC 9728 discovery will not
  auto-connect — they need a manual JWKS URI configuration today.
  Acceptable: no upstream MCP client today blocks on RFC 9728.
- No Prometheus `/metrics` endpoint may surprise operators wiring
  legacy Prometheus pull. The OTel collector workaround is
  well-trodden; documented in `docs/identity/otel-wiring.md`.

**Neutral**

- The "implicit drain via sequential ingress" position must be
  re-examined the moment per-request concurrency lands. Captured here
  so the v0.2 review revisits §4 of this ADR.

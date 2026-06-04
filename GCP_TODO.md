# Agent Gateway Gap Plan

Source spec: <https://docs.cloud.google.com/gemini-enterprise-agent-platform/govern/gateways/agent-gateway-overview>

This document lives at the repo root because it is a working plan, not
internal scratch. It captures (a) what TrogonStack has today against
the Agent Gateway capability surface, (b) what we are missing, (c) the
decisions we have made about what to actually build vs. drop, and (d)
the sequenced work plan.

The capability surface was inventoried directly from the Google docs.
The codebase coverage was confirmed by surveying the crates under
`rsworkspace/crates/`. Inline file paths point to the evidence so a
future reader can verify before changing direction.

---

## 1. Decision summary

| # | Item                                       | Decision            | Rationale (short)                                                                |
|---|--------------------------------------------|---------------------|----------------------------------------------------------------------------------|
| 1 | mTLS edge (Block G)                        | **Build**           | Test scaffold already names this; biggest single security gap.                   |
| 2 | Content-scanner callout (plugin stage)     | **Wire seam, ship stub** | `PluginStage` already exists; we ship the contract + a no-op scanner.       |
| 3 | A2A per-skill policy bundles + ingress audit | **Build**         | `a2a-gateway/src/lib.rs:7` already calls this out as future work.                |
| 4 | Outbound DLP scanner                       | **Wire seam, defer impl** | Use the same plugin contract as #2; ship redaction stub, not ML.            |
| 5 | Toxic tool-combination policy              | **Build (light)**   | Cheap to add as a CEL combinator on the existing hierarchical policy.            |
| 6 | Read-only vs read-write tool capability    | **Build (data model)** | Add `ToolCapability` enum; thread through registry + policy.                  |
| 7 | OAuth `authorization_code` + PKCE          | **Build**           | Needed for "Cursor/Claude Code/Gemini CLI"-style clients we want to support.     |
| 8 | OAuth `client_credentials`                 | **Skip**            | WIF (RFC 8693) covers the workload case; second grant adds risk without payoff. |
| 9 | Frontend reverse-proxy / passthrough HTTP  | **Build (thin)**    | A first-class HTTP ingress in front of MCP/A2A gateways; reuse axum.             |
| 10 | General egress proxy (TCP/HTTP)           | **Skip**            | Token-forwarding egress is sufficient for our model; TCP proxy is a product detour. |
| 11 | Per-region IAP-equivalent (Cloud IAP)     | **Skip**            | Out of scope for an on-prem / multi-cloud product.                               |
| 12 | "Cloud Logging / Cloud Trace" SaaS links  | **Skip as SaaS coupling** | We already emit OTLP + structured audit; integrators wire their own sinks. |
| 13 | Semantic Governance / Service Extensions  | **Reframe**         | We expose this through the existing plugin/callout seam; no new primitive.       |
| 14 | Model Armor product integration           | **Skip product-specific** | Hook in #2 + #4 lets any scanner integrate; we don't ship a Google dep.    |
| 15 | VPC-SC compatibility, org constraints     | **N/A**             | GCP-native concerns; not relevant to us.                                         |

Lines we are explicitly **not** crossing: GCP-native primitives (Cloud
IAP, VPC-SC, Cloud Logging-as-required-sink), product-specific
integrations (Model Armor by name), and convenience grants that
duplicate what WIF already gives us.

---

## 2. Capability matrix (status as of this writing)

### Present — no work needed

| Capability                                | Evidence |
|-------------------------------------------|----------|
| SPIFFE IDs                                | `trogon-sts/src/spiffe_id.rs` (`SpiffeId::parse`, `trust_domain()`); STS-minted SVIDs in `workload_svid.rs` |
| DPoP / `cnf.jwk` / jkt binding            | `trogon-aauth-verify/src/jkt.rs`, `trogon-aauth-person/src/core.rs:91`, `nats_pop.rs` |
| WIF / RFC 8693 token exchange             | `trogon-aauth-person/src/wif_exchange.rs`, `trogon-sts/src/exchange.rs:311` |
| Per-tool authorization                    | `trogon-mcp-gateway/tests/hierarchical_policy_merge.rs`, `trogon-agent-registry/src/types.rs:21` (`allowed_tools`) |
| MCP method-level authorization            | `trogon-mcp-gateway/src/authz.rs`; `admin_api.rs` `method=tools/list`, `method=tools/call` |
| Agent registry                            | `trogon-agent-registry/src/types.rs` (`AgentRecord`), NATS KV backing |
| Multi-region                              | `trogon-mcp-gateway/src/multi_region/` (`RegionRouter`, `RegionTopology`) |
| Structured audit                          | `trogon-mcp-gateway/src/audit.rs` JetStream `{prefix}.audit.{outcome}.{direction}.{method_root}` |
| OpenTelemetry tracing                     | `trogon-telemetry/src/`; W3C `traceparent` in `plugin/mod.rs:28` |
| Dry-run / shadow mode                     | `AudienceShadowMode::Shadow`; `GET /admin/policy/inspect`; `aauth.rs:92` |
| Default-deny on no-policy / unknown-server | `policy/hierarchical.rs:807` `default_deny_when_no_bundles`; `tenancy_boundary.rs:244` |
| Tenant scoping                            | `tests/tenancy_boundary.rs`; metric label `LABEL_TENANT` |
| gRPC                                      | `trogon-gateway-xds` (tonic + tonic-prost for envoy ADS/LDS/CDS) |
| REST                                      | `a2a-nats-server/src/router.rs` (axum, SSE) |

### Partial — finish the existing surface

| Capability                                | What ships                                                                 | What's missing                                                                                  |
|-------------------------------------------|----------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------|
| A2A per-skill authz                       | Tier1 declarative + tier2 CEL + tier3 redaction in `a2a-gateway/src/policy/` | Per-skill bundles, ingress audit emission. `a2a-gateway/src/lib.rs:7` names this future work.   |
| Content-scanner hook                      | `PluginStage::{PreAuthz, PreCall, PostCall}` over NATS in `trogon-mcp-gateway/src/plugin/mod.rs` | No bundled scanner; no published subject contract.                                              |
| Outbound DLP                              | JSONPath field redaction in `a2a-redaction/` + Wasm bundles                | PII regex / ML classifier; integration contract for an external scanner.                        |
| Egress                                    | RFC 8693 mesh-token mint + cache in `trogon-mcp-gateway/src/egress/`        | General HTTP/TCP egress proxy — but see decision #10: skip.                                     |
| Frontend HTTP ingress                     | `GatewayMode::Passthrough` mode bit, `list_filter.rs:86` `passthrough_outcome()` | Dedicated HTTP reverse-proxy component in front of MCP/A2A.                                     |

### Absent — net-new work

| Capability                                | Notes                                                                                                                    |
|-------------------------------------------|--------------------------------------------------------------------------------------------------------------------------|
| mTLS termination at edge                  | `trogon-mcp-gateway/tests/tls_edge.rs` is all `#[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]`. |
| Toxic tool-combination policy             | No multi-tool combinator rule.                                                                                           |
| Read-only vs read-write tool capability   | Not a first-class primitive. OAuth-style scope strings (`read:tools`) exist but aren't tool-capability flags.            |
| `authorization_code` + PKCE grant         | Needed for human-driven clients (CLIs, IDE extensions).                                                                  |

---

## 3. Workstreams (sequenced)

Each workstream is independently shippable. They are ordered by
dependency (earlier blocks unblock later) and by how much the gap
costs us today.

### 3.1 Block G — mTLS edge for `trogon-mcp-gateway`

**Why now**: The single largest defensive gap. The Google spec
treats mTLS as a *default* — every other capability rides on top.
Tests are already written; production code is not.

**Decisions**:
- TLS 1.3 only. The scaffold already chose this; don't relitigate.
- SPIFFE SVID SAN validation on the client cert. We already mint
  SVIDs in `trogon-sts`; reuse `SpiffeId::parse` for the check.
- Trust domain enforced from config, not from cert. One trust
  domain per gateway instance; reject anything else.
- mTLS is **required** for NATS-fronting deployments, **optional**
  for the admin HTTP API behind a separate flag. Don't merge the
  two policies.

**Acceptance**:
- All `#[ignore]` markers removed from `tests/tls_edge.rs`.
- `mtls_required`, `mtls_san_mismatch`, `mtls_trust_domain_mismatch`
  passing against a real `rustls` server.
- Cert reload without restart, driven by a file-watcher (matches
  how Envoy's SDS surface works).

**Files**:
- New: `trogon-mcp-gateway/src/tls/{mod.rs,server.rs,verifier.rs,reload.rs}`
- Test: `trogon-mcp-gateway/tests/tls_edge.rs` (un-ignore)
- Config: extend `trogon-mcp-gateway/src/config.rs` with `TlsConfig`

### 3.2 Plugin-stage scanner contract + content-scan stub

**Why now**: `PluginStage::{PreAuthz, PreCall, PostCall}` is the
seam Google's spec calls "Service Extensions" / "Model Armor
integration." We have the seam. We do not have a documented
contract or a default scanner that integrators can drop in.

**Decisions**:
- Single named subject: `{prefix}.plugin.contentscan`. Both prompt
  scanning (PreCall) and outbound scanning (PostCall) reuse it; the
  payload carries `stage` so a single scanner can serve both.
- Request payload: `{request_id, tenant, agent, tool, stage,
  direction, content_type, content}` as JSON.
- Response payload: `{decision: allow|block|redact, findings:
  [{type, location, severity}], redacted_content?}`.
- We ship a **stub scanner** in `trogon-content-scan-stub` that
  always allows but emits an audit entry. Integrators replace this
  with Lakera / Model Armor / in-house.
- Decision: do not bundle regex / ML scanners. That's a product on
  its own; out of scope.

**Acceptance**:
- Subject + payload schemas in `trogon-mcp-gateway/src/plugin/contentscan.rs`.
- Stub crate compiles and subscribes; integration test that exercises
  PreCall + PostCall flow against the stub.
- Schema documented in `docs/reference/plugin-contentscan.md`.

**Files**:
- New: `trogon-mcp-gateway/src/plugin/contentscan.rs`
- New crate: `rsworkspace/crates/trogon-content-scan-stub/`
- Test: `trogon-mcp-gateway/tests/contentscan_e2e.rs`

### 3.3 A2A per-skill policy bundles + ingress audit

**Why now**: Explicitly flagged as future work in
`a2a-gateway/src/lib.rs:7`. Finishing it closes the symmetry gap
between MCP gateway (which has per-method policy) and A2A gateway
(which currently doesn't have per-skill policy).

**Decisions**:
- "Skill" is keyed by the A2A skill ID exactly as the agent card
  publishes it. Don't invent a synthetic skill name space.
- Bundle shape mirrors MCP's hierarchical policy: tenant → agent →
  skill, with default-deny when no rule matches in `require` mode.
- Ingress audit reuses the existing structured-audit envelope; new
  subject `{prefix}.a2a.audit.{outcome}.ingress.{skill}`.

**Acceptance**:
- `a2a-gateway/src/lib.rs:7` comment can be deleted.
- Per-skill `allow` / `deny` test, plus shadow-mode test that
  emits audit without enforcing.
- `default_deny_when_no_bundles` parity test with MCP.

**Files**:
- New: `a2a-gateway/src/policy/per_skill.rs`
- New: `a2a-gateway/src/audit_ingress.rs`
- Tests: `a2a-gateway/tests/per_skill_policy.rs`

### 3.4 Read-only vs read-write tool capability

**Why now**: Two-line model addition that unlocks coarse-grained
policy ("agents in role X get read-only on all tools by default").
Without it, every policy is per-tool, which doesn't scale.

**Decisions**:
- New enum on `AgentRecord` / tool metadata:
  `pub enum ToolCapability { Read, Write }`.
- The tool's MCP server declares the capability; the gateway treats
  it as authoritative. If unset, treat as `Write` (fail-safe).
- New policy primitive: `allow_capabilities: Vec<ToolCapability>`
  on `AgentRecord` alongside `allowed_tools`.
- Resolution order: `allowed_tools` (explicit) → `allow_capabilities`
  (coarse) → default-deny.

**Acceptance**:
- `ToolCapability` round-trips through the registry KV.
- Policy test: agent with `allow_capabilities: [Read]` can call
  `get_*` but is denied `set_*` even when both are registered.

**Files**:
- `trogon-agent-registry/src/types.rs` (extend `AgentRecord`)
- `trogon-mcp-gateway/src/authz.rs` (add resolution)
- Tests: `trogon-mcp-gateway/tests/tool_capability_resolution.rs`

### 3.5 Toxic tool-combination policy

**Why now**: Cheap to add on top of the existing hierarchical
policy and the only meaningful "compound risk" primitive we don't
have. Spec calls it out explicitly.

**Decisions**:
- Rule shape:
  ```toml
  [[deny_combinations]]
  tools = ["send_email", "read_secrets"]
  reason = "exfiltration risk"
  ```
- Evaluated at session level, not per-call. We track tool-set per
  agent session and deny the first call that completes a forbidden
  set.
- Default rule set ships empty; operators opt in.

**Acceptance**:
- `tests/toxic_combination_session.rs`: agent calls `read_secrets`
  → ok; subsequent `send_email` in same session → deny with reason.

**Files**:
- New: `trogon-mcp-gateway/src/policy/toxic_combinations.rs`
- Extend `policy/hierarchical.rs` to consult it

### 3.6 OAuth `authorization_code` + PKCE for human clients

**Why now**: We want Cursor / Claude Code / IDE extension users to
authenticate as themselves, not as a workload. WIF can't do that.

**Decisions**:
- New endpoints on `trogon-aauth-person/src/http.rs`:
  - `GET  /aauth/oauth/authorize`
  - `POST /aauth/oauth/token` (grant_type=authorization_code, with
    `code_verifier`)
- PKCE is **required** (S256 only). No plain.
- Reuses the existing `PersonCore` consent surface — `authorize`
  goes through `ConsentPolicy::decide`. Don't fork the consent
  model.
- Client registration is record-based, not dynamic — admins post
  `OAuthClientRecord` via NATS (mirror of `wif_admin`).
- No refresh tokens in v0.1. Short-lived access token + re-auth.

**Acceptance**:
- Round-trip test: PKCE-S256 code exchange yields an `aa-auth+jwt`.
- Invalid `code_verifier` returns 400.
- Client without registration returns 401.

**Files**:
- New: `trogon-aauth-person/src/oauth_code.rs`
- New: `trogon-aauth-person/src/oauth_clients.rs` (records + store)
- Extend: `trogon-aauth-person/src/http.rs` (new routes)

### 3.7 HTTP ingress reverse-proxy (`trogon-frontend-proxy`)

**Why now**: The Google spec assumes external clients hit the
gateway over HTTPS and the gateway dispatches to MCP / A2A
backends. We have the backends; we don't have the front door.

**Decisions**:
- New crate `trogon-frontend-proxy`. Single binary. Axum + reqwest
  for passthrough.
- TLS termination via the same code path as workstream 3.1.
- Route table is data-driven: tenant → backend (MCP or A2A) → upstream
  URL. Stored in JetStream KV bucket `trogon-frontend-routes`.
- Egress to backends carries a mesh JWT minted by `trogon-sts`. No
  bare passthrough; every hop is authenticated.
- Defer rate limiting, defer WAF rules. Door first, knobs later.

**Acceptance**:
- E2E test: HTTPS client → frontend proxy → MCP gateway → backend
  with correct tenant attribution end to end.
- Audit entry for every dispatched request, even on backend failure.

**Files**:
- New crate: `rsworkspace/crates/trogon-frontend-proxy/`
- New route bucket: `trogon-frontend-routes` in `trogon-nats`

---

## 4. Sequencing

The blocks above are roughly independent, but two ordering
constraints matter:

```
3.1 mTLS edge  ──────┐
                     ├──> 3.7 frontend proxy
3.2 scanner hook ────┘
3.2 scanner hook  ──> 3.3 A2A per-skill (reuses audit shape)
3.4 capability  ──> 3.5 toxic combinations (capability gives the
                          coarse default that combinations refine)
3.6 OAuth code  ──> (independent; ship anytime)
```

Recommended order:
1. **3.1** mTLS edge — security floor first.
2. **3.2** scanner contract — short, unblocks future scanners.
3. **3.4** capability primitive — model change cheaper now than later.
4. **3.5** toxic combinations — builds on 3.4.
5. **3.3** A2A per-skill — parallelizable, sized like 3.1.
6. **3.6** OAuth code — pick up whenever there's bandwidth.
7. **3.7** frontend proxy — last because it depends on 3.1 + 3.2.

---

## 5. Explicit non-goals

- **GCP-native integrations**: Cloud IAP, VPC Service Controls,
  Cloud Logging as required sink, organization policy constraints.
  Out of scope; we are not a GCP product.
- **Bundled content scanners**: We ship the seam, not the scanner.
  Bundling pulls in legal + model-licensing concerns we shouldn't
  own.
- **General TCP/HTTP egress proxy**: Our model is identity-gated
  egress, not perimeter egress. The token-minting egress we have
  is sufficient.
- **`client_credentials` grant**: WIF (RFC 8693) is the better
  primitive. Two ways to do the same thing is a footgun.
- **Refresh tokens (v0.1)**: Short-lived access + re-auth. Add later
  if the UX demands it.
- **Drop-in Agent Gateway API compatibility**: We share concepts
  (WIF, SPIFFE, mTLS, per-tool authz), not surfaces. Don't drift
  toward `aiplatform.googleapis.com` shapes.

---

## 6. What to do *before* starting any block

For each block:
1. Open a short proposal PR with the section above as the PR body.
2. Stub the new files/crates with `todo!()` and the test names.
3. Get sign-off on subject names and config keys (they're hard to
   change post-merge).
4. Then implement.

This prevents the "I wrote 800 lines and now we're renaming the
subject" failure mode that bit us during the WIF work.

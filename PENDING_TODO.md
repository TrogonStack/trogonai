# PENDING_TODO — Agent Identity Gaps vs. Uber's "Agent Identity Crisis"

Reference: https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/
Companion to: `MCP_GATEWAY_PLAN.md`

This file enumerates the work *not yet in the plan* (or only implicit) to bring the TrogonAI MCP gateway / A2A surface up to the architecture Uber describes. Items are grouped by track. Each item lists: **decisions to make (paper)**, **wire/spec deliverables**, **code deliverables**, and **acceptance signal**.

Block 0 decisions are **Accepted as of 2026-05-27** (see the Decisions table below). The rest of this document is the execution plan that flows from those decisions — no further approval gate.

## Verified branch snapshot — 2026-05-26

Branch reviewed: `yordis/agentgateway`.

### Already present in the branch

- **NATS account-scoped caller authentication.** `a2a-auth-callout` mints NATS User JWTs with `sub`, `aud`, `caller_id`, `data`, and NATS subject permissions.
- **Caller-bounded subject ACL shape.** Minted callers publish to `a2a.gateway.>` and subscribe to `_INBOX.{caller_id}.>` / `a2a.push.{caller_id}.>`.
- **A2A gateway caller attribution.** `a2a-gateway` verifies the `A2a-Caller-Jwt` message header, with `A2A_GATEWAY_TRUST_CALLER_HEADERS` as a labs-only fallback.
- **Gateway policy substrate.** The branch has gateway forwarding, Tier-1 SpiceDB/declarative checks, Tier-2 CEL hooks, Tier-3 redaction, audit envelopes, and trace IDs.
- **MCP gateway JWT ingress.** `trogon-mcp-gateway` validates issuer, audience, expiration, `sub`, and tenant, then strips forgeable tenant headers and projects verified context to backend headers.
- **Bridge reminting foundation.** `a2a-bridge` can mint a caller User JWT per request instead of using one bridge-wide caller identity.

### Missing vs. Uber's target model

The current branch proves perimeter authentication and gateway enforcement. It does **not** yet implement the agent-delegation identity plane described by Uber.

- **No Agent Registry.** There is no authoritative record binding `agent_id` to owner, version, definition digest, allowed workloads, allowed tools, or lifecycle state.
- **No workload attestation.** There is no SPIFFE/SPIRE/SVID or equivalent trust anchor for proving which workload is running an agent.
- **No STS / token-exchange service.** There is no service that exchanges a caller token plus workload proof for a short-lived, target-audience token.
- **No per-hop mesh token.** Current JWTs are account/caller scoped; they are not minted per hop with narrowed `aud`, `scope`, and TTL.
- **No actor chain.** There is no `act_chain` claim, schema, verification logic, CEL surface, audit field, loop detection, or max-depth enforcement.
- **No purpose / intent claim.** Policies cannot yet reason over why an agent is acting, only who the caller appears to be and which target/method/resource is requested.
- **No gateway egress token minting.** The MCP gateway propagates verified context headers; it does not replace inbound identity with a backend-scoped STS token.
- **No standardized identity-aware A2A SDK contract.** The current client can attach a minted caller JWT to gateway ingress; it does not do target lookup, token exchange, actor-chain verification, or typed `Caller` handling.
- **No agent-centric traffic view.** Audit exists, but not a lineage view answering "what did agent X do on behalf of user Y across this chain?"
- **No adaptive access path.** There is no step-up approval, human-in-the-loop hold/resume flow, or policy result tied to actor-chain risk.
- **No agent-identity rollout mode.** There is no `off` / `shadow` / `enforce` mode for validating `aud`, `agent_id`, `wkl`, and `act_chain` before hard rejection.

### Practical conclusion (superseded by 2026-05-27 snapshot below)

The auth-callout JWT is the **bootstrap credential** (ADR 0003 Accepted). The 2026-05-27 snapshot below is the current source of truth.

## Verified branch snapshot — 2026-05-27

Swarm run `20260527T055911Z-6d70` merged into `yordis/agentgateway` (26 commits, `cargo check`/`cargo test -p trogon-mcp-gateway` green).

### Decisions

The 6 Block-0 ADRs are **Accepted** as of 2026-05-27. Implementation in the rest of this file is bound to these choices; no further approval gate.

| # | Decision | Reference |
|---|---|---|
| 0001 | **Hybrid tenancy.** Account-per-tenant in production; JWT `tenant` claim in dev/CI/single-tenant. Authority order: `nats.account` > validated `jwt.tenant` > never client headers. SpiceDB principals namespaced `tenant:{tenant_id}/user:{sub}` (and `agent:{agent_id}`) in both modes. | `docs/adr/0001-tenancy-model.md` |
| 0002 | **Two-claim identity.** `sub` = logical actor (`agent:{tenant}/{name}` for agents, originator sub for humans). `wkl` = attested workload (SPIFFE URI or `sentinel:human` / `sentinel:batch` / `sentinel:api-key`). `agent_id` required iff `wkl` is SPIFFE *and* caller is an agent workload. New `auth_method` claim is mandatory. | `docs/adr/0002-identity-layers.md` |
| 0003 | **Bootstrap → mesh.** Auth-callout JWT is bootstrap; STS mints short-lived per-hop tokens. The transitional read-vs-write hybrid is permitted during shadow→enforce migration only, then retired. | `docs/adr/0003-bootstrap-vs-mesh-tokens.md` |
| 0004 | **STS on NATS.** STS runs as `trogon-sts`, a queue-group consumer on `mcp.sts.exchange`. Wire contract is transport-agnostic so an HTTP facade can be added later. Every exchange emits `mcp.audit.sts.{outcome}`. | `docs/adr/0004-sts-form-factor.md` |
| 0005 | **TTL 120s default (60–300s range), per-backend `aud`.** Audience URIs of the form `urn:trogon:mcp:backend:{tenant}:{server_id}` / `urn:trogon:a2a:agent:{tenant}:{agent_id}` / `urn:trogon:mcp:gateway:{tenant}:{instance_id}` / `urn:trogon:mcp:client:{tenant}:{client_id}`. Clock skew ±30 s. Tools live in `scope`, not `aud`. Hard reject on `aud` mismatch in enforce; log in shadow. New JSON-RPC error code `-32109 audience_mismatch`. | `docs/adr/0005-token-ttl-and-audience.md` |
| 0006 | **Cloud KMS / Vault Transit in prod, file PEM in dev.** JWKS published primarily via NATS KV `mcp-jwks/mesh/current`, secondarily via HTTPS `/.well-known/jwks.json`, tertiarily via request/reply on `mcp.jwks.mesh.get`. Overlap window ≥ max mesh TTL + skew. Distinct `iss` for bootstrap vs. mesh. | `docs/adr/0006-mesh-token-signing-keys.md` |

### Already landed (in `yordis/agentgateway` as of this snapshot)

- **Identity specs accepted as contracts.** `docs/identity/jwt-claim-schema.md`, `act-chain.md`, `sdk.md`, `registry.md`. Used as the source of truth by the work below.
- **Shadow-mode claim parsing.** Optional JWT claim fields parsed in `trogon-mcp-gateway` (`agent_id`, `agent_version`, `wkl`, `wkl_attested_at`, `purpose`, `session_id`). Exposed in CEL as `jwt.agent_id`, `jwt.agent_version`, `jwt.wkl`, `jwt.purpose`, `jwt.session_id`. New env var `MCP_GATEWAY_AGENT_IDENTITY = off (default) / shadow / enforce`. Enforce path is currently a log-only stub — finishing it is in the roadmap below.
- **Shadow-mode `act_chain` header.** `act_chain.rs` module + `ActChainEntry { sub, agent_id, wkl, iat }`. Header `mcp-act-chain` parsed on ingress, stripped from forgeable client input, gateway-appended in shadow/enforce, depth-capped at 8 with WARN on overflow. `MCP_GATEWAY_IDENTITY_SUB` env var for the gateway's appended entry.
- **Audit envelope extended.** Optional identity fields on the audit envelope (`agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`, `act_chain`), all `skip_serializing_if = "Option::is_none"` so the existing wire format is byte-identical when nothing is set.

### Execution roadmap (drives to completion, no gates)

Ordered for forward motion. Each item is concrete and independent enough to fan out.

1. **Finish ingress + enforce.** Promote `MCP_GATEWAY_AGENT_IDENTITY=enforce` from log-only stub to hard reject for: missing `wkl` when `auth_method=svid`/`spire`, SPIFFE `wkl` without matching `agent_id`, `aud != self`, `act_chain` over depth, `(agent_id, wkl)` loop. Strip/refuse client-supplied `agent_id` / `wkl` / `auth_method` JWT claims at ingress unless minted by a trusted issuer. Add JSON-RPC error `-32109 audience_mismatch`.
2. **Expose `act_chain` in CEL.** Add `jwt.act_chain` as `list<map>`, plus helpers `chain.contains(agent_id)`, `chain.originator()`, `chain.depth()`. Wire into the existing CEL evaluation in `policy.rs`.
3. **Update `MCP_GATEWAY_PLAN.md`.** Add `mcp-act-chain` row to § Wire-Format Pins (`MCP_GATEWAY_PLAN.md:829`), append the new CEL variables to § CEL variable namespace, document the optional `act_chain` field in § Audit envelope schema.
4. **Identity overview + STS-exchange docs.** Write `docs/identity/overview.md` (single-page architecture + mesh token + chain diagram, anchored on ADRs 0001–0006) and `docs/identity/sts-exchange.md` (request/response, examples, error codes, audit events) — these are the operator-facing reference for everything below.
5. **Agent registry (minimum viable).** Stand up the NATS KV bucket `mcp-agent-registry` (Git-as-source-of-truth synced by a control-plane signer); implement `mcp.registry.agent.lookup` queue-group consumer returning the registry entry or NACK; emit mutations to `mcp.audit.registry.*`. Schema per `docs/identity/registry.md`.
6. **JWKS publisher.** New `trogon-jwks-publisher` sidecar that exports public JWKS from KMS/Vault to KV `mcp-jwks/mesh/current` + HTTPS well-known + `mcp.jwks.mesh.get`. Gateway adds a KV-watcher source for hot reload. Rotation runbook in the same crate's README.
7. **`trogon-sts` v1.** Queue-group consumer on `mcp.sts.exchange`. Inputs/outputs per ADRs 0003–0005 and `docs/identity/sts-exchange.md`. Hot caches: trust bundle, registry, signing keys. Per-`wkl` and per-`agent_id` rate limits. Emits `mcp.audit.sts.{outcome}`. File-PEM signer for dev; KMS/Vault for prod.
8. **Gateway egress minting.** Replace verified-context propagation in the MCP gateway with an STS exchange before backend egress. Cache key per ADR 0005; serve callback direction with `aud=client_id`. Same change in any A2A gateway hop.
9. **A2A SDK v1 (Rust).** Implement the contract in `docs/identity/sdk.md`: `call(target_agent, payload, purpose)` (lookup → exchange → send → receive), `serve(handler)` (verify → verify `act_chain` → typed `Caller`). OpenTelemetry attributes. Then TypeScript, then Python.
10. **SPIRE wiring (production attestation).** Replace sentinel-`wkl` fallback for service workloads with real SVID attestation at STS ingress; trust-bundle distribution via NATS KV `mcp-trust-bundles`. Spec details live with Block 1.2 below.
11. **Agent-traffic view.** Build the index on top of the already-populated audit envelope: timeline, chain explorer, top-N dashboards, SIEM export (Block 4). Pick CEF/OCSF format and stand up the JetStream durable consumer.
12. **Adaptive access.** Step-up auth, human-in-the-loop approvals (new error `-32107 approval_required`, approval responses on `mcp.approvals.{request_id}`), context-aware rate limits keyed by `(agent_id, purpose, tenant)`, anomaly feature emission (Block 5).

---

## Block 0 — Cross-cutting decisions (Accepted 2026-05-27)

All six decisions are committed. Implementation in the blocks below is bound to these choices.

- [x] **Tenancy model — Accepted.** Hybrid: NATS account per tenant in production, JWT `tenant` claim in dev/CI/single-tenant. Authority order: `nats.account` > validated `jwt.tenant` > never client headers. SpiceDB principals namespaced `tenant:{tenant_id}/user:{sub}` (and `agent:{agent_id}`) in both modes. → `docs/adr/0001-tenancy-model.md`.

- [x] **Identity layering — Accepted.** Two-claim identity: `sub` = logical actor (`agent:{tenant}/{name}` for agents, originator sub for humans); `wkl` = attested workload (SPIFFE URI or sentinel `sentinel:human` / `sentinel:batch` / `sentinel:api-key`). `agent_id` mandatory iff `wkl` is SPIFFE and caller is an agent workload. New `auth_method` claim is mandatory on every JWT. → `docs/adr/0002-identity-layers.md`.

- [x] **Bootstrap vs. mesh tokens — Accepted.** Option (b): the NATS auth-callout JWT is the **bootstrap** credential; STS mints short-lived per-hop mesh tokens. Transitional read-vs-write hybrid is permitted during shadow→enforce migration only, then retired. → `docs/adr/0003-bootstrap-vs-mesh-tokens.md`.

- [x] **STS form factor — Accepted.** Option (a): NATS service `trogon-sts` on `mcp.sts.exchange`, queue-group consumer. Wire contract transport-agnostic so an HTTP facade can be added later. Every exchange emits `mcp.audit.sts.{outcome}`. → `docs/adr/0004-sts-form-factor.md`.

- [x] **TTL and `aud` discipline — Accepted.** Default mesh TTL 120 s (range 60–300 s via registry); clock skew ±30 s; per-backend `aud` URIs (`urn:trogon:mcp:backend:{tenant}:{server_id}` etc.); tools live in `scope`, not `aud`. Hard reject on `aud` mismatch in enforce mode (JSON-RPC `-32109 audience_mismatch`); log in shadow. Egress cache keyed `{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}`, max age TTL/2. → `docs/adr/0005-token-ttl-and-audience.md`.

- [x] **Key material custody — Accepted.** Cloud KMS or Vault Transit in production; file PEM in dev. JWKS publication: NATS KV `mcp-jwks/mesh/current` (primary), HTTPS `/.well-known/jwks.json` (secondary), request/reply `mcp.jwks.mesh.get` (tertiary). Overlap window ≥ max mesh TTL + skew. Distinct `iss` for bootstrap vs. mesh. → `docs/adr/0006-mesh-token-signing-keys.md`.

---

## Block 1 — Agent identity primitives

Today the gateway sees `jwt.sub`, `jwt.tenant`, `jwt.roles` and nothing distinguishes an agent from the workload running it. Uber's Agent Registry exists to bridge this gap.

### 1.1 Agent registry

Spec at `docs/identity/registry.md` is the accepted contract. Implementation tasks:

- [x] **Agent entity — decided.** Fields per `docs/identity/registry.md`: `agent_id` (namespaced `{tenant}/{name}`), `agent_version`, `agent_definition_digest`, `owner_team`, `allowed_workloads` (SPIFFE IDs), `allowed_tools`, `allowed_audiences`, `allowed_purposes`, `mesh_token_ttl_s`, `metadata`.
- [x] **Storage — decided.** NATS KV bucket `mcp-agent-registry` is the runtime cache; Git manifest is source of truth, synced by a control-plane signer.
- [ ] **Lifecycle implementation.** Build the control-plane signer + sync loop: registration, version bump, deprecation, revocation via signed manifests. Writes restricted to the signer identity.
- [ ] **Lookup API implementation.** Stand up the `mcp.registry.agent.lookup` queue-group consumer; return full record + current `allowed_workloads`, or NACK on miss.
- [ ] **Audit hook implementation.** Wire every registry mutation to `mcp.audit.registry.*`.

**Acceptance:** A workload (SPIFFE ID) cannot present `agent_id=X` to the STS unless the registry confirms it is in `allowed_workloads` for `X`.

### 1.2 Workload attestation (SPIFFE / SPIRE)

Attestation model is fixed by ADR 0002 (sentinel `wkl` namespace for non-SPIFFE, SPIFFE URI for service workloads). Implementation tasks:

- [x] **Attestation source — decided.** SPIRE server for production; file-based SVIDs for dev/CI. Cloud IMDS-based attestation is an optional add-on after SPIRE is in.
- [ ] **Trust handshake implementation.** At STS: client presents SVID (mTLS or JWT-SVID in the exchange request); STS validates against trust bundle. Trust-bundle distribution via NATS KV `mcp-trust-bundles`, refreshed every N minutes.
- [ ] **SVID → `wkl` mapping implementation.** Map SPIFFE ID (e.g. `spiffe://acme.local/ns/prod/sa/oncall-agent`) to JWT claim `wkl` + `wkl_attested_at` at STS mint time.
- [x] **Non-SPIFFE fallback — decided.** Sentinel `wkl` values: `sentinel:human` (+ `auth_method: "oidc"` / `"mtls"` / `"api-key"`), `sentinel:batch`, `sentinel:api-key`. See ADR 0002.

**Acceptance:** STS refuses to mint a mesh token without a valid SVID (or explicit non-SPIFFE auth-method in a documented allow-list).

### 1.3 JWT claim schema additions

Spec at `docs/identity/jwt-claim-schema.md` is the accepted contract. Shadow-mode parsing landed (commit `d5754a40b`); remaining gaps below.

- [x] **New claims standardized.** Parsing implemented for all but `act_chain` (which lives on `mcp-act-chain` header per ADR 0002 / `act-chain.md`).
  - `agent_id` (string, optional for non-agent callers). Parsed.
  - `agent_version` (string). Parsed.
  - `wkl` (SPIFFE ID string). Parsed (unverified — no SPIRE yet).
  - `wkl_attested_at` (timestamp). Parsed.
  - `act_chain` (array, see Block 2.2). Lives on the `mcp-act-chain` header, not on the JWT; see Block 2.2.
  - `purpose` / `intent` (free-form string, see Block 2.3). Parsed.
  - `session_id` (already implicit; lift to claim). Parsed.
- [x] **Update CEL variable namespace** (`MCP_GATEWAY_PLAN.md:941`) to expose `jwt.agent_id`, `jwt.wkl`, `jwt.act_chain`, `jwt.purpose`. Done for `agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`. `jwt.act_chain` is still missing — see Block 2.2 CEL surface.
- [ ] **Update ingress-hardening rule** (`MCP_GATEWAY_PLAN.md:835`) to also strip/refuse client-supplied `act_chain`, `agent_id`, `wkl`. Only `mcp-act-chain` header is stripped today; client-supplied `agent_id`/`wkl` *claims* on a JWT are still accepted at face value when present.

**Acceptance:** A JWT without `wkl` is rejected by the gateway in `require` mode (NOT YET — `MCP_GATEWAY_AGENT_IDENTITY=enforce` is a log-only stub). CEL rules can reference `jwt.agent_id` in policy bundles (DONE).

---

## Block 2 — Per-hop token exchange and delegation

This is the largest piece of net-new work. Today: one connect-time JWT covering broad subject ACL. Target: short-lived, audience-scoped JWT per hop with `act_chain` lineage.

### 2.1 Security Token Service (STS)

Form factor and contract fixed by ADRs 0004/0005/0006. Implementation tasks for `trogon-sts` v1:

- [x] **Exchange contract — decided.** RFC 8693-inspired. Inputs: `subject_token`, `subject_token_type`, `audience` (target hop URI), `scope` (optional tool-name list), `requested_token_type` (default JWT), `purpose`, `actor_token` (caller's SVID). Outputs: `access_token`, `issued_token_type`, `expires_in`. Wire spec lives at `docs/identity/sts-exchange.md` (item 4 in roadmap).
- [ ] **Validation pipeline implementation.** Verify `subject_token` signature against trusted issuer set; verify SVID against trust bundle; look up `agent_id` in registry, verify `wkl ∈ allowed_workloads`; check `audience ∈ allowed_audiences`; append caller to `act_chain` (cap depth 8); mint new JWT with reduced scope, narrowed `aud`, fresh `iat`/`exp` per ADR 0005.
- [x] **Transport — decided.** NATS queue-group consumer on `mcp.sts.exchange`, per-request reply inbox (ADR 0004). HTTP facade deferred to v2.
- [ ] **Latency budget implementation.** Target P99 < 40 ms. In-memory trust-bundle, registry, and signing-key caches; async refresh.
- [ ] **Rate limiting implementation.** 100 exchanges / 10 s / `wkl`, 500 / 10 s / `agent_id` (defaults; per-bundle override). Circuit breaker on registry / SpiceDB outage.
- [x] **Failure mode — decided.** **Fail-closed.** STS down → mesh stops; gateways return structured error, agents retry with backoff. No degraded-mode bypass (ADR 0004).
- [ ] **Audit implementation.** Emit every success/deny/rate-limit exchange to `mcp.audit.sts.{outcome}` with full inputs and minted claims (not signature).

**Acceptance:** Calling backend B from agent A requires A to exchange its token for one with `aud=B`, `exp ≤ now+TTL`, and `act_chain` ending in A. The gateway rejects any token where `aud` doesn't match its own identity.

### 2.2 Actor chain (`act_chain`)

Spec at `docs/identity/act-chain.md` is the accepted contract. Shadow-mode header projection landed (commit `ebe012c8d`); audit-envelope embedding landed (commit `047147811`).

- [x] **Schema — decided.** `ActChainEntry { sub, agent_id, wkl, iat }`. Order: oldest (originating user) first, current actor last. Integrity by outer-JWT signature only — each STS that appends is the only trusted appender (per-entry signing rejected as too costly for the 40 ms budget).
- [x] **Propagation — decided.** On every STS exchange: new entry appended, never rewritten. Full chain serialized into the audit envelope (`act_chain` optional field, landed).
- [ ] **Verification implementation.** Registry resolution of each entry's `sub`/`agent_id` at receipt time; depth cap 8 (configurable per bundle) — convert current shadow-mode WARN into hard reject in enforce; loop detection on `(agent_id, wkl)` duplicates → reject.
- [ ] **CEL surface implementation.** Expose `jwt.act_chain` as `list<map>`; add helpers `chain.contains(agent_id)`, `chain.originator()`, `chain.depth()`. Wire into `policy.rs` evaluation.
- [x] **Header projection — landed.** `mcp-act-chain` parsed on ingress, stripped from forgeable client input, gateway-appended in shadow/enforce. Header-table row in `MCP_GATEWAY_PLAN.md` is item 3 of the execution roadmap.

**Acceptance:** A four-hop call (`user → A → B → C → backend`) produces a chain of length 4 at the backend, visible in audit, and rejected if any link is missing. Currently: the gateway appends its own entry in shadow and serializes the chain into the audit envelope; depth violations WARN but do not reject; registry resolution is absent.

### 2.3 Intent / purpose claim

- [x] **Shape — decided.** Enumerated set per agent in the registry (`allowed_purposes`). Per `docs/identity/registry.md`.
- [ ] **Propagation implementation.** Originating client sets initial `purpose`; STS preserves or refines on exchange (refining requires `purpose ∈ allowed_purposes` for the refining agent). Land with STS v1 (Block 2.1).
- [x] **CEL + audit surface — landed.** `jwt.purpose` exposed in CEL; `purpose` field on audit envelope.

**Acceptance:** SpiceDB policy can gate on `(agent_id, purpose, resource)` triples instead of just `(subject, resource)`. Claim values are now reachable from CEL; whether SpiceDB rules actually consume them is bundle work.

### 2.4 Gateway egress: mint, don't propagate

- [ ] **Replace context propagation with STS exchange.** Gateway calls STS before backend egress, attaches the new token, drops the inbound one. Same change in any A2A gateway hop.
- [ ] **Cache exchanges** by `{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}`, max age `min(floor(exp - now - 30s), TTL/2)`, per ADR 0005.
- [x] **Callback direction — decided.** Server → client callbacks re-exchange with `aud=urn:trogon:mcp:client:{tenant}:{client_id}` (ADR 0005).

**Acceptance:** Backend logs show `aud=<backend>`, `iss=<sts>`, `act_chain` includes the gateway as the last hop before the backend.

---

## Block 3 — A2A client SDK

Uber's "Standardized A2A Client" is the thing that makes the secure path the only path developers reach for. Today there's a `yordis/feat-a2a-nats` branch but no documented SDK contract.

- [x] **Client contract — decided.** Per `docs/identity/sdk.md`. Constructor takes SVID source, STS NATS subject, optional registry endpoint, default audience policy. `call(target_agent, payload, purpose)` does lookup → exchange (`aud=target`) → send → receive. `serve(handler)` does verify inbound token → verify `act_chain` → invoke handler with typed `Caller { originator, chain, purpose }`.
- [x] **Language order — decided.** Rust first (matches gateway crates), then TypeScript (matches `tsworkspace/`), then Python.
- [ ] **Telemetry implementation.** Emit OpenTelemetry spans with `agent.id`, `agent.chain.depth`, `agent.purpose` attributes on every call.
- [ ] **Lint implementation.** CI lint that flags direct NATS publishes to `mcp.gateway.request.*` from agent code that isn't the SDK.
- [ ] **Docs implementation.** Quickstart, recipe for adding a new agent, recipe for verifying chain in a handler.

**Acceptance:** A new agent can be written in ≤ 50 lines and gets correct identity propagation for free.

---

## Block 4 — Observability for agentic traffic

You have the audit stream (`MCP_GATEWAY_PLAN.md:269`, `:784`). Missing: the agent-centric *view*.

- [ ] **Define the agent-traffic schema** on top of audit envelopes. Underlying audit envelope now carries `agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`, `act_chain` as optional fields (commit `047147811`); the *view/index* on top of those fields is not built.
  - Index by: `originator_sub`, `agent_id`, `chain_root`, `session_id`, `trace_id`.
- [ ] **Build the timeline view.**
  - One row per hop, with: timestamp, caller `agent_id`, callee `agent_id`/backend, tool name, decision (allow/deny/redact), latency, `purpose`.
- [ ] **Build the chain explorer.**
  - Given a `trace_id`, render the full delegation tree.
- [ ] **Top-N dashboards.**
  - Most-active agents, most-denied agents, deepest chains, longest chains by tenant.
- [ ] **SIEM export.**
  - Decide format (CEF, OCSF, raw JSON). JetStream durable consumer feeds it.
- [ ] **Decide UI substrate.**
  - Reuse `trogon-gateway` UI? Standalone? CLI-first (`agctl traffic --agent oncall`)?

**Acceptance:** A security engineer can answer "what did agent X do on behalf of user Y in the last hour, and what was blocked" in < 30 seconds.

---

## Block 5 — Dynamic / adaptive access control (Uber's Layer 2)

CEL + SpiceDB are the substrate; this block delivers the policies and flows on top. Lands after STS + SDK so it has identity to gate on.

- [ ] **Step-up auth implementation.** Signal source: tool-level annotation `sensitive: true` + CEL rule. Step-up channel: re-auth via OIDC for human originators; service workloads escalate via approval flow below.
- [ ] **Human-in-the-loop approvals implementation.** JSON-RPC error `-32107 approval_required` with `approval_url` / `approval_subject`. Approval responses land on `mcp.approvals.{request_id}`; gateway resumes the request. TTL on pending approvals; default-deny on expiry.
- [ ] **Context-aware throttling implementation.** Rate limits keyed by `(agent_id, purpose, tenant)` instead of just `jwt.sub`.
- [ ] **Anomaly feature emission.** Emit features (chain depth, novel `(agent_id, purpose, target)` tuples, exchange-rate spikes) to a side stream for downstream anomaly scoring. Scoring service itself is out of scope here.

**Acceptance:** A tool can be flagged `sensitive`; calls to it require explicit human approval; the approval is audited with the same `act_chain`.

---

## Block 6 — Migration and compatibility

Cutover is incremental: keep the bootstrap path while shadow-mode validates the mesh path, then flip to enforce.

- [x] **Feature flag — landed.** `MCP_GATEWAY_AGENT_IDENTITY = off (default) / shadow / enforce` (commit `d5754a40b`). `enforce` is currently log-only; finishing it is item 1 of the execution roadmap.
- [ ] **Shadow-mode completion.** Already: missing `agent_id` on `tools/call` and `act_chain` depth overflow WARN in shadow. Still to do: validate `aud == self` and emit `aud_mismatch` audit events in shadow (no rejection until enforce).
- [x] **Coexistence — decided.** During shadow: bootstrap auth-callout JWT remains authoritative for NATS subject ACL; new claims only inform CEL and audit. ADR 0003 retires the transitional hybrid once enforce is live.
- [ ] **Backfill implementation.** Inventory current callers; assign `agent_id`; register in the new registry; provision SVIDs (file-based for legacy, SPIRE for service workloads).
- [x] **Cutover criteria — decided.** Zero shadow-mode `aud_mismatch` / `agent_id_missing` / `act_chain_depth_overflow` events for 7 consecutive days at production traffic; STS P99 < 40 ms over the same window; rollback runbook reviewed (set `MCP_GATEWAY_AGENT_IDENTITY=shadow`, drain mesh-token cache).

**Acceptance:** A staged rollout plan with named owners, dates, and a written rollback.

---

## Block 7 — Documentation deliverables

- [x] **ADRs from Block 0 — Accepted.** Six documents at `docs/adr/0001-…0006-…`.
- [x] `docs/identity/act-chain.md` — Accepted contract.
- [x] `docs/identity/registry.md` — Accepted contract.
- [x] `docs/identity/sdk.md` — Accepted contract.
- [x] `docs/identity/jwt-claim-schema.md` — Accepted contract.
- [ ] `docs/identity/overview.md` — single-page diagram of agent identity, mesh tokens, chain (item 4 of roadmap).
- [ ] `docs/identity/sts-exchange.md` — full request/response, examples, error codes (item 4 of roadmap).
- [ ] Update `MCP_GATEWAY_PLAN.md` § Wire-Format Pins with `mcp-act-chain` header row and new claims (item 3 of roadmap).
- [ ] Update `MCP_GATEWAY_PLAN.md` § CEL variable namespace with `jwt.agent_id`, `jwt.act_chain`, `jwt.purpose`, etc. (item 3 of roadmap).
- [ ] Update `MCP_GATEWAY_PLAN.md` § Audit envelope schema to document the optional `act_chain` field (item 3 of roadmap).

---

## Operational follow-ups (decide as the roadmap reaches them — do not block on them)

These are the residual open questions from the six ADRs. Each has a working default; revisit once the relevant block lands and real data is available. None of them block execution.

- Gateway's own identity: SPIFFE ID + privileged role today; promote to a registered `agent_id` if/when policy needs to gate on it.
- Batch / non-interactive originators in `act_chain`: use sentinel `wkl:sentinel:batch` with `auth_method: "service-account"` as originator entry.
- Maximum `act_chain` depth: 8 (configurable per bundle). Revisit if real workflows hit the cap.
- `purpose` refinement audit: STS records before/after `purpose` on every exchange in `mcp.audit.sts.{outcome}`.
- STS exposure to user clients: mesh-internal only in v1; HTTP facade per ADR 0004 lands when a user-facing client needs it.
- Offline dev without SPIRE: file-based SVIDs + `sentinel:human` fallback (ADR 0002 + ADR 0006 dev profile).
- `trogon-decider` placement: STS stateless in v1 with JetStream audit; reconsider event-sourced STS if replay/consistency proves valuable (ADR 0004 open question 2).
- Multi-region: regional STS instances with local registry cache, default. Trust-bundle replication SLA TBD with first multi-region deployment (ADR 0006 §6).
- OAuth-MCP composition (`MCP_GATEWAY_PLAN.md:67`): OAuth access token enters as `subject_token` to STS — same TTL / `aud` rules apply (ADR 0005 §OQ6).

---

## Driving to completion

Authoritative ordering lives in the **Execution roadmap** at the top of this file. Items 1–3 (enforce-mode hardening, CEL `act_chain` surface, `MCP_GATEWAY_PLAN.md` wire pins) are immediate and independent — fan out. Items 4–7 (overview/STS-exchange docs, registry v1, JWKS publisher, `trogon-sts` v1) are the critical path. Items 8–12 (gateway egress minting, A2A SDK, SPIRE, traffic view, adaptive access) build on top.

Decisions are settled. Build.

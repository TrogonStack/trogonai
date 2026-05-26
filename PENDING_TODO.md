# PENDING_TODO — Agent Identity Gaps vs. Uber's "Agent Identity Crisis"

Reference: https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/
Companion to: `MCP_GATEWAY_PLAN.md`

This file enumerates the work *not yet in the plan* (or only implicit) to bring the TrogonAI MCP gateway / A2A surface up to the architecture Uber describes. Items are grouped by track. Each item lists: **decisions to make (paper)**, **wire/spec deliverables**, **code deliverables**, and **acceptance signal**.

Nothing here is "start coding" — Blocks 0–2 are decision-making and spec work that gate the code.

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

### Practical conclusion

Treat the existing auth-callout JWT as the **bootstrap credential** until Block 0 decides otherwise. The next durable work is not more gateway forwarding; it is pinning the identity model and wire contracts so every later claim, subject, audit envelope, and SDK method has a stable target.

---

## Block 0 — Cross-cutting decisions that gate everything else

These decisions change the shape of every claim, subject, and audit envelope downstream. Resolve before writing any new code.

- [ ] **Tenancy model** (already open as Block A in `MCP_GATEWAY_PLAN.md:52`).
  - Options: NATS account per tenant (hard) vs. tenant-as-JWT-claim (soft) vs. hybrid.
  - Blocks: STS audience naming, `act_chain` shape, agent registry namespacing, audit retention scoping, SpiceDB principal naming.
  - Output: ADR `docs/adr/0001-tenancy-model.md`.

- [ ] **Identity layering: workload vs. agent vs. user.**
  - Decide the canonical triplet shape: `workload_id` (SPIFFE), `agent_id` (registry), `user_sub` (originator).
  - Decide whether `agent_id` is mandatory on *every* JWT in the mesh, or only for agent-originated calls.
  - Decide how non-agent callers (humans via UI, batch jobs) are represented in the same chain.
  - Output: ADR `docs/adr/0002-identity-layers.md`.

- [ ] **Bootstrap vs. mesh tokens.**
  - Decide: is the NATS auth-callout JWT the *only* token (current plan), or is it a *bootstrap* token that must be exchanged at STS before any cross-agent call?
  - This is the single most consequential decision in this document. It determines whether per-hop tokens (Block 2) are additive or replace existing flow.
  - Output: ADR `docs/adr/0003-bootstrap-vs-mesh-tokens.md`.

- [ ] **STS form factor.**
  - Options: (a) NATS service on `mcp.sts.exchange`; (b) HTTP service; (c) library embedded in the gateway; (d) external IdP add-on.
  - Trade-offs: latency budget (Uber's P99 target: 40 ms), key custody, blast radius on outage, multi-region story.
  - Output: ADR `docs/adr/0004-sts-form-factor.md`.

- [ ] **TTL and `aud` discipline.**
  - Pick concrete numbers: mesh-token TTL (target: 60–300 s), clock-skew tolerance, refresh strategy, `aud` granularity (per-backend? per-tool? per-target-subject prefix?).
  - Decide whether to enforce `aud == this_gateway`/`aud == this_backend` in CEL as a *hard* gate, not a *soft* check.
  - Output: ADR `docs/adr/0005-token-ttl-and-audience.md`.

- [ ] **Key material custody.**
  - Decide signing-key location for mesh tokens: NATS Operator keystore, KMS (cloud), HashiCorp Vault, file-based.
  - Decide rotation cadence and JWKS publication subject/URL (the gateway currently only *validates*; minting is new).
  - Output: ADR `docs/adr/0006-mesh-token-signing-keys.md`.

---

## Block 1 — Agent identity primitives

Today the gateway sees `jwt.sub`, `jwt.tenant`, `jwt.roles` and nothing distinguishes an agent from the workload running it. Uber's Agent Registry exists to bridge this gap.

### 1.1 Agent registry

- [ ] **Define the agent entity.**
  - Fields: `agent_id`, `agent_version`, `agent_definition_digest`, `owner_team`, `allowed_workloads` (SPIFFE IDs), `allowed_tools` (coarse capability list), `metadata`.
  - Decide: is `agent_id` opaque (UUID) or namespaced (`acme/oncall-agent`)?
- [ ] **Storage.**
  - Decide: NATS KV bucket `mcp-agent-registry` vs. external (Postgres/Git-as-source-of-truth).
  - Recommendation: KV for runtime cache; Git/manifest as source of truth, synced via control plane.
- [ ] **Lifecycle.**
  - Registration, version bump, deprecation, revocation. Who can write to the registry? (Probably control-plane only, via signed manifests.)
- [ ] **Lookup API.**
  - Subject: `mcp.registry.agent.lookup` (request/reply).
  - Return: full agent record + current allowed workloads, or NACK.
- [ ] **Audit hook.**
  - Every registry mutation emits to `mcp.audit.registry.*`.

**Acceptance:** A workload (SPIFFE ID) cannot present `agent_id=X` to the STS unless the registry confirms it is in `allowed_workloads` for `X`.

### 1.2 Workload attestation (SPIFFE / SPIRE)

- [ ] **Decide attestation source.**
  - Options: SPIRE server (heavyweight, k8s-native), file-based SVIDs (dev), mTLS cert with custom SAN (lightest), cloud IMDS-based (AWS/GCP).
  - Recommendation: SPIRE for prod, file-based fallback for dev.
- [ ] **Define the trust handshake.**
  - At STS: client presents SVID (mTLS or as a JWT-SVID in the request); STS validates against trust bundle.
  - Trust-bundle distribution: NATS KV `mcp-trust-bundles`, refreshed every N minutes.
- [ ] **SVID → workload claim mapping.**
  - SPIFFE ID `spiffe://acme.local/ns/prod/sa/oncall-agent` → JWT claim `wkl` (workload-id) + `wkl_attested_at`.
- [ ] **Document fallback for non-SPIFFE callers.**
  - Humans via UI, API-key callers, etc. — what fills `wkl`? Probably a sentinel like `wkl: "human"` plus `auth_method: "oidc"`.

**Acceptance:** STS refuses to mint a mesh token without a valid SVID (or explicit non-SPIFFE auth-method in a documented allow-list).

### 1.3 JWT claim schema additions

- [ ] **New claims to standardize (paper first, then wire-format pin in `MCP_GATEWAY_PLAN.md` § Wire-Format Pins):**
  - `agent_id` (string, optional for non-agent callers).
  - `agent_version` (string).
  - `wkl` (SPIFFE ID string).
  - `wkl_attested_at` (timestamp).
  - `act_chain` (array, see Block 2.2).
  - `purpose` / `intent` (free-form string, see Block 2.3).
  - `session_id` (already implicit; lift to claim).
- [ ] **Update CEL variable namespace** (`MCP_GATEWAY_PLAN.md:941`) to expose `jwt.agent_id`, `jwt.wkl`, `jwt.act_chain`, `jwt.purpose`.
- [ ] **Update ingress-hardening rule** (`MCP_GATEWAY_PLAN.md:835`) to also strip/refuse client-supplied `act_chain`, `agent_id`, `wkl`.

**Acceptance:** A JWT without `wkl` is rejected by the gateway in `require` mode. CEL rules can reference `jwt.agent_id` in policy bundles.

---

## Block 2 — Per-hop token exchange and delegation

This is the largest piece of net-new work. Today: one connect-time JWT covering broad subject ACL. Target: short-lived, audience-scoped JWT per hop with `act_chain` lineage.

### 2.1 Security Token Service (STS)

- [ ] **Spec the exchange contract** (loosely RFC 8693).
  - Inputs: `subject_token` (caller's current JWT), `subject_token_type`, `audience` (target hop), `scope` (optional, tool-name list), `requested_token_type` (default `urn:ietf:params:oauth:token-type:jwt`), `purpose` (string), `actor_token` (the caller's workload SVID).
  - Outputs: `access_token`, `issued_token_type`, `expires_in`.
- [ ] **Spec the validation pipeline inside STS.**
  - Verify subject_token signature against trusted issuer set.
  - Verify SVID against trust bundle.
  - Look up `agent_id` in registry; verify `wkl ∈ allowed_workloads`.
  - Check the `aud` request is permitted (registry: `allowed_audiences`? bundle: per-agent ACL?).
  - Append caller to `act_chain`; cap length (e.g., 8) to prevent runaway delegation.
  - Mint new JWT with reduced scope, narrowed `aud`, fresh `iat`/`exp`.
- [ ] **Decide subject vs. HTTP.**
  - If NATS: `mcp.sts.exchange` queue-group consumer; reply inbox per request.
  - If HTTP: standalone service at `sts.<env>.<domain>` behind mTLS.
- [ ] **Latency budget.**
  - Target P99 < 40 ms (Uber's published number). Cache trust bundle and registry hot path in memory; refresh async.
- [ ] **Rate limiting + abuse controls.**
  - Per-`wkl` exchange rate; per-`agent_id` rate; circuit breaker on registry/SpiceDB outages.
- [ ] **Failure modes.**
  - STS down → mesh stops. Decide: fail-closed (correct) vs. degraded-mode escape hatch (dangerous). Document.
- [ ] **Audit.**
  - Every exchange (success and failure) emits to `mcp.audit.sts.{outcome}` with the *full* inputs and the minted token's claims (not the signature).

**Acceptance:** Calling backend B from agent A requires A to exchange its token for one with `aud=B`, `exp ≤ now+TTL`, and `act_chain` ending in A. The gateway rejects any token where `aud` doesn't match its own identity.

### 2.2 Actor chain (`act_chain`)

- [ ] **Schema.**
  - Array of objects, each: `{ "sub": "...", "agent_id": "...", "wkl": "...", "iat": <ts> }`.
  - Order: oldest (originating user) first, current actor last.
  - Decide: signed-per-entry (each STS that appends signs that entry) vs. integrity-by-outer-JWT-only (simpler; trusts each STS).
- [ ] **Propagation rules.**
  - On every STS exchange: new entry appended, never rewritten.
  - On audit: full chain serialized into envelope (`MCP_GATEWAY_PLAN.md:784`).
- [ ] **Verification rules.**
  - Each entry's `sub`/`agent_id` must resolve in the registry at the time of receipt (or have been valid at `iat` — pick one).
  - Maximum depth (default 8). Configurable per bundle.
  - Loop detection: same `(agent_id, wkl)` appearing twice → reject.
- [ ] **CEL surface.**
  - `jwt.act_chain` as `list<map>`.
  - Helpers: `chain.contains(agent_id)`, `chain.originator()`, `chain.depth()`.
- [ ] **Header projection.**
  - Add `mcp-act-chain` (compact JSON, gateway-set, ingress-stripped) to the table at `MCP_GATEWAY_PLAN.md:829`.

**Acceptance:** A four-hop call (`user → A → B → C → backend`) produces a chain of length 4 at the backend, visible in audit, and rejected if any link is missing.

### 2.3 Intent / purpose claim

- [ ] **Decide whether `purpose` is free-form or enumerated.**
  - Enumerated is harder to game but harder to extend. Recommendation: enumerated set per agent in the registry (`allowed_purposes`).
- [ ] **Propagation.**
  - Originating client sets initial `purpose`; subsequent exchanges either preserve or refine it (refining requires `purpose ∈ allowed_purposes` for the refining agent).
- [ ] **Surface in CEL** (`jwt.purpose`) and in audit envelope.

**Acceptance:** SpiceDB policy can gate on `(agent_id, purpose, resource)` triples instead of just `(subject, resource)`.

### 2.4 Gateway egress: mint, don't propagate

- [ ] **Replace "propagate verified workload context" with "mint downstream token."**
  - Current commit `49dd9e7dc` propagates context. Target: gateway *calls STS* before egress to backend, attaches the new token, drops the inbound one.
- [ ] **Cache exchanges** by `(caller_sub, target_aud, session_id, scope)` keyed up to TTL/2 to stay within latency budget.
- [ ] **Decide egress on callbacks** (server → client direction, `MCP_GATEWAY_PLAN.md:446`): is the callback token also exchanged? Probably yes, with `aud=client_id`.

**Acceptance:** Backend logs show `aud=<backend>`, `iss=<sts>`, `act_chain` includes the gateway as the last hop before the backend.

---

## Block 3 — A2A client SDK

Uber's "Standardized A2A Client" is the thing that makes the secure path the only path developers reach for. Today there's a `yordis/feat-a2a-nats` branch but no documented SDK contract.

- [ ] **Spec the client contract.**
  - Constructor takes: own SVID source, STS endpoint, registry endpoint (optional, for self-introspection), default audience policy.
  - Method `call(target_agent, payload, purpose)` does: lookup target → exchange token (aud=target) → send → receive → return.
  - Method `serve(handler)` does: verify inbound token → verify `act_chain` → invoke handler with a typed `Caller` struct exposing `originator`, `chain`, `purpose`.
- [ ] **Languages.**
  - Decide priority: Rust first (matches gateway crates), then TypeScript (matches `tsworkspace/`), then Python (agent ecosystem).
- [ ] **Telemetry.**
  - Client emits OpenTelemetry spans with `agent.id`, `agent.chain.depth`, `agent.purpose` attributes on every call.
- [ ] **"Don't roll your own" enforcement.**
  - CI lint that flags direct NATS publishes to `mcp.gateway.request.*` from agent code that isn't the SDK.
- [ ] **Docs.**
  - Quickstart, recipe for adding a new agent, recipe for verifying chain in a handler.

**Acceptance:** A new agent can be written in ≤ 50 lines and gets correct identity propagation for free.

---

## Block 4 — Observability for agentic traffic

You have the audit stream (`MCP_GATEWAY_PLAN.md:269`, `:784`). Missing: the agent-centric *view*.

- [ ] **Define the agent-traffic schema** on top of audit envelopes.
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

Not on the current roadmap. CEL + SpiceDB are the right substrate, but the *policies* and *flows* are missing.

- [ ] **Step-up auth.**
  - Decide signal source: tool-level annotation (`sensitive: true`), CEL rule, SpiceDB relation.
  - Decide step-up channel: re-auth via OIDC, hardware key, second-factor through `trogon-gateway` (Slack approval).
- [ ] **Human-in-the-loop approvals.**
  - New JSON-RPC error code (`-32107 approval_required`) with `approval_url` / `approval_subject`.
  - Approval responses land on `mcp.approvals.{request_id}`; gateway resumes the request.
  - TTL on pending approvals; default-deny on expiry.
- [ ] **Context-aware throttling.**
  - Rate limits keyed by `(agent_id, purpose, tenant)` instead of just `jwt.sub` (`MCP_GATEWAY_PLAN.md:959`).
- [ ] **Anomaly hooks.**
  - Emit features to a side stream for downstream anomaly scoring (out of scope to build; in scope to *expose*).

**Acceptance:** A tool can be flagged `sensitive`; calls to it require explicit human approval; the approval is audited with the same `act_chain`.

---

## Block 6 — Migration and compatibility

The current plan ships Phase 0 (auth-callout + subject ACL) without any of the above. Migration must be incremental.

- [ ] **Feature flags.**
  - `MCP_GATEWAY_AGENT_IDENTITY` — off / shadow / enforce (mirrors the existing `MCP_GATEWAY_JWT_*` phasing in `MCP_GATEWAY_PLAN.md:80`).
- [ ] **Shadow mode.**
  - Validate `act_chain` and `aud` but only log violations; don't reject.
- [ ] **Coexistence with existing JWT path.**
  - During shadow: existing connect-time JWT still authoritative for subject ACL; new claims only inform CEL and audit.
- [ ] **Backfill plan for existing agents.**
  - Inventory of current callers; assign `agent_id`; register in the new registry; emit SVIDs.
- [ ] **Cutover criteria.**
  - Zero shadow-mode violations for N days; STS P99 within budget; rollback runbook reviewed.

**Acceptance:** A staged rollout plan with named owners, dates, and a written rollback.

---

## Block 7 — Documentation deliverables (paper, before code)

- [ ] ADRs from Block 0 (six documents).
- [ ] `docs/identity/overview.md` — single-page diagram of agent identity, mesh tokens, chain.
- [ ] `docs/identity/sts-exchange.md` — full request/response, examples, error codes.
- [ ] `docs/identity/act-chain.md` — schema, verification rules, examples.
- [ ] `docs/identity/registry.md` — entity model, lifecycle, write path.
- [ ] `docs/identity/sdk.md` — A2A client quickstart.
- [ ] Update `MCP_GATEWAY_PLAN.md` § Wire-Format Pins with new headers and claims.
- [ ] Update `MCP_GATEWAY_PLAN.md` § CEL variable namespace with `jwt.agent_id`, `jwt.act_chain`, etc.
- [ ] Update `MCP_GATEWAY_PLAN.md` § Audit envelope schema to embed `act_chain`.

---

## Open questions to resolve during decision-making

- [ ] Should the gateway *itself* have an `agent_id`, or is it identified purely by its workload SPIFFE ID and a privileged role?
- [ ] How are *batch* / non-interactive jobs represented in `act_chain` (no human originator)?
- [ ] Does `act_chain` need per-entry signatures, or is outer-JWT signature sufficient? (Affects size and STS cost.)
- [ ] Maximum `act_chain` depth — is 8 right? Higher allows real workflows; lower bounds blast radius.
- [ ] When `purpose` is refined down-chain, how is the change audited and bounded?
- [ ] Do we expose STS exchange to *user* clients, or only mesh-internal services? (If user clients call STS, the UI needs to know how.)
- [ ] How do we test all of this offline (dev) without standing up SPIRE?
- [ ] Where does `trogon-decider` fit? Is the STS itself a decider? Is the registry an event-sourced aggregate?
- [ ] Multi-region: is STS regional or global? What's the trust-bundle replication SLA?
- [ ] How does this compose with the upcoming OAuth-MCP work (`MCP_GATEWAY_PLAN.md:67`)?

---

## Priority recommendation (from gap analysis)

Cheapest wins that unlock the most downstream work:

1. **Block 0** (decisions) — nothing else moves without these.
2. **Block 1.3** (claim schema) + **Block 2.2** (`act_chain`) — additive, observable in shadow, no breakage.
3. **Block 3** (A2A SDK contract) — write the contract before STS; STS exists to serve the SDK.
4. **Block 1.1** (registry, minimal) — needed before STS can do meaningful validation.
5. **Block 2.1** (STS) — the largest piece; everything before this de-risks it.
6. **Block 1.2** (SPIRE) — can ship STS with a documented "trust-anchor TBD" first, harden later.
7. **Block 4** (observability) — value compounds once chains exist.
8. **Block 5** (adaptive access) — last, because it depends on every prior block.

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

Three swarm runs merged into `yordis/agentgateway`:

- `20260527T055911Z-6d70` — shadow-mode parsing, audit envelope, identity specs (26 commits).
- `20260527T074306Z-7416` — enforce-mode hardening, CEL `act_chain` surface, plan wire pins, overview/STS-exchange docs, and four new crates: `trogon-agent-registry`, `trogon-jwks-publisher`, `trogon-sts` (+ shared `trogon-identity-types`), `trogon-a2a-sdk` (21 commits).
- `20260527T193237Z-1271` — production hardening across seven branches: registry controller + `agctl`, AWS KMS + Vault Transit signers for both JWKS publisher and STS, STS circuit breaker + latency probe + chain resolution, SDK live integration tests + serve-side OTel + CI lint, gateway egress STS minting + LRU cache, `trogon-traffic-view` spec + skeleton, and `ActChainEntry` unification.

Workspace status: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all green.

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
- **Operator-facing reference docs.** `docs/identity/overview.md` (architecture, mesh-token model, 4-hop chain diagram) and `docs/identity/sts-exchange.md` (NATS wire contract, request/response schemas, error codes, audit envelope).
- **Plan wire pins.** `MCP_GATEWAY_PLAN.md` § Wire-Format Pins, § JSON-RPC errors (new `-32109 audience_mismatch`), § Audit envelope, and § CEL namespace all updated for the agent-identity claims and `mcp-act-chain` header.
- **Shadow + enforce claim parsing.** All optional JWT claim fields parsed in `trogon-mcp-gateway` (`agent_id`, `agent_version`, `wkl`, `wkl_attested_at`, `purpose`, `session_id`, `act_chain`, `auth_method`). Exposed in CEL as `jwt.agent_id`, `jwt.agent_version`, `jwt.wkl`, `jwt.purpose`, `jwt.session_id`, `jwt.act_chain`, plus helpers `chain.contains(agent_id)`, `chain.originator()`, `chain.depth()`.
- **Enforce-mode hardening.** `MCP_GATEWAY_AGENT_IDENTITY=enforce` is no longer a log-only stub. Hard rejects (before SpiceDB) on: missing `wkl` when `auth_method=svid|spire`, SPIFFE `wkl` without `agent_id`, `aud` mismatch, `act_chain` depth > 8, duplicate `(agent_id, wkl)`. Untrusted issuers have `agent_id`/`wkl`/`auth_method`/`act_chain` stripped before policy evaluation. New JSON-RPC error codes `-32109 audience_mismatch`, `-32113`/`-32114` act-chain failures, `-32117 agent_identity_required`, `-32118 auth_required`.
- **Audit envelope extended.** Optional identity fields on the audit envelope (`agent_id`, `agent_version`, `wkl`, `purpose`, `session_id`, `act_chain`), all `skip_serializing_if = "Option::is_none"` so the existing wire format is byte-identical when nothing is set.
- **`trogon-agent-registry` v1 (runtime lookup).** NATS KV bucket `mcp-agent-registry`, queue-group consumer on `mcp.registry.agent.lookup` with cache-first reads, audit events on `mcp.audit.registry.{lookup.*,put,delete}`. Sample manifest under `examples/`.
- **`trogon-jwks-publisher` v1 (file PEM).** RSA + Ed25519 file PEM key source, publishes mesh JWKS to KV `mcp-jwks/mesh/current`, HTTPS `/.well-known/jwks.json`, and request/reply `mcp.jwks.mesh.get`.
- **`trogon-sts` v1 (file PEM signer).** Queue-group consumer on `mcp.sts.exchange`. Verifies bootstrap/mesh `subject_token` (separate JWKS caches), validates `actor_token` against a file trust bundle, looks up registry, appends `act_chain`, mints mesh JWT (TTL 120 s default, range 60–300 s via `mesh_token_ttl_s`). Rate limits 100/10 s/`wkl` + 500/10 s/`agent_id`, audit emit to `mcp.audit.sts.{outcome}`.
- **`trogon-a2a-sdk` Rust skeleton.** Traits for `SvidSource`, `Sts`, `Registry`, `MessageTransport`, `Jwks`. NATS impls under `nats` feature. `Client::call` does lookup → exchange → publish; `serve` verifies JWT + `aud` + chain depth/loops and yields a typed `Caller { originator, chain, purpose }`. Mock-based unit tests pass; 33-line `examples/echo_agent.rs`.
- **Shared `trogon-identity-types`.** Extracted `ActChainEntry` + `MAX_ACT_CHAIN_DEPTH`; used by `trogon-sts`, `trogon-mcp-gateway`, and `trogon-a2a-sdk` (unification closed by `20260527T193237Z-1271`).
- **Production signers + resilience.** `trogon-jwks-publisher` and `trogon-sts` both ship AWS KMS (`kms-aws`) and Vault Transit (`vault`) signers; `trogon-sts` adds a circuit breaker on registry / SpiceDB, act-chain registry resolution with TTL cache, and the `trogon-sts-probe` latency sidecar.
- **Registry control plane.** `trogon-agent-registry-controller` + `agctl` (signer, Git→KV sync, TOML manifest validation, lifecycle audit events); runbook at `docs/identity/registry-operations.md`.
- **Gateway egress minting.** `trogon-mcp-gateway` calls `trogon-sts-client` before backend egress, attaches the minted token, drops the inbound credential, with an LRU cache keyed per ADR 0005 and mode-gated by `MCP_GATEWAY_AGENT_IDENTITY`.
- **A2A SDK hardening.** Live NATS+STS+registry integration test, serve-side OTel attributes, registry-driven default purpose, CI lint forbidding non-SDK publishes to `mcp.gateway.request.*`.
- **`trogon-traffic-view` skeleton + spec.** Crate skeleton (`AuditConsumer` / `TrafficIndex` / `SiemExporter` traits) and full spec at `docs/identity/agent-traffic.md` (OCSF v1 SIEM export, `agctl traffic` CLI surface).

### Execution roadmap (drives to completion, no gates)

Ordered for forward motion. Items 1–4 landed clean. Items 5–7 landed the dev-profile of the service; the **production-profile work to finish each one** is enumerated under "Carry-over from `20260527T074306Z-7416`" below — those items must close before agent-identity v1 ships.

1. **Finish ingress + enforce.** ✅ Closed. `MCP_GATEWAY_AGENT_IDENTITY=enforce` hard-rejects on missing `wkl`/`agent_id`, `aud` mismatch, depth/loop violations; untrusted issuers have agent-identity claims stripped before policy.
2. **Expose `act_chain` in CEL.** ✅ Closed. `jwt.act_chain` plus `chain.contains` / `chain.originator` / `chain.depth` helpers wired into `policy.rs`.
3. **Update `MCP_GATEWAY_PLAN.md`.** ✅ Closed. Header table, CEL namespace, audit envelope, and JSON-RPC error sections all updated.
4. **Identity overview + STS-exchange docs.** ✅ Closed. `docs/identity/overview.md` and `docs/identity/sts-exchange.md`.
5. **Agent registry.** Dev profile ✅ (runtime lookup, KV, audit hooks). Production-profile remaining work in **Carry-over §Registry control plane**.
6. **JWKS publisher.** Dev profile ✅ (file PEM, all three publication channels). Production-profile remaining work in **Carry-over §JWKS production custody**.
7. **`trogon-sts` v1.** Dev profile ✅ (exchange, registry, act_chain, caches, rate limits, audit). Production-profile remaining work in **Carry-over §STS production signer** and **§STS resilience**.
8. **Gateway egress minting.** ✅ Closed. `trogon-mcp-gateway` calls `trogon-sts-client` before backend egress, attaches the minted token, drops the inbound credential. LRU cache keyed per ADR 0005; callback direction uses `aud=urn:trogon:mcp:client:{tenant}:{client_id}`. Mode-gated by `MCP_GATEWAY_AGENT_IDENTITY`.
9. **A2A SDK v1 (Rust).** Skeleton ✅. Remaining work in **Carry-over §A2A SDK gaps**. Then TypeScript, then Python.
10. **SPIRE wiring (production attestation).** Replace sentinel-`wkl` fallback for service workloads with real SVID attestation at STS ingress; trust-bundle distribution via NATS KV `mcp-trust-bundles`. Spec details live with Block 1.2 below.
11. **Agent-traffic view.** Build the index on top of the already-populated audit envelope: timeline, chain explorer, top-N dashboards, SIEM export (Block 4). Pick CEF/OCSF format and stand up the JetStream durable consumer.
12. **Adaptive access.** Step-up auth, human-in-the-loop approvals (new error `-32107 approval_required`, approval responses on `mcp.approvals.{request_id}`), context-aware rate limits keyed by `(agent_id, purpose, tenant)`, anomaly feature emission (Block 5).

### Carry-over from `20260527T074306Z-7416` — closed by `20260527T193237Z-1271`

The 2026-05-27 production-hardening swarm landed seven branches into `yordis/agentgateway` (see `.trogonai/agent-handoff/20260527T193237Z-1271/`). Every bullet below now ships; remaining production-readiness work is enumerated in the new "Open after 2026-05-27 swarm" section.

**Registry control plane (closes roadmap item 5).**
- [x] `trogon-agent-registry-controller` landed: Ed25519 signer + Git sync loop reads signed TOML manifests, validates owner / version monotonicity / definition digest, writes to KV `mcp-agent-registry` under the signer identity.
- [x] KV write ACL test for signer-only writes ships alongside the controller; non-signer writes are rejected.
- [x] Signed-manifest schema pinned as TOML in `docs/identity/registry.md`; `agctl registry sync` validates locally before commit.
- [x] Lifecycle events on `mcp.audit.registry.{registered,bumped,deprecated,revoked}` published by the controller.
- [x] Operator runbook at `docs/identity/registry-operations.md` (bootstrap, signer rotation, KV corruption recovery, emergency revocation).

**JWKS production custody (closes roadmap item 6).**
- [x] AWS KMS picked as the v1 cloud KMS (RSA_2048, PSS_SHA_256), pinned in ADR 0006 §Implementation; `KmsSigner` implemented behind the `kms-aws` feature.
- [x] Vault Transit `VaultSigner` implemented behind the `vault` feature.
- [x] End-to-end rotation test at `crates/trogon-jwks-publisher/tests/rotation_e2e.rs` validates KV / HTTPS / req-reply convergence and retired-key verification.
- [x] KMS and Vault recipes documented in `crates/trogon-jwks-publisher/README.md`.

**STS production signer (mirrors JWKS custody; closes roadmap item 7).**
- [x] AWS KMS `KmsSigner` implemented in `trogon-sts` behind `kms-aws`.
- [x] Vault Transit `VaultSigner` implemented behind `vault`.
- [x] Integration test asserts identical `kid` / `iss` between STS and JWKS publisher for the active key.

**STS resilience (closes roadmap item 7).**
- [x] Circuit breaker (closed / open / half-open, configurable cool-down) wraps registry and SpiceDB calls inside `trogon-sts`; trips fail-closed with `dependency_unavailable`.
- [x] Synthetic-exchange latency probe ships as the `trogon-sts-probe` binary; emits to `mcp.metrics.sts.latency` and `mcp.audit.sts.latency_violation` on P99 > 40 ms.
- [x] Failure-modes section in `docs/identity/sts-exchange.md` enumerates registry / SpiceDB / JWKS / signer / chain-entry-revoked triggers with audit reasons, JSON-RPC codes, and recovery actions.

**Act-chain registry resolution (closes Block 2.2 acceptance).**
- [x] STS receipt walks every entry in the inbound `act_chain`, resolves `agent_id` against the registry, and rejects revoked / unknown entries (`mcp.audit.sts.deny` with `reason: "act_chain_entry_revoked"`, `data.offending_index`, offending `agent_id`).
- [x] TTL-bounded chain-resolution cache (default 60 s) keeps the additional latency under budget; `STS_CHAIN_RESOLUTION_MODE=off|cache|strict` env flag exposes the dial.
- [ ] Gateway-ingress-side chain resolution still pending (STS-side is the higher-leverage gate). Track under "Open after 2026-05-27 swarm".

**Purpose handling at the originator (closes Block 2.3 acceptance).**
- [x] `trogon-a2a-sdk` `Client::call` resolves the caller's `allowed_purposes` from the registry when `purpose=None`: single-allowed → use it, multiple → `Error::PurposeRequired`, empty → synthesize `purpose:default`. Pinned in `docs/identity/sdk.md` §Default purpose handling.
- [ ] STS-side empty-purpose rejection still tracked separately. Move to "Open after 2026-05-27 swarm".

**A2A SDK gaps (closes roadmap item 9 — Rust track).**
- [x] Live integration test (`crates/trogon-a2a-sdk/tests/integration_live.rs`, behind `--features live`) boots NATS + STS + registry in-process and exercises `Client::call` + `serve` end-to-end.
- [x] `agent.id` / `agent.chain.depth` / `agent.purpose` attributes added to serve-side OTel spans.
- [x] CI lint at `scripts/ci/check-direct-nats-publishes.sh` fails the build on non-SDK publishes to `mcp.gateway.request.*`; wired into `.github/workflows/ci-rust.yml`.
- [x] Quickstart + recipes in `crates/trogon-a2a-sdk/README.md` and `docs/identity/sdk.md` (default purpose, telemetry attributes).

**Crate-type unification.**
- [x] `trogon-mcp-gateway::act_chain` re-exports `ActChainEntry` / `MAX_ACT_CHAIN_DEPTH` / `parse_act_chain` from `trogon-identity-types`; gateway-local definition removed.
- [x] `trogon-a2a-sdk` drops its local `ActChainEntry`; depends on `trogon-identity-types` directly.
- [x] `static_assertions::assert_type_eq_all!` test in `trogon-mcp-gateway` asserts the gateway-exposed type IS the shared type; SDK has the same guard.

### Open after 2026-05-27 swarm

- [ ] Agent-traffic view v1 implementation (spec + skeleton landed as `trogon-traffic-view`; `docs/identity/agent-traffic.md` covers schema, projector, OCSF export, `agctl traffic` CLI surface).
- [x] Gateway-ingress-side act-chain registry resolution (STS receipt resolution already landed).
- [x] STS-side empty-purpose rejection (`mcp.audit.sts.deny` reason `purpose_missing`).
- [ ] SPIRE wiring (Block 1.2 trust handshake + SVID → `wkl` mapping at STS).
- [ ] Adaptive access (Block 5: step-up auth, human-in-the-loop approvals with `-32107 approval_required`, anomaly emission).
- [ ] A2A SDK TypeScript + Python tracks.

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
- [x] **Lifecycle implementation.** `trogon-agent-registry-controller` ships the Ed25519 signer + Git sync loop (registration, version bump, deprecation, revocation via signed TOML manifests); KV writes restricted to the signer identity.
- [x] **Lookup API implementation.** `mcp.registry.agent.lookup` queue-group consumer landed in `trogon-agent-registry`; returns the full record (or `not_found` / `revoked` reply) with cache-first reads against KV.
- [x] **Audit hook implementation.** `mcp.audit.registry.{lookup.*,put,delete}` events emitted on every mutation and lookup outcome.

**Acceptance:** A workload (SPIFFE ID) cannot present `agent_id=X` to the STS unless the registry confirms it is in `allowed_workloads` for `X`. Enforced by `trogon-sts` exchange pipeline as of this snapshot.

### 1.2 Workload attestation (SPIFFE / SPIRE)

Attestation model is fixed by ADR 0002 (sentinel `wkl` namespace for non-SPIFFE, SPIFFE URI for service workloads). Implementation tasks:

- [x] **Attestation source — decided.** SPIRE server for production; file-based SVIDs for dev/CI. Cloud IMDS-based attestation is an optional add-on after SPIRE is in.
- [ ] **Trust handshake implementation.** At STS: client presents SVID (mTLS or JWT-SVID in the exchange request); STS validates against trust bundle. Trust-bundle distribution via NATS KV `mcp-trust-bundles`, refreshed every N minutes.
- [ ] **SVID → `wkl` mapping implementation.** Map SPIFFE ID (e.g. `spiffe://acme.local/ns/prod/sa/oncall-agent`) to JWT claim `wkl` + `wkl_attested_at` at STS mint time.
- [x] **Non-SPIFFE fallback — decided.** Sentinel `wkl` values: `sentinel:human` (+ `auth_method: "oidc"` / `"mtls"` / `"api-key"`), `sentinel:batch`, `sentinel:api-key`. See ADR 0002.

**Acceptance:** STS refuses to mint a mesh token without a valid SVID (or explicit non-SPIFFE auth-method in a documented allow-list).

### 1.3 JWT claim schema additions

Spec at `docs/identity/jwt-claim-schema.md` is the accepted contract. Shadow-mode parsing landed (commit `d5754a40b`); remaining gaps below.

- [x] **New claims standardized.** Parsing implemented for all required claims; `act_chain` lives on `mcp-act-chain` header per ADR 0002 / `act-chain.md`.
  - `agent_id` (string, optional for non-agent callers). Parsed.
  - `agent_version` (string). Parsed.
  - `wkl` (SPIFFE ID string). Parsed (unverified — no SPIRE yet).
  - `wkl_attested_at` (timestamp). Parsed.
  - `act_chain` (array, see Block 2.2). Lives on the `mcp-act-chain` header, not on the JWT; see Block 2.2.
  - `purpose` / `intent` (free-form string, see Block 2.3). Parsed.
  - `session_id` (already implicit; lift to claim). Parsed.
  - `auth_method` (string). Parsed.
- [x] **Update CEL variable namespace** (`MCP_GATEWAY_PLAN.md:941`). All required fields exposed: `jwt.agent_id`, `jwt.agent_version`, `jwt.wkl`, `jwt.purpose`, `jwt.session_id`, `jwt.act_chain`. Helpers `chain.contains` / `chain.originator` / `chain.depth` added.
- [x] **Update ingress-hardening rule** (`MCP_GATEWAY_PLAN.md:835`). Client-supplied `mcp-act-chain` header is stripped; JWT-borne `agent_id` / `wkl` / `auth_method` / `act_chain` claims are stripped before policy unless the `iss` is in `MCP_GATEWAY_TRUSTED_MINT_ISSUERS`.

**Acceptance:** A JWT without `wkl` is rejected by the gateway in `enforce` mode when `auth_method ∈ {svid, spire}` — landed via enforce-mode hardening. CEL rules can reference `jwt.agent_id` and `jwt.act_chain` in policy bundles.

---

## Block 2 — Per-hop token exchange and delegation

This is the largest piece of net-new work. Today: one connect-time JWT covering broad subject ACL. Target: short-lived, audience-scoped JWT per hop with `act_chain` lineage.

### 2.1 Security Token Service (STS)

Form factor and contract fixed by ADRs 0004/0005/0006. Implementation tasks for `trogon-sts` v1:

- [x] **Exchange contract — decided.** RFC 8693-inspired. Inputs: `subject_token`, `subject_token_type`, `audience` (target hop URI), `scope` (optional tool-name list), `requested_token_type` (default JWT), `purpose`, `actor_token` (caller's SVID). Outputs: `access_token`, `issued_token_type`, `expires_in`. Wire spec lives at `docs/identity/sts-exchange.md`.
- [x] **Validation pipeline implementation.** `trogon-sts` verifies `subject_token` (bootstrap vs mesh issuer via `iss` peek + separate JWKS caches), validates `actor_token` against a file trust bundle, looks up `agent_id` in registry, asserts `wkl ∈ allowed_workloads` / `audience ∈ allowed_audiences` / `purpose ∈ allowed_purposes`, appends to `act_chain` (depth ≥ 8 or `(agent_id, wkl)` duplicate → reject), mints mesh JWT with narrowed `aud`/`scope` and TTL 120 s default (60–300 s via `mesh_token_ttl_s`).
- [x] **Transport — decided.** NATS queue-group consumer on `mcp.sts.exchange`, per-request reply inbox (ADR 0004). HTTP facade deferred to v2.
- [x] **Latency budget implementation.** In-memory moka caches for JWKS (bootstrap + mesh), trust bundle, and registry (≤ 60 s TTL). KV watch on `mcp-jwks/mesh/current` for hot reload. (P99 production verification tracked in Carry-over §STS resilience.)
- [x] **Rate limiting implementation.** 100 exchanges / 10 s / `wkl`, 500 / 10 s / `agent_id` per-instance defaults. (Circuit breaker on registry / SpiceDB outage tracked in Carry-over §STS resilience.)
- [x] **Failure mode — decided.** **Fail-closed.** STS down → mesh stops; gateways return structured error, agents retry with backoff. No degraded-mode bypass (ADR 0004).
- [x] **Audit implementation.** Every exchange (success / deny / rate-limit) emits to `mcp.audit.sts.{outcome}` with full inputs and minted claims (not signature).

**Acceptance:** Calling backend B from agent A requires A to exchange its token for one with `aud=B`, `exp ≤ now+TTL`, and `act_chain` ending in A. The gateway rejects any token where `aud` doesn't match its own identity.

### 2.2 Actor chain (`act_chain`)

Spec at `docs/identity/act-chain.md` is the accepted contract. Shadow-mode header projection landed (commit `ebe012c8d`); audit-envelope embedding landed (commit `047147811`).

- [x] **Schema — decided.** `ActChainEntry { sub, agent_id, wkl, iat }`. Order: oldest (originating user) first, current actor last. Integrity by outer-JWT signature only — each STS that appends is the only trusted appender (per-entry signing rejected as too costly for the 40 ms budget).
- [x] **Propagation — decided.** On every STS exchange: new entry appended, never rewritten. Full chain serialized into the audit envelope (`act_chain` optional field, landed).
- [x] **Verification implementation.** Depth cap 8 enforced (hard reject in enforce mode at both gateway ingress and STS exchange); loop detection on `(agent_id, wkl)` duplicates rejects with `-32114`. (Per-entry registry resolution at receipt time tracked in Carry-over §Act-chain registry resolution.)
- [x] **CEL surface implementation.** `jwt.act_chain` exposed as `list<map>` in `policy.rs`; helpers `chain.contains(agent_id)`, `chain.originator()`, `chain.depth()` registered on the CEL context.
- [x] **Header projection — landed.** `mcp-act-chain` parsed on ingress, stripped from forgeable client input, gateway-appended in shadow/enforce. `MCP_GATEWAY_PLAN.md` header-table row landed.

**Acceptance:** A four-hop call (`user → A → B → C → backend`) produces a chain of length 4 at the backend, visible in audit, and (in enforce) rejected on depth > 8 or duplicate `(agent_id, wkl)`. End-to-end registry-resolution of every chain entry is the remaining gap.

### 2.3 Intent / purpose claim

- [x] **Shape — decided.** Enumerated set per agent in the registry (`allowed_purposes`). Per `docs/identity/registry.md`.
- [x] **Propagation implementation.** `trogon-sts` asserts `purpose ∈ allowed_purposes` for the refining agent on each exchange; rejected exchanges audit with the requested purpose. (Originating-client default-purpose handling tracked in Carry-over §Purpose handling at the originator.)
- [x] **CEL + audit surface — landed.** `jwt.purpose` exposed in CEL; `purpose` field on audit envelope.

**Acceptance:** SpiceDB policy can gate on `(agent_id, purpose, resource)` triples instead of just `(subject, resource)`. Claim values are now reachable from CEL; whether SpiceDB rules actually consume them is bundle work.

### 2.4 Gateway egress: mint, don't propagate

- [x] **Replace context propagation with STS exchange.** `trogon-mcp-gateway` egress path now calls `trogon-sts-client` (request/reply on `mcp.sts.exchange`) before backend egress, attaches the minted mesh token as `Authorization: Bearer …`, and drops inbound `Authorization` / `A2a-Caller-Jwt`. Mode-gated by `MCP_GATEWAY_AGENT_IDENTITY=off|shadow|enforce`.
- [x] **Cache exchanges** by `{tenant}:{caller_sub}:{target_aud}:{session_id}:{scope_fingerprint}`, age `min(floor(exp - now - 30s), TTL/2)`, default cap 60 s, bounded LRU (default 10 000, env `MCP_GATEWAY_MESH_CACHE_MAX_ENTRIES`). Proactive refresh at remaining TTL ≤ TTL/4. Metrics emitted for hit/miss/refresh/eviction.
- [x] **Callback direction — implemented.** Server → client callbacks re-exchange with `aud=urn:trogon:mcp:client:{tenant}:{client_id}` (ADR 0005).

**Acceptance:** Backend logs show `aud=<backend>`, `iss=<sts>`, `act_chain` includes the gateway as the last hop before the backend.

---

## Block 3 — A2A client SDK

Uber's "Standardized A2A Client" is the thing that makes the secure path the only path developers reach for. Today there's a `yordis/feat-a2a-nats` branch but no documented SDK contract.

- [x] **Client contract — decided.** Per `docs/identity/sdk.md`. Constructor takes SVID source, STS NATS subject, optional registry endpoint, default audience policy. `call(target_agent, payload, purpose)` does lookup → exchange (`aud=target`) → send → receive. `serve(handler)` does verify inbound token → verify `act_chain` → invoke handler with typed `Caller { originator, chain, purpose }`.
- [x] **Language order — decided.** Rust first (matches gateway crates), then TypeScript (matches `tsworkspace/`), then Python.
- [x] **Rust skeleton landed.** `trogon-a2a-sdk`: `SvidSource` / `Sts` / `Registry` / `MessageTransport` / `Jwks` traits; NATS impls behind `nats` feature; `Client::call` does lookup → exchange → publish with `A2a-Caller-Jwt`; `serve` verifies JWT + `aud` + chain depth/loops, yields typed `Caller`. Mock-based unit tests pass; 33-line `examples/echo_agent.rs`. (Live integration tests tracked in Carry-over §A2A SDK gaps.)
- [x] **Telemetry implementation.** OpenTelemetry spans emitted from `Client::call` with `agent.id` / `agent.chain.depth` / `agent.purpose` attributes. (Serve-side span attribute coverage tracked in Carry-over §A2A SDK gaps.)
- [x] **Lint implementation.** `scripts/ci/check-direct-nats-publishes.sh` (wired into `.github/workflows/ci-rust.yml`) fails the build on non-SDK publishes to `mcp.gateway.request.*`.
- [x] **Docs implementation.** Quickstart, "add a new agent" recipe, and "verify chain in a handler" recipe live in `crates/trogon-a2a-sdk/README.md`; `docs/identity/sdk.md` carries the default-purpose and telemetry-attributes sections.
- [ ] **TypeScript SDK.** Next once Rust contract is exercised by real callers.
- [ ] **Python SDK.** After TypeScript.

**Acceptance:** A new agent can be written in ≤ 50 lines and gets correct identity propagation for free. `examples/echo_agent.rs` is 33 lines today against mocked deps.

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
- [x] **Human-in-the-loop approvals implementation.** JSON-RPC error `-32107 approval_required` with `approval_url` / `approval_subject`. Approval responses land on `mcp.approvals.{request_id}`; gateway resumes the request. TTL on pending approvals; default-deny on expiry.
- [ ] **Context-aware throttling implementation.** Rate limits keyed by `(agent_id, purpose, tenant)` instead of just `jwt.sub`.
- [x] **Anomaly feature emission.** Emit features (chain depth, novel `(agent_id, purpose, target)` tuples, exchange-rate spikes) to a side stream for downstream anomaly scoring. Scoring service itself is out of scope here.

**Acceptance:** A tool can be flagged `sensitive`; calls to it require explicit human approval; the approval is audited with the same `act_chain`.

---

## Block 6 — Migration and compatibility

Cutover is incremental: keep the bootstrap path while shadow-mode validates the mesh path, then flip to enforce.

- [x] **Feature flag — landed.** `MCP_GATEWAY_AGENT_IDENTITY = off (default) / shadow / enforce`. `enforce` is no longer log-only — hard rejects per the enforce-mode hardening section above.
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
- [x] `docs/identity/overview.md` — single-page architecture + mesh-token + 4-hop chain diagram.
- [x] `docs/identity/sts-exchange.md` — full request/response, examples, error codes, audit envelope.
- [x] Update `MCP_GATEWAY_PLAN.md` § Wire-Format Pins with `mcp-act-chain` header row and new claims.
- [x] Update `MCP_GATEWAY_PLAN.md` § CEL variable namespace with `jwt.agent_id`, `jwt.act_chain`, `jwt.purpose`, etc., plus `chain.*` helpers.
- [x] Update `MCP_GATEWAY_PLAN.md` § Audit envelope schema to document the optional identity fields.
- [x] `trogon-agent-registry` — control-plane signer + sync-loop runbook lives at `docs/identity/registry-operations.md`.
- [x] `trogon-jwks-publisher` — KMS / Vault custody recipes in `crates/trogon-jwks-publisher/README.md`; AWS KMS + Vault Transit signers shipped behind `kms-aws` / `vault` features.
- [x] `docs/identity/agent-traffic.md` — agent-traffic view spec (index schema, timeline, chain explorer, OCSF export, `agctl traffic` CLI surface). v1 implementation tracked under "Open after 2026-05-27 swarm".

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

Authoritative ordering lives in the **Execution roadmap** at the top of this file. Roadmap items 1–9 (Rust track) landed across the three 2026-05-27 swarm runs. The remaining critical path is:

- **Item 10 (SPIRE wiring)** — replace the sentinel-`wkl` fallback for service workloads with real SVID attestation at STS ingress; trust-bundle distribution via NATS KV `mcp-trust-bundles`.
- **Item 11 (Agent-traffic view)** — implement the indexer + projector + OCSF exporter against the spec already accepted at `docs/identity/agent-traffic.md`; the `trogon-traffic-view` crate skeleton is the home.
- **Item 12 (Adaptive access)** — step-up auth, human-in-the-loop approvals (`-32107`), context-aware throttling, anomaly emission.

Smaller residual items live in **Open after 2026-05-27 swarm**: gateway-ingress chain resolution, STS-side empty-purpose reject, A2A SDK TypeScript + Python tracks, and the remaining shadow-mode telemetry / backfill bullets in Block 6.

Decisions are settled. Build.

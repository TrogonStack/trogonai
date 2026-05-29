# PENDING_TODO — Agent Identity

Reference: https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/
Companion to: `MCP_GATEWAY_PLAN.md`

**Status: closed.** Every planned block landed on `yordis/agentgateway`. The decisions, contracts, and shipped surfaces below are now the authoritative references; this file no longer tracks open work other than the operational follow-ups at the bottom.

## Binding decisions

| # | Decision | Reference |
|---|---|---|
| 0001 | Hybrid tenancy (account per tenant in prod, JWT `tenant` claim in dev/CI). | `docs/adr/0001-tenancy-model.md` |
| 0002 | Two-claim identity (`sub` = logical actor, `wkl` = attested workload). | `docs/adr/0002-identity-layers.md` |
| 0003 | Bootstrap auth-callout JWT → STS-minted per-hop mesh tokens. | `docs/adr/0003-bootstrap-vs-mesh-tokens.md` |
| 0004 | STS as NATS queue-group on `mcp.sts.exchange`. | `docs/adr/0004-sts-form-factor.md` |
| 0005 | Mesh TTL 120 s default (60–300 s range), per-backend `aud`, hard reject on mismatch in enforce. | `docs/adr/0005-token-ttl-and-audience.md` |
| 0006 | Cloud KMS / Vault Transit in prod, file PEM in dev; JWKS over KV + HTTPS + req-reply. | `docs/adr/0006-mesh-token-signing-keys.md` |
| 0033 | CLI-first agent-traffic UI via `agctl traffic`; web console deferred. | `docs/adr/0033-agent-traffic-ui-substrate.md` |

## Shipped surfaces

- **Identity specs (accepted contracts):** `docs/identity/{overview,sts-exchange,sdk,registry,act-chain,jwt-claim-schema,agent-traffic,registry-operations}.md`.
- **Gateway:** `MCP_GATEWAY_AGENT_IDENTITY=off|shadow|enforce` claim parsing, enforce-mode hard rejects, CEL `jwt.*` + `chain.*` surface, egress STS minting with LRU cache, audit envelope identity fields.
- **Crates:** `trogon-agent-registry` (+ controller + `agctl`), `trogon-jwks-publisher` (file PEM, AWS KMS, Vault Transit), `trogon-sts` (+ probe, circuit breaker, chain resolution), `trogon-identity-types`, `trogon-a2a-sdk` (Rust), `trogon-traffic-view`.
- **A2A SDKs:** Rust (`rsworkspace/crates/trogon-a2a-sdk`), TypeScript (`@trogonai/a2a-sdk`), Python (`trogonai-a2a-sdk`).
- **Adaptive access:** step-up auth, `-32107 approval_required` flow, context-aware throttling, anomaly feature emission.

## Posture

**Greenfield.** No legacy fleet, no backfill step. Every caller is onboarded into `trogon-agent-registry` via signed manifest commits processed by `trogon-agent-registry-controller`; SVIDs come from SPIRE (service workloads) or file PEM (dev/CI). The shadow→enforce gradient remains as a first-deploy safety net rather than a coexistence mechanism.

First-deploy gate: every onboarded caller passes shadow-mode without `aud_mismatch` / `agent_id_missing` / `act_chain_depth_overflow` events across acceptance traffic, and STS P99 < 40 ms over the same window. When real traffic begins to accumulate, escalate to the original bar — zero shadow-mode events for 7 consecutive days — before flipping `MCP_GATEWAY_AGENT_IDENTITY=enforce`. Rollback: set `MCP_GATEWAY_AGENT_IDENTITY=shadow`, drain mesh-token cache.

## Operational follow-ups (decide as conditions warrant — none block execution)

- Gateway's own identity: SPIFFE ID + privileged role today; promote to a registered `agent_id` if/when policy needs to gate on it.
- Batch / non-interactive originators in `act_chain`: use sentinel `wkl:sentinel:batch` with `auth_method: "service-account"` as originator entry.
- Maximum `act_chain` depth: 8 (configurable per bundle). Revisit if real workflows hit the cap.
- `purpose` refinement audit: STS records before/after `purpose` on every exchange in `mcp.audit.sts.{outcome}`.
- STS exposure to user clients: mesh-internal only in v1; HTTP facade per ADR 0004 lands when a user-facing client needs it.
- Offline dev without SPIRE: file-based SVIDs + `sentinel:human` fallback (ADR 0002 + ADR 0006 dev profile).
- `trogon-decider` placement: STS stateless in v1 with JetStream audit; reconsider event-sourced STS if replay/consistency proves valuable (ADR 0004 open question 2).
- Multi-region: regional STS instances with local registry cache, default. Trust-bundle replication SLA TBD with first multi-region deployment (ADR 0006 §6).
- OAuth-MCP composition (`MCP_GATEWAY_PLAN.md:67`): OAuth access token enters as `subject_token` to STS — same TTL / `aud` rules apply (ADR 0005 §OQ6).
- Backfill hook: if a non-greenfield deployment ever needs bulk import, add an `agctl registry backfill` subcommand that consumes a TOML inventory and emits signed manifests via the existing controller path.

# PENDING_TODO — Agent Identity

Reference: https://www.uber.com/us/en/blog/solving-the-agent-identity-crisis/
Companion to: `MCP_GATEWAY_PLAN.md`

**Status: closed.** Every planned block landed on `yordis/agentgateway`; the residual ADR open-questions are resolved in "Decisions log" rather than parked.

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

## Decisions log (former operational follow-ups)

- **`act_chain` depth = 8**, per-bundle override allowed. Shipped; revisit only if a real workflow saturates the cap.
- **STS exposure to user clients = mesh-internal only in v1.** Wire contract is transport-agnostic (ADR 0004), so the HTTP facade is a future PR triggered by a user-facing caller, not a planning gap.
- **Offline dev path = file PEM SVIDs + `sentinel:human` fallback** per ADR 0002 + ADR 0006 dev profile. Shipped.
- **STS placement = stateless service + JetStream audit.** Not event-sourced. ADR 0004 OQ2 is closed at this verdict; reopening requires a concrete replay-or-consistency requirement.
- **Multi-region = single-region in v1; regional STS instances with local registry cache when expanding.** Trust-bundle replication SLA is set at first multi-region deploy (no SLA without a region pair).
- **Purpose refinement audit = no-op in v1.** STS validates `purpose ∈ allowed_purposes` without transforming it; there is no before/after to record. If a refinement step is ever introduced, that PR adds `purpose_requested` / `purpose_resolved` to the STS audit envelope at the same time.
- **OAuth-MCP composition = deferred until first OAuth-fronted MCP client.** ADR 0005 §OQ6 already pins the semantics (OAuth access token enters STS as `subject_token`); the multi-issuer config is a same-day add when the first caller arrives, not standing work.

## Open work

None. The two residuals (gateway as registered agent, batch-originator example) landed in `act_chain.rs`, `docs/identity/act-chain.md`, and `rsworkspace/crates/trogon-agent-registry/examples/mcp-gateway.toml`.

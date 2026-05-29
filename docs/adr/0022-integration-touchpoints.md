# ADR 0022: MCP Gateway Integration Touch-Points

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-29) |
| **Date** | 2026-05-29 |
| **Deciders** | *(platform security / mcp gateway -- TBD)* |
| **Blocks** | `MCP_GATEWAY_PLAN.md` Block C item 9 (Integration touch-points); unblocks decider bridge design, Slack approval wiring, and multi-consumer audit fan-out planning |
| **Related** | [integration-touchpoints.md](../identity/integration-touchpoints.md) (design spec); [reference-audit-envelope.md](../identity/reference-audit-envelope.md); [adaptive-access.md](../identity/adaptive-access.md); [agent-traffic.md](../identity/agent-traffic.md); [act-chain.md](../identity/act-chain.md); [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Wire-Format Pin 7 (audit envelope schema); [ADR 0001](0001-tenancy-model.md) (tenancy / shared account); [ADR 0011](0011-nats-auth-callout.md) (perimeter auth); [ADR 0016](0016-multi-region-topology.md) (regional audit streams) |

## Context

TrogonStack already ships two gateway-class ingress paths and an event-sourced decision library:

| Component | Role today | NATS surface |
|-----------|------------|--------------|
| **`trogon-mcp-gateway`** | MCP JSON-RPC policy enforcement, egress mesh mint, JetStream audit publish | `{prefix}.gateway.request.>`, `{prefix}.audit.>`, `mcp.sts.exchange`, `mcp.registry.agent.lookup`, `mcp.approvals.*` |
| **`trogon-gateway`** | Slack (and similar) ingress; publishes Slack events to core NATS | `slack.{event.type}` on stream **`SLACK`**, filter `slack.>` |
| **`trogon-decider`** | Library for event-sourced command → event decisions | **No** NATS command subject to MCP gateway in-repo today |

The MCP gateway program must plug into **`trogon-decider`** and coexist with **`trogon-gateway`** without re-inventing either. Block C item 9 in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) left three questions open:

1. How does the gateway emit to / consume from `trogon-decider`?
2. How does it coexist with `trogon-gateway` (Slack, approvals)?
3. Is **`trogon-mcp-gateway` itself a decider workload** (processing events) or only an emitter?

The operator reference [integration-touchpoints.md](../identity/integration-touchpoints.md) inventories ten integration families (STS, decider, approvals, registry, SpiceDB, NATS transport, trust bundles, traffic indexer, anomaly pipeline, `agctl`). This ADR formalizes the **decider intersection**, **Slack / approval coexistence**, **shared NATS substrate**, and **subject-namespace discipline** — the pieces Block C item 9 names explicitly.

**Why a durable decision is required now**

Without pinning integration shape:

- A second audit pipeline (direct DB, bypass stream) will fork compliance consumers and SIEM wiring.
- Risk / HITL evaluators may embed in-process logic that cannot be replayed or audited consistently with Trogon's event-sourced direction.
- `trogon-gateway` and `trogon-mcp-gateway` may collide on subject names, queue groups, or auth assumptions when both serve the same tenant.
- Downstream work (decider bridge crate, Slack approval publisher, traffic indexer widening) lacks a single canonical contract for fan-out from `MCP_AUDIT`.

**Current branch snapshot**

- Gateway publishes audit to JetStream stream **`MCP_AUDIT`** under `{prefix}.audit.{outcome}.{direction}.{method_root}` ([integration-touchpoints.md §6](../identity/integration-touchpoints.md#6-gateway--nats-core-and-jetstream)).
- `trogon-traffic-view` durable consumer **`agent-traffic-projector`** filters `mcp.audit.>` but today acks STS and registry subjects only; gateway-shaped rows are classified for future widening.
- Human-in-the-loop uses core NATS pub/sub on `mcp.approvals.{request_id}` and `mcp.approvals.step-up.{request_id}`; gateway subscribes; external publishers grant ([adaptive-access.md](../identity/adaptive-access.md)).
- `trogon-gateway` ingests Slack into `slack.>`; it does **not** publish to `mcp.approvals.*` in the current tree — a bridge is **proposed**.
- `trogon-decider` is a library plus runtime contracts; no in-repo NATS subscriber translates MCP audit into decider events yet.

**Audit envelope as the natural intersection (Pin 7)**

[MCP_GATEWAY_PLAN.md §7](../../MCP_GATEWAY_PLAN.md#7-audit-envelope-schema) (Wire-Format Pin 7) pins the gateway audit JSON schema field `schema: "trogon.mcp.audit/v1"`. STS emits `trogon.mcp.audit.sts/v1`. Registry and approval flows have sibling shapes documented in [reference-audit-envelope.md](../identity/reference-audit-envelope.md). The legal/compliance record is the JetStream append-only stream; decider, traffic indexer, and future risk services are **downstream consumers** of the same envelopes — not parallel write paths.

---

## Decision

**Adopt audit-as-events on shared JetStream `MCP_AUDIT` as the sole gateway → decider integration.** `trogon-mcp-gateway` is a **publish-only emitter** on the MCP hot path — it is **not** a decider workload. A separate **decider audit bridge** service (backed by `trogon-decider` + `trogon-decider-runtime`) consumes `MCP_AUDIT` and appends decider domain events. **`trogon-gateway`** coexists in the **same NATS account per tenant** ([ADR 0001](0001-tenancy-model.md)) with **disjoint subject trees** (`slack.>` vs `{prefix}.*`). Slack-driven approvals reach the MCP gateway via a **proposed bridge** that publishes to `mcp.approvals.{request_id}` after operator action.

### 1. Gateway ↔ trogon-decider (audit-as-events)

| Field | Value |
|-------|-------|
| **Direction** | gateway → decider (indirect, via JetStream fan-out) |
| **Gateway role** | **Emitter only** — append audit envelopes; no decider command handler in gateway process |
| **Wire (today)** | Gateway publishes to `{prefix}.audit.>` on stream **`MCP_AUDIT`** |
| **Wire (proposed bridge)** | Durable JetStream consumer on **`MCP_AUDIT`** translates selected audit schemas into decider domain events |
| **No dedicated command subject** | Gateway does **not** publish to `mcp.decider.*` or equivalent command subjects in v1 |

#### Audit envelope → decider mapping (pinned)

**Ingress (JetStream)**

| Property | Value |
|----------|-------|
| **Stream name** | `MCP_AUDIT` (env `MCP_GATEWAY_AUDIT_STREAM`, default `MCP_AUDIT`) |
| **Capture filter** | `{prefix}.audit.>` (default prefix `mcp` → `mcp.audit.>`) |
| **Gateway publish subject pattern** | `{prefix}.audit.{outcome}.{direction}.{method_root}` — see [reference-audit-envelope.md §1.1](../identity/reference-audit-envelope.md#11-covered-audit-families) |
| **Sticky rule** | Audit subjects are append-only; consumers must not trim per-request compliance history |

**Decider bridge consumer (proposed service, not in gateway)**

| Property | Value |
|----------|-------|
| **Durable consumer name** | `mcp-decider-audit-bridge` |
| **Filter subject** | `{prefix}.audit.>` (same tree as traffic indexer) |
| **Ack policy** | Explicit ack after successful decider append; retry on transient storage errors |
| **Owner** | Separate deployment using `trogon-decider` + `trogon-decider-runtime`; **not** `trogon-mcp-gateway` |

**Envelope schemas consumed (phase 1 bridge scope)**

| `schema` value | NATS subject family (examples) | Decider aggregate (proposed) | Decider event stream id prefix (proposed) |
|----------------|-------------------------------|------------------------------|-------------------------------------------|
| `trogon.mcp.audit/v1` | `mcp.audit.allow.request.tools`, `mcp.audit.deny.request.tools`, … | `McpGatewayDecision` | `decider/mcp-gateway/{tenant}/decisions` |
| `trogon.mcp.audit.sts/v1` | `mcp.audit.sts.success`, `mcp.audit.sts.deny`, … | `McpStsExchange` | `decider/mcp-sts/{tenant}/exchanges` |
| `trogon.mcp.audit.registry.*` **(phase 2)** | `mcp.audit.registry.lookup.*`, lifecycle subjects | `McpRegistryEvent` | `decider/mcp-registry/{tenant}/mutations` |

The **`decider/...` stream id prefix** is the decider-runtime storage key namespace — **not** a core NATS pub/sub subject. It avoids collision with `{prefix}.audit.>` and `slack.>` trees. Tenant `{tenant}` is taken from the envelope payload (`tenant` / `tenant_id`); under hard tenancy it must match the NATS account mapping ([ADR 0001](0001-tenancy-model.md)).

**Translation rule (normative)**

1. Bridge receives JetStream message on `{prefix}.audit.>`.
2. Parse JSON; require known `schema` major version (`trogon.mcp.audit/v1`, `trogon.mcp.audit.sts/v1`).
3. Map envelope fields (Pin 7: `trace_id`, `tenant`, `caller`, `decision`, `method`, `act_chain`, `spicedb`, …) into a typed decider command input.
4. Execute decider logic; append resulting domain events to the aggregate stream id for that family.
5. Ack JetStream message only after durable append succeeds.

Unknown schema major versions: **skip with metric** (do not poison the consumer); unknown minor fields: tolerate per Pin 7 forward-compat rule.

#### Is `trogon-mcp-gateway` a decider?

| Question | Answer |
|----------|--------|
| Does the gateway process decider commands on the MCP hot path? | **No** |
| Does the gateway host `trogon-decider` replay / append? | **No** |
| Does the gateway publish audit that decider consumes? | **Yes** — best-effort publish; request path continues on publish failure today |
| Where does risk / HITL evaluation move? | **Proposed** separate **`trogon-decider`-backed evaluator service** replaces in-process `policy::evaluate_risk` for HITL/risk classes ([integration-touchpoints.md §2](../identity/integration-touchpoints.md#2-gateway--trogon-decider)); gateway may **read** proposed risk scores from `mcp.metrics.anomaly.risk` without being a decider |

Gateway remains a **Policy Enforcement Point** that emits facts; decider services derive durable decision history from those facts.

### 2. Gateway ↔ trogon-gateway (Slack) coexistence

| Field | Value |
|-------|-------|
| **NATS account** | **Same account per tenant** when both MCP and Slack ingress are offered ([ADR 0001](0001-tenancy-model.md) cross-protocol rule) |
| **Separate accounts** | **Not** required for Slack vs MCP — isolation is account-per-tenant, not service-per-account |
| **Auth pipeline** | **Not identical** — MCP perimeter uses NATS auth callout → bootstrap User JWT ([ADR 0011](0011-nats-auth-callout.md)); Slack ingress uses `trogon-gateway::source::slack` → `slack.>` with Slack signing-secret verification, not MCP OAuth callout |
| **Shared substrate** | Same regional NATS cluster, same tenant account, shared JetStream domain per [ADR 0016](0016-multi-region-topology.md) |

#### Subject and stream separation

| Owner | Stream / pattern | Purpose |
|-------|------------------|---------|
| `trogon-gateway` | JetStream **`SLACK`**, filter `slack.>` | Slack event ingress |
| `trogon-mcp-gateway` | Core `{prefix}.gateway.request.>`, `{prefix}.server.>`, `_INBOX.gateway.{instance_id}.>` | MCP JSON-RPC |
| `trogon-mcp-gateway` | JetStream **`MCP_AUDIT`**, filter `{prefix}.audit.>` | Compliance audit |
| Both (bridge) | Core `mcp.approvals.{request_id}`, `mcp.approvals.step-up.{request_id}` | HITL / step-up decisions |

There is **no** crate named `trogon-agent-gateway` in this repo; Slack ingress lives under **`trogon-gateway`**.

#### Slack → approval bridge (proposed)

| Step | Behavior |
|------|----------|
| 1 | MCP gateway parks call; client receives `-32107 approval_required` with `approval_subject = mcp.approvals.{request_id}` |
| 2 | Operator acts in Slack UI handled by `trogon-gateway` |
| 3 | **Proposed** Slack interaction handler publishes `ApprovalDecisionMessage` JSON to `mcp.approvals.{request_id}` |
| 4 | Gateway `approvals` module resumes or denies parked call |

Until the bridge lands, operators or automation publish approval decisions directly to the approval subject ([adaptive-access.md § Implementing an approval client](../identity/adaptive-access.md#implementing-an-approval-client)).

### 3. Naming collision avoidance

| Namespace | Prefix / pattern | Owner | Collision rule |
|-----------|------------------|-------|----------------|
| MCP data plane | `{prefix}.` (default `mcp.`) | MCP gateway, STS, registry | Configurable via `MCP_PREFIX` / `mcp_nats::Config`; never reuse `slack.` |
| Slack ingress | `slack.` | `trogon-gateway` | Must not publish under `{prefix}.` except via explicit approval bridge |
| A2A (same account) | `a2a.` | A2A services | Orthogonal tree per [ADR 0001](0001-tenancy-model.md) |
| Decider storage ids | `decider/` **(internal)** | Decider bridge | Not published to core NATS; no subject subscription |
| Approvals | `mcp.approvals.` | Shared convention | `{request_id}` suffix is gateway-correlated id; no wildcards |
| Metrics (non-audit) | `mcp.metrics.` | Gateway, STS probes | Not on `MCP_AUDIT`; do not confuse with audit consumers |
| Queue groups | `mcp-gateway`, `trogon-sts`, `trogon-agent-registry` | MCP services | **`trogon-gateway` must not join `mcp-gateway`** — separate queue group per service |

**Multi-consumer fan-out on `MCP_AUDIT`**

Multiple durable consumers may attach to the same stream with independent filters:

| Consumer | Filter | Purpose |
|----------|--------|---------|
| `agent-traffic-projector` | `mcp.audit.>` | Postgres traffic index / OCSF ([agent-traffic.md](../identity/agent-traffic.md)) |
| `mcp-decider-audit-bridge` **(proposed)** | `{prefix}.audit.>` | Decider domain event projection |
| Operator SIEM **(external)** | `{prefix}.audit.>` or subset | Compliance export |

Consumers must tolerate duplicate processing semantics only within their own projection — they do **not** share cursor state.

### 4. End-to-end integration diagram (ASCII)

```text
                    ┌─────────────────────┐
  MCP client ──────►│ trogon-mcp-gateway  │──► backend mcp.server.*
                    │  (queue: mcp-gateway)│
                    └─────────┬───────────┘
                              │ publish
                              ▼
                    ┌─────────────────────┐
                    │ JetStream MCP_AUDIT │
                    │  mcp.audit.>        │
                    └─────────┬───────────┘
              fan-out         │         fan-out
         ┌────────────────────┼────────────────────┐
         ▼                    ▼                    ▼
  agent-traffic-      mcp-decider-audit-     (SIEM / other
  projector           bridge (proposed)       durable consumers)
         │                    │
         ▼                    ▼
    Postgres            decider/{aggregate}
    traffic index       event streams

  Slack ──► trogon-gateway ──► slack.> (SLACK stream)
                │
                └── (proposed) ──► mcp.approvals.{request_id} ──► MCP gateway sub
```

---

## Consequences

### Positive

- **Single audit-of-truth.** JetStream `MCP_AUDIT` remains the legal record ([MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) audit story); decider, traffic view, and SIEM are projections — no competing write path.
- **Gateway hot path stays simple.** Publish-only audit avoids decider latency and storage failures on every `tools/call`; bridge retries decoupled from request completion.
- **Reuses existing Trogon patterns.** Same fan-out model as `trogon-traffic-view`; same approval subjects as adaptive-access; same account-per-tenant rule as A2A + MCP coexistence.
- **Event-sourced risk path.** Moving HITL/risk to a decider-backed service enables replay, test fixtures (`trogon-decider::testing`), and consistent domain events instead of opaque in-process state.
- **Clear ownership boundaries.** `trogon-gateway` owns Slack ingress; `trogon-mcp-gateway` owns MCP policy; decider bridge owns audit translation — teams can ship independently.

### Negative

- **Shared NATS substrate coupling.** Gateway, Slack ingress, STS, registry, and multiple audit consumers depend on regional NATS JetStream health ([ADR 0016](0016-multi-region-topology.md)). JetStream pressure affects all consumers.
- **Audit duplication (logical).** The same envelope bytes are read by traffic projector, decider bridge, and optional SIEM — multiplied egress bandwidth and consumer CPU (fan-out cost).
- **Mitigation — independent durables.** Each consumer maintains its own ack cursor; scale consumers horizontally; monitor lag per consumer name.
- **Eventual consistency for decider-derived views.** Decider projections lag gateway decisions by bridge processing time; HITL fail-closed on decider unreachable is **proposed** and adds availability dependency for risk classes.
- **Bridge schema drift.** Gateway must emit Pin 7 fields the bridge expects; envelope changes require coordinated bridge updates (version gate on `schema`).

### Neutral

- **Best-effort audit publish today.** Gateway logs publish failure and continues request path ([integration-touchpoints.md §6](../identity/integration-touchpoints.md)); decider and traffic index may miss rows — unchanged by this ADR; hardening is a separate reliability item.
- **Phase 2 registry audit in decider bridge** extends mapping table without changing gateway publish shape.

---

## Alternatives considered

### Direct DB writes for audit (gateway → Postgres / warehouse, bypass JetStream)

Gateway or a sidecar writes audit rows directly to Postgres, BigQuery, or similar; decider reads from DB changefeeds.

| Assessment | |
|------------|---|
| **Rejected because** | Violates the JetStream-first legal record in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) and [agent-traffic.md](../identity/agent-traffic.md); forks compliance consumers; `trogon-decider` loses a single ordered append log; direct DB writes on the hot path add latency and connection pool coupling. **Decider is the audit-of-truth for decision history** — facts still originate from JetStream envelopes. |

### Separate audit stream that bypasses decider (parallel `MCP_AUDIT_DECIDER` only)

Gateway double-publishes: one copy to `MCP_AUDIT` for SIEM, one to a decider-only stream with a different schema.

| Assessment | |
|------------|---|
| **Rejected because** | Ratchets duplicate downstream consumers and schema maintenance; operators must sync retention and residency on two streams ([ADR 0016](0016-multi-region-topology.md)); drift between copies breaks replay guarantees. **Chosen:** one stream, many durables — same pattern as `agent-traffic-projector`. |

### Embed decider in `trogon-mcp-gateway` process (gateway as decider workload)

Gateway hosts decider replay, append, and risk evaluation in-process on the MCP queue-group worker.

| Assessment | |
|------------|---|
| **Rejected because** | Couples decision storage I/O to ingress latency; complicates horizontal scale (each replica replays independently unless shared store); blurs PEP vs decision-engine boundaries; contradicts paper owner split ("gateway stays publish-only"). |

### Dedicated NATS account for Slack vs MCP

Place `trogon-gateway` in a Slack-only account and MCP in another account for the same customer.

| Assessment | |
|------------|---|
| **Rejected as default** | Conflicts with [ADR 0001](0001-tenancy-model.md) cross-protocol rule (one account per tenant for A2A + MCP); approval bridge would require export/import for `mcp.approvals.*`; doubles NSC provisioning. Acceptable only as a regulated split — not the Trogon default. |

### Synchronous request/reply to decider on every gated call

Introduce `mcp.decider.evaluate` R/R subject; gateway awaits decider before backend forward.

| Assessment | |
|------------|---|
| **Rejected for v1** | Adds NATS RTT + storage append to hot path; duplicates audit envelope content on the wire; paper spec explicitly states no dedicated command subject today and proposes audit consumption instead. Fail-closed risk evaluation may add **read** paths later without making gateway a command publisher. |

---

## Implementation notes

### Gateway publish contract (unchanged responsibilities)

| Surface | Detail |
|---------|--------|
| Audit stream init | `audit::ensure_audit_stream` — retention bootstrap `max_messages: 100_000`; operator may override |
| Skip init | `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT=1` |
| Envelope type | `trogon_mcp_gateway::audit::AuditEnvelope` → target wire `schema: trogon.mcp.audit/v1` (Pin 7) |
| Related producers on same stream | STS `mcp.audit.sts.*`, registry `mcp.audit.registry.*` — bridge consumes when schema mapped |

### Decider bridge (proposed crate / deployment)

| Item | Notes |
|------|-------|
| Crate home | New binary or extend existing projector pattern alongside `trogon-traffic-view` |
| Dependencies | `trogon-decider`, `trogon-decider-runtime`, JetStream client |
| Config | `MCP_DECIDER_BRIDGE_ENABLED` (default `0` until bridge tested), `MCP_DECIDER_BRIDGE_CONSUMER=mcp-decider-audit-bridge`, `MCP_DECIDER_BRIDGE_STREAM=MCP_AUDIT` |
| Storage backend | Pluggable via `trogon-decider-runtime` stream adapters — NATS KV / Postgres / other not pinned here |
| Idempotency | Key on `(stream_seq, event_id)` or envelope hash to survive redelivery |

### Risk / approval evaluator (proposed follow-on)

| Item | Notes |
|------|-------|
| Replaces | In-process `policy::evaluate_risk` for HITL/risk classes ([adaptive-access.md](../identity/adaptive-access.md)) |
| Input | Decider state rebuilt from bridged events + optional live reads |
| Output | Gateway may subscribe to `mcp.metrics.anomaly.risk` **(proposed)** or consult decider read model |
| Fail-mode | **Proposed** fail-closed when decider unreachable for HITL/risk classes ([failure-mode-matrix.md FM-DECIDER](../identity/failure-mode-matrix.md)) |

### trogon-gateway approval bridge (proposed)

| Item | Notes |
|------|-------|
| Publisher | `trogon-gateway` Slack interaction handler |
| Subject | `mcp.approvals.{request_id}` or `mcp.approvals.step-up.{request_id}` |
| Payload | `ApprovalDecisionMessage` — [integration-touchpoints.md §3](../identity/integration-touchpoints.md#3-gateway--approvals-and-trogon-gateway-slack) |
| Authz | Publisher service User must have `publish` on `mcp.approvals.>` only — not `{prefix}.server.>` |

### Multi-region ([ADR 0016](0016-multi-region-topology.md))

| Asset | Scope |
|-------|-------|
| `MCP_AUDIT` | **Regional** — gateway publishes to local stream only |
| Decider bridge consumer | **Regional** — consumes regional `MCP_AUDIT` |
| Decider event storage | **Regional** by default; cross-region aggregation deferred |
| Slack `SLACK` stream | **Regional** |

Cross-region decider topology (global vs regional aggregates, mirror of decider streams) is **not** decided here — see Open questions.

### Environment variables (summary)

| Variable | Service | Integration |
|----------|---------|-------------|
| `MCP_GATEWAY_AUDIT_STREAM`, `MCP_GATEWAY_SKIP_AUDIT_STREAM_INIT` | MCP gateway | Audit publish |
| `MCP_PREFIX` | MCP stack | Subject prefix / collision avoidance |
| `MCP_DECIDER_BRIDGE_ENABLED`, `MCP_DECIDER_BRIDGE_CONSUMER` | Decider bridge | Config gate |
| `MCP_GATEWAY_REGION` | MCP gateway | Regional placement ([ADR 0016](0016-multi-region-topology.md)) |

Full gateway env table: [integration-touchpoints.md § Environment variables](../identity/integration-touchpoints.md#environment-variables-gateway-process).

---

## Status of supporting work

| Item | Status | Owner / anchor |
|------|--------|----------------|
| Design spec [integration-touchpoints.md](../identity/integration-touchpoints.md) | **Done** (paper) | Block C reference |
| Gateway audit publish to `MCP_AUDIT` | **Done** (branch) | `trogon-mcp-gateway::audit` |
| Pin 7 audit envelope schema | **Done** (plan) | [MCP_GATEWAY_PLAN.md §7](../../MCP_GATEWAY_PLAN.md#7-audit-envelope-schema), [reference-audit-envelope.md](../identity/reference-audit-envelope.md) |
| Traffic indexer consumer | **Partial** | `agent-traffic-projector`; gateway subjects classified, not yet acked |
| Decider audit bridge consumer | **Pending** | `mcp-decider-audit-bridge` |
| Decider-backed risk evaluator | **Pending** | Replaces `policy::evaluate_risk` |
| Slack → `mcp.approvals.*` bridge | **Pending** | `trogon-gateway` |
| `MCP_GATEWAY_PLAN.md` Block C item 9 checkbox | **Pending** | Editorial after ADR acceptance |
| This ADR | **Accepted** (2026-05-29) | `docs/adr/0022-integration-touchpoints.md` |

---

## Open questions

| # | Question | Default / deferral |
|---|----------|-------------------|
| 1 | **Cross-region decider topology** — global aggregate vs regional-only replay; mirror of decider storage across regions | **Deferred to [ADR 0016](0016-multi-region-topology.md)** and follow-on implementation; default **regional** bridge + regional storage |
| 2 | Exact decider event type names and aggregate boundaries for `McpGatewayDecision` vs session-scoped risk state | Bridge implementation ticket; mapping table in this ADR is **proposed** |
| 3 | Fail-closed decider unreachable for HITL — timeout budget and fallback to inline heuristic | [failure-mode-matrix.md FM-DECIDER](../identity/failure-mode-matrix.md); default fail-closed for production |
| 4 | Include registry audit schemas in phase-1 bridge or phase-2 only | **Phase 2** unless registry decisions needed for risk |
| 5 | Hash-chained audit envelopes (prior digest in envelope) — decider idempotency interaction | [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) tamper-evidence note; bridge must treat chain as opaque metadata until Pin lands |
| 6 | OAuth-specific audit subjects (`mcp.audit.oauth.*` proposed) — decider mapping | Defer until [oauth-mcp-integration.md](../identity/oauth-mcp-integration.md) subjects ship |

---

## Rollback plan

If audit-as-events decider integration proves wrong (schema churn, consumer lag incidents, or decider storage unsuitable):

1. **Disable the bridge (immediate).** Set `MCP_DECIDER_BRIDGE_ENABLED=0` (or scale bridge deployment to zero). Gateway and `MCP_AUDIT` traffic continue unchanged; decider projections stop updating; gateway remains publish-only.
2. **Private audit prefix (controlled degradation).** Optionally repoint gateway audit publish to a **non-decider** subject prefix via config (e.g. `MCP_GATEWAY_AUDIT_SUBJECT_ROOT=mcp.audit.private`) and a dedicated stream **`MCP_AUDIT_PRIVATE`** — SIEM/traffic consumers keep using original `MCP_AUDIT` filter; decider bridge filter excludes private tree. Requires env change + stream bootstrap; use during bridge bugs only.
3. **Revert risk path.** Keep in-process `policy::evaluate_risk` feature-flagged until decider evaluator matches SLOs; gateway need not restart decider on rollback.
4. **Deprecation cycle.** If a synchronous `mcp.decider.*` R/R path was experimented with in dev, remove bridge consumer before deleting stream — no production R/R was accepted in this ADR.

Rollback does **not** require gateway code deploy for step 1; config gate on the bridge is the primary escape hatch.

---

*Design context: [integration-touchpoints.md](../identity/integration-touchpoints.md). This ADR is the durable decision record for Block C item 9; wire-level detail remains in the spec and reference-audit-envelope.*

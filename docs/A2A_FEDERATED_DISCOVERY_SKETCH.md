# A2A federated discovery — Phase 4 sketch

Engineering sketch for **cross-Account AgentCard discovery** (Phase 4). Federation is **off by default**; operators opt in by signing NATS Account exports of `{prefix}.discover.>` and matching imports. The gateway applies **SpiceDB catalog shaping** at the import boundary so callers only see federated AgentCards they are authorized to view. Not implemented in-tree yet — shape is decided in [`../A2A_PENDING_DECISION.md`](../A2A_PENDING_DECISION.md) §8 and tracked in [`../A2A_TODO.md`](../A2A_TODO.md) §Phase 4.

## Related links

| Document | Purpose |
|----------|---------|
| [`../A2A_PLAN.md`](../A2A_PLAN.md) | Master architecture — tenancy model, discovery rationale (KV vs scatter-gather), SpiceDB tuple table, Phase 4 delivery |
| [`../A2A_TODO.md`](../A2A_TODO.md) | Phase 4 tracker — federated discovery exports, `a2a-bridge`, cross-binding tests |
| [`./A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./A2A_NSC_ACCOUNT_BOOTSTRAP.md) | Per-Account NSC bootstrap — registrar/discover ACLs; cross-Account export/import is explicitly future |
| [`./A2A_GATEWAY_ROADMAP.md`](./A2A_GATEWAY_ROADMAP.md) | Gateway SpiceDB Tier 1 + `BulkCheckPermission` for catalog shaping (same engine gates federated cards) |
| [`./A2A_BRIDGE_SKETCH.md`](./A2A_BRIDGE_SKETCH.md) | HTTPS sidecar — multi-cluster callers still connect to one Account; federation is a discover visibility concern, not a bridge substitute |
| [`./catalog-kv-watch.md`](./catalog-kv-watch.md) | Push-driven catalog freshness inside a single Account |
| [`./A2A_JETSTREAM_ACCOUNT_STREAMS.md`](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) | `A2A_AGENT_CARDS` KV — authoritative per Account |

---

## Single-Account MVP vs multi-org federation

Today the binding runs inside **one shared NATS Account** for development. Subjects already match the Account-per-tenant decision (no `{tenant}` segment); JetStream assets reuse the same names (`A2A_AGENT_CARDS`, `A2A_EVENTS`) because Account membership disambiguates.

| Dimension | Single-Account MVP (now) | Multi-org federation (Phase 4) |
|-----------|--------------------------|--------------------------------|
| **Tenancy** | One Account hosts all agents and callers | One Account per org/tenant; isolation is NATS Account boundary |
| **Catalog authority** | `A2A_AGENT_CARDS` KV in that Account is the sole source of truth | Each **publisher Account** owns its KV; consumers never write foreign cards into local KV |
| **Discover path** | `{prefix}.discover.{agent_id}` request/reply served by local `DiscoverService` reading local KV | Same local path for **native** agents; **federated** agents appear via imported `{prefix}.discover.>` from peer Accounts |
| **Cross-Account traffic** | None — all subjects stay inside one Account | Operator-signed **export** (publisher) + **import** (consumer) of `{prefix}.discover.>` only; gateway/task/push subjects stay Account-local unless separately exported |
| **Authz shaping** | SpiceDB `BulkCheckPermission` on local agent ids (Phase 1) | Same checks, extended with **federated agent** resource tuples and import-side filtering |
| **Operational posture** | Prove registrar ACL, schema validation, discover service | Explicit federation contracts, audit of cross-Account discover, revocation via export removal |
| **Bridge / HTTPS clients** | `a2a-bridge` connects to caller's Account like any NATS client | Bridge does **not** bypass federation — it mints into the caller Account and sees the same shaped discover responses as native clients |

**Rollout guidance:** ship and harden single-Account discovery (Phase 0 catalog + Phase 1 gateway shaping) before enabling exports. Federation adds operator workflow and SpiceDB tuple complexity, not a replacement for local KV/registrar correctness.

---

## Trust boundaries

Federation exposes **AgentCard metadata only** across Accounts — not task RPC, streaming, or push subjects.

```
┌─────────────────────────────┐         export/import          ┌─────────────────────────────┐
│  Publisher Account (Org A)  │  ──── {prefix}.discover.> ────► │  Consumer Account (Org B)   │
│                             │         (operator-signed)       │                             │
│  A2A_AGENT_CARDS (auth)     │                                 │  A2A_AGENT_CARDS (auth)     │
│  DiscoverService            │                                 │  DiscoverService (local)    │
│  Registrar (writes KV)      │                                 │  Gateway + SpiceDB shaping  │
└─────────────────────────────┘                                 └─────────────────────────────┘
         ▲                                                                 ▲
         │ local register only                                           │ local invoke only
         │                                                                 │
    Org A agents                                                    Org B callers / gateway
```

| Boundary | Trusted party | What crosses it | What stays local |
|----------|---------------|-----------------|------------------|
| **Account (default)** | Operator JWT hierarchy | Nothing — zero cross-tenant visibility | All A2A subjects |
| **Discover export** | Operator signs export on publisher Account | `{prefix}.discover.>` request/reply traffic only | `{prefix}.catalog.register.*`, KV puts, `{prefix}.gateway.>`, `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` |
| **Gateway (consumer Account)** | Org-standard SpiceDB + Tier 1 bundle | Filtered AgentCard JSON to authorized callers | Raw imported replies never bypass shaping |
| **Registrar** | Publisher Account only | Writes to publisher's `A2A_AGENT_CARDS` | Cannot register agents into a consumer Account's KV via federation |

**Enterprise default:** no exports configured → callers see only agents registered in their own Account. Federation is an explicit operator contract, not a runtime default.

---

## Operator-signed Account export contract

Cross-Account discovery uses NATS **Account JWT exports/imports** ([NATS exports/imports](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_intro/jwt/resolver)). The operator signs both sides; application code does not mint export JWTs.

### Publisher Account (exports AgentCards)

1. **Export subject:** `{prefix}.discover.>` (default prefix `a2a` → `a2a.discover.>`).
2. **Export type:** **Service** export (request/reply) — `DiscoverService` in `a2a-nats-discovery` handles `{prefix}.discover.*` and replies from KV.
3. **Accounts allowed to import:** explicit list of consumer Account public keys (no wildcard import in production).
4. **Local ACL unchanged:** registrar User retains `{prefix}.catalog.register.*` + KV write; export does **not** grant foreign Accounts register or KV access.

Illustrative `nsc` outline (placeholders — follow current NATS docs for exact flags):

```bash
# On publisher Account — export discover service to consumer Account(s)
nsc add export -a [PUBLISHER_ACCOUNT] \
  --name a2a-discover \
  --service --subject "a2a.discover.>" \
  --accounts [CONSUMER_ACCOUNT_PUBKEY,...]
```

### Consumer Account (imports foreign catalogs)

1. **Import subject:** `{prefix}.discover.>` from the named publisher Account(s).
2. **Local token / mapping:** consumer-side configuration records `(publisher_account_id → import_name)` for gateway federation resolution (implementation detail — not a second KV copy).
3. **No automatic KV replication:** import delivers **live discover request/reply** to the publisher's `DiscoverService`; consumer Account KV remains authoritative **only for locally registered agents**.

```bash
# On consumer Account — import discover from publisher
nsc add import -a [CONSUMER_ACCOUNT] \
  --name a2a-discover-from-[PUBLISHER] \
  --service --subject "a2a.discover.>" \
  --account [PUBLISHER_ACCOUNT_PUBKEY] \
  --remote-subject "a2a.discover.>"
```

### Contract invariants

| Invariant | Rationale |
|-----------|-----------|
| Export **`{prefix}.discover.>` only** | AgentCards are semi-static metadata; task/push paths stay Account-local per plan §Decisions |
| Operator signs both export and import | Prevents tenant self-service cross-org visibility |
| Publisher registrar remains sole KV writer | Schema validation (`a2a-pack`) and ACL posture stay on the owning Account |
| Revocation = remove export or import | No application-level "soft delete" across Accounts — operator JWT change is the kill switch |
| AgentCard `transports` must declare reachable endpoints | Federated card may advertise HTTPS or NATS URLs in **publisher** infrastructure; consumer gateway still enforces invoke auth separately |

Detailed per-Account ACL templates for registrar/gateway/caller Users: [`./A2A_NSC_ACCOUNT_BOOTSTRAP.md`](./A2A_NSC_ACCOUNT_BOOTSTRAP.md). Cross-Account federation steps remain **out of scope** for that bootstrap outline until Phase 4 automation lands.

---

## Authoritative catalog KV vs imported discover

Two distinct read paths coexist in a federated consumer Account:

| Path | Source | Write path | Read API | Federation role |
|------|--------|------------|----------|-----------------|
| **Local authoritative** | `A2A_AGENT_CARDS` KV in consumer Account | `{prefix}.catalog.register.{agent_id}` → `CatalogRegistrarService` → `KvCatalogStore::put_card` | KV get/watch, `{prefix}.discover.{agent_id}` (local `DiscoverService`) | Native agents for this org |
| **Imported (federated)** | Publisher Account's KV (via publisher `DiscoverService`) | **None in consumer Account** — read-only via NATS import | Gateway-initiated discover to imported `{prefix}.discover.{agent_id}` on publisher Account | Foreign org agents |

**Design rule:** the consumer Account **does not mirror** federated AgentCards into local KV. Reasons aligned with [`../A2A_PLAN.md`](../A2A_PLAN.md) §AgentCard discovery:

- **Single writer per card** — publisher registrar + schema validation remain authoritative; stale replicas in consumer KV would fight KV watch semantics.
- **Revocation stays operator-simple** — drop import → federated agents disappear without tombstone sweeps in consumer KV.
- **SpiceDB shaping applies at merge time** — gateway composes "visible catalog" from local discover + allowed imported discovers after permission checks, not from a pre-merged KV snapshot.

### Client-visible catalog merge (gateway)

When Phase 1 catalog shaping ships on `a2a-gateway`:

1. Enumerate **local** agent ids (KV keys or configured catalog list source).
2. For each configured **federation import**, resolve candidate agent ids (operator-configured allowlist or lazy per-id discover — product choice at implementation).
3. **`BulkCheckPermission`** — `user:{sub}` / `view` / `agent:{agent_id}` for each candidate, including federated ids namespaced per `a2a-pack` tuple conventions (e.g. `agent:{publisher_account}:{agent_id}` — exact encoding belongs in bundle).
4. Return shaped AgentCard list / single-card discover responses; deny or omit unauthorized federated entries.

Local `DiscoverService` in `a2a-nats-discovery` remains a thin KV read for **non-gateway** callers inside the Account; gateway federation merge is an **ingress policy** concern per [`./A2A_GATEWAY_ROADMAP.md`](./A2A_GATEWAY_ROADMAP.md).

---

## SpiceDB at the federation boundary

SpiceDB gates the **import side** — only authorized callers see imported AgentCards ([`../A2A_PENDING_DECISION.md`](../A2A_PENDING_DECISION.md) §8). The gateway holds the org-standard SpiceDB client ([`../A2A_PLAN.md`](../A2A_PLAN.md) §SpiceDB, [`./A2A_GATEWAY_ROADMAP.md`](./A2A_GATEWAY_ROADMAP.md) §Coordination with SpiceDB Tier 1).

### Tuple model (sketch)

| Scenario | Subject | Permission | Resource |
|----------|---------|------------|----------|
| Local discover / list | `user:{sub}` | `view` | `agent:{agent_id}` |
| Federated discover | `user:{sub}` | `view` | `agent:{publisher_account}:{agent_id}` (bundle-defined) |
| Invoke federated agent (separate from discover) | `user:{sub}` | `invoke` / `invoke_stream` | Same federated resource — discover visibility ≠ invoke permission |

**BulkCheckPermission** reuses the Phase 1 catalog-shaping path: federation adds federated resource ids to the bulk check set; ZedToken cache remains per-session.

### Boundary behavior

| Stage | Check | On deny |
|-------|-------|---------|
| **Gateway discover/list** | SpiceDB before returning imported AgentCard | Omit card or return not-found (policy bundle chooses fail-closed vs silent omit) |
| **Gateway invoke** | Standard method tuples — federated agent invoke requires explicit `invoke` grant even if `view` passed | JSON-RPC authz error + ingress audit |
| **Direct NATS import abuse** | Subject ACL prevents callers from subscribing to imported discover without going through gateway for shaped list; raw import is service-level wiring | NATS layer denies unauthorized Accounts |

Cross-Account **SpiceDB principals carry Account identity** in JWT `data` (auth callout sketch) so federation checks can attribute which consumer org the caller belongs to when evaluating cross-org `view` grants.

Audit: federated discover allow/deny uses the same ingress `AuditEnvelope` shape as local discover once gateway decision sites land — include `rules_fired` for federation policy ids and the federated resource id.

---

## Interaction with `a2a-bridge`

[`./A2A_BRIDGE_SKETCH.md`](./A2A_BRIDGE_SKETCH.md) covers HTTPS clients that re-mint into the **caller's tenant Account**. Federation does not change bridge placement:

- Bridge publishes on `{prefix}.gateway.{agent_id}.{method}` inside the caller Account.
- Discover for HTTPS clients flows through the same gateway/catalog path as native NATS clients.
- Multi-cluster bridge deployments still connect **one Account per tenant**; cross-region discover visibility is governed by Account export contracts, not by bridge routing.

Invoke of a **federated** agent may require bridge or gateway to reach the **publisher Account's** gateway (separate export decision — out of scope for discover-only federation). Phase 4 cross-binding tests in [`../A2A_TODO.md`](../A2A_TODO.md) validate discover + invoke stories once `a2a-bridge` exists.

---

## Explicit non-goals

| Area | Owner / note |
|------|----------------|
| KV replication across Accounts | Rejected — imported discover is live request/reply |
| Federated `{prefix}.gateway.>` / task / push exports | Not part of discover federation; separate operator decision if ever needed |
| Scatter-gather discovery | Health/`$SRV.PING` tooling is Phase 4 ops, not the catalog primitive ([`../A2A_PLAN.md`](../A2A_PLAN.md)) |
| Automated `nsc` export/import CI | Future operator tooling; this sketch is the contract |
| Registrar changes for federation | Publisher `a2a-nats-discovery` unchanged; consumer may run local registrar for native agents only |

---

## Implementation tracker

See [`../A2A_TODO.md`](../A2A_TODO.md) §Phase 4:

- [ ] Federated discovery — operator-signed Account export contract for `{prefix}.discover.>`; SpiceDB gating at import boundary
- [ ] `a2a-bridge` crate (prerequisite for cross-binding collaboration tests)
- [ ] Cross-binding collaboration tests

**Suggested ordering** (from [`../A2A_TODO.md`](../A2A_TODO.md)):

1. Single-Account catalog hardening — registrar ACL, schema validation, gateway `BulkCheckPermission` on **local** agents (Phase 0–1).
2. Document and test operator export/import contract in staging (two Accounts).
3. Gateway federation merge + federated SpiceDB tuples in `a2a-pack`.
4. `a2a-bridge` + end-to-end cross-org discover/invoke tests.

# ADR 0001: Tenancy Model

| Field | Value |
|-------|-------|
| **Status** | Accepted (2026-05-27) |
| **Date** | 2026-05-27 |
| **Deciders** | *(author / platform security — TBD)* |
| **Blocks** | Tenancy model decision for MCP gateway deployment topology |
| **Related** | NATS subject topology and tenancy boundary; A2A operator docs (account-per-tenant already landed for A2A) |

## Context

Multi-tenant MCP and A2A traffic needs a single, durable answer to: **where does the tenant boundary live?** The subject grammar is already fixed — tenancy is **not** a subject segment (`mcp.gateway.request.{server_id}.{method}` carries no `{tenant}` token). Tenant identity must therefore live in NATS infrastructure (accounts), in JWT claims, or in both.

This decision gates everything in the agent-identity program:

- **STS** must know whether `aud` names a backend inside one tenant namespace or across tenants.
- **`act_chain`** entries must be resolvable in a registry scoped per tenant.
- **Audit** retention, SIEM export, and agent-traffic dashboards partition by tenant.
- **SpiceDB** principals and resource tuples must not leak across customers.
- **JetStream** streams (`MCP_AUDIT`, session KV, agent registry KV, trust bundles) and **NATS KV** config buckets need a placement rule.
- **Subject ACLs** today are caller-bounded (`mcp.gateway.>`, `_INBOX.{caller_id}.>`) but do not encode tenant; a wrong tenancy model lets one tenant's JWT authorize another tenant's subjects if accounts are shared.

**What breaks if this stays undecided:**

- Block 1 (agent registry, workload attestation) ships with ambiguous key namespaces and cross-tenant lookup risk.
- Block 2 (STS exchange, `act_chain`) cannot pin audience naming or chain verification scope.
- Block 4 (agent-centric observability) cannot define retention, indexes, or "top agents per tenant" without rework.
- MCP gateway Phase 2+ (hierarchical policy merge, cluster-wide rate limits) will fork implementations for "one cluster, many tenants" vs "one tenant per account."

**Current state (branch snapshot):**

- **A2A** operator docs and NSC bootstrap already assume **one NATS Account per tenant** (hard isolation); subjects are identical across accounts; the account is the boundary.
- **MCP gateway** validates JWT `sub`, `tenant` (claim key configurable), strips forgeable `trogon-mcp-tenant` on egress, and optionally scopes audit envelopes by tenant header/claim — within a **shared** NATS account in dev-style deployments.
- **Auth callout** mints NATS User JWTs with subject permissions, not account boundaries.

Until ADR 0001 is accepted, treat tenancy as **provisional**: implement tenant claims in envelopes and CEL, but do not assume a single production topology.

## Decision drivers

| Driver | Why tenancy model matters |
|--------|---------------------------|
| **STS audience naming** | `aud` must resolve to a gateway/backend identity unambiguous within the tenant; cross-tenant `aud` is either impossible (hard) or explicitly federated (hybrid). |
| **`act_chain` shape** | Chain entries reference `sub`, `agent_id`, `wkl`; registry and STS must validate within one tenant context; loop detection and depth caps are per-tenant policies. |
| **Agent registry namespacing** | KV keys (`mcp-agent-registry`) or external store IDs must not collide across customers; revocation is tenant-scoped. |
| **Audit retention scoping** | JetStream `MCP_AUDIT` limits, SIEM consumers, and hash chains partition per account or per tenant-keyed stream. |
| **SpiceDB principal naming** | `user:{sub}` vs `tenant:{t}/user:{sub}` — tuple namespace must match isolation guarantees. |
| **JetStream stream layout** | One `MCP_AUDIT` per account (natural) vs one stream with tenant in envelope only (requires careful ACL on publish). |
| **Subject ACLs** | Hard tenancy: ACL templates repeat per account; soft tenancy: all tenants share subject space — isolation is **only** JWT + policy. |
| **KV namespaces** | `mcp-gateway-config`, `mcp-sessions`, trust bundles, schema cache — per-account buckets vs single bucket with `{tenant}/` key prefix. |

**Explicit non-goal:** Adding `{tenant}` to MCP/A2A subject paths. That remains out of scope per `MCP_GATEWAY_PLAN.md`.

## Considered options

### (a) NATS account per tenant (hard isolation)

Each customer (tenant) receives a dedicated NATS Account under the Operator. Subject strings are **identical** in every account (`mcp.gateway.request.github.tools.call`). Isolation is enforced by NATS: accounts cannot see each other's subjects without signed export/import. Gateway, auth callout, JetStream assets, and KV buckets are provisioned **per account** (mirrors landed A2A pattern in `docs/a2a/how-to/operators/nsc-account-bootstrap.md`).

JWT still carries a **tenant claim** for audit legibility, SpiceDB principal naming, and cross-account SIEM aggregation — not for subject routing.

### (b) Tenant as JWT claim (soft isolation)

Single NATS Account (or few shared accounts) for many tenants. Tenant id is a validated JWT claim (`tenant` / `https://trogon.ai/tenant`); gateway strips client-supplied tenant headers. Isolation is **application-layer**: CEL, SpiceDB, rate limits, registry lookups keyed by `jwt.tenant`. Optional `MCP_PREFIX` per deployment (`acme.mcp` vs `globex.mcp`) partitions subjects without full account ops — useful for labs, not a substitute for customer isolation.

### (c) Hybrid

**Production:** account per tenant (a). **Dev / single-customer / internal:** single account + tenant claim (b). **Escape hatch:** `MCP_PREFIX` override for coarse partitioning on one cluster without NSC churn. Code paths accept both: `nats.account` from connection context **and** `jwt.tenant` from claims; policy and audit **must** record both when present. Cross-tenant features (support tools, global SIEM) use explicit account export/import or a dedicated operator-level aggregator — never implicit subject overlap.

## Options comparison

### (a) Account per tenant

| Pros | Cons |
|------|------|
| Strongest blast-radius containment: compromised JWT cannot publish outside the account's subject space. | **Irreversible ops model:** NSC/Operator provisioning per customer; automation and billing attach to Account lifecycle. |
| JetStream retention, KV buckets, and stream names scope naturally (no tenant prefix in every key). | Gateway HA story is "gateway per account" or account imports — more moving parts than one shared gateway pool. |
| Aligns with A2A Phase 0 already documented; one tenancy story across protocols. | Cross-tenant analytics requires export to operator account or external warehouse — not ad hoc SQL on one stream. |
| Subject ACL templates are copy-paste per tenant; no "tenant A publishes to tenant B's server_id" path. | |

**Irreversible consequences:** Operator/account topology, per-tenant JetStream asset names (same logical name, different account), and inability to "merge" two production tenants into one account without migration. Federation between tenants becomes an explicit export/import design, not a config toggle.

### (b) Tenant as JWT claim

| Pros | Cons |
|------|------|
| Fastest local dev and smallest NATS footprint; matches current MCP gateway tests and Phase 1 harness. | **Irreversible weakness if chosen for production:** subject space is shared — a bug in ACL minting or gateway bypass exposes all tenants. |
| One gateway queue group serves all tenants; cluster-wide rate limits and config KV are simpler. | Every datastore must tenant-prefix keys (`{tenant}/agent_id`); a missed prefix is a data leak. |
| SpiceDB can use one deployment with tenant in tuple id — familiar to app teams. | JetStream `MCP_AUDIT` published by gateway must be trusted to set `tenant` correctly; consumers filter, not enforce. |

**Irreversible consequences:** If production ships on soft tenancy first, migrating to hard tenancy requires re-homing connections, re-provisioning streams/KV per account, and rewriting registry/audit indexes — a **data migration**, not a flag flip.

### (c) Hybrid

| Pros | Cons |
|------|------|
| Production isolation without forcing developers through NSC for every laptop. | Two code paths to test; bugs may appear only in one mode (e.g. missing `nats.account` in CEL). |
| Matches `MCP_GATEWAY_PLAN.md` default recommendation. | Operators must document which mode each environment uses; drift causes "works in dev, leaks in prod" incidents. |
| JWT tenant claim stays valuable for SIEM and SpiceDB even under hard tenancy. | |

**Irreversible consequences:** The **abstraction boundary** (always thread `TenantContext { account, claim }` through gateway, STS, audit) must be designed once; retrofitting later is expensive. Choosing hybrid commits to dual-mode tests in CI, not to "soft only."

## Decision

**Adopt (c) hybrid**, with a clear environment split:

| Environment | Mode | Rationale |
|-------------|------|-----------|
| Production multi-customer | **(a) account per tenant** | Matches A2A; meets blast-radius and compliance expectations; JetStream/KV scoping is native. |
| Dev / CI / single-tenant deploy | **(b) JWT claim** (optional `MCP_PREFIX`) | Minimizes NSC friction; gateway and auth callout already emit/consume tenant claims. |
| Cross-protocol | **Same rule per tenant** | A2A and MCP for customer X should live in the **same** NATS Account where both are offered — one tenant boundary, two subject trees (`a2a.*`, `mcp.*`). |

**Rationale in one line:** Subject topology is fixed and tenant-free; **hard isolation belongs in the NATS Account** for production, while **JWT tenant remains the portable identity field** for policy, audit, and SpiceDB — never the sole production guardrail.

Implementation bias for downstream specs:

1. **Authority order:** `nats.account` (when distinct accounts) > validated `jwt.tenant` > never client headers.
2. **Registry / STS / audit:** APIs accept `tenant_id` derived from that context; keys in soft mode use `{tenant_id}/…` prefix; in hard mode, account id may equal tenant id or map 1:1 via config.
3. **SpiceDB:** Prefer `tenant:{tenant_id}/user:{sub}` (and `tenant:{tenant_id}/agent:{agent_id}`) even in hard mode so SIEM and SpiceDB stay aligned when aggregating across accounts.

## Consequences

### Block 1 — Agent identity primitives

- **Agent registry:** Hard mode — bucket `mcp-agent-registry` per account (same name, isolated). Soft mode — single bucket, keys `"{tenant}/{agent_id}"`. STS lookup must pass tenant context; acceptance test ("workload cannot claim agent X") is per-tenant.
- **Workload attestation:** Trust bundle KV per account in hard mode; SPIFFE trust domain may be per-tenant (`spiffe://{tenant}.…`) — document in ADR 0002 follow-on.
- **JWT claims:** `tenant` claim **required** in audit and CEL in both modes; under hard tenancy it must **match** the account mapping table even though subjects do not carry it.

### Block 2 — Per-hop token exchange and delegation

- **STS `aud`:** Names a target **within the same tenant** as the subject token (e.g. `mcp-gateway`, `mcp-server-github`, `agent:{id}`). Cross-tenant exchange is out of scope unless export/import federation is designed separately.
- **`act_chain`:** Entries do not include tenant in each object if chain is evaluated inside one account connection; for soft mode, add `tenant` to outer JWT only. Registry validation at each hop uses tenant-scoped registry view.
- **Gateway egress:** STS call includes tenant context from connection + JWT; cache key includes `tenant_id` (and `account` if hybrid).

### Block 4 — Observability for agentic traffic

- **Audit indexes:** Primary partition key = `tenant` (claim); secondary = `nats.account` when aggregating operator-wide.
- **Retention:** Per-account JetStream limits in hard mode; per-tenant quota config in soft mode.
- **Dashboards:** "Top agents per tenant" uses envelope `tenant`; cross-tenant dashboards only on operator/SIEM tier with explicit export.

## Open questions (for author / deciders)

1. **1:1 mapping:** Is `tenant_id` always equal to NATS Account name, or can one Account host multiple logical tenants (sub-tenants)?
2. **Shared gateway pool:** In hard mode, does one gateway process connect to multiple accounts via imports, or one gateway deployment replica set per tenant?
3. **SpiceDB topology:** One SpiceDB cluster with tenant in tuple id vs per-tenant SpiceDB — cost vs isolation.
4. **Auth callout deployment:** Per-account callout service vs central callout that mints Users into the correct account — affects blast radius and key custody.
5. **Bootstrap credential:** Does the existing auth-callout User JWT remain valid **only** inside the caller's account, or can one callout mint for many accounts (central IdP pattern)?
6. **Tenant claim under hard tenancy:** Required always, or only when SIEM/SpiceDB aggregation needs a stable string distinct from account name?
7. **`MCP_PREFIX` vs account:** When should operators use prefix override instead of a new Account — internal envs only, or a supported multi-tenant lite offering?
8. **A2A + MCP unification:** Confirm single Account hosts both `a2a.*` and `mcp.*` for one customer — any exception for regulated split?
9. **Migration:** Existing single-account pilots — documented cutover to per-account (streams, KV, connections) before enforce-mode agent identity?
10. **Rate limits:** Per-tenant inflight (`MCP_GATEWAY_PLAN.md` defaults) — cluster-wide KV in soft mode vs per-account semaphores only in hard mode?

---

*This document is a DRAFT for human review. Status must not move to Accepted until open questions are resolved and Block 0 sign-off is recorded.*

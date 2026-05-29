# Agent Registry

**Status:** DRAFT — for human review. Blocks implementation until Block 0 ADRs land.


---

## Purpose

The Agent Registry is the **authoritative record** that binds an `agent_id` to everything downstream identity services need to authorize agent behavior:

| Binding | Consumer |
|---|---|
| Owner team | Audit, SIEM, ownership escalation |
| Version + definition digest | Drift detection, supply-chain attestation |
| Allowed workloads (SPIFFE IDs) | STS: "this workload may act as this agent" |
| Allowed tools (coarse capabilities) | Gateway CEL, STS scope narrowing |
| Allowed audiences | STS: permitted `aud` values per [ADR 0005](../adr/0005-token-ttl-and-audience.md) |
| Allowed purposes | STS + gateway: permitted `purpose` values per Block 2.3 |
| Lifecycle state | Fail-closed on deprecated/revoked agents |

The registry does **not** mint tokens, attest workloads, or enforce policy. It is the read-mostly catalog that STS, the gateway, and operators consult. Every mutation is auditable.

**Acceptance criterion:** a workload (SPIFFE ID) cannot present `agent_id=X` to the STS unless the registry confirms the SPIFFE ID is in `allowed_workloads` for `X`.

---

## Identity layering (Block 0 dependency)

Block 0 defines three layers that the registry sits between:

```
user_sub (originator)  →  act_chain entries
agent_id (registry)    →  this document
workload_id / wkl      →  SPIFFE attestation (Block 1.2)
```

The registry owns **`agent_id` semantics only**. Whether `agent_id` is mandatory on every mesh JWT, how humans and batch jobs appear in `act_chain`, and how tenancy scopes registry entries are **Block 0 outputs** that this spec parameterizes but does not decide.

**Tenancy interaction (preliminary):**

| Tenancy model (ADR 0001) | Registry scoping |
|---|---|
| NATS account per tenant (hard) | One KV bucket `mcp-agent-registry` per account; `agent_id` slug may omit tenant prefix because the account is the boundary |
| Tenant as JWT claim (soft) | Single bucket; KV keys are fully-qualified `agent_id`; `jwt.tenant` must match the record's tenant segment |
| Hybrid | Org-wide base manifest + per-account override manifests layered into KV (mirrors `mcp-gateway-config` pattern) |

Until ADR 0001 is ratified, manifests and examples use **fully-qualified** ids (`acme/oncall-agent`) so the same record shape works in either mode.

---

## Entity schema

### Record shape

One registry entry = one **versioned** agent definition. The latest non-revoked version is the default lookup target; historical versions remain addressable for audit replay.

```json
{
  "agent_id": "acme/oncall-agent",
  "agent_version": "3.2.1",
  "agent_definition_digest": "sha256:abc123…",
  "owner_team": "platform-sre",
  "allowed_workloads": [
    "spiffe://acme.local/ns/prod/sa/oncall-agent"
  ],
  "allowed_tools": [
    "pagerduty.page",
    "github.issues.read",
    "slack.post_message"
  ],
  "allowed_audiences": [
    "mcp.server.pagerduty",
    "mcp.server.github",
    "agent:acme/incident-coordinator"
  ],
  "allowed_purposes": [
    "incident.response",
    "incident.triage"
  ],
  "metadata": {
    "description": "Pages on-call and opens incident threads",
    "repo": "https://github.com/acme/oncall-agent",
    "contact": "platform-sre@acme.example"
  },
  "lifecycle_state": "active"
}
```

### Field definitions

| Field | Type | Required | Description |
|---|---|---|---|
| `agent_id` | string | yes | Stable agent identifier. See § Agent ID format. |
| `agent_version` | string | yes | Semver or monotonic build id (`1.4.0`, `20260527T055911Z`). |
| `agent_definition_digest` | string | yes | Content hash of the agent definition artifact (container digest, WASM hash, or Git tree SHA). Format: `{algo}:{hex}`. |
| `owner_team` | string | yes | Owning team slug for escalation and manifest signing ACLs. |
| `allowed_workloads` | string[] | yes | SPIFFE IDs permitted to present this `agent_id` at STS. Empty array = no workload may act as this agent (registration-only / suspended). |
| `allowed_tools` | string[] | yes | Coarse capability names (not MCP tool URIs). Gateway and STS intersect requested scope with this set. Convention: `{backend}.{capability}` or `*` for privileged agents (discouraged). |
| `allowed_audiences` | string[] | yes | Permitted STS `audience` values this agent may request. Aligns with ADR 0005 granularity (`mcp.server.{id}`, `agent:{agent_id}`, regional STS audiences). Empty = deny all exchange. |
| `allowed_purposes` | string[] | yes | Enumerated purpose strings this agent (or downstream refiners) may carry. See Block 2.3. Empty = agent may not set `purpose` (inherit-only from upstream chain). |
| `metadata` | map | no | Free-form operator annotations. Not used in authorization. |
| `lifecycle_state` | enum | yes | `active` \| `deprecated` \| `revoked`. See § Lifecycle. |

### Agent ID format — recommendation

**Recommend: namespaced slug `{tenant}/{agent_slug}`.**

| Segment | Pattern | Example |
|---|---|---|
| `tenant` | `[a-z0-9][a-z0-9-]{0,62}` | `acme` |
| `agent_slug` | `[a-z0-9][a-z0-9-]{0,62}` | `oncall-agent` |
| Full id | `{tenant}/{agent_slug}` | `acme/oncall-agent` |

**Rationale:** Human legibility in audit; tenancy alignment with `jwt.tenant` and SpiceDB (`agent:acme/oncall-agent`); consistency with A2A AgentCard KV keys; parallels SPIFFE path-shaped ids.

**Alternative considered: opaque UUID.** Rejected as primary format — pushes human context into `metadata`, degrades audit readability, still requires a tenant column. UUIDs may live in `metadata.internal_id` for CMDB correlation.

**Validation:** exactly one `/`; no `.` characters; lowercase at lint time; immutable once registered (new identity → new `agent_id`).

### Versioning

- KV stores **one key per `(agent_id, agent_version)`** plus a **`{agent_id}/@latest` pointer** updated atomically on each version bump.
- Lookup without `version` resolves `@latest` → highest semver (or lexicographic for non-semver build ids, configurable per tenant).
- STS and gateway cache the resolved record keyed by `(agent_id, agent_version)`.

---

## Storage

### Source of truth: Git manifests

Agent records live in version-controlled YAML under a repo path such as:

```
agents/
  acme/
    oncall-agent.yaml      # one file per agent_id; versions appended or replaced per bump policy
  globex/
    billing-agent.yaml
```

Manifest properties:

- Signed by an org-controlled NKey.
- PR-reviewed; CI lints schema, SPIFFE ID format, and `allowed_*` cross-references.
- Merge to `main` triggers the control-plane sync.

### Runtime cache: NATS KV

| Property | Value |
|---|---|
| Bucket | `mcp-agent-registry` (override via `MCP_PREFIX` → `{prefix}-agent-registry` if ADR 0001 selects prefix partitioning) |
| Key | `{agent_id}/{agent_version}` (URL-encoded `/` → `%2F` if the KV client requires flat keys) |
| Pointer key | `{agent_id}/@latest` |
| History | JetStream KV revision history retained for rollback forensics |
| Tenancy | One bucket per NATS account (hard tenancy) or tenant-prefixed keys (soft tenancy) |

A **registry controller** (NATS service on the control-plane subject namespace) watches Git (or OCI/K8s CRDs) and projects signed manifests into KV. The controller is the **only writer** to the bucket at runtime; agents and gateways are read-only.

Hot-path consumers (STS, gateway) hold an in-memory snapshot fed by KV watch + periodic full reconcile. Target refresh latency: < 5 s P99 under normal churn.

### Relationship to A2A AgentCard catalog

`A2A_AGENT_CARDS` holds **discovery metadata** (routing); this registry holds **identity bindings**. Same `agent_id` in both; catalog registration alone does not grant mesh identity.

---

## Lifecycle operations

All mutations flow **Git → signed manifest → controller → KV → audit**. No runtime API for agents to self-register.

| Operation | Manifest action | `lifecycle_state` | Effect |
|---|---|---|---|
| **Registration** | Add new `{agent_id}` with initial version | `active` | Workloads in `allowed_workloads` may obtain mesh tokens |
| **Version bump** | New `agent_version` + updated digest / allow-lists | `active` | `@latest` pointer moves; prior version retained |
| **Deprecation** | Set state on current version (or all versions) | `deprecated` | STS may mint with short TTL + audit warning; gateway shadow mode logs; enforce mode rejects new exchanges |
| **Revocation** | Set state; optionally clear `allowed_workloads` | `revoked` | Immediate fail-closed: STS and gateway reject all new tokens for this id |

### Who can write?

| Actor | Write path | Notes |
|---|---|---|
| Platform / control-plane automation | Signed manifest merge → controller | Primary path |
| Emergency break-glass | Direct KV put with operator NKey + mandatory audit | Requires dual control; pointer rollback documented in runbook |
| Agents, gateways, STS | **None** | Read-only via lookup API or KV watch |
| Auth callout | **None** | Bootstrap JWT does not embed registry state |

Manifest signing ACLs are keyed on `owner_team` + platform admin role. An agent team can propose changes via PR; merge requires platform approval for first registration and for `allowed_audiences` expansion.

---

## Lookup API

Request/reply over NATS. Available to gateway, STS, SDK (self-introspection), and control-plane tooling.

### Subject

```
mcp.registry.agent.lookup
```

Queue-group consumer on the registry service. Callers publish a request and receive a reply on their inbox. Subject ACL: gateway, STS, and control-plane principals only — not edge callers.

### Request

```json
{
  "agent_id": "acme/oncall-agent",
  "version": "3.2.1"
}
```

| Field | Required | Description |
|---|---|---|
| `agent_id` | yes | Namespaced agent identifier |
| `version` | no | Pin a specific version. Omit → resolve `@latest`. |

### Response — success

HTTP-equivalent status in envelope; reply body is the full record (schema in § Entity schema) plus envelope fields:

```json
{
  "status": "ok",
  "record": { "...": "..." },
  "resolved_version": "3.2.1",
  "kv_revision": 42
}
```

`kv_revision` lets consumers detect stale caches.

### Response — NACK

```json
{
  "status": "not_found",
  "agent_id": "acme/oncall-agent",
  "reason": "no matching version"
}
```

| `status` | When |
|---|---|
| `not_found` | Unknown `agent_id` or version |
| `revoked` | Record exists but `lifecycle_state=revoked` |
| `deprecated` | Record exists, deprecated (caller decides whether to proceed — STS treats as policy input) |
| `error` | Registry service internal failure |

NACK responses are idempotent and carry no partial record. Consumers cache by `(agent_id, version)` with KV-watch invalidation; SDK local TTL ≤ 60 s.

---

## Audit

Every registry mutation emits an audit envelope to:

```
mcp.audit.registry.{outcome}
```

Where `{outcome}` follows the gateway audit subject grammar: `allow` for successful sync, `deny` for rejected manifest, `error` for controller failure.

Envelope payload (minimum):

| Field | Description |
|---|---|
| `event_type` | `register` \| `version_bump` \| `deprecate` \| `revoke` \| `sync` |
| `agent_id`, `agent_version` | Target record |
| `prior_lifecycle_state`, `new_lifecycle_state` | State transition |
| `manifest_digest` | Git/OCI artifact hash |
| `operator` | PR author or break-glass NKey id |
| `kv_revision` | Post-write revision |
| `trace_id` | Correlation with CI pipeline |

Lookup operations are not audited per call (volume). STS exchange audit captures `kv_revision` used at exchange time. Stream: `MCP_AUDIT`, filter `mcp.audit.>`.

---

## Bootstrap

### Fresh cluster

1. **Operator provisions KV bucket** `mcp-agent-registry` (per account / tenant policy).
2. **Seed manifest** in Git: platform agents (`registry-controller`, `sts`, `gateway`) plus any day-zero agents required for smoke tests.
3. **Controller first sync** writes seed records and `@latest` pointers.
4. **Trust chain**: controller NKey registered in operator config; manifest signature verification key in KV `mcp-gateway-config` or embedded in controller.
5. **Auth callout unchanged** — continues minting bootstrap User JWTs with `sub`, `caller_id`, `aud` (NATS account), and subject ACLs. It does **not** read the registry.

### Bootstrap JWT → mesh token handoff

Per ADR 0003:

```
Connect:  auth-callout JWT  (broad NATS ACL, no agent_id required)
Act:      STS exchange      (registry lookup + SVID → mesh JWT with agent_id, aud, act_chain)
```

During Phase 0 coexistence (`MCP_GATEWAY_AGENT_IDENTITY=off`), the registry may be populated and audited but STS enforcement is inactive. Shadow mode validates registry lookups and logs mismatches without rejecting.

### Day-zero agents

Minimum seed set:

| `agent_id` | Role |
|---|---|
| `{org}/registry-controller` | Sync service identity |
| `{org}/sts` | Token exchange service |
| `{org}/mcp-gateway` | Gateway egress mint path |

Each entry includes the service's SPIFFE ID in `allowed_workloads`.

---

## Failure modes

| Condition | Recommended behavior | Rationale |
|---|---|---|
| Registry KV unreachable | **Fail-closed** for STS exchange and gateway enforce mode | Cannot verify `allowed_workloads`; accepting would violate acceptance criterion |
| Registry unreachable, shadow mode | Allow bootstrap path; log `registry_miss` audit event | Migration safety |
| Stale in-memory cache (watch lag) | Serve cached record if `kv_revision` ≤ last seen; if lookup API returns higher revision, refresh synchronously | Balance availability vs correctness |
| Cached read fallback after TTL expiry | **Do not** extend TTL silently beyond 2× configured refresh interval | Prevents indefinite stale allow-lists after revocation |
| Controller down, KV static | Continue serving last synced state | Git SoT unchanged; revocation delayed until controller returns |
| Manifest signature invalid | Reject sync; emit `mcp.audit.registry.deny` | Tamper evidence |
| Break-glass KV write without Git | Allowed for revocation only; triggers page to `owner_team` | Emergency containment |

STS rate limiting during registry outage: circuit-break after N consecutive lookup failures; return `503`-class NACK on `mcp.sts.exchange` (see Block 2.1).

---

## Open questions

1. **ADR 0001 tenancy** — Does hard tenancy allow tenant-less `agent_id` slugs inside an account, or always require fully-qualified ids for SIEM portability?
2. **ADR 0003 bootstrap** — Can a bootstrap JWT carry an optional `agent_id` hint for shadow validation before STS exists, or must the hint always be ignored?
3. **`allowed_audiences` vs policy bundles** — Is the registry the sole source of audience ACLs, or do SpiceDB relations further narrow per agent? (Block 2.1 lists both options.)
4. **Historical lookup** — When verifying `act_chain`, must entry `iat` resolve against the registry version valid at that timestamp, or current state only?
5. **Definition digest scope** — Container image only, or include policy bundle hash and tool manifest hash in a composite digest?
6. **Multi-region** — Is KV replicated cross-region, or regional registry with global Git SoT and async sync (affects revocation SLA)?
7. **Gateway `agent_id`** — Does the gateway have its own registry entry, or is it identified purely by SPIFFE + privileged role?
8. **Batch / non-interactive jobs** — Registry entry shape for jobs without a human originator: sentinel purpose set? Separate `agent_id` namespace?
9. **Event sourcing** — Should `trogon-decider` own registry state as an aggregate, or is Git+KV projection sufficient?
10. **OAuth-MCP composition** — Does OAuth client registration map 1:1 to `agent_id`, or can one OAuth client rotate across agents?

---

## Related documents

`docs/adr/0005-token-ttl-and-audience.md` · `docs/a2a/explanation/auth-callout-design.md` · `docs/identity/registry-operations.md` (operator runbook)

---

## Signed manifest schema

TOML chosen for diff readability and comment support; YAML rejected because we already pay the TOML tax in `Cargo.toml`/`mise.toml` and don't want a second parser footprint.

Git manifests are the **authoritative source**. Each file is a signed envelope containing a `[record]` block (same fields as § Entity schema, minus runtime-only timestamps) and a detached Ed25519 `[signature]`.

```toml
manifest_version = 1

[record]
agent_id = "acme/oncall-agent"
agent_version = "3.2.1"
agent_definition_digest = "sha256:abc123def456"
owner_team = "platform-sre"
allowed_workloads = ["spiffe://acme.local/ns/prod/sa/oncall-agent"]
allowed_tools = ["pagerduty.page"]
allowed_audiences = ["mcp.server.pagerduty"]
allowed_purposes = ["incident.response"]
mesh_token_ttl_s = 300
lifecycle_state = "active"

[record.metadata]
description = "Pages on-call"

[signature]
algorithm = "ed25519"
key_id = "registry-signer-current"
value = "<base64 signature>"
```

| Field | Required | Notes |
|---|---|---|
| `manifest_version` | yes | Must be `1` |
| `record.*` | yes | Same semantics as § Entity schema |
| `signature.algorithm` | yes | Must be `ed25519` |
| `signature.key_id` | yes | Signer key identifier for rotation audit |
| `signature.value` | yes | Base64 Ed25519 over canonical JSON of `record` |

Validation rules enforced by `agctl registry sync` and the controller:

- `agent_id` — exactly one `/`, lowercase segments, no `.`
- `owner_team` — non-empty
- `agent_definition_digest` — `{algo}:{hex}` (e.g. `sha256:…`)
- `agent_version` — monotonic per `agent_id` within the repo and relative to KV `@latest`

Examples: `rsworkspace/crates/trogon-agent-registry/examples/*.toml`.

---

## Authoritative source: Git → controller → KV

```
┌─────────────────────┐
│  Git manifest repo  │  agents/{tenant}/{agent}.toml (signed TOML)
│  (source of truth)  │
└──────────┬──────────┘
           │ merge to main
           ▼
┌─────────────────────┐     Ed25519 verify      ┌──────────────────────┐
│ registry-controller │ ───────────────────────►│  agctl registry sync │
│  (signer account)   │     (CI pre-commit)   │  (local validation)  │
└──────────┬──────────┘                       └──────────────────────┘
           │ PUT {agent_id}/{version}
           │ PUT {agent_id}/@latest
           ▼
┌─────────────────────┐
│  NATS KV bucket     │  mcp-agent-registry (controller-only writes)
│  mcp-agent-registry │
└──────────┬──────────┘
           │ watch / lookup
           ▼
┌─────────────────────┐     mcp.registry.agent.lookup
│ trogon-agent-registry│ ◄────────────────────────── STS / gateway / SDK
│  (read-only)         │
└─────────────────────┘
           │
           ▼
   mcp.audit.registry.{registered,bumped,deprecated,revoked}
   (controller mutations — distinct from lookup.* audit on read path)
```

**Write ACL:** only the controller's NATS account may `PUT`/`DELETE` in `mcp-agent-registry`. Lookup nodes, STS, and gateways are read-only consumers.


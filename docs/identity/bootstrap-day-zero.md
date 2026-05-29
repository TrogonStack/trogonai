# Bootstrap / day-zero — operator how-to and posture

**Status:** Design spec (Block C, paper). Resolves the open item in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) Block C — *Bootstrap / day-zero behavior with empty bundle*.

**Diátaxis:** This document is a **how-to** (numbered bootstrap sequence, idempotency, failure recovery) plus **explanation** (why the default posture is mixed, how partial state behaves, migration semantics).

**Related:** [identity overview](overview.md), [registry operations](registry-operations.md), [STS exchange](sts-exchange.md), [A2A SDK contract](sdk.md), [MCP gateway plan § NATS Subject Topology](../../MCP_GATEWAY_PLAN.md#nats-subject-topology).

---

## 1. What "day zero" means

**Day zero** is the state of a tenant NATS account (or a soft-tenancy deployment inside a shared account) immediately after JetStream/KV buckets are provisioned but **before** the identity and policy control plane has been seeded. The MCP gateway process may be running; clients may hold bootstrap NATS User JWTs from the auth callout; **no successful gated MCP call should be assumed safe** until the bootstrap sequence completes.

### 1.1 Empty or missing control-plane artifacts

| Artifact | KV bucket / store | Expected key shape (tenant-scoped) | Day-zero state |
|---|---|---|---|
| SPIFFE trust bundle | `mcp-trust-bundles` | `{trust_domain}` (e.g. `local`, `acme.local`) | Bucket may exist (auto-created by `trogon-sts-publish-trust-bundle`) but **no key**, or key with empty PEM |
| Policy bundle pointer | `mcp-gateway-config` | `bundle/active` (manifest revision pointer) | **No entry** — no signed bundle loaded; gateway has no CEL/WASM policy tier |
| Mesh signing JWKS | `mcp-jwks` | `mesh/current` | Often missing on day zero until `trogon-jwks-publisher` runs |
| Bootstrap issuer JWKS | `mcp-jwks` or HTTPS | `bootstrap/current` or auth-callout URL | May exist from auth-callout bootstrap **independently** of MCP gateway readiness |
| Agent registry | `mcp-agent-registry` | `{agent_id}/{version}`, `{agent_id}/@latest` | **Empty** — no `@latest` pointers |
| MCP backend discovery | *(JetStream KV or config entry)* | Federation/backend list under `mcp-gateway-config` | **Empty** — no registered `server_id` targets |
| SpiceDB schema | SpiceDB datastore | Zed schema + relationships | **Seed only** — e.g. A2A dev seed (`user`, `agent`, `task`, `agent_card` in `devops/docker/compose/services/spicedb/schema.zed`) without MCP resource definitions (`trogon/mcp_tool`, `trogon/mcp_resource`, …) or tenant tuples |
| Audit stream | JetStream | Stream `MCP_AUDIT`, filter `{MCP_PREFIX}.audit.>` | May exist from gateway startup (`MCP_GATEWAY_AUDIT_STREAM`, default `MCP_AUDIT`) even when policy is absent |

> **Naming note.** Some planning docs refer colloquially to a "policy bundles bucket." The **implemented** config distribution bucket is `mcp-gateway-config`. Binary WASM artifacts use JetStream Object Store (Phase 3); Phase 1–2 store the active bundle pointer and Tier-1 YAML in KV. There is **no** separate `mcp-policy-bundles` bucket in the codebase today.

### 1.2 Runtime services on day zero

| Service | Typical state | Implication |
|---|---|---|
| `trogon-mcp-gateway` | Running, queue group on `mcp.gateway.request.>` | Forwards or denies per posture (§2); Phase 1 **allow-all on SpiceDB-gated methods when `MCP_GATEWAY_SPICEDB_ENDPOINT` is unset** — day-zero posture must override that default in production |
| `trogon-sts` | May be running | Exchanges fail at registry or attestation steps until registry + trust bundle exist ([sts-exchange.md § Validation pipeline](sts-exchange.md#validation-pipeline)) |
| `trogon-agent-registry` | Running with empty KV | Lookup NACK / miss — STS fail-closed in enforce mode |
| `trogon-agent-registry-controller` | Not required for first smoke test if KV seeded manually | Git → KV path documented in [registry-operations.md](registry-operations.md) |
| Backend MCP server (`mcp-nats`) | May be running on `mcp.server.{id}.>` | Gateway cannot reach it usefully until discovery/config lists the server and policy allows the call |

### 1.3 Tenant readiness predicate

Define **`tenant_initialised`** as the conjunction of:

1. **Trust:** KV `mcp-trust-bundles/{trust_domain}` contains a valid SPIFFE bundle PEM for the tenant's trust domain (`MCP_STS_TRUST_DOMAIN`).
2. **Policy:** KV `mcp-gateway-config/bundle/active` points at a **signature-valid** bundle revision the gateway has loaded.
3. **Registry:** At least one agent record exists at `mcp-agent-registry/{agent_id}/@latest` with `lifecycle_state = active` for the workload that will call the gateway (required when `MCP_GATEWAY_AGENT_IDENTITY=enforce`).
4. **Authorization graph:** SpiceDB schema includes MCP object definitions **and** minimum relationships for the smoke-test principal (see §3 step c).
5. **Backend target:** `mcp-gateway-config` (or discovery announcement) lists the MCP `server_id` that will receive forwarded traffic.

Until `tenant_initialised` is true, the tenant is in **day-zero posture**. Individual steps may complete out of order; §4 describes partial-state behavior.

### 1.4 What still works on day zero

These paths are **outside** the gateway data plane and remain available to operators with appropriate NATS ACLs:

- NATS KV `PUT` / `GET` for bootstrap artifacts (controller service account, break-glass creds).
- Auth callout at CONNECT (bootstrap User JWT with `mcp.gateway.request.>` publish ACL).
- `mcp.sts.exchange` request/reply (returns structured errors until registry + attestation succeed).
- `mcp.registry.agent.lookup` (returns miss until seeded).
- `mcp.control.discovery.register.{server_id}` publish from backend MCP server.
- JetStream audit consumers on `mcp.audit.>` (may receive deny/error envelopes from partial bootstrap).

---

## 2. Default posture decision

### 2.1 Decision: **Mixed** (fail-closed for gated work, limited allow for session/catalog)

Three options were considered:

| Option | Behavior | Verdict |
|---|---|---|
| **Strict fail-closed** | Every gateway request denied; audit `reason: tenant_uninitialised` | Safest, but blocks `initialize` / `ping` smoke tests through the same path operators use post-bootstrap |
| **Permissive fall-through** | All requests forwarded; audit `bootstrap_mode: true` | Ergonomic for dev, **unacceptable** for production — unauthenticated policy bypass |
| **Mixed** | Gated methods denied; session/catalog methods allowed with auditable bootstrap flag | **Selected** — balances safety and operability |

**Production default:** **Mixed** posture without an env override.

**Rationale:**

1. **Security where it matters.** `tools/call` and `resources/read` are the Phase 1 SpiceDB-gated surface ([trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md)). Denying them until `tenant_initialised` prevents silent allow-all (today's behavior when SpiceDB endpoint is unset) from becoming an accidental production configuration.

2. **Operability for the same wire path.** Operators smoke-test `{MCP_PREFIX}.gateway.request.{server_id}.initialize` and `.ping` through the edge zone before any tool tuples exist in SpiceDB. Strict deny on those methods forces every bring-up to flip a global fail-open flag first, which is harder to audit and easier to forget.

3. **Consistency with STS/gateway fail-closed culture.** [overview.md § Failure modes](overview.md#failure-modes) and [sts-exchange.md § Failure modes](sts-exchange.md#failure-modes) already fail-closed on identity and registry. Mixed extends that to **authorization-bearing** MCP methods only, not to connectivity probes.

4. **Clear audit story.** Gated denies emit `reason: tenant_uninitialised` (see §2.3). Allowed bootstrap-path methods emit `bootstrap_mode: true` in the audit envelope `extra` field so SIEM rules can alert on any non-zero rate after cutover.

### 2.2 Method classification

Assume default `MCP_PREFIX=mcp`. All subjects below are prefixed with `{MCP_PREFIX}.` in production when operators override the prefix.

| Class | JSON-RPC / subject suffix | Day-zero mixed posture |
|---|---|---|
| **Gated data plane** | `tools.call`, `resources.read`; callback methods with side effects (`sampling.createMessage`, `elicitation.create`) | **Deny** before backend publish |
| **Session / catalog** | `initialize`, `ping`, `tools.list`, `resources.list`, `prompts.list`, `prompts.get` (read-only) | **Allow** with `bootstrap_mode` audit; egress may include `mcp-bootstrap-incomplete: true` header **(proposed header)** |
| **Notifications** | `notifications.*` | **Deny** (treat as gated — may trigger server side effects) |
| **Control plane** | `mcp.control.>` (existing subjects) | **Unchanged** — not handled by gateway request consumer |
| **Bootstrap probe (proposed)** | `mcp.gateway.bootstrap.>` | **Allow** read-only probe handlers (see §6.2) |

### 2.3 Deny and audit contract (gated methods)

When a gated method arrives and `tenant_initialised` is false:

| Field | Value |
|---|---|
| JSON-RPC `data.reason` | `tenant_uninitialised` **(proposed stable string;** not yet in `rpc_codes.rs`) |
| JSON-RPC `data.missing` | Array of failed readiness checks, e.g. `["policy_bundle","registry","spicedb_tuples"]` **(proposed)** |
| Audit subject | `mcp.audit.deny.request.{method_root}` |
| Audit envelope `decision` | `deny` |
| Audit envelope `rules_fired` | `["tenant-readiness"]` **(proposed rule id)** |
| Audit envelope `extra.reason` | `tenant_uninitialised` |
| Audit envelope `extra.readiness` | Object with per-check booleans |

No backend publish on `mcp.server.{server_id}.>` occurs on deny.

### 2.4 Allow path contract (session / catalog)

When a session/catalog method arrives and `tenant_initialised` is false:

| Field | Value |
|---|---|
| JSON-RPC | Forward to backend if target server is registered and reachable; otherwise `-32103` `backend_unreachable` |
| Audit subject | `mcp.audit.allow.request.{method_root}` or `mcp.audit.error.*` on backend failure |
| Audit envelope `extra.bootstrap_mode` | `true` |
| Audit envelope `extra.tenant_initialised` | `false` |

Operators should treat sustained `bootstrap_mode: true` traffic after cutover as a misconfiguration alert.

### 2.5 Mode overrides (operator intent)

| Mode | Configuration | Use |
|---|---|---|
| **Mixed (default)** | *(no env)* | Production |
| **Strict** | `MCP_GATEWAY_BOOTSTRAP_POSTURE=strict` **(proposed)** | Pen-test, compliance scans, " prove fail-closed" |
| **Permissive (dev only)** | `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` **(proposed)** | Local dev only; every allow emits `bootstrap_mode: true`; gated methods still log would-deny unless `MCP_GATEWAY_SPICEDB_ENDPOINT` set |

Permissive mode must be rejected at gateway startup when `MCP_GATEWAY_AGENT_IDENTITY=enforce` **(proposed guard)**.

---

## 3. Bootstrap sequence

Goal: go from an empty tenant account to the **first successful `tools/call`** through `{MCP_PREFIX}.gateway.request.{server_id}.tools.call` with enforce-mode identity and SpiceDB allow.

**Prerequisites:** NATS account provisioned; auth callout issuing bootstrap JWTs; JetStream enabled; gateway and STS processes deployable; operator holds KV-write and SpiceDB admin credentials.

### Step 0 — Provision JetStream assets (once per account)

Idempotent account-level setup. Aligns with [registry-operations.md § Bootstrap (fresh cluster)](registry-operations.md#bootstrap-fresh-cluster).

1. Create KV buckets (or verify auto-create on first write):
   - `mcp-trust-bundles`
   - `mcp-gateway-config`
   - `mcp-agent-registry`
   - `mcp-jwks`
   - `mcp-sessions` (when session KV is enabled)
2. Create JetStream stream `MCP_AUDIT` with subject filter `{MCP_PREFIX}.audit.>` (default prefix `mcp`).
3. Apply NATS ACL templates: gateway service account, controller write to `mcp-agent-registry`, STS consumer on `mcp.sts.exchange`, clients publish `{MCP_PREFIX}.gateway.request.>` only.

**Verify:** `nats kv ls` lists buckets; `nats stream info MCP_AUDIT` succeeds.

### Step a — Publish trust bundle to `mcp-trust-bundles`

**Why first:** STS SVID attestation (`MCP_STS_REQUIRE_ATTESTATION=1`) and gateway enforce-mode `wkl` validation depend on trust bundle material ([overview.md](overview.md), [sts-exchange.md § Validation pipeline step 2](sts-exchange.md#validation-pipeline)).

**Action:**

```bash
export NATS_URL=nats://127.0.0.1:4222
export MCP_STS_TRUST_BUNDLE_PATH=/path/to/bundle.pem
export MCP_STS_TRUST_DOMAIN=acme.local   # match SPIFFE IDs in registry

cargo run -p trogon-sts --bin trogon-sts-publish-trust-bundle
```

Equivalent: `nats kv put mcp-trust-bundles acme.local @bundle.pem`

**Verify:** `nats kv get mcp-trust-bundles acme.local` returns non-empty PEM; STS logs show trust bundle watch fired.

**Idempotency:** `PUT` same key replaces bundle; watchers reload. Safe to re-run.

**Gateway while missing:** Mixed posture — STS cannot mint attested mesh tokens; gateway ingress in `enforce` rejects missing `wkl`. Session/catalog gateway calls may still work in `shadow`/`off` identity modes only.

### Step b — Register agents and MCP backend target

Two distinct registrations are required:

#### b.1 Agent registry (for STS and SDK)

Follow [registry-operations.md § Bootstrap](registry-operations.md#bootstrap-fresh-cluster):

1. Seed signed manifest under `agents/{tenant}/{agent}.toml`.
2. Run `trogon-agent-registry-controller` **or** dev-equivalent `agctl registry sync --repo … --verify-key …`.
3. Confirm lookup: request/reply on `mcp.registry.agent.lookup` returns the seeded record.

Minimum manifest fields: `agent_id`, `allowed_workloads` (caller's SPIFFE ID), `allowed_audiences` (include `urn:trogon:mcp:gateway:{tenant}:{gateway_id}` and backend URI), `allowed_purposes`, `lifecycle_state: active`.

#### b.2 MCP backend server (for gateway routing)

Backend MCP servers use existing `mcp-nats` subjects. Register the server id in gateway config and announce liveness:

1. **Config:** Put backend entry in `mcp-gateway-config` keyed by `backend/{server_id}` with `{ "server_id": "github", "enabled": true }` (shape **proposed** — exact key TBD in bundle loader spec).
2. **Discovery:** On server start, publish to `mcp.control.discovery.register.{server_id}`.
3. Run `mcp-nats` server subscribed on `mcp.server.github.>`.

**Verify:** Gateway logs show discovery event; `tools/list` in bootstrap_mode returns tools (may be empty catalog).

**Idempotency:** Re-sync registry from Git; re-publish discovery. `@latest` pointer updates monotonically.

**Gateway while missing:** Gated calls deny with `tenant_uninitialised` (`missing` includes `registry` and/or `backend_target`). `tools/call` to unknown server returns `-32103` `backend_unreachable`.

### Step c — Publish minimum-viable policy bundle

**Action:**

1. Author Tier-1 YAML with:
   - Default deny for `tools/call` / `resources/read` except explicit test tool.
   - CEL rule delegating to `spicedb.check` for production paths.
   - Audit envelope defaults.
2. Sign manifest with org NKey (Phase 3 bundle format; Phase 1 may use unsigned dev bundle only when `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` **(proposed)** — production requires signature).
3. Write active pointer: `nats kv put mcp-gateway-config bundle/active @manifest.json`
4. Optional belt-and-braces: publish `mcp.control.bundle.reload`.

**SpiceDB minimum:**

Write MCP schema definitions (`trogon/mcp_tool`, `trogon/mcp_resource`, subject type) and seed tuples for smoke principal:

```text
trogon/mcp_tool:github/create_issue#allowed@user:smoke-tester
```

Use `zed schema write` + `zed relationship create` or repo seed job (pattern: `devops/docker/compose/services/a2a-spicedb-seed`).

**Verify:** Gateway log line `bundle loaded revision=N`; readiness check `policy_bundle=true`; SpiceDB `zed permission check` succeeds for smoke tuple.

**Idempotency:** KV revision increments; gateway hot-swaps; in-flight requests complete on old bundle revision.

**Gateway while missing:** Mixed posture denies gated methods with `-32108` / `tenant_uninitialised`, `missing` includes `policy_bundle`.

### Step d — Mint first bootstrap token and mesh token

**Bootstrap NATS JWT (CONNECT):**

1. Authenticate to auth callout (OIDC / mTLS / API key per deployment).
2. Receive User JWT with `publish` on `{MCP_PREFIX}.gateway.request.>` and `sub` for smoke tester.

**Mesh token (STS exchange):**

```json
{
  "subject_token": "<bootstrap-user-jwt>",
  "subject_token_type": "urn:ietf:params:oauth:token-type:jwt",
  "actor_token": "<PEM SVID or sentinel per MCP_STS_REQUIRE_ATTESTATION>",
  "audience": "urn:trogon:mcp:gateway:acme:gw-prod-1",
  "purpose": "bootstrap.smoke",
  "scope": "tool:github::create_issue"
}
```

Publish to `mcp.sts.exchange`; await reply on `_INBOX.*`.

**Verify:** Response includes `access_token` JWT with `wkl`, `agent_id`, `act_chain` length ≥ 1; audit on `mcp.audit.sts.success`.

**Idempotency:** Each exchange mints a fresh token; safe to repeat.

**Gateway while missing prior steps:** Exchange fails with `invalid_target`, `access_denied`, or `unauthorized_client` per [sts-exchange.md § Response schema (error)](sts-exchange.md#response-schema-error).

### Step e — Smoke-test with the SDK

Use [trogon-a2a-sdk](../../rsworkspace/crates/trogon-a2a-sdk/README.md) — the supported path for agent-initiated MCP calls ([sdk.md](sdk.md)).

```bash
export NATS_URL=nats://127.0.0.1:4222
export AGENT_ID=acme/smoke-agent
# Wire STS, registry, mesh JWKS per README …

cargo run -p trogon-a2a-sdk --example echo_agent --features nats
```

**MCP `tools/call` check (conceptual):**

```rust
client.call_mcp(
    McpServerId::new("github")?,
    RpcMethod::tools_call(),
    &json!({ "name": "create_issue", "arguments": { /* … */ } }),
    Some(&Purpose::new("bootstrap.smoke")),
).await?;
```

**Verify success criteria:**

| Signal | Expected |
|---|---|
| JSON-RPC result | Tool success payload (not `-32108`) |
| Audit | `mcp.audit.allow.request.tools` with `bootstrap_mode: false`, `tenant_initialised: true` |
| Backend | Received mesh JWT on `mcp-caller-sub` / Bearer, not bootstrap JWT ([ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md)) |
| Readiness | `mcp.control.tenant.ready.{tenant}` beacon **(proposed)** published by gateway |

---

## 4. Per-step idempotency, ordering, and partial-state behavior

### 4.1 Ordering constraints

```text
         ┌─────────────────┐
         │ 0: JetStream/KV │
         └────────┬────────┘
                  │
         ┌────────▼────────┐
         │ a: trust bundle │◄── STS attestation hard dependency
         └────────┬────────┘
                  │
    ┌─────────────┼─────────────┐
    │             │             │
    ▼             ▼             ▼
 b.1 registry  b.2 backend   c.1 policy
    │             │             │
    └─────────────┼─────────────┘
                  │
         ┌────────▼────────┐
         │ c.2 SpiceDB     │◄── can parallel with c.1 after schema write
         └────────┬────────┘
                  │
         ┌────────▼────────┐
         │ d: STS mint     │◄── needs a + b.1 (+ attestation)
         └────────┬────────┘
                  │
         ┌────────▼────────┐
         │ e: tools/call   │◄── needs all readiness checks
         └─────────────────┘
```

**Hard dependencies:**

- `d` requires `a` and `b.1` (registry + trust).
- `e` gated success requires `tenant_initialised` (§1.3).

**Soft parallelization:** `b.2`, `c`, and SpiceDB seeding can proceed in parallel once KV exists.

### 4.2 Idempotency summary

| Step | Operation | Idempotent? | Notes |
|---|---|---|---|
| a | Trust bundle `PUT` | Yes | Monotonic revision; watch reload |
| b.1 | Registry sync | Yes | Git authoritative; controller reconciles |
| b.2 | Discovery publish | Yes | Repeated announcements refresh TTL |
| c | Bundle pointer update | Yes | Gateway atomic swap; rollback = prior revision |
| c | SpiceDB tuple create | Mostly | `zed relationship touch` idempotent; schema write is replace |
| d | STS exchange | Yes | New token each time; no server mutation |
| e | SDK call | No | Tool may have side effects — use dry-run tool in smoke |

### 4.3 Gateway behavior matrix (mixed posture)

Rows: readiness check missing. Columns: method class.

| Missing check | Session/catalog (`initialize`, `ping`, `*/list`) | Gated (`tools/call`, `resources/read`) |
|---|---|---|
| Trust bundle | Allow† / STS fails for mesh | Deny `tenant_uninitialised` |
| Policy bundle | Allow `bootstrap_mode` | Deny `-32108` |
| Registry empty | Allow† | Deny `tenant_uninitialised` |
| SpiceDB tuples | Allow `bootstrap_mode` | Deny `-32100` policy deny or `-32108` if bundle requires SpiceDB |
| Backend unregistered | `-32103` backend_unreachable | Deny or `-32103` if routing fails first |

† With bootstrap JWT only (`MCP_GATEWAY_AGENT_IDENTITY=off`); **enforce** mode still requires mesh token from STS → effectively blocked until `d` succeeds.

### 4.4 Readiness cache

Gateway evaluates `tenant_initialised` every **30 s** **(proposed default)** and on KV watch events for `mcp-gateway-config`, `mcp-trust-bundles`, and `mcp-agent-registry`. Negative results are cached **5 s** to avoid KV hot loops. Positive result cached until any watched key revision changes.

---

## 5. Failure modes

### 5.1 KV bucket created but empty

**Symptoms:** Buckets exist; gateway starts; all gated calls deny.

**Behavior:** Mixed posture — catalog/session calls may succeed with `bootstrap_mode: true`; audit `extra.readiness` lists all false.

**Recovery:** Run §3 sequence; no gateway restart required if watchers active.

**Anti-pattern:** Enabling `MCP_GATEWAY_BOOTSTRAP_POSTURE=permissive` to "unblock" without writing artifacts — leaves permanent SIEM noise and bypasses SpiceDB.

### 5.2 Policy bundle present but signature invalid

**Symptoms:** Gateway rejects load; log `bundle signature invalid`; pointer revision stuck.

**Behavior:** Treat as **no bundle** — gated deny `-32108`; audit `extra.reason: bundle_signature_invalid` **(proposed)**.

**Recovery:**

1. Fix signature or republish with trusted NKey.
2. Roll back pointer: `nats kv put mcp-gateway-config bundle/active @last-known-good.json`
3. Publish `mcp.control.bundle.reload`.

**Idempotency:** Failed load does not advance in-memory active revision.

### 5.3 Trust bundle present but no matching SVID

**Symptoms:** STS returns `unauthorized_client`; gateway `-32110` invalid token in enforce mode.

**Behavior:** Fail-closed at STS ([sts-exchange.md](sts-exchange.md)); mesh mint impossible.

**Recovery:**

1. Confirm `actor_token` SVID SAN matches `spiffe://{trust_domain}/…`.
2. Confirm registry `allowed_workloads` includes that SPIFFE ID.
3. Re-issue SVID from SPIRE / local dev cert.

### 5.4 Partial bootstrap — one step skipped

| Skipped step | First `tools/call` outcome |
|---|---|
| a trust | STS/gateway enforce reject; shadow may allow unattested `wkl` with audit `wkl_unattested` |
| b.1 registry | STS `invalid_target` / `access_denied`; gateway deny |
| b.2 backend | `-32103` backend_unreachable |
| c policy | `-32108` tenant_uninitialised |
| c SpiceDB | `-32100` policy_deny after bundle loads |
| d mesh mint | `-32109` / `-32106` / `-32110` depending on token state |

**Audit trail:** Each deny should include `extra.missing` array **(proposed)** so operators can see the first failing check without enabling permissive mode.

### 5.5 SpiceDB reachable but schema is seed-only

**Symptoms:** Permission check errors `object definition not found` or undefined object type.

**Behavior:** `-32107` `authz_unreachable` or `-32100` depending on gateway SpiceDB error mapping.

**Recovery:** Apply MCP schema + relationships (§3c); restart not required — gateway retries per request.

### 5.6 Registry populated but agent revoked

Not day-zero — included for contrast: STS `access_denied`; audit `mcp.audit.sts.deny`. Distinct from `tenant_uninitialised`.

---

## 6. Operator escape hatches

Use only during controlled bring-up windows. Remove before enforce cutover.

### 6.1 Fail-open during bring-up (env)

| Variable | Values | Effect |
|---|---|---|
| `MCP_GATEWAY_BOOTSTRAP_POSTURE` **(proposed)** | `strict` \| `mixed` (default) \| `permissive` | See §2.5 |
| `MCP_GATEWAY_SPICEDB_ENDPOINT` | unset | **Current Phase 1 behavior:** allow-all on gated methods — **do not use in prod day-zero** |
| `MCP_GATEWAY_AGENT_IDENTITY` | `off` / `shadow` / `enforce` | `off` skips mesh enforcement ([overview.md § Rollout modes](overview.md#rollout-modes)) |
| `MCP_STS_REQUIRE_ATTESTATION` | unset | Shadow attestation — see [sts-exchange.md § STS attestation modes](sts-exchange.md#sts-attestation-modes-mcp_sts_require_attestation) |
| `STS_CHAIN_RESOLUTION_MODE` | `off` | Emergency only — skips chain walk ([sts-exchange.md](sts-exchange.md)) |

**Recommended bring-up window:** `MCP_GATEWAY_AGENT_IDENTITY=shadow`, mixed posture, permissive **disabled**. Fix readiness rather than enabling permissive.

### 6.2 Ready beacon (proposed)

**Subject:** `mcp.control.tenant.ready.{tenant}` **(proposed)**

Gateway publishes `{ "tenant": "acme", "initialised": true, "checks": { … }, "ts": "…" }` when `tenant_initialised` transitions false → true.

Operators subscribe during bring-up:

```bash
nats sub "mcp.control.tenant.ready.acme"
```

Existing related subject: `mcp.control.gateway.heartbeat.{instance_id}` — instance liveness, not tenant readiness.

### 6.3 Audit subscriptions during bring-up

| Subscription | Purpose |
|---|---|
| `mcp.audit.deny.>` | All denials; filter `extra.reason == tenant_uninitialised` |
| `mcp.audit.sts.>` | STS exchange success/deny during step d |
| `mcp.audit.registry.>` | Registry controller events during step b.1 |
| `mcp.audit.allow.request.tools` with `bootstrap_mode: true` | Residual bootstrap traffic after cutover |

JetStream durable consumer on stream `MCP_AUDIT` recommended for SIEM replay.

### 6.4 Bootstrap probe subject (proposed)

**Subject tree:** `mcp.gateway.bootstrap.{tenant}.>` **(proposed)**

Read-only request/reply handlers for:

- `readiness` — return JSON readiness object without hitting MCP backend.
- `version` — gateway build info + loaded bundle revision.

Does not replace §3e — confirms gateway routing before backend exists.

### 6.5 Break-glass KV write

Per [registry-operations.md § Emergency revocation](registry-operations.md#emergency-revocation): dual-control KV creds may write registry or config when Git/controller unavailable. Audit to `mcp.audit.registry.*` or manual record with `operator=break-glass`.

---

## 7. Migration — returning a production tenant to day-zero without availability gap

**Goal:** Rotate all keys, bundles, and trust material (compromise recovery, tenant offboarding rehearsal, environment clone) **without** a window where gated calls fail-open.

### 7.1 Principles

1. **Never** switch to permissive posture in production.
2. **Overlap** trust and signing material ([ADR 0006](../adr/0006-mesh-token-signing-keys.md)): JWKS overlap ≥ max mesh TTL + skew.
3. **Atomic cutover** for policy bundle pointer — prepare new revision before flipping `bundle/active`.
4. **Drain** in-flight mesh tokens (default TTL 120 s) before revoking old STS signing key.

### 7.2 Zero-downtime rotation sequence

1. **Announce maintenance** — increase audit sampling; no config change yet.
2. **Publish overlapping trust bundle** — add new CA to PEM in `mcp-trust-bundles/{trust_domain}` before issuing new SVIDs.
3. **Publish overlapping mesh JWKS** — add new `kid` to `mcp-jwks/mesh/current`; keep old `kid` until all gateways report reload.
4. **Stage new policy bundle** — write `mcp-gateway-config/bundle/staging` **(proposed key)**; gateway validates signature offline **(proposed feature)**.
5. **Rotate registry manifests** — bump `agent_version`; controller syncs; verify `@latest`.
6. **Flip bundle pointer** — atomic `bundle/active` swap; watch `mcp.control.bundle.reload`.
7. **Re-seed SpiceDB** — relationship updates additive first, then revoke old tuples.
8. **Rotate STS signing key** — KMS rotation per ADR 0006; remove old `kid` only after overlap window.
9. **Revoke old bootstrap issuer** — auth-callout JWKS update; bootstrap tokens expire naturally.
10. **Simulate day-zero deny** — in staging account, confirm mixed posture denies gated calls when pointer removed; **do not repeat in prod** unless testing break-glass.

### 7.3 Hard reset to day-zero (compromise)

When immediate fail-closed is required:

1. Set `bundle/active` to empty / delete key → gated methods deny `-32108` immediately.
2. Revoke all agents: `lifecycle_state = revoked` in Git fast-track ([registry-operations.md § Emergency revocation](registry-operations.md#emergency-revocation)).
3. Purge `mcp-jwks/mesh/current` **only with STS stopped** — otherwise validators fail open on stale cache until TTL; prefer replacing with empty JWKS `{"keys":[]}` to force fail-closed.
4. Delete trust bundle key → STS attestation fail-closed.

**Availability impact:** Gated traffic stops immediately (desired). Session/catalog traffic may still flow under mixed posture — switch to `MCP_GATEWAY_BOOTSTRAP_POSTURE=strict` **(proposed)** for full deny if needed.

**Restore:** Full §3 bootstrap sequence on clean material.

### 7.4 Clone tenant to new account

1. Export Git manifests and signed bundles (not compromised KV).
2. Provision new NATS account ( [ADR 0001](../adr/0001-tenancy-model.md) ).
3. Run §3 with **new** trust domain and signing keys — do not copy private keys via KV export.
4. Update auth callout tenant routing to new account.

---

## 8. Cross-reference index

| Topic | Document |
|---|---|
| Agent registry bootstrap | [registry-operations.md § Bootstrap (fresh cluster)](registry-operations.md#bootstrap-fresh-cluster) |
| Registry entity schema | [registry.md](registry.md) |
| Identity layers and enforce modes | [overview.md](overview.md) |
| STS wire contract | [sts-exchange.md](sts-exchange.md) |
| SDK smoke path | [sdk.md](sdk.md), [trogon-a2a-sdk README](../../rsworkspace/crates/trogon-a2a-sdk/README.md) |
| Tenancy | [ADR 0001](../adr/0001-tenancy-model.md) |
| Bootstrap vs mesh tokens | [ADR 0003](../adr/0003-bootstrap-vs-mesh-tokens.md) |

---

## 9. Implementation gap (Phase 1 vs this spec)

`trogon-mcp-gateway` Phase 1 still **allow-all** on gated methods when `MCP_GATEWAY_SPICEDB_ENDPOINT` is unset and does not load policy bundles from KV. Treat that as **dev-only** until engineering lands: readiness evaluator + KV watches, mixed method classifier, audit `extra.reason` / `extra.bootstrap_mode`, `MCP_GATEWAY_BOOTSTRAP_POSTURE` **(proposed)**, and `-32108` with `data.reason: tenant_uninitialised` **(proposed)**.

Illustrative audit deny envelope when implemented:

```json
{
  "schema": "trogon.mcp.audit/v1",
  "decision": "deny",
  "method": "tools/call",
  "extra": {
    "reason": "tenant_uninitialised",
    "readiness": { "policy_bundle": false, "spicedb": false }
  }
}
```

Minimal SpiceDB smoke tuples (after MCP schema write): `trogon/mcp_tool:github/create_issue#allowed@user:smoke-tester` — object ids must match `MCP_GATEWAY_SPICEDB_*` in [trogon-mcp-gateway README](../../rsworkspace/crates/trogon-mcp-gateway/README.md).

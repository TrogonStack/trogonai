# A2A auth callout — Phase 0 implementation sketch

Engineering sketch for the **auth callout service** tracked in [A2A TODO](../A2A_TODO.md) (Phase 0 — perimeter & catalog). This document describes *what* to build and *how it wires into* the A2A-over-NATS binding; it is not a deployment runbook.

Related decisions: [A2A pending decisions](../A2A_PENDING_DECISION.md) (auth callout deployment, subject ACL, push DLQ).

---

## 1. Goal

The auth callout terminates **external credentials** at the NATS perimeter and **mints Account-scoped User JWTs** that clients use for all subsequent NATS connections inside a tenant Account.

Two outcomes matter for downstream A2A work:

1. **Perimeter JWT** — every caller connects with a short-lived User JWT whose `aud` is the tenant NATS Account and whose publish/subscribe permissions are bounded to Phase 0 ACL templates (gateway ingress + caller inbox only).
2. **Stable `caller_id`** — the callout maps external identity (OIDC `sub`, mTLS cert subject, transitional API key principal) to a **token-safe, stable segment** reused across:
   - `_INBOX.{caller_id}.>` subscribe ACL on the minted User
   - `a2a.push.{caller_id}.>` consumer ACL (push read path; provision at mint time even though Phase 0 checklist emphasizes gateway-side publish)
   - `{prefix}.push.dlq.{caller_id}.{task_id}` DLQ subject segments when push delivery fails
   - Gateway ingress tracing, audit enrichment, and (later) SpiceDB resource attribution

The mapping **external principal → `caller_id`** must be deterministic for a given tenant Account so ACL rows and DLQ subjects remain stable across reconnects and JWT re-mints.

---

## 2. NATS mechanics (auth callout)

NATS **Authorization Callout** ([NATS authorization callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout)) lets a cluster delegate credential validation to an external service. When callout is enabled on an Account, the server does **not** accept raw username/password or unsigned bearer tokens for that Account; instead it forwards an auth decision request to a trusted subscriber.

### Callout placement

- Configure the **tenant Account JWT** (or Operator policy) to reference the callout service identity and enable auth callout for User connections into that Account.
- Run the **auth callout process** as a NATS client with permission to subscribe to the system auth subject (typically a dedicated service User on the system Account or an Operator-delegated callout principal — follow your org’s NATS operator model; no secrets in this repo).

### `$SYS.REQ.USER.AUTH` request/reply (generic shape)

The server publishes an auth request; the callout service **must reply** with either an authorization success (User JWT + claims) or an explicit denial. Exact wire encoding follows NATS server version and callout extension docs; treat the following as **field placeholders** for implementers:

**Request (server → callout)** — illustrative fields only:

- **`user_nkey`** / **`user_jwt`** — client-supplied credential material presented at connect time
- **`account`** — target Account public key or name the client is attempting to join
- **`client_info`** — connection metadata (TLS state, client IP, user-agent / client library id)
- **`connect_opts`** — optional tags or headers the client attached for routing (e.g. tenant hint, auth scheme selector)
- **`issuer_account`** — Account context for multi-Account callout routing

**Reply (callout → server)** — on success:

- **`user_jwt`** — signed **User JWT bound to the tenant Account** (short TTL; refresh via reconnect + re-callout)
- **`account`** — Account identifier the User is authorized for (must match tenant boundary)
- **`permissions`** — publish/subscribe allow lists embedded in the JWT or returned as claim templates the server merges (Phase 0: `a2a.gateway.>` publish, `_INBOX.{caller_id}.>` subscribe, plus caller push read if applicable)
- **`claims`** — additional JWT claim payload the server attaches to the issued credential

On failure, reply with an **error** / **authorization denied** indicator; the server rejects the connection.

### Credential sources (preference order)

Landed in [A2A pending decisions](../A2A_PENDING_DECISION.md):

1. **OIDC** — primary. Validate IdP token / introspection; map claims → tenant Account + `caller_id`.
2. **mTLS** — service-to-service. Map certificate subject / SAN → Account principal.
3. **API keys** — transitional only for clients migrating off legacy bearer flows.

The callout service owns **all** verification logic; NATS sees only minted User JWTs with bounded ACLs.

---

## 3. Claim layout sketch

Minted User JWTs should carry enough structure for NATS ACL binding, gateway correlation, and (later) SpiceDB checks. **This repo does not define the SpiceDB principal schema** — only the NATS-facing contract.

### Standard JWT claims

| Claim | Role |
|-------|------|
| **`aud`** | NATS **Account** public key or name (= tenant boundary). Must match the Account the client connects into. |
| **`sub`** | **External identity** as issued by the IdP or mTLS/API-key layer (opaque to NATS ACL templates). |
| **`exp` / `iat` / `nbf`** | Short-lived User credential; re-mint on reconnect. |

### Custom / namespaced claims

| Claim | Role |
|-------|------|
| **`caller_id`** (or equivalent custom claim) | Stable, **token-safe** segment (no `.` characters) used in subject ACL tokens: `_INBOX.{caller_id}.>`, `a2a.push.{caller_id}.>`, `{prefix}.push.dlq.{caller_id}.{task_id}`. Derive from external `sub` via a tenant-scoped hash or org registry — document the derivation in the callout service, not here. |
| **`data`** (JSON object) | **SpiceDB-ready principal payload** — org-standard mapping from external identity to authorization tuples. Opaque to `a2a-nats` / `a2a-gateway`; consumed by gateway policy (Phase 1). Keep JSON compact; avoid PII beyond what policy requires. |

### Illustrative pseudo-JSON (not normative)

```json
{
  "aud": "ABCDTenantAccountPubKey",
  "sub": "oidc|tenant-acme|user-7f3a…",
  "caller_id": "usr_7f3a9c2b",
  "data": {
    "principal_type": "…",
    "tenant_ref": "…",
    "spicedb_subject": "…"
  }
}
```

**Permissions** (whether embedded in JWT or applied via callout reply templates) must mirror the Phase 0 caller row: publish `a2a.gateway.>`, subscribe `_INBOX.{caller_id}.>`, and (recommended at mint time) subscribe `a2a.push.{caller_id}.>` for NATS push targets.

---

## 4. Operator wiring

Account provisioning, User role templates, and JetStream bootstrap are documented in **[A2A NSC account bootstrap](./A2A_NSC_ACCOUNT_BOOTSTRAP.md)**. The auth callout **replaces static per-caller `nsc add user`** for production human/OIDC identities; NSC steps remain the reference for **gateway**, **registrar**, and **caller ACL shape**.

### Phase 0 ACL rows (caller vs gateway)

From the bootstrap ACL table — substitute `{prefix}` if not using default `a2a`:

| NATS JWT role | Publish (allow) | Subscribe (allow) | Minted by |
|---------------|-----------------|-------------------|-----------|
| **Caller User** | `{prefix}.gateway.>` | `_INBOX.{caller_id}.>` | **Auth callout** (dynamic) |
| **Gateway User** | `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` | `{prefix}.gateway.>` | **NSC** (long-lived service User) |

Additional bootstrap roles (registrar, agents) are unchanged; see the full table in [A2A NSC account bootstrap](./A2A_NSC_ACCOUNT_BOOTSTRAP.md).

### Callout operator checklist (summary)

1. Enable auth callout on the tenant Account JWT per NATS operator docs.
2. Deploy callout subscriber with `$SYS.REQ.USER.AUTH` (or org-equivalent) subscription rights.
3. Ensure Account signing keys used to mint User JWTs are authorized for that Account.
4. Verify a minted caller User **cannot** publish `{prefix}.agent.>` or `{prefix}.catalog.register.*` (negative test).
5. Verify gateway User **cannot** subscribe `_INBOX.>` broadly (only `{prefix}.gateway.>`).

Static caller User templates in the bootstrap doc become **documentation-only** once runtime minting is live.

---

## 5. Repo integration points

Where Phase 0 auth callout work meets in-tree crates today (mostly seams and placeholders).

### `a2a-gateway` — ingress

- **Crate:** `rsworkspace/crates/a2a-gateway`
- **Today:** queue-group subscriber on `{prefix}.gateway.>`; opaque forward to `{prefix}.agent.{agent_id}.{method}` via `a2a_nats::gateway_ingress` (`resolve_gateway_ingress_subject`). **No JWT validation** at ingress yet.
- **Future (auth callout):**
  - Parse minted User JWT / connection identity on each ingress message (exact mechanism: connection-bound identity vs message headers — align with NATS client capabilities).
  - Extract **`caller_id`** from JWT custom claim; record on ingress span (`gateway.ingress.dispatch` already reserves `caller_id = tracing::field::Empty` in `runtime.rs`).
  - Thread identity into Phase 1 audit envelopes (`rules_fired`, allow/deny) and SpiceDB checks.
- **Non-goal today:** gateway does **not** publish push DLQ messages; terminal DLQ originates from the agent `Bridge` (see [A2A push DLQ ops](./A2A_PUSH_DLQ_OPS.md)).

### `a2a-nats` — `Config::push_dlq_caller_segment`

- **Crate:** `rsworkspace/crates/a2a-nats`
- **Field:** `Config::push_dlq_caller_segment` (builder: `with_push_dlq_caller_segment`)
- **Default:** `"_"` (`DEFAULT_PUSH_DLQ_CALLER_SEGMENT` in `constants.rs`) when no richer identity is available.
- **Used by:** agent `Bridge` / `message/stream` terminal push path → JetStream publish to `{prefix}.push.dlq.{caller_id}.{task_id}` (`push::dlq`).
- **When agents run behind minted identities:** propagate JWT-derived **`caller_id`** into agent runtime config (env or bootstrap hook — **not wired in-tree yet**). Operators/agents should set the segment to the same token-safe value the callout encodes in ACLs so DLQ subjects align with `_INBOX.{caller_id}.>` and push consumer ACLs.
- **Constraint:** `{caller_id}` must be a **single NATS subject token** (no `.`); `push/dlq.rs` documents this invariant.

### Cross-cutting flow (Phase 0 target)

```
External client ──OIDC/mTLS/API key──► Auth callout
                         │
                         ▼ mint User JWT (aud=Account, caller_id, data)
              Caller NATS connect ──publish──► {prefix}.gateway.{agent}.{method}
                         │
                         ▼
              a2a-gateway ingress ──forward──► {prefix}.agent.{agent}.{method}
                         │                      (future: span/audit caller_id)
                         ▼
              Agent Bridge / message/stream ──on push failure──►
                         {prefix}.push.dlq.{caller_id}.{task_id}
```

### Out of scope for this sketch

- SpiceDB client wiring (Phase 1)
- `a2a-bridge` HTTPS sidecar re-mint path (Phase 4) — same callout contract, different ingress transport
- Tier 1–3 policy bundles (`a2a-pack`)

---

## See also

- [A2A TODO](../A2A_TODO.md) — Phase 0 checklist item: auth callout service
- [A2A NSC account bootstrap](./A2A_NSC_ACCOUNT_BOOTSTRAP.md) — Operator / Account / User hierarchy and ACL table
- [A2A per-Account JetStream assets](./A2A_JETSTREAM_ACCOUNT_STREAMS.md) — `A2A_PUSH_DLQ` subject shape
- [A2A push DLQ ops](./A2A_PUSH_DLQ_OPS.md) — operator consumption of `{caller_id}` segments

# A2A auth callout — design reference

Stable design reference for the **`a2a-auth-callout`** service (`rsworkspace/crates/a2a-auth-callout`). It describes what shipped on the NATS perimeter and how that wiring connects to the rest of the A2A-over-NATS binding.

**Operator runbook** (pinned versions, env tables, NSC sample config, rotation): [`A2A_AUTH_CALLOUT_DEPLOYMENT.md`](../how-to/operators/auth-callout-deployment.md).

Related decisions: [A2A architecture §Decisions](architecture.md) (auth callout deployment, subject ACL, push DLQ).

---

## 1. Goal

The auth callout terminates **external credentials** at the NATS perimeter and **mints Account-scoped User JWTs** that clients use for subsequent NATS connections inside a tenant Account.

Two outcomes matter for downstream A2A work:

1. **Perimeter JWT** — every caller connects with a short-lived User JWT whose `aud` is the tenant NATS Account and whose publish/subscribe permissions are bounded to Phase 0 ACL templates (gateway ingress + caller inbox only).
2. **Stable `caller_id`** — the callout maps external identity (OIDC `sub`, mTLS cert subject, transitional API key principal) to a **token-safe, stable segment** reused across:
   - `_INBOX.{caller_id}.>` subscribe ACL on the minted User
   - `a2a.push.{caller_id}.>` consumer ACL (push read path; provision at mint time even though Phase 0 checklist emphasizes gateway-side publish)
   - `{prefix}.push.dlq.{caller_id}.{task_id}` DLQ subject segments when push delivery fails
   - Gateway ingress tracing, audit enrichment, and (later) SpiceDB resource attribution

The mapping **external principal → `caller_id`** is deterministic for a given tenant Account so ACL rows and DLQ subjects stay stable across reconnects and JWT re-mints.

---

## 2. NATS mechanics (auth callout)

NATS **Authorization Callout** ([NATS authorization callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout)) delegates credential validation to an external service. When callout is enabled on an Account, the server does **not** accept raw username/password or unsigned bearer tokens for that Account; instead it forwards an auth decision request to a trusted subscriber.

### Callout placement

- Configure the **tenant Account JWT** (or Operator policy) to reference the callout service identity and enable auth callout for User connections into that Account.
- Run **`a2a-auth-callout`** as a NATS client with permission to subscribe to `$SYS.REQ.USER.AUTH` (typically a dedicated service User on the system Account or an Operator-delegated callout principal — follow your org’s NATS operator model).

### Pinned wire format (`$SYS.REQ.USER.AUTH`)

Implementation lives in `rsworkspace/crates/a2a-auth-callout/src/wire/` and is pinned against NATS server **2.14.x** (see deployment doc).

| Layer | Behavior |
|-------|----------|
| **Subject** | Server publishes auth requests on `$SYS.REQ.USER.AUTH`; the callout **must** reply on the message inbox. |
| **Request JWT** | Server-signed NKey JWT (`ed25519-nkey`) with `aud` = `nats-authorization-request` and a `nats` block carrying `server_id`, `user_nkey`, `client_info`, `connect_opts`, optional `client_tls`. Verified with `AUTH_CALLOUT_SERVER_NKEY_PUBLIC` (public NKey of `authorization.auth_callout.issuer`). |
| **Response JWT** | Callout-signed NKey JWT whose `sub` is the request `user_nkey`, `aud` is `server_id.id`, and `nats.jwt` holds the minted user credential (or `nats.error` on denial). Signed with `AUTH_CALLOUT_ISSUER_NKEY_SEED`. |
| **XKey envelope** | Optional. When `auth_callout.xkey` is set on the server, the request payload may be encrypted with the account XKey; the callout decrypts with `AUTH_CALLOUT_XKEY_SEED` and seals responses using the server **one-time** public key from header `Nats-Server-Xkey`. Request decryption also uses `AUTH_CALLOUT_SERVER_XKEY_PUBLIC` (server **persistent** XKey). |
| **Denial** | Signed authorization-response JWT **without** `nats.jwt`; `nats.error` carries an opaque [`DenialCategory`](../../../rsworkspace/crates/a2a-auth-callout/src/denial_category.rs) string (never OIDC/x509/verifier exception text). |

Full wire walkthrough, version pins, and sample `nats-server` config: [`A2A_AUTH_CALLOUT_DEPLOYMENT.md`](../how-to/operators/auth-callout-deployment.md).

### Bridge path (internal JSON)

The HTTPS bridge does **not** speak the server JWT envelope. It request/replies on `a2a.bridge.auth.callout.request` with JSON `BridgeMintRequest` / `BridgeMintResponse`. The in-process `BridgeMintAdapter` synthesizes `ServerAuthRequestClaims` so the same `CalloutDispatcher` logic runs. Only the server-facing `Subscriber` needs the real wire format.

### Credential sources (preference order)

Landed in [A2A architecture §Decisions](architecture.md); implemented in `credentials/`:

1. **OIDC** — primary (`AUTH_CALLOUT_OIDC_ISSUER`, `AUTH_CALLOUT_OIDC_AUDIENCES`). JWKS discovery; map claims → tenant Account + `caller_id`.
2. **mTLS** — service-to-service (`AUTH_CALLOUT_MTLS_TRUST_ANCHORS`). Map certificate subject / SAN → Account principal.
3. **API keys** — transitional HMAC-SHA256 verifier (optional in dispatcher; not enabled in the default binary entry point).

The callout owns **all** verification logic; NATS sees only minted User JWTs with bounded ACLs.

---

## 3. Inner User JWT claim layout

Minted User JWTs (`nats.jwt` in the authorization response) carry enough structure for NATS ACL binding, gateway correlation, and (later) SpiceDB checks. **This repo does not define the SpiceDB principal schema** — only the NATS-facing contract (`jwt.rs`, `permissions.rs`).

### Standard JWT claims

| Claim | Role |
|-------|------|
| **`aud`** | NATS **Account** name (= tenant boundary). Must match the Account the client connects into. |
| **`sub`** | **External identity** as issued by the IdP or mTLS/API-key layer (opaque to NATS ACL templates). |
| **`exp` / `iat` / `nbf`** | Short-lived User credential; TTL from `AUTH_CALLOUT_USER_JWT_TTL_SECS` (default 300s). Re-mint on reconnect. |
| **`kid`** | Key version string on the JWS header and claim; matches the active `SigningKeySource` handle (`current` / `previous` during rotation). |

### Custom / namespaced claims

| Claim | Role |
|-------|------|
| **`caller_id`** | Stable, **token-safe** segment (no `.` characters) used in subject ACL tokens: `_INBOX.{caller_id}.>`, `a2a.push.{caller_id}.>`, `{prefix}.push.dlq.{caller_id}.{task_id}`. Derived from external `sub` via tenant-scoped rules in the minter. |
| **`data`** | **SpiceDB-ready principal payload** (`SpiceDbPrincipal`) — org-standard mapping from external identity to authorization tuples. Opaque to `a2a-nats` / `a2a-gateway`; consumed by gateway policy (Phase 1). |

### Issued permissions

`IssuedPermissions::default_for_caller` mirrors `scripts/acl-templates/caller.acl`: publish `{prefix}.gateway.>`, subscribe `_INBOX.{caller_id}.>` and `a2a.push.{caller_id}.>` (recommended at mint time for NATS push targets).

### Illustrative pseudo-JSON (not normative)

```json
{
  "aud": "APP",
  "sub": "oidc|tenant-acme|user-7f3a…",
  "caller_id": "usr_7f3a9c2b",
  "kid": "current",
  "data": {
    "principal_type": "…",
    "tenant_ref": "…",
    "spicedb_subject": "…"
  }
}
```

The in-tree minter emits NATS-native User JWTs (`ed25519-nkey`, `nats.pub` / `nats.sub` from `IssuedPermissions`, `sub` = caller User NKey, Account NKey `iss`) via `SigningKeySource`. Acceptance against a live `nats-server` is exercised by the testcontainer integration suite.

---

## 4. Signing key custody (inner `nats.jwt`)

Inner User JWTs are signed through the pluggable **`SigningKeySource`** trait (`signing_key_source/`). Select custody with `AUTH_CALLOUT_SIGNING_KEY_SOURCE`:

| Value | Use | Notes |
|-------|-----|--------|
| `env` | Local dev only | Default. Reads `AUTH_CALLOUT_SIGNING_SECRET` (+ optional `AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS`). Logs a one-time warning that env secrets are not for production. |
| `file` | Production / VMs | Requires `AUTH_CALLOUT_SIGNING_KEY_PATH` (current key material). Optional `AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH` for overlap during rotation. Raw PEM or shared-secret bytes (no JSON envelope). |
| `vault` | Planned | Not wired; process exits if selected. |

**Rotation:** `KeyVersion` labels (`current`, `previous`) identify overlap keys. Mint always uses `current()`; `accepted()` returns both during overlap. Minted JWTs carry `kid` (and matching JWS header `kid`). Verifiers select the decoding key from `accepted()` using `kid`, with trial verification only when `kid` is absent.

Operator rotation steps: [`A2A_AUTH_CALLOUT_DEPLOYMENT.md` § Signing key custody](../how-to/operators/auth-callout-deployment.md#signing-key-custody).

---

## 5. Account resolution

`AccountResolver` (`account_resolver.rs`) binds external principals to tenant Account names. The shipped binary uses **`StaticAccountResolver`** driven by `AUTH_CALLOUT_ALLOWED_ACCOUNTS` (comma-separated allowlist). Requests for accounts outside the list yield `unknown_account` on the wire.

---

## 6. Operator wiring

Account provisioning, User role templates, and JetStream bootstrap are documented in **[A2A NSC account bootstrap](../how-to/operators/nsc-account-bootstrap.md)**. The auth callout **replaces static per-caller `nsc add user`** for production human/OIDC identities; NSC steps remain the reference for **gateway**, **registrar**, and **caller ACL shape**.

### Phase 0 ACL rows (caller vs gateway)

From the bootstrap ACL table — substitute `{prefix}` if not using default `a2a`:

| NATS JWT role | Publish (allow) | Subscribe (allow) | Minted by |
|---------------|-----------------|-------------------|-----------|
| **Caller User** | `{prefix}.gateway.>` | `_INBOX.{caller_id}.>` | **Auth callout** (dynamic) |
| **Gateway User** | `{prefix}.agent.>`, `{prefix}.task.>`, `{prefix}.push.>` | `{prefix}.gateway.>` | **NSC** (long-lived service User) |

Additional bootstrap roles (registrar, agents) are unchanged; see the full table in [A2A NSC account bootstrap](../how-to/operators/nsc-account-bootstrap.md).

### Callout operator checklist (summary)

1. Enable auth callout on the tenant Account JWT per NATS operator docs.
2. Deploy `a2a-auth-callout` with `$SYS.REQ.USER.AUTH` subscription rights and wire-format NKey/XKey envs (see deployment doc).
3. Configure `SigningKeySource` and account allowlist for production custody.
4. Verify a minted caller User **cannot** publish `{prefix}.agent.>` or `{prefix}.catalog.register.*` (negative test).
5. Verify gateway User **cannot** subscribe `_INBOX.>` broadly (only `{prefix}.gateway.>`).

Static caller User templates in the bootstrap doc are **documentation-only** once runtime minting is live.

---

## 7. Repo integration points

Where auth callout work meets in-tree crates.

### `a2a-auth-callout` — service

- **`subscriber.rs`** — NATS loop on `$SYS.REQ.USER.AUTH`; decode server request, `CalloutDispatcher::dispatch`, encode signed response (success or `DenialCategory` denial).
- **`dispatcher.rs`** — Routes OIDC bearer / mTLS client cert / API-key material off `ServerAuthRequestClaims`, resolves Account, mints via `SigningKeySource`.
- **`main.rs`** — Assembles wire codec, signing-key source, resolver, and credential verifiers.

### `a2a-gateway` — ingress

- **Crate:** `rsworkspace/crates/a2a-gateway`
- **Today:** Queue-group subscriber on `{prefix}.gateway.>`; opaque forward to `{prefix}.agent.{agent_id}.{method}`. Caller identity from verified `A2a-Caller-Jwt` message header (`MessageCallerIdentitySource`); deprecated `X-A2a-Spicedb-Principal` / `X-A2a-Caller-Id` only when `A2A_GATEWAY_TRUST_CALLER_HEADERS=1` and no JWT header verifies. **`a2a-nats::Client`** (NATS-native HTTP adapter and direct embedders) attaches the same header on gateway ingress publishes when [`routing_via_gateway_ingress`](../../../rsworkspace/crates/a2a-nats/src/client/handle.rs) is configured with a `MintedUserJwt` carrier.
- **Target:** Parse minted User JWT / connection identity on each ingress message; extract **`caller_id`** for spans and Phase 1 audit; thread identity into SpiceDB checks.
- **Non-goal:** gateway does **not** publish push DLQ messages; terminal DLQ originates from the agent `Bridge` (see [A2A push DLQ ops](../how-to/operators/push-dlq-triage.md)).

### `a2a-nats` — `Config::push_dlq_caller_segment`

- **Field:** `Config::push_dlq_caller_segment` (builder: `with_push_dlq_caller_segment`)
- **Default:** `"_"` when no richer identity is available.
- **Used by:** agent `Bridge` / `message/stream` terminal push path → JetStream publish to `{prefix}.push.dlq.{caller_id}.{task_id}`.
- **When agents run behind minted identities:** gateway ingress forwards JWT `data` on NATS header **`X-A2a-Spicedb-Principal`**; agent **`Bridge`** reads it via **`PrincipalCarrier`** and derives DLQ **`{caller_id}`** from **`spicedb_subject`**. **`A2A_PUSH_DLQ_CALLER_SEGMENT`** remains the fallback when no principal arrives.
- **Constraint:** `{caller_id}` must be a **single NATS subject token** (no `.`).

### Cross-cutting flow

```
External client ──OIDC/mTLS/API key──► a2a-auth-callout ($SYS.REQ.USER.AUTH)
                         │
                         ▼ mint User JWT (aud=Account, caller_id, data)
              Caller NATS connect ──publish──► {prefix}.gateway.{agent}.{method}
                         │
                         ▼
              a2a-gateway ingress ──forward──► {prefix}.agent.{agent}.{method}
                         │                      (caller_id on span/audit when wired)
                         ▼
              Agent Bridge / message/stream ──on push failure──►
                         {prefix}.push.dlq.{caller_id}.{task_id}
```

### Out of scope for this design doc

- SpiceDB client wiring (Phase 1) beyond carrying `data` on the mint
- `a2a-bridge` HTTPS sidecar production NATS transport — same callout contract, different ingress ([`A2A_BRIDGE_SKETCH.md`](bridge-sketch.md))
- Tier 1–3 policy bundles (`a2a-pack`)

---

## See also

- [A2A auth callout deployment](../how-to/operators/auth-callout-deployment.md) — operator runbook
- [A2A runtime env](../reference/runtime-env.md) — `AUTH_CALLOUT_*` env reference
- [A2A TODO](architecture.md) — Phase 0 checklist
- [A2A NSC account bootstrap](../how-to/operators/nsc-account-bootstrap.md) — Operator / Account / User hierarchy and ACL table
- [A2A per-Account JetStream assets](../reference/jetstream-account-streams.md) — `A2A_PUSH_DLQ` subject shape
- [A2A push DLQ ops](../how-to/operators/push-dlq-triage.md) — operator consumption of `{caller_id}` segments

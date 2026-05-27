# ADR 0006 — Mesh Token Signing Key Custody

## Status

**DRAFT** — for human review. Blocks STS implementation (PENDING_TODO Block 2.1) and gateway egress minting (Block 2.4).

## Context

The mesh Security Token Service (STS) will **mint** short-lived, audience-scoped JWTs per hop. Today `trogon-mcp-gateway` only **validates** inbound JWTs via `MCP_GATEWAY_JWT_*`: JWKS URI, static RSA PEM, or HS256 secret, with phased `off` / `validate` / `require` (`MCP_GATEWAY_PLAN.md:80`). Minting introduces a new trust anchor: whoever holds the STS signing key can impersonate any mesh identity within the claims the STS is allowed to emit.

Requirements from Block 0 and Block 2.1:

- **Online rotation** — no full-mesh outage; overlap window where old and new keys both verify.
- **Auditability** — security and compliance need a durable record of signing operations (who minted what, when, with which key version).
- **Multi-region** — STS may be regional; verifiers (gateways, backends, peer STS instances) must converge on the same public key set without a single global chokepoint.
- **Separation from bootstrap credentials** — NATS auth-callout User JWTs remain a distinct issuer (`iss`) and key lineage from mesh tokens (see ADR 0003, when published).

Mesh tokens should use **RS256 or ES256** (P-256) so existing gateway JWKS verification paths apply. NATS-shaped `ed25519-nkey` User JWTs remain appropriate for connect-time NATS credentials, not for RFC 7519 mesh tokens consumed by HTTP/MCP gateways.

## Options considered

### (a) NATS Operator keystore (NKey-style)

Store an Account or Operator NKey seed in the NATS Operator keystore; STS signs mesh JWTs with that material (possibly wrapped as JWS with a `kid` derived from the NKey public key).

| Driver | Assessment |
|--------|------------|
| Rotation cadence | Operator tooling supports key rollover; overlap requires manual or scripted dual-key deploy similar to auth-callout `current` / `previous`. |
| Audit trail | NATS server logs only; no per-sign API audit unless STS emits its own audit events. |
| Multi-region | Seeds must be replicated securely to each STS replica; no native geo-redundant HSM. |
| Dev ergonomics | Good if devs already run Operator-generated seeds; poor if mesh tokens must be RS256 and NKey is Ed25519-only without translation. |
| Cost | Low (no cloud KMS line item). |

**Fit:** Reuse patterns from `a2a-auth-callout` inner User JWT signing, but **not** ideal as the primary mesh-token algorithm if verifiers expect RSA/EC JWKS.

### (b) Cloud KMS (AWS KMS / GCP KMS / Azure Key Vault)

STS calls `Sign` on an asymmetric key; private key never leaves the HSM. Public key exported for JWKS; `kid` = KMS key version ARN / version id.

| Driver | Assessment |
|--------|------------|
| Rotation cadence | Native automatic or scheduled key rotation with version history; both versions verify during overlap. |
| Audit trail | CloudTrail / Cloud Audit Logs record every `Sign` call — strong compliance story. |
| Multi-region | Multi-region keys (AWS) or regional replicas with replicated key material; verifiers pull one JWKS document listing all active versions. |
| Dev ergonomics | Requires cloud credentials locally; LocalStack / emulator partial. |
| Cost | Per-request signing cost at STS exchange volume; usually acceptable vs. breach cost. |

**Fit:** Strong default for production when STS runs in the same cloud as the control plane.

### (c) HashiCorp Vault Transit

STS uses Transit `sign` / `verify` with a named key; export public keys for JWKS assembly. Rotation via `rotate` + `min_decryption_version` / `min_encryption_version` semantics.

| Driver | Assessment |
|--------|------------|
| Rotation cadence | Built-in rotation with versioned keys; overlap window configurable. |
| Audit trail | Vault audit device logs every Transit operation. |
| Multi-region | Performance Replication or DR secondaries; JWKS publisher must aggregate versions from the primary. |
| Dev ergonomics | `vault server -dev` works; aligns with planned `AUTH_CALLOUT_SIGNING_KEY_SOURCE=vault`. |
| Cost | Vault Enterprise for replication; OSS sufficient for single-cluster dev. |

**Fit:** Best when TrogonAI already operates Vault for secrets and wants cloud-agnostic HSM-backed signing.

### (d) File-based PEMs (dev only)

Raw PEM files on disk (`current` + optional `previous`), mirroring `AUTH_CALLOUT_SIGNING_KEY_PATH` / `_PREVIOUS_PATH`. Process reads at startup or on SIGHUP reload.

| Driver | Assessment |
|--------|------------|
| Rotation cadence | Manual file swap + redeploy or reload; acceptable for dev, risky for prod. |
| Audit trail | None beyond OS file access logs. |
| Multi-region | Copy files to every host — error-prone, no centralized version authority. |
| Dev ergonomics | Excellent — matches smoke tests, CI, and offline work. |
| Cost | Zero. |

**Fit:** **Dev and CI only.** Explicitly forbidden in production policy (same rule as `AUTH_CALLOUT_SIGNING_KEY_SOURCE=env`).

## Decision drivers (summary)

| Driver | Weight | Notes |
|--------|--------|-------|
| Rotation cadence | High | Mesh TTL 60–300 s (ADR 0005); signing key rotation is slower (days–weeks) but must not invalidate in-flight tokens. |
| Audit trail of signing | High | Every STS exchange is audited; signing-key operations should be independently auditable in the custody backend. |
| Multi-region replication | Medium | JWKS is the replication surface; custody backend choice affects how fast new keys appear in all regions. |
| Dev ergonomics | Medium | Must work offline without KMS/Vault for Block 6 migration and SDK development. |
| Cost | Low–medium | STS sign volume ≈ exchange rate; dominated by engineering time if rotation is manual. |

## JWKS publication

Verifiers need a stable way to fetch `{ keys: [ { kty, kid, use, alg, n/e or x/y/crv } ] }` for the mesh STS `iss`.

### Publication channels

| Channel | Subject / location | Pros | Cons |
|---------|-------------------|------|------|
| **NATS request/reply** | `mcp.jwks.mesh.get` (singleton) or `mcp.jwks.mesh.{region}` | On-bus consumers (gateways, NATS-hosted STS) need no HTTP egress; fits NATS-native mesh. | External OAuth clients cannot use it without a bridge. |
| **HTTPS well-known** | `https://sts.<env>.<domain>/.well-known/jwks.json` | Standard OIDC/JWT ecosystem; `MCP_GATEWAY_JWT_JWKS_URI` already supports HTTPS JWKS. | Requires TLS termination and DNS; extra hop for in-mesh NATS-only callers. |
| **NATS KV bucket** | Bucket `mcp-jwks`, key `mesh/current` (JSON JWKS document) | Durable, watchable; one writer (JWKS publisher sidecar) pushes after rotation; subscribers get push updates. | Not a substitute for signed HTTP discovery in federated scenarios unless KV access is trusted. |

**Recommendation:** Publish through **all three**, with one **authoritative writer**:

1. **Source of truth:** KMS/Vault holds private keys; a **JWKS publisher** component exports the public set after every rotation event.
2. **Primary consumer path (in-mesh):** NATS KV `mcp-jwks` / `mesh/current` — gateways and STS replicas watch and hot-reload.
3. **Secondary (interop):** HTTPS `/.well-known/jwks.json` — same document bytes as KV value.
4. **Tertiary (debug / bootstrap):** `mcp.jwks.mesh.get` request/reply returns the current document for callers that cannot watch KV yet.

### Refresh cadence

| Consumer | Refresh strategy |
|----------|------------------|
| `trogon-mcp-gateway` | KV watcher (immediate on change) **or** HTTPS poll with TTL ≤ 5 min (matches existing JWKS cache ~5 min in README). |
| STS (verify `subject_token`) | In-memory cache + KV watch; trust bundle refresh async (Block 1.2). |
| External IdP / OAuth-MCP | HTTPS only; honor `Cache-Control: max-age=300`. |

### Backward compatibility during rotation

1. Publisher adds **new** key to JWKS with a new `kid`; keeps **previous** key in the set.
2. STS signs new tokens with the new `kid` only (`current` semantics, same as auth-callout).
3. Overlap window ≥ **max mesh token TTL** (ADR 0005) + clock skew (`MCP_GATEWAY_JWT_LEEWAY_SECS`, default 60 s).
4. After overlap, publisher removes the old `kid` from JWKS; verifiers reject tokens signed with retired keys.
5. Emergency revocation: remove `kid` from JWKS immediately; accept brief legitimate-token rejection (fail-closed).

Distinct `iss` values separate bootstrap (auth-callout) from mesh (STS) keys so rotation of one does not invalidate the other.

## Recommendation

| Environment | Signing custody | JWKS consumption |
|-------------|-----------------|------------------|
| **Production** | **Cloud KMS** when STS runs in AWS/GCP/Azure; **Vault Transit** when Vault is already the org standard or multi-cloud signing is required. | KV watch primary; HTTPS well-known for external verifiers. |
| **Development / CI** | **File-based PEM** (`MCP_STS_SIGNING_KEY_PATH` + optional `_PREVIOUS_PATH`), RS256 2048-bit or ES256 P-256. | Static PEM via `MCP_GATEWAY_JWT_RSA_PUBLIC_KEY_PEM` or local JWKS file served by test harness; HS256 only for unit tests. |

**Not recommended for mesh tokens:** NATS Operator NKey as the signing primitive (option a) — keep NKeys for NATS account/callout JWTs; use KMS/Vault RSA/EC for mesh JWTs to align with gateway JWKS verification.

**Rationale:** Production needs HSM-backed keys, API-level signing audit, and versioned rotation without redeploying PEM files to N hosts. Dev needs zero cloud dependencies. A single JWKS document replicated via KV + HTTPS decouples custody from verification and supports multi-region STS without sharing private key material across regions (each region's STS calls the same KMS key or a replicated Vault key).

## Consequences

### Positive

- Gateways can validate mesh tokens with existing RSA/EC JWKS code paths.
- Rotation overlap matches auth-callout operational muscle memory (`current` / `previous`, `kid` in JWS header).
- Cloud audit logs provide independent evidence for compliance (SR-05 in threat model).

### Negative / follow-on work

- **`trogon-mcp-gateway` validator config** must grow beyond `MCP_GATEWAY_JWT_JWKS_URI`:
  - `MCP_GATEWAY_JWT_JWKS_SOURCE` = `uri` | `nats_kv` | `static_pem` (default `uri` for backward compatibility).
  - When `nats_kv`: `MCP_GATEWAY_JWT_JWKS_KV_BUCKET` (default `mcp-jwks`), `MCP_GATEWAY_JWT_JWKS_KV_KEY` (default `mesh/current`), with watcher-driven reload.
  - Support **multiple active keys** in one JWKS document (required for rotation overlap).
  - `MCP_GATEWAY_JWT_ISSUERS` must include the mesh STS issuer URL in `enforce` mode.
- **New STS crate/service** needs a `SigningKeySource` trait (mirror `a2a-auth-callout`) with backends: `kms`, `vault`, `file`, and `dev_only_env`.
- **JWKS publisher** job or sidecar: watch KMS/Vault key versions → assemble JWKS → write KV + serve HTTP.
- **Operational runbook** for rotation (generate → publish dual-key JWKS → wait TTL → retire old `kid`), analogous to auth-callout deployment doc.

### Neutral

- Bootstrap auth-callout signing keys remain separate; no change to `AUTH_CALLOUT_SIGNING_KEY_*` unless ADR 0003 collapses issuers (unlikely).

## Open questions

- [ ] **Single global KMS key vs. per-region keys** under one `iss` — one JWKS with multiple `kid`s, or regional `iss` subdomains?
- [ ] **JWKS publisher ownership** — sidecar with STS, standalone control-plane job, or KMS-triggered Lambda?
- [ ] **Algorithm choice:** RS256 (widest compatibility) vs. ES256 (smaller tokens, already supported by gateway)?
- [ ] **Vault vs. KMS** when both exist — pick one org-wide standard or support both via `SigningKeySource` config?
- [ ] **Emergency revocation SLA** — how quickly must `kid` removal propagate via KV watch vs. HTTPS cache TTL?
- [ ] **OAuth-MCP composition** (`MCP_GATEWAY_PLAN.md:67`) — does the external IdP trust mesh JWKS over HTTPS, or only internal KV?
- [ ] **Per-tenant signing keys** (ADR 0001) — one KMS key per tenant vs. single mesh key with tenant in claims?
- [ ] **Auth-callout `vault` source** — implement once and share Vault client/config with STS, or separate paths?

## References

- `PENDING_TODO.md` — Block 0 (Key material custody), Block 2.1 (STS)
- `MCP_GATEWAY_PLAN.md:80` — gateway JWT validation modes
- `docs/a2a/how-to/operators/auth-callout-deployment.md` — rotation overlap pattern
- `rsworkspace/crates/trogon-mcp-gateway/README.md` — `MCP_GATEWAY_JWT_*` variables
- `docs/security/threat-model.md` — SR-05 operator key compromise

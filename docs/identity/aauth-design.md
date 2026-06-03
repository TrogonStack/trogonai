# AAuth Design for Trogon (HTTP + NATS)

Status: draft, decisions locked for v0 implementation on `yordis/agentgateway`.
Spec source: `draft-hardt-aauth-protocol-02` and `draft-hardt-aauth-bootstrap-01`.

This doc records the binding decisions taken without round-tripping with the user, so
that implementation can proceed unblocked. Every decision lists why the alternative was
rejected.

## D1. Token format

- **Algorithm:** `ES256` (EC P-256). Matches what `cnf.jwk` is expected to carry in
  the spec, composes with existing `jsonwebtoken` + `JwkSet` code in
  `trogon-mcp-gateway::jwt`, and is already wired in `pick_jwk`.
- **Header `typ` values** per spec: `aa-agent+jwt`, `aa-resource+jwt`, `aa-auth+jwt`.
- **EdDSA (Ed25519)** is allowed as an opt-in alg for self-hosted agents (Tier-1
  hardware-backed keys often expose Ed25519). Not the default.

Rejected: HS256 (symmetric), RS256 (too large for header-budget PoP), NATS NKeys
(not a JWK format).

## D2. Proof-of-possession

- **HTTP path:** RFC 9421 HTTP Message Signatures, as written in the spec.
  Covered components: `@method`, `@target-uri`, `content-digest`, `signature-key`.
  Token is presented via `Signature-Key: sig=jwt; jwt="<aa-agent+jwt>"`.

- **NATS path:** A new "NATS Message Signature" envelope, structurally analogous to
  RFC 9421 but adapted for NATS. Carried as NATS headers on the message:
  - `AAuth-Token`: the `aa-agent+jwt`.
  - `AAuth-Sig-Created`: integer Unix seconds.
  - `AAuth-Sig-Nonce`: random 128-bit hex; gateway tracks for replay window.
  - `AAuth-Sig-Input`: ordered, comma-separated list of covered components:
    `("@subject" "@reply" "content-digest" "aauth-token" "aauth-sig-created" "aauth-sig-nonce")`.
  - `AAuth-Sig`: base64url-encoded P-256 ECDSA signature over the canonical base.
  - `Content-Digest`: `sha-256=:<b64>:` of the raw NATS payload bytes (lowercase
    field name and `sha-256` per RFC 9530).

  Canonical base (mirrors RFC 9421 shape):
  ```
  "@subject": <subject>
  "@reply": <reply-or-empty>
  "content-digest": sha-256=:<digest>:
  "aauth-token": <jwt>
  "aauth-sig-created": <unix-seconds>
  "aauth-sig-nonce": <hex>
  "@signature-params": ("@subject" "@reply" "content-digest" "aauth-token" "aauth-sig-created" "aauth-sig-nonce");created=<ts>;keyid="<jkt>"
  ```

Rejected: NATS NKey signing (good for connection auth, wrong layer for end-to-end
agent identity), wrapping the whole payload in JOSE JWS (forces every backend to
unwrap and bloats payload), and DPoP (HTTP-only and the spec already chose RFC 9421).

## D3. Challenge envelope

- **HTTP:** `401 Unauthorized` with
  `AAuth-Requirement: requirement=auth-token; resource-token="<aa-resource+jwt>"`.
- **NATS:** Resource reply with NATS headers:
  - `Nats-Service-Error-Code: 401`
  - `AAuth-Requirement: requirement=auth-token; resource-token="<aa-resource+jwt>"`
  Body left empty. Mirrors the HTTP envelope so SDKs share parser code.

## D4. Person Server transport

The PS speaks both HTTP and NATS. NATS is first-class; HTTP is a thin axum facade
that calls the same handler crate.

NATS subject layout (prefix configurable, default `aauth`):

| Endpoint           | NATS subject                          | HTTP route                       |
|--------------------|---------------------------------------|----------------------------------|
| Metadata           | `aauth.{ps_id}.meta.get`              | `GET /.well-known/aauth-person.json` |
| JWKS               | `aauth.{ps_id}.jwks.get`              | `GET /.well-known/jwks.json`     |
| Bootstrap          | `aauth.{ps_id}.bootstrap`             | `POST /aauth/agent`              |
| Token exchange     | `aauth.{ps_id}.token`                 | `POST /aauth/token`              |
| Permission         | `aauth.{ps_id}.permission`            | `POST /aauth/permission`         |
| Audit              | `aauth.{ps_id}.audit`                 | `POST /aauth/audit`              |
| Interaction (v2)   | `aauth.{ps_id}.interaction`           | `POST /aauth/interaction`        |
| Mission (v2)       | `aauth.{ps_id}.mission`               | `POST /aauth/mission`            |

Queue group: `trogon-aauth-person-{ps_id}` for horizontal scaling.

## D5. JWKS distribution

Reuse `trogon-jwks-publisher` (NATS KV + req-rep model) for AAuth keys:

- KV bucket per role: `aauth-ps-jwks`, `aauth-resource-jwks`, `aauth-agent-jwks`.
- KV key: `current` (active set) plus history.
- Req-rep subjects above.
- HTTP `.well-known/*` endpoints read the same in-process JWKS state.

## D6. Storage

NATS JetStream KV is the default backing store. Buckets:

| Bucket              | Key                          | Value                            |
|---------------------|------------------------------|----------------------------------|
| `aauth-agents`      | `{agent_id}`                 | agent record + current `cnf.jwk` |
| `aauth-resources`   | `{resource_id}`              | resource registration            |
| `aauth-consents`    | `{principal}/{agent}/{rsrc}` | granted scopes + expiry          |
| `aauth-jti`         | `{jti}`                      | replay-protection sentinel (TTL) |
| `aauth-nonces`      | `{nonce}`                    | replay-protection sentinel (TTL) |
| `aauth-ps-keys`     | `current`                    | signing key material (encrypted) |

Pluggable trait for tests/in-memory.

## D7. Access modes shipped in v0

- **Identity-based** (mandatory). Resource verifies `aa-agent+jwt` + PoP and applies
  CEL policy. No exchange.
- **PS-managed (3-party)** (mandatory). Resource emits `aa-resource+jwt` challenge;
  agent exchanges at PS for `aa-auth+jwt`; resource verifies on retry.

Deferred to a follow-up: resource-managed 2-party (consent at resource), 4-party
federated, mission tokens, interaction relay UI, audit endpoint storage backend.

## D8. Crate layout

Add to existing workspace (no new top-level renames):

- Extend `trogon-identity-types` with an `aauth` module: typ enums, claim structs,
  PoP envelope structs, error types. No runtime dependencies beyond `serde`.
- New `trogon-aauth-verify` (lib): token + PoP verification (HTTP and NATS),
  challenge minter, JWKS resolver.
- New `trogon-aauth-person` (lib + bin): Person Server core + axum/NATS adapters.
- New `trogon-aauth-sdk` (lib): agent-side keypair, signing, bootstrap, 3-party loop,
  HTTP + NATS transports.

Gateway changes: `trogon-mcp-gateway::aauth` module that wires
`trogon-aauth-verify` into the existing ingress path next to (not replacing)
`jwt.rs`. Selection by config knob `MCP_GATEWAY_AAUTH_MODE = off|shadow|enforce`.

## D9. Replay protection

- `jti` and `AAuth-Sig-Nonce` recorded in `aauth-jti` / `aauth-nonces` KV with TTL =
  token `exp - iat + 60s` skew.
- `AAuth-Sig-Created` must be within `±300s` of server time.
- Signature components include the nonce so identical replays fail.

## D10. Tenancy

AAuth `iss` and `sub` map onto existing `GatewayIdentity`:

- `caller_sub` ← `sub` from `aa-auth+jwt` (or `agent` claim if identity-only).
- `agent_id` ← `sub` from `aa-agent+jwt`.
- `issuer` ← `iss`.
- `tenant` derived from a configurable AAuth claim (`tenant`, default), matching
  the existing `MCP_GATEWAY_JWT_TENANT_CLAIM` convention.

## D11. Out of scope for v0

- 4-party federation across PS/AS.
- Mission tokens, deferred 202 responses, interaction relay UX.
- Hardware-key attestation in bootstrap (we accept any registered pubkey).
- Cross-PS trust chain discovery.
- Multi-tenant PS isolation beyond config namespacing.

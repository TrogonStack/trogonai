# AAuth Implementation Plan

Reference: [AAuth Full Demo — Christian Posta](https://blog.christianposta.com/aauth-full-demo/)

> **Status:** v0 implementation landed on `yordis/agentgateway`. The runtime
> handshake is documented in [`docs/identity/aauth-handshake.md`](./docs/identity/aauth-handshake.md);
> the binding design decisions live in [`docs/identity/aauth-design.md`](./docs/identity/aauth-design.md).
> Crates shipped: `trogon-aauth-verify`, `trogon-aauth-person`, `trogon-aauth-sdk`,
> plus `aauth.rs` ingress wired into `trogon-mcp-gateway`.

AAuth ("Agent Auth") is an IETF draft by Dick Hardt (original OAuth 2.0 author) that
defines identity and access management primitives purpose-built for autonomous agents
calling MCP / A2A / HTTP resources. This plan maps the protocol onto Trogon's existing
gateway + identity stack and lists the deltas we need to ship.

## 1. What AAuth Defines

Three JWT token types:

- `aa-agent+jwt` — non-bearer identity token, issued by an **Agent Provider** (Person
  Server) at agent bootstrap. Asserts who the agent is without authenticating a user.
- `aa-resource+jwt` — challenge token returned by a resource on `401`, telling the
  agent which Person Server / scopes it must obtain authorization from.
- `aa-auth+jwt` — authorization token, obtained by the agent exchanging the
  `aa-resource+jwt` at its Person Server. Carries user consent / delegation claims.

Two access modes:

1. **Identity-based** — agent presents `aa-agent+jwt`, resource verifies locally,
   policy decides. No 3-party flow.
2. **3-party (Person-Server-managed)** — resource issues `aa-resource+jwt` challenge,
   agent exchanges it for `aa-auth+jwt`, retries with the new token.

Cross-cutting requirements:

- Proof-of-possession: agent signs every request with a key bound to its token
  (DPoP-style), so stolen tokens cannot be replayed.
- JWKS distribution for both Person Server and agent keys.
- Any HTTP/MCP/A2A resource becomes "AAuth-compliant" via an ExtAuthz wrapper —
  resources are not modified.

## 2. Reference Demo Components

| # | Component                | Role                                                                 |
|---|--------------------------|----------------------------------------------------------------------|
| 1 | Agentgateway             | PEP for LLM/MCP/A2A traffic, applies AAuth policy                    |
| 2 | `aauth-service` ExtAuthz | Envoy ExtAuthz that turns any resource into an AAuth resource        |
| 3 | Python AAuth library     | Agent-side: key gen, request signing, token exchange                 |
| 4 | Go AAuth library         | Resource-side: token + PoP signature validation                      |
| 5 | AAuth Person Server      | Issues `aa-agent+jwt` and `aa-auth+jwt`; runs user consent UX        |
| 6 | Demo agents + backends   | Supply-chain & market-analysis agents exercising both access modes   |

## 3. Mapping to Existing Trogon Crates

Most of the surface area already exists; AAuth is largely a new token format + a
Person Server + signing libraries on top.

| AAuth role                        | Existing crate / doc                                             | Status |
|-----------------------------------|------------------------------------------------------------------|--------|
| PEP for MCP                       | `trogon-mcp-gateway`                                             | exists |
| PEP for A2A                       | `a2a-gateway`, `a2a-auth-callout`                                | exists |
| Identity types / JWT claims       | `trogon-identity-types`, `docs/identity/jwt-claim-schema.md`     | extend |
| Token exchange / STS              | `trogon-sts`, `trogon-sts-client`, `docs/identity/sts-exchange.md` | extend |
| JWKS publishing                   | `trogon-jwks-publisher`                                          | extend |
| OAuth integration                 | `docs/identity/oauth-mcp-integration.md`                         | extend |
| ExtAuthz callout pattern          | `a2a-auth-callout`, `docs/identity/nats-callout-plugin.md`       | reuse  |
| Person Server (Agent Provider)    | —                                                                | **new** |
| Agent-side signing SDK (Python)   | `pyworkspace/packages/a2a-sdk`                                   | deferred |
| Agent-side signing SDK (Rust)     | `rsworkspace/crates/trogon-aauth-sdk`                            | shipped |
| Resource-side verifier (Rust)     | `rsworkspace/crates/trogon-aauth-verify`                         | shipped |
| Person Server (Agent Provider)    | `rsworkspace/crates/trogon-aauth-person`                         | shipped |

## 4. Deltas to Ship

### 4.1 Token & claim definitions
- Add `aa-agent+jwt`, `aa-resource+jwt`, `aa-auth+jwt` typ values and claim structs
  to `trogon-identity-types`.
- Extend `docs/identity/jwt-claim-schema.md` with the AAuth claim set
  (`agent_id`, `principal`, `cnf` PoP key, `resource`, `consent_id`, …).
- Decide PoP binding: DPoP (RFC 9449) header style vs. JWS-signed request envelope.

### 4.2 Person Server (new service)
- Endpoints:
  - `POST /aauth/agent` — bootstrap, returns `aa-agent+jwt` bound to agent's pubkey.
  - `POST /aauth/token` — exchange `aa-resource+jwt` → `aa-auth+jwt`.
  - `GET  /.well-known/jwks.json` — signing keys (via `trogon-jwks-publisher`).
  - `GET  /aauth/consent/...` — user-facing consent UX.
- Persist: agents, registered resources, consent grants, key material.
- Threat model entry in `docs/security/threat-model.md`.

### 4.3 Resource-side enforcement
- New `trogon-aauth-verify` crate (Rust): verify token typ, signature, PoP, audience,
  scope, freshness.
- Wire it into `trogon-mcp-gateway` and `a2a-gateway` as an auth mode alongside
  current OAuth/STS. Reuse the ExtAuthz callout shape from `a2a-auth-callout`.
- Standard 401 challenge emitter that mints `aa-resource+jwt`.

### 4.4 Agent-side SDKs
- Rust: extend `trogon-a2a-sdk` (and an `mcp` sibling) with: keypair gen, bootstrap
  call, request signing, automatic 401→exchange→retry loop.
- Python: same surface in `pyworkspace/packages/a2a-sdk` so existing demo agents can
  adopt AAuth with a client switch.

### 4.5 Policy & observability
- Policy: expose AAuth claims (`agent_id`, `principal`, `consent_id`) as CEL
  variables — update `docs/identity/reference-cel-variables.md`.
- Audit: include token typ + `consent_id` in the audit envelope
  (`docs/identity/reference-audit-envelope.md`).
- OTel: spans for bootstrap, challenge, exchange, retry (per `docs/identity/otel-wiring.md`).

### 4.6 Demo & docs
- End-to-end demo under `docs/get-started/` mirroring the Posta walkthrough:
  supply-chain agent + market-analysis agent against an MCP backend through
  `trogon-mcp-gateway`, both identity-based and 3-party modes.
- How-to: "Make an existing MCP resource AAuth-compliant" using the gateway ExtAuthz path.

## 5. v0 Shipping Status

- `trogon-identity-types::aauth` — token typ constants (`aa-agent+jwt`,
  `aa-resource+jwt`, `aa-auth+jwt`), claim structs, `Requirement` header
  parser, NATS PoP envelope and canonical-base builder.
- `trogon-aauth-verify` — `TokenVerifier`, `NatsPopVerifier` (RFC 9421-shaped
  envelope for NATS), `ChallengeMinter` for `aa-resource+jwt`,
  `InMemoryReplayStore`, `StaticJwks`, `jwk_thumbprint`.
- `trogon-aauth-person` — `PersonCore::{bootstrap, exchange}`, in-memory store,
  consent policy trait + `AllowConfiguredScopes` MVP policy, HTTP router and
  NATS service (`aauth.{ps_id}.{bootstrap, token, jwks.get}`).
- `trogon-aauth-sdk` — `AgentKeypair` (ES256), `PersonHttpClient`,
  `PersonNatsClient`, `NatsRequestSigner`, `HttpRequestSigner`,
  `parse_challenge_headers` for the 401-retry loop.
- `trogon-mcp-gateway::aauth` — `AAuthIngress` wired into the existing
  `GatewayIdentity` / `IdentityDeny` shapes; emits the canonical
  `AAuth-Requirement` challenge header on enforce-mode deny.

Tests proving the loop:

- `crates/trogon-aauth-verify/tests/nats_pop_roundtrip.rs`
- `crates/trogon-aauth-person/tests/end_to_end.rs`
- `crates/trogon-aauth-sdk/tests/sdk_against_person_core.rs`
- `crates/trogon-aauth-sdk/tests/gateway_demo.rs`

Deferred from v0 (see `docs/identity/aauth-handshake.md` for the list):
2-party flow, 4-party federation, mission tokens, interaction-relay UI,
NATS KV-backed replay store, Python SDK.

## 6. Phasing

1. **Spec lock** — token types, PoP scheme, claim schema, JWKS layout. Land in
   `trogon-identity-types` + docs. No runtime code yet.
2. **Verifier** — `trogon-aauth-verify` + integration in `trogon-mcp-gateway` behind
   a feature flag; serves the 401 challenge but accepts existing tokens too.
3. **Person Server MVP** — bootstrap + exchange only, no consent UI. Static
   resource registry from config.
4. **Agent SDKs** — Rust then Python; close the loop end-to-end identity-based mode.
5. **3-party + consent UX** — `aa-auth+jwt` issuance, user consent screens, replay
   the Posta demo against our gateway.
6. **A2A coverage** — extend verifier + SDK to `a2a-gateway` and `trogon-a2a-sdk`.
7. **Hardening** — key rotation, JWKS caching, rate limits, threat-model review.

## 7. Open Questions

- PoP shape: DPoP vs. signed-envelope. DPoP composes better with HTTP intermediaries;
  signed-envelope is friendlier to MCP/A2A non-HTTP transports (e.g. NATS bridge).
- Relationship between `aa-auth+jwt` and our existing STS-exchanged tokens — keep
  both, or have STS mint `aa-auth+jwt` natively?
- Multi-tenant Person Server vs. one per org — affects JWKS discovery and trust roots.
- Where consent state lives (NATS KV vs. relational) given existing on-bus patterns
  in `docs/identity/on-bus-vs-hybrid.md`.

## 8. Out of Scope (for v1)

- Cross–Person-Server federation / trust chains.
- Hardware-bound agent keys (TPM / TEE attestation).
- Human-in-the-loop step-up auth beyond the basic consent screen.

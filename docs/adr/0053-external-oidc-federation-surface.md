---
number: "0053"
slug: external-oidc-federation-surface
status: draft
date: 2026-08-07
---

# ADR#0053: External OIDC Federation Surface for Agent Identity

## Context

[ADR#0036](./0036-agent-self-certifying-identity.md) anchors an
[agent](../glossary/agent) to a self-certifying key and defers a *global
resolution* layer, "publishing the identifier-to-current-key mapping to a
replicated substrate so parties outside this system can resolve it", with
root-key rotation deferred alongside it because the two arrive together.
[ADR#0038](./0038-agent-identity-crypto-suite.md) pins the suite and makes
crypto-agility structural. [ADR#0017](./0017-aauth-agent-authentication.md)
ships the verification path and a well-known publisher. None of them says how
an agent identity is presented to a party that is not this platform and does
not speak AAuth.

That gap is now load-bearing, because the shape of the answer is not the one
the deferred layer implies. Every cloud identity provider that accepts an
external workload assertion, Microsoft Entra's workload identity federation
being the concrete case examined here, consumes exactly one thing: an OIDC
discovery document plus a JWKS, over HTTPS, at a stable issuer URL. It does
not resolve DIDs, read transparency logs, or fetch AAuth well-known
documents. The pragmatic substrate for
[ADR#0036](./0036-agent-self-certifying-identity.md)'s deferred layer is the
boring one that already has universal client support.

Three facts about that surface make it a decision rather than an
implementation detail:

1. **It is not the surface we already publish.**
   `trogon-jwks-publisher` serves `GET /.well-known/{dwk}` for the four
   filenames the AAuth draft registers, and each response is a bare `JwkSet`.
   There is no `/.well-known/openid-configuration`, no `issuer` field, and no
   `jwks_uri`. A key set and a discovery document both publish trust material
   over HTTPS, and they are still different protocol surfaces: an IdP handed
   an AAuth `dwk` URL as an issuer will fail discovery before it ever looks at
   a key.

2. **Our entire algorithm allowlist is excluded.** Entra's federation supports
   only RS256-signed issuers and additionally requires the published key set
   to contain *nothing but* RSA signing keys. `trogon-aauth-verify` admits
   `ES256 | ES384 | EdDSA`; `trogon-jwks-publisher` mints ES256;
   [ADR#0038](./0038-agent-identity-crypto-suite.md) makes Ed25519 the default
   root. Every one of those is refused. The "nothing but RSA" half is the
   sharper constraint, because it means an RSA key cannot simply be added
   alongside the existing ones in a shared document.

3. **The mapping, not the token, is where the trust decision lives.** An IdP
   binds an external assertion to a governed identity through an exact,
   case-sensitive match on `iss`, `sub`, and `aud`, with no wildcards and a
   hard cap on how many such bindings one identity may hold. A valid assertion
   proves which workload is running; it grants nothing. If the mapping is
   wrong, a short-lived, correctly-signed token only lets the wrong workload
   become the wrong principal faster.

The platform also already holds a position this surface must not quietly
contradict. [ADR#0037](./0037-agent-identity-governance.md) requires that "the
signature is valid" and "the agent is currently authorized" never collapse
into one check, and [ADR#0049](./0049-revocation-latency-target.md) commits to
a p99 five-second revocation-to-invalidation target. Neither survives a
federation boundary unamended: a third-party IdP will not call this
platform's status list.

## Decision

### 1. The federation surface is a separate issuer, not an added key

External OIDC federation is served by its own issuer origin, publishing its
own `/.well-known/openid-configuration` and its own `jwks_uri`, distinct from
the AAuth well-known documents of
[ADR#0017](./0017-aauth-agent-authentication.md). The two are not merged and
the AAuth `dwk` documents are never advertised as an OIDC issuer.

This is forced, not stylistic. The IdP constraint that the key set contain
only RSA keys is incompatible with an AAuth document holding the EC and OKP
keys the mesh verifies against, so one document cannot serve both. Keeping
them separate also keeps the blast radius separate: the federation issuer is
internet-facing and consumed by parties outside our control, while the AAuth
documents serve the mesh.

### 2. RS256 only on that surface, under the conditional profile

Assertions minted for external federation are signed RS256 over RSA-2048
keys, adopted as the conditional profile
[ADR#0038](./0038-agent-identity-crypto-suite.md) Decision 4 records. This
profile exists for this purpose and no other. It is never an agent's root
anchor, never enters the `did:key` identifier, and never widens
`trogon-aauth-verify`'s inbound allowlist: this platform *signs* RS256 here
and continues to refuse to *verify* it on the mesh.

### 3. The binding is a first-class resource with exact-match semantics

The mapping from a platform-attested identity to the external principal it may
assume is modeled as an explicit, enumerable, auditable resource, anchored in
the project hierarchy of
[ADR#0046](./0046-project-anchored-resource-hierarchy.md), with exact-match
semantics on issuer, subject, and audience. Not a pattern, not a policy
expression, not a wildcard.

Pattern matching is rejected for this binding even where an IdP offers it. It
reduces the number of objects to manage and changes the risk model in the same
move: a subject pattern makes the security of the whole federation depend on
how tightly the upstream registration policy constrains what subjects can be
minted, which relocates the trust decision somewhere it is much harder to
audit. Enumerable bindings are the property worth paying object count for.

### 4. One audience per assertion, minted at the moment of exchange

An assertion minted for an external IdP carries exactly one audience, the one
that IdP requires. Tokens minted for any internal purpose are never presented
as external assertions and vice versa: a multi-audience bearer token is
replayable to every recipient it names.

Because such an assertion is a bearer credential for its whole validity
window, it is requested only when an exchange is about to happen, never
cached; never logged, traced, or written to an event; and never placed
anywhere an agent's model context can reach it, which includes prompts, tool
arguments, and tool results. What *is* cached is the resource token the
exchange returns, keyed by its own lifetime, never the assertion that bought
it.

This is a weaker posture than the platform holds for its own ingress, where
[ADR#0051](./0051-fully-bound-request-signing.md) binds a request token to its
target and payload with a nonce, and that asymmetry is inherent: the external
IdP's protocol defines what it accepts, and it accepts a bearer assertion.
The mitigations above are what is available, so they are requirements rather
than hardening.

### 5. Revocation across the boundary is TTL plus unbinding, and it is slower

[ADR#0049](./0049-revocation-latency-target.md)'s five-second target is
in-platform: it measures a revocation event reaching this platform's own
runtime projection. It does not extend across a federation boundary, because
the external IdP does not consult this platform's status list and
[ADR#0037](./0037-agent-identity-governance.md)'s mandatory status check
cannot be pushed onto it.

Standing across the boundary is therefore bounded by two things only: the
assertion TTL, which is kept short and is the primary lever, and deleting the
binding of Decision 3, which propagates asynchronously through the IdP's own
caches and is not immediate. Both are measured and neither is presented as
equivalent to in-platform revocation. Offboarding an agent must revoke on both
planes; revoking only in-platform leaves a window in which an already-issued
external resource token still works.

### 6. Assertion validity is scoped, and delegated authority is not implied

An assertion presented on this surface authenticates a *client*: it says which
platform-attested identity is calling. It never carries delegated user
authority. Where a downstream call acts on a person's behalf, that
authority comes from the person-linked principal
[ADR#0017](./0017-aauth-agent-authentication.md) Decision 5 already makes
authoritative, carried separately. This surface is not a shortcut around
delegated consent, and an integration that treats a successful federation
exchange as sufficient for an on-behalf-of call has widened authority without
anyone deciding to.

### 7. Deferred, and named so it is not assumed

Two things this decision deliberately does not settle:

- **Which principal the downstream provider sees.** Today
  [ADR#0032](./0032-model-route-and-credential-binding.md) Decision 4 brokers
  hosted model access through a session-scoped proxy holding platform
  credentials, so the provider's own audit log names the platform, not the
  agent. Exchanging a per-agent assertion so the provider attributes calls to
  the agent is the strictly better shape and is not built. Until it is, agent
  attribution at a hosted provider is a platform-side record only.
- **Workload attestation.** Proof of possession
  ([ADR#0017](./0017-aauth-agent-authentication.md),
  [ADR#0036](./0036-agent-self-certifying-identity.md)) proves control of a
  key, not that the process holding it is the runtime the platform expects; a
  leaked private key satisfies it. Binding key use to an attested workload is
  the deferred operational-key tier of
  [ADR#0036](./0036-agent-self-certifying-identity.md), and this surface
  inherits that limitation rather than fixing it.

## Consequences

- [ADR#0036](./0036-agent-self-certifying-identity.md)'s deferred global
  resolution layer gains a concrete, unglamorous shape for the external case:
  an OIDC issuer with a JWKS. It does not close the layer, because a DID
  resolution story is still what makes the *identifier* portable; it closes
  the narrower question of how a non-AAuth party verifies us today.
- A second published key set exists, with its own rotation schedule, its own
  overlap window, and its own monitoring, and it must be coordinated with
  SPIRE-style key rotation on the signing side and the IdP's cache on the
  consuming side. Four rotation responsibilities that must not drift apart is
  the standing operational cost of this decision.
- Federation failures need to be distinguishable in telemetry
  ([ADR#0008](./0008-opentelemetry-observability.md)): a missing binding, an
  unreachable or mismatched issuer, and IdP cache lag after a binding change
  have different remediations, and the last class is expected to require
  retry rather than a fix.
- The platform now signs with an algorithm it refuses to verify. That is
  intentional and worth stating plainly, because the asymmetry looks like a
  bug to anyone reading only one side of it.
- Adopting this surface expands what an attacker gains from compromising the
  signing path: an assertion minted here is accepted by a third party under
  the mapping's authority, and revoking it is slower than revoking anything
  in-platform.

## References

- [ADR#0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [ADR#0037: Agent Identity Governance: Decentralized Verification under Governed Authority](./0037-agent-identity-governance.md)
- [ADR#0038: Agent Identity Cryptographic Suite and Crypto-Agility](./0038-agent-identity-crypto-suite.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [RFC 7517: JSON Web Key (JWK)](https://www.rfc-editor.org/rfc/rfc7517)
- [RFC 8414: OAuth 2.0 Authorization Server Metadata](https://www.rfc-editor.org/rfc/rfc8414)
- [RFC 8615: Well-Known Uniform Resource Identifiers](https://www.rfc-editor.org/rfc/rfc8615)
- [OpenID Connect Discovery 1.0](https://openid.net/specs/openid-connect-discovery-1_0.html)
- [Microsoft Entra: workload identity federation considerations](https://github.com/MicrosoftDocs/entra-docs/blob/main/docs/workload-id/workload-identity-federation-considerations.md)
- [SPIFFE Workload Identity and Entra Agent ID: the trust gap](https://dev.to/astaykov/your-spiffe-workload-can-authenticate-as-an-entra-agent-id-but-mind-the-trust-gap-3969)
- [From JWT-SVID to Entra Agent ID: a working SPIFFE PoC](https://dev.to/astaykov/from-jwt-svid-to-entra-agent-id-a-working-spiffe-poc-ic4)
- [ADR index](./index.md)

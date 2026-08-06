---
number: "0050"
slug: signed-first-caller-authentication
status: accepted
date: 2026-08-05
---

# ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication

## Context

The API key platform offers two caller authentication modes: verifier-only
bearer keys, where the caller presents a shared secret on every request, and
Coinbase-style signed keys, where the caller holds a private key and signs a
short-lived request token. The initial framing treated bearer as the normal
tier and signed as the high-authority exception, and left the first signed-key
algorithm undecided.

The two modes are not peers on security properties. With a bearer key, the
raw secret exists in three places: at rest on the caller's side, on the wire
in every request, and once, at issuance, in a response body
([ADR#0048](./0048-one-time-plaintext-exposure.md) bounds that last moment).
A breach of platform storage would still yield nothing (verifier digests
only), but every proxy, log line, and TLS-terminating hop on the request
path sees a replayable credential. With a signed key, the platform stores a
public key, which is not a secret; the wire carries a token bound to one
method, host, and path that expires in minutes and cannot be replayed past
its nonce; rotation is a public-key swap; and every request carries a
signature attributing it to a specific key, an audit property bearer keys
structurally cannot have.

The industry has already moved this direction for exactly these reasons:
request signing in AWS SigV4, signed service-account JWTs in GCP, DPoP
sender-constrained tokens in OAuth (RFC 9449), and WebAuthn replacing
passwords wholesale. Coinbase's protocol is the closest reference for the
token shape, with one caveat worth designing out: their portal generates the
key pair server-side and hands the private key to the caller as a one-time
download, which reintroduces the plaintext-issuance moment the protocol
exists to eliminate.

What signed keys cannot do is remove plaintext from flows the platform does
not define. Provider-side material (HMAC webhook signing secrets, bot
tokens, OAuth refresh tokens) is raw material the platform must hold because
the counterparty's protocol demands the actual value. The decision below
therefore draws the deprecation boundary explicitly rather than implying
plaintext can vanish everywhere.

## Decision

### 1. Signed is the strongly recommended default

Wherever the platform offers callers an authentication choice, signed
proof-of-possession is the strongly recommended mode and the default posture
of documentation, UI flows, and SDK examples. Bearer keys are the explicitly
labeled compatibility tier for tooling that cannot sign requests, not a peer
option.

### 2. Client-generated key pairs only

The platform never generates, transmits, stores, or displays a private key.
Signed-key registration accepts a public key and nothing else. There is no
server-side generation convenience and no one-time private-key download;
the [ADR#0048](./0048-one-time-plaintext-exposure.md) one-time-display
machinery does not apply to this tier because no secret ever exists on the
platform side.

### 3. Algorithms: Ed25519 default, ES256 accepted

Ed25519 (JWS `EdDSA`) is the default and recommended algorithm: deterministic
signatures, no ECDSA nonce-reuse failure mode, small keys, wide library
support. ES256 is also accepted for ecosystem and Coinbase-shaped
compatibility. This closes the open "first signed-key algorithm" decision.

### 4. Request token contract

The signed request token is a short-lived JWT binding the request target,
issued-at, expiry, and a nonce. The full binding set (transport-mapped
target, payload digest, validity bounds, replay store, server-nonce
escalation) is specified in
[ADR#0051](./0051-fully-bound-request-signing.md), which amends this
section.

### 5. Bearer remains, policy-bounded

Bearer keys keep verifier-only storage and one-time display per
[ADR#0048](./0048-one-time-plaintext-exposure.md). Keyspace policy can
disallow bearer issuance entirely, and root and management keyspaces are
signed-only from the start. Deprecating bearer where it is no longer needed
is a per-keyspace policy action, not a platform migration.

### 6. The plaintext deprecation boundary

Plaintext-out (platform-issued secrets) is deprecable: it shrinks as
keyspaces move to signed mode and can reach zero for a given keyspace.
Plaintext-in (provider-defined secrets held for HMAC verification and
provider API calls) is not the platform's to deprecate; it persists as raw
material in OpenBao for as long as providers define shared-secret protocols.
Where a provider offers asymmetric verification (for example Ed25519-signed
webhooks), the platform prefers it per source and stores only the public
verification material.

## Consequences

- Signed mode is built as the primary path rather than the advanced option;
  bearer verification remains constant-time and verifier-only but is
  documented as the compatibility tier.
- The platform takes on a nonce replay cache and clock-skew tolerance;
  callers on the recommended tier take on private-key custody.
- Audit and `ApiPrincipal` gain per-request key attribution from signatures.
- An ES256-first implementation default is superseded by Ed25519 default with
  ES256 compatibility, closing the signed-key algorithm question.

## References

- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- RFC 9449 (OAuth DPoP); RFC 8032 (Ed25519)

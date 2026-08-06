---
number: "0051"
slug: fully-bound-request-signing
status: accepted
date: 2026-08-05
---

# ADR#0051: Fully Bound Per-Request Signing Contract

## Context

[ADR#0050](./0050-signed-first-caller-authentication.md) made signed
proof-of-possession the strongly recommended caller authentication and
sketched the request token as a short-lived JWT binding method, host, path,
expiry, and a nonce. Studying the strongest deployed variants of the pattern
(Coinbase's Wallet token, DPoP with server-issued nonces per RFC 9449, and
WIMSE workload proof tokens) showed the sketch leaves real gaps:

- Without payload binding, a token authorizes any body sent to that endpoint
  within its validity window. The nonce stops a second use, but whoever
  holds an unspent token can attach it to a different payload.
- Method, host, and path are HTTP grammar. Trogonai's internals ride NATS
  (jsonrpc-nats, mcp-nats, the A2A stack), where the addressable target is a
  subject, and a request may transit brokers and JetStream persistence,
  which means more parties see a request in flight than on a single TLS
  hop. Binding must be defined per transport, and payload binding matters
  more on NATS, not less.
- A purely client-chosen nonce lets the platform detect reuse but never
  demand freshness; DPoP's server-issued nonce closes that for surfaces
  that warrant it.

With full binding, the security statement becomes concrete: a captured
request cannot be altered or reused (wrong body, spent nonce, expired
within a minute or two); the one residual power of an intercepted unspent
token is to deliver the caller's exact request once, racing the legitimate
sender for its single admission. A breach of the signed-key tier yields
public keys only, while provider-held plaintext in OpenBao remains, bounded
separately by [ADR#0050](./0050-signed-first-caller-authentication.md)
section 6. Within the signed tier, the sole remaining secret is the
private key on the client machine.

## Decision

### 1. Single-use, fully bound tokens

Every signed request carries a fresh signature; a token authorizes exactly
one request. There is no reduced-binding or multi-use mode.

### 2. Required binding claims

- Request target, transport-mapped: for HTTP, method, host, and canonical
  path; for NATS, the subject and operation. One canonical serialization is
  defined in the API contracts and shared by both; that definition fixes
  case, default-port elision, and percent-encoding for HTTP targets, ships
  with canonicalization test vectors for both transports, and lands before
  `api_key.verify_signed_request` does.
- Payload digest: SHA-256 over the exact request body, with a defined
  constant digest for bodyless requests. Always required.
- Time: issued-at and expiry. The validity window is at most 2 minutes,
  1 minute by default; verifiers tolerate at most 30 seconds of clock skew.
- Uniqueness: a client-generated `jti`, checked and recorded against a
  replay store scoped by key id.

### 3. Server-nonce escalation

The protocol supports DPoP-style server-issued nonces from the first
version: a verifier may reject with a fresh nonce challenge that the client
must bind into its retry. Whether a surface demands it is keyspace policy;
root and management keyspaces demand it by default.

### 4. Replay store

Replay records are keyed by key id and `jti` and live for the validity
ceiling plus skew, in NATS KV per
[ADR#0047](./0047-event-sourced-credential-metadata.md). Check-and-record
is a single atomic conditional create of the `(key id, jti)` record, never
a read followed by a write; a key-already-exists result is the replay
rejection. Signed-request verification fails closed when the replay store
cannot be consulted.

### 5. Inherited posture

Algorithms and key custody follow [ADR#0050](./0050-signed-first-caller-authentication.md):
Ed25519 default, ES256 accepted, client-generated key pairs only. This
contract applies to end-caller authorization at the gateway and riding over
NATS internally; NATS connection identity remains the native NKeys/JWT
machinery, and event provenance remains
[ADR#0039](./0039-self-authenticating-event-provenance.md).

### 6. NATS transport and admission-time verification

Over NATS, the token rides in message headers, the bound target is the
concrete subject the message was published on plus the operation in the
payload envelope, and the payload digest covers the raw payload bytes (a
NATS payload is a single byte slice, so no canonicalization rules are
needed). Subject mapping that rewrites subjects breaks signatures the same
way rewriting proxies do on HTTP, and is unsupported in front of signed
subjects.

The token is verified once, at admission, by the first service that accepts
the request while the token is fresh. From that point authority and origin
travel as provenance on the resulting events per
[ADR#0039](./0039-self-authenticating-event-provenance.md). Downstream and
JetStream consumers do not re-verify caller tokens as a matter of course;
an expired token inside a persisted message is the expected state of an
already-admitted request, not an error. Guidance rather than a hard rule:
per-request signing is worth carrying over NATS wherever a receiver acts on
end-caller authority that connection identity alone cannot establish (agent
tool invocations, management commands, anything done on behalf of a
customer key); pure infrastructure traffic whose authority is fully decided
by NKeys/JWT connection identity and subject permissions does not need it.

## Consequences

- `api_key.verify_signed_request` implements the full binding set from its
  first version; there is no partially bound rollout stage to migrate away
  from later.
- The platform operates a replay store whose size is bounded by request
  rate times the record lifetime, the validity ceiling plus clock skew
  (150 seconds as specified), roughly two and a half minutes of traffic.
- Clients must know the complete body before signing; streaming uploads
  would need a digest-first design, which is acceptable for a management
  API surface.
- Intermediaries that rewrite paths or bodies break signatures by design;
  the canonicalization rules in the API contracts are the compatibility
  surface, and rewriting proxies are unsupported in front of signed routes.
- Amends the token-contract sketch in
  [ADR#0050](./0050-signed-first-caller-authentication.md) section 4; that
  section now defers to this contract.

## References

- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0047: Event Stream as the Credential Metadata Source of Truth](./0047-event-sourced-credential-metadata.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- RFC 9449 (OAuth DPoP, server nonce); WIMSE workload proof token draft;
  AWS SigV4 signed payload hash; Coinbase Wallet request token

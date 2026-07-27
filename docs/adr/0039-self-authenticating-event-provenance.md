---
number: "0039"
slug: self-authenticating-event-provenance
status: draft
date: 2026-07-27
---

# ADR#0039: Self-Authenticating Event Provenance

## Context

Authenticity is checked once, at the edge. AAuth proof-of-possession
([ADR#0017](./0017-aauth-agent-authentication.md)) authenticates the calling
agent, and the authorizer hook hands `decide` a typed `CommandPrincipal`
before it runs ([ADR#0026](./0026-command-authorization-principal.md)). Once
`decide` returns, the resulting events are appended with no actor signature of
their own. Event headers are freeform envelope metadata, and the runtime
deliberately does not derive them: an application that wants a fixed header
builds it itself before execution
([ADR#0026](./0026-command-authorization-principal.md), citing
[Event Metadata](../architecture/event-metadata.md)).

The consequence is that an event copied out of the store carries no proof of
who authored it. Its trustworthiness rests entirely on the append path: a
reader trusts an event because they trust the service that wrote it, not
because the event itself proves anything. Proof does not travel with the
data. Move the event to cold storage, replicate it into another organization,
or hand it to an auditor with no access to the original store, and the
authorship claim goes with it.

The property this platform wants is the one
[ADR#0036](./0036-agent-self-certifying-identity.md) and
[ADR#0037](./0037-agent-identity-governance.md) already establish for
identity: the Nostr-shaped reference model, where every artifact is
self-authenticating and can be verified by anyone, offline, without a
central authority. Applied to events, the goal is that every event is
self-authenticating and can move between stores, streams, or organizations
without losing proof of authorship.

The [command](../glossary/command)/[event](../glossary/event) split of CQRS
changes where the signature has to attach. In Nostr the actor signs the
event directly: there is exactly one artifact, and the signature covers it.
Here an agent authors a command, an expression of intent, and a
deterministic [decider](../glossary/decider) derives the events, the
outcome, from that command and the prior stream. There is no single artifact
for the actor to sign after the fact, because the actor never produces the
event. Authorship proof therefore has to attach to the command, and the
events that command produces have to carry that proof forward.

## Decision

### 1. The agent signs the exact command bytes

The agent submitting a command signs the exact serialized command bytes with
its identity key ([ADR#0036](./0036-agent-self-certifying-identity.md)),
under the suite [ADR#0038](./0038-agent-identity-crypto-suite.md) fixes: an
Ed25519 default, with conditional P-256 and
[BIP-340](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
Schnorr profiles and a future hybrid
[FIPS 204](https://csrc.nist.gov/pubs/fips/204/final) ML-DSA path recorded
alongside it. This ADR takes no position on which algorithm is in force at
a given time; it only fixes that the command bytes are what gets signed. The
signature is a detached JWS ([RFC 7515](https://www.rfc-editor.org/rfc/rfc7515)):
the protected header carries `alg` and a `kid` that references the agent's
[did:key](https://w3c-ccg.github.io/did-method-key/) identifier, and the
payload is detached because the signed material is the command bytes already
on the wire, not a copy re-embedded in the signature envelope.

### 2. The runtime persists provenance as envelope headers, never payload fields

The runtime persists provenance on every event emitted by that command's
execution, and it does so as envelope headers, never as payload fields: the
detached JWS, the signer's DID/`kid`, and a digest of the signed command
bytes. The exact command bytes are preserved, either inline in the stream or
content-addressed alongside it, so any party holding an event can retrieve
the bytes the signature covers and re-verify later, without depending on the
store that first appended it.

### 3. No re-serialization: sign and store the same bytes

Signing the exact bytes and storing those same bytes deliberately sidesteps
[protobuf](../glossary/protocol-buffers)'s non-canonical encoding. Nothing is
ever re-serialized in order to verify a signature: the verifier checks the
signature against the identical byte string the signer produced and the
runtime stored. Canonicalization would only be needed by a consumer that
decodes the command into a typed message and then re-serializes it to
reconstruct the signed bytes; this design never asks a consumer to do that,
so the canonicalization problem does not arise.

### 4. Signing stays at key-holding boundaries; the decider stays keyless

Signing happens only where a private key can legitimately live: the agent or
client itself, or a custody-plane signer
([ADR#0023](./0023-secret-management-and-key-custody-direction.md),
[ADR#0033](./0033-two-tier-key-custody-product-model.md)). The
[decider](../glossary/decider), native and [WASM](../glossary/wasm) alike,
remains keyless and signature-unaware: it never sees a private key, never
verifies a signature, and never branches on one. This is the same boundary
[ADR#0026](./0026-command-authorization-principal.md) already draws for
identity generally, keeping it out of `decide`. No signature logic enters
domain payloads or `AgentConfiguration`
([ADR#0025](./0025-agent-definition-data-ownership.md)).

### 5. Scope: author-attributable events only

Provenance applies to author-attributable events, the ones produced by a
command a specific agent submitted. System-derived events, timers,
projections, and reconciliation, and any other event the runtime produces
without an authoring command, carry no provenance header. This keeps hot
internal paths cheap: nothing forces a signature-verification cost onto a
path that never had an authoring principal to prove.

### 6. Outcome verification, beyond authorship

Authorship is not the only thing a signed command buys. Because the decider
is deterministic, a third party holding the signed command, the prior events
in the stream, and the exact decider build can re-execute `decide` and
confirm that the recorded events follow from that command under the rules
the decider encodes. This is strictly stronger than the reference model:
Nostr's signature proves only who authored a blob, never that the blob is
the correct consequence of anything. Here the same signed artifact anchors
both checks, who submitted it, and whether what followed from it is
correct.

### 7. Status currency composes with, but does not replace, provenance

A provenance signature proves authorship at write time: this agent, holding
this key, produced this command, at this moment. It says nothing about
whether that key is still trusted at read time. Whether the signing key has
since been revoked is answered by the status mechanisms of
[ADR#0037](./0037-agent-identity-governance.md). The two checks compose and
neither substitutes for the other: a valid signature over a revoked key
still proves authorship at the time of signing, and a status check without a
signature proves nothing about who acted.

## Consequences

- The audit trail becomes portable and trustless: events move across
  streams, cold storage, or organizations with their proof of authorship
  intact, instead of that trust resting on the append path that first wrote
  them.
- The cost is real: one signature per command, the command bytes themselves
  now need to be stored somewhere reachable, and a provenance header schema
  still needs to be defined. That schema lands alongside the deferred
  command layer this ADR does not build; this ADR records the direction,
  not the wire shape.
- Verification reuses the single AAuth/JOSE path
  [ADR#0038](./0038-agent-identity-crypto-suite.md) fixes; this ADR adds no
  parallel signature stack to build, operate, or key separately.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0033: Two-Tier Key Custody Product Model](./0033-two-tier-key-custody-product-model.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [ADR#0037: Agent Identity Governance: Decentralized Verification under Governed Authority](./0037-agent-identity-governance.md)
- [ADR#0038: Agent Identity Cryptographic Suite and Crypto-Agility](./0038-agent-identity-crypto-suite.md)
- [Event Metadata](../architecture/event-metadata.md)
- [RFC 7515: JSON Web Signature (JWS)](https://www.rfc-editor.org/rfc/rfc7515)
- [NIST FIPS 204: Module-Lattice-Based Digital Signature Standard (ML-DSA)](https://csrc.nist.gov/pubs/fips/204/final)
- [BIP-340: Schnorr Signatures for secp256k1](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
- [did:key Method Specification (W3C Credentials Community Group draft)](https://w3c-ccg.github.io/did-method-key/)
- [Buzz: Nostr-based agent identity](https://github.com/block/buzz)
- [ADR index](./index.md)

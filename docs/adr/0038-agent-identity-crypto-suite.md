---
number: "0038"
slug: agent-identity-crypto-suite
status: draft
date: 2026-07-27
---

# ADR#0038: Agent Identity Cryptographic Suite and Crypto-Agility

## Context

[ADR#0036](./0036-agent-self-certifying-identity.md) anchors an agent's
identity to its root public key, in a DID or JWK-thumbprint form, and leaves
the algorithm, the identifier method, and the signature format unfixed.
[ADR#0037](./0037-agent-identity-governance.md) requires decentralized
verification under governed authority: any party must be able to check a
signature against the anchored key without asking a central issuer, while
governance decides which keys a verifier trusts. This ADR extends that
governed stance from keys to algorithms, through the verifier allowlist of
Decision 3. Neither prior decision names a curve, a key encoding, or a
signature envelope, and an agent identity anchor cannot be implemented
without picking all three.

The platform already has running parts of an answer.
[ADR#0017](./0017-aauth-agent-authentication.md) binds an agent to a
`cnf.jwk` public key and verifies proof of possession in
`trogon-aauth-verify`; the claim set is JOSE/JWK throughout. NATS NKeys, the
identity primitive the mesh's transport layer already uses, are Ed25519
keys. Picking a suite that ignores this stack would mean a second
verification path alongside the one AAuth already runs.

The risk this ADR manages is narrower than "which curve is best": it is
hardcoding one algorithm with no migration story. Every asymmetric signature
scheme has a shelf life, whether from a classical cryptanalytic break or from
a cryptographically relevant quantum computer running Shor's algorithm. An
identity format that bakes in one algorithm with no path to add or replace it
turns that eventual break into an identity redesign, forcing every agent to
be re-anchored under a new scheme with no continuity from the old one. This
ADR fixes the default suite for agent identity keys and signatures, and fixes
crypto-agility as a requirement of the format itself, so that a break is
survived by rotation rather than by another architecture decision.

## Decision

### 1. Default suite: Ed25519, JWK, did:key, JWS

The root identity key defined in [ADR#0036](./0036-agent-self-certifying-identity.md)
is an **Ed25519** key (EdDSA, [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032)).
It is carried as a **JWK** with `kty: OKP`, `crv: Ed25519`, `alg: EdDSA`, per
[RFC 8037](https://www.rfc-editor.org/rfc/rfc8037), and verified on the
existing AAuth path ([ADR#0017](./0017-aauth-agent-authentication.md)):
`agent_jkt` is the [RFC 7638](https://www.rfc-editor.org/rfc/rfc7638) JWK
thumbprint of that same key, so identity and AAuth's proof-of-possession
binding derive from one key encoding, not two.

The identifier form is `did:key`. Its multicodec prefix self-describes the
curve, so an Ed25519 key always yields an identifier beginning `did:key:z6Mk`;
the identifier is never ambiguous about which algorithm produced it, and
resolving it never requires a lookup, only decoding the prefix.

Signatures are carried as [JWS](https://www.rfc-editor.org/rfc/rfc7515)
(RFC 7515). Where the signed payload already lives elsewhere, such as an
event body recorded separately from its provenance envelope
([ADR#0039](./0039-self-authenticating-event-provenance.md)), the signature
is a detached JWS: the protected header and signature travel without a copy
of the payload the header commits to.

### 2. Why Ed25519 over the alternatives

- **Misuse resistance.** EdDSA derives its per-signature nonce
  deterministically inside the scheme itself, so no caller can supply a weak
  or repeated nonce. This removes the failure class that has produced real
  key-recovery incidents against randomized ECDSA: the 2010 Sony PlayStation
  3 firmware signing key, recovered because the signer reused a constant
  nonce across signatures, and the 2013 Android `SecureRandom` weakness that
  exposed Bitcoin wallet private keys derived from ECDSA signatures with
  correlated nonces. [RFC 6979](https://www.rfc-editor.org/rfc/rfc6979)
  retrofits deterministic nonce derivation onto ECDSA, and
  [BIP-340](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
  Schnorr specifies a default nonce derivation, but no scheme's verifier can
  see how a signer derived a nonce; the structural difference is the signing
  interface. EdDSA's signing path takes no caller-supplied nonce and no
  per-signature randomness, so there is nothing for an implementation to get
  wrong, whereas deployed randomized ECDSA demands fresh randomness on every
  signature and fails catastrophically when it is weak. The critique above
  therefore targets classic randomized ECDSA as deployed, not the secp256k1
  curve itself.
- **Parameter provenance.** Curve25519's parameters derive from a published,
  rigid generation procedure with every constant explained. NIST P-256 carries
  an unexplained-seed provenance question that Curve25519 does not, because
  the seed used to generate its curve constants has never been shown to
  derive from a documented, reproducible process.
- **Deployment maturity.** Ed25519 is a default or supported signing scheme
  in OpenSSH and TLS 1.3, and Signal signs with the closely related XEdDSA
  construction over the same curve, so the platform is adopting an
  already-hardened, widely reviewed implementation surface rather than a
  novel one.
- **Stack fit.** NATS NKeys are Ed25519 and AAuth's claim sets are JOSE/JWK,
  so this default keeps one verification path end to end instead of adding a
  second key type and a second verifier for identity alone.

### 3. Crypto-agility is a first-class requirement

Crypto-agility is not deferred as future work; it is a property the format
enforces from the start. Every persisted key and signature is self-describing:
the JWK `kty`/`crv`/`alg` fields, the JWS protected `alg` header, and the
`did:key` multicodec prefix all name their own algorithm inline. Verifiers
enforce an explicit algorithm allowlist rather than accepting whatever `alg`
a message claims.

An agent's identity may bind more than one key, and the binding mechanism
must be named, because plain `did:key` cannot express it: a `did:key`
document is derived entirely from the single key encoded in the identifier.
An additional key is bound by a signed event on the agent's own stream,
authorized by the root key, which any holder of the stream can verify
without an external registry; the stream is the authoritative key record, as
[ADR#0036](./0036-agent-self-certifying-identity.md) already establishes.
The `did:key` identifier therefore names the root anchor only, and adding a
second algorithm is additive, never a replacement of the identifier.
Publishing that key record beyond the stream remains the deferred global
resolution layer of [ADR#0036](./0036-agent-self-certifying-identity.md),
and rotation of the root key itself, which needs that resolution layer to
hold the identifier stable, remains deferred with it. The rotation this ADR
speaks of is algorithm rotation under an unchanged root: rebinding keys and
retiring algorithms from the verifier allowlist, not replacing the root
identity key.

The format itself never constrains which algorithms may exist; it only makes
every key and signature name its own algorithm so a verifier's policy, not
the wire format, decides which algorithms are accepted at a given time. That
verifier-side allowlist, not any single chosen curve, is the durable security
decision this ADR makes: it is what lets a broken algorithm be survived by
rebinding a new key and rotating verifier policy rather than by redesigning
the identity format itself.

### 4. Conditional profiles, added as bound keys, never as root replacements

Two further curves are recorded as conditional profiles, each with the
trigger that would justify adopting it. Either arrives as an additional key
bound to the agent's identity through the stream-attested binding of
Decision 3, never as a replacement for the Ed25519 root:

- **P-256 (ES256)**, carried in [COSE](https://www.rfc-editor.org/rfc/rfc9052)
  ([RFC 9052](https://www.rfc-editor.org/rfc/rfc9052) and
  [RFC 9053](https://www.rfc-editor.org/rfc/rfc9053)) where a compact binary
  envelope fits better than JOSE's JSON encoding, if hardware-backed custody
  or WebAuthn-class attestation drives the requirement. FIPS 186-5 approves
  EdDSA at the algorithm level, so the differentiator is not FIPS approval in
  the abstract but the FIPS 140-validated module and hardware fleet: HSMs,
  TPMs, secure enclaves, and FIDO2 authenticators overwhelmingly speak P-256
  today. A custody requirement that lands on one of those roots is the
  trigger for this profile, not a general preference for it.
- **secp256k1 with BIP-340 Schnorr**, only if Nostr-native or blockchain-native
  interop becomes a requirement. Whether that requirement should be taken on
  is a governance question, recorded as a stance in
  [ADR#0037](./0037-agent-identity-governance.md), not a decision this ADR
  makes on its own.

### 5. Post-quantum path

Every elliptic-curve scheme named above, Ed25519, P-256, and secp256k1 alike,
falls to Shor's algorithm on a cryptographically relevant quantum computer;
none of them is quantum-resistant by construction. The recorded upgrade path
is hybrid signing: Ed25519 plus [ML-DSA](https://csrc.nist.gov/pubs/fips/204/final)
(FIPS 204), added as an additional tagged key under Decision 3 once JOSE,
COSE, and DID conventions for representing ML-DSA keys and signatures mature.
ML-DSA keys and signatures are kilobyte-scale, roughly forty to eighty times
larger than Ed25519's 32-byte keys and 64-byte signatures, which makes
adopting it before those conventions stabilize and before a concrete threat
justifies the cost premature. Migration to the hybrid suite is
therefore planned work, tracked against maturing standards and an assessed
threat timeline, not an emergency response to an already-broken algorithm.

## Consequences

- There is one verification path today: EdDSA signatures over Ed25519 keys,
  verified on the existing AAuth plane ([ADR#0017](./0017-aauth-agent-authentication.md)).
  Adding a curve later, whether P-256, secp256k1, or a post-quantum scheme, is
  a verifier policy change plus a key binding on an existing agent identity,
  not a redesign of the identity format or a re-anchoring of existing agents.
- Curve-fixed identifier forms, such as a Nostr `npub`-style encoding that
  presumes one fixed curve in the identifier itself, are rejected. Identifiers
  and keys in this platform are always algorithm-tagged, so an identifier
  never has to be reinterpreted if the algorithm it names is later retired.
- Private key custody is unaffected by this ADR and remains on the security
  plane defined by [ADR#0023](./0023-secret-management-and-key-custody-direction.md)
  and [ADR#0033](./0033-two-tier-key-custody-product-model.md). This ADR
  governs public-key algorithms, encodings, and signature formats only; it
  says nothing about where or how the corresponding private keys are stored
  or protected.

## References

- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0033: Two-Tier Key Custody Product Model](./0033-two-tier-key-custody-product-model.md)
- [ADR#0036: Agent Self-Certifying Cryptographic Identity](./0036-agent-self-certifying-identity.md)
- [ADR#0037: Agent Identity Governance](./0037-agent-identity-governance.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- [RFC 6979: Deterministic Usage of DSA and ECDSA](https://www.rfc-editor.org/rfc/rfc6979)
- [RFC 7515: JSON Web Signature (JWS)](https://www.rfc-editor.org/rfc/rfc7515)
- [RFC 7638: JSON Web Key (JWK) Thumbprint](https://www.rfc-editor.org/rfc/rfc7638)
- [RFC 8032: Edwards-Curve Digital Signature Algorithm (EdDSA)](https://www.rfc-editor.org/rfc/rfc8032)
- [RFC 8037: CFRG Elliptic Curve Diffie-Hellman and Signatures in JOSE](https://www.rfc-editor.org/rfc/rfc8037)
- [RFC 9052: CBOR Object Signing and Encryption (COSE): Structures and Process](https://www.rfc-editor.org/rfc/rfc9052)
- [RFC 9053: CBOR Object Signing and Encryption (COSE): Initial Algorithms](https://www.rfc-editor.org/rfc/rfc9053)
- [NIST FIPS 204: Module-Lattice-Based Digital Signature Standard (ML-DSA)](https://csrc.nist.gov/pubs/fips/204/final)
- [BIP-340: Schnorr Signatures for secp256k1](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
- [did:key Method Specification (W3C Credentials Community Group draft)](https://w3c-ccg.github.io/did-method-key/)
- [Buzz: Nostr-based agent identity](https://github.com/block/buzz)
- [ADR index](./index.md)

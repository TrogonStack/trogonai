---
number: "0058"
slug: api-key-format-and-verifier-construction
status: accepted
date: 2026-08-15
---

# ADR#0058: API Key Format and Bearer Verifier Construction

## Context

Three of the API key platform's open questions gate any key issuance at
all: the exact key format, the verifier digest construction, and where the
verifier pepper lives. The first also gates the signed tier, because the
`kid` a signed request carries is the same identifier a bearer key exposes
for lookup, and deciding two different identifier schemes for the two tiers
would double the lookup path for no benefit.

[ADR#0050](./0050-signed-first-caller-authentication.md) demotes bearer to
the policy-bounded compatibility tier, which changes how prominent the
construction is but not whether it has to be right: a compatibility tier
still authenticates real callers, and a weak verifier there is a weak
verifier on the platform.

One fact decides the verifier construction on its own. The bearer secret is
CSPRNG output at full width, not a user-chosen password. Password hashing
exists to stretch a low-entropy input so that guessing it costs real work;
against 256 bits of uniform randomness the guessing attack already costs
more than the universe affords, so the stretching buys nothing. Its cost,
meanwhile, is memory-hard work on the platform's hottest path, reachable by
an unauthenticated caller sending garbage, which turns a defensive control
into a denial-of-service amplifier.

The format has a second audience beyond the platform. A platform-issued
credential pasted into a public repository should be findable and
revocable by the secret-scanning ecosystem, and that ecosystem matches
regular expressions and, where a checksum exists, validates offline before
reporting. A format chosen without that in mind is a format that gets
changed later, after keys are in circulation.

## Decision

### 1. Key format

```text
tg_<env>_<key_id>_<secret><checksum>
```

- `tg` is the fixed product prefix.
- `<env>` is `live` or `test`, rendered from the issuing keyspace's
  environment attribute. Environments are attributes rather than hierarchy
  levels per [ADR#0046](./0046-project-anchored-resource-hierarchy.md)
  section 4, so this marker is a rendering of the keyspace attribute and
  not a second source of truth. It stays consistent because keyspace
  environment is immutable after creation
  ([ADR#0059](./0059-api-keyspaces-and-root-key-bootstrap.md)).
- `<key_id>` is the `ApiKeyId` itself: ULID-shaped, Crockford base32,
  26 characters, lowercased. Lookup is therefore a primary-key read rather
  than a scan on a secondary token. `public_prefix` in the API_KEY.md data
  model stops being a separately minted value; it is the derived display
  form `tg_<env>_<key_id>`.
- `<secret>` is 32 bytes from a CSPRNG, base62-encoded (43 characters).
  Base62 avoids `-`, `_`, and `=`, so the value survives shell and URL
  contexts and selects as one word on a double click.
- `<checksum>` is CRC32C over everything preceding it, base62-encoded to a
  fixed 6 characters. It lets SDKs and scanners reject typos and
  truncations offline, with no lookup and no verification attempt.

The scanner-facing pattern is fixed by the above:

```text
tg_(live|test)_[0-9a-hjkmnp-tv-z]{26}_[0-9A-Za-z]{49}
```

Distinctiveness comes from the whole pattern, not from the two-character
prefix, which is why `tg` is short enough to stay pleasant in a shell
without costing scanner precision.

The prefix carries no authority. Authorization comes from the key record
after verification, exactly as API_KEY.md already states.

### 2. Bearer verifier: HMAC-SHA-256 under a pepper

```text
verifier_digest = HMAC-SHA-256(
  pepper[verifier_key_version],
  "tgapikey/v1" || 0x00 || key_id || 0x00 || secret
)
```

The domain-separation label and the bound `key_id` mean a digest cannot be
transplanted onto a different key record. Comparison is constant time, the
same discipline the internal admin API's token check already uses.

Argon2id, scrypt, and bcrypt are rejected for the reason in the context: no
stretching benefit against a full-width random secret, and memory-hard work
on every authenticated request, reachable pre-authentication. Unkeyed
SHA-256 is rejected because it gives a stolen database an offline
verification oracle that needs no second secret.

Each key record stores the `verifier_key_version` its digest was computed
under. Verification uses the version on the record. Pepper rotation is
therefore a dual-version window (new keys under `v(n+1)`, existing keys
still verifying under `v(n)` until they are rerolled or expire) rather than
a fleet-wide forced reroll, which would otherwise be the only way to
rotate a pepper the platform cannot re-derive digests without.

### 3. Pepper custody: OpenBao KV, resolved at boot, never on the request path

The pepper is stored in OpenBao KV and resolved once at process start by
the verifying service, through the secrets service as an opaque ref per
[ADR#0057](./0057-credential-platform-extraction-boundary.md), not by the
verifier holding an OpenBao client of its own. It is held in memory for the
process lifetime.

- OpenBao Transit is rejected: it would put a round trip on every
  authenticated request and make caller authentication unavailable exactly
  when OpenBao is. Credential resolution can afford to wait on OpenBao;
  authentication cannot.
- A deployment environment variable is rejected: rotation would require a
  redeploy, and the value appears in process environment listings and crash
  dumps.
- A cloud KMS call per verification is rejected for Transit's latency
  reason, and additionally because it would put a cloud dependency in front
  of self-hosted authentication.

The pepper is never in the API-key table, and never in any event, state
snapshot, projection, idempotency record, log, metric, or trace. It is
wrapped in `SecretString` like every other material on the platform, so
redaction is structural rather than a formatting rule, matching the
existing no-plaintext-in-logs and no-plaintext-in-traces guarantees.

Failure to resolve the pepper at boot is fatal. The verifying service
refuses to start rather than starting with bearer verification silently
disabled or failing open.

### 4. Signed keys share the identifier, not the format

A signed key has no wire-format secret, so it has no `<secret>` or
`<checksum>` component. The `kid` in the request token header is the same
`ApiKeyId`, which keeps key lookup one code path across both tiers.
`public_key_fingerprint` is SHA-256 over the SPKI DER encoding, base64url,
displayed with a `sha256:` prefix. It is the value the bootstrap and
rotation flows in
[ADR#0059](./0059-api-keyspaces-and-root-key-bootstrap.md) match on.

## Consequences

- Bearer verification is one HMAC and one constant-time compare: no network
  call, no tunable work factor, and no pre-authentication cost an attacker
  can amplify.
- Pepper rotation becomes a dual-version window rather than a fleet-wide
  reroll, at the cost of one small integer on every key record.
- `public_prefix` stops being independently minted state and becomes a
  derived display value, removing a field that could disagree with the id
  it was standing in for.
- The scanner-facing pattern is fixed before any key is issued, so
  registering it with secret-scanning partners is follow-up work rather
  than a format migration.
- The checksum gives SDKs an offline rejection for typos and truncations,
  which keeps that class of failure out of the verification failure metrics
  the alerting work depends on.
- The verifying service gains a hard boot dependency on the secrets
  service, which is the intended trade: unavailable is safer than
  unauthenticated.

## References

- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0057: Credential Platform Extraction Boundary](./0057-credential-platform-extraction-boundary.md)
- [ADR#0059: API Keyspaces and Root Key Bootstrap](./0059-api-keyspaces-and-root-key-bootstrap.md)
- RFC 2104 (HMAC-SHA-256); RFC 9106 (Argon2, the password-hashing family
  this rejects for full-width random secrets)

---
number: "0063"
slug: api-keyspaces-and-root-key-bootstrap
status: accepted
date: 2026-08-15
---

# ADR#0063: API Keyspaces and Root Key Bootstrap

## Context

Two accepted ADRs already depend on keyspaces without defining them.
[ADR#0050](./0050-signed-first-caller-authentication.md) section 5 says
keyspace policy can disallow bearer issuance entirely and that root and
management keyspaces are signed-only.
[ADR#0051](./0051-fully-bound-request-signing.md) section 3 says whether a
surface demands server nonces is keyspace policy. Nothing says what a
keyspace is, where it sits in the resource hierarchy, what its policy
fields are, or which ones exist when a project is created. API_KEY.md
leaves "what are the first keyspaces" open, and the two ADRs above have
been writing checks against that answer.

The harder half is bootstrap. To create a key you must authenticate with a
key, and API_KEY.md leaves the self-hosted root key bootstrap flow open.
The obvious answer, printing an initial root token at first boot the way
`bao operator init` does, fails on this platform for two specific reasons:
it mints a bearer key into a keyspace ADR#0050 declares signed-only, and it
reintroduces the plaintext-issuance moment signed mode exists to remove,
at the single highest-authority credential the deployment will ever have.
An unauthenticated first-run window is worse: it converts a bootstrap
problem into a race.

The remaining candidates are an out-of-band operator tool with direct store
access, and supplying a public key as deployment configuration. Only the
second is declarative, and the platform is already moving its OpenBao
policy application toward a declarative apply.

## Decision

### 1. A keyspace is a project-anchored policy boundary

Keyspaces are named under the project per
[ADR#0046](./0046-project-anchored-resource-hierarchy.md) section 3:

```text
projects/{project}/api-keyspaces/{keyspace}
projects/{project}/api-keyspaces/{keyspace}/keys/{key}
```

A keyspace is registry-resolved and appears in the resource name, so it is
a handle per [ADR#0040](./0040-contract-field-vocabulary.md) and carries
bare `name`. Its own identity is `id`. Individual keys are not handles;
they carry `id` and `display_name`.

### 2. Keyspace policy fields

```text
environment          immutable after creation
bearer_issuance      allowed | disallowed
allowed_algorithms   non-empty subset of {Ed25519, ES256}
server_nonce         optional | required
max_token_lifetime   <= 2 minutes (the ADR#0051 ceiling)
default_key_expiry
max_active_keys
reroll_grace         (ADR#0065)
```

Environment immutability is load-bearing rather than incidental: it is
what lets the wire-format environment marker in
[ADR#0062](./0062-api-key-format-and-verifier-construction.md) be a
rendering of the keyspace attribute instead of a denormalized copy that
can drift. Moving a keyspace between environments is not an operation;
creating a new keyspace is.

### 3. Two keyspaces ship with every project

Project creation creates both. Neither is created by the user, and neither
can be deleted while it holds active keys.

```text
management
  -> bearer_issuance: disallowed  (ADR#0050 section 5)
  -> server_nonce: required       (ADR#0051 section 3)
  -> holds root and management keys for the management API

default
  -> bearer_issuance: allowed
  -> server_nonce: optional
  -> holds ordinary product API keys
```

Users may create additional keyspaces. Creating them automatically is what
removes the chicken-and-egg on the cloud path: a project always has
somewhere to put its first key at the moment it exists.

### 4. Self-hosted bootstrap: register a root public key from configuration

```text
1. The operator generates a key pair locally with the CLI.
2. Only the public key is supplied to the platform, as deployment
   configuration.
3. On first boot the platform registers it as a root signed key in the
   project's management keyspace.
4. The private key never leaves the operator's machine.
```

- Registration is idempotent by public-key fingerprint (the `sha256:` SPKI
  fingerprint from ADR#0062 section 4), so re-applying the same
  configuration is a no-op and the flow is safe under a declarative apply.
- If a root key already exists under a different fingerprint, first boot
  does not replace it. Adding a second root key is an explicit operation,
  never a side effect of editing configuration.
- The registration emits `api_key.created` with bootstrap provenance and a
  null actor, because no authenticated actor exists before the first key
  does. This is the only unattributed create on the platform, and the audit
  fact records it as unattributed rather than inventing a system actor to
  fill the field.
- No secret crosses the boundary at any point, so
  [ADR#0048](./0048-one-time-plaintext-exposure.md)'s one-time display
  never engages and ADR#0050 section 2 holds without an exception carved
  into it.

### 5. Cloud has no separate bootstrap

On the hosted product the first root key is registered by an authenticated
human session through the console, against the human identity system
([ADR#0053](./0053-external-oidc-federation-surface.md)), not by an API
key. Bootstrap is a self-hosted concern only. Saying so explicitly is what
keeps the unauthenticated-first-run window from reappearing later as a
cloud onboarding convenience.

### 6. Break-glass recovery when every root private key is lost

An operator CLI with direct store access, runnable only on a platform host,
registers an additional root public key. It maps to the
`break_glass_admin` role in the authorization matrix and emits an audit
fact. Because the matrix records that no OpenBao audit device exists yet,
provisioning one is a prerequisite for this path being usable in
production, not separate work.

Losing every root private key with no break-glass host access is
unrecoverable by design. That is the correct trade for a platform whose
central promise is that it never holds a caller's private key.

## Consequences

- ADR#0050 section 5 and ADR#0051 section 3 stop referring to an undefined
  object; keyspace policy has a field list an implementation can read.
- Every project gets a signed-only management keyspace from creation, so
  the management API has no bearer-authenticated era to migrate off later.
- Bootstrap becomes declarative and re-appliable, which suits the same
  Terraform direction the OpenBao policy work is headed for, and produces
  no secret to lose, leak, or rotate.
- Self-hosted operators take on private-key custody at install time, before
  they have used the product. The CLI has to make key generation a single
  obvious command, or this decision becomes the platform's first
  onboarding cliff.
- One audit fact in the system has a null actor, and every consumer of
  audit facts has to tolerate that rather than assume attribution.
- Provisioning an OpenBao audit device moves from a general operational gap
  to a blocker on break-glass recovery.

## References

- [ADR#0040: Contract Field Vocabulary: Identifiers, Handles, and Display Labels](./0040-contract-field-vocabulary.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0053: External OIDC Federation Surface for Agent Identity](./0053-external-oidc-federation-surface.md)
- [ADR#0062: API Key Format and Bearer Verifier Construction](./0062-api-key-format-and-verifier-construction.md)
- [ADR#0065: API Key Rotation Grace and the First-Release Audit Set](./0065-api-key-rotation-grace-and-audit-set.md)

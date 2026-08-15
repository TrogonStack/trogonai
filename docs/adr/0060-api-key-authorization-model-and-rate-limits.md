---
number: "0060"
slug: api-key-authorization-model-and-rate-limits
status: accepted
date: 2026-08-15
---

# ADR#0060: API Key Authorization Model and Rate Limits

## Context

API_KEY.md's data model gives a key six authorization-shaped fields:
`scopes`, `roles`, `direct_permissions`, `allowed_vaults`,
`allowed_integrations`, and `allowed_environments`. `ApiPrincipal` carries
five of them. Two of those, scopes and permissions, are the same idea under
two names, and nothing in the design says which one an authorization check
reads or what happens when they disagree. Shipping both would mean every
future check has to answer that question again.

Two questions are explicitly open: how much RBAC belongs in the first
milestone, and whether identity-level shared rate limits are needed
immediately. A third is open in practice rather than on the list: API_KEY.md
states that a key must not create another key with more authority than the
creator can delegate, but nothing says what enforces it, and a delegation
rule that lives only in prose is a rule that ships broken.

The rate-limit question has an infrastructure constraint attached. The
design documents mention Valkey, but Valkey is not deployed anywhere in
this repository; it appears only as a named non-goal. The replay store
already lives in NATS JetStream KV per
[ADR#0051](./0051-fully-bound-request-signing.md) section 4.

## Decision

### 1. One vocabulary: permissions

`scopes` is dropped, from `ApiKey` and from `ApiPrincipal`. Every
authorization check reads one effective permission set:

```text
effective_permissions =
  union(permissions of assigned roles) union direct_permissions
```

This amends the `ApiPrincipal` shape in both API_KEY.md and
CREDENTIAL_PLATFORM_SPEC.md, which currently list `scopes` and `roles` side
by side.

### 2. Built-in roles only in the first milestone

A fixed, platform-defined set. Customers assign roles; they do not author
them.

```text
owner      every permission in the project
admin      manage vaults, credentials, integrations, and keys
operator   rotate and revoke credentials; create runtime sessions
reader     metadata reads only
runtime    resolve refs it is authorized for; no management verbs
```

Customer-defined roles are deferred. They need a role-management surface, a
delegation story for role authorship itself, and a UI, none of which exist,
and the built-in set already covers every persona the authorization matrix
names.

### 3. `direct_permissions` stays, bounded and visible

It is the escape hatch for the case a built-in role does not fit, and it is
constrained rather than free:

- it may only add permissions the assigning actor already holds (section 5);
- it is rendered in the UI as a distinct badge, never folded into the role
  display;
- keys carrying direct permissions are marked as such in list views.

API_KEY.md warns that direct permissions make audits harder. This turns
that warning into a product property somebody can see, rather than a line
of documentation the person granting them has not read.

### 4. Resource scope is a separate, ANDed dimension

`allowed_vaults`, `allowed_integrations`, and `allowed_environments`, along
with the runtime-side `allowed_runtime_services` and `allowed_hosts` that
already exist in code, narrow where a permission applies. They never grant.
A request must pass both the permission check and every applicable scope
check.

Each is a value object, not a bare list, specifically to close the
empty-list ambiguity that otherwise decides a security question by
accident:

```text
ResourceScope::All          project-wide, still subject to permissions
ResourceScope::Only(set)    exactly these; set must be non-empty
```

`Only(empty)` is a validation error at creation time rather than a runtime
condition whose meaning has to be guessed. Nothing in the system has to
decide whether an empty list means everything or nothing, because an empty
list cannot exist.

### 5. The delegation ceiling is a checked invariant

Create and patch fail with `permission_denied` when the resulting key's
effective permissions are not a subset of the acting principal's effective
permissions, or when its resource scope is not contained in the acting
principal's scope. `api_keys.create` confers the ability to create a key,
never the ability to grant an authority the creator lacks.

Root keys are not exempt. A root key's ceiling is simply the whole project,
so the same check runs and passes.

### 6. Per-key and per-project rate limits first; identity-level deferred

`RateLimitPolicy.subject_kind` keeps `identity` and `route` as declared
values so adding them later is a policy row rather than a schema migration.
Neither is enforced in the first milestone, and the specification says so
plainly instead of implying the model is fully covered.

Identity-level limits need a counter shared across keys, which is the one
shape that converts a per-key local decision into cross-key contention on
the admission path. There is no shipped identity concept to attach one to,
so paying that cost now buys nothing.

### 7. Counters live in NATS KV, with a named exit condition

Rate-limit counters use NATS JetStream KV with revision-checked updates,
alongside the replay store. This adds no infrastructure dependency, which
matters more in the first milestone than counter throughput does.

The ceiling is real and stated rather than discovered later: compare-and-set
on a per-key record bounds write throughput per key, so the first
milestone's limiter is coarse. The trigger for moving to a dedicated
counter store is sustained CAS retry rates on the admission path, or a
configured per-key limit high enough that the counter becomes the
bottleneck before the limit does.

### 8. Fail behavior splits by what the limit protects

```text
ordinary keyspaces   fail open when the counter store is unavailable
management keyspaces fail closed
```

A rate limiter on product traffic is a fairness control, and its outage
should not become an API outage. On a management keyspace the same
mechanism is an abuse control on privileged operations, and unavailability
should not widen what a root key can attempt in a burst.

Both paths emit a distinct metric, so a fail-open window is visible rather
than silent.

This is deliberately unlike the replay store, which fails closed in every
case per ADR#0051 section 4. Replay protection is a security control; rate
limiting mostly is not, except where it is, and that exception is exactly
the management keyspace.

## Consequences

- One authorization vocabulary, so no check has to reconcile scopes against
  permissions and no key can carry two disagreeing grants.
- `ApiPrincipal` loses a field, which is a breaking change to a shape that
  exists only in design documents today and would be expensive to make
  after the first implementation.
- Customers cannot author roles in the first release, and some will ask.
  The escape hatch is `direct_permissions`, which is visible and bounded
  rather than convenient.
- Resource scoping becomes value-object-shaped, which forecloses the
  empty-list footgun at the type level instead of in review.
- The delegation ceiling becomes a test rather than a paragraph.
- Rate limiting starts coarse and NATS-backed, with the conditions for
  outgrowing it written down before it is built.
- A fail-open window on ordinary traffic is accepted and instrumented,
  rather than avoided by making the limiter a single point of failure for
  the whole API.

## References

- [ADR#0047: Event Stream as the Credential Metadata Source of Truth](./0047-event-sourced-credential-metadata.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0059: API Keyspaces and Root Key Bootstrap](./0059-api-keyspaces-and-root-key-bootstrap.md)

---
number: "0027"
slug: decider-multi-tenancy-primitive
status: accepted
date: 2026-07-15
---

# ADR#0027: Declared Subject Scope for Decider Stream Resolution

## Context

`trogon_decider_nats::JetStreamStore<Resolver>` binds to exactly one physical
[JetStream](../glossary/jetstream) `events_stream` and one physical KV `snapshot_bucket` per store
instance. Isolation across tenants depends entirely on the
`StreamSubjectResolver<StreamId>` implementation an application supplies --
the trait's own doc comment said implementations "usually compose a tenant or
aggregate prefix with the caller's stream id." "Usually" was the whole
enforcement mechanism. `resolve_subject_state` returned a
`SubjectState { subject, current_position }` that the store passed straight to
JetStream, and `ReadSnapshotRequest`/`WriteSnapshotRequest` carry a bare
`snapshot_id: &StreamId` with the same absence. Nothing in `JetStreamStore`
checked that a resolved subject was actually scoped to anything; the store read
and wrote whatever the resolver computed.

The event-metadata story is the same convention, not a type: if a workflow
needs tenancy recorded, `docs/architecture/event-metadata.md` says the
application builds it as a header at the boundary, validated there, before
calling `CommandExecution::with_headers`. The stream store's own tests
demonstrate exactly this pattern with `Headers::from_entries([("tenant",
"trogon"), ...])` -- a plain string key an application chose to set, invisible
to the store, and unrelated to which physical subject or KV key the event
actually landed on.

A resolver bug, two tenants' resolvers computing overlapping subjects, or a
caller passing an already-prefixed id where an unprefixed one was expected
would silently cross tenant boundaries, because nothing the store could see
described where a resolver was supposed to write.

## Decision

### 1. A resolver declares the subtree it writes into

`StreamSubjectResolver` gains

```rust
fn subject_scope(&self) -> Option<&SubjectScope> { None }
```

`SubjectScope` is a validated subject prefix, normalized to end on a token
boundary, refusing wildcards and malformed dots. It offers `contains(&StreamSubject)`
and `pattern()`, the latter being the `prefix.>` subscription form.

The primitive is a *subject scope*, not a `Tenant`. Tenancy is one thing a
scope can encode; a module subtree and an aggregate family are others, and the
decider crates have all three in-tree while having no business with the word
"tenant" (the draft's first open question, resolved below). A consumer that
wants a `Tenant` type owns it and builds a scope from it.

### 2. The store holds the resolver to the scope it declared

`JetStreamStore` verifies the resolved subject falls inside the declared scope
before every read and before every append, and returns
`JetStreamStoreError::SubjectOutsideScope { subject, scope }` on mismatch.

This is a real check and not a resolver agreeing with itself, because the two
values compared have different origins: the scope is fixed when the resolver is
*constructed*, and the subject is derived per call from a stream id the
resolver did not choose. A resolver that composes the wrong prefix, or that is
handed an id already carrying someone else's prefix, is caught. The check runs
before the publish, so a refused write leaves the escaping subject empty.

A resolver that declares no scope is passed through unchecked. That is the
honest answer for an open topology, and it is what makes the whole decision
non-breaking (the draft's second open question, resolved below).

### 3. Snapshot keys stay out of scope

The snapshot surface gets no equivalent. `snapshot_key` composes a
`SnapshotType`-derived prefix with a `snapshot_id` the caller passes in
directly as a `&str`; there is no snapshot resolver, so there is no
construction-time fact for a per-call key to be checked against. Adding a
tenant parameter there would have the caller supply both halves of the
comparison, which verifies only that the caller is self-consistent. Scoping a
snapshot key remains the caller's, composed into the `snapshot_id`.

### 4. Physical per-tenant isolation needs no new API

A tenant needing its own stream and bucket gets one by constructing its own
`JetStreamStore`; the store is already one-stream-one-bucket and already
parameterized by resolver. The L5 host does exactly this per module today. No
`TenantBinding` type is introduced, because it would only re-express the
builder that exists.

### 5. Migration

Every existing resolver keeps compiling and keeps its current behavior,
because `subject_scope` defaults to `None`. Adopting the check is one method on
a resolver that already knows its own prefix. Nothing historical is rewritten;
validation applies going forward from adoption, the same posture
[ADR#0013](./0013-origin-stream-sequence-header.md) takes toward provenance
metadata on pre-existing events.

## Alternatives Considered

### A `Tenant` value object required on `StreamSubjectResolver`

The original draft. Rejected on two counts.

It puts deployment topology in crates whose admission bar is business-agnostic
and domain-level, and it names one use of scoping as though it were the only
one: the sole in-repo resolver with a real isolation boundary to defend
(`ModuleEventSubjects`, which keeps one module's events out of another's
subtree) is not multi-tenancy at all, and would have had to pretend to be a
tenant to get the check.

It also breaks every existing resolver implementation, including single-tenant
consumers who gain nothing, which the draft correctly flagged as a trade-off
requiring sign-off. A defaulted method needs no such sign-off, and buys the
same guarantee for any resolver that opts in.

### Keep tenancy as a header-only convention, add lint or test discipline

Rejected. This is the "no design, just a doc comment" state the audit
identified. A header is invisible to the store, which is the actual isolation
boundary for JetStream reads and writes; a lint on application code cannot
catch a resolver bug that computes an unscoped subject internally.

### Require a dedicated physical stream and bucket per tenant, always

Rejected as the default. It forecloses small or low-volume tenants sharing
infrastructure cheaply, which is the reason the current single-shared-stream
design exists, and it is a heavier operational default than most deployments
need. It remains available per Decision 4, as a deployment choice rather than
an API.

### Enforce tenancy only through NATS account/subject permissions

Rejected as the sole mechanism. Server-side ACLs protect against a
compromised or malicious client but do nothing for a resolver bug inside a
trusted process holding broad JetStream credentials -- which is this crate's
own client shape. A host-side check catches programming errors ACLs cannot
see, the same reasoning [ADR#0023](./0023-secret-management-and-key-custody-direction.md)
applies to keeping OpenBao access behind a single trusted client rather than
relying on network policy alone.

### Keep the verification entirely in a consumer-owned layer

A multi-tenant consumer could wrap `StreamSubjectResolver` and validate the
resolved subject before handing it to the store. Rejected as the *only*
mechanism, though it is what Decision 3 leaves in place for snapshot keys.

Two reasons the check belongs in the store for subjects. The wrapper would
reimplement the same comparison in every consumer, and a wrapper is the layer
most likely to be omitted precisely where a bug already exists. More
importantly, the store is the last place the subject is a value rather than a
wire operation, so a check there is the last one that can still refuse.

What the consumer-owned argument does correctly rule out is the `Tenant` type
itself, which is why Decision 1 keeps the primitive at the altitude the store
can actually reason about and leaves the tenancy vocabulary above it.

## Non-Goals

- Designing per-tenant NATS accounts, credentials, or ACL policy. That is an
  operational/deployment concern layered on top of this primitive.
- A `Tenant` type, in these crates. Consumers own their tenancy vocabulary and
  project it onto a `SubjectScope`.
- Scoping snapshot keys at the store boundary (Decision 3).
- Retrofitting historical subjects written before adoption. Validation applies
  going forward only.
- Cross-tenant data sharing or tenant-to-tenant migration tooling.
- Changing the command-authorization boundary
  ([ADR#0026](./0026-command-authorization-principal.md)). A scope is a
  storage-resolution concept; associating a caller's authorization principal
  with a scope is left to the application boundary.

## Resolved Questions

The draft blocked acceptance on two questions. Both are answered by moving the
primitive down an altitude, from a tenancy concept to a subject-scoping one.

1. **Placement.** Does a tenancy primitive belong in the decider crates at all?
   No. `Tenant` is deployment topology and stays with the consumer. What does
   belong is `SubjectScope`, because a subject prefix is a thing these crates
   already own the grammar for, and because scoping serves the module and
   aggregate boundaries in-tree today as well as it would serve a tenant.
2. **The breaking resolver change.** Is breaking every existing resolver an
   acceptable cost? The question does not arise. `subject_scope` is defaulted,
   so existing implementations compile and behave unchanged, and the guarantee
   is available to any resolver that opts in. The only source-breaking residue
   is a new `JetStreamStoreError` variant.

## Invariants

- A `SubjectScope` never contains a wildcard, so `contains` can never be
  vacuously true.
- A scope's prefix always ends on a token boundary, so a scope over `orders`
  does not contain `orders_archive.7`.
- A subject equal to the bare prefix is outside the scope: a stream occupies a
  token of its own within the subtree.
- The store's check runs before the JetStream operation, on both the read and
  the append path. A refused append leaves the subject it reached for empty.
- A resolver returning `None` from `subject_scope` is unchecked, and this is
  the default. Absence of a scope is never treated as a violation.

## Consequences

- `JetStreamStoreError` gains a `SubjectOutsideScope` variant that exhaustive
  callers must add to their match arms. This is the only source-breaking part
  of the decision, and it is a variant addition rather than a signature change.
- `ModuleEventSubjects::new` became fallible, because the module name is the
  whole of its scope and a name that cannot form one leaves the resolver with
  no subtree to be held to. The host learns this at startup rather than on a
  command.
- A consumer gets no protection until it declares a scope. The default is
  deliberately fail-open, so the crates cannot claim isolation they have not
  been told how to check. Only the L5 host declares one today.
- Snapshot keys remain unverified at the store boundary. A deployment whose
  isolation depends on snapshot scoping carries that risk in its own layer, and
  Decision 3 records why.

## References

- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [Event Metadata](../architecture/event-metadata.md)

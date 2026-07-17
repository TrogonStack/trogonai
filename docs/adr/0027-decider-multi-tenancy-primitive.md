---
number: "0027"
slug: decider-multi-tenancy-primitive
status: draft
date: 2026-07-15
---

# ADR#0027: Tenant Value Object for Decider Stream and Snapshot Resolution

## Context

`trogon_decider_nats::JetStreamStore<Resolver>` binds to exactly one physical
JetStream `events_stream` and one physical KV `snapshot_bucket` per store
instance. Isolation across tenants depends entirely on the
`StreamSubjectResolver<StreamId>` implementation an application supplies --
the trait's own doc comment says implementations "usually compose a tenant or
aggregate prefix with the caller's stream id." "Usually" is the whole
enforcement mechanism. `resolve_subject_state` returns a
`SubjectState { subject, current_position }` with no tenant field anywhere in
it, and `ReadSnapshotRequest`/`WriteSnapshotRequest` carry a bare
`snapshot_id: &StreamId` with the same absence. Nothing in `JetStreamStore`
checks that a resolved subject or snapshot key is actually scoped to the
tenant the caller believes it is operating as; the store reads and writes
whatever the resolver computed.

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
would silently cross tenant boundaries today, because "tenant" is not a
concept the store or the resolver contract has any way to check against.

## Decision

### 1. A `Tenant` value object

Introduce `Tenant` as a validated, non-empty identifier type in
`trogon_decider_runtime`, alongside the crate's existing value objects
(`StreamPosition`, `EventId`). A `Tenant` is constructed once, at the
application boundary that knows which tenant is operating, the same place
that already assembles `Headers` and a `CommandPrincipal`
([ADR#0026](./0026-command-authorization-principal.md)) before execution.

### 2. Tenant-aware resolution, verified at the store boundary

Extend `StreamSubjectResolver` and the snapshot key resolution surface
(currently the bare `&str` snapshot id passed into
`crate::snapshot_store::{read_snapshot, write_snapshot}`) to take a `Tenant`
explicitly, and to return a tenant-scoped result type rather than a bare
`StreamSubject`/`String` -- so "did the resolver honor the tenant" is a
value the store can check, not a hope about the resolver's implementation.
`JetStreamStore` validates the resolved subject/key against the requested
`Tenant` before reading or writing and returns a typed error on mismatch,
turning a resolver bug into a caught error instead of a silent cross-tenant
read or write.

### 3. Optional per-tenant stream/bucket binding

`JetStreamStoreBuilder` gains an optional `TenantBinding` (`Shared` or
`Dedicated(events_stream, snapshot_bucket)`), resolved per tenant. Most
tenants keep today's default: one shared physical stream and bucket,
distinguished by subject/key scoping alone. A tenant that needs hard physical
isolation (noisy-neighbor containment, a compliance requirement, or its own
retention posture) can be bound to its own stream and bucket without changing
`JetStreamStore`'s public shape or any caller's resolver code.

### 4. Migration for existing single-tenant deployments

An existing deployment supplies one static `Tenant` value at the point where
it constructs its resolver today -- a one-line change, not a data migration.
Its existing subjects and snapshot keys already satisfy the new validation
trivially, because there is exactly one tenant and the resolver's output
already matches it. No historical subject or key is rewritten; validation
applies going forward from adoption, the same posture
[ADR#0013](./0013-origin-stream-sequence-header.md) takes toward provenance
metadata on pre-existing events.

## Alternatives Considered

### Keep tenancy as a header-only convention, add lint or test discipline

Rejected. This is the exact "no design, just a doc comment" state the audit
identified. A header is invisible to the store, which is the actual
isolation boundary for JetStream and KV reads and writes; a lint on
application code cannot catch a resolver bug that computes an unscoped
subject internally.

### Require a dedicated physical stream and bucket per tenant, always

Rejected as the default. It forecloses small or low-volume tenants sharing
infrastructure cheaply, which is the reason the current single-shared-stream
design exists, and it is a heavier operational default than most deployments
need. Kept as an opt-in `TenantBinding::Dedicated` rather than the only
supported mode.

### Enforce tenancy only through NATS account/subject permissions

Rejected as the sole mechanism. Server-side ACLs protect against a
compromised or malicious client but do nothing for a resolver bug inside a
trusted process holding broad JetStream credentials -- which is this crate's
own client shape. A host-side check catches programming errors ACLs cannot
see, the same reasoning [ADR#0023](./0023-secret-management-and-key-custody-direction.md)
applies to keeping OpenBao access behind a single trusted client rather than
relying on network policy alone.

## Non-Goals

- Designing per-tenant NATS accounts, credentials, or ACL policy. That is an
  operational/deployment concern layered on top of this primitive.
- Retrofitting historical subjects or snapshot keys written before adoption.
  Validation applies going forward only.
- Cross-tenant data sharing or tenant-to-tenant migration tooling.
- Changing the command-authorization boundary
  ([ADR#0026](./0026-command-authorization-principal.md)). `Tenant` is a
  storage-resolution concept; associating a caller's authorization principal
  with a tenant is left to the application boundary.

## Consequences

- `StreamSubjectResolver` and the snapshot key resolution surface gain a
  required `Tenant` parameter -- a breaking change for existing resolver
  implementations, mitigated by the single-static-tenant migration path.
- `JetStreamStoreError` gains a tenant-scope-mismatch variant that existing
  callers must add to their match arms.
- Operators gain the option of per-tenant physical isolation, which adds
  operational surface: more streams and buckets to provision, monitor, and
  retention-manage. This composes with, and multiplies the scope of,
  retention and compaction (see the companion retention ADR): per-tenant
  buckets mean per-tenant watermark computation instead of one.
- Gets harder: any code that constructs a stream id or resolver ad hoc,
  without going through the `Tenant`-aware path, must be updated -- which is
  every current caller, since "append a string prefix" is the entire
  mechanism today.

## References

- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [Event Metadata](../architecture/event-metadata.md)

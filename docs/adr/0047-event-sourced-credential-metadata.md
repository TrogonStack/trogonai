---
number: "0047"
slug: event-sourced-credential-metadata
status: accepted
date: 2026-08-05
---

# ADR#0047: Event Stream as the Credential Metadata Source of Truth

## Context

Which store owns credential metadata is an open question for the credential
platform: Postgres, NATS KV, or an existing control-plane database. The
question predates the shipped slice, and the shipped slice has already
answered half of it by construction: the credential aggregate is an
event-sourced decider on NATS JetStream (the
[ADR#0035](./0035-session-store-decider-aggregate.md) pattern), with protobuf
state snapshots on a fixed frequency, a protobuf NATS KV idempotency ledger,
and checkpointed read-side projections (runtime projection and recovery
worker) whose cursors live in KV buckets. The gateway has no database
dependency.

Introducing a relational store now would create a second write model beside
the stream and the dual-write hazards that come with it, in exchange for
query capabilities (listing, filtering) that no shipped surface requires yet.
The first product slices need correct command handling, idempotent retries,
and runtime resolution, all of which the stream already provides.

## Decision

The event stream is the source of truth for credential metadata. No
relational database enters the platform for the first version.

- Operational records (idempotency, projection and worker checkpoints, and
  operation records once they exist) live in NATS KV as protobuf payloads,
  keyed and scoped the way the existing idempotency ledger is.
- Listing and query surfaces are deferred. When the product needs them, they
  are built as read-side projections over the same streams. Rebuildability is
  bounded by retention: a projection rebuild replays the retained event
  range, and events purged below the
  [ADR#0029](./0029-decider-retention-and-truncation-watermark.md) watermark
  are not recoverable by a rebuild; past truncation, only the aggregate's
  own snapshot-carried state survives, and that serves the write side, not
  a projection replay.
- A relational store may arrive later only as another projection consumer.
  It never becomes a write model, and no command handler ever writes to it
  directly.

## Consequences

- Persistence for vaults, credentials, versions, operations, idempotency
  records, and audit facts is reframed: each is either an aggregate on the
  stream, a KV record, or a projection, not a table.
- The public API's first slice ships without list endpoints; single-resource
  reads resolve through projections or aggregate replay. List endpoints
  arrive with their projections.
- Audit facts are events; an audit query surface is a projection over the
  retained stream range.
  [ADR#0029](./0029-decider-retention-and-truncation-watermark.md) keeps
  truncation an operator-invoked decision, so preserving audit-relevant
  history is a retention-policy commitment made before any purge runs.
- Retries and recovery keep exactly one consistency mechanism (stream
  position plus scoped idempotency), avoiding cross-store reconciliation
  between a database and the stream.
- Accepted limitation: no ad-hoc queries until a projection exists for the
  question being asked.

## References

- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0046: Project-Anchored Resource Hierarchy for the Credential Platform](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0029: Snapshot-Derived Retention Watermark for Decider Streams](./0029-decider-retention-and-truncation-watermark.md)

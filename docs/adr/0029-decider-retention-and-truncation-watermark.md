---
number: "0029"
slug: decider-retention-and-truncation-watermark
status: draft
date: 2026-07-15
---

# ADR#0029: Snapshot-Derived Retention Watermark for Decider Streams

## Context

Nothing in `trogon-decider-nats` computes the minimum JetStream stream
sequence still needed for a logical stream. A snapshot's recorded
`StreamPosition` is, in principle, exactly the value that makes earlier
events safe to discard for that stream: both `CommandExecution` and
`WasmCommandExecution` resume replay from `ReadFrom::after(snapshot_position)`
after loading a snapshot, so nothing before the newest snapshot's position is
ever read again in the normal execution path. But nothing aggregates this
across the snapshot ids and checkpoints (`snapshots.checkpoint.*` keys,
tracked via `NatsSnapshotConfig`) that share the one physical stream and KV
bucket, and `JetStreamStore` never calls a JetStream purge or trim operation
at all -- it only reads stream info and raw messages, publishes, and reads or
writes KV entries. Streams and KV history both grow unboundedly by default.

Because every logical stream is a subject on one shared physical JetStream
stream, any retention action is a shared, blunt instrument unless it targets
a specific subject. A stream-wide `max_age`/`max_bytes`/`max_msgs` limit
cannot know that one logical stream's oldest event is still needed while
another's is not; tuning it for a fast-moving stream would silently evict a
slow-moving stream's un-snapshotted history.

[ADR#0013](./0013-origin-stream-sequence-header.md) already separates
physical JetStream stream sequence (authoritative for checkpoints,
high-water marks, and optimistic concurrency) from
`Trogon-Origin-Stream-Sequence` (provenance metadata only, present only on
restored or migrated events). Any retention design has to key off physical
positions, the same ones snapshots already record, never off origin
provenance -- a stream that has been restored has origin and physical
positions that diverge by design, and origin sequence is absent on the
common case of an ordinary append.

## Decision

### 1. A safe-truncation watermark API

Define a watermark computation, `MinimumRequiredSequence` (or equivalent),
that reports the minimum physical stream sequence that must be retained for
a given logical stream: `min(snapshot.position)` across every outstanding
snapshot and checkpoint that currently exists for that stream id, using the
`StreamPosition` values `SnapshotWrite`/`Snapshot` already record. Everything
strictly below that minimum is safe to discard for that logical stream,
because no execution path resumes from earlier than the newest retained
snapshot's position. The API is a read-only query: it reports a watermark
per stream id (or in aggregate across a physical stream), and computing it
has no side effect.

### 2. Truncation is invoked by an operator or a scheduled job, never by the store

`JetStreamStore` and the snapshot store expose the watermark as something a
caller queries; no method on either issues a JetStream purge or trim call as
a side effect of a normal read, append, or snapshot write. A separate,
explicitly invoked maintenance job (an operator command, a scheduled task, or
an admin action) reads the watermark and issues the purge. This keeps
"decide whether to truncate," an operational and reviewable decision, apart
from "execute a command," the store's actual job -- the same separation
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) draws
between routine operation and an operator-invoked, out-of-band action.

### 3. Recommended retention policy posture

Default to no automatic stream-wide `max_age`/`max_msgs` limit on the shared
physical stream: a stream-wide limit cannot know which logical streams still
need history no snapshot has covered yet. Retention is watermark-driven,
per-subject purge instead, run on a schedule that trails behind snapshot
cadence -- a job that fails to run for a while only means storage grows, never
that a needed event disappears prematurely. A conservative stream-wide
`max_age` far longer than any expected snapshot interval is a reasonable
backstop, but it is a safety net, not the retention mechanism.

### 4. Interplay with [ADR#0013](./0013-origin-stream-sequence-header.md) physical positions

The watermark is computed and purge is executed exclusively in terms of
physical JetStream stream sequence, never `Trogon-Origin-Stream-Sequence`. A
restored or migrated stream is truncated using its current physical sequence
like any other stream; origin provenance headers on events that remain after
truncation are unaffected and continue to describe where those events
originally lived.

## Alternatives Considered

### Let JetStream's own stream-level retention policy do the whole job

Rejected as the sole mechanism. `max_age`/`max_msgs`/`max_bytes` operate on
the whole physical stream and cannot know that one logical stream's oldest
event is still needed while another's is not.

### Auto-truncate whenever a new snapshot is written

Rejected. It turns every snapshot write into an operation with unbounded
downstream cost (a purge call), removes the operator's ability to pause,
review, or rate-limit truncation independently of command traffic, and makes
a bug in watermark computation immediately destructive instead of surfacing
first as an observable, reviewable report.

### Key the watermark off `Trogon-Origin-Stream-Sequence`

Rejected. That header is provenance metadata only
([ADR#0013](./0013-origin-stream-sequence-header.md)), deliberately not
authoritative for checkpoints or optimistic concurrency, and absent on the
common case of an ordinary append. Using it would either break on
non-restored streams or misjudge safety on restored ones, where physical and
origin positions diverge by design.

## Non-Goals

- Implementing the purge/trim job itself -- its scheduling, failure handling,
  or authorization. Only the watermark API it consumes and the
  invocation-boundary rule (operator/job, never the store) are in scope.
- A full KV bucket history-depth (compaction) design. Noting that KV
  revision history exists and grows is in scope; deciding how many
  historical revisions per snapshot key to retain is follow-up work once a
  stream-side watermark exists to model it against.
- Changing snapshot frequency or policy (`SnapshotPolicy`,
  `FrequencySnapshot`). Retention consumes whatever cadence a decider already
  chose.
- Per-tenant retention policy differences. Composes with, but does not
  depend on, [ADR#0027](./0027-decider-multi-tenancy-primitive.md).

## Consequences

- Operators gain an explicit, reviewable maintenance surface instead of
  unbounded growth, and an operational responsibility that has no effect
  until a deployment actually wires in and schedules the job.
- The watermark's correctness matters more than most read paths in this
  crate: undercounting outstanding snapshots or checkpoints is a data-loss
  bug, not a performance bug, and needs test coverage against every snapshot
  type a decider can register, not just the common case.
- Gets harder: adding a new snapshot type or checkpoint kind to a decider
  now carries a second obligation beyond storage -- it must be discoverable
  by whatever aggregates "all outstanding snapshots for this stream," or the
  watermark silently ignores it and a purge could delete events it still
  needs.
- Because retention purges by subject against one shared physical stream,
  purge operations for many logical streams still serialize against that one
  stream's admin surface. Operators truncating frequently across many
  tenants or streams should expect purge throughput, not just storage size,
  to become a capacity question.

## References

- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0027: Tenant Value Object for Decider Stream and Snapshot Resolution](./0027-decider-multi-tenancy-primitive.md)
- [NATS JetStream Source and Mirror Streams](https://docs.nats.io/nats-concepts/jetstream/source_and_mirror)

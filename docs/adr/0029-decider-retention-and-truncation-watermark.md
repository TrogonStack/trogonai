---
number: "0029"
slug: decider-retention-and-truncation-watermark
status: accepted
date: 2026-07-15
---

# ADR#0029: Snapshot-Derived Retention Watermark for Decider Streams

## Context

Nothing in `trogon-decider-nats` computes the minimum [JetStream](../glossary/jetstream) [stream](../glossary/stream)
sequence still needed for a logical stream. A [snapshot](../glossary/snapshot)'s recorded
`StreamPosition` is, in principle, exactly the value that makes earlier
events safe to discard for that stream: both `CommandExecution` and
`WasmCommandExecution` resume replay from `ReadFrom::after(snapshot_position)`
after loading a snapshot, so nothing before the newest snapshot's position is
ever read again in the normal execution path. But nothing aggregates this
across the snapshot ids and [checkpoints](../glossary/checkpoint) (`snapshots.checkpoint.*` keys,
tracked via `NatsSnapshotConfig`) that share the one physical stream and [KV
bucket](../glossary/kv-bucket), and `JetStreamStore` never calls a JetStream purge or trim operation
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

Define a watermark computation that reports the minimum physical stream
sequence that must be retained for a given logical stream:
`min(snapshot.position)` across every outstanding snapshot and checkpoint
that currently exists for that stream id, using the `StreamPosition` values
`SnapshotWrite`/`Snapshot` already record. Everything strictly below that
minimum is safe to discard for that logical stream, because no execution path
resumes from earlier than the newest retained snapshot's position. The API is
a read-only query: it reports a watermark per stream id (or in aggregate
across a physical stream), and computing it has no side effect.

"Nothing may be discarded" is a real answer, not a missing one, so the
watermark is a total type rather than a bare sequence: `RetentionWatermark`
is `RetainAll` or `DiscardBelow(StreamPosition)`, ordered so that `RetainAll`
is the least element and combining two constraints is their minimum. Every
case the computation cannot justify a boundary for -- a stream nobody has
snapshotted, a checkpoint that has recorded no progress, a stream one
observed snapshot type covers and another does not -- resolves to
`RetainAll`, so an incomplete picture over-retains instead of over-deleting.

Snapshot types are folded one at a time, because each carries its own payload
type and no single call can see them all. That makes the completeness of the
fold the caller's obligation, which the ADR already records as its sharpest
consequence; the builder's job is to make the unsafe direction unreachable
for whatever it *was* shown, not to pretend it was shown everything.

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

### 3. The read-only query lives in `trogon-decider-nats`

The watermark is derivable only from things that crate owns: the
`snapshots.data.*` and `snapshots.checkpoint.*` key grammar, the encoded
snapshot envelope that carries the `StreamPosition`, and the fact that
positions on this adapter *are* physical JetStream sequences. A maintenance
layer above it could not compute a watermark without reimplementing that
grammar, and a second implementation of the key layout is exactly the kind of
duplication that turns a rename into a data-loss bug.

This does not import operational policy into a business-agnostic crate,
because the query decides nothing: it reports what the store already knows,
the way `subject_current_position` and `read_checkpoint` do. The policy --
whether to truncate, how often, how far behind to trail -- stays with the
operator or job, and the purge call stays out of the crate entirely, per
Decision 2. That line, query in, action out, is what keeps the placement
consistent with the domain-level admission bar.

### 4. Recommended retention policy posture

Default to no automatic stream-wide `max_age`/`max_msgs` limit on the shared
physical stream: a stream-wide limit cannot know which logical streams still
need history no snapshot has covered yet. Retention is watermark-driven,
per-subject purge instead, run on a schedule that trails behind snapshot
cadence -- a job that fails to run for a while only means storage grows, never
that a needed event disappears prematurely. A conservative stream-wide
`max_age` far longer than any expected snapshot interval is a reasonable
backstop, but it is a safety net, not the retention mechanism.

### 5. Interplay with [ADR#0013](./0013-origin-stream-sequence-header.md) physical positions

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

### Put the watermark query in a separate maintenance or tooling layer

Rejected (Open Question 1, resolved). The appeal is that retention is an
operational concern and the decider crates are meant to stay
business-agnostic. But the query reads the snapshot bucket's own key grammar
and envelope format, so a layer above would have to reimplement both, and the
two copies would disagree the first time either changed. The concern the bar
actually protects against is policy, not knowledge: this ADR keeps the purge
itself out of the crate (Decision 2), which is where the operational decision
lives. A maintenance layer remains the right home for the job that consumes
the watermark.

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
- Per-[tenant](../glossary/tenant) retention policy differences. Composes with, but does not
  depend on, [ADR#0027](./0027-decider-multi-tenancy-primitive.md). The caveat
  an earlier draft of that ADR created, a `TenantBinding::Dedicated` type
  forcing per-tenant watermark computation, no longer applies: #0027 ships no
  such type. A deployment that does give a tenant its own stream and bucket
  computes a watermark per store, which is what this ADR's API already does.

## Resolved Questions

1. **Placement.** Resolved: the read-only query lives in
   `trogon-decider-nats` (Decision 3); the purge stays outside it
   (Decision 2). See the maintenance-layer alternative above for the
   rebuttal.
2. **Relationship to [ADR#0035](./0035-session-store-decider-aggregate.md).**
   Resolved for now by scope rather than by precedence: what this ADR ships
   is the read-only watermark, which that draft explicitly retains as a
   diagnostic even where it declares session streams keep-forever. The two
   documents therefore do not conflict on anything either of them currently
   specifies. The supersession claim only bites once a purge job exists, and
   designing that job is a Non-Goal here, so [ADR#0035](./0035-session-store-decider-aggregate.md) stays free to settle
   session-stream retention on its own terms.

Only the watermark computation and its placement were accepted. The purge
job remains unspecified and unbuilt, and nothing in the store calls one.

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
- The aggregate watermark across a physical stream is the one number that can
  be wrong by omission rather than by miscalculation: a logical stream nobody
  has ever snapshotted does not appear in the report at all, so a trim of the
  whole physical stream at that aggregate would delete exactly the history
  that stream still needs. Callers that want a sound aggregate have to
  declare the stream ids they know exist, and a report over an incomplete set
  of ids is only safe to act on per stream.
- Watermarks are keyed by snapshot id, which both execution paths set to the
  stream id, so mapping a watermark onto the subject a purge would target
  needs the same `StreamSubjectResolver` the caller configured. The crate
  reports positions, not subjects.
- Because retention purges by subject against one shared physical stream,
  purge operations for many logical streams still serialize against that one
  stream's admin surface. Operators truncating frequently across many
  tenants or streams should expect purge throughput, not just storage size,
  to become a capacity question.

## References

- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0027: Declared Subject Scope for Decider Stream Resolution](./0027-decider-multi-tenancy-primitive.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [NATS JetStream Source and Mirror Streams](https://docs.nats.io/nats-concepts/jetstream/source_and_mirror)

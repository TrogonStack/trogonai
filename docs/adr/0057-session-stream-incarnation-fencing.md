---
number: "0057"
slug: session-stream-incarnation-fencing
status: draft
date: 2026-08-21
---

# ADR#0057: Stream Incarnation Fencing by Subject Isolation and Sealing

## Context

[ADR#0035](./0035-session-store-decider-aggregate.md) facet 2 gives the Session
aggregate optimistic concurrency through a single server-side guard. The runtime
resolves the default [write precondition](../glossary/write-precondition) to
`At(current_position)`, `append_stream` turns it into a
`Nats-Expected-Last-Subject-Sequence` header, and a mismatch surfaces as a typed
`WrongExpectedVersion`. Creation batches use `NoStream`, which is that same
header carrying `0`. That guard is the whole of the fence: `Any` and
`StreamExists` send no header at all.

The guard compares a number against the [JetStream](../glossary/jetstream)
sequence of the last message on the session's subject. The comparison is only
meaningful while that number keeps naming the same event. Two operations break
that assumption:

1. **A physical stream rebuilt or restored to an earlier point.** Sequences are
   reissued. A writer holding `At(5)` from before the rebuild can match a
   different event that now sits at sequence 5, and the append lands as though
   the writer had read current state.
2. **A session's subject emptied.** `subject_current_position` returns `None`,
   the guard resolves to `NoStream`, and a redelivered creation command starts a
   second history under an identity that already has one.

Neither is hypothetical in the current design. ADR#0035 facet 7 keeps the log
forever, which rules out purging a session subject as an ordinary operation, but
the same facet admits cold-storage tiering that evicts already-immutable events
from the hot stream and restores them on demand. Eviction is the second case
exactly. Disaster recovery is the first. And [ADR#0013](./0013-origin-stream-sequence-header.md)
already contemplates a restore path that re-appends archived events into a
replacement stream, where every sequence is new by construction.

The maintenance contract added for schema migration and salvage
([Session Maintenance](../architecture/session-maintenance.md)) made the gap
concrete: `StreamBoundary` carries an optional `incarnation` field and
`IndeterminateReason` carries `SOURCE_INCARNATION_CHANGED`, both of which are
unusable until something defines what an incarnation is and how a writer is kept
out of the wrong one.

The constraint that shapes every option is that JetStream offers exactly one
server-side guard per publish, and facet 2 has already spent it. Anything else a
writer checks is checked in a separate round trip, before the append rather than
with it.

## Decision

### 1. A live session subject's messages are never removed

No command, repair, operator action, or storage policy deletes, purges, or
evicts messages from the subject of a session that still exists. This is facet 7
stated as a rule about storage rather than as a consequence about semantics, and
it is what makes case 2 above unreachable: a subject that is never emptied never
resolves to `NoStream` for a session that already has a history.

The [session doctor](../architecture/session-doctor.md) already respects this.
Every `RepairAction` operates on derived state, and none of them reach the event
stream.

**This amends ADR#0035's cold-storage tiering bullet.** Eviction from a live
subject is no longer admissible in any form, because a partially evicted subject
and a fully evicted one produce the stale-guard and false-`NoStream` failures
respectively. A deployment that must bound the hot stream rebuilds into a new
incarnation under rule 2; it does not edit the one it is running on.

### 2. An incarnation belongs to the physical stream, never to one session

A **stream incarnation** is a label on the physical JetStream stream that holds
session events. It advances when, and only when, the platform rebuilds that
stream: a restore that does not preserve sequences, a disaster-recovery rebuild,
or a fleet schema migration. It never advances for a single session, because a
per-session incarnation would need a per-session fence and the only atomic
per-subject guard is already spoken for.

Every session in an incarnation moves to the next one together. A rebuild is a
fleet operation by definition, not an operation with fleet-wide side effects.

### 3. The incarnation is a subject token, and the fence is the subject

The incarnation appears as a token in the stream-owned prefix of the session
event subject, so incarnation `i1` and incarnation `i2` occupy disjoint subject
spaces and are bound by different physical streams:

```
sessions.i1.events.{session_id}      bound by SESSION_EVENTS_I1
sessions.i2.events.{session_id}      bound by SESSION_EVENTS_I2
```

`append_stream` publishes by subject. A writer that resolved its subject under
`i1` publishes to `i1` no matter what it believes about sequences, so its guard
is evaluated against the incarnation it actually read. It cannot reach `i2`, and
no number it holds can be made to match there.

This is what a checked incarnation token cannot give. Reading a token and then
publishing is two operations, and the rebuild can land between them. Putting the
incarnation in the address makes the check unnecessary rather than making it
faster: a message cannot be published to the wrong incarnation and simultaneously
appear in the right one.

### 4. Retiring an incarnation means sealing it

When a rebuild completes, the retired stream is sealed. JetStream sealing is
permanent and rejects every publish, delete, and purge for the life of the
stream. It cannot be lifted.

Sealing is what turns silent harmlessness into a loud failure. Without it, a
stale writer's append succeeds into a stream nobody reads, and the writer
believes it wrote. With it, the append fails at the server, and the writer
learns it is holding a subject that no longer accepts writes.

Irreversibility is the point. A fence an operator can lower under pressure is
not a fence, and the pressure to lower it arrives during exactly the incident
that produced the rebuild.

### 5. The ordinal is the only position that crosses an incarnation

`SessionOrdinal` is fold-derived from a session's own events, so it survives a
rebuild that preserves those events in order, which the migration contract
already requires as an admissibility rule. Stream sequences do not survive.

Everything that names a position from outside the stream already uses ordinals:
a fork's `context_prefix_boundary`, checkpoint evidence, `ConsistencyToken`,
`PageCursor`. Those references keep their meaning across a rebuild. Nothing else
does, which is why sequences stay confined to optimistic concurrency and
read-side checkpoints, and why ADR#0013 keeps `Trogon-Origin-Stream-Sequence` to
provenance.

### 6. Derived state does not cross an incarnation boundary

Projection checkpoints, snapshots, and materialized read models are scoped to the
incarnation whose sequences they cite, and are rebuilt after a rebuild.

A checkpoint from a retired incarnation is not stale. It is not applicable, and
applicability is decided by comparing incarnation labels rather than by comparing
sequence numbers, because the numbers are comparable and wrong. That is the whole
hazard restated on the read side: two positions from different incarnations
compare cleanly and mean nothing.

### 7. A session lost to a rebuild is salvaged, not recreated

If a lossy restore drops a session entirely, its subject in the new incarnation
is empty, `NoStream` succeeds, and a creation command mints a fresh history under
an identity that once meant something else.

That is data loss showing through, not a fencing failure, and no guard can
distinguish it from a legitimate first creation. The honest handling is salvage:
copy what can still be read into a new identity and mark it as a copy, per the
maintenance contract. Reusing the identity is the one case where requiring a new
Session identity is right, and it is right because the content is gone, not
because the stream was rebuilt.

### 8. `StreamBoundary.incarnation` becomes required

The field exists to keep a boundary computed in one incarnation from being
compared against a boundary in another. A migration's source boundary names the
retiring incarnation and its expected target names the new one, and
`INDETERMINATE_REASON_SOURCE_INCARNATION_CHANGED` fires when the source has moved
somewhere the intent did not anticipate. An unset incarnation is not "the same
incarnation", so leaving it optional leaves the comparison unable to fail.

## Alternatives Considered

### A stream-incarnation token checked with every invariant-bearing command

Rejected on atomicity. JetStream exposes one server-side guard per publish and
facet 2 spent it on `Nats-Expected-Last-Subject-Sequence`. A separately checked
token is a read followed by a write, so a rebuild that lands between them leaves
the append proceeding on a token that was true when it was read. That narrows the
race rather than closing it, and a fence that narrows a race is not a fence.

It also charges every invariant-bearing command an extra round trip forever, to
guard against an event that happens at most once per disaster.

### Prohibit stream recreation and require a new Session identity

Adopted for a session whose content was actually lost (rule 7), rejected as the
general rule. A disaster-recovery rebuild that preserves every event would, under
a literal reading, invalidate every session identity in the store: every fork
boundary, every stored `context_prefix_boundary`, every external reference held
by a client. That converts a recoverable outage into a total identity reset, and
it does so in the case where nothing was lost.

Identity should change when content changed. It should not change because the
bytes moved.

### Make restore preserve an independently verified incarnation identity

Adopted in substance, rejected in mechanism. Preserving an incarnation identity
across a restore is right, and rules 2 and 3 do it. Verifying it is where the
option fails, for the same reason the checked token fails: verification is a
separate operation from the append it is meant to protect. Carrying the identity
in the subject removes the need to verify anything.

### Start the rebuilt stream above every sequence the retired one issued

JetStream lets a stream be created with a starting sequence, so a rebuild could
begin above the retired stream's last sequence and defeat every stale `At(N)`.

Rejected as insufficient alone. It does nothing about `NoStream`, which carries
`0` and succeeds on any subject the rebuild left empty, which is precisely the
set of sessions a lossy restore dropped. It also keeps both incarnations in one
subject space, so nothing structural stops an old writer from reaching the live
stream. Once the subject spaces are disjoint the sequence spaces are independent
anyway, so the mechanism buys nothing on top of rules 3 and 4.

### Record the incarnation on each event and let the doctor report mixed streams

Rejected as a primary mechanism, because the requirement is to fence a writer
out, and detection reports damage after it lands.

It is also unnecessary. With the incarnation in the subject, a stream containing
events from two incarnations is unrepresentable, so there is no header to record
and no finding to raise. Self-evidencing beats attested, and this is the main
reason to prefer the subject over a field.

### A per-session incarnation

Rejected. It would need a per-session fence, and the atomic per-subject guard is
taken. Nothing in the design produces one either: rule 1 forbids removing a live
session's messages, so a single session's sequence space cannot restart while the
stream around it stays put.

## Consequences

- The fence is enforced by the server, atomically, on the operation it protects.
  A stale writer fails on its next append rather than on a check it might skip.
- The hot path is unchanged. No command pays for an incarnation check, because
  there is no incarnation check.
- Sealing cannot be undone, so an operator cannot restore write access to a
  retired incarnation during an incident. Recovering something from a retired
  incarnation means reading it and salvaging forward.
- A rebuild cannot be scoped to one session. The cheapest remedy for a single
  corrupt session is salvage, not a rebuild, which is the outcome the
  maintenance contract already assumes.
- Every projection, snapshot, and checkpoint is rebuilt after an incarnation
  change, so recovery cost scales with the whole store rather than with the
  damage. The doctor's existing `RESET_PROJECTION_CHECKPOINT` and
  `REBUILD_PROJECTION` actions cover the per-session case; the fleet case is an
  operational procedure that does not exist yet.
- Retired incarnations are retained, because facet 7 does not let events leave
  the log and sealing frees no bytes. Storage grows by roughly one full copy per
  rebuild. That is a cost keep-forever already accepts, stated rather than
  discovered.
- Anything that caches a resolved subject across a rebuild fails loudly. Writers
  must re-resolve the subject rather than retry the append, because a sealed
  stream rejects retries forever.
- ADR#0035's cold-storage tiering bullet is amended by rule 1 and is now
  reachable only as a rebuild.
- `StreamBoundary.incarnation` moves from optional to required, which is a
  presence change inside `v1alpha1` and wire-compatible.

## References

- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0029: Decider Retention and Truncation Watermark](./0029-decider-retention-and-truncation-watermark.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [Session Aggregate](../architecture/session-aggregate.md)
- [Session Doctor](../architecture/session-doctor.md)
- [Session Maintenance](../architecture/session-maintenance.md)

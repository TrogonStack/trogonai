# Session Crash Boundaries

Every place a Session write can be interrupted, what the caller is owed there,
and what a retry is allowed to do. This page is the specification the
crash-boundary tests will assert against. It documents the protobuf contract
that exists today and the derivation rule the contract depends on. There is no
Rust implementation yet, and there are therefore no tests yet.

See [Session Aggregate](./session-aggregate.md) for the write side and
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 2 for the
optimistic concurrency this builds on.

## The failure this exists for

The store accepts an event and the acknowledgment is lost. The writer knows it
published and nothing else.

Reporting failure is wrong, because the event may be committed and the caller
will act on a lie. Reporting success is wrong for the mirrored reason. Retrying
blindly is wrong because it writes a second copy of a fact that already
happened, and a session log with two `SessionStarted` events is not repairable
by anything downstream of it.

So the writer has to be told the outcome is unknown, and then be able to resolve
it. `WriteOutcome` is the first half. The identity rule below is the second.

## Identity is derived, never assigned

[Session Aggregate](./session-aggregate.md) already fixes the derivation:
UUIDv5 over the resolved stream subject, the command type, the command's
caller-supplied idempotency key, and the index within the batch. `Nats-Msg-Id`
carries the result, so a redelivered command publishes byte-identical ids and
collapses in the duplicate window.

The choice of terms is what makes a retry safe, and it is worth being precise
about which failures it covers.

**It keys on the command, not on the event.** A retry does not have to reproduce
the event's bytes, which it could not do anyway: events carry timestamps, so a
regenerated event differs from the one that may already be committed. Deriving
from the command means the retry reproduces the identity without reproducing the
encoding.

**It covers redelivery of the same command.** Same key, same subject, same
index, same id, so the store recognizes the repeat. That is the lost
acknowledgment case, and it resolves without anything else being true.

**It does not cover reuse of a key across different commands.** Two different
commands sent under one idempotency key derive the same id, and duplicate
detection keys on identity alone: the second is discarded and acknowledged as a
success. That is silent data loss wearing the shape of idempotency, and no
derivation prevents it, because the caller controls the key.

Which is why `CONFLICT_KIND_IDENTITY_REUSED_WITH_DIFFERENT_CONTENT` is a
reported conflict rather than an unreachable state. It is the direct consequence
of putting a caller-supplied value in the identity, and detecting it is the
price of that choice.

Two things this depends on do not exist yet. `EventIdentity for SessionEvent` is
the empty default, so the runtime assigns a fresh UUIDv7 per publish: a retry
today produces a different id and duplicate detection does nothing at all. And
the command protos carry no idempotency-key field, because ADR#0035 leaves the
key's structural home explicitly open. The derivation is documented and
unimplemented on both ends.

## Resolving a duplicate acknowledgment

The store's duplicate acknowledgment carries the sequence of the original
message. Today `append_stream` treats that acknowledgment as an error and
discards the sequence, which turns the one signal that resolves a lost
acknowledgment into a failure.

A duplicate acknowledgment is not ambiguous. Identity is derived from the
command, so a duplicate means this command's events are already on the log, and
the only thing left to establish is where. The rule is: read the message at the
returned sequence, resolve its `SessionOrdinal`, and report
`WRITE_STATE_DEDUPLICATED` with that ordinal. The read costs nothing on the
happy path, because a duplicate acknowledgment only happens on a retry.

It is worth being explicit that the store is not the place divergence is caught.
Reuse of one idempotency key across two different commands derives the same id,
and duplicate detection keys on identity alone, so the store would discard the
second command's events and acknowledge success. ADR#0035 catches this earlier:
a guarded command re-replays and no-ops on a key the aggregate has already seen,
so the reuse is refused at decide time and nothing is published.
`CONFLICT_KIND_IDENTITY_REUSED_WITH_DIFFERENT_CONTENT` reports that refusal, and
it reports it as a refusal, with nothing appended.

The division matters because the store cannot make this call. A correct retry
that re-enters the decider produces a different encoding of the same decision,
since events carry timestamps, so a store comparing bytes would flag every
command-layer retry as divergence. The aggregate compares keys against
decisions, which is the comparison that actually answers the question.

## The duplicate-detection window bounds where dedup happens, not whether

Duplicate detection lives in a per-subject table the server keeps for a
configured window. `DuplicateWindow` is documented in the substrate as an
operational capacity knob rather than a delivery guarantee, and that is the
honest description: it is bounded, and past it the store cannot tell a retry
from a first attempt.

Past it, dedup moves rather than disappearing. A guarded command's retry
re-replays and no-ops on its idempotency key at the aggregate. An unguarded fact
relies on readers collapsing identical ids. So the window is not the last line
of defense, and a retry that outlives it is not automatically a duplicate on the
log.

What the window does bound is who can answer. Past it, the store no longer can,
and `UNKNOWN_WRITE_REASON_DEDUPLICATION_WINDOW_EXPIRED` says exactly that: the
outcome is determinable, and not from here. Reporting it as an ordinary timeout
would point a caller at a retry the store can no longer make safe.

## The boundary matrix

Each row is a point where the process can stop. `state` is what the caller must
observe. Disposition is derived from the state rather than reported alongside
it, because two fields for one fact can disagree.

### Append

| Boundary | State | Retry may |
| --- | --- | --- |
| Rejected before publish (invalid header, invalid event) | `NOT_APPENDED` | retry the command |
| Head guard refused (`WrongExpectedVersion`) | `CONFLICT` / `WRONG_EXPECTED_HEAD` | re-read and decide again, never retry unchanged |
| Creation batch found a non-empty stream | `CONFLICT` / `STREAM_ALREADY_EXISTS` | nothing; the identity is taken |
| Subject belongs to a sealed incarnation | `CONFLICT` / `INCARNATION_RETIRED` | re-resolve the subject, never retry |
| Published, no acknowledgment observed | `UNKNOWN` / `ACK_TIMEOUT` | retry, which commits or deduplicates |
| Acknowledgment arrived unreadable | `UNKNOWN` / `ACK_UNREADABLE` | retry |
| Acknowledged | `COMMITTED` | nothing |
| Duplicate acknowledgment | `DEDUPLICATED` | nothing |
| Idempotency key already seen with a different decision | `CONFLICT` / `IDENTITY_REUSED_WITH_DIFFERENT_CONTENT` | nothing; refused at decide time, nothing published |
| Retry aged past the duplicate window | `UNKNOWN` / `DEDUPLICATION_WINDOW_EXPIRED` | resolve at the aggregate or by reading, not by republishing |

### Batch

Creation batches are atomic: `SessionStarted` with its plan and workspace,
`ForkSession`, delegated-child creation, `RecoverSession`. Each writes several
events under one `NoStream` guard.

| Boundary | State | Retry may |
| --- | --- | --- |
| First member refused by the guard | `CONFLICT` / `STREAM_ALREADY_EXISTS` | nothing |
| Some members acknowledged, some not | `UNKNOWN` / `BATCH_PARTIALLY_ACKNOWLEDGED` | read the stream, never retry the batch |
| Every member acknowledged | `COMMITTED` | nothing |
| Whole batch deduplicated | `DEDUPLICATED` | nothing |

The partial case is the one worth stating plainly: a stream holding half a
creation batch is a shape no fold expects, and retrying the batch cannot fix it,
because the `NoStream` guard now sees a non-empty stream. It resolves by reading
what is actually there.

### Side effects around the append

These boundaries are not about the store. They are about the order the platform
does things in, and each one has exactly one safe order.

| Boundary | Safe order | Why |
| --- | --- | --- |
| Artifact bytes and the event referencing them | bytes first, event second | an event pointing at bytes that were never written is unrepairable; bytes nobody references are an orphan the doctor can find |
| Event and projection update | event first, projection second | the event is the source of truth and the projection is rebuildable |
| Projector checkpoint and materialized write | write first, checkpoint second | a checkpoint ahead of its view claims work it did not do, which is `FINDING_KIND_PROJECTION_CHECKPOINT_INCONSISTENT`; a checkpoint behind its view replays, which is idempotent |
| Checkpoint artifact upload and `CheckpointProduced` | upload first, event second | same rule as artifacts, and restoration admits a checkpoint only against evidence that is actually readable |
| Migration staging and commit | intent first, target second | see [Session Maintenance](./session-maintenance.md); the intent is what makes a lost commit reconcilable rather than unknown |
| Redaction and dependent projections | event first, propagation second | a mask that propagates before it is durable can be lost, and a mask that is lost is a disclosure |

The asymmetry in every row is the same one: do the reversible thing first. An
orphaned artifact is findable and releasable. A dangling reference is neither.

## What the tests must assert

For each row above: inject the failure at that boundary, and assert the caller
observed exactly the listed state, then that the prescribed retry reaches a
terminal state without producing a second copy of any event.

Two assertions are worth calling out because they are the ones a passing suite
can most easily fake.

**No boundary produces best effort.** Every row terminates in `COMMITTED`,
`DEDUPLICATED`, `NOT_APPENDED`, `CONFLICT`, or `UNKNOWN`. A test that accepts an
untyped error at any boundary is asserting that the boundary is unspecified.

**`UNKNOWN` is asserted as unknown, not as either answer.** The temptation is to
let the test settle the ambiguity by inspecting the stream and asserting what
really happened. That tests the store and not the contract. What the caller was
told is the thing under test, because acting on the wrong half of an unknown is
the failure the whole page exists for.

## Layout

`proto/trogonai/session/sessions/v1alpha1/write_outcome.proto`: `WriteOutcome`,
`WriteState`, `IndeterminateWrite`, `UnknownWriteReason`, `WriteConflict`,
`ConflictKind`.

It sits with the commands rather than in a sibling subtree, because it is not an
operator surface or a read contract. It is the other half of every command: the
answer, in the same package and the same release cadence as the request.

## Status

Shipped: the outcome contract above, lint-clean, formatted, building, and
generating Rust bindings.

Not shipped: the tests, and everything they need. There is no Session runtime,
no command handler, and no NATS adapter for sessions, so there is nothing to
interrupt. Writing the suite against a mock written for the occasion would
assert that the mock behaves the way the mock was written.

Three implementation gaps are named above rather than left to be rediscovered.

`EventIdentity for SessionEvent` is the empty default, so the runtime assigns a
fresh UUIDv7 per publish and a retry today produces a different id. Duplicate
detection does nothing, which collapses four rows of the matrix into one
undetected duplicate.

A duplicate acknowledgment is surfaced as an error and its sequence is
discarded, so the one signal that resolves a lost acknowledgment is thrown away
at the point it arrives.

The command idempotency key has no structural home, which ADR#0035 leaves open
deliberately. Until it does, the derivation has no input and the aggregate's
seen-key horizon has nothing to key on, so both halves of the dedup story are
waiting on the same decision.

ADR#0035 already says this machinery is a substrate obligation rather than a
shipped guarantee. This page is what it has to satisfy.

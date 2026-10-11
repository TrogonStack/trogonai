---
number: "0069"
slug: event-envelope-records-issuer-and-first-recorded-time
status: draft
date: 2026-10-11
---

# ADR#0069: The Event Envelope Records Who Issued an Event and When It Was First Recorded

## Context

Contracts keep copying provenance into payloads: who issued the command, and
when its events were recorded. `ProjectCreated.creator`, the `*_by` actor
fields across `proto/trogonai/workflows`, and the host-stamped
`ScheduleOccurrenceRecorded.recorded_at` are each a payload copy of something
the event envelope should own. On a command, a caller-filled identity field is
worse than a copy, because it lets the caller claim to be someone else.

Removing those fields is only correct if the platform records the same facts
itself, and today it does not:

- The store-agnostic envelope in `trogon-decider-runtime` is
  `Event { id, type, content, headers }`, read back as `StreamEvent`, which
  adds `stream_id`, `stream_position`, and `recorded_at`.
- `CommandPrincipal { kind, id, claims }` is typed, but
  [ADR#0026](./0026-command-authorization-principal.md) makes it an
  authorization-time input that "is not required to be persisted verbatim",
  and leaves each application to derive its own header from it. Nothing in
  production derives one, so no event records who issued it.
- `StreamEvent.recorded_at` is documented as the store timestamp for the
  persisted event, without saying which write. The NATS adapter derives it at
  read time from the message's own store timestamp, so re-appending an event
  into another stream ([ADR#0013](./0013-origin-stream-sequence-header.md)),
  restoring it at the application level, or rebuilding a stream
  ([ADR#0059](./0059-session-stream-incarnation-fencing.md)) gives it the time
  of the new write. The original sequence is carried forward; the original
  time is not.
- `Headers` is an untyped string map that callers can fill, which
  [ADR#0026](./0026-command-authorization-principal.md) already rejected as a
  place for identity, because a verified value cannot be told from a forged
  one.

So the envelope abstraction exists, but it guarantees neither fact that would
let a contract leave provenance out of its payload.

## Decision

### 1. The envelope carries the issuing principal as a typed part

The stored event envelope gains a typed issuer: the kind and identifier of the
`CommandPrincipal` the execution ran under. The runtime sets it on every event
an execution appends, from the principal the execution was given, and from
nowhere else: never from `Headers`, never from the command payload, never from
`decide`. Claims are authorization inputs and are not recorded.

An execution without a principal appends events without an issuer, and that
absence is visible to readers rather than filled with a placeholder. Which
executions must have a principal is an admission decision, not an envelope
one.

This amends [ADR#0026](./0026-command-authorization-principal.md) Decision 1:
the principal is still evaluated before `decide` runs and still kept out of
`Headers`, but its kind and identifier are now persisted. The objection that
ADR raised against a header entry stands, and is why the issuer is a typed
part of the envelope instead.

### 2. `recorded_at` means first recorded, and survives every move

`recorded_at` is the time the event was first appended to any stream. Every
store adapter preserves it across re-append into another stream, application
restore, replication, and stream rebuild, the same way the origin sequence is
preserved. An adapter whose store timestamps each write must carry the
original time forward itself.

### 3. Readers get both from the envelope API

`StreamEvent` exposes the issuer and `recorded_at` to every consumer:
projections, processors, replay, and any read API built on them. No consumer
reads adapter-specific metadata to recover either one.

### 4. How an adapter stores them is the adapter's concern

This decision fixes the logical envelope, not its encoding. Each store adapter
chooses its own physical representation and documents it; no contract, ADR,
or consumer depends on that choice.

### 5. Payloads keep domain facts, not provenance

With these guarantees in place, a command or event does not carry the issuing
principal or its own recorded time. Two kinds of value still belong in the
payload, because they are domain facts rather than provenance:

- An actor who is not the issuer, such as an approver named in a command a
  system process issues on their behalf.
- A domain time that differs from recorded time, such as when a scheduled
  occurrence was due or when an external system says something happened.
  [ADR#0035](./0035-session-store-decider-aggregate.md)'s event-time policy
  already draws this line; this decision supplies the envelope half it
  assumes.

A domain time is named for what it means, never with a generic name a reader
would mistake for recorded time.

## Consequences

- Contracts can drop payload provenance only once the runtime, every store
  adapter, and the decider host satisfy Decisions 1 to 3. Until then a payload
  field that a contract already has stays, and any removal waits for this
  decision to be implemented.
- The WASM decider host has no principal to record until ingress delivers one;
  [ADR#0026](./0026-command-authorization-principal.md) already names that
  ingress as missing, and this decision raises its priority.
- The in-memory adapter, which stamps `recorded_at` at append, already matches
  Decision 2 for a single stream; the NATS adapter does not, and needs to
  carry the original time on any re-append.
- Signed provenance ([ADR#0039](./0039-self-authenticating-event-provenance.md))
  builds on the recorded issuer rather than replacing it: a signature proves
  the issuer, the issuer field says who it is.
- Causation and tenancy remain named categories of metadata without a typed
  envelope part; each needs its own decision.

## Alternatives Considered

### Keep provenance in each contract's payload

Rejected. Every contract reinvents the same fields under different names, a
command field lets the caller choose its own identity, and a host-stamped copy
of recorded time drifts from the store's own record.

### A conventional header name for the principal

Rejected for the reason [ADR#0026](./0026-command-authorization-principal.md)
gives: `Headers` cannot tell a value the runtime set from one a caller typed.
It would also tie the guarantee to whichever store encodes headers that way.

### Use each store's native timestamp as recorded time

Rejected. A store timestamp is the time of that particular write, so it
changes whenever an event moves, which is exactly when provenance matters
most.

## References

- [Event Metadata](../architecture/event-metadata.md)
- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- [ADR#0059: Stream Incarnation Fencing by Subject Isolation and Sealing](./0059-session-stream-incarnation-fencing.md)

---
number: "0069"
slug: event-envelope-records-issuer-and-times
status: draft
date: 2026-10-11
---

# ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened

## Context

Every event answers two questions besides its domain fact: who caused it, and
when. Contracts keep answering them in their own payloads, each in its own
way: a `creator` here, a `requested_by` or `cancelled_by` there, a
`recorded_at` copied in by whoever built the request. The same fact ends up
modeled once per contract, under different names, present on some events and
missing on others. When the payload field is filled by the requester, the
record says whoever the requester claimed to be.

These answers are not domain facts. They are the same for every event in the
system, so they belong to the platform that carries events, not to each
contract that defines one. The platform already wraps every event in an
envelope beside its payload, but that envelope does not yet promise either
answer: today it records no actor at all, and its only time is the store's
timestamp for the latest write, which changes whenever an event is copied,
restored, or moved.

## Decision

### 1. Provenance is platform metadata on every event

The platform attaches provenance to the envelope of every event it carries,
for every contract, without the contract asking. No contract models it, and
no contract can leave it out. A rule that holds for one event type and not
another is not a guarantee anyone can build on, so provenance is all or
nothing across the system.

### 2. Every event records the actor that caused it

The envelope names the actor that caused the event: its kind (person, agent,
service) and its stable identifier. A person or agent acting directly is an
actor; so is a scheduler, processor, or other system process acting on its
own, under an identity of its own. There is no event without an actor.

The actor comes from the identity the platform authenticated, never from
anything the requester wrote. A value the requester can set is a claim, not a
record.

### 3. Every event records a time for each step that observed it

Being written to storage is the last moment in an event's life, not the only
one worth knowing. The envelope records each step separately:

- `issued_at`: when the platform accepted the request that led to the event.
- `produced_at`: when the event was produced from that request.
- `recorded_at`: when the event was first written to storage.

The envelope also carries the occurrence time: when the producer says the
event happened. It differs from the other times for a device that synced hours
later, an import of historical facts, or an outside system reporting its own
timestamp. Every event has one, so it is event context, recorded as the
producer's claim and never confused with a time the platform observed.

The gaps between the platform's times are diagnostics in their own right: waiting between
acceptance and production, retries and contention between production and
storage. Each time is stamped by the part of the platform that observed that
step, never by the requester and never copied from the payload.

### 4. Provenance survives everything that moves an event

Copying an event into another stream, restoring it from a backup or archive,
replicating it, or rebuilding a stream keeps its original actor and times. A
storage system that timestamps each write on its own does not get to redefine
when an event was first recorded; the platform carries the original values
forward.

### 5. Readers get provenance from the envelope, in one shape

Every consumer reads the actor and times the same way, from the envelope,
whatever stored the event. No consumer reaches into storage-specific metadata
to recover them, and how a given storage system encodes them is its own
concern, documented alongside it.

Rebuilding state from events is a reader too. A rule that depends on who acted
before, such as only the author may withdraw or a reviewer must differ from the
author, reads that actor from the envelopes of the earlier events, not from a
copy in their payloads. A reader that could see only payloads would force every
such rule to copy provenance back into them, which is the duplication this
decision removes.

### 6. Payloads keep domain facts, not provenance

With provenance on every envelope, a payload does not repeat who caused the
event or when it was recorded. Two kinds of value still belong in the payload,
because they are domain facts rather than provenance:

- An actor other than the one that caused the event, such as an approver
  named in a request a system process makes on their behalf.
- A time that is part of the business fact, such as when a scheduled
  occurrence was due, when a change takes effect, or when a measurement was
  taken.

A domain time is named for what it means (`due_at`, `effective_at`,
`measured_at`), never with a generic name a reader would mistake for an
envelope or occurrence time.

## Consequences

- A contract can leave provenance out of its payload only once the platform
  meets these guarantees. Until then, removing such a field loses the only
  record of it, so removals wait for the platform.
- Every system process that produces events needs an identity of its own.
- Where each envelope value lives, and how each storage system encodes it, is
  documented in [Event Metadata](../architecture/event-metadata.md), because
  it is specific to this platform and is the first thing a contract author
  needs before leaving provenance out of a payload.
- Today the platform records no actor and only a latest-write timestamp, so
  meeting this decision is platform work, tracked against this ADR.
- Signed provenance ([ADR#0039](./0039-self-authenticating-event-provenance.md))
  builds on the recorded actor rather than replacing it: a signature proves
  the actor, the envelope says who it is.
- [ADR#0035](./0035-session-store-decider-aggregate.md)'s event-time policy
  keeps occurrence timestamps in a few payloads because the envelope has no
  occurrence time today. Once it does, each of those is either a domain time
  under its own name or moves to the envelope.
- Causation and tenancy are also cross-cutting metadata, and each needs its
  own decision.

## Alternatives Considered

### Provenance in each contract's payload

Rejected. Every contract reinvents the same fields under different names,
some events end up with them and some without, and a field the requester fills
in records a claim rather than a fact.

### A conventional metadata entry that contracts opt into

Rejected. Opt-in metadata is present only where someone remembered it, and a
free-form entry cannot tell a value the platform set from one a requester
supplied ([ADR#0026](./0026-command-authorization-principal.md) rejects it
for the same reason).

### The storage system's own timestamp as the only time

Rejected. It is the time of one particular write, it changes whenever an event
moves, and it says nothing about when the request was accepted or the event
produced.

## References

- [Event Metadata](../architecture/event-metadata.md)
- [Actor and Authority Taxonomy for Managed Systems](https://github.com/straw-hat-team/adr/blob/main/src/adrs/8779742261/README.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)

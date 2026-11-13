---
title: Sessions
aside: false
pageClass: wide-page
product:
  name: Sessions
  tagline: Durable, replayable history of everything an agent did.
  order: 4
  status: planned
  icon: history
  color: "#F59E0B"
guarantees:
  - icon: history
    title: Nothing is rewritten
    text: Events are never edited or purged. Hiding a session deletes nothing from its log.
  - icon: circle-help
    title: Honest about unknowns
    text: A step the session cannot confirm is recorded as unknown, never guessed as failed.
  - icon: copy
    title: Safe to retry
    text: A repeated command or write is recognized, so a retry never records an event twice.
  - icon: clock
    title: Fresh, or it says so
    text: Every read reports how current it is. A read that must see your write gets it or a clear error.
  - icon: fingerprint
    title: Artifacts are checked
    text: Readers check artifact bytes against the digest in the log. When verification is required, a mismatch is refused, not warned about.
  - icon: triangle-alert
    title: One bad call stays one
    text: A malformed tool call from a provider is set aside without throwing out the good calls next to it.
  - icon: stethoscope
    title: Repairs never touch history
    text: The session doctor finds and fixes problems in views and indexes. No repair touches the event stream.
progress:
  - name: Event-sourcing runtime
    status: built
    note: Deciders, snapshots, projections and their telemetry, shared across the platform. Sessions will run on it.
  - name: Session event, command and state contracts
    status: partial
    note: Protobuf schemas define every event, command and state a session can have.
  - name: Query, artifact, doctor and maintenance contracts
    status: partial
    note: The wire contracts for reading and operating sessions are defined in protobuf.
  - name: Session log and decider on NATS JetStream
    status: designed
    note: One stream per session, with guarded appends and duplicate detection.
  - name: Read models and the query service
    status: designed
    note: Views folded from the log, with freshness on every answer and reads of your own writes.
  - name: Crash recovery and safe retries
    status: designed
    note: Reconciling interrupted tool calls, and a typed outcome for every write.
  - name: Artifacts, structured diffs and terminal replay
    status: designed
    note: Large output stored outside the log and verified against the digest the log records.
  - name: Doctor and maintenance
    status: designed
    note: Health reports, view repair, schema migration and salvage of damaged sessions.
---

<ProductHeader />

A session is the full record of an agent's work: every message, tool call, file
change and artifact, in the order it happened. The record is the source of
truth, so a session can survive a crash, be resumed or forked, and be replayed
exactly as it happened.

::: info Designed, not built yet
The session contracts are defined. The service that records and serves sessions
is not built yet. [Where it stands](#where-it-stands) has the detail.
:::

## Everything the agent did, in order

Each step becomes an event in the session's log. Large output such as diffs and
terminal logs is kept alongside, so the log stays small and replay stays exact.

<SessionTape />

## One log, every view

Writes pass through one gate and land in one log. Every screen and query reads a
view built from that log, so a view can always be rebuilt and never overrules
it.

<SessionPipeline />

## Built on events

Every change to a session is an event, published on NATS. That one decision is
what makes sessions easy to extend, watch and change: anything that needs to
know about a session follows its events instead of calling the service that
wrote them.

<SessionEventDriven />

## Honest after a crash

Runners crash, networks drop and retries happen. A session records what it knows
and nothing more, so recovery starts from the truth.

<SessionRecovery />

## Start, fork, delegate, recover

Sessions branch the way work does, and every branch keeps its own history.

<SessionLineage />

## What we promise

<FeatureGrid :items="$frontmatter.guarantees" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

## Learn more

**Understand the design**

- [Session Aggregate](/architecture/session-aggregate)
- [Session Crash Boundaries](/architecture/session-crash-boundaries)
- [Session Tool Effects](/architecture/session-tool-effects)
- [Session Provider Faults](/architecture/session-provider-faults)
- [Session Schema Boundaries](/architecture/session-schema-boundaries)

**Reading sessions**

- [Session Query Contract](/architecture/session-queries)
- [Session Projection Freshness](/architecture/session-projection-freshness)
- [Session Pagination](/architecture/session-pagination)
- [Session Resume Index](/architecture/session-resume-index)
- [Session Title and Preview](/architecture/session-title-and-preview)
- [Session Presentation Caches](/architecture/session-presentation-cache)

**Artifacts and replay**

- [Session Artifacts](/architecture/session-artifacts)
- [Session Structured Diff](/architecture/session-structured-diff)
- [Session Terminal Replay](/architecture/session-terminal-replay)
- [Session Detached Work](/architecture/session-detached-work)

**Operating sessions**

- [Session Doctor](/architecture/session-doctor)
- [Session Maintenance](/architecture/session-maintenance)

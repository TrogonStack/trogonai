---
number: "0035"
slug: session-store-decider-aggregate
status: draft
date: 2026-07-23
---

# ADR#0035: Session Store as a Decider Aggregate on NATS JetStream

## Context

A platform [Session](../glossary/session) ([ADR#0031](./0031-agent-implementation-and-session-plan.md))
is one execution of a pinned agent revision. It durably owns its identity and
[SessionExecutionPlan](../glossary/sessionexecutionplan), an append-only
[ExecutionAttempt](../glossary/executionattempt) fact sequence, the transcript
and output, authorized tool and delegation dispatch, cancellation intent and
terminal outcome, the parent-child collaboration graph, and a durable operation
ledger that makes tool and delegation side effects idempotent (deduplicated by
operation id and digest, with outcome reconciliation for indeterminate results).
Nothing on the curated line persists any of this: before this decision the
[ADR index](./index.md) ran `0000`-`0034` with no session-store record, and
[ADR#0024](./0024-agent-platform-stream-topology.md) deliberately left "sessions
each keep their own streams" and the physical [JetStream](../glossary/jetstream)
topology "for a separate decision before infrastructure provisioning." This is
that decision.

The [session-store research corpus](../research/session-store/index.md) studied
nine agent products and distilled a
[working definition](../research/session-store/synthesis.md) -- "a stored session
is an ordered, addressable record of everything that happened in one agent run,
durable enough to survive a crash, complete enough that the model-visible
context and every read model can be rebuilt from it alone" -- plus nine forced
design decisions and two gaps the whole industry left open: subagent cascade
semantics and retention on an unbounded log. That synthesis is decision-time
input; where it differs from an accepted record, the ADR is authoritative.

The substrate already fits the corpus's conclusion. `trogon-decider` and
`trogon-decider-nats` give a [decider](../glossary/decider) aggregate a
subject-per-logical-[stream](../glossary/stream) topology via a caller-supplied
`StreamSubjectResolver`, an atomic multi-[event](../glossary/event) append, KV
[snapshots](../glossary/snapshot), subject-filtered replay
([projections](../glossary/projection)), and durable
[processors](../glossary/processor). Two facts are load-bearing here, and are
implemented in the substrate today:

- **Real optimistic concurrency already ships, by default.** When a command
  declares no [`WRITE_PRECONDITION`](../glossary/write-precondition), the runtime
  resolves the append guard to `At(current_position)`
  (`trogon-decider-runtime::execution` via `Option<StreamPosition>::into`), which
  `append_stream` turns into a `Nats-Expected-Last-Subject-Sequence` header and
  raises a typed `WrongExpectedVersion` on conflict
  (`trogon-decider-nats::stream_store`). Only `NoStream` (guard `0`) and
  `At(position)` are enforced server-side; `Any` and `StreamExists` send no wire
  guard. So the corpus's forced decision #3 -- a caller-supplied expected-version
  precondition on every append, which the study found only OpenCode has -- is
  already satisfied on this substrate for free, unless an aggregate opts out.
- **Physical sequence is the order of record.**
  [ADR#0013](./0013-origin-stream-sequence-header.md) makes the current JetStream
  stream sequence authoritative for checkpoints, high-water marks, and optimistic
  concurrency, and confines `Trogon-Origin-Stream-Sequence` to provenance on
  restore/backfill/migration/rebuild only.

Prior art exists but on the wrong mechanism. The `origin/platform` branch carries
a mature session store (`crates/session`, ~22k lines, 213 tests: a 46-arm
`SessionEventPayload`, a `{PREFIX}_SESSION_EVENTS` stream with a
`sessions.{id}.events` subject per session, an artifact claim-check by sha256, a
deterministic context-twin prompt compiler, and a `SessionBranched` fork). Its
domain model is the best available input, but it hand-rolls persistence directly
on JetStream/KV: `append_event` self-assigns the next sequence by reading the
whole subject back and guards concurrent writers with an application-level KV
lease, not JetStream's native expected-sequence guard -- a real lost-update
hazard the substrate on the curated line already closes. This ADR **ratifies that
domain model and deliberately supersedes its persistence mechanism**.

Several primitives this decision leans on are themselves `draft`, not accepted,
and some are not yet implemented in the substrate: [ADR#0026](./0026-command-authorization-principal.md)'s `CommandPrincipal`/
`CommandAuthorizer`, [ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s `Tenant`/`TenantBinding`, [ADR#0028](./0028-decider-admission-control-and-backpressure.md)'s admission
limiter, [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)'s snapshot-derived
[retention watermark](../glossary/retention-watermark), and [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s Session
model. Where this ADR names those types it is naming proposed primitives it
depends on, not shipped code, and decisions that build on them are provisional to
that extent. The two facts marked "already ships, by default" above
(expected-sequence OCC and physical-sequence order) are the exception -- those are
implemented in `trogon-decider`/`trogon-decider-nats` today.

## Decision

### 1. A Session is a decider aggregate: one logical stream per Session, one subject on a shared physical stream

Each Session, each subagent, and each fork is its own logical stream -- its own
subject on a shared physical JetStream stream `SESSION_EVENTS` -- never events
inlined into another stream. This follows [ADR#0024](./0024-agent-platform-stream-topology.md)'s placement rule (a fact
belongs in a stream only when its order is load-bearing for that stream's
invariants) and its single-write-once-per-fact rule (cross-stream questions are
answered by projections, never by mirrored writes).

A `StreamSubjectResolver` maps the opaque `SessionId` to the subject
`session.sessions.events.<session_id>` (pattern `session.sessions.events.>`),
mirroring the one production precedent, `trogon-scheduler`'s
`scheduler.schedules.events.<key>`. `SessionId` is opaque and time-sortable by
construction (the prior art's `sess_`-prefixed id is ratified) but its sort order
is never load-bearing (see facet 2); a session id that is not subject-token-safe
is mapped through a routing-key transform, as the scheduler does. This corrects
the prior art's id-in-the-middle `sessions.{id}.events` to the trailing-token
form the resolver and subject-filtered projector expect.

Multi-[tenant](../glossary/tenant) scoping is expected to use draft
[ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s proposed `Tenant`: the
resolver and snapshot-key surface would take a `Tenant`, the store would validate
the resolved subject against it, and a tenant needing hard isolation would opt
into `TenantBinding::Dedicated`. That type does not exist in the substrate yet;
until it lands, a session is scoped by its subject alone. Proto lives under
`proto/trogonai/session/sessions/v1/` (domain `session`, aggregate `sessions`),
per [ADR#0009](./0009-protocol-buffers-wire-contracts.md).

### 2. Append-only mutation, opaque identity, physical-sequence order, and per-command optimistic concurrency

Append is the only mutation primitive. `decide` returns only new events, `evolve`
is the only place state changes, and `append_stream` is the only write path.
Every retroactive operation -- rewind, revert, compaction, delete, cancel -- is a
**new appended event interpreted at replay**, never an edit or a delete of stored
messages. No command ever purges or trims the stream; the log is keep-forever
(facet 7), and physical bytes only ever relocate between storage tiers, never leave
the record. This
is forced decision #1, and it is what separates this store from the corpus's
cautionary cases (Goose's `DELETE`+re-`INSERT`, Hermes's flag flips).

Identity and order are separate concerns (forced decision #2). The `SessionId` is
an opaque addressing key only; the sole monotonic order is the physical JetStream
stream sequence ([ADR#0013](./0013-origin-stream-sequence-header.md)). The prior art's application-assigned per-session
`Seq` is dropped -- it is exactly what forced the read-last-sequence-then-lease
anti-pattern, and JetStream already assigns the authoritative sequence.

Optimistic concurrency is applied per command by whether `decide` branches on the
current head, not uniformly. The rule: guard an append with the head sequence only
when a stale decision would violate an invariant; otherwise append unconditionally.

- **Commuting happened-facts** -- `UserMessageRecorded`,
  `AssistantMessageStarted`/`Completed`, the `ToolCall*` events, `ArtifactRecorded`,
  `FileChanged` -- declare `WRITE_PRECONDITION = Any`. They record that something
  happened; `decide` does not branch on the exact head, a racing user message and
  assistant message are both valid, and their order is arrival order. Appends never
  overwrite, so there is no lost update to prevent -- a head guard here would only
  force a wasted retry (re-replay, re-decide) on the highest-volume path. `Any`
  appends to the tail with no server-side guard and never retries; its appends are
  therefore at-least-once, with duplicates deduped by readers (see below), not
  prevented at write.
- **Invariant-bearing transitions** -- the commands recording `SessionClosed`/
  `SessionCancelled`/`SessionFailed`/`SessionDeleted`, `SessionRewound`, `Compacted`,
  `DelegationDispatched`, and `SubagentDetached` (named here by the fact each records;
  the command is the verb form, e.g. `CloseSession` records `SessionClosed`,
  `DispatchDelegation` records `DelegationDispatched`) -- declare no
  `WRITE_PRECONDITION`, so the runtime applies the default `At(current_position)`: a
  command that read state at sequence N fails if any writer appended since (the
  low-level `stream_store` `WrongExpectedVersion`, surfaced to callers as
  `JetStreamStoreError::OptimisticConcurrencyConflict`, then `CommandError::Append`,
  then retried against the new head). Here `decide` genuinely branches on the head --
  reject a second close, refuse to spawn under a terminal parent (facet 6), validate
  a rewind target, admit one active compaction -- so a stale decision must be
  rejected, not appended.
- **Creation** -- `CreateSession` and a fork's first event (`SessionForked`, facet
  5) declare `WRITE_PRECONDITION = NoStream` (guard `0`), making creation atomic and
  exactly-once. `StreamExists` is never used, because it sends no server-side guard.

This refines the corpus's forced decision #3 (a precondition on *every* append) to
a precondition on every *invariant-bearing* append, on the reasoning that commuting
facts have no stale-decision failure mode to catch. It still supersedes the prior
art's pessimistic KV lease, which serialized *all* writes: here only the
invariant-bearing transitions coordinate, and via OCC rather than a lock. Uniform
strict OCC and no-OCC-anywhere are both weighed and rejected under Alternatives.

**Concurrency and turn-taking.** Because the high-volume path uses `Any`, an agent
and a user appending at the same time both land in arrival order with no conflict
and no retry -- the common "user sends a message while the agent is mid-turn" case
needs no lock. The store always records a user message in arrival order -- append-only
leaves no other option, and that is the faithful record of what happened. Whether
that message *interrupts* the open agent turn or waits for the next turn boundary is
an agent-loop decision expressed as a following command (for example a turn-cancel),
never something the store enforces or reorders. Streamed
assistant-token deltas are delivered over ephemeral core-NATS pub/sub for the live
view and are not appended per token; the durable log records coarse facts
(`AssistantMessageStarted`, then `AssistantMessageCompleted` with the full
message), so per-session durable write volume -- and thus contention on the guarded
lifecycle path -- stays low.

Multi-writer and multi-host correctness (forced decision #8) follow from the same
substrate: the log lives on the NATS cluster, any replica may append, the guarded
commands detect and retry conflicts, and the `Any` commands need no coordination at
all. There is no shared-filesystem or single-writer assumption to violate. An
advisory per-session command inbox (a single durable consumer applying a session's
commands in arrival order) would remove even the lifecycle-path retries, but it is
an optional latency optimization, not a correctness requirement, and is a Non-Goal
here.

**At-least-once delivery and idempotency.** Commands are processed at-least-once --
a NATS processor redelivers a command after a crash before its ack. On the `Any` path
this makes an append **at-least-once, not exactly-once**: two concurrent deliveries
sharing an idempotency key can both replay state before either append is visible, both
conclude the key is new, and both append. A `decide`-time key check is only a cheap
best-effort filter, never atomic, precisely because `Any` sends no server-side guard.
So every event carries an idempotency key and **readers dedup by it** -- the fold and
every projection collapse duplicate keys, the same way they already tolerate
post-terminal facts (see Consequences). A command that needs exactly-once *append*
must instead take the guarded (`At(current_position)`) path, where the sequence guard
makes check-and-append atomic: the losing delivery gets `WrongExpectedVersion`,
re-replays, sees the key, and no-ops. Exactly-once external *side effects* (tool,
delegation) are handled separately by [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s
operation ledger, which reserves the operation id atomically before the effect.

### 3. Every event is a typed protobuf, schema-validated at the storage boundary

Session events are protobuf messages under `proto/trogonai/session/sessions/v1/`
(`edition = "2024"`, structurally-required fields tagged
`[features.field_presence = LEGACY_REQUIRED]`), following the scheduler
precedent: present-tense `VerbNoun` commands (`CreateSession`, `ForkSession`,
`RewindSession`, `DeleteSession`, `DispatchDelegation`), past-participle
`NounVerbed` events (`SessionStarted`, `SessionForked`, `SessionRewound`,
`Compacted`, `ToolCallCompleted`), one `SessionEvent` oneof envelope importing
every event file, and `state`/`projections`/`checkpoints` sibling subtrees whose
read-model value types are redefined locally to decouple their evolution from the
write side.

Every event is decoded and validated through its generated codec on both append
and replay (forced decision #9,
[ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md)): a malformed
payload is rejected at the boundary and never reaches durable storage. This ADR
adds an observable decode-failure metric for session events -- the decider crate
emits no such metric today and its append/replay decode-error paths are currently
silent -- following [ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md)'s principle that boundary decode failures must be
measured, not dropped. Unlike the prior art, no `InvalidEventRejected` event is
persisted: rejection happens before the write, so there is nothing to record.
Schema evolution is additive (new optional fields, reserved retired numbers),
never a per-event version branch. Every event carries a small envelope alongside
its typed payload -- an event id (which doubles as the append idempotency key of
facet 2), an append timestamp, and the acting principal ([ADR#0026](./0026-command-authorization-principal.md)) -- so dedup,
audit, and authorization are answerable from the event itself.

The prior art's 46-arm payload is ratified as the starting vocabulary checklist,
not copied verbatim: lifecycle (`SessionStarted`, which stores the
`StoredSessionExecutionPlan` once per [ADR#0031](./0031-agent-implementation-and-session-plan.md); `SessionClosed`,
`SessionCancelled`, `SessionFailed`, `SessionDeleted`), the ExecutionAttempt
facts ([ADR#0031](./0031-agent-implementation-and-session-plan.md)), conversation (`UserMessageRecorded`,
`AssistantMessageStarted`/`Completed`), tool lifecycle
(`ToolCallRequested`/`Approved`/`Started`/`Completed`/`Failed`), artifacts
(`ArtifactRecorded`, claim-check by sha256), file changes, compaction
(`Compacted`, facet 4), rewind (`SessionRewound`), fork (`SessionForked`, facet
5), delegation and subagents (facet 6), and the operation-ledger reservations
that make tool and delegation side effects idempotent -- [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s ledger
deduplicates by operation id and digest and reconciles indeterminate outcomes; it
is not exactly-once execution. Every command is gated before `decide` by the
proposed `CommandPrincipal`/`CommandAuthorizer` of draft
[ADR#0026](./0026-command-authorization-principal.md), once those land.

### 4. Compaction is a self-sufficient in-stream marker the store only records

Compaction is an upstream agent-loop concern (forced decision #4): the store
neither triggers nor understands it. The loop decides when the transcript nears
the context window and how to summarize; the store persists a single `Compacted`
event carrying the summary content inline plus the kept-tail boundary
(`from_sequence`, `to_sequence`), ratifying the prior art's `SummaryCreated`
shape. The model-visible view folds from the newest `Compacted` summary and every
event after it; the full history below the boundary remains on the stream for
audit and rewind.

The marker is self-sufficient: no out-of-band sidecar is required to replay
across a compaction boundary. This resolves the corpus's open sub-question in
favor of the in-stream marker (Claude Agent SDK, Codex CLI) over Grok Build's
fail-closed external `compaction_checkpoints/{id}.json`, keeping one recovery
story instead of a second artifact that can go missing. It also corrects the
platform compactor crate, which overwrote the stored message list wholesale -- the
exact destructive pattern the corpus warns against.

### 5. Fork is a shared-prefix reference composed at replay time

Fork mints a new logical stream and never lets the copy reference the source id
(forced decision #5), but it does not physically copy history. `ForkSession`
appends `SessionForked{session_id, source_session_id, history_base_seq,
forked_at}` as the first event on the child subject under the `NoStream` guard,
so the fork point is atomic and exactly-once even under retries. `history_base_seq`
is resolved at the application boundary (as principal and tenant already are) by
reading the source subject's current position just before the command; because
source sequences only grow, a race captures a marginally older or newer prefix,
never a corrupt one.

Replay composition lives in the store's history-load path, transparent to the
`Decider` trait's single-`StreamId` contract. The positions here are physical
JetStream stream sequences -- sparse and shared across every subject on
`SESSION_EVENTS`, not per-subject ordinals -- so the composition is expressed in
those positions, not small integers: if a child's first event is `SessionForked`,
the store folds the source-subject events up to and including `history_base_seq`,
then the child subject in full -- its first event, the `SessionForked` marker, folds
into `state.forked_from`, and the child's own events follow -- and evolves the
concatenation; otherwise it does an ordinary subject read. The marker is folded, not
skipped: reading the child from strictly after it would drop the fork metadata. This composes recursively for fork-of-fork chains.
Immediately after the `SessionForked` append succeeds, the fork handler folds the
source prefix **and the marker itself** into one sealing
[snapshot](../glossary/snapshot) at the marker's returned `StreamPosition`, so the
snapshot already carries `forked_from`; steady-state replay resumes from it and reads
only the child events after the marker (`ReadFrom::after` the snapshot position),
never re-reading the source and never dropping the fork metadata, bounding cost to
O(1) regardless of fork depth. A crash between the marker append and the snapshot write self-heals: the
next execution finds the marker without a snapshot, falls back to the full
two-segment replay, and writes the missing snapshot. This is the corpus's
converged-on cheaper alternative (Codex CLI's `history_base`), available because
events are immutable, and it needs no `Trogon-Origin-Stream-Sequence` -- a fork
mints a new aggregate, it does not migrate an existing one, so [ADR#0013](./0013-origin-stream-sequence-header.md)'s closed
use-case list is neither stretched nor needed.

### 6. Subagents are sibling streams with an explicit cascade policy, reconciled by a process manager

A subagent is its own logical stream linked by two distinct facts (never a
cross-stream transaction, which `decide` cannot express): the parent records
`DelegationDispatched{operation_id, child_session_id, dispatched_at_sequence,
cascade_policy}` (reusing [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s operation-ledger id to dedupe dispatch), and
the child records `SubagentLinked{parent_session_id, parent_sequence_at_dispatch,
cascade_policy}` -- `parent_sequence_at_dispatch` is a plain domain field copied
from the parent's physical sequence, never `Trogon-Origin-Stream-Sequence`. The
`cascade_policy` is `CASCADE_ON_PARENT_TERMINAL` (the safe default) or
`INDEPENDENT` (an intentional, recorded orphan) -- closing the industry-wide
silent-orphan gap (forced decision #6) by making both outcomes explicit facts.

A parent reaches a terminal state through a Session-level event on its own
subject: `SessionClosed` (normal completion), `SessionCancelled`, `SessionFailed`,
or `SessionDeleted`, plus the partial-invalidation case `SessionRewound`. Crash
is not itself a trigger: an `ExecutionAttemptEnded` is per-attempt ([ADR#0031](./0031-agent-implementation-and-session-plan.md)'s
outcome set is `failed | cancelled | terminated`, and a restart may still
follow), so a liveness watchdog that concludes no further attempt will run records
a Session-level `SessionFailed`, and that is what cascades. A reconciler
[processor](../glossary/processor) subscribed to `session.sessions.events.>`
matches those Session-level terminal markers and dispatches
`ReconcileParentTerminal` to each eligible child -- discovered through the
parent-to-children lineage projection folded from `DelegationDispatched` -- whose
`decide` emits one atomic `[SubagentParentTerminated, SessionCancelled]` batch on
the child's own subject, or a typed no-op error if already terminal. On a
`SessionRewound` trigger only the children whose `parent_sequence_at_dispatch` is
at or after the rewound-to sequence are cascade-cancelled; children dispatched
from still-valid prior history are left untouched.

Cascade is transitive because `SessionCancelled` is itself a terminal marker the
same reconciler reacts to; this is not free, though: a chain of depth D takes D
sequential reconciler round-trips, each bounded by the processor's redelivery
policy, and discovery depends on the lineage projection's freshness.
`DispatchDelegation` refuses to spawn under an already-terminal parent, closing
the common ordering of the race inside one stream's OCC (it is one of facet 2's
`At(current_position)`-guarded transitions, which is what makes the refusal race-safe);
`DetachSubagent`/`SubagentDetached` (mirrored on both streams, each side needing
it for its own invariant) records an intentional detach. Crash and downtime are
covered by bounded processor redelivery plus a scheduled orphan-closure sweep
that reads already-durable projections and snapshots -- never raw events, so it is
immune to any retention race and needs no extension to [ADR#0029](./0029-decider-retention-and-truncation-watermark.md). This sweep
closes logical cascade only; it never purges bytes.

### 7. The log is never truncated: keep-forever, with snapshot-bounded replay

An append-only session log is the immutable source of truth, so it is never
truncated or purged (forced decision #7): no event ever leaves the log. This is
"never edit an event" taken to its end -- deletion is just a slower edit -- and it
matches the two purest event-sourced products in the corpus (T3 Code, OpenCode),
which also keep history unbounded.

Storage is managed without ever removing a fact:

- **Snapshots bound replay, not storage.** The runtime resumes from the newest
  snapshot and replays only the tail after it (facet 8), so a long session costs
  O(tail) to load even though its log grows forever. [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)'s
  `MinimumRequiredSequence` watermark is kept only as a read-only diagnostic; the
  session store never issues the purge [ADR#0029](./0029-decider-retention-and-truncation-watermark.md) permits.
- **Cold-storage tiering is an optional, reversible, non-semantic relocation.** If a
  deployment must bound the hot JetStream stream, already-immutable old events may be
  copied to the JetStream Object Store, evicted from the hot stream, and restored on
  demand through [ADR#0013](./0013-origin-stream-sequence-header.md)'s restore path (the one authorized use of
  `Trogon-Origin-Stream-Sequence`). This moves bytes between tiers; it never edits or
  logically deletes an event, and whether to enable it is deferred to deployment, not
  decided here.

Because nothing is ever logically removed, the machinery an archive-then-purge
design would need -- a verified-archive KV ledger, a fork-lineage watermark term so a
live child's parent prefix is not purged, and the purge-versus-new-fork race -- simply
does not arise. A fork's parent prefix is always present.

### 8. Listing, search, and summaries are rebuildable projections, never the source of truth

No read model is authoritative. A `Projector::catch_up` folds the stream into a
`SessionProjection` (`projections/v1`) that denormalizes the fields a picker
needs, checkpointing `last_applied_stream_position` after each event exactly as
the scheduler does. Queries are `verb + noun` Rust functions over the KV
projection ([ADR#0014](./0014-command-and-query-naming.md)) -- `get_session`,
`list_sessions` -- with no query protos, since the projection value is the read
contract. The model-visible context is compiled deterministically from the event
log bounded by the latest `Compacted` marker (facet 4), ratifying the prior art's
context-twin/token-budget compiler as a projection. Any full-text or vector search
subsystem is a separate, independently bootstrapped projection off the same log,
out of scope here.

Resume rebuilds a session's state the way the runtime rebuilds any decider
aggregate: load the newest snapshot for the session, then replay only the tail after
it (a fork resumes from its sealing snapshot, facet 5), so resume cost tracks
snapshot cadence, not transcript length.

## Alternatives Considered

### Reuse the platform `crates/session` persistence as-is

Rejected. Its `append_event` self-assigns the next sequence by reading the subject
back and serializes writers with an application-level KV lease, with no
server-side compare-and-swap. That is a lost-update hazard the substrate closes
with `expected_last_subject_sequence`; the domain model is salvaged, the
mechanism is not.

### Fork as a physical O(history) copy

Rejected. Copying the source events under a new subject either duplicates the same
facts under two subjects (violating [ADR#0024](./0024-agent-platform-stream-topology.md)'s single-write-once-per-fact rule) or
tags them with `Trogon-Origin-Stream-Sequence` to fake provenance for a case
[ADR#0013](./0013-origin-stream-sequence-header.md) does not authorize (a fork is a new aggregate, not a migration). It also
pays O(history) cost per fork and needs crash-resumable copy machinery that does
not exist in `trogon-decider-nats`, for no benefit over the shared-prefix
reference of facet 5.

### Fork as a content-addressed snapshot share (LangGraph-style)

Rejected. JetStream's Object Store is name-addressed, not content-addressed, so
this bolts an entirely new content-hashed blob store and its garbage collector
onto the substrate and abandons the event-sourced model the decision is scoped to.

### Truncating the session log (purge-only, or archive-then-purge)

Rejected. Any design that removes an event from the log -- an irreversible
watermark-keyed purge, or an archive-then-purge that evicts after copying -- is a
logical deletion, and deletion is a slower edit of an append-only log. It forecloses
audit and rewind past the truncation point and (for archive-then-purge) drags in a
verified-archive KV ledger, a fork-lineage watermark term, and a purge-versus-new-fork
race. Keep-forever with snapshot-bounded replay and optional reversible cold-tiering
(facet 7) avoids all of it. A dedicated `RetentionLedger` decider, considered to make
such a purge crash-safe, is moot once nothing is purged.

### Subagent cascade via a cross-stream transaction or atomic multi-stream delete

Rejected because it is unavailable: a single `decide` names exactly one
`StreamId`, and JetStream offers no atomic write across subjects. The reconciler
process manager plus idempotent per-child commands (facet 6) is the append-only,
crash-safe realization.

### Compaction with an out-of-band recovery sidecar

Rejected as the default. A sidecar (Grok Build's model) adds a second artifact
that can go missing when rewinding past the boundary; the self-sufficient
in-stream marker of facet 4 keeps a single recovery story on a keep-forever log.

### A trait-level `WritePrecondition::At(N)` variant, or an explicit ownership/claim protocol

Rejected as unnecessary. The runtime already applies `At(current_position)` by
default, so no trait-level variant is needed -- one would only let an aggregate
weaken, never strengthen, the guarantee. And because the per-subject
expected-sequence guard already gives multi-writer correctness on a
server-side log, an OpenCode-style `events.claim` ownership protocol is a latency
optimization, not a correctness requirement.

### Uniform optimistic concurrency on every append

Rejected. Guarding every append with `At(current_position)` taxes the
highest-volume path (message, tool, and artifact events) with a head check and a
retry under contention, for no gain: those events commute and never overwrite, so
there is no stale-decision failure mode to catch. Facet 2 applies OCC only to the
invariant-bearing transitions, where a stale decision would actually corrupt an
invariant.

### No optimistic concurrency anywhere; enforce invariants only at the fold

Rejected. Dropping the head guard on the lifecycle transitions reintroduces the
races facet 6 exists to close -- spawning a subagent under a just-terminal parent,
a double close, a rewind against a moved head -- leaving a malformed (if still
append-only) log that every reader must defensively reconcile. Enforcing those
few invariants once, at the write boundary via `At(current_position)`, is cheaper
and safer than making every projection tolerant of them. This is the industry
default (only OpenCode enforced any precondition), and the hybrid keeps its cheap
hot path without inheriting its weak lifecycle guarantees.

## Non-Goals

- **A shared multi-party (human + multiple agents) conversation.** This ADR covers
  one agent per Session plus *directed* multi-agent collaboration via delegation
  (facet 6). It does not model a *symmetric* room where several independent agents and
  humans co-participate as peers. That is deferred to a future companion
  `Conversation` aggregate (named deliberately -- not `Session`, `Thread`, or
  `Context`, each already taken here), shaped like a Slack channel: a durable room
  with a participant roster and messages inside it, plus, if wanted, Slack-style
  message-anchored reply grouping (an `in_reply_to` relation on a message, one level
  deep -- no nested subthreads, matching Slack, Discord, and Matrix). Each participant
  Session keeps its own stream as the write-of-record; the merged view is a fold over
  those streams keyed by the conversation, in the cross-stream-fact idiom of facet 6.
  It aligns with A2A's `contextId` (the currently-unwired `context_id` value object in
  `a2a-nats` is the hook) but extends it, since A2A leaves `contextId` a
  correlation-only spec gap. It is deferred rather than decided because its one piece
  with no prior art -- a total order across several independently-appending Session
  streams -- needs its own design pass.
- **Mid-session model or runner switching.** The prior art's `trogonai-switching`
  crate and `cambio-modelo.md` mutate a running session's model. [ADR#0031](./0031-agent-implementation-and-session-plan.md) makes
  the SessionExecutionPlan immutable and requires a new Session for any change to
  what runs; the continuity concern becomes a fork or a new session. Deferred
  pending [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s resolution.
- **The optional cold-tiering job** (facet 7) -- its scheduling, authorization,
  failure handling, and whether a deployment enables it at all -- is out of scope;
  the log is keep-forever by default and tiering only relocates bytes reversibly.
- **How long cold-tiered copies live in the Object Store** -- a follow-up question
  only if tiering is enabled, analogous to [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)'s deferred KV history-depth.
- **Per-tenant retention or admission fairness.** [ADR#0028](./0028-decider-admission-control-and-backpressure.md) leaves QoS open and
  [ADR#0027](./0027-decider-multi-tenancy-primitive.md) scopes `Tenant` to storage resolution only; a per-tenant policy
  composes with both but is not decided here.
- **Tenant-to-authorization-principal linkage** -- [ADR#0027](./0027-decider-multi-tenancy-primitive.md) Non-Goal; [ADR#0026](./0026-command-authorization-principal.md)
  owns the principal.
- **Concrete Rust and proto contracts** -- exact message fields, KV key shapes,
  and the event-envelope / idempotency-key representation are implementation-level
  follow-up.

## Consequences

- A Session gains rewind and audit for free from event-sourcing, plus real
  expected-sequence optimistic concurrency on its invariant-bearing transitions -- a
  narrower form of the guarantee only one studied product had (OpenCode guarded every
  append; here only the invariant-bearing transitions are) -- while the high-volume
  transcript path appends without a head guard and never retries.
- Because commuting facts append without a head guard, a happened-fact (say a late
  `ToolCallCompleted`) can land after a terminal marker. The fold treats a session's
  first terminal marker as authoritative and ignores or reconciles later
  happened-facts, rather than the store rejecting them at write time; every read
  model over the transcript must be written to tolerate this.
- No caller migration is needed on the curated line (greenfield). The platform
  `crates/session` domain model is salvaged; its persistence is rewritten as a
  decider, and the switching subsystem is dropped pending [ADR#0031](./0031-agent-implementation-and-session-plan.md).
- New standing services appear: a subagent reconciler processor plus a scheduled
  orphan-closure sweep (facet 6); optional cold-storage tiering (facet 7) adds one
  more, but only if a deployment chooses to bound the hot stream.
- Gets harder: adding an event type now carries a proto plus a typed-decode plus a
  projection-fold obligation, and every command must carry an idempotency key its
  `decide` dedups on, since an append-only log cannot later edit away a
  double-appended fact.
- Cascade is eventually consistent: a child spawned concurrently with a parent's
  terminal marker may run a reconciler cycle or two before it is cancelled, and a
  deep collaboration chain cascades in O(depth) reconciler round-trips; callers
  expecting synchronous cascade are surprised. Graph acyclicity must be enforced
  at `DispatchDelegation` time, since the store cannot detect a cycle.
- Fork-of-fork chains cost O(depth) to replay unless sealed, so snapshot-on-fork
  is mandatory (facet 5). The log grows unbounded -- nothing is ever purged -- so
  storage is a capacity-planning concern, bounded in replay cost by snapshots and,
  if a deployment opts in, in hot-stream size by reversible cold-tiering.
- This decision depends on five still-draft ADRs (0026, 0027, 0028, 0029, 0031);
  each that changes before acceptance can reopen the facet that builds on it.

## References

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0014: Command and Query Naming](./0014-command-and-query-naming.md)
- [ADR#0021: Typed Decode over Passthrough Forwarding](./0021-typed-decode-over-passthrough-forwarding.md)
- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0026: Command Authorization Principal and Authorizer Hook for Decider Execution](./0026-command-authorization-principal.md)
- [ADR#0027: Tenant Value Object for Decider Stream and Snapshot Resolution](./0027-decider-multi-tenancy-primitive.md)
- [ADR#0028: Admission Control for Decider Command Execution](./0028-decider-admission-control-and-backpressure.md)
- [ADR#0029: Snapshot-Derived Retention Watermark for Decider Streams](./0029-decider-retention-and-truncation-watermark.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [Session store research synthesis](../research/session-store/synthesis.md)

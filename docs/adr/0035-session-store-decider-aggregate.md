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
  stream sequence authoritative for [read-side checkpoints](../glossary/checkpoint),
  high-water marks, and optimistic concurrency, and confines
  `Trogon-Origin-Stream-Sequence` to provenance on
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
until it lands, a session is scoped by its subject alone, and **shared
multi-tenant deployment of this store is explicitly blocked on [ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s
resolver contract landing** -- no `tenant_id` is added speculatively ahead of it.
Proto lives under `proto/trogonai/session/sessions/v1alpha1/` (domain `session`,
aggregate `sessions`), per [ADR#0009](./0009-protocol-buffers-wire-contracts.md).

The `v1alpha1` suffix is the honest stability signal, not a placeholder to drop
casually: the contract depends on five still-draft ADRs (0026, 0027, 0028, 0029,
0031) and on the substrate obligations facet 2 lists as prerequisites, so it is
promoted to `v1` only by a later decision, once this ADR and those dependencies
are accepted and those obligations are met. `v1alpha1` is also the room in which
[ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s tenant scoping, once accepted, lands additively rather than as a
breaking rename. `events.proto` carries a file-level comment naming this
promotion criteria.

### 2. Append-only mutation, opaque identity, ordinal anchors, and per-command optimistic concurrency

Append is the only mutation primitive. `decide` returns only new events, `evolve`
is the only place state changes, and `append_stream` is the only write path.
Every retroactive operation -- rewind, revert, compaction, hide, cancel -- is a
**new appended event interpreted at replay**, never an edit or a delete of stored
messages. No command ever purges or trims the stream; the log is keep-forever
(facet 7), and physical bytes only ever relocate between storage tiers, never
leave the record. This is forced decision #1, and it is what separates this
store from the corpus's cautionary cases (Goose's `DELETE`+re-`INSERT`, Hermes's
flag flips).

**`SessionOrdinal`: a logical anchor, never a physical JetStream sequence.** The
`SessionId` is an opaque addressing key; order and durable cross-references are
separate concerns from identity (forced decision #2). A payload that must
reference another event's position -- a fork's inherited-prefix boundary, a
rewind's inclusive keep-through boundary, a compaction's covered range, a
harness recovery checkpoint's coverage, a delegated child's dispatch
point -- uses `SessionOrdinal`: the
1-indexed position of an already-appended event within its own subject's fold
order, derived by counting at fold time, never read from JetStream message
metadata. Because it is fold-derived rather than physically assigned, it is
stable across restore, backfill, migration, and cold-tier relocation (facet 7),
all of which reassign physical stream sequences without rewriting event bytes
([ADR#0013](./0013-origin-stream-sequence-header.md)). This resolves the tension
the prior draft left open: a domain payload never repeats a physical position
that only makes sense in the stream that originally held it.

Three construction rules hold for every `SessionOrdinal` field:

1. **Same-stream past reference.** Any event may reference an already-appended
   event's ordinal on its own stream; the value is settled at the moment it is
   folded and stays replay-stable forever after.
2. **Cross-stream reference.** An event may carry another stream's ordinal only
   when it is copied from that stream's already-settled fold state -- for
   example, the child copying the parent's dispatch ordinal only after the
   parent's append has acked (facet 6).
3. **No self-position naming.** An event never writes its own predicted position
   into its payload; its ordinal is implied by where it lands in its own
   stream's fold, never asserted in advance. This is what kills the decide-time
   counter race a `WRITE_PRECONDITION = Any` guard would otherwise invite.

This does not reintroduce the prior art's application-assigned per-session `Seq`,
even though both are per-session integers: the prior art's `Seq` was assigned
*and guarded* at write time -- exactly the read-last-then-lease anti-pattern this
ADR supersedes. `SessionOrdinal` guards nothing and is assigned by nobody; it
exists purely as a fold-derived reference to something that already happened.
Physical JetStream sequence remains authoritative for OCC guards, processor
[checkpoints](../glossary/checkpoint), and consumption
([ADR#0013](./0013-origin-stream-sequence-header.md));
a `SessionOrdinal` never substitutes for it, and a physical sequence never
enters a domain payload.

**Write-precondition classification.** Optimistic concurrency is applied per
command by a three-way classification of the fact being appended, not uniformly
(refining forced decision #3). Every command declares exactly one
`WRITE_PRECONDITION`:

| Precondition | Commands (named by the fact recorded) | Why |
| --- | --- | --- |
| `NoStream` (guard `0`) | `CreateSession` records `[SessionStarted]`; `ForkSession` records `[SessionStarted, SessionForked]`; delegated child creation records `[SessionStarted, ParentLinked]` | Creation is atomic and exactly-once; the stream must not already exist. |
| `At(current_position)` | `SessionClosed`, `SessionCancelled`, `SessionFailed`, `SessionHidden`, `SessionRewound`, `Compacted`, `ExecutionAttemptStarted`, `ExecutionAttemptReady`, `ExecutionAttemptEnded`, `ToolCallApproved`, `ToolCallDenied`, `OperationReserved`, `OperationOutcomeRecorded`, `OperationCancellationRequested`, `DelegationDispatched`, `ExternalDelegationDispatched`, `ParentTerminated`, `ParentHistoryInvalidated`, `DelegationDetached`, `ParentDetached`, `RedactionApplied`, `ArtifactErased` | `decide` genuinely branches on the current head for each of these: one active attempt, Ready-after-Started, mutually exclusive approve/deny and complete/fail decisions, one terminal outcome per ledger operation, and one saga step per dispatch or detach. A stale decision here would violate an invariant, so it must be rejected, not appended. |
| `Any` (no server-side guard) | `UserMessageRecorded`, `AssistantMessageStarted`, `AssistantMessageCompleted`, `AssistantMessageFailed`, `ToolCallRequested`, `ToolCallStarted`, `ToolCallCompleted`, `ToolCallFailed`, `ArtifactRecorded`, `FileChanged`, `CheckpointProduced`, `SystemNoticeRecorded`, `TodoUpdated`, `SessionRenamed`, `SessionArchived`, `SessionUnarchived` | These commute: `decide` does not need the exact head to be correct, appends never overwrite, and the highest-volume path stays retry-free. |

`StreamExists` is never used, because it sends no server-side guard.

Some `Any` facts need an explicit fold rule, because commuting is not the same
claim as conflict-free (forced decision #4's correction): a handful of `Any`
facts can still disagree about a shared entity, and an append-only log cannot
resolve that by refusing the write. The fold resolves it deterministically
instead:

- **Per-entity first-terminal-outcome-wins.** `ToolCallCompleted` versus
  `ToolCallFailed` (keyed by `tool_execution_id`) and `AssistantMessageCompleted`
  versus `AssistantMessageFailed` (keyed by `message_id`) resolve to whichever
  reaches the fold first in the stream's total order; a later conflicting
  outcome is retained on the log (nothing is ever dropped) but is audit-only,
  surfaced by a projection flag, never folded into state. Because fold order is
  the stream's own total order, this is replay-deterministic regardless of
  arrival timing.
- **`TodoUpdated`: highest-`revision`-wins.** Every update carries a required,
  monotonic `revision` from the session's single logical writer (the active
  attempt's loop); the fold keeps whichever update has the highest revision seen
  so far, independent of arrival order -- which is what makes it truly commuting
  rather than merely unguarded. Ties resolve to the first occurrence in fold
  order.
- **`CheckpointProduced`: first evidence per checkpoint id wins.** The first
  admitted artifact evidence for a `checkpoint_id` is the value restoration may
  select. A later payload reusing that id is retained for audit but cannot
  replace the first value. The command idempotency key includes a canonical
  digest of the complete checkpoint evidence (the exact `Checkpoint` bytes the
  command persists), not only the artifact digest, so a later payload that
  differs in any evidence field remains visible while byte-identical
  redelivery collapses through the event identity contract below.
- **Post-terminal happened-facts remain audit-only**, generalizing the existing
  rule: a session's first terminal marker is authoritative, and any
  happened-fact folded after it (a late `ToolCallCompleted`, for instance) is
  retained on the log but never changes state.

The reasoning above (`ExecutionAttempt*` mint and advance a monotonic attempt
counter under one-active-attempt and Ready-after-Started invariants;
`ToolCallApproved`/`Denied` are mutually exclusive human-paced decisions; the
ledger and delegation events guard reservation and saga invariants) is why those
events moved out of the corpus-style "commuting happened-fact" bucket the
original event catalog put them in: all are low-volume, so the contention
argument for `Any` does not apply to them. The head guard closes concurrent
races; a sequential crash-then-retry duplicate is closed separately, by the
identity and dedup contract below.

This refines the corpus's forced decision #3 (a precondition on *every* append)
to a precondition scoped to what each fact actually needs: creation gets
`NoStream`, invariant-bearing transitions get `At`, and commuting facts get
`Any`. It still supersedes the prior art's pessimistic KV lease, which
serialized *all* writes: here only the invariant-bearing and creation
transitions coordinate, and via OCC rather than a lock. Uniform strict OCC and
no-OCC-anywhere are both weighed and rejected under Alternatives.

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

**Identity and dedup contract.** Commands are processed at-least-once -- a NATS
processor redelivers a command after a crash before its ack -- so every command
carries a caller-supplied idempotency key, stable across redelivery, and no
domain payload gains a separate identity field of its own. The runtime derives
each appended event's envelope `Event.id` deterministically: UUIDv5 over
`(resolved stream subject, command type, command idempotency key, index of the
event within the decision's batch)`. The subject and command type are in the
derivation because `Nats-Msg-Id` dedup is stream-wide and every session subject
shares the one physical `SESSION_EVENTS` stream (facet 1): without them, two
different sessions -- or two different commands -- reusing one idempotency key
would collide to one id and the second append would be silently swallowed as a
duplicate. With them, key uniqueness only has to hold per session and command
type, which the caller can actually guarantee. A redelivered command therefore
reproduces byte-identical event ids on retry, while distinct events within one
multi-event batch (a `[SessionStarted, SessionForked]` fork, say) stay distinct
-- no batch aliasing, and no cross-session aliasing. This is what makes
the `Any` path's at-least-once append safe: two concurrent redeliveries sharing
a key both replay state before either append is visible, both conclude the key
is new, and both append -- but they append the *same* event id, so the fold and
every projection collapse them as one fact once dedup is in place, rather than
relying on a `decide`-time key check that is only a best-effort filter under
`Any`. On the guarded (`At`) path the sequence guard already makes
check-and-append atomic, so the losing delivery gets `WrongExpectedVersion`,
re-replays, sees the key already folded, and no-ops.

Publish dedup closes the write side: the append sets the NATS header
`Nats-Msg-Id` to the event id, and a duplicate acknowledgement inside the
JetStream dedup window is idempotent success, not an append error. Fold dedup
closes the read side: `evolve` (native and WASM) must receive the envelope event
id, and the fold and every projection collapse events with an already-seen id,
with snapshots persisting that seen-key state across a restore. The seen-key
state is bounded by a horizon that is not a guess: duplicates originate only
from command redelivery, so every deployment must configure the command
transport's maximum redelivery lag to be finite and the retained seen-key
horizon to cover it (seen-key horizon >= max command redelivery lag >=
JetStream duplicate window -- a stated configuration invariant, enforced with
the other substrate obligations below, not hoped for). A duplicate id beyond
that horizon is impossible by that invariant, not merely unlikely. Entity-keyed
facts are additionally immune regardless of the id set: a duplicate terminal
outcome no-ops under first-terminal-outcome-wins per entity id, a repeated
`TodoUpdated` no-ops under highest-revision-wins, harness recovery checkpoint
evidence applies first-wins per checkpoint id, and artifact and operation facts
collapse on their own stable ids --
the seen-key horizon is defense for the purely arrival-ordered facts (`UserMessageRecorded`,
`FileChanged`, `SystemNoticeRecorded`). Beyond the dedup window, a guarded
(`At`) command's retry re-replays and no-ops on its idempotency key as before;
an `Any` fact past the window relies on this reader-side collapse by identical
id. `Trogon-Correlation-Id`
and `Trogon-Causation-Id` are the correlation and causation headers, validated at
the append boundary (non-empty, header-safe values, per the runtime's existing
header validation). None of this is optional plumbing layered on later: a
conflicting fact recorded under a *different* idempotency key is not a duplicate
at all, and is resolved instead by the fold rules above, not by this contract.

**This is a substrate obligation, not a shipped guarantee.** None of the dedup
machinery above exists in the runtime today, and an earlier draft of this ADR
incorrectly asserted that readers already dedup by an event identity that native
and WASM replay do not currently expose to `evolve`. The corrected claim: this
store cannot go live until the substrate obligations below are met.

**Substrate obligations (prerequisites to implementation).** The following are
tracked in the backlog as prerequisites, not covered by this ADR's wire contract
alone:

- Evolve-visible event identity in both native and WASM replay paths.
- A deterministic id-derivation hook (the `EventIdentity::event_id` override
  point already exists in the runtime and is where this plugs in).
- Duplicate-publish-ack treated as success rather than an append error.
- A decode-failure metric (facet 3).
- Command-receipt tests for crash-before-ack, concurrent redelivery, and
  multi-event batches.
- Bounded command redelivery enforced at the command transport, with the
  configuration invariant seen-key horizon >= max command redelivery lag >=
  JetStream duplicate window validated at deployment, so no duplicate id can
  outlive the fold's retained seen-key state.

### 3. Every event is a typed protobuf, schema-validated at the storage boundary

Session events are protobuf messages under `proto/trogonai/session/sessions/v1alpha1/`
(`edition = "2024"`, structurally-required fields tagged
`[features.field_presence = LEGACY_REQUIRED]`), following the scheduler
precedent: present-tense `VerbNoun` commands (`CreateSession`, `ForkSession`,
`RewindSession`, `HideSession`, `DispatchDelegation`), past-participle
`NounVerbed` events (`SessionStarted`, `SessionForked`, `SessionRewound`,
`Compacted`, `ToolCallCompleted`), one `SessionEvent` oneof envelope importing
every event file, and `state`/`projections`/`checkpoints` sibling subtrees whose
read-model value types are redefined locally to decouple their evolution from the
write side.

Validation belongs to a Session-owned append and replay boundary, not the
generic runtime or NATS adapter. Otherwise domain-specific ownership and stream
address checks either leak into infrastructure or can be skipped by another
Session write path. That boundary validates a whole batch before publish,
requires each decoded event to be an owned Session event whose payload
`session_id` matches the addressed stream, and revalidates replay before
`evolve` (forced decision #9,
[ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md)).
`validate_session_event` owns local same-event shape checks. Plan, lifecycle,
and history relationships remain Session `decide` and `evolve` invariants,
protected by the command's OCC classification instead of being pushed into a
generic codec. A malformed batch is rejected before any member reaches durable
storage; malformed replay fails closed before state changes.
Decode, local validation, and stream-identity failures are typed, observable,
and non-retryable until the input or addressed stream changes, preventing a
poison message from cycling without new evidence.

This ADR adds an observable decode-failure metric for session events -- the
decider crate emits no such metric today and its append/replay decode-error
paths are currently silent -- following
[ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md)'s principle that boundary decode failures must be
measured, not dropped. Unlike the prior art, no `InvalidEventRejected` event is
persisted: rejection happens before the write, so there is nothing to record.
Schema evolution is additive (new optional fields, reserved retired numbers),
never a per-event version branch.

Every event carries a small envelope alongside its typed payload: an `Event.id`
deterministically derived from the command's idempotency key and its batch index
(facet 2) -- not itself the idempotency key, which lives on the command, not the
event -- an append timestamp (transport metadata; see the event-time policy
below), the acting principal ([ADR#0026](./0026-command-authorization-principal.md)),
and the correlation and causation headers (facet 2). Dedup, audit, and
authorization are answerable from the envelope; occurrence time, when it
matters, is answerable from the payload instead.

The prior art's 46-arm payload is ratified as the starting vocabulary checklist,
not copied verbatim: lifecycle (`SessionStarted`, which stores the
`StoredSessionExecutionPlan` once per [ADR#0031](./0031-agent-implementation-and-session-plan.md); `SessionClosed`,
`SessionCancelled`, `SessionFailed`, `SessionHidden`), the ExecutionAttempt
facts ([ADR#0031](./0031-agent-implementation-and-session-plan.md)), conversation (`UserMessageRecorded`,
`AssistantMessageStarted`/`Completed`), tool lifecycle
(`ToolCallRequested`/`Approved`/`Denied`/`Started`/`Completed`/`Failed`), artifacts
(`ArtifactRecorded`, claim-check by sha256), file changes, compaction
(`Compacted`, facet 4), rewind (`SessionRewound`), fork (`SessionForked`, facet
5), delegation, external delegation, and rewind/terminal cascade sagas (facet
6), redaction and artifact erasure (facet 7), reversible listing state
(`SessionRenamed`, `SessionArchived`, `SessionUnarchived`), and the
operation-ledger reservations that make tool and delegation side effects
idempotent -- [ADR#0031](./0031-agent-implementation-and-session-plan.md)'s ledger
deduplicates by operation id and digest and reconciles indeterminate outcomes; it
is not exactly-once execution. Every command is gated before `decide` by the
proposed `CommandPrincipal`/`CommandAuthorizer` of draft
[ADR#0026](./0026-command-authorization-principal.md), once those land.

**Four records with separate authority.** These records solve different
failures:

1. The typed event log is authoritative. It is the only record from which the
   Session aggregate and read models are rebuilt.
2. An aggregate [snapshot](../glossary/snapshot) is an advisory cached fold of
   that log. Corruption or incompatibility falls back to earlier replay.
3. A harness recovery checkpoint is an opaque artifact used only when the
   platform continues process state from an in-flight harness loop. It cannot
   replace event replay or satisfy an aggregate snapshot.
4. A read-side [checkpoint](../glossary/checkpoint) is only a consumer's
   processed stream position.

The protobuf `Checkpoint` keeps its existing wire name, but ADR prose uses
harness recovery checkpoint for it so these concepts do not collapse into one
another.

**Harness recovery checkpoint admission.** The `Checkpoint` embedded in
`CheckpointProduced` and in `ExecutionAttemptStarted.restored_checkpoint` has
its own `checkpoint_id`, `producing_execution_attempt_id`, `covers_through` (a
`SessionOrdinal`, facet 2), and `session_execution_plan_digest`, alongside its
`reference`, `checkpoint_type`, `digest`, and `implementation_version`, plus
the capture-attestation reference and digest and the effective-history digest
that [ADR#0031](./0031-agent-implementation-and-session-plan.md) requires as
the admission proof of semantic coverage.
`CheckpointProduced` stays `{session_id, checkpoint}` and
`ExecutionAttemptStarted.restored_checkpoint` stays embedded because the latter
is evidence of exactly what the new attempt restored.

The Session-owned command boundary applies the full harness recovery
checkpoint admission contract from
[ADR#0031](./0031-agent-implementation-and-session-plan.md) before append. The
standalone payload validator enforces local shape, including equality between a
restored checkpoint's plan digest and the plan digest on its containing
`ExecutionAttemptStarted`. The Session decider separately enforces attempt,
stored-plan, and ordinal relationships against folded state. Artifact
verification enforces sealing, digest, the capture attestation and its
effective-history equality, and compatibility with the harness implementation
version committed by the plan.

`CheckpointProduced` records evidence about a cut that is already settled. The
evidence remains historically valid when later Session events append, although
a later rewind, redaction, or artifact erasure reaching history at or before
its cut makes it ineligible for restoration. Production therefore uses
`Any`. The command boundary verifies that the producing attempt exists, its plan
digest matches the Session, and `covers_through` names settled in-session
history.

Restoration is the head-dependent choice. At the current head selected by
`StartExecutionAttempt`, `decide` requires all of the following:

- `checkpoint_id` resolves to the first admitted `CheckpointProduced` evidence
  selected by the fold;
- the complete embedded `Checkpoint` value exactly equals that first evidence,
  not only its plan digest;
- `covers_through` remains in effective Session history after every rewind
  folded through the selected head;
- no `RedactionApplied` folded through the selected head targets an event at
  or before `covers_through`, and no `ArtifactErased` erases an artifact
  recorded at or before it, because tail replay cannot rebuild the sealed
  prefix a reinterpretation retargets (a `Compacted` marker after the cut does
  not disqualify: it masks nothing and folds identically in the tail and in a
  fresh replay); and
- the producing attempt and stored Session plan still satisfy the recovery
  contract.

`ExecutionAttemptStarted` therefore remains `At(current_position)`. After it is
appended, the supervisor verifies and restores the artifact, then replays the
exact effective tail after `covers_through` through the head selected by the
start command. Ready and new work wait until that replay completes, as specified
by [ADR#0031](./0031-agent-implementation-and-session-plan.md).

`Checkpoint.covers_through` is the core Session replay cut. Internal harness
coordinates remain inside the opaque, versioned artifact, not core event fields
or `SessionOrdinal` values. A partial, mutable, digest-mismatched, ambiguously
correlated, or incompatible artifact is not admissible. Missing or invalid
checkpoint evidence falls back to authoritative event replay and a fresh
ExecutionAttempt. Future
Claude, Codex, or other product integrations translate at the edge and cannot
add their session identities, transcript layouts, or resume coordinates to the
core Session schema.

**Tool-fact ownership.** `ToolCallRequested` and the provider-visible
`ToolUseBlock`/`ToolResultBlock` embedded in message events are not the same
fact copied twice by accident; each owns a distinct concern and both are kept.
`ToolCallRequested` owns the execution-request record: what the platform was
asked to run (name, `input_json` as dispatched, the operation link).
`ToolUseBlock`/`ToolResultBlock` own the provider-visible transcript form: the
exact blocks the model emitted or received, required for faithful provider
replay. They join by `tool_call_id`/`tool_use_id`; equality between the two is
expected but not structurally enforced, since normalization may differ between
the execution record and the transcript form. The model-visible context
compiles from message events; the execution and audit trail folds from
lifecycle events. No atomic ordering is guaranteed among these `Any` facts --
every fact is self-contained and joins by id, never by arrival adjacency. (An
id-only slimming of `ToolCallRequested` was considered and rejected: under
streaming, `ToolCallRequested` and the approval UI precede
`AssistantMessageCompleted`, so an id-only request event cannot drive approval
before the message that would supply the missing fields exists.)

**Unmodelled provider blocks are kept verbatim, never interpreted.**
`ContentBlock` gains a `ProviderBlock` arm carrying the emitting provider, the
provider's own block discriminator, and either inline bytes or an `ArtifactRef`.
It exists so a provider shipping a new block type does not force a schema change
before a session using it can be recorded or replayed; the alternative is
dropping the block, which silently corrupts replay of the turn that contained
it. It is the concession `ThinkingBlock.signature` already makes, generalized.
The rule is write-verbatim, read-never: a projection must never interpret this
payload, and a block any reader needs to understand is a block that has earned
its own arm in the oneof. Using it as an extension point for our own data would
turn the canonical form back into the untyped provider blob this facet exists to
replace.

**Turn identity is stamped, never inferred.** A turn is one
user-prompt-to-final-assistant-message cycle, including every tool call made on
the way. It is the unit a person names when they ask to rewind, retry, or cost
out work, so it is carried as a `turn_id` on every conversation and tool event
rather than reconstructed at read time. It cannot be folded: concurrent
`Any`-precondition appends give no reliable "next event after" relation to infer
membership from (facet 2, and the tool-fact ownership note above). It is
required on `UserMessageRecorded`, the three `AssistantMessage*` events, and
`ToolCallRequested`/`Started`/`Completed`/`Failed`, and optional on
`ToolCallApproved`/`Denied`, where an external approver may hold the call
identity without the turn context.

**Reads are observations on the completing call, not facts of their own.** A
session reads far more than it writes, so a per-read event would dominate the
log while carrying almost no decision value. Instead `ToolCallCompleted` carries
`repeated ResourceObservation observed`: the uri, what the read found, and the
byte range actually seen, with `complete` distinguishing a whole-resource read
from a partial one. Absence is a recorded outcome rather than a missing digest,
so a producer that failed to hash content is never mistaken for one that
looked and found nothing, and an absent outcome marked complete is a
confirmed-absent-in-full precondition exactly as strong as a whole-resource
read. This is what makes a replayed write checkable rather than merely
repeatable: the observations are the preconditions the call was decided under,
so a replay can compare current digests against them and refuse a stale
apply. It keeps
[ADR#0024](./0024-agent-platform-stream-topology.md)'s "a fact is recorded once"
rule intact by hanging the read set on the fact that already exists rather than
multiplying events.

Correspondingly, `FileChanged` requires `tool_call_id` and `turn_id`: every
recorded change is attributed to the call that made it. A change with no
proximate tool call is not an unattributed `FileChanged`; it surfaces as a
`ResourceObservation` with a digest that no longer matches what was last
observed, which is the honest shape for "something changed and we do not know
who did it."

The two facts only share a resource when their locations resolve to the same
URI: `ResourceObservation.uri` is already in that form, while `FileChanged.path`
is workspace-relative and must be resolved against the session's
`WorkspaceRef.uri` (`workspace.uri + "/" + path`) before the comparison holds.
The asymmetry is deliberate: `FileChanged` is inherently workspace-scoped,
while `ResourceObservation` also covers resources a workspace-relative path
cannot name at all, such as a fetched URL or an MCP resource, and those have no
`FileChanged` counterpart to join against.

**Typed process outcome.** `ToolCallCompleted.termination` carries a
`CommandTermination` oneof of `exit_code` or `signal`, plus a
`google.protobuf.Duration duration`. It belongs to the execution/audit fold, not
the provider transcript: a command that ran and exited non-zero is
`ToolCallCompleted` with `TOOL_CALL_RESULT_STATUS_APPLICATION_ERROR` and
termination set, while a command that never ran at all is `ToolCallFailed` with
no termination. Parsing an exit status back out of result text is not a
projection this store asks readers to write.

**Workspace, settings, change shape, and usage completeness.**
`SessionStarted.workspace` is a required `WorkspaceRef` (`workspace_id`, `uri`,
optional `revision`), so the ground a session's file facts refer to is recorded
with the session rather than inferred from paths.
`AssistantMessageStarted.settings` carries the `ModelSettings` a completion was
requested under, which is what makes a replay reproducible rather than merely
re-runnable. `FileChanged.diff` carries a `DiffSummary` (added/removed lines,
truncation flag, rendered artifact) so a change list renders from one read
instead of fetching both artifact sides per row. `TokenUsage.completeness`
distinguishes a final counter from a partial one; unset reads as final, which is
what every counter recorded before the field meant.
`ArtifactRef.untruncated_size_bytes` records the pre-truncation size when the
referenced bytes are themselves a truncation, so a reader can tell "1 KB of
output" from "1 KB of a 40 MB output" without fetching anything.

**Event-time policy.** Envelope append time is transport metadata; fold logic
never depends on it. A payload carries its own occurrence timestamp only where
the fact has a real external occurrence distinct from when it was appended:
`ExecutionAttemptStarted.started_at`, `ExecutionAttemptReady.ready_at`,
`ExecutionAttemptEnded.ended_at` (all required, matching
[ADR#0031](./0031-agent-implementation-and-session-plan.md)'s attempt shapes),
plus the existing `CanonicalMessage.created_at`, `ArtifactMetadata.created_at`,
and `ExternalArtifact.fetched_at`, whose comments are clarified to occurrence
time, not append time. Everything else relies on append order, not clocks. This
is a narrow, named exception, not a general license to add timestamps.

**Validation ownership at the append boundary.** `LEGACY_REQUIRED` enforces
presence, not semantic validity. `validate_session_event` therefore owns only
checks answerable from one decoded event. At minimum it rejects:

- Empty or malformed identifiers within the event.
- Nonzero, supported enum values wherever a field is a required enum, including
  `CascadePolicy` (persisted values only `1` or `2`; the command layer applies
  the safe default before append, per the [cascade policy](../glossary/cascade-policy)
  glossary correction).
- Set oneofs: `ContentBlock.kind`, `ToolCallResult.kind`,
  `ArtifactMetadata.source`, `OperationOutcomeRecorded.outcome`,
  `CommandTermination.outcome`, `ProviderBlock.payload`,
  `ResourceObservation.outcome`.
- Role agreement: `UserMessageRecorded.message.role` is `USER`;
  `AssistantMessageCompleted.message.role` is `ASSISTANT`.
- `FILE_CHANGE_KIND_RENAMED` requires `previous_path`; non-renames must omit
  it.
- Locally ordered compaction bounds (`covers_from <= covers_through`).
- `matched_stop_sequence` present if and only if `FINISH_REASON_STOP_SEQUENCE`.
- Positive attempt numbers and the within-event coupling between attempt number
  and presence of `previous_attempt_id`.
- A restored checkpoint plan digest equal to its containing
  `ExecutionAttemptStarted` plan digest.
- Supported digest algorithms with length checks matching the algorithm
  (claim-check digests are sha256 in `v1alpha1`; `Digest.algorithm` is the
  additive escape hatch for a later one).
- Well-formed `input_json`.
- Valid ISO 4217 currency codes.
- Valid timestamps, and non-negative durations with sub-second nanos.
- Unique todo ids with valid statuses.
- Finite `ModelSettings` floats; provider-specific numeric ranges are
  deliberately not enforced here, since they change per model and would make the
  storage boundary a provider-compatibility oracle.
- `ArtifactRef.untruncated_size_bytes`, when set, strictly greater than
  `size_bytes`; equal or smaller means the field is meaningless and is a
  producer bug.
- `DiffSummary.rendered` present whenever `DiffSummary.truncated` is true: a
  truncated diff with nothing to fetch is unreadable.
- `ResourceObservation` with a non-empty uri, a set `outcome`, a nonzero
  `range.length` when a range is given, and no `range` when the outcome is
  absent.

The Session-owned append and replay boundary, outside
`validate_session_event`, verifies that the decoded type belongs to Session and
that its payload `session_id` matches the addressed stream. The Session command
boundary also computes request digests over the exact bytes it will persist.
`decide` and `evolve` own every history-dependent relationship: assistant
start/completion id and model joins, tool lifecycle joins, in-session ordinal
existence and compaction ordering, exact attempt lineage, first-wins checkpoint
evidence selection, complete restored-checkpoint equality with that evidence,
continued effectiveness of `covers_through` after rewind, and equality with the
stored Session plan.
This split prevents a local payload validator from claiming facts that only
the command context or folded history can prove.

Every unset oneof, unspecified enum, and malformed same-event shape above is
rejected before append, never persisted and reconciled later.

### 4. Compaction is a self-sufficient in-stream marker the store only records

Compaction is an upstream agent-loop concern (forced decision #4): the store
neither triggers nor understands it. The loop decides when the transcript nears
the context window and how to summarize; the store persists a single `Compacted`
event carrying the summary content inline plus the covered range
(`covers_from`, `covers_through`, both a `SessionOrdinal`, facet 2, both
inclusive): exactly the events the summary replaces in the model-visible view,
ratifying the prior art's `SummaryCreated` shape. The model-visible view folds
from the newest `Compacted` summary and every event strictly after
`covers_through`; the covered events remain on the stream for audit and rewind.

The marker is self-sufficient: no out-of-band sidecar is required to replay
across a compaction boundary. This resolves the corpus's open sub-question in
favor of the in-stream marker (Claude Agent SDK, Codex CLI) over Grok Build's
fail-closed external `compaction_checkpoints/{id}.json`, keeping one recovery
story instead of a second artifact that can go missing. It also corrects the
platform compactor crate, which overwrote the stored message list wholesale -- the
exact destructive pattern the corpus warns against.

### 5. Fork is an atomic, self-contained creation; inheritance is by explicit reference

Fork mints a genuinely independent Session, not a lazily-composed replay view of
the source (forced decision #5). `ForkSession` appends an atomic
`[SessionStarted, SessionForked]` batch as the first two events on the new child
subject, under the `NoStream` guard, so the fork point is atomic and
exactly-once even under retries. `SessionStarted` carries the fork's own new
`StoredSessionExecutionPlan`, exactly as any other creation does
([ADR#0031](./0031-agent-implementation-and-session-plan.md)'s per-Session plan
binding is satisfied, closing a gap the prior design left: a fork used to have
no plan of its own). `SessionForked`, second in the batch, carries
`source_session_id`, `context_prefix_boundary` (a `SessionOrdinal` on the source
stream, inclusive, facet 2), and `reason`.

**The child aggregate folds only its own stream.** Fork replay never folds
source events into child aggregate state; this removes, wholesale, the imported
plan, attempts, ledger, terminal state, children, and rewind/compaction state
that a naive prefix-replay would otherwise drag in. Inheritance is instead an
explicit, named boundary between three categories:

- **Inherited by reference**, through the model-visible context projection
  keyed by `(source_session_id, context_prefix_boundary)`: conversation
  messages, compaction summaries, and artifact references within the source
  prefix.
- **Reset**: lifecycle, execution plan (the fork stores its own), execution
  attempts, operation ledger, todo state, delegation links.
- **Forbidden**: source terminal state, source rewind/compaction markers as
  aggregate state, source children.

Fork from a terminal source is legal and needs nothing special: the fork has
its own fresh lifecycle regardless of what happened to the source afterward.
Fork-of-fork chains resolve because the context projection walks prefixes
recursively; a cycle is impossible by construction, since a fork can only ever
reference an already-existing source's already-settled past, never a session
that does not yet exist or one still being created. A missing or unreadable
ancestor at projection time is a typed projection error, not a fold-time crash;
`ForkSession` itself validates that the source exists at creation time, before
the child stream is created.

A later rewind of the source does not retroactively alter a fork's context:
`context_prefix_boundary` is an immutable snapshot-in-time reference into a
keep-forever log (facet 7), not a live pointer. Rewind cascade (facet 6)
applies to delegated children, never to forks -- a fork's relationship to its
source is a one-time copy-by-reference at creation, not an ongoing dependency a
source mutation could invalidate.

**Replay cost is no longer an aggregate concern.** Because the child fold never
touches the source stream, the old mandatory sealing snapshot is no longer a
correctness requirement: a fork's own replay cost is O(child events), the same
as any other Session, regardless of fork depth. The prefix-walk cost that a deep
fork-of-fork chain does incur lives entirely in the context projection now,
which caches and checkpoints like any other projection (facet 8) -- it is a
read-side performance concern, not a write-side correctness one.

### 6. Child sessions: parent-first dispatch, rewind invalidation distinct from termination, and a two-fact detach saga

A child session is its own logical stream linked to its parent by facts
recorded on each side, never a cross-stream transaction (`decide` cannot express
one, and JetStream offers no atomic write across subjects).

**Dispatch is parent-first, with crash-safe repair.** `DispatchDelegation`
(`At`-guarded, on the parent stream) appends `DelegationDispatched{session_id,
operation_id, child_session_id, cascade_policy}` -- no position field; the
event's own fold ordinal is the dispatch point, so nothing needs to name it.
Only after that append acks does child creation happen: the atomic
`[SessionStarted, ParentLinked]` batch under `NoStream` on the new child
subject. `ParentLinked` carries `operation_id` (the saga's join key, reusing the
operation-ledger id) and `parent_dispatched_at`, a `SessionOrdinal` (facet 2)
copied from the parent's `DelegationDispatched` event only after that parent
append has acked. `DelegationDispatched.cascade_policy` is the authoritative
saga input -- the crash-repair path mints the child from the parent fact alone,
so the parent fact must carry everything creation needs -- and
`ParentLinked.cascade_policy` is copied verbatim from it by the creation batch;
`decide` rejects a creation whose copy differs as a typed conflict, so the two
records cannot diverge. If the process crashes between the parent append and the
child creation, a reconciler observes a `DelegationDispatched` with no
corresponding child stream and re-issues child creation; `NoStream` makes that
repair exactly-once, and a duplicate creation attempt simply no-ops on
`WrongExpectedVersion`. The reverse ordering -- a child claiming a parent that
never actually dispatched it -- cannot occur by construction, because nothing
but this saga produces a `ParentLinked`.

**Acyclicity by construction.** `DispatchDelegation` always mints a fresh
`child_session_id`; `ParentLinked` is valid only inside the creation batch, so
re-parenting an already-existing stream is rejected by `decide` (the stream
already exists, so the `NoStream` batch fails outright). A cycle would require
an edge into a pre-existing session, which this makes impossible -- the graph is
a directed tree by construction, not by a runtime cycle check.

**Rewind invalidates history; it does not terminate.** `SessionRewound.keep_through`
(a `SessionOrdinal`, facet 2) is inclusive: events `[1..keep_through]` remain
valid, and this replaces the old ambiguous `>= to_sequence` rule. Children are
affected only if their `parent_dispatched_at` is strictly greater than
`keep_through` -- a child dispatched at or before the boundary survives
untouched; only a child dispatched from history that no longer exists is
invalidated. Because a rewound parent is not terminal and may keep running,
invalidating such a child is **not** the same event as a terminal cascade: the
reconciler appends a distinct atomic `[ParentHistoryInvalidated,
SessionCancelled{reason = PARENT_REWIND_CASCADE}]` batch on the child, new
event `ParentHistoryInvalidated{session_id, parent_session_id,
parent_keep_through, triggering_event_id}` naming exactly what happened and why,
and only when `cascade_policy = CASCADE_ON_PARENT_TERMINAL`; an `INDEPENDENT`
child records nothing and keeps running through the rewind.

**Terminal cascade stays a separate batch, for a separate cause.** A parent
reaches a terminal state through a Session-level event on its own subject --
`SessionClosed`, `SessionCancelled`, `SessionFailed`, or `SessionHidden` -- and
`ParentTerminated` now carries a typed `cause` (`ParentTerminalCause`:
`CLOSED`, `CANCELLED`, `FAILED`, `HIDDEN`) plus `triggering_event_id`, so a
reader no longer has to infer which of the four actually happened. Crash is
not itself a trigger: an `ExecutionAttemptEnded` is per-attempt, so a liveness
watchdog that concludes no further attempt will run records a Session-level
`SessionFailed`, and that is what cascades. A reconciler
[processor](../glossary/processor) subscribed to `session.sessions.events.>`
matches these Session-level terminal markers and dispatches a reconcile command
to each eligible child (discovered through the parent-to-children lineage
projection folded from `DelegationDispatched`), whose `decide` emits the atomic
`[ParentTerminated, SessionCancelled]` batch on the child's own subject,
distinct from the rewind-cascade batch above, or a typed no-op error if already
terminal. Cascade is transitive, because `SessionCancelled` is itself a
terminal marker the same reconciler reacts to; a chain of depth D still takes D
sequential reconciler round-trips, each bounded by the processor's redelivery
policy.

**Detach is two causally linked local facts, not a mirrored write.** The prior
design wrote the same detach fact onto both streams, violating
[ADR#0024](./0024-agent-platform-stream-topology.md)'s record-once rule: a fact
is recorded once, in the stream whose invariant needs it. Detach is now
genuinely two local facts, one per stream, joined by one durable
`detach_operation_id` rather than by re-deriving the same event twice: the
parent records `DelegationDetached{session_id, child_session_id,
detach_operation_id, reason}` (no `parent_session_id` -- it is implicit in the
stream the fact lives on), and the child records the new
`ParentDetached{session_id, parent_session_id, detach_operation_id}`. Each is
its own invariant-bearing (`At`-guarded) local fact, satisfying [ADR#0024](./0024-agent-platform-stream-topology.md)'s
record-once rule because neither is a copy of the other -- they are two
different streams' own truths about the same operation. Crash repair follows
the same shape as dispatch: the reconciler completes whichever side is missing,
idempotently, deduping by `detach_operation_id`; a duplicate delivery no-ops
because `decide` sees the operation id already folded on that side.

**External delegation carries the evidence [ADR#0031](./0031-agent-implementation-and-session-plan.md) requires.** New event
`ExternalDelegationDispatched{session_id, operation_id, delegate_reference,
authenticated_remote_subject, authorization_reference, request_digest,
correlation_id}` (`At`-guarded, on the dispatching session's stream) carries
exactly the resolved delegate reference, authenticated remote subject,
authorization reference, request digest, and correlation id
[ADR#0031](./0031-agent-implementation-and-session-plan.md) requires for an
[external delegated agent](../glossary/external-delegated-agent).

**Operation outcome is a typed oneof, not a flat enum with conditionally-meaningful
fields.** `OperationOutcomeRecorded` carries `oneof outcome { OperationSucceeded
succeeded; OperationFailed failed; OperationCancelled cancelled;
OperationUnknown unknown; }`, each variant holding exactly the evidence its case
needs (a response digest and optional artifact reference for success, a failure
digest for failure, a canceling actor for cancellation). `unknown` is the only
non-terminal case, and may be superseded exactly once by a determinate outcome
under the `At` guard; every determinate outcome is terminal, one per
`operation_id`. The join key across the saga is `operation_id`; `correlation_id`
is not repeated here because it is only meaningful on the dispatch event that
carries it.

**Cancellation intent is recorded separately from cancellation outcome.** New
event `OperationCancellationRequested{session_id, operation_id, reason}`
records intent to cancel; `decide` rejects it as a typed no-op if the operation
is already terminal. After intent, the eventual outcome is `cancelled`, or
`succeeded`/`failed` if the underlying effect won the race; reconciliation
after a crash follows the ledger, the same durable source of truth the rest of
the saga uses.

**Parent receipt of a result is a ledger fact, not a side-channel.**
`SessionClosed` gains an optional `ArtifactRef result_ref`, a claim-check to the
session's final output. The parent's durable receipt of a child or external
result is that delegation operation's `OperationOutcomeRecorded` on the
*parent's own* stream, written by the reconciler; the `At` guard plus
one-terminal-outcome-per-operation makes redelivery of that receipt idempotent.
Delivering a result to a still-running attempt in real time is derived from the
ledger rather than a separate durable event, and can be added additively later
if needed.

`OperationKind` value 2 is renamed `OPERATION_KIND_CHILD_SESSION_DELEGATION`,
and gains `OPERATION_KIND_EXTERNAL_DELEGATION = 3`. Model requests explicitly
do not use the operation ledger in `v1alpha1`: their retry and billing identity
is `message_id` plus attempt, recorded via the message events and
`TokenUsage`, not the ledger this facet otherwise governs.

### 7. The log is never truncated: keep-forever, with a read-time redaction and erasure contract

An append-only session log is the immutable source of truth, so it is never
truncated or purged (forced decision #7): no event ever leaves the log. This is
"never edit an event" taken to its end -- deletion is just a slower edit -- and it
matches the two purest event-sourced products in the corpus (T3 Code, OpenCode),
which also keep history unbounded. Keep-forever is deliberate, not naive, so it
needs an explicit privacy and redaction contract rather than leaving "delete" to
mean something the log cannot actually do.

**`SessionHidden` replaces `SessionDeleted`.** The event is renamed (file
`session_hidden.proto`) because the old name promised erasure the log does not
perform. It is a terminal visibility tombstone with a typed reason
(`SESSION_HIDDEN_REASON_USER_REQUESTED`, `SESSION_HIDDEN_REASON_RETENTION_POLICY`):
it removes the session from every default surface and still cascades as a
terminal marker (facet 6), but nothing about it deletes bytes.

**`RedactionApplied` masks at read time, keep-forever underneath.** Reinstated
`RedactionApplied{session_id, repeated string redacted_event_ids, reason}`
(`At`-guarded): the fold and every projection mask the targeted events'
content at read time, while the original bytes remain on the keep-forever log.
Because the identity contract (facet 2) makes a redelivered duplicate share one
deterministic event id, redacting by event id automatically covers every
duplicate of that event too -- there is no second copy under a different id for
a masking pass to miss. Redacting a source stream also automatically masks
every fork's inherited context, because a fork reads source events by
reference rather than by copy (facet 5): there is exactly one place the bytes
live, so exactly one redaction covers every view of them. Redacting an event
at or before an admitted harness recovery checkpoint's `covers_through` also
makes that checkpoint ineligible for restoration (facet 3), so sealed harness
state never resurrects masked content either; restoration falls back to
authoritative replay, which applies the mask from the first fact.

**`ArtifactErased` separates artifact-byte lifecycle from event-log retention.**
New event `ArtifactErased{session_id, artifact_id, reason}` (`At`-guarded)
records out-of-band destruction of claim-checked artifact bytes; the
artifact's digest and metadata remain on the log as provenance even after the
bytes themselves are gone.

**Ingress rules keep secrets out in the first place.** Credential-bearing URLs,
signed URLs, and other secrets are prohibited in durable fields;
`ExternalArtifact.source_url` must be credential-free, and ingress
secret-scanning is a command-boundary validation obligation (facet 3), not a
downstream cleanup step.

**Erasure-grade deletion is explicitly deferred, not silently dropped.** Legal
or user-requested erasure beyond masking -- per-session encryption and key
destruction, interacting with the key-custody ADRs -- is deferred to a named
follow-up ADR. The interim story for `v1alpha1` is redaction masks plus
artifact-byte erasure, not cryptographic shredding.

**This explicitly supersedes [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)
for session streams.** Session streams never issue the
[ADR#0029](./0029-decider-retention-and-truncation-watermark.md) purge; its
`MinimumRequiredSequence` [retention watermark](../glossary/retention-watermark)
stays a read-only diagnostic for this store, and this ADR states that
supersession explicitly rather than leaving the two decisions in tension.

Storage is otherwise managed without ever removing a fact:

- **Aggregate snapshots bound replay, not storage.** The runtime resumes the
  Session aggregate from the newest snapshot and replays only the tail after it
  (facet 8), so a long session costs O(tail) to load even though its log grows
  forever. This does not restore process-local harness state.
- **Cold-storage tiering is an optional, reversible, non-semantic relocation.** If a
  deployment must bound the hot JetStream stream, already-immutable old events may be
  copied to the JetStream Object Store, evicted from the hot stream, and restored on
  demand through [ADR#0013](./0013-origin-stream-sequence-header.md)'s restore path (the one authorized use of
  `Trogon-Origin-Stream-Sequence`). This moves bytes between tiers; it never edits or
  logically deletes an event, and whether to enable it is deferred to deployment, not
  decided here.

Because nothing is ever logically removed, the machinery an archive-then-purge
design would need -- a verified-archive KV ledger, a fork-lineage watermark term so a
live fork's source prefix is not purged, and the purge-versus-new-fork race -- simply
does not arise. A fork's source prefix is always present.

### 8. Listing, search, and summaries are rebuildable projections, never the source of truth

No read model is authoritative. A `Projector::catch_up` folds the stream into a
`SessionProjection` (`projections/v1`) that denormalizes the fields a picker
needs, checkpointing `last_applied_stream_position` after each event exactly as
the scheduler does. Queries are `verb + noun` Rust functions over the KV
projection ([ADR#0014](./0014-command-and-query-naming.md)) -- `get_session`,
`list_sessions` -- with no query protos, since the projection value is the read
contract. The model-visible context is compiled deterministically from the event
log bounded by the latest `Compacted` marker (facet 4), ratifying the prior art's
context-twin/token-budget compiler as a projection, and, for a fork, recursively
resolving its source's context prefix the same way (facet 5). Any full-text or
vector search subsystem is a separate, independently bootstrapped projection off
the same log, out of scope here.

Resume rebuilds a session's state the way the runtime rebuilds any decider
aggregate: load the newest snapshot for the session, then replay only the tail
after it -- a fork resumes purely from its own child-stream snapshot, since it
never folded source events into its aggregate state to begin with (facet 5) --
so aggregate resume cost tracks snapshot cadence, not transcript length. If the
platform harness supports restoring process-local in-flight state, that
continuation uses an admitted harness recovery checkpoint under facet 3. A
missing or invalid checkpoint never changes what the event log says happened;
the platform replays authoritative history and starts a fresh attempt. Only
incomplete authoritative history or an indeterminate side effect that requires
reconciliation can block recovery.

### Command-by-command matrix

Every command implied by the 41-arm event catalog, with the state `decide`
reads, its write precondition (facet 2), the batch it emits, the invariant it
guards, and its idempotency key. Rows are deliberately terse; exact command
proto shapes are implementation-level follow-up (Non-Goals below). Execution
attempts stay inside the Session aggregate rather than a separately-guarded
attempt aggregate, because a `decide` names exactly one stream and attempt
facts must be guarded against the same session's lifecycle in one atomic
decision.

| Command | State read | Precondition | Emitted batch | Invariant | Idempotency key |
| --- | --- | --- | --- | --- | --- |
| `CreateSession` | none | `NoStream` | `[SessionStarted]` | stream must not exist | `session_id` |
| `ForkSession` | source existence | `NoStream` (child) | `[SessionStarted, SessionForked]` | child stream must not exist; source must exist | `session_id` (child) |
| `DispatchDelegation` | parent head | `At` (parent) | `[DelegationDispatched]` | parent not already terminal | `operation_id` |
| `CreateChildSession` (saga step) | child stream existence | `NoStream` (child) | `[SessionStarted, ParentLinked]` | child stream must not exist; `parent_dispatched_at` and `cascade_policy` copied verbatim from the parent's `DelegationDispatched` | `operation_id` |
| `CloseSession` | head | `At` | `[SessionClosed]` | not already terminal | `session_id` + terminal-request id |
| `CancelSession` | head | `At` | `[SessionCancelled]` | not already terminal | `session_id` + terminal-request id |
| `FailSession` | head | `At` | `[SessionFailed]` | not already terminal | `session_id` + terminal-request id |
| `HideSession` | head | `At` | `[SessionHidden]` | none beyond head match | hide-request id |
| `RewindSession` | head | `At` | `[SessionRewound]` | `keep_through` within current log | rewind-request id |
| `CompactSession` | head | `At` | `[Compacted]` | `covers_from <= covers_through`, ordered, in-session | compaction-request id |
| `StartExecutionAttempt` | head, active-attempt state, effective history, checkpoint evidence | `At` | `[ExecutionAttemptStarted]` | one active attempt; monotonic attempt number; restored checkpoint exactly equals first admitted evidence; `covers_through` remains effective, with no later redaction or artifact erasure reaching at or before it; restore and tail replay through the selected head precede Ready | attempt id |
| `MarkExecutionAttemptReady` | head, attempt state | `At` | `[ExecutionAttemptReady]` | Ready only after Started and, for restore, verified artifact recovery plus effective-tail replay through the start head | attempt id |
| `EndExecutionAttempt` | head, attempt state | `At` | `[ExecutionAttemptEnded]` | one outcome per attempt | attempt id |
| `ApproveToolCall` | head, tool-call state | `At` | `[ToolCallApproved]` | mutually exclusive with deny | `tool_call_id` |
| `DenyToolCall` | head, tool-call state | `At` | `[ToolCallDenied]` | mutually exclusive with approve; blocks start/complete | `tool_call_id` |
| `ReserveOperation` | head, ledger state | `At` | `[OperationReserved]` | one reservation per `operation_id` | `operation_id` |
| `RecordOperationOutcome` | head, ledger state | `At` | `[OperationOutcomeRecorded]` | one terminal outcome per `operation_id`; `unknown` supersedable once | `operation_id` |
| `RequestOperationCancellation` | head, ledger state | `At` | `[OperationCancellationRequested]` | rejected if operation already terminal | `operation_id` + cancellation-request id |
| `DispatchExternalDelegation` | head | `At` | `[ExternalDelegationDispatched]` | one dispatch per `operation_id` | `operation_id` |
| `ReconcileParentTerminal` (reconciler) | child head, parent lineage | `At` (child) | `[ParentTerminated, SessionCancelled]` | no-op if child already terminal | `triggering_event_id` |
| `ReconcileParentRewind` (reconciler) | child head, parent lineage | `At` (child) | `[ParentHistoryInvalidated, SessionCancelled]` | only if `parent_dispatched_at > keep_through` and cascade policy allows | `triggering_event_id` |
| `DetachDelegation` (parent side) | parent head | `At` (parent) | `[DelegationDetached]` | one detach per `detach_operation_id` | `detach_operation_id` |
| `DetachDelegation` (child side / repair) | child head | `At` (child) | `[ParentDetached]` | one detach per `detach_operation_id` | `detach_operation_id` |
| `ApplyRedaction` | head | `At` | `[RedactionApplied]` | targeted event ids exist in-session | redaction-request id |
| `EraseArtifact` | head | `At` | `[ArtifactErased]` | artifact exists and is claim-checked | `artifact_id` + erasure-request id |
| `RecordUserMessage` | none | `Any` | `[UserMessageRecorded]` | role is `USER` | `message_id` |
| `StartAssistantMessage` | none | `Any` | `[AssistantMessageStarted]` | none | `message_id` |
| `CompleteAssistantMessage` | none | `Any` | `[AssistantMessageCompleted]` | role is `ASSISTANT`; id/model agree with start | `message_id` |
| `FailAssistantMessage` | none | `Any` | `[AssistantMessageFailed]` | id references a started message; first terminal outcome per id wins | `message_id` |
| `RequestToolCall` | none | `Any` | `[ToolCallRequested]` | none | `tool_call_id` |
| `StartToolCall` | none | `Any` | `[ToolCallStarted]` | `tool_call_id` matches a request | `tool_call_id` |
| `CompleteToolCall` | none | `Any` | `[ToolCallCompleted]` | first-terminal-outcome-wins vs. `ToolCallFailed` | `tool_execution_id` |
| `FailToolCall` | none | `Any` | `[ToolCallFailed]` | first-terminal-outcome-wins vs. `ToolCallCompleted` | `tool_execution_id` |
| `RecordArtifact` | none | `Any` | `[ArtifactRecorded]` | source oneof set; MIME fallback rule | `artifact_id` |
| `RecordFileChange` | none | `Any` | `[FileChanged]` | `RENAMED` requires `previous_path`; others omit it | change id |
| `ProduceCheckpoint` | settled history, plan, producing attempt | `Any` | `[CheckpointProduced]` | artifact admissible; attempt and plan match; `covers_through` settled; first evidence wins per `checkpoint_id` | `checkpoint_id` + canonical digest of the complete checkpoint evidence |
| `RecordSystemNotice` | none | `Any` | `[SystemNoticeRecorded]` | none | notice id |
| `UpdateTodo` | none | `Any` | `[TodoUpdated]` | `revision` monotonic from single logical writer; highest-revision-wins fold | `session_id` + `revision` |
| `RenameSession` | none | `Any` | `[SessionRenamed]` | none | rename-request id |
| `ArchiveSession` | none | `Any` | `[SessionArchived]` | none | archive-request id |
| `UnarchiveSession` | none | `Any` | `[SessionUnarchived]` | none | unarchive-request id |

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
not exist in `trogon-decider-nats`, for no benefit over facet 5's
reference-by-context-projection, which achieves the same shared prefix without
importing any of it into child aggregate state.

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
- **Runtime and command implementation.** Concrete message fields, the event
  envelope, and the idempotency-key representation are now decided in the
  `v1alpha1` proto package (facets 1-7 above); what remains implementation-level
  follow-up is commands, aggregate `initial_state`/`evolve`/`decide`,
  projections, aggregate snapshots, harness recovery checkpoint
  admission, and the substrate obligations facet 2 lists as prerequisites.

## Consequences

- A Session gains rewind and audit for free from event-sourcing, plus real
  expected-sequence optimistic concurrency on its creation and invariant-bearing
  transitions -- a narrower form of the guarantee only one studied product had
  (OpenCode guarded every append; here creation and invariant-bearing
  transitions are, per the `NoStream`/`At`/`Any` classification of facet 2) --
  while the high-volume transcript path appends without a head guard and never
  retries.
- Durable cross-references (`context_prefix_boundary`, `keep_through`,
  `covers_from`/`covers_through`, `parent_dispatched_at`) are now `SessionOrdinal`
  fold-derived positions, not physical JetStream sequences, so they survive
  restore, backfill, migration, and cold-tier relocation without needing a
  translation layer (facet 2); [ADR#0013](./0013-origin-stream-sequence-header.md)'s
  origin-sequence header remains purely a provenance concern for these
  payloads, never a domain reference.
- The event log remains authoritative when an aggregate snapshot or harness
  recovery artifact is missing. A bad aggregate snapshot falls back to replay;
  a bad harness recovery checkpoint falls back to replay and a fresh attempt.
  Only incomplete authoritative history or an indeterminate side effect can
  prevent that recovery path.
- Because commuting facts append without a head guard, two conflicting outcomes
  for the same entity can both land on the log (a `ToolCallCompleted` and a
  `ToolCallFailed` for the same `tool_execution_id`, say). The fold resolves
  these deterministically -- first-terminal-outcome-wins per entity,
  highest-revision-wins for `TodoUpdated`, first terminal marker wins for the
  session itself -- rather than the store rejecting either at write time
  (facet 2); every read model over the transcript must be written to honor
  these fold rules, not just tolerate late facts.
- No caller migration is needed on the curated line (greenfield). The platform
  `crates/session` domain model is salvaged; its persistence is rewritten as a
  decider, and the switching subsystem is dropped pending [ADR#0031](./0031-agent-implementation-and-session-plan.md).
- New standing services appear: a dispatch-and-detach saga reconciler with
  crash repair for both directions (facet 6), a rewind-invalidation cascade
  distinct from terminal cascade (facet 6), plus a scheduled orphan-closure
  sweep; optional cold-storage tiering (facet 7) adds one more, but only if a
  deployment chooses to bound the hot stream.
- Gets harder: adding an event type now carries a proto plus a typed-decode plus
  a projection-fold obligation, and every command must carry an idempotency key
  its event ids are deterministically derived from (facet 2), which cannot
  happen until the substrate obligations facet 2 lists (evolve-visible
  identity, WASM parity, duplicate-ack success) are actually implemented --
  this store does not go live on wire contract alone.
- Cascade is eventually consistent: a child spawned concurrently with a parent's
  terminal marker may run a reconciler cycle or two before it is cancelled, and a
  deep collaboration chain cascades in O(depth) reconciler round-trips; callers
  expecting synchronous cascade are surprised. Graph acyclicity is guaranteed by
  construction, not by a runtime check (facet 6), and detach is now two
  independently-guarded local facts joined by `detach_operation_id` rather than
  a mirrored write, closing the [ADR#0024](./0024-agent-platform-stream-topology.md)
  record-once violation the prior design had.
- Fork replay is O(child events) only, regardless of fork depth or
  fork-of-fork chaining, because the child aggregate never folds source events
  at all; the sealing snapshot the prior design required is no longer a
  correctness obligation. The prefix-walk cost of a deep fork-of-fork chain
  moved to the context projection (facet 5, facet 8), where it is a read-side
  caching concern like any other projection, not a write-side replay cost. The
  log grows unbounded -- nothing is ever purged -- so storage is a
  capacity-planning concern, bounded in replay cost by snapshots and, if a
  deployment opts in, in hot-stream size by reversible cold-tiering.
- Privacy now has an explicit contract instead of an absent one: `SessionHidden`
  is honestly a visibility tombstone, `RedactionApplied` and `ArtifactErased`
  give the store a read-time masking and artifact-destruction story, and
  [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)'s purge is
  explicitly superseded for session streams (facet 7). Erasure-grade deletion
  (crypto-shredding) is still a named gap, deferred to a follow-up ADR, not
  silently unresolved.
- This decision depends on five still-draft ADRs (0026, 0027, 0028, 0029,
  0031); each that changes before acceptance can reopen the facet that builds
  on it. The package is named `v1alpha1`, not `v1`, precisely because of that
  dependency and because the substrate obligations facet 2 lists are not yet
  met; promotion to `v1` is a later, separate decision. Shared multi-tenant
  deployment additionally waits on [ADR#0027](./0027-decider-multi-tenancy-primitive.md)'s
  resolver contract (facet 1).

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

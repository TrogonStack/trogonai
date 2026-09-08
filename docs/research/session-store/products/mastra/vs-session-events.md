# Mastra compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Mastra](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 11/12**: evolution scars 3/3 (`OM_MIGRATION_COLUMNS`, a
15-entry backward-compatibility column list carried forward on every init,
`stores/pg/src/storage/domains/memory/index.ts:40-55`, wired into an
`alterTable` call at `stores/pg/src/storage/domains/memory/index.ts:210-215`;
a v1/v2 message-shape migration with a read-time conversion shim, per the
dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning)
section), operational age 3/3 (two independently corroborated, externally
filed, closed production incidents, each fixed with a code comment naming the
issue number: `stores/pg/src/storage/domains/memory/index.ts:197-202` names
issue #18298, a bundler-induced self-referential-import deadlock in `mastra
build` output, confirmed closed via `gh issue view 18298 --repo
mastra-ai/mastra` (created 2026-06-22); `stores/pg/src/storage/domains/memory/index.ts:808-810`
names issue #11150, a `ROW_NUMBER()`-based pagination query that caused
multi-minute scans on large `mastra_messages` tables, confirmed closed via
`gh issue view 11150 --repo mastra-ai/mastra` (created 2025-12-13); both are
real, technically specific field failures with a shipped fix, not merely a
defensive comment), exposure 3/3 (Apache-2.0, 19+ backend storage adapters
including Postgres, LibSQL, DynamoDB, MongoDB, and others per the dossier's
[Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery)
section; every backend surveyed here is a network database, so multi-client
access from multiple hosts is load-bearing production behavior, not a toy
default; the `ee/` carve-out in `LICENSE` (lines 3-7) is independent
confirmation that a paid tier is built on top of this same open-source store),
design independence 3/3 (no evidence anywhere in the surveyed code of
persistence logic forked from an upstream project; the store is Mastra's own
`MemoryStorage` domain contract, and the divergent atomicity guarantees across
its own backends, documented below, are themselves evidence of organic,
independent per-backend evolution rather than a single inherited design).
Scored one point below Letta (11/12) would be an over-correction: the
evidence here is comparably strong, but is held to 11/12 rather than 12/12
because operational-age evidence, while externally corroborated, is drawn
from a `stores/pg` implementation file whose current domain-refactored shape
is comparatively recent relative to the two-incident sample size; the
externally-verified incidents earn the third point on that axis, but the
overall score is reported conservatively rather than at the ceiling.

## The one structural difference everything else follows from

Mastra defines exactly one storage contract, `MemoryStorage`
(`packages/core/src/storage/domains/memory/base.ts:38-134`), and then lets
each of its many backend packages implement that contract with materially
different atomicity guarantees, silently, at the backend level. This is not
a hypothetical concern: it is directly observable across the four backends
this comparison surveyed in depth.

`stores/pg/src/storage/domains/memory/index.ts:1355-1447` wraps `saveMessages`
in a real Postgres transaction (`this.#db.client.tx(async t => {...})`,
opened at line 1401), batching the message inserts and the thread
`updatedAt` bump inside one commit. Its `deleteThread`
(`stores/pg/src/storage/domains/memory/index.ts:765-803`) does the same:
message deletion, a scan of `pg_tables` for `memory_messages%` vector
tables, per-table vector-row purges, and the thread-row delete all happen
inside one `this.#db.client.tx(...)` call.

`stores/libsql/src/storage/domains/memory/index.ts:1356-1381` deliberately
does not use a transaction for `deleteThread`, and says so in a code
comment at lines 1358-1361: "Not using a transaction to avoid SQLITE_BUSY
errors when multiple deleteThread calls run concurrently. The two deletes
are independent and orphaned messages (if thread delete fails) would be
cleaned up on next delete attempt." Atomicity is traded away explicitly, in
writing, for concurrent-write availability.

`stores/dynamodb/src/storage/domains/memory/index.ts:599-671` writes each
message in `saveMessages` with a sequential `.put().go()` call
(line 644), and on any failure runs a compensating rollback loop
(lines 647-658) that deletes the messages already written. That rollback's
own failure path does not re-throw: it only logs
(`this.logger.error('Failed to rollback message during save error', ...)`
at lines 651-655), so a rollback that itself fails leaves partially-written
state with no propagated error.

`stores/mongodb/src/storage/domains/memory/index.ts:695-760` wraps
`saveMessages` in `this.#connector.withTransaction(...)`, but that
connector method degrades silently when the deployment topology does not
support it. `stores/mongodb/src/storage/connectors/MongoDBConnector.ts:117-136`
documents this exactly in its own doc comment: "Runs `fn` inside a
transaction when the deployment supports it... On a standalone server (or
custom handler) it degrades to running `fn` directly with an undefined
session: best-effort sequential, no atomicity." `deleteThread`
(`stores/mongodb/src/storage/domains/memory/index.ts:1212-1238`) goes
further and never even attempts a transaction, with a comment (lines
1214-1222) explaining that a transactional `deleteMany` is capped by
`transactionLifetimeLimitSeconds` (60s default) and "a large thread would
abort and become permanently undeletable," so a plain, non-transactional
`deleteMany` is used instead because it "commits incrementally and always
completes."

Four implementations of one interface, four different, independently
justified answers to "is this write atomic": always (pg), never by design
(libsql), sequential-with-fallible-compensation (dynamodb), and
topology-conditional-with-silent-degradation (mongodb). This is not a
mistake in any one backend; each comment is a reasoned, backend-specific
trade-off. It is the natural consequence of a single abstract interface
sitting on top of storage engines with genuinely different transaction
models, and no mechanism in the interface itself communicates which
guarantee a caller is actually getting for a given deployment.

We do not have this problem, by construction rather than by discipline.
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1 makes append-only mutation through `append_stream` the
only write path, and decision 2's `WRITE_PRECONDITION` classification
(`NoStream`/`At`/`Any`) is enforced once, at the substrate level, for every
command, not re-implemented per backend. Every multi-event fact our design
ever needs atomically is expressed as a single batch append under one
precondition on one stream: fork is `[SessionStarted, SessionForked]` under
`NoStream` (decision 5, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 672-674), child creation is
`[SessionStarted, ParentLinked]` under `NoStream` (decision 6, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines
733-734), and terminal or rewind cascade are `[ParentTerminated,
SessionCancelled]` and `[ParentHistoryInvalidated, SessionCancelled]`, each
under `At` on the child's own stream (decision 6, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 766-787).
There is exactly one substrate (NATS JetStream) and exactly one atomicity
guarantee (single-stream, single-writer, OCC-guarded append), so the
question "which atomicity do I actually get here" never has more than one
answer. Mastra's four-way divergence is the cleanest evidence in this
corpus that "one store interface" is not the same claim as "one storage
guarantee," and that the second claim is the one that actually matters at
the write path.

## Mapping

Two words need disambiguating before the table, because a naive lookup
would silently misread both:

- **"Thread."** Mastra's `StorageThreadType`
  (`packages/core/src/memory/types.ts:35-50`, confirmed no `parentThreadId`
  field) is the closest analogue to our Session, but it is deliberately
  thin: a thread has no execution-plan binding, no lifecycle state machine,
  and no terminal state. Our `SessionStarted` (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:1-25`)
  binds a session to a `StoredSessionExecutionPlan` and a `WorkspaceRef` at
  creation, and a session reaches one of four terminal states
  (`SessionClosed`, `SessionCancelled`, `SessionFailed`, `SessionHidden`).
  Mastra's thread has none of this: it is a container for messages, not a
  bounded, terminable execution.
- **"Fork."** Mastra's `cloneThread`
  (`stores/pg/src/storage/domains/memory/index.ts:1745-1790`) is a genuine,
  observed physical copy: it fetches the source thread, mints a new thread
  ID, and inside a transaction copies matching message rows into new rows
  under the new thread ID (per `StorageCloneThreadInput`/`StorageCloneThreadOutput`,
  `packages/core/src/storage/types.ts:215-252`). Our `SessionForked`
  (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:1-39`) never copies an event: it appends a
  `context_prefix_boundary` (`SessionOrdinal`) that the model-visible-context
  projection resolves by reference into the source stream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
  5, lines 683-696). Mastra's word for the same English verb names the
  opposite mechanism.

| Mastra | Ours | Verdict |
| --- | --- | --- |
| `StorageThreadType` (`packages/core/src/memory/types.ts:35-50`) | `SessionStarted` (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:1-25`) plus lifecycle events | Semantic mismatch, see above; Mastra's thread has no plan binding and no terminal state |
| `mastra.generateId()` falling back to `randomUUID()` (`packages/core/src/mastra/index.ts:1128-1143`) | `SessionId` opaque identity ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2, lines 140-141) | Equivalent principle: identity is an opaque key, not a derived or structured value |
| `Message.id`, ordered by `createdAt` with per-query, sometimes inconsistent tiebreaks, see below | `CanonicalMessage.message_id` (`proto/trogonai/session/sessions/v1alpha1/message.proto:1-40`); order is `SessionOrdinal`, fold-derived, never a stored or queried column (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto:1-16`) | Ours, decisively: see recommendation 3 |
| `cloneThread` physical copy (`stores/pg/src/storage/domains/memory/index.ts:1745-1790`) | `SessionForked{source_session_id, context_prefix_boundary}` (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:1-39`), inherited by reference ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 5) | Semantic mismatch, see above; trade-off, see below |
| Cosmetic thread-ID string concatenation for agent-to-agent delegation, `` `${threadId}-${randomUUID()}` `` (`packages/core/src/agent/agent.ts:4716-4725`), no consumer reconstructs hierarchy from it | `DelegationDispatched`/`ParentLinked` (`proto/trogonai/session/sessions/v1alpha1/delegation_dispatched.proto:1-26`, `proto/trogonai/session/sessions/v1alpha1/parent_linked.proto:1-28`), a typed, fold-consumed fact pair | Ours, decisively: see "What our design already does better" |
| `forkedSubagent`/`parentThreadId` opaque metadata tags on a cloned thread (`packages/core/src/agent-controller/agent-controller.ts:1845-1861`), read back only by `listThreads`'s default filter (`packages/core/src/agent-controller/agent-controller.ts:1005-1013`) | Same as above; `CascadePolicy` (`proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto:1-17`) governs whether a link matters for cascade at all | Ours, decisively: see below |
| `SessionRecord.parentSessionId`/`subagentDepth`, real migrated nullable schema columns (`packages/core/src/storage/constants.ts:447-448`, `packages/core/src/storage/domains/harness/types.ts:26-27`) with zero producers or consumers found anywhere in `packages/core/src` outside their own declarations | Same as above | Ours, decisively: a third, mutually-unaware mechanism is exactly the failure mode our single typed fact pair avoids |
| `HarnessStorage` has no `deleteSession` at all; deletion is `updateSession(id, {deletedAt: new Date()})`, a soft-delete-by-convention not enforced by any schema rule (`packages/core/src/storage/domains/harness/base.ts:1-91`) | `SessionHidden` (`proto/trogonai/session/sessions/v1alpha1/session_hidden.proto:1-26`), a typed, named terminal visibility tombstone with a `SessionHiddenReason` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively: see below |
| `MessageHistory` processor: bounded eager reload, `perPage: this.lastMessages` (default 10), `orderBy: {field: 'createdAt', direction: 'DESC'}` (`packages/core/src/processors/memory/message-history.ts:113-119`, default at `packages/core/src/memory/memory.ts:82-83`) | Aggregate resume folds from the newest snapshot plus tail ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8, lines 943-953); model-visible context compiles from the log bounded by the latest `Compacted` marker (decision 8) | Trade-off, see below |
| `RetentionConfig`/`TableRetentionPolicy`/`PruneOptions`, age-based, table-granular, caller-scheduled, never auto-run (`packages/core/src/storage/retention.ts:19-97`); `prune()` "never reclaims disk" (`packages/core/src/storage/retention.ts:48-51`) | `SessionHidden`/`RedactionApplied`/`ArtifactErased`, a three-tier privacy contract over a keep-forever log ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Different problem, see "The two gaps" below |
| No compaction primitive at the storage layer at all, per the dossier's [Compaction and history management](./index.md#compaction-and-history-management) section | `Compacted` (`proto/trogonai/session/sessions/v1alpha1/compacted.proto:1-50`), a self-sufficient in-stream marker the store only records ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 4) | Gap in Mastra, by the dossier's own account |
| `HarnessPendingItemRecord`/pending-item array on `SessionRecord` (`packages/core/src/storage/domains/harness/types.ts:7-19`) | `ToolCallApproved`/`ToolCallDenied` (facet 2, command matrix) | Roughly equivalent, different granularity: Mastra's is a mutable array field, ours is an append-only fact per call |

## What we should consider changing

Ordered by how consequential the underlying question is, not by
implementation cost.

### 1. Add an explicit lint or test asserting every listing/projection path sorts strictly by `SessionOrdinal` with no alternate tiebreak

**The change.** Add a repo-level check (test or lint rule) that any code
reading events for a session in order sorts strictly by the fold-derived
`SessionOrdinal` and never introduces a second, independently-chosen sort
key or tiebreak for the same logical sequence.

**Evidence anchor.** Mastra, store maturity 11/12:
`stores/mongodb/src/storage/domains/memory/index.ts:289` sorts
`{ createdAt: -1, id: -1 }` (a tiebreak present), line 298 sorts
`{ createdAt: 1, id: 1 }` (a tiebreak present, opposite direction), but line
322 (`listMessagesById`) sorts only `{ createdAt: -1 }` (no tiebreak) and
line 1322, inside `cloneThread`'s message query, sorts only
`{ createdAt: 1 }` (no tiebreak). Four call sites in one backend file, over
the same conceptual ordering, with three distinct sort shapes. Where two
messages share a `createdAt` value (a real possibility if a caller does not
control clock resolution, or on bulk-imported/migrated rows), the
no-tiebreak call sites have no defined order between them, and the
tiebreak call sites disagree with each other about which field breaks the
tie and in which direction.

**Blast radius.** Additive. This does not touch a proto file or an ADR
decision; it is a new automated check against existing behavior we already
intend ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2, lines 140-154: `SessionOrdinal` is "derived by
counting at fold time, never read from JetStream message metadata").

**Why.** Our fold-derived `SessionOrdinal` already makes this class of bug
structurally hard to introduce inside `evolve`, because ordering comes from
counting during a single, canonical fold, not from a query-time `ORDER BY`
clause chosen independently at each call site. But `SessionOrdinal` is a
domain-level guarantee; nothing today stops a future read-side projection
(decision 8) from adding its own `ORDER BY` against a denormalized KV
store and getting it subtly wrong the same way Mastra's four call sites
did, independently, inside one backend. Mastra is not evidence that our
design has this bug; it is evidence of exactly how easily it is introduced
when nothing enforces a single, canonical ordering rule at every read site,
which is worth guarding against explicitly rather than trusting to review.

**What it costs us.** One test or lint rule to write and maintain; no
schema or behavior change to reject.

### 2. Confirm that our multi-event atomic batches (fork, child creation, cascade) remain single-stream, single-append operations as the command surface grows, and do not acquire a Mastra-style "compensating rollback across independent writes" shape

**The change.** No change to [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) today. This is a standing constraint
to preserve: every future command that needs to make more than one fact
true atomically must express that as a single batch append under one
`WRITE_PRECONDITION` on one stream (as fork, child creation, and cascade
already do), never as multiple independent appends across streams with an
application-level rollback if a later one fails.

**Evidence anchor.** Mastra, store maturity 11/12: DynamoDB's `saveMessages`
(`stores/dynamodb/src/storage/domains/memory/index.ts:599-671`) writes
each message with an independent `.put().go()` call and, on failure midway,
runs a compensating delete loop over the messages already written
(lines 647-658) whose own failure path only logs
(`this.logger.error(...)`, lines 651-655) rather than propagating an error.
This is the shape a design gets when a logical multi-item write has no
single atomic primitive underneath it: correctness depends on a
best-effort undo that itself can silently fail.

**Blast radius.** Additive as a standing principle; it names no specific
schema change today. It becomes breaking-the-decision only if a future
proposal tries to relax it, in which case [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decisions 5 and 6 (the
`[SessionStarted, SessionForked]` and `[SessionStarted, ParentLinked]`
atomic batches) are the decisions being contradicted.

**Why.** Every multi-fact atomic operation [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) currently defines stays
inside one JetStream subject specifically because "JetStream offers no
atomic write across subjects" (Alternatives Considered: "Subagent cascade
via a cross-stream transaction or atomic multi-stream delete... rejected
because it is unavailable"). That constraint already forced the right
design for the cases we have. The risk this recommendation names is a
future command that needs to touch two streams' invariants at once, where
a well-intentioned implementer reaches for "write both, and roll back the
first if the second fails" instead of restructuring the operation as
two separately-guarded local facts joined by an operation id, the pattern
decision 6 already uses for detach (`DelegationDetached`/`ParentDetached`,
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 793-808). DynamoDB's rollback-that-can-itself-fail is the
concrete shape of what happens when that discipline lapses.

**What it costs us.** Nothing today; this recommendation exists to make the
constraint explicit enough that recommendation 2 is reached for
deliberately, not rediscovered after a future command's compensating
rollback fails in production.

### 3. Do not introduce a second, mutable parent-child linking field for delegation without deprecating the others, if our system ever grows more than one

**The change under consideration, and why to reject it as a general
practice.** A future feature (a debugger view, an export tool, an
analytics pipeline) proposing its own denormalized, independently-populated
field for "which session is this session's parent," separate from the
`ParentLinked`/`DelegationDispatched` fact pair.

**Evidence anchor.** Mastra, store maturity 11/12: three mutually-unaware
mechanisms coexist for the same concept. (1) Cosmetic thread-ID
concatenation, `` `${inputData.threadId}-${randomUUID()}` `` and
`` `${inputData.resourceId}-${agentName}` ``
(`packages/core/src/agent/agent.ts:4716-4725`), with no consumer anywhere in
`packages/core/src` reconstructing hierarchy from the string shape. (2) The
`cloneThreadForFork` mechanism (`packages/core/src/agent-controller/agent-controller.ts:1845-1861`),
which tags a cloned thread's opaque `metadata` JSON column with
`forkedSubagent: true` and `parentThreadId`, read back in exactly one place
(the `listThreads` default filter, lines 1005-1013) plus one more
independent tagging call site for a different execution path
(`packages/core/src/loop/workflows/agentic-execution/goal-step.ts:308-310`).
(3) A formally-typed `SessionRecord.parentSessionId`/`subagentDepth` schema
column pair (`packages/core/src/storage/constants.ts:447-448`,
`packages/core/src/storage/domains/harness/types.ts:26-27`) with zero
producers or consumers anywhere in `packages/core/src` outside their own
declarations, confirmed by grep. None of the three mechanisms reads or
writes either of the others.

**Blast radius.** Additive as a standing principle; no schema change is
proposed. It would become breaking-the-decision only if adopted, since
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already names `DelegationDispatched`/`ParentLinked` as
the sole parent-child linking mechanism, and a second field would
contradict that by construction.

**Why.** None of Mastra's three mechanisms is individually a bad idea; each
was reasonable in isolation, at the moment it was added, for its own
call site. The failure is that nothing forced a single point where "does
this session have a parent" gets answered, so three answers now coexist,
one of which (the harness schema columns) is populated nowhere and reads as
"aspirational" rather than dead, per the dossier's own [Subagents and nested
sessions](./index.md#subagents-and-nested-sessions) section, which treats
this explicitly as an open question rather than a resolved dead-code
finding. `ParentLinked`/`DelegationDispatched` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) is
already the single mechanism; this recommendation's value is naming, ahead
of time, why a second one should not get added later for a narrower
purpose (a debugger view, a fast-path query) the way Mastra's third
mechanism apparently was.

**What it costs us.** Nothing to reject a second mechanism; the cost this
guards against is the confusion of maintaining three unreconciled sources
of the same fact, which is what would need to be paid down later if this
were not stated now.

### 4. Consider allowing a bounded, cheap read path for the common "just the recent tail" resume case, distinct from full aggregate replay

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 already resumes efficiently by loading
"the newest snapshot for the session, then replay only the tail after it."
Consider whether a caller-facing read API should also expose a
lighter-weight, explicitly-bounded "last N messages" query, analogous to
Mastra's default, that does not require resolving snapshot state at all,
for callers (a chat-history preview UI, a debugging tool) that need recent
context but not full, correct aggregate state.

**Evidence anchor.** Mastra, store maturity 11/12: `MessageHistory`
(`packages/core/src/processors/memory/message-history.ts:113-119`) resumes
by calling `storage.listMessages({..., perPage: this.lastMessages,
orderBy: {field: 'createdAt', direction: 'DESC'}})`, defaulting
`lastMessages` to 10 (`packages/core/src/memory/memory.ts:82-83`). This is a
bounded eager reload, not a replay: it is a cheap, single query with a
fixed page size, unrelated to snapshot cadence or fold correctness.

**Blast radius.** Additive. This would be a new, explicitly non-authoritative
read-side query against the same log, not a change to how aggregate resume
works ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8's snapshot-plus-tail path stays exactly as
specified).

**Why it is a good idea, or why it is not.** This is recorded as a
question, not a firm recommendation, because it is not clear the platform
has a caller that needs "recent tail, no correctness guarantee" as
distinct from "correct aggregate state, snapshot-bounded cost." Mastra's
default of 10 messages is also a real footgun for anything beyond casual
display: the dossier's [Read and resume path](./index.md#read-and-resume-path)
section notes this is a bounded eager reload, not a replay, meaning a
caller relying on it for anything beyond a UI preview gets a silently
truncated view of history with no signal that truncation happened. If we
add this, it must be clearly named as a preview query, never confused with
resume, and must not become a second, informal notion of "current state" the
way Letta's `Agent.message_ids` did (see the Letta comparison,
"What our design already does better").

**What it costs us.** If adopted: a new bounded query shape and a
documentation obligation to keep it clearly distinct from resume. If
rejected: nothing; aggregate resume via decision 8 already covers the
correctness-sensitive case, and this recommendation names the gap without
asserting it must be filled.

## What our design already does better

- **A single typed fact pair for parent-child linking, not three
  independent, unreconciled mechanisms.** `DelegationDispatched`/`ParentLinked`
  (`proto/trogonai/session/sessions/v1alpha1/delegation_dispatched.proto:1-26`, `proto/trogonai/session/sessions/v1alpha1/parent_linked.proto:1-28`) are the
  only way a child-session relationship is ever recorded ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
  6). Mastra's three coexisting mechanisms (cosmetic ID concatenation,
  opaque clone metadata, unused harness schema columns) mean answering
  "does this thread have a parent" depends on which of three places you
  look, and the dossier confirms at least one of the three
  (`parentSessionId`/`subagentDepth`) has no confirmed producer or consumer
  at all.
- **Cascade on parent deletion, rewind, or crash is a named, typed policy,
  not silent orphaning.** Mastra's grep-confirmed absence of any
  `deleteThread` implementation reading `forkedSubagent`/`parentThreadId`
  means deleting a parent thread orphans its fork children with no
  detection or cleanup path, per the dossier's own inference in
  [Subagents and nested sessions](./index.md#subagents-and-nested-sessions).
  Our `CascadePolicy` (`proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto:1-17`) and the reconciler-driven
  `[ParentTerminated, SessionCancelled]`/`[ParentHistoryInvalidated,
  SessionCancelled]` batches ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6, lines 773-791, 758-771)
  make what happens on parent termination or rewind an explicit, typed,
  reconciled outcome, never a link that simply stops resolving.
- **Fork is atomic and by-reference; nothing to keep consistent, nothing to
  physically copy.** Mastra's `cloneThread` is a genuine, transactional
  physical copy of message rows (`stores/pg/src/storage/domains/memory/index.ts:1745-1790`),
  which means a large source thread makes forking an O(history) operation.
  `SessionForked{source_session_id, context_prefix_boundary}`
  (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:1-39`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 5) makes fork O(1) at
  write time and resolves inheritance by reference into a keep-forever log,
  so no fork ever needs a copy to be kept in sync with anything.
- **Deletion is a named, typed vocabulary, not one soft-delete convention
  reused everywhere.** `HarnessStorage` has no `deleteSession` method at
  all (`packages/core/src/storage/domains/harness/base.ts:1-91`); deletion
  is `updateSession(id, {deletedAt: new Date()})`, an unenforced
  soft-delete-by-convention that the in-memory reference implementation
  does not even filter on read
  (per the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions)
  section). `SessionHidden` (`proto/trogonai/session/sessions/v1alpha1/session_hidden.proto:1-26`), `RedactionApplied`
  (`proto/trogonai/session/sessions/v1alpha1/redaction_applied.proto:1-20`), and `ArtifactErased`
  (`proto/trogonai/session/sessions/v1alpha1/artifact_erased.proto:1-18`) are three distinct, typed, `At`-guarded
  events, each meaning exactly one thing ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7), so "what
  happens when you delete something" is never left to a convention a
  particular backend may or may not enforce.
- **Ordering is a single fold-derived fact, never a per-query choice.**
  Mastra's four surveyed sort call sites in one backend file disagree with
  each other about tiebreak field and direction
  (`stores/mongodb/src/storage/domains/memory/index.ts:289,298,322,1322`).
  Our `SessionOrdinal` (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto:1-16`) is derived once, at
  fold time, from a canonical order ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), so there is
  never more than one answer to "which of these two events came first."

## Trade-offs, not gaps

- **A real ACID transaction on one substrate versus JetStream's
  single-stream atomicity.** Postgres's `saveMessages` and `deleteThread`
  (`stores/pg/src/storage/domains/memory/index.ts:1355-1447, 765-803`) get a
  genuine multi-table transaction because everything lives in one
  relational database. Our atomic batches (fork, child creation, cascade)
  are real, but bounded to one JetStream subject, because our substrate
  offers no cross-subject atomic write (Alternatives Considered: "unavailable").
  Postgres's atomicity is a property of colocated storage available to one
  Mastra backend, not a design choice available to every backend it ships,
  or to us; the trade-off we accepted (single-stream atomic batches plus a
  reconciler for cross-stream facts) is the honest cost of a topology that
  does not offer that shortcut, matching the same trade-off already
  recorded against Letta's colocated-transaction cascade.
- **A bounded eager reload versus fold-from-log-with-snapshot.** Mastra's
  default resume (`MessageHistory`, `lastMessages: 10`,
  `packages/core/src/memory/memory.ts:82-83`) is cheap and simple: one
  query, fixed page size, no snapshot management. Our resume replays a
  snapshot plus tail to reconstruct correct aggregate state ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
  decision 8). Mastra's approach is cheaper for the common "just show
  recent messages" case and needs no snapshot infrastructure at all; ours
  is more expensive to build and maintain but gives a caller a
  correctness guarantee ("this is the actual, complete state") that a
  bounded reload structurally cannot: Mastra's own default silently drops
  anything older than the 10th-most-recent message, with no signal to the
  caller that truncation occurred.
- **Physical fork copy versus fork-by-reference.** `cloneThread`
  (`stores/pg/src/storage/domains/memory/index.ts:1745-1790`) gives a fork a
  fully independent, self-contained set of message rows: nothing about the
  source thread's later mutation, redaction, or deletion can affect the
  clone, because the clone owns its own copies. `SessionForked`
  (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:1-39`) is cheaper at fork time and automatically
  inherits any later redaction of the source prefix, per [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
  7's point that "redacting a source stream also automatically masks every
  fork's inherited context" (lines 879-882), but that same property means a
  fork is never fully independent of its source's continued existence and
  masking policy. Mastra's model buys isolation at the cost of copy time and
  storage; ours buys cheap forking and automatic redaction propagation at
  the cost of the fork never being a self-contained artifact.

## What not to copy

- **Letting three independent mechanisms answer the same structural
  question.** Cosmetic ID concatenation, opaque clone metadata, and unused
  harness schema columns all separately claim to represent "this session's
  parent," and none of the three is aware of the other two. If a future
  feature needs a parent-child fact we do not already have, it must extend
  `ParentLinked`/`DelegationDispatched`, never add a fourth, independent
  mechanism alongside them.
- **A schema column that looks authoritative but has no confirmed producer
  or consumer.** `parentSessionId`/`subagentDepth`
  (`packages/core/src/storage/constants.ts:447-448`) are real, migrated,
  nullable columns that read as load-bearing to anyone inspecting the
  schema, but the dossier's own grep found no call site anywhere in
  `packages/core/src` that populates or reads them. A typed field with no
  confirmed writer is worse than no field at all, because it invites a
  future reader to trust it. Every event in our catalog must have a real,
  identifiable producer before it ships.
- **A default bounded reload that silently and permanently drops history
  with no signal.** `MessageHistory`'s default of 10 messages
  (`packages/core/src/memory/memory.ts:82-83`) means any message beyond the
  most recent 10 is invisible to a resumed conversation by default, with no
  indication to the caller that truncation happened. If we ever expose a
  bounded "recent tail" read (recommendation 4), it must be explicitly
  named as a preview, never presented as equivalent to full resumed state.
- **A compensating rollback whose own failure path is silent.** DynamoDB's
  `saveMessages` rollback loop (`stores/dynamodb/src/storage/domains/memory/index.ts:647-658`)
  only logs when the rollback itself fails, rather than surfacing that
  failure to the caller. Any future compensating-action path in our system
  (a reconciler repair, a saga step) must propagate its own failure rather
  than swallowing it, exactly the discipline [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6's crash
  repair already follows (a duplicate creation attempt no-ops on
  `WrongExpectedVersion`, rather than silently continuing).
- **Silent atomicity degradation based on runtime topology, with no signal
  to the caller.** MongoDB's `withTransaction`
  (`stores/mongodb/src/storage/connectors/MongoDBConnector.ts:117-136`)
  transparently downgrades from transactional to best-effort-sequential
  depending on whether the connected deployment is a replica set or a
  standalone server, with no error, warning, or capability flag exposed to
  the caller. Any future capability that depends on the deployment topology
  in our system must fail loudly or expose its degraded mode explicitly,
  never degrade a stated guarantee silently based on what happens to be
  running underneath it.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream, linked by two typed facts recorded on each side
(`DelegationDispatched` on the parent, `ParentLinked` on the child);
terminal cascade and rewind invalidation are separate, distinct reconciled
batches, governed by `CascadePolicy`; and acyclicity holds by construction
because `DispatchDelegation` always mints a fresh `child_session_id`. The
question is whether Mastra's evidence validates, challenges, or refines
that position.

**What Mastra does when a parent is deleted, rewound, or crashes while a
child is live.** There is no rewind concept in Mastra's storage layer at
all (per the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)
section), so Mastra offers no evidence whatsoever about rewind cascade;
that half of decision 6 is untested by this comparison, not validated or
challenged. On deletion, the evidence is direct and specific: none of the
four backends' `deleteThread` implementations reads the `forkedSubagent`
or `parentThreadId` metadata keys, confirmed by grep across `stores/`
returning zero hits for either string outside `packages/core`. Concretely,
deleting a parent thread whose `packages/core/src/agent-controller/agent-controller.ts:1845-1861`-created fork children
still exist leaves those children's `metadata.parentThreadId` pointing at
a thread that no longer exists, with no detection and no cleanup path
observed in any backend's `deleteThread`. On crash: `HarnessStorage` has no
`deleteSession` and no cascade hook of any kind
(`packages/core/src/storage/domains/harness/base.ts:1-91`), and
`subagentDepth`, the one schema field that suggests an intended nesting
bound, is not incremented or checked anywhere in `packages/core/src` per
the dossier's own confirmed grep; the actual depth-one limit is enforced
by instructing the model not to recurse in a system prompt
(`packages/core/src/agent-controller/tools.ts:25`), not by any storage-layer
guard.

**Does this validate, challenge, or refine decision 6?** It validates the
core structural claim decision 6's Alternatives section already makes:
that a blind, storage-native cascade is not something to rely on for a
parent-child relationship with real invariants, and that cascade needs to
be an explicit, orchestrated concern rather than left to the substrate.
Mastra does not even attempt storage-native cascade here (no backend reads
the link metadata on delete), so unlike Letta's `RESTRICT`-plus-app-level-
orchestration precedent, Mastra is not independent positive evidence for
any particular cascade mechanism; it is evidence of what happens when no
cascade mechanism exists at all: permanent, silent orphaning, confirmed by
absence of code rather than inferred from a comment. This sharpens the
case for decision 6's reconciler-driven, transitive cascade
(`[ParentTerminated, SessionCancelled]`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 773-791) as
something the industry has not converged on even at the level of "detect
the orphaning," let alone "guard it before it happens." Mastra is silent on
rewind invalidation specifically (it has no rewind primitive to compare
against), and its prompt-text depth limit offers no evidence for or against
our construction-based acyclicity guarantee, since Mastra's limit is
advisory (a system-prompt instruction) rather than structural.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
aggregate snapshots that bound replay cost, not storage size. The question
is whether Mastra's evidence validates, challenges, or refines that design.

**What Mastra does.** Retention is real, but it is the product's
responsibility, never the store's: `RetentionConfig`/`TableRetentionPolicy`/
`PruneOptions` (`packages/core/src/storage/retention.ts:19-97`) let a caller
configure an age-based `maxAge` per table per domain, and
`MastraCompositeStore.prune()` (`packages/core/src/storage/base.ts:479-502`)
runs it, cooperatively and resumably (`maxBatches`/`maxRows`/`pauseMs`/
`AbortSignal`, `packages/core/src/storage/retention.ts:53-84`), but nothing inside the library calls
`prune()` on any schedule; a deployment must wire that up itself. Deletion
is a real, hard row delete, not a mask: `TableRetentionPolicy.maxAge`-driven
pruning permanently removes rows, and the doc comment states plainly that
`prune()` "only deletes rows: it never reclaims disk," leaving actual disk
reclamation to the underlying database and its operator
(`packages/core/src/storage/retention.ts:48-51`). There is no redaction concept anywhere in the
surveyed code: a row is either present in full or gone. Delete cascade is
also scoped narrowly, and inconsistently across backends: Postgres's
`deleteThread` is the one observed backend that also sweeps vector-index
tables (`stores/pg/src/storage/domains/memory/index.ts:772-786`, scanning
`pg_tables` for `memory_messages%`), while libsql, dynamodb, and mongodb's
`deleteThread` implementations touch only their own message and thread
tables, per the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)
section. And separate from any explicit retention policy, cascade never
crosses the fork link either way, matching the same absence of cascade
already documented under "Subagent cascade" above.

**Does this validate, challenge, or refine decision 7?** It refines
decision 7 by sharpening what the added complexity of a three-tier privacy
contract buys over Mastra's default. Mastra's model gives a caller exactly
two states for any given row: present in full, or permanently, physically
gone once a `maxAge`-based prune reaches it. There is no masking tier and
no artifact-byte-only erasure tier: `RedactionApplied`
(`proto/trogonai/session/sessions/v1alpha1/redaction_applied.proto:1-20`) and `ArtifactErased`
(`proto/trogonai/session/sessions/v1alpha1/artifact_erased.proto:1-18`) have no Mastra analogue at all. This is not
evidence that decision 7's finer granularity is unnecessary; it is evidence
that even a mature, shipped, multi-backend store with an explicit,
documented retention API still treats "keep" and "delete" as the only two
states, which sharpens exactly what decision 7's masking and byte-erasure
tiers add relative to the industry default: a middle ground between
"everything, unmasked, forever" and "gone." Mastra also validates the
"pruning must be cooperative and resumable, never a single unbounded
sweep" principle decision 7's cold-tiering language shares in spirit (the
`Object Store` restore path, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 915-921), independently
arriving at `maxBatches`/`pauseMs`/`AbortSignal` for the same reason we
would need equivalent bounds on any batched, resumable operation over a
large log.

One point is genuinely new evidence from Mastra, not already covered by
Letta's comparison: retention being purely a product-owned, unscheduled
concern (nothing in the library calls `prune()` on a timer) means the
actual growth-bound guarantee for any Mastra deployment depends entirely on
whether the deploying team remembered to wire up a scheduler. Decision 7
does not have this exposure, because keep-forever is the explicit, stated
default with no retention job required for correctness; a deployment that
never wires anything up gets exactly the behavior decision 7 already
specifies (nothing is pruned), rather than an unbounded table nobody
noticed was never being pruned. This is presented as validation of
decision 7's choice to make "never delete" the unconditional default rather
than an opt-out from a scheduled job that a deployment might simply forget
to configure.

## Open questions for the ADR

1. Should [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) or a follow-up name, explicitly, a repo-level check that
   every read path sorting events for a session must use `SessionOrdinal`
   with no independently-chosen tiebreak, given Mastra's four internally
   inconsistent sort call sites in one backend file? (Recommendation 1.)
2. Should the ADR state, as a standing constraint on future commands, that
   no atomic multi-fact operation may ever be implemented as sequential
   independent writes with an application-level compensating rollback,
   given DynamoDB's `saveMessages` rollback whose own failure path is
   silent? (Recommendation 2.)
3. If our system ever needs more than one mechanism to express a
   parent-child relationship (for a debugger view, an export tool, or an
   analytics pipeline), should the ADR require that mechanism to be built
   on `ParentLinked`/`DelegationDispatched` rather than added as an
   independent field, given that Mastra's three coexisting, mutually-unaware
   mechanisms are the direct product of not requiring this? (Recommendation
   3.)
4. Does the platform need a caller-facing, explicitly-bounded "recent tail"
   read query, distinct from full aggregate resume, for callers that need
   cheap recent context without a correctness guarantee, the way Mastra's
   `MessageHistory` default serves that need today? If added, how should it
   be named and documented so it is never mistaken for resumed state?
   (Recommendation 4.)
5. Mastra's retention is entirely product-scheduled, with no default
   pruning job and no masking tier. Decision 7 already avoids the
   "forgot to schedule the job" exposure by making keep-forever the
   unconditional default; does the ADR need to say anything more about how
   a future cold-tiering job ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) lines 915-921) should behave if a
   deployment never enables it, to preserve that same "correct even if
   nobody configures anything" property?
6. Mastra's cascade evidence is entirely an absence (no backend's
   `deleteThread` reads fork-link metadata) rather than a positive
   alternative design. Is there a case in the corpus so far where deletion
   cascade for a subagent-style link has been implemented correctly at the
   storage layer, or does this remain, across every product surveyed, a gap
   nobody has actually solved rather than solved differently?

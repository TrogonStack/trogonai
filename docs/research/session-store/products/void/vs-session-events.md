# Void compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Void](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 1/12**: evolution scars 0/3 (the storage key was renamed
twice, `void.chatThreadStorage` → `...StorageI` → `...StorageII`
(`src/vs/workbench/contrib/void/common/storageKeys.ts:14-19`), but the
dossier found no migration code anywhere in the tree carrying old data
forward; the axis rewards a format change that *carries data forward*, and
this is the opposite: a rename that abandons it, so it earns nothing here
even though it is real evidence of *something*), operational age 0/3 (no
first-commit date and no issue reports of corruption, growth, or lock
contention were found or cited; the only age signal is inline version
comments, "1.0.2" / "1.0.3", which date the renames but not a discovered
failure), exposure 1/3 (Void is a real, vendor-shipped VS Code fork, but the
dossier cites no adoption-scale or multi-host evidence, and the storage
model is explicitly single-installation, single-process, single-machine
with "no remote writeback, shared filesystem handling, or cross-host
reconciliation" (see the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section),
so the exposure this axis rewards, resume surviving crashes/upgrades/hosts at
scale, is simply not exercised), design independence 0/3 (the store itself,
durability, atomicity, the key-value substrate, is unmodified upstream VS
Code `IStorageService`/SQLite code Void does not touch directly
(`src/vs/platform/storage/electron-main/storageMain.ts:285`, `:361`); Void's own
code contributes the domain shape serialized into it, not the persistence
mechanism, and this axis scores the store, not the schema on top of it).

This score is low on purpose: Void's answers below carry little independent
weight and should not be read as an industry norm on any axis where a
higher-scoring store (Cline at 10/12, fx) disagrees.

## The one structural difference everything else follows from

Void has no event log. The durable record for an entire installation is one
`ChatThreads` map, holding every thread and every message ever created,
serialized with `JSON.stringify` and written to a single VS Code storage key,
`THREAD_STORAGE_KEY = 'void.chatThreadStorageII'`
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`,
`src/vs/workbench/contrib/void/common/storageKeys.ts:19`). Every mutation
computes a new whole map and calls `_storeAllThreads()` with the complete
object (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`,
called at `:942`, `:1289`, `:1646`, `:1659`, `:1675`, `:1696`). There is no append operation, no positional ordinal, and no
schema-version field: versioning is done by renaming the key itself
(`src/vs/workbench/contrib/void/common/storageKeys.ts:14-19`).

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2 makes append the only mutation primitive: "rewind,
revert, compaction, and hide are all new appended events, never edits or
deletes to old ones." Void sits at the pole this decision was written to
avoid: not "append-only with looser discipline" like the JSONL products in
the corpus, but whole-document-rewrite with no discipline at all. Every
other difference below (no subagent concept, no growth bound, destructive
rewind-then-continue, ad hoc versioning) is a direct consequence of this one
fact: there is no concept of an event, only a snapshot of current state that
gets replaced in full.

## Mapping

**Semantic mismatch: "checkpoint."** Void's `CheckpointEntry`
(`role: 'checkpoint'`) is a message-array entry carrying, per touched file, a
full `entireFileCode` snapshot inline in the transcript itself
(`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:38-46`,
`src/vs/workbench/contrib/void/common/editCodeServiceTypes.ts:115-118`): it is an undo point living in the same mutable blob as the
conversation. Our `Checkpoint` (`proto/trogonai/session/sessions/v1alpha1/checkpoint.proto:17-38`)
is an opaque, out-of-line, digest-verified artifact reference used only for
harness process-state recovery, one of four records [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3
deliberately keeps separate authority for ("harness recovery checkpoint,"
"aggregate snapshot," "read-side checkpoint," and the typed event log
itself). A reader mapping the two terms naively would assume equivalence;
they solve different problems and share almost nothing but the English word.

| Void | Ours | Verdict |
| --- | --- | --- |
| Single `ChatThreads` map under one `StorageScope.APPLICATION` key, all threads for the installation (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`) | One logical stream per session, subject `session.sessions.events.<session_id>` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Ours, decisively: no shared blob whose size is every thread ever created |
| Thread id: client-generated `generateUuid()`, plain UUID, no ordering semantics (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:211`) | Opaque `SessionId`, time-sortable by construction but sort order never load-bearing ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Equivalent identity concept, addressed differently |
| `createdAt`/`lastModified` ISO strings on `ThreadType`, sidebar sorts on `lastModified` (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:114-144`, `src/vs/workbench/contrib/void/browser/react/src/sidebar-tsx/SidebarThreadSelector.tsx:37-38`) | No mutable "last modified" field; order is fold-derived `SessionOrdinal` (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto`) | Ours, since there is nothing to keep in sync with the log it summarizes |
| `_storeAllThreads()`: full rewrite of the whole map on every mutation (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`) | Append is the only mutation primitive ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively |
| No positional dedup key, no expected-version precondition, "last `_setState` call simply wins" (see the dossier's [Write and append path (ordering, durability, concurrency, delivery)](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section) | Per-command `WRITE_PRECONDITION` (`NoStream`/`At`/`Any`), server-enforced ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively |
| `ToolMessage<T>` mutates a `type` field in place through a lifecycle (`invalid_params → tool_request → running_now → tool_error/success/rejected`), one object overwritten (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:11-28`) | Separate immutable facts: `ToolCallRequested`/`Started`/`Completed`/`Failed` (`proto/trogonai/session/sessions/v1alpha1/tool_call_requested.proto`, `tool_call_completed.proto`) | Ours, decisively: no single mutable record whose history is only its current value |
| `CheckpointEntry.entireFileCode`: full file content inlined per touched file, no dedup (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:38-46`, `src/vs/workbench/contrib/void/common/editCodeServiceTypes.ts:115-118`) | `FileChanged.before_ref`/`after_ref`, content-addressed `ArtifactRef` claim-checks (`proto/trogonai/session/sessions/v1alpha1/file_changed.proto:30-36`, `artifact.proto:14-34`) | Ours, decisively |
| `duplicateThread(threadId)`: deep clone, fresh id, no lineage field on either copy (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1663-1677`) | `SessionForked{source_session_id, context_prefix_boundary}` (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:17-27`) | Ours, decisively: typed, durable lineage vs. two independent records with no link back |
| Rewind: `jumpToCheckpointBeforeMessageIdx` moves a pointer only; the *next* user message calls `thread.messages.slice(0, checkpointIdx + 1)` and persists the truncated array (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1080`, `:1272-1290`) | `SessionRewound{keep_through}`, a pure marker (`proto/trogonai/session/sessions/v1alpha1/session_rewound.proto:16-22`); nothing is ever sliced or deleted ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively: this is the calibration point below |
| Versioning: rename `THREAD_STORAGE_KEY`, no migration code, old data under the old key becomes inaccessible (`src/vs/workbench/contrib/void/common/storageKeys.ts:14-19`; see the dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning) section for the migration-code absence) | Schema evolution is additive only: new optional fields, reserved retired numbers, never a per-event version branch ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) | Ours, decisively: this is the other calibration point below |
| `currentThreadId` explicitly not persisted; no "resume where you left off" (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:308`, `:1628-1648`) | No equivalent: which session a client last had open is a client/UI concern, not a store fact | Neither, out of scope for both, noted for completeness |
| Listing is global across every workspace ever opened, `StorageScope.APPLICATION` (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:406`, `:420`, `src/vs/platform/storage/common/storage.ts:225-228`) | `SessionStarted.workspace`, a required `WorkspaceRef` (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:19-23`, `workspace.proto`) | Trade-off, not a plain gap: see below |
| No subagent/child-session concept anywhere; tool calls are ordinary entries in the same thread (see the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section) | `DelegationDispatched`/`ParentLinked`/`CascadePolicy` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Ours; see "Subagent cascade" below, this is not evidence either way, it is absence |
| No retention, no TTL, no growth bound; every thread and message accumulates forever, re-serialized whole on every mutation (see the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section) | Keep-forever log with snapshot-bounded replay, `SessionHidden`/`RedactionApplied`/`ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours; see "Retention" below |

## What we should consider changing

Given the store maturity score, this section is short by design. Void
supports one clarification, not a schema change.

### 1. State explicitly, in [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2, that "non-destructive rewind" is a property of the write path following it, not of the rewind event alone

**The change.** Add one sentence to [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2's `SessionRewound`
discussion (or to `proto/trogonai/session/sessions/v1alpha1/session_rewound.proto`'s
comment) naming the exact failure this store is designed to avoid: a rewind
marker by itself guarantees nothing if a *later* command is allowed to
delete or truncate anything.

**Evidence anchor.** Void, store maturity 1/12 (thin evidence, cited only
as a cautionary counterexample, not an industry norm):
`jumpToCheckpointBeforeMessageIdx` moves a pointer and by itself does *not*
truncate the message array; truncation happens lazily, on the next user
message, via `thread.messages.slice(0, checkpointIdx + 1)`, an in-place,
irreversible rewrite (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1080`, `:1272-1290`).
The dossier's own conclusion: "'non-destructive rewind' is not a property of
the rewind operation alone, it is a property of what the next write does"
(paraphrasing the dossier's [What this implies for our Session Store (our inference)](./index.md#what-this-implies-for-our-session-store-our-inference) section).

**Blast radius.** Additive: this is a documentation clarification. Our
`SessionRewound.keep_through` (`session_rewound.proto:16-22`) already is
what Void's rewind pointer is not: `RewindSession` never deletes, slices, or
edits an event, and there is no "next write" in our design that could later
discard the abandoned branch, because there is no write primitive other than
append. The recommendation costs nothing to implement; it exists only to
name the failure mode explicitly so a future implementer of `decide`/`evolve`
for `RewindSession` cannot accidentally reintroduce Void's pattern (for
example, by having a follow-on compaction or cleanup pass physically drop
events after `keep_through` as an "optimization").

**Why.** Void is a real, shipped instance of exactly the failure mode an
append-only rewind exists to prevent: the abandoned branch is not
recoverable, not auditable, and not distinguishable after the fact from a
branch that was never taken. It is worth one sentence in the ADR precisely
because the two halves of "non-destructive rewind" (a marker, and a write
path that respects it) are easy to build correctly in isolation and easy to
violate by adding one destructive step later; Void shows what that looks
like once it happens.

**Cost.** None beyond the sentence. No new field, no new event, no new
projection.

No further changes are recommended. A 1/12 store does not support
recommendations beyond a single, narrowly-scoped documentation note; every
other observation about Void surfaces either as something our design already
does better (below) or as a pattern to explicitly reject (also below).

## What our design already does better

- **Append-only mutation vs. whole-document rewrite.** Every Void mutation,
  a new message, an edit, a delete, a checkpoint jump, reads the entire
  `ChatThreads` map and rewrites the entire thing
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`). Our append
  is O(1) in total session size regardless of how large the log has grown
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2); a session's history is never re-read and
  re-written in full to record one more fact.
- **Content-addressed artifacts vs. inlined full-file checkpoints.** Void's
  `CheckpointEntry` embeds `entireFileCode`, the whole file, per touched
  path, inline in the same array that gets rewritten on every mutation
  (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:38-46`,
  `src/vs/workbench/contrib/void/common/editCodeServiceTypes.ts:115-118`). Our
  `FileChanged.before_ref`/`after_ref` `ArtifactRef` pair deduplicates
  identical content globally by digest (`artifact.proto:14-34`) and keeps
  the event itself small.
- **Typed, durable fork lineage vs. an unlinked deep clone.** `duplicateThread`
  produces two threads with no durable record that either came from the
  other (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1663-1677`).
  `SessionForked` names `source_session_id` and a `context_prefix_boundary`
  permanently (`session_forked.proto:17-27`).
- **Real optimistic concurrency vs. "last call wins."** Void has no
  compare-and-swap of any kind; concurrent writers simply overwrite each
  other (see the dossier's [Write and append path (ordering, durability, concurrency, delivery)](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section). Our
  guarded commands use a server-enforced `WRITE_PRECONDITION`
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2).
- **Workspace binding as a recorded fact vs. no binding at all.** Void's
  threads carry no path/cwd component and are listed globally regardless of
  which folder is open (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:406`, `:420`,
  `:114-144`). `SessionStarted.workspace` is a required `WorkspaceRef`
  (`session_started.proto:19-23`).

## Trade-offs, not gaps

- **Global, workspace-agnostic listing vs. workspace-scoped sessions.**
  Void's `StorageScope.APPLICATION` means every chat thread ever created in
  one VS Code installation is enumerable from any project
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:406`, `:420`,
  `src/vs/platform/storage/common/storage.ts:225-228`). This may be
  a deliberate product choice, "see every chat you've ever had, regardless
  of which repo you have open," not an oversight; the dossier does not
  establish intent either way. Our design requires every session to carry a
  `WorkspaceRef` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1), which buys workspace-scoped audit and
  a queryable binding at the cost of making "all my chats across every
  project" a cross-workspace query rather than one map iteration. Neither
  side is wrong; they are answering different questions about what a
  session list is for.
- **Single-writer simplicity vs. multi-writer/multi-host correctness.**
  Void's "last `_setState` call simply wins" concurrency model
  (see the dossier's [Write and append path (ordering, durability, concurrency, delivery)](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section) is
  adequate and cheap for a single-user, single-process desktop editor. Our
  server-enforced OCC ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) exists to survive multi-writer
  and multi-host correctness (decision 8's forced answer), which Void's
  problem domain never has to solve.

## What not to copy

- **Whole-document rewrite as the only write primitive.** Every mutation,
  regardless of size, rewrites the complete `ChatThreads` map
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`). This is the
  direct structural cause of every other item on this list.
- **Unbounded single-key growth with no retention story.** Every thread and
  every message, including full-file checkpoints, accumulates forever under
  one storage key with no TTL, no cap, and no eviction
  (see the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section, flagged in
  the dossier itself as inference, since no explicit cap was found rather
  than proven absent by exhaustive testing).
- **Destructive truncation as the second half of "rewind."** `slice(0,
  checkpointIdx + 1)` permanently discards everything after a rewind point
  the moment the user sends the next message
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1272-1290`). Recorded
  here explicitly as the failure mode recommendation 1 above names.
- **Versioning by renaming the storage key, with no migration code.**
  `void.chatThreadStorage` → `...StorageI` → `...StorageII`
  (`src/vs/workbench/contrib/void/common/storageKeys.ts:14-19`), with a
  standing comment warning that "changing this format is a big deal" but no
  migration logic anywhere under `src/vs/workbench/contrib/void/`
  implementing that warning
  (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:49`; see the
  dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning)
  section for the absence of migration code). Every
  version bump silently abandons every user's prior thread history rather
  than carrying it forward. This is not a hypothetical risk of an
  unversioned format; it is a concrete, shipped instance of exactly the
  failure mode a migration ratchet exists to prevent. Contrast Zed's
  `sqlez::Connection::migrate`, which compares each shipped migration's
  stored SQL text against the compiled `Domain::MIGRATIONS` array and
  **panics** on any mismatch unless a step explicitly opts into
  `should_allow_migration_change`
  (`crates/sqlez/src/migrations.rs:37-104` in the Zed repo): a hard
  fail loudly, at connection time, rather than a silent, unannounced loss of
  every prior thread. Void demonstrates the two ends of the same axis in one
  corpus: fail loud and refuse to proceed (Zed), or rename the key and quietly
  orphan the old data (Void). Our own schema evolution is additive-only in
  `v1alpha1` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3), which sidesteps the choice entirely for
  now; it does not yet say what happens the day an additive change is not
  enough (see Open questions).

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position here: a child session is its
own logical stream, linked by `DelegationDispatched`/`ParentLinked`, with an
explicit `CascadePolicy` (`CASCADE_ON_PARENT_TERMINAL` or `INDEPENDENT`) and
a reconciler that cascades terminal state and rewind-invalidation as
distinct, typed batches. Void's evidence does not validate, challenge, or
refine that position; it simply has none to compare against. `ChatThreads`
is a flat `{ [id]: ThreadType }` map with no parent/child field anywhere in
`ThreadType`, and targeted greps for `subagent`, `childThread`, and
`parentThread` returned no matches
(see the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section). Tool calls,
including MCP calls tagged via `mcpServerName`, are recorded as ordinary
entries in the same thread's own message array; there is no sub-thread, no
delegation, and therefore no cascade-on-delete question to answer at all:
`deleteThread` removes exactly one key from the flat map
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1651-1661`), with
nothing else to cascade to, invalidate, or orphan. Stated plainly, as the
research prompt requires even when the honest answer is absence: **Void has
no position on subagent cascade**, because Void has no subagent concept.
This is thin, uninformative evidence on this axis, not a data point that
moves decision 6 in any direction.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
"aggregate snapshots bound replay, not storage" so an append stays O(1)
regardless of log length. Void's evidence sharpens, rather than
contradicts, this design, though it is thin evidence (no confirmed field
failure, only source-level inference): the dossier found **no** retention
policy, no TTL, no scheduled cleanup, and no growth bound anywhere: "every
thread and every message (including full-file-content checkpoints)
accumulates forever in one JSON value under one storage key, re-serialized
and rewritten on every single mutation"
(see the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section, marked in
the dossier itself as inference, since no explicit cap was found rather than
proven never to exist).

Void's failure mode is structurally worse than "an unbounded log with no
retention policy," which is what decision 7 accepts as its trade-off: Void
has no append primitive at all, so *every* mutation, not only growth past
some threshold, already costs O(total installation history): reading and
re-serializing every thread and every message the user has ever had, on
every keystroke's worth of state change. Decision 7's snapshot-bounded
replay exists specifically so growth without bound does not also mean cost
without bound; Void demonstrates the naive alternative directly, with no
event log underneath it at all, rather than merely lacking a retention
policy on top of one. This validates the structural bet in decision 7 (keep
the log, bound replay cost with snapshots) more sharply than a product that
has an event log but simply has not designed retention for it, but it says
nothing new about the retention *policy* question itself (keep-forever vs.
a TTL), because Void has no policy to compare and, per the dossier's own
flagged uncertainty, no confirmed user-facing failure report to point to
either. Carried forward as inference, not hardened into a claim: if Void
has ever caused a user-visible slowdown or freeze from installation-wide
blob size, no issue report documenting it was found in this pass.

## Open questions for the ADR

- [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3 makes `v1alpha1` schema evolution additive-only
  (new optional fields, reserved retired numbers, never a version branch).
  Void's storage-key-rename pattern is a concrete example of what happens
  when a product needs a genuinely breaking change and has no migration
  story: history is silently abandoned. The ADR states that promotion from
  `v1alpha1` to `v1` is "a later, separate decision" but does not yet say
  what happens to already-persisted `v1alpha1` events at that boundary, or
  whether a future breaking change to an accepted `v1` would follow Zed's
  fail-loud ratchet, a replay/rewrite migration, or something else. This is
  not urgent, nothing in `v1alpha1` has needed a breaking change yet, but
  Void and Zed together show the two ends of the outcome space if the
  question is left unanswered until the day it is forced.
- Is a client-facing "which session was I last looking at" pointer (Void's
  `currentThreadId`, deliberately *not* persisted,
  `src/vs/workbench/contrib/void/browser/chatThreadService.ts:308`, `:1628-1648`) something the
  Session Store should ever record, or is it correctly out of scope as a
  pure client/UI concern with no session-store fact backing it? Void answers
  "out of scope" by omission rather than by a stated design choice; worth
  confirming our own design agrees for the same reason, not by default.

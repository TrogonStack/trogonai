# Cline compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Cline](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and ADR#0035 on 2026-08-04.

**Store maturity: 10/12** -- evolution scars 2/3 (a real generational cut-over,
classic per-task flat files to the SDK's row-plus-document store, with
back-compat read paths still live -- `legacy-state-reader.ts`,
`normalizeStoredMessageModelMetadata` migrating flat `providerId`/`modelId`
onto nested `modelInfo` -- but the current generation's own schema has never
needed a version bump past `version: 1`, and `migrateTaskHistoryToFile` in
`state-migrations.ts:65-67` is an unimplemented stub, so only one full-cut
migration exists, not iterative in-place evolution), operational age 2/3
(`cline/cline#9011`, opened 2026-02-01 and closed 2026-07-03, is a real,
source-corroborated growth failure -- a 10.5MB/2016-message task freezing the
JetBrains IDE -- but the dossier is explicit that issue-level confirmation
exists only for the legacy generation's named files
(`api_conversation_history.json`, `ui_messages.json`), not for the current SDK
generation, which is confirmed only at the source level), exposure 3/3
(vendor-shipped across VS Code, CLI, and a hub surface, pluggable
`SessionPersistenceAdapter` with a SQLite-backed primary and a JSON-index
fallback), design independence 3/3 (no evidence in the dossier that the SDK
session store was forked from another product's persistence code; it reads
only its own prior generation).

## The one structural difference everything else follows from

Cline's SDK-generation store has no append operation anywhere. Every durable
artifact -- the session row, the manifest file
(`{sessionsDir}/{sessionId}/{sessionId}.json`,
`sdk/packages/core/src/session/models/session-manifest.ts:6-28`), the
messages file (`{sessionId}.messages.json`,
`sdk/packages/core/src/services/session-artifacts.ts:75-80`), and the
compaction sidecar (`{sessionId}.compaction.json`,
`sdk/packages/core/src/session/models/session-compaction.ts:25-34`) -- is a mutable
whole document, read in full and rewritten in full on every persist.
`persistSessionMessages()` re-serializes the entire `messages: []` array with
`JSON.stringify(payload, null, 2)` and a plain `writeFileSync` on every turn
(`sdk/packages/core/src/session/stores/session-manifest-store.ts:157-176`); there is
no positional append, no line-oriented log, no cursor. This is not a
difference of commit granularity the way fx's one-event-per-turn model is
(fx still appends, just coarsely); Cline's store has no granularity concept
at all, because "persist" always means "replace the whole document."

ADR#0035 makes append-only mutation the only primitive session state ever
undergoes (decision 2: "rewind, revert, compaction, and hide are all new
appended events, never edits or deletes to old ones"), and ties every write to
a server-enforced `WRITE_PRECONDITION` (`NoStream` / `At(current_position)` /
`Any`) rather than a client-issued compare-and-swap. Cline's closest analogue
-- `statusLock`/`expectedStatusLock`, a version-stamped integer column CAS'd
via `UPDATE ... WHERE session_id = ? AND status_lock = ?`
(`sdk/packages/core/src/session/services/session-service.ts:194-208`), retried up to
`OCC_MAX_RETRIES = 4` (`persistence-service.ts:40`) -- only protects the row.
The messages file and the manifest, which carry the actual conversation
content, have no locking or versioning at all.

Everything else in this comparison is downstream of that one fact: the
durability asymmetry (below), the unbounded growth confirmed by issue #9011
(see the industry-gaps section), and the one-level-deep cascade (see the industry-gaps section) are
all consequences of a store built around "rewrite the document," not
"append the fact."

## Mapping

| Cline | Ours | Verdict |
| --- | --- | --- |
| `SessionRow.session_id` (SQLite `sessions` table / `sessions.index.json`; `taskId == sessionId`) | Opaque `SessionId`; one logical stream per session on subject `session.sessions.events.<session_id>` (ADR#0035 decision 1) | Equivalent identity concept |
| `SessionRow.status` + `status_lock` (28-column row, client-issued CAS, `session-service.ts:194-208`) | Lifecycle folded from `SessionStarted`/`SessionClosed`/`SessionCancelled`/`SessionFailed`/`SessionHidden`, guarded by JetStream `At(current_position)` (`Nats-Expected-Last-Subject-Sequence`, ADR#0035 decision 2) | Ours, decisively -- no denormalized status-plus-version column that can drift from the log; the guard is enforced by the broker, not a client-issued `UPDATE ... WHERE` |
| `SessionRow.parentSessionId` / `parentAgentId` / `agentId` / `isSubagent` (`sdk/packages/core/src/session/models/session-row.ts:6-34`) | `DelegationDispatched{child_session_id, operation_id}` (`proto/trogonai/session/sessions/v1alpha1/delegation_dispatched.proto`) on the parent's stream, `ParentLinked{parent_session_id, operation_id, parent_dispatched_at}` (`parent_linked.proto`) on the child's | Ours -- one fact recorded once on each side (ADR#0035 decision 6) vs. four plain columns on a mutable row that can be edited independently of any event |
| `SessionRow.cwd` / `workspace_root` | `SessionStarted.workspace`, a required `WorkspaceRef{workspace_id, uri, revision}` (`session_started.proto`, `workspace.proto`) | Ours, decisively -- see below |
| Deterministic subagent id `makeSubSessionId(rootSessionId, agentId)`; re-spawning a named subagent reuses its row (`session-graph.ts:9-17`, `TeamChildSessionManager.upsertSubagentSession`, `team-child-session-manager.ts:132-160`) | No deterministic child-id scheme in the catalog; `DispatchDelegation` always mints a fresh `child_session_id` (ADR#0035 decision 6) | Deliberate divergence -- see recommendation 2 |
| `SessionCompactionStateSchema` sidecar: `{source_message_count, source_prefix_hash, source_last_message_key, messages[]}` (`session-compaction.ts:25-34`) | `Compacted{covers_from, covers_through, summary_content, tokens_before, tokens_after}`, a single in-stream marker (`compacted.proto`) | Ours, decisively -- no second file whose hash can silently drift from the transcript it summarizes; see below |
| `CheckpointEntry{ref, createdAt, runCount, kind}` / `CheckpointMetadata{latest, history}` stored in manifest metadata; `ref` is a private git ref `refs/cline/checkpoints/{sessionId}/{runCount}` (`sdk/packages/core/src/types/sessions.ts:33-44`) | `Checkpoint{reference, checkpoint_type, digest, checkpoint_id, producing_execution_attempt_id, covers_through, session_execution_plan_digest}` inside `CheckpointProduced` / `ExecutionAttemptStarted.restored_checkpoint` (`checkpoint.proto`, `checkpoint_produced.proto`) | Semantic mismatch, not a plain equivalence -- see below |
| `runCount`, an integer keying checkpoint refs, surviving compaction (`getUserRunSpan`) | `turn_id`, stamped (not inferred) on `UserMessageRecorded`, all three `AssistantMessage*` events, and `ToolCallRequested/Started/Completed/Failed` (ADR#0035 decision 3) | Ours -- same underlying concept (a turn survives compaction and re-identifies related facts), generalized past checkpoint-keying to every conversational and tool event |
| `MessageWithMetadata{role, content, modelInfo{id,provider,family?}, metrics{inputTokens,outputTokens,cacheReadTokens,cacheWriteTokens,cost?}, ts}` (`sdk/packages/shared/src/llms/messages.ts:131-156`) | `CanonicalMessage{message_id, role, content, model, usage, created_at}` (`message.proto`) | Equivalent |
| `ContentBlock` union: `TextContent, FileContent, ImageContent, ToolUseContent, ToolResultContent, ThinkingContent, RedactedThinkingContent` | `ContentBlock` oneof: `text, artifact_ref, ThinkingBlock, ToolUseBlock, ToolResultBlock, bytes redacted_thinking, ProviderBlock` (`message.proto`) | Equivalent; ours additionally keeps an unmodelled-provider-block escape hatch (`ProviderBlock`, ADR#0035 decision 3) Cline has no analogue for |
| `metrics{inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens, cost}` with no finality marker | `TokenUsage{input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, cost, completeness}` where `completeness` is `UsageCompleteness{FINAL, PARTIAL}` (`token_usage.proto`) | Ours, decisively -- Cline has no field distinguishing a mid-stream token reading from a final one; this is the same gap fx's stage-two comparison flagged (its item 6) and it now recurs independently in a second, unrelated product, which is corroborating evidence the distinction is worth having |
| Legacy `FileContextTracker.addFileToFileContextTracker()` appends to `metadata.files_in_context` on every file-read/edit/mention with no cap, rewriting all of `task_metadata.json` each time (`FileContextTracker.ts`) | `ResourceObservation` on `ToolCallCompleted.observed` (`repeated ResourceObservation`, `tool_call_completed.proto`, `resource_observation.proto`) | Ours, decisively -- the same information (what did this call put in context) is attached to the completing call itself rather than accreted forever in a separately-rewritten unbounded array; see the retention gap below for the one place this pattern can still bite us |
| `SessionRow.transcript_path` / `messages_path` / `hook_path` | No equivalent -- storage location is an implementation detail of a store adapter, not a domain fact | Ours, by design (nothing to record) |
| Spawn queue: `enqueueSpawnRequest`/`claimSpawnRequest` against `subagent_spawn_queue` (SQL) or `subagent-spawn-queue.json`, at-least-once, `consumed_at` marks completion | `OperationReserved{operation_id, request_digest, operation_kind}` / `OperationOutcomeRecorded{oneof succeeded, failed, cancelled, unknown}` (`operation_reserved.proto`, `operation_outcome_recorded.proto`), `OPERATION_KIND_CHILD_SESSION_DELEGATION` | Ours -- a typed outcome oneof including a non-terminal `unknown` state, vs. Cline's binary consumed/unconsumed marker with no modelled failure outcome |
| `reconcileDeadSessions()` / `isPidAlive()`, run as a side effect of `listSessions()`, transitions dead-PID rows to `status: "failed"` with `metadata{terminal_marker, terminal_marker_source: "stale_session_reconciler"}` (`persistence-service.ts:424-508`) | `SessionFailed{reason, detail}` (`session_failed.proto`), triggered per ADR#0035 decision 6 by "a liveness watchdog that concludes no further attempt will run" | Ours, mostly -- same concept (dead-process detection becomes a terminal fact), but Cline's version is opportunistic (runs only when something happens to list sessions); see recommendation 3 |
| `applyStatusToRunningChildSessions(sessionId, "cancelled")`: a direct, synchronous status push into the children's rows on parent terminal transition (`persistence-service.ts:205-211`) | A reconciler process manager reacting to `session.sessions.events.>`, appending an atomic `[ParentTerminated, SessionCancelled]` batch per child (ADR#0035 decision 6) | Trade-off, not a plain win -- see the subagent cascade gap below |
| Official docs describe checkpoints as a "shadow Git repository" (`docs/core-workflows/checkpoints.mdx`); the source implements private refs inside the user's own repo instead | ADR#0035 decision 3 explicitly keeps "harness recovery checkpoint," "aggregate snapshot," and "read-side checkpoint" as four records with separate authority precisely to avoid this kind of concept collapse | Not a store feature to compare -- a methodology point: our naming discipline exists to prevent exactly the doc/code drift Cline shipped |
| No redaction or byte-erasure concept found anywhere in the dossier | `RedactionApplied{redacted_event_ids, reason}` (`redaction_applied.proto`), `ArtifactErased{artifact_id, reason}` (`artifact_erased.proto`) | Ours, decisively |
| No retention/TTL policy found; deletion is manual only (`deleteSession()`) | `SessionHidden{reason}` (`session_hidden.proto`), a visibility tombstone, plus deferred crypto-shredding (ADR#0035 decision 7) | Ours, partially -- neither side deletes bytes, but we at least have a typed tombstone and a masking story; genuine erasure is an open, named gap on both sides |

## What we should consider changing

### 1. Bound fanout in the parent-to-children lineage projection the reconciler dispatches against

**The change.** ADR#0035 decision 6 has the terminal-cascade reconciler
discover children through "a parent-to-children lineage projection folded
from `DelegationDispatched`," with no stated bound on how many children that
projection returns for one parent, or on what happens once dispatch to all of
them is in flight.

**Evidence anchor.** Cline, store maturity 10/12:
`deleteSession()` queries rows with `parentSessionId === id` capped at
`limit: 2000` before cascading the delete
(`sdk/packages/core/src/session/services/persistence-service.ts:557-609`). The dossier
treats this as a latent orphan path rather than a present bug precisely
because nothing bounds fanout today; a 2001st child is silently excluded from
the cascade.

**Blast radius.** Additive if implemented as a paginated/cursor-following read
on the lineage projection; Breaking, cheap if implemented as a hard fanout
cap enforced in `decide` at `DispatchDelegation` time (new validation,
nothing persisted changes shape).

**Why.** Decision 6's cascade is transitive by construction and costs
D sequential reconciler round-trips for a chain of *depth* D -- the ADR's own
consequence list names that cost explicitly. It says nothing about *width*:
a single parent with an unbounded number of live children is a different
risk axis, and an unbounded or mis-paginated lineage projection can silently
drop children from a wide cascade the exact way Cline's `limit: 2000` would
silently exclude the 2001st row from a delete. Given the reconciler is
already event-driven and idempotent, closing this is cheap insurance; leaving
it open makes decision 6's "transitive by construction" claim conditional on
an unstated width assumption, the same way Cline's one-level cascade turned
out to be conditional on an unstated depth assumption.

**Cost.** A paginated lineage read (if the projection is a KV/document store,
this is a cursor, not a schema change) or an enforced cap on live children at
dispatch time, which is a real product decision (what should the cap be, and
what should happen when it's hit) that the ADR does not currently need to
make.

### 2. Do not adopt deterministic, re-usable child session ids

**The change under consideration, and why to reject it.** Cline's
`makeSubSessionId(rootSessionId, agentId)` is a pure function of parent and
agent name, so re-invoking a named subagent updates the same row instead of
minting a new one (`session-graph.ts:9-17`,
`TeamChildSessionManager.upsertSubagentSession`,
`team-child-session-manager.ts:132-160`).

**Evidence anchor.** Cline, store maturity 10/12, same citations above.

**Blast radius.** Breaking the decision -- ADR#0035 decision 6: "Acyclicity is
enforced by construction... `DispatchDelegation` always mints a fresh
`child_session_id`... A cycle would require an edge into a pre-existing
session, which this makes impossible." Making a child id a deterministic
function of `(parent, agent_name)` reintroduces exactly the case decision 6
was written to rule out: dispatch into a stream that may already exist.

**Why not to do this.** Cline's convenience buys a single durable "slot" per
named subagent, so a UI can show "this agent's latest run" without a query.
That convenience is real, but it is a *read-model* concern, not an identity
scheme: the same effect is available as a projection keyed by
`(parent_session_id, agent_name)` that resolves to the most recently
dispatched child, without touching how child ids are minted. Recording this
here is meant to stop it from being re-proposed on the grounds that Cline
does it and it seems ergonomic -- the ergonomics are real, the identity
mechanism that buys them is not compatible with decision 6's acyclicity
argument.

**Cost of the alternative.** A new projection (`get_latest_child_for_agent`
or similar), not a schema or identity change.

### 3. Make the liveness watchdog an explicit standing process, not a side effect of a read path

**The change.** ADR#0035 decision 6 says "a liveness watchdog that concludes
no further attempt will run records a Session-level `SessionFailed`," but
does not say when or how often that watchdog runs.

**Evidence anchor.** Cline, store maturity 10/12: `reconcileDeadSessions()`
and its `isPidAlive()` check run only as a side effect of `listSessions()`
being called (`persistence-service.ts:424-508, 510-535`) -- there is no
independent daemon; a crashed session with no active reader can sit in
`running` status indefinitely.

**Blast radius.** Additive -- a clarifying note in ADR#0035 (Consequences
already names "new standing services" including reconciler/watchdog
processes; this makes explicit that the watchdog must be one of them, not an
incidental side effect of a query).

**Why.** Cline's own dossier shows the failure mode directly: staleness is
discovered lazily, "the next time anyone lists," which means a crashed
session with no active reader can block whatever single-active-attempt
invariant depends on its terminal state for an unbounded time. Our design
already leans toward a standing watchdog (the ADR uses the present participle
"watchdog," implying an ongoing process), but nothing currently rules out an
implementation that regresses to Cline's pattern for expedience.

**Cost.** A real standing service to build and operate (already acknowledged
in the ADR's Consequences); this recommendation is only about making explicit
that it must not be query-triggered, which costs nothing to write down.

### 4. State whether the aggregate snapshot's own write needs an atomicity guarantee, and why or why not

**The change.** ADR#0035 decision 8 / facet 3 describes the aggregate
snapshot as "an advisory cached fold of that log. Corruption or
incompatibility falls back to earlier replay" but does not state whether the
snapshot's own write path needs any atomicity contract (temp-write-then-
rename, fsync, or none of the above).

**Evidence anchor.** Cline, store maturity 10/12: the compaction sidecar
gets a proper atomic write -- temp file with a `wx` flag, `writeFile`, `sync()`
fsync, close, `rename()`, best-effort parent-directory fsync
(`sdk/packages/core/src/session/stores/atomic-file.ts:23-53`) -- while the two
artifacts that actually hold conversation content, the messages file and the
manifest, use bare `writeFileSync` with no temp file and no fsync
(`session-manifest-store.ts:71-78, 157-176, 174-175`). Cline gave its
*least* load-bearing artifact the *most* durability.

**Blast radius.** Additive -- this is a documentation/Non-Goals clarification,
not a schema change.

**Why.** Cline's inconsistency is a real problem for Cline specifically
because its manifest and messages file have no independent source of truth
to fall back to if a torn write corrupts them; the compaction sidecar, which
got the careful atomic-write treatment, is the *recomputable* one. Our
situation inverts this: the event log is authoritative and the snapshot is
explicitly disposable on corruption, so a torn snapshot write is a
non-issue by the ADR's own design, provided the fallback path (replay from
the log) is actually exercised on read, not just described in prose. The
useful lesson from Cline is not "match their atomicity," it's "say
out loud, for each stored artifact, whether its atomicity matters, and why" --
Cline never did that audit and got the priority backwards as a result.

**Cost.** None beyond writing the sentence; it becomes real cost only if the
audit turns up a spot where the snapshot write path is assumed durable by an
implementer who didn't read this far into the ADR.

### 5. Consider a cap or claim-check threshold on `ToolCallCompleted.observed`

**The change.** `ToolCallCompleted.observed` is `repeated ResourceObservation`
(`tool_call_completed.proto`) with no stated bound on how many observations
one call can carry.

**Evidence anchor.** Cline, store maturity 10/12 (this recommendation reuses
the growth evidence from the industry-gaps section below: `files_in_context`
unbounded growth and `cline/cline#9011`, both confirming that "many small
observations, never bounded" is a real failure mode in this problem space,
even though Cline's specific mechanism differs -- it accretes across
*many separate* full-document rewrites, where ours would accrete *within one
event*).

**Blast radius.** Additive if implemented as a soft warning/metric; Breaking,
cheap if a hard cap forces large observation sets through `ArtifactRef`
instead of being inlined (the message already has that escape hatch --
`ResourceObservation`'s digest-based outcome doesn't inline content, only a
`ByteRange` and a digest, so the risk is observation *count*, not observation
*size*, for a tool that touches very many resources in one call, e.g. a
repo-wide search).

**Why.** This is the one place the append-only design does not automatically
avoid Cline's growth pattern: an append-only log still allows a single event
to grow unbounded if a repeated field inside it has no cap. It is worth
naming even though -- see the retention gap below -- nothing in the dossier
demonstrates this has actually happened to us or to Cline; it's inference
from the field's shape, not a confirmed failure.

**Cost.** A cap requires deciding what "too many observations for one call"
means product-side; a soft warning costs only a metric.

## What our design already does better

- **Server-enforced OCC vs. client-issued CAS.** JetStream's
  `At(current_position)` guard (`Nats-Expected-Last-Subject-Sequence`) rejects
  a stale writer at the broker; Cline's `statusLock` CAS is a client-issued
  `UPDATE ... WHERE status_lock = ?` retried up to four times
  (`session-service.ts:194-208`, `persistence-service.ts:40`) that only
  covers the row -- the messages file and manifest that hold the actual
  content have no equivalent protection at all.
- **Content is claim-checked, not inlined.** `ArtifactRef{artifact_id, digest,
  size_bytes, ...}` (`artifact.proto`) and `ResourceObservation`'s
  digest-based outcome (`resource_observation.proto`) keep large content out
  of the event log by reference; Cline's `MessageWithMetadata.content` and
  `files_in_context` both inline full content into documents that get
  rewritten whole on every touch.
- **Typed, per-entity terminal-outcome resolution.** `ToolCallCompleted` vs.
  `ToolCallFailed`, and `AssistantMessageCompleted` vs.
  `AssistantMessageFailed`, compete under an explicit first-terminal-outcome-
  wins fold rule keyed by `tool_execution_id`/`message_id`
  (`tool_call_completed.proto`, `tool_call_failed.proto`,
  `assistant_message_completed.proto`, `assistant_message_failed.proto`; ADR
  decision 4). Cline's dual-transcript resolution (below) shows what happens
  without this: a non-final turn's cancel-vs-clean-end outcome is simply lost.
- **Redaction and erasure are named, typed events, not absent concepts.**
  `RedactionApplied` and `ArtifactErased` have no analogue anywhere in the
  Cline dossier.
- **Workspace binding is a required, recorded fact, not an inferred column.**
  `SessionStarted.workspace` is a required `WorkspaceRef`; Cline's `cwd` is a
  plain SQL column with no relocation/rename reconciliation found in the
  dossier -- a session whose working directory moved on disk is simply not
  handled.
- **Typed process-termination facts, kept separate from what the model saw.**
  `CommandTermination{exit_code | signal}` belongs to `ToolCallCompleted`,
  deliberately not to `ToolCallResult`, "so an exit status the model never
  saw must not enter the replay shape" (`command_termination.proto`). Cline
  has no equivalent typed separation between execution outcome and
  provider-visible result.
- **Compaction is one self-sufficient in-stream marker.** `Compacted{
  covers_from, covers_through, summary_content}` (`compacted.proto`) needs no
  second file. Cline's sidecar (`SessionCompactionStateSchema`) is a second
  artifact whose `source_prefix_hash` had to be redefined mid-flight to
  exclude `id`/`ts` after the team discovered hashing transport-identity
  fields "made projection fail for semantically identical prefixes... so
  persistence was silently rejected every turn"
  (`session-compaction.ts:79-85`) -- a class of bug that cannot occur if there
  is no second hash to keep in sync with the thing it summarizes.

## Trade-offs, not gaps

- **Synchronous same-transaction status push vs. eventually-consistent
  reconciler cascade.** Cline's `applyStatusToRunningChildSessions` pushes a
  parent's terminal status into every currently-running child row directly,
  in the same code path, no round trip. Ours is deliberately eventually
  consistent (ADR#0035 decision 6, Consequences: "a deep collaboration chain
  cascades in O(depth) reconciler round-trips; callers expecting synchronous
  cascade are surprised"). Cline's approach is simpler and faster for a flat,
  shallow graph; ours is correct at unbounded depth and survives a crash
  mid-cascade, which Cline's direct push does not need to survive because it
  assumes the graph stays flat. Neither is free: Cline pays with the orphan
  risk in the industry-gaps section below; we pay with cascade latency.
- **Opportunistic crash detection vs. a dedicated watchdog.** Cline detects
  a dead process only when someone lists sessions; a standing watchdog (ours)
  costs an always-on process but detects failure without depending on read
  traffic. Recommendation 3 above already surfaces this as worth making
  explicit rather than leaving implicit.
- **`runCount` as a bare integer vs. `turn_id` as a stamped identifier.**
  Both survive compaction and let a system re-identify facts belonging to the
  same user-driven turn. Cline's is narrower (only checkpoints key off it);
  ours is broader (stamped on every conversational and tool event) at the
  cost of being one more field every producer must set correctly.

## What not to copy

- **Bare `writeFileSync` for the artifacts that matter most.** The messages
  file and manifest -- the two documents that actually hold conversation
  content -- get no temp-file-then-rename, no fsync
  (`session-manifest-store.ts:71-78, 157-176`), while the compaction sidecar,
  which is fully recomputable, gets the careful atomic write
  (`atomic-file.ts:23-53`). Durability effort should track what is
  irreplaceable if lost, not what happened to be built most recently.
- **Whole-document rewrite as the only write primitive.** Rewriting an
  entire messages array on every turn is the direct cause of the growth
  failure documented in `cline/cline#9011`. Our append-only log with
  bounded-cost tail replay exists specifically so that a session's *history*
  never has to be re-read and re-written in full to record one more fact.
- **Unbounded accretion into a single mutable field.** The legacy
  `files_in_context` array (`FileContextTracker.ts`) grows forever with no
  cap, no eviction, and a full-file rewrite on every addition. Recommendation
  5 above exists precisely because our own `ToolCallCompleted.observed` is a
  `repeated` field that could, in principle, grow the same way inside a
  single event if nothing bounds it.
- **Documenting a mechanism that isn't the one shipped.** Cline's own docs
  describe checkpoints as a "shadow Git repository"; the source implements
  private refs in the user's real repo instead. This is a process lesson, not
  a store lesson: keep the ADR's terminology (harness recovery checkpoint,
  aggregate snapshot, read-side checkpoint) as precise as it currently is,
  specifically so no future doc describes one as if it were another.

## The two gaps the industry has not closed

### Subagent cascade

ADR#0035 decision 6 already takes a position here: a child session is its
own logical stream, linked by facts on each side
(`DelegationDispatched`/`ParentLinked`); terminal cascade is driven by a
reconciler reacting to terminal markers on `session.sessions.events.>`, and
"cascade is transitive, because `SessionCancelled` is itself a terminal
marker the same reconciler reacts to; a chain of depth D still takes D
sequential reconciler round-trips." The question for this section is whether
Cline's evidence validates, challenges, or refines that position -- not
whether we still need one.

**What Cline does.** `deleteSession()` cascades exactly one level: for a
non-subagent session it queries rows with `parentSessionId === id` (capped at
`limit: 2000`), deletes the parent row, deletes the matched child rows
directly, and for each deletes its checkpoint refs, messages file,
compaction state, and manifest file
(`sdk/packages/core/src/session/services/persistence-service.ts:557-609`). Critically,
that child query sits inside `if (!row.isSubagent)`
(`persistence-service.ts:566`): deleting a session that is *itself* a
subagent never looks for its own children. The children found by the query
are deleted directly, not recursed into. The dossier's own conclusion:
"Cline is safe from orphaning today only because the parent-child graph is in
practice one level deep; the guarantee is a property of how deep the graph
happens to get, not of the delete algorithm." No maximum-nesting-depth guard
was found anywhere in the persistence layer (flagged as an open question in
the dossier, since one could plausibly live in tool-definition or agent-loop
code not read for the dossier). Rewind has no separate cascade concept at
all: `findCheckpointForRun()`/`trimMessagesToCheckpoint()` simply throw if the
target run has been folded into a compacted summary, and status propagation
to *running* children on parent terminal transition is a direct,
synchronous row push (`applyStatusToRunningChildSessions`,
`persistence-service.ts:205-211`), not a reconciled, replayable cascade.

**Does this validate, challenge, or refine decision 6?** It validates the
core design choice and sharpens one risk decision 6 does not yet name.
Cline is the clearest evidence in the corpus that "cascade that looks
complete" and "cascade that is actually transitive" are different claims: a
one-level cascade is invisible as a limitation for as long as the graph
happens to stay flat, and becomes an orphan path the moment it doesn't.
Decision 6's transitive-by-construction design (cascade *is* the reconciler
reacting to its own emitted terminal markers, recursively, rather than a
one-shot query for direct children) is exactly the structural fix for the
failure mode Cline's own dossier calls out in itself. Cline does not
challenge decision 6's shape; it does surface two things decision 6's text
does not yet cover, both of which are already written up as recommendation 1
above: an explicit fanout bound on the lineage-discovery projection (Cline's
analogue is the silent `limit: 2000`, which excludes rather than fails loud),
and, separately, whether an explicit maximum nesting *depth* should be
enforced given that a deep chain now costs D sequential reconciler round
trips per decision 6's own Consequences -- Cline shows no position on depth
limits either (only that a depth cap "could plausibly live in tool-definition
or agent-loop code not read for this dossier"), so this is not evidence for
adding one, only evidence that the industry (this product included) has not
converged on an answer and it remains an explicit choice for the ADR owner,
not a borrowed norm.

### Retention on an unbounded log

ADR#0035 decision 7 already takes a position here: keep-forever, with
`SessionHidden` as a visibility tombstone (no bytes deleted), `RedactionApplied`
for read-time masking, `ArtifactErased` for out-of-band artifact-byte
destruction, and "aggregate snapshots bound replay, not storage" so that
resume cost is O(tail after the newest snapshot) "even as the log grows
forever." The question here is whether Cline's confirmed growth failure
validates that design or exposes a cost the ADR does not bound.

**What Cline does.** Nothing bounds the size of a session's durable record.
Two independent mechanisms confirm this: the legacy
`FileContextTracker.addFileToFileContextTracker()` appends to
`metadata.files_in_context` on every file-read/edit/mention event with no
cap, trim, or eviction, rewriting all of `task_metadata.json` each time
(`FileContextTracker.ts`); and the current SDK-generation messages file is
both written (`persistSessionMessages()`,
`session-manifest-store.ts:157-176`) and read
(`readPersistedMessagesFile()`, `runtime-host-support.ts:53-75`) as a single
whole-file JSON blob with no offset, limit, or pagination anywhere. The
corroborating field evidence is `cline/cline#9011` (opened 2026-02-01, closed
2026-07-03): a task reaching roughly 10.5MB and 2016 UI messages / 590 API
messages caused the JetBrains IDE to become unresponsive or freeze
indefinitely when the task was opened. The dossier is careful that this
issue names the *legacy* files specifically, so it is source-confirmed for
the SDK generation and only source-confirmed, not issue-confirmed, that the
SDK generation's own full-file-rewrite pattern would fail the same way at
comparable size -- but the write and read paths scale identically (linear in
total session size, no bound), so the mechanism is the same even where the
field report is not.

**Does this validate, challenge, or refine decision 7?** It validates the
structural fix and sharpens where the ADR's claim needs to be read narrowly.
Cline's failure mode is specifically the cost of a full linear read-and-parse
plus a full linear rewrite on every turn -- exactly what decision 7's
snapshot-bounded replay is designed to avoid, since our runtime "resumes from
the newest snapshot and replays only the tail," never the whole log, and
never rewrites the log to make room for new facts. On its own terms the
retention design does not reproduce Cline's failure mode: an append is O(1)
in total session size regardless of how large the log has grown, which is
the one property Cline's design lacks entirely.

That said, two costs the ADR does not explicitly bound are worth naming
rather than assuming away, because they are the parts of our read path that
still scale with something:

- **Model-visible context compilation.** ADR#0035 decision 8 says the
  model-visible context is "compiled deterministically from the event log,
  bounded by the latest `Compacted` marker." That bound is on how far *back*
  the compilation reads, not on how much content sits between the last
  compaction and the current turn -- a very long uncompacted run (compaction
  never triggers, or is deferred) has no stated bound on this cost. This is
  an agent-loop/compaction-policy concern under decision 4, not a store
  defect, but the ADR does not currently say who is responsible for
  guaranteeing compaction actually happens before this cost grows large.
- **`ToolCallCompleted.observed`, per recommendation 5.** A single event
  can still grow unbounded if nothing caps how many `ResourceObservation`
  entries one call accumulates. This is the one place a purely append-only
  design does not automatically inherit Cline's protection-by-boundedness,
  because the accretion happens *inside* one fact rather than *across* many
  rewritten documents.

Neither of these is confirmed as an actual failure anywhere in the corpus --
they are inferences from the shape of the design, flagged per the "mark
inference as inference" rule, not evidence-backed gaps the way Cline's own
growth failure is. They are the honest edges of "does snapshot-bounded replay
avoid Cline's failure mode," not a claim that decision 7 is wrong.

## Open questions for the ADR

1. Should the parent-to-children lineage projection the terminal-cascade
   reconciler reads from (decision 6) have an explicit fanout bound, and if
   so, what should happen to a request that would exceed it -- reject the
   dispatch, or degrade to a paginated/best-effort cascade?
2. Should there be an explicit maximum subagent nesting depth, given that
   decision 6's cascade cost is O(depth) reconciler round-trips per event, and
   neither Cline nor (per the dossier) any product it was checked against
   enforces one?
3. Who is responsible for guaranteeing that `Compacted` markers are emitted
   often enough that model-visible-context compilation (decision 8) never has
   to walk an unboundedly long uncompacted tail -- the agent loop's
   compaction-trigger policy, or a store-side backstop?
4. Should `ToolCallCompleted.observed` carry an explicit cap, or a documented
   expectation that a tool touching very many resources reports a summary
   `ArtifactRef` instead of one `ResourceObservation` per resource?
5. Does the aggregate snapshot's write path (decision 8) need any atomicity
   guarantee of its own, or is "corruption falls back to replay" sufficient
   justification to leave it unspecified -- and if the latter, should that
   reasoning be stated in the ADR so a future implementer does not add
   unneeded ceremony (or, conversely, skip needed ceremony assuming it's
   already covered)?

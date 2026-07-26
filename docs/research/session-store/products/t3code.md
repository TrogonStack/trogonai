# T3 Code: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of
[pingdotgg/t3code](https://github.com/pingdotgg/t3code) (checked out from the
`TrogonStack/t3code` fork) at commit
`b4b66491e39a9dd72e7f52de47a10cb5c41dc78a` (committed 2026-07-21), the source
of "T3 Code", a web GUI that wraps coding-agent CLIs (Codex, Claude, Cursor,
OpenCode). Every citation below was re-verified against this exact commit on
2026-07-23 (working tree clean). Citations use repo-relative `path:line`
shorthand.

Unlike most products in this corpus, T3 Code does not run the model itself: it
orchestrates provider CLIs and is, structurally, a **CQRS / event-sourced
orchestration server** written in TypeScript on the Effect runtime. The durable
"session" is therefore split across two tiers: T3's own append-only event log
(the orchestration truth) and the provider CLI's native conversation memory
(referenced by an opaque resume cursor). The persistence subsystem lives under
`apps/server/src/persistence/` (the event store, migrations, projection
repositories) and `apps/server/src/orchestration/` (the decider, projector,
engine, and reactors).

## The storage model

The durable session is an **append-only event log**: the SQLite table
`orchestration_events` in a single per-server database
`{baseDir}/userdata/state.sqlite` (`apps/server/src/config.ts:101-105`,
`stateDir` is `userdata` in prod / `dev` in dev, `dbPath = join(stateDir,
"state.sqlite")`). The database opens in WAL mode with foreign keys on
(`apps/server/src/persistence/Layers/Sqlite.ts:36-37`). The event store's own
docstring states its role plainly: it "owns durable append/replay access to the
orchestration event stream. It does not reduce events into read models or apply
command validation rules" (`apps/server/src/persistence/Services/OrchestrationEventStore.ts:4-6`).

The event row is the source of truth
(`apps/server/src/persistence/Migrations/001_OrchestrationEvents.ts:8-23`):

```sql
CREATE TABLE IF NOT EXISTS orchestration_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  aggregate_kind TEXT NOT NULL,
  stream_id TEXT NOT NULL,
  stream_version INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  command_id TEXT,
  causation_event_id TEXT,
  correlation_id TEXT,
  actor_kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL
)
```

This is unambiguously **session-as-log** (event-sourced). There are two
aggregate kinds — `project` and `thread`
(`packages/contracts/src/orchestration.ts:899`) — and a "session" in product
terms is a **thread** (`OrchestrationThread`,
`packages/contracts/src/orchestration.ts:347-381`), reconstructed by folding the
thread-scoped events of one `stream_id`.

What is authoritative vs. derived:

- **Authoritative**: `orchestration_events`. It is append-only; nothing rewrites
  or deletes rows on the normal path.
- **Derived / rebuildable**: the whole `projection_*` family of tables
  (`projection_projects`, `projection_threads`, `projection_thread_messages`,
  `projection_thread_activities`, `projection_thread_sessions`,
  `projection_turns`, `projection_pending_approvals`, plus the projector cursor
  table `projection_state`, `apps/server/src/persistence/Migrations/005_Projections.ts`).
  Each is materialized by a projector that replays the log from its own
  `last_applied_sequence` (`apps/server/src/orchestration/Layers/ProjectionPipeline.ts:1592-1606`),
  so they are pure projections and fully rebuildable.
- **External runtime handle (not the log, not a projection)**: the
  `provider_session_runtime` table holds a per-thread opaque `resume_cursor_json`
  that points into the *provider CLI's own* native session store
  (`apps/server/src/persistence/Migrations/004_ProviderSessionRuntime.ts:8-18`,
  `apps/server/src/persistence/ProviderSessionRuntime.ts:50`). The actual
  model-side transcript for, e.g., a Codex thread lives in Codex's store; T3
  references it by cursor.
- **Content-addressed side store**: git-ref checkpoints of the working tree per
  turn, with a diff cache table `checkpoint_diff_blobs`
  (`apps/server/src/persistence/Migrations/003_CheckpointDiffBlobs.ts`), plus
  attachment blobs under `{stateDir}/attachments` (`config.ts:106`).

## Keying and identity

- **Store scope**: one `state.sqlite` per server install/base directory
  (`config.ts:101-105`). There is no cwd- or project-path-encoded key in the
  event key; all projects and threads share one DB and are separated by columns.
- **Event key**: `(aggregate_kind, stream_id, stream_version)` with a global
  monotonic `sequence`. `stream_id` is a `ProjectId` or `ThreadId`
  (`OrchestrationEventStore.ts` `Schema.Union([ProjectId, ThreadId])`,
  `apps/server/src/persistence/Layers/OrchestrationEventStore.ts:53-54`;
  event base fields `packages/contracts/src/orchestration.ts:1101-1111`).
- **Id minting**: all ids are branded, trimmed, non-empty strings
  (`packages/contracts/src/baseSchemas.ts:26-60`). Command-facing ids
  (`commandId`, `threadId`, `projectId`, `messageId`) are **client-supplied**;
  the server mints `eventId` as a UUIDv4 via `crypto.randomUUIDv4`
  (`apps/server/src/orchestration/decider.ts:41-50`). No id encodes ordering or
  location — ordering comes entirely from `sequence` (global) and
  `stream_version` (per aggregate), not from the id.
- **Listing scope**: global within the DB. Threads are scoped to a project by
  `project_id`; a project is anchored to a `workspaceRoot` filesystem path
  (`OrchestrationProject`, `packages/contracts/src/orchestration.ts:211-222`).
- **Relocation / rename**: a thread's `branch` and `worktreePath` are mutable
  fields carried by `thread.meta-updated` events
  (`decider.ts:417-452`, projector `projector.ts:388-405`); moving the working
  directory does not change identity because the key is the `threadId`, not a
  path. Title/model renames are likewise `thread.meta-updated` events.

## The store interface

The event store **is** a pluggable service (an Effect `Context.Service`), so the
contract is exported verbatim
(`apps/server/src/persistence/Services/OrchestrationEventStore.ts:22-55`):

```ts
export interface OrchestrationEventStoreShape {
  readonly append: (
    event: Omit<OrchestrationEvent, "sequence">,
  ) => Effect.Effect<OrchestrationEvent, OrchestrationEventStoreError>;

  readonly readFromSequence: (
    sequenceExclusive: number,
    limit?: number,
  ) => Stream.Stream<OrchestrationEvent, OrchestrationEventStoreError>;

  readonly readAll: () => Stream.Stream<OrchestrationEvent, OrchestrationEventStoreError>;
}
```

`append` assigns `sequence` and `stream_version` at insert time and returns the
stored event; `readFromSequence` is an exclusive-cursor paged replay; `readAll`
is `readFromSequence(0, MAX_SAFE_INTEGER)`
(`apps/server/src/persistence/Layers/OrchestrationEventStore.ts:184-267`). The
store never validates commands or builds read models.

The write side that callers actually use is the **OrchestrationEngine** service
(`apps/server/src/orchestration/Services/OrchestrationEngine.ts`), whose
reconstructed contract is:

- `dispatch(command) -> Effect<{ sequence }, OrchestrationDispatchError>` — the
  single mutation entrypoint. It enqueues onto a single-writer command queue and
  awaits the result (`Layers/OrchestrationEngine.ts:318-327`).
- `readEvents(fromSequenceExclusive, limit) -> Stream<OrchestrationEvent>` — thin
  pass-through to `eventStore.readFromSequence` (`Layers/OrchestrationEngine.ts:315-316`).
- `streamDomainEvents -> Stream<OrchestrationEvent>` — a fresh PubSub
  subscription per consumer for live fan-out (`Layers/OrchestrationEngine.ts:335-337`).

Supporting durable repositories (each its own `Context.Service`, reconstructed):

- **Command receipts** (idempotency ledger): `upsert(receipt)`,
  `getByCommandId({commandId}) -> Option<receipt>`
  (`apps/server/src/persistence/Services/OrchestrationCommandReceipts.ts:44-63`),
  backed by table from `Migrations/002_OrchestrationCommandReceipts.ts`.
- **Projection repositories** (one per read-model table) plus a
  `ProjectionState` cursor repo, driven by the pipeline's `bootstrap` and
  `projectEvent` operations (`Services/ProjectionPipeline.ts`,
  `Layers/ProjectionPipeline.ts:1608-1644`).
- **ProjectionSnapshotQuery**: `getSnapshot()` and `getCommandReadModel()` build
  the full read model from the projection tables in one transaction
  (`apps/server/src/orchestration/Layers/ProjectionSnapshotQuery.ts:954-1243`).
- **ProviderSessionRuntimeRepository**: `upsert`, `getByThreadId`, `list`,
  `deleteByThreadId` over the opaque resume-cursor rows
  (`apps/server/src/persistence/ProviderSessionRuntime.ts:64-103`).

The client-facing RPC surface over WebSocket is
`orchestration.dispatchCommand`, `orchestration.replayEvents`,
`orchestration.subscribeThread`, `orchestration.subscribeShell`,
`orchestration.getTurnDiff`, `orchestration.getFullThreadDiff`,
`orchestration.getArchivedShellSnapshot`
(`packages/contracts/src/orchestration.ts:25-33`, `1341-1370`).

## Write and append path (ordering, durability, concurrency, delivery)

- **Commit shape**: a command is decided into one or more events by a pure
  decider (`decideOrchestrationCommand(command, readModel)`,
  `apps/server/src/orchestration/decider.ts:97-876`), then each event is
  `INSERT`ed. The append computes the next per-stream version inline
  (`apps/server/src/persistence/Layers/OrchestrationEventStore.ts:106-157`):

  ```sql
  COALESCE(
    (SELECT stream_version + 1 FROM orchestration_events
     WHERE aggregate_kind = ? AND stream_id = ?
     ORDER BY stream_version DESC LIMIT 1),
    0)
  ```

- **Ordering**: two levels. `sequence INTEGER PRIMARY KEY AUTOINCREMENT` is the
  global order; `stream_version` is the per-aggregate order, enforced unique by
  `idx_orch_events_stream_version ON (aggregate_kind, stream_id, stream_version)`
  (`Migrations/001_OrchestrationEvents.ts:26-28`). That unique index is the
  optimistic-concurrency guard: two events cannot occupy the same stream version.
- **Atomicity**: `OrchestrationEngine.processEnvelope` wraps the whole
  command in `sql.withTransaction`: for each planned event it appends, folds it
  into the in-memory command read model, runs the projection pipeline, then
  upserts the command receipt — all in one transaction
  (`Layers/OrchestrationEngine.ts:175-219`). So the log append and the SQLite
  projections commit together (WAL, `Sqlite.ts:36`).
- **Concurrency**: **single-writer per server**. Every command flows through one
  unbounded queue consumed by exactly one worker fiber
  (`Layers/OrchestrationEngine.ts:96`, `309-310`), so commands are strictly
  serialized. There is **no caller-supplied expected-version precondition**; OCC
  is implicit in the single-writer queue plus the unique `stream_version` index.
- **Delivery / idempotence**: **exactly-once command application** via the
  command-receipt ledger. Before deciding, the engine looks up the receipt by
  `commandId`: an `accepted` receipt short-circuits and returns the stored
  `resultSequence`; a `rejected` receipt fails fast; otherwise it proceeds and
  writes a new receipt inside the same transaction
  (`Layers/OrchestrationEngine.ts:144-157`, `196-204`). Since `commandId` is
  client-supplied, a retried dispatch is deduplicated. On dispatch failure the
  engine reconciles its in-memory read model by re-reading persisted events from
  the pre-dispatch sequence (`Layers/OrchestrationEngine.ts:119-132`, `270-298`).

## Read and resume path

- **Server startup**: projections are bootstrapped by replaying the log from
  each projector's `last_applied_sequence`
  (`Layers/ProjectionPipeline.ts:1592-1606`), then the engine seeds its in-memory
  command read model from the projection tables, not from a full replay
  (`getCommandReadModel`, `Layers/OrchestrationEngine.ts:306-307`). So resume of
  the *engine* reads the derived projections; the log is replayed only to fill
  any projector gap.
- **Client resume**: `subscribeThread` / `subscribeShell` take an optional
  `afterSequence` cursor. When present, the server skips the full snapshot and
  replays events after that sequence before switching to live streaming;
  overlapping events are deduped by sequence on the client
  (`packages/contracts/src/orchestration.ts:474-508`). Without a cursor, the
  server sends a full projection snapshot (`getSnapshot`,
  `ProjectionSnapshotQuery.ts:954-1243`) then live events. A raw
  `orchestration.replayEvents` RPC reads the log from a sequence
  (`orchestration.ts:1333-1339`).
- **Provider conversation resume** is a **separate tier**: the model-side
  transcript is not in T3's log. Resuming the underlying agent uses the opaque
  `resumeCursor` from `provider_session_runtime` handed back to the provider
  adapter (`ProviderSessionRuntime.ts:50`; Codex resume/fork returns a
  `CodexResumeCursor { threadId }`, `apps/server/src/provider/Layers/CodexAdapter.ts:1667`).
- **Bounds / pagination**: event replay pages at `READ_PAGE_SIZE = 500` with a
  default limit of `1_000` (`Layers/OrchestrationEventStore.ts:67-68`). Read-model
  materialization is capped: `MAX_THREAD_MESSAGES = 2_000`,
  `MAX_THREAD_CHECKPOINTS = 500` (`apps/server/src/orchestration/projector.ts:33-34`),
  activities capped at 500 and proposed plans at 200 (`projector.ts:734`, `579`).
  These caps bound the *view*, not the log.

## Listing, summaries, and search

- **Listing** is a set of SQL queries over the projection tables, not a directory
  scan or a log replay. The shell snapshot lists `projection_projects` and
  `projection_threads` (`ProjectionSnapshotQuery.ts:300-420`), and the thread row
  is denormalized with picker-facing summary fields: `latest_user_message_at`,
  `pending_approval_count`, `pending_user_input_count`,
  `has_actionable_proposed_plan`, `latest_turn_id`, `archived_at`
  (`ProjectionThreadShell`, `packages/contracts/src/orchestration.ts:403-428`;
  columns added by later migrations, e.g. `023`, `017`, `033`). No scale numbers
  are quoted in-source.
- **Summary consistency**: the projection row *is* the denormalized read model.
  It is kept consistent by being written in the same transaction as the log
  append on the live path (`Layers/OrchestrationEngine.ts:184`), and each
  projector records its `last_applied_sequence` in `projection_state` atomically
  with its own writes (`Layers/ProjectionPipeline.ts:1568-1578`), so a projector
  can resume exactly where it left off.
- **Search**: there is **no full-text or vector search subsystem** (in contrast
  to Hermes's FTS5). Discovery is ordinary SQL filtering over the projection
  tables; there is nothing to bootstrap or keep in sync beyond the projections
  themselves.

## Entry/message structure and versioning

- **Event envelope** (`packages/contracts/src/orchestration.ts:1101-1111`):
  `sequence`, `eventId`, `aggregateKind`, `aggregateId`, `occurredAt` (ISO
  string), `commandId`, `causationEventId`, `correlationId`, `metadata`, plus a
  discriminant `type` and a per-type `payload`. `metadata` carries provider
  provenance (`providerTurnId`, `providerItemId`, `adapterKey`, `requestId`,
  `ingestedAt`, `orchestration.ts:1092-1098`). There are **23 event types**
  (`OrchestrationEventType`, `orchestration.ts:872-897`), each with a typed
  payload schema and a corresponding union member
  (`OrchestrationEvent`, `orchestration.ts:1113-1230`).
- **Store interpretation**: `payload` and `metadata` are persisted as JSON `TEXT`
  (`payload_json`, `metadata_json`), so the row is opaque bytes at rest, but the
  store **decodes and validates them through Effect Schema** on both append and
  read (`decodeEvent`, `Layers/OrchestrationEventStore.ts:31-33`, `204-208`). The
  store also derives `actor_kind` (`client`/`server`/`provider`) from the
  command id prefix and metadata (`inferActorKind`,
  `Layers/OrchestrationEventStore.ts:70-90`). Dedup/identity keys: `event_id`
  (UNIQUE) for the event, `command_id` for command idempotency.
- **Chaining**: `causationEventId` links an event to the event that caused it —
  e.g. `thread.turn-start-requested` is stamped with the `thread.message-sent`
  event's id as its cause (`decider.ts:557`). `correlationId` is by design the
  originating `commandId` (`orchestration.ts:146-148`). Thread lineage is carried
  in payload fields (`parentThreadId`, `forkedFromThreadId`,
  `forkedUpToMessageId`), not in envelope pointers.
- **Versioning**: there is **no explicit per-event schema-version field**.
  Format evolution is handled additively by the schemas themselves —
  `withDecodingDefault` supplies defaults for fields added later (e.g.
  `runtimeMode`, `interactionMode`, `orchestration.ts:934-937`), and a
  pre-decoding transform absorbs the legacy `{provider}` → `{instanceId}` model
  selection shape without a migration (`ModelSelection`,
  `orchestration.ts:81-114`). Because projections are rebuildable, read-model
  schema changes replay from the log.
- **DB schema migrations** are a **forward-only ratchet** run at startup via
  `effect_sql_migrations`, statically imported and sorted by id, applied only if
  greater than the last recorded id (`apps/server/src/persistence/Migrations.ts:61-136`).
  Migrations run 1–33 then jump to 41 (`Migrations.ts:61-96`), a gap not explained
  in-source. No down-migrations exist.

## Compaction and history management

- There is **no compaction, summarization, or truncation of the event log**. The
  durable `orchestration_events` stream grows monotonically. The only "shrinking"
  is view-side: read-model materialization caps (`MAX_THREAD_MESSAGES = 2_000`
  etc., `projector.ts:33-34`, `484`) bound what the UI holds, not what is stored.
- Model-visible context compaction is an **upstream / provider concern**: the
  wrapped CLI manages its own context window. T3 records the outcome of turns as
  events but does not compact them.
- The one summarization primitive is **summary-mode fork** (below), which
  produces a single new assistant message in a *new* thread rather than rewriting
  the source thread's history.

## Rewind, checkpoints, and fork

- **Rewind/revert is expressed as appended events, interpreted at fold time —
  never a destructive edit.** A `thread.checkpoint.revert` command produces a
  `thread.checkpoint-revert-requested` event (`decider.ts:649-669`); the
  CheckpointReactor restores the working tree and then dispatches
  `thread.revert.complete`, producing `thread.reverted` (`decider.ts:816-835`).
  The projector folds `thread.reverted` by *filtering* the view to entities whose
  `checkpointTurnCount <= turnCount` — dropping later messages, activities,
  proposed plans, and checkpoints from the projection while the underlying events
  remain in the log (`projector.ts:665-714`). This is exactly the
  append-marker-replayed pattern (as opposed to Hermes's in-place flag mutation).
- **File-state checkpoints** are git-ref based, one per `(thread, turnCount)`.
  The CheckpointReactor captures a real git checkpoint when a turn completes,
  keyed by `checkpointRefForThreadTurn(threadId, turnCount)`, and only in a git
  workspace (`apps/server/src/orchestration/Layers/CheckpointReactor.ts:177-319`).
  The turn's file summary and `checkpointRef` are carried in
  `thread.turn-diff-completed` (`ThreadTurnDiffCompletedPayload`,
  `orchestration.ts:1076-1085`); the full diff text is cached in
  `checkpoint_diff_blobs` (`Migrations/003_CheckpointDiffBlobs.ts`). File content
  is therefore stored/deduped by git (content-addressed refs), not inlined in the
  event log.
- **Fork** is a two-phase, **copy-plus-lineage** operation into a new stream. A
  `thread.fork` command must go through `ThreadForkService` — the plain decider
  explicitly rejects it (`decider.ts:264-269`). The service attempts a **native
  provider fork** first (Codex advertises `capabilities.nativeFork: true` and its
  `forkThread` returns a new provider thread id as the resume cursor,
  `CodexAdapter.ts:1647-1667`, `1759`), then dispatches `thread.fork.finalize`,
  which the decider turns into a `thread.forked` event
  (`decider.ts:271-348`). For `full-history` mode the source messages up to the
  cutoff are **copied** into the new thread; for `summary` mode a single
  generated assistant summary message is produced (`decider.ts:283-320`). The
  `thread.forked` payload records `forkedFromThreadId`, `forkedUpToMessageId`,
  `forkMode`, and `pendingForkContextText` (injected ahead of the first prompt
  when the new thread has no native provider memory), and the projector inserts
  the new thread with its copied messages (`projector.ts:309-353`).

## Subagents and nested sessions

- A subagent is a **first-class sibling thread** in the same project, not nested
  storage and not entries in the parent transcript. It gets its own `threadId`
  (its own event stream) and is linked to its parent by `parentThreadId`, carried
  on `thread.created` (`ThreadCreatedPayload.parentThreadId`,
  `orchestration.ts:940`; projector `projector.ts:286`). The design is documented
  in the fork ADR: "each subagent appears as its own thread: openable,
  inspectable mid-flight, steerable, and resumable"
  (`docs/fork/0003-native-subagent-threads.md`).
- Subagents can run on a **different provider/model** than the parent, and each
  defaults to its own git worktree; read-only children can share the parent's
  checkout (`docs/fork/0003`).
- **Nesting is bounded**: "subagent trees can nest up to five levels deep, and at
  most eight subagents may run at once across a tree" (`docs/fork/0003`). These
  guardrails live in the delegation/tool layer, not in the store schema.
- **Parent delete**: `thread.deleted` triggers only that thread's runtime cleanup
  (stop provider session, close terminals with `deleteHistory: true`) via the
  ThreadDeletionReactor (`apps/server/src/orchestration/Layers/ThreadDeletionReactor.ts:44-64`).
  No cascade to child threads was found in the event/decider path — children keep
  their `parentThreadId` and would be orphaned rather than cascade-deleted (see
  Open questions).

## Retention, deletion, and multi-host

- **Retention**: none. No TTL, lifecycle sweep, or scheduled cleanup of
  `orchestration_events` or projections was found. The log is retained
  indefinitely.
- **Deletion** is command-driven and **soft in the read model, physical only for
  external artifacts**. `project.delete` fans out to `thread.delete` for each
  active thread and then deletes the project, as one decided command sequence
  (`decider.ts:184-227`). A `thread.deleted` event is appended (the event itself
  is never removed); the projector sets `deleted_at` on the projection row and
  schedules physical removal of that thread's attachment directory as a projector
  side-effect (`deletedThreadIds` → `deleteThreadAttachments`,
  `Layers/ProjectionPipeline.ts:436-463`); the ThreadDeletionReactor best-effort
  stops the provider session and deletes terminal history
  (`ThreadDeletionReactor.ts:44-64`). Snapshot queries filter `deleted_at IS
  NULL` for active listings (`ProjectionSnapshotQuery.ts:381`).
- **Multi-host**: not a first-class path. The model assumes a single server
  process holding one local `state.sqlite` (single-writer command queue,
  `Layers/OrchestrationEngine.ts:96`, `309-310`). Remote *access* is a networking
  concern — the server is exposed over Tailscale/SSH (the `tailscale` and `ssh`
  packages, `docs/architecture/remote.md`) — but the database is never shared
  across hosts, and there is no distributed coordination or remote writeback.

## Interop with foreign session stores

T3 Code's entire purpose is to drive foreign agent CLIs (Codex `app-server`,
Claude Code, Cursor CLI, OpenCode), so it interoperates at the **runtime** level
rather than the **storage** level. It does **not** discover, import, or parse
those CLIs' native on-disk session files. Instead, for each thread it stores an
opaque `resume_cursor_json` in `provider_session_runtime`
(`Migrations/004_ProviderSessionRuntime.ts:15`,
`ProviderSessionRuntime.ts:50`) and delegates start/resume/fork to the provider
adapter, which reconstitutes the provider's own session from that cursor
(`CodexResumeCursor { threadId }`, `CodexAdapter.ts:1667`). The provider CLI
remains the owner of the raw model transcript; T3's event log is the
orchestration layer over it. The `provider_session_runtime` row is a mutable
upsert-by-`threadId` record (`ProviderSessionRuntime.ts:150-187`), not part of
the append-only log.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* T3 Code is the corpus's **cleanest
event-sourced example**: the durable session is an append-only
`orchestration_events` log keyed by `(aggregate_kind, stream_id,
stream_version)` with a global `sequence`, and every list/summary/detail view is
a **rebuildable SQLite projection** driven by per-projector `last_applied_sequence`
cursors. This is almost exactly the shape our design targets, and it validates
several specific choices:

- **Decider/projector/read-model split with a single-writer command queue and a
  transactional append+project step** is a working, coherent pattern
  (`decider.ts`, `projector.ts`, `Layers/OrchestrationEngine.ts:175-219`). It
  gives OCC "for free" via a unique `(aggregate_kind, stream_id, stream_version)`
  index without a caller-supplied expected version — worth contrasting with an
  explicit expected-version precondition, which would be needed the moment
  writers are no longer serialized by one process.
- **Command receipts keyed by client-supplied `commandId`** are a clean,
  durable idempotence mechanism (exactly-once command application), directly
  addressing the crash-mid-write dedup gap that Hermes solved only with an
  in-memory marker. We should adopt an equivalent command/idempotency ledger.
- **Causation/correlation on every event** (`causationEventId`, `correlationId =
  commandId`) is a low-cost provenance backbone we should keep.
- **Retroactive operations as replayed events, not destructive edits**
  (`thread.reverted` filtered at fold time, `projector.ts:665-714`) is the
  behavior our event-sourced design wants, and a strong precedent over Hermes's
  in-place flag mutation.
- **Two-tier custody** is the most transferable idea here: T3 owns the
  orchestration *facts* in its event log but does **not** own the model
  transcript — it stores only an opaque resume handle and delegates to the
  provider. This suggests our Session Store can be the authoritative orchestration
  log while the heavy raw transcript lives elsewhere, provided the durable log
  records the resume handle and enough provenance to re-derive views.
- **Cautions**: (1) **no retention or log-truncation/snapshotting** — the log
  grows unbounded and projection bootstrap is a full replay from each cursor;
  our design needs an explicit snapshot/retention story that theirs lacks. (2)
  **Fork is physical copy-plus-lineage into a new stream** (`decider.ts:283-320`),
  not a shared-prefix reference — simple but O(history) per fork; our design
  could reference a shared event prefix instead. (3) Delete is soft in
  projections but events are retained forever, so "deleted" data is still fully
  present in the log — a privacy/retention consideration we must decide
  deliberately.

## Open questions

- **Log growth / snapshotting**: no truncation, compaction, or snapshot of
  `orchestration_events` was found. Whether large installs ever bound the log or
  precompute projector snapshots (beyond the per-projector cursor) is unresolved.
- **Child cascade on parent delete**: `thread.deleted` cleanup targets only the
  deleted thread's runtime (`ThreadDeletionReactor.ts:44-64`); no cascade to
  subagent children was traced, so children appear to orphan (dangling
  `parentThreadId`). The intended lifecycle of children on parent delete/revert
  was not pinned down.
- **Migration id gap 33 → 41** (`Migrations.ts:94-95`): whether 34–40 were
  dropped, renumbered, or reserved is not explained in-source.
- **Resume-cursor contents per provider**: only `CodexResumeCursor { threadId }`
  was traced (`CodexAdapter.ts:1667`); the cursor/`runtime_payload_json` shapes
  for Claude, Cursor, and OpenCode adapters were not fully enumerated.
- **Multi-process safety**: correctness relies on a single server process's
  single-writer queue; the behavior if two server processes opened the same
  `state.sqlite` (the unique `stream_version` index would reject a clash, but the
  in-memory read models would diverge) was not exercised.

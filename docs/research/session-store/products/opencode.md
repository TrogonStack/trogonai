# OpenCode: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of the `anomalyco/opencode` fork
(`git@github.com:anomalyco/opencode.git`) on branch `dev` at commit
`62e4641235d7847dadc60da37cca8a023dd54fc1` (committed 2026-07-23). Every
citation below was verified against this exact commit on 2026-07-23. Citations
use repo-relative `path:line` shorthand.

> Scope note. This is the `anomalyco` fork, a heavily reworked OpenCode built on
> a TypeScript monorepo running the Effect runtime. It is **mid-migration**
> between two persistence models that both exist in the tree:
>
> 1. **Legacy filesystem store** (upstream OpenCode's model): per-key JSON files
>    on disk under `~/.local/share/opencode/storage/`, a mutable
>    session-as-directory model (`packages/opencode/src/storage/storage.ts`).
> 2. **New event-sourced SQLite store** ("v2"): an append-only event log with
>    rebuildable SQL projections, under `packages/core/src/**` and served by the
>    newer `packages/server`. This is the direction of travel and the design most
>    relevant to our event-sourced Session Store.
>
> This dossier centers on the **v2 event-sourced store** (the authoritative,
> current design) and documents the legacy filesystem store where it clarifies
> the migration or a still-live code path. Where a section applies only to one
> tier, it says so.

The v2 persistence subsystem is split across three packages: `packages/schema`
(the event/message/session Effect-Schema contracts), `packages/core/src/event*`
(the durable event store), and `packages/core/src/session/**` (the session
projections, projector, and read/resume logic). Structurally it is the same
**CQRS / event-sourced** shape as T3 Code in this corpus: a durable append-only
log plus rebuildable read-model projections, on Effect + SQLite.

## The storage model

The durable v2 session is an **append-only event log**: the SQLite table `event`,
one row per durable event, keyed per aggregate with a monotonic per-aggregate
sequence (`packages/core/src/event/sql.ts:10-25`):

```ts
export const EventSequenceTable = sqliteTable("event_sequence", {
  aggregate_id: text().notNull().primaryKey(),
  seq: integer().notNull(),
  owner_id: text(),
})

export const EventTable = sqliteTable(
  "event",
  {
    id: text().$type<EventV2.ID>().primaryKey(),
    aggregate_id: text().notNull().references(() => EventSequenceTable.aggregate_id, { onDelete: "cascade" }),
    seq: integer().notNull(),
    type: text().notNull(),
    data: text({ mode: "json" }).$type<Record<string, unknown>>().notNull(),
  },
  (table) => [
    uniqueIndex("event_aggregate_seq_idx").on(table.aggregate_id, table.seq),
    index("event_aggregate_type_seq_idx").on(table.aggregate_id, table.type, table.seq),
  ],
)
```

The aggregate for a session is the **`sessionID`**: every session event
definition declares `durable: { aggregate: "sessionID", version: N }`
(`packages/schema/src/session-event.ts:38-49`, `502-507`). So a "session" is the
fold of all `event` rows with `aggregate_id = <sessionID>`, ordered by `seq`.
This is unambiguously **session-as-log** (event-sourced).

What is authoritative vs. derived:

- **Authoritative**: the `event` table (`packages/core/src/event/sql.ts:10-25`)
  and its head-sequence sidecar `event_sequence`. Rows are append-only on the
  normal path; nothing rewrites an existing `(aggregate_id, seq)`.
- **Derived / rebuildable projections** (all in
  `packages/core/src/session/sql.ts`): `session` (the read-model row / summary),
  `session_message` (the v2 message projection, seq-ordered), `message` + `part`
  (the legacy-v1-shaped message projection), `session_input` (admitted/promoted
  prompt queue), `session_context_epoch` (the compaction/system-context
  baseline), and `todo`. Each is rebuilt by replaying the log through the
  projectors (`packages/core/src/session/projector.ts:211-455`).
- **Content-addressed side store**: filesystem snapshots of the working tree per
  assistant step, stored as git trees in a per-project shadow git dir
  (`packages/core/src/snapshot.ts:91-135`), referenced from assistant messages
  by tree id (`Assistant.snapshot.start/end`,
  `packages/schema/src/session-message.ts:171-175`). File content is stored/deduped
  by git, not inlined in the log.

A crucial detail: the v2 projector consumes **both** the new `session.next.*`
durable events **and** the legacy v1 `session.created` / `message.updated` /
`message.part.updated` events, projecting all of them into the same SQL tables
(`packages/core/src/session/projector.ts:215-330`). The unified durable manifest
is literally the union of v1 and v2 durable definitions
(`packages/schema/src/durable-event-manifest.ts:12-15`). So the one event log is
the source of truth; the `session`/`message`/`part` tables are v1-shaped
projections and `session_message` is the v2-shaped projection of that same log.

The **legacy filesystem store** (still used by much of `packages/opencode`) is,
by contrast, **session-as-directory of mutable JSON documents**: a key-value
store where each key is a path and each value a `.json` file
(`packages/opencode/src/storage/storage.ts:53-65`), with `read`/`write`/`update`/
`remove`/`list` operations and forward-only on-disk migrations
(`storage.ts:81-211`). That is a mutable-document model, not a log.

## Keying and identity

- **Store scope (v2)**: one SQLite database per install/channel, default
  `~/.local/share/opencode/opencode.db` (or `opencode-<channel>.db` for dev
  channels), overridable by `OPENCODE_DB` (`packages/core/src/database/database.ts:43-55`).
  All projects, workspaces, and sessions share one DB and are separated by
  columns, not by directory. WAL mode, `synchronous = NORMAL`, foreign keys on
  (`database.ts:27-31`).
- **Event key**: `(aggregate_id, seq)` unique
  (`packages/core/src/event/sql.ts:22`), where `aggregate_id` is the `sessionID`.
  Global ordering is not a single autoincrement column (unlike T3); ordering is
  strictly **per-aggregate `seq`**.
- **Session id minting**: `SessionID` is a branded string `ses_` + a
  **time-ordered, descending-sortable** 26-char identifier
  (`packages/schema/src/session-id.ts:5-15`). The generator packs
  `timestamp*0x1000 + counter` and, for `descending()`, bit-inverts it so newer
  ids sort *first* lexicographically, then appends random bytes
  (`packages/schema/src/identifier.ts:15-30`). So the id **encodes creation time
  and ordering** (client/process-minted, ULID-like). Event ids (`evt_`) and
  message ids (`msg_`) use the **ascending** variant
  (`packages/schema/src/event.ts:9-12`, `packages/schema/src/session-message.ts:12-15`).
- **Listing scope**: global within the DB, filtered by column. `list` accepts a
  directory, a project id (+ optional subpath), or nothing (all sessions), plus
  an optional `workspaceID` (`packages/core/src/session.ts:55-77`, `268-303`).
  Cross-project enumeration is the no-filter case.
- **Location vs identity**: a session row carries `project_id`, `workspace_id`,
  `directory` (absolute), and `path` (relative subpath within the project)
  separately from its `id` (`packages/core/src/session/sql.ts:22-60`). Because the
  key is the `sessionID`, **relocation does not change identity**: a
  `session.next.moved` event updates `directory`/`path`/`workspace_id` in place
  and resets the context epoch (`packages/core/src/session/projector.ts:243-258`).
  Moving directories is a mutable projection field on a stable id, and directory
  paths are normalized cross-platform at the column boundary
  (`packages/core/src/database/path.ts:27-75`).

## The store interface

There are two relevant contracts. First, the **generic durable event store**,
`EventV2.Service`, which every aggregate (sessions included) writes through. It
is an Effect `Context.Service`, so the contract is exported verbatim
(`packages/core/src/event.ts:126-148`):

```ts
export interface Interface {
  readonly publish: <D extends Definition>(
    definition: D,
    data: Data<D>,
    options?: PublishOptions,
  ) => Effect.Effect<Payload<D>>
  readonly subscribe: <D extends Definition>(definition: D) => Stream.Stream<Payload<D>>
  readonly all: () => Stream.Stream<Payload>
  readonly durable: (input: { readonly aggregateID: string; readonly after?: number }) => Stream.Stream<Payload>
  /** @deprecated Use `all()` and consume the returned stream. */
  readonly listen: (listener: Subscriber) => Effect.Effect<Unsubscribe>
  readonly project: <D extends Definition>(definition: D, projector: Subscriber<D>) => Effect.Effect<void>
  readonly replay: (
    event: SerializedEvent,
    options?: { readonly publish?: boolean; readonly ownerID?: string; readonly strictOwner?: boolean },
  ) => Effect.Effect<void>
  readonly replayAll: (
    events: SerializedEvent[],
    options?: { readonly publish?: boolean; readonly ownerID?: string; readonly strictOwner?: boolean },
  ) => Effect.Effect<string | undefined>
  readonly remove: (aggregateID: string) => Effect.Effect<void>
  readonly claim: (aggregateID: string, ownerID: string) => Effect.Effect<void>
}
```

`PublishOptions` includes a **`commit(seq)` hook** — "Local operational
projection committed atomically with a new durable event"
(`packages/core/src/event.ts:118-124`). `publish` appends one event (assigning
the next `seq`), runs registered projectors, runs the commit hook, and writes the
row — all inside one transaction (see next section). `project(definition,
projector)` registers a synchronous projector that runs inside that transaction.
`durable({aggregateID, after})` is a resumable historical-then-live stream of one
aggregate's events. `replay`/`replayAll` are idempotent re-application (used for
migration and multi-host sync). `remove` physically deletes an aggregate's log.
`claim` sets the owner of an aggregate.

The module-level helpers `latestSequence(db, aggregateID)` (current head, `-1` if
none, `packages/core/src/event.ts:21-32`) and `readAggregate(db, {aggregateID,
after, limit, manifest})` (paged decode of the log, `event.ts:63-108`) round out
the raw store surface.

Second, the **session-specific read store**, `SessionStore.Service`, a read-only
projection accessor (`packages/core/src/session/store.ts:14-24`):

```ts
export interface Interface {
  readonly get: (sessionID: SessionSchema.ID) => Effect.Effect<SessionSchema.Info | undefined>
  readonly context: (sessionID: SessionSchema.ID) => Effect.Effect<SessionMessage.Message[], MessageDecodeError>
  readonly runnerContext: (
    sessionID: SessionSchema.ID,
    baselineSeq: number,
  ) => Effect.Effect<SessionMessage.Message[], MessageDecodeError>
  readonly message: (
    messageID: SessionMessage.ID,
  ) => Effect.Effect<{ readonly sessionID: SessionSchema.ID; readonly message: SessionMessage.Message } | undefined>
}
```

The higher-level, caller-facing **`SessionV2.Service`** wraps both and is the
operational contract the server and tools actually use
(`packages/core/src/session.ts:113-180`). Its operations (verbatim signatures in
that block):

- `create(input) -> Info` — mints a session id, resolves/creates the project row,
  and publishes a `session.created` event; idempotent by id
  (`session.ts:208-262`).
- `get(sessionID) -> Info | NotFoundError` (`session.ts:263-267`).
- `list(input?) -> Info[]` — keyset-paginated SQL over `session`
  (`session.ts:268-303`).
- `messages({sessionID, limit?, order?, cursor?}) -> Message[]` — seq-ordered,
  cursor-paginated read of the `session_message` projection
  (`session.ts:304-337`).
- `message({sessionID, messageID}) -> Message | undefined` (`session.ts:338-341`).
- `context(sessionID) -> Message[]` — the model-visible context (compaction- and
  epoch-aware fold, see Read/resume) (`session.ts:342-345`).
- `events({sessionID, after?}) -> Stream<DurableEvent>` — live durable tail of the
  session's log (`session.ts:346-351`).
- `history({sessionID, after?, limit}) -> {events, hasMore}` — paged raw event
  history (`session.ts:352-359`).
- `switchAgent` / `switchModel` — publish the corresponding events
  (`session.ts:393-416`).
- `prompt({id?, sessionID, prompt, delivery?, resume?}) -> Admitted` — admits a
  user prompt into the input queue and (unless `resume:false`) wakes execution
  (`session.ts:360-386`).
- `shell` / `skill` / `compact` / `wait` — currently return
  `OperationUnavailableError` in this build (`session.ts:387-424`).
- `resume(sessionID)` / `interrupt(sessionID)` / `active` — execution control
  (`session.ts:425-432`).
- `revert.stage` / `revert.clear` / `revert.commit` — retroactive rewind
  (`session.ts:433-453`).

Notably, **there is no `delete` on the v2 `SessionV2.Service`** (the interface
block `session.ts:113-180` has none). Deletion exists only as a projector for the
legacy `session.deleted` event (`projector.ts:259-261`) and the raw
`events.remove(aggregateID)` (`event.ts:514-523`); see Retention/deletion.

## Write and append path (ordering, durability, concurrency, delivery)

- **Commit shape**: callers never `INSERT` directly. They call
  `events.publish(Definition, data, options?)`, which builds a payload and calls
  `commitDurableEvent` (`packages/core/src/event.ts:205-367`). For a durable
  event, that runs an **uninterruptible SQLite transaction**
  (`event.ts:237-353`, `db.transaction(..., { behavior: "immediate" })`) that:
  1. reads the current head seq and owner for the aggregate (`event.ts:243-249`);
  2. computes `seq = latest + 1` (`event.ts:294`);
  3. runs every registered projector for the event type **inside the same
     transaction** (`event.ts:320-322`);
  4. runs the optional `commit(seq)` hook (`event.ts:323`);
  5. upserts `event_sequence.seq` and inserts the `event` row (`event.ts:324-348`).
  So the log append and all read-model projections **commit atomically** (WAL).
- **Ordering**: the per-aggregate `seq` (`event_sequence` head + unique
  `(aggregate_id, seq)` index) is the sole ordering authority. The
  `session_message` projection carries that same `seq` and has a
  `uniqueIndex(session_id, seq)` (`packages/core/src/session/sql.ts:119-138`), so
  message order mirrors event order exactly. Message/session ids additionally
  encode time but are not the ordering key.
- **Atomicity / durability**: single immediate transaction per event, WAL +
  `synchronous = NORMAL` + `busy_timeout = 5000`
  (`packages/core/src/database/database.ts:27-31`). There is no temp-file-and-
  rename; durability is SQLite's. (The **legacy** store instead uses per-file
  JSON writes guarded by an in-process reentrant read/write lock per key,
  `packages/opencode/src/storage/storage.ts:218-299` — no fsync/rename dance
  either.)
- **Concurrency / OCC**: append uses an **implicit optimistic-concurrency guard**.
  On a normal publish there is no caller-supplied expected version — `seq` is
  just `latest + 1`, and the unique `(aggregate_id, seq)` index makes a concurrent
  double-append fail. On **replay** the caller *does* pass an expected `seq`, and
  the store enforces `seq === latest + 1`, dying with a "Sequence mismatch" if not
  (`event.ts:295-302`). Replays are also **idempotent**: if the incoming seq is
  `<= latest`, the store compares the stored row's id/type/data and returns
  quietly when they match, or dies with "Replay diverged" when they do not
  (`event.ts:262-290`). Duplicate event ids at a new seq are rejected
  (`event.ts:303-315`).
- **Ownership (multi-writer across hosts)**: `event_sequence.owner_id` plus the
  `strictOwner` replay flag implement per-aggregate ownership. A replay with
  `strictOwner:true` dies if the stored owner differs
  (`event.ts:254-261`); `claim(aggregateID, ownerID)` reassigns it
  (`event.ts:525-532`). This is the multi-host coordination primitive (see
  Retention/multi-host).
- **Delivery / idempotence at the prompt layer**: user prompts are a two-phase,
  idempotent-by-message-id flow. `SessionInput.admit` first looks up an existing
  row by `messageID` and returns it if present, otherwise publishes
  `session.next.prompt.admitted` recording the `admitted_seq`
  (`packages/core/src/session/input.ts:41-81`); on a lifecycle-conflict defect it
  re-reads and returns the stored admission. Later, the input is **promoted** to a
  `session.next.prompted` event recording `promoted_seq`
  (`input.ts:118-168`, `216-243`). `session_input` has unique indexes on both
  `admitted_seq` and `promoted_seq` (`session/sql.ts:157-165`). Delivery is thus
  **at-least-once with dedup by message id**, split into `"steer"` (promoted at a
  turn cutoff) and `"queue"` (promoted one-at-a-time when idle)
  (`packages/schema/src/session-delivery.ts:5-6`, `input.ts:245-288`).

## Read and resume path

- **Session detail / listing** read the **projections**, not the log. `get` is a
  single-row `SELECT` from `session` (`packages/core/src/session/store.ts:35-38`);
  `messages` is a seq-ordered, cursor-paginated `SELECT` from `session_message`
  (`packages/core/src/session.ts:304-337`); `list` is a keyset-paginated `SELECT`
  from `session` ordered by `(time_created, id)` with an anchor for
  previous/next paging (`session.ts:268-303`).
- **Model-visible context** (`context` / `runnerContext`) is a
  **compaction- and epoch-aware fold** of `session_message`, not a raw read
  (`packages/core/src/session/history.ts:66-99`). `load` finds the latest
  `compaction` message and the context-epoch `baseline_seq`, then selects messages
  with `seq >= compaction.seq` (plus `system` messages after the baseline), so the
  returned transcript is the post-compaction window carried forward from the
  epoch baseline (`history.ts:24-80`). `runnerContext(sessionID, baselineSeq)` is
  the same fold parameterized for a live run (`history.ts:82-99`).
- **Resume of execution** is orchestrated by `SessionExecution`/`SessionRunner`
  (`packages/core/src/session/execution.ts`), backed by a per-key **single-writer
  run coordinator**: it serializes execution per session id while letting
  different sessions run concurrently, coalesces follow-up wakeups, and supports
  interrupt (`packages/core/src/session/run-coordinator.ts:5-104`). `prompt` calls
  `execution.wake(sessionID)` after admitting input (`session.ts:382`), and
  `resume(sessionID)` calls `execution.resume` (`session.ts:426-429`). So resume
  reads the durable projections to reconstruct context, then drives the model.
- **Live tail**: `events({sessionID, after})` returns
  `EventV2.durable({aggregateID, after})`, which reads historical rows after the
  cursor and then switches to a live subscription via a per-aggregate wake PubSub
  (`packages/core/src/event.ts:565-604`). This is the reconnect/catch-up path for
  UIs.
- **Bounds / pagination**: `messages` and `history` take explicit `limit` +
  cursor/anchor (`session.ts:304-359`); `history` returns `hasMore`
  (`readAggregate` fetches `limit + 1`, `event.ts:87-107`). No hard transcript-size
  cap was found in the v2 read path; the model-visible window is bounded by
  compaction, not by a row cap.

## Listing, summaries, and search

- **Listing** is SQL over the `session` projection, ordered by
  `(time_created, id)` with keyset anchors, filtered by directory / project /
  workspace (`packages/core/src/session.ts:268-303`; indexes
  `session_project_idx`, `session_workspace_idx`, `session_parent_idx`,
  `packages/core/src/session/sql.ts:61-65`). No directory scan, no log replay. No
  scale numbers are quoted in-source.
- **Summary sidecar / read model**: the `session` row **is** the denormalized
  read model and is maintained at write time in the same transaction as the log
  append. It denormalizes `title`, `slug`, `directory`/`path`, `agent`, `model`,
  `share_url`, running `cost` and the six token counters, `summary_additions/
  deletions/files/diffs`, `revert` state, `permission`, and `time_*`
  (`packages/core/src/session/sql.ts:22-60`). Token/cost totals are incremented
  transactionally as step/part events project (`applyUsage`,
  `packages/core/src/session/projector.ts:90-110`, `312-329`), and reversed with
  `sign = -1` when a message/part is removed — so the denormalized totals stay
  consistent with the log by construction.
- **Search**: there is **no FTS or vector subsystem**. `list`'s `search` is a
  `LIKE '%...%'` filter over `session.title` only
  (`packages/core/src/session.ts:277`). Nothing to bootstrap or keep in sync.

## Entry/message structure and versioning

There are two layered structures: the **durable event** (what is stored) and the
**projected message** (what `context`/`messages` return).

- **Event envelope** (`packages/schema/src/event.ts:29-40`, `47-73`): `id`
  (`evt_…`), `type` (dotted string, e.g. `session.next.step.started`), optional
  `metadata`, optional `location`, a per-type `data` struct, and a `durable`
  descriptor `{ aggregateID, seq, version }`. Every durable session event shares a
  `Base` of `{ timestamp, sessionID }` (`packages/schema/src/session-event.ts:27-30`).
  The event catalog is large and fine-grained — agent/model switch, moved,
  prompted/prompt.admitted, context.updated, synthetic, shell start/end, step
  start/end/failed, text start/delta/end, reasoning start/delta/end, tool
  input/called/progress/success/failed, retried, compaction start/delta/end, and
  revert staged/cleared/committed (`session-event.ts:54-512`). **Stream-fragment
  events (`*.delta`) are deliberately non-durable** — only the `*.ended`
  full-value boundary is replayable (`session-event.ts:209-210`, `247`, `291`);
  the `DurableDefinitions` inventory excludes the deltas (`session-event.ts:448-477`
  vs the full `Definitions` `479-512`).
- **Projected message type** (`packages/schema/src/session-message.ts:200-213`):
  a tagged union `Message = AgentSwitched | ModelSwitched | User | Synthetic |
  System | Shell | Assistant | Compaction`. An `Assistant` message carries a
  `content` array of `text | reasoning | tool` parts, a `ToolState` union
  (`pending | running | completed | error`, `session-message.ts:81-119`),
  `snapshot {start, end, files}`, `finish`, `cost`, and `tokens`
  (`session-message.ts:164-189`). The projector folds streaming events into these
  rows via an immer-based updater keyed by assistant/tool/text/reasoning id
  (`packages/core/src/session/message-updater.ts:78-395`).
- **Store interpretation**: `event.data` is stored as JSON `TEXT`
  (`packages/core/src/event/sql.ts:19`) but is **decoded and validated through
  Effect Schema on both append and read** (`Schema.encodeUnknownSync` /
  `decodeSerializedEvent`, `event.ts:50-61`, `250-251`). The projection tables'
  `data` columns are likewise typed JSON (`session/sql.ts:130`, `77`, `92`). The
  store relies on `event.id` (primary key) and `(aggregate_id, seq)` for identity/
  dedup; message identity is `session_message.id` (the `msg_…`).
- **Versioning**: each durable event definition carries an explicit integer
  `version`, and the stored `event.type` is the **versioned type**
  `"${type}.${version}"` (`packages/schema/src/event.ts:118`,
  `packages/core/src/event.ts:343`). Most session events are `version: 1`; step
  settlement events (`Step.Ended`, `Step.Failed`) are `version: 2`
  (`session-event.ts:44-49`, `162-194`). The durable manifest is a map keyed by
  versioned type (`packages/schema/src/event.ts:105-113`), and the read path
  decodes by looking the definition up by versioned type — so **multiple event
  versions can coexist in the log**. This is a genuine schema-version ratchet at
  the event level, complementing additive schema defaults.
- **DB schema migrations** for the projections are a forward-only, generated set
  applied at startup (`packages/core/src/database/migration.ts` +
  `migration.gen.ts`, invoked from `database.ts:33`), e.g.
  `20260511173437_session-metadata` and `20260601010001_normalize_storage_paths`.
- **Legacy on-disk format** evolved via numbered filesystem migrations recorded
  in a `migration` marker file (`packages/opencode/src/storage/storage.ts:81-243`),
  which is how project/session/message/part JSON was reshuffled between layouts.

## Compaction and history management

- **Compaction is upstream of the store** (a run-time concern) but leaves a
  **durable marker** in the log. `SessionCompaction` summarizes the transcript
  with the LLM when the estimated request exceeds the model's context minus a
  buffer (`packages/core/src/session/compaction.ts:225-236`), publishing
  `session.next.compaction.started` and then
  `session.next.compaction.ended { reason, text (summary), recent }`
  (`compaction.ts:186-222`). The `Ended` event projects into a `compaction`
  message row (`packages/core/src/session/message-updater.ts:377-389`).
- **The model-visible view shrinks; the durable log does not.** `context` reads
  only messages at/after the latest `compaction` seq (plus post-baseline system
  messages) (`packages/core/src/session/history.ts:13-80`), so the summary plus
  the retained "recent" tail replaces the pre-compaction body in what the model
  sees, while every original event stays in the `event` table.
- **Context epoch / baseline**: the `session_context_epoch` row holds a
  `baseline` + `baseline_seq` + a `SystemContext.Snapshot`
  (`packages/core/src/session/sql.ts:168-176`). It is advanced/replaced as the
  system context reconciles across turns and compactions
  (`packages/core/src/session/context-epoch.ts:40-78`), and is **reset** on
  `session.next.moved` and on revert-commit (`projector.ts:256`, `452`). Resume
  reads honor this baseline so a moved/reverted session rebuilds its system
  context cleanly.

## Rewind, checkpoints, and fork

- **Rewind is expressed as appended events, interpreted at fold/replay time.** A
  three-step flow: `revert.stage` restores files and publishes
  `session.next.revert.staged` recording the `Revert.State`
  (`packages/core/src/session/revert.ts:60-96`); `revert.clear` restores and
  publishes `revert.cleared` (`revert.ts:98-111`); `revert.commit` publishes
  `revert.committed { messageID }` (`revert.ts:113-121`). The **committed**
  projector then physically **truncates the projections** — deletes
  `session_message` rows with `seq > boundary.seq`, deletes later `session_input`
  rows, clears `revert`, and resets the context epoch
  (`packages/core/src/session/projector.ts:415-454`). The underlying `event` rows
  are **not** deleted; because `revert.committed` is itself a durable event later
  in the log, replaying the log re-applies the same truncation deterministically.
  This is the append-marker-interpreted-at-replay pattern (contrast the legacy /
  in-place styles elsewhere in the corpus).
- **File-state checkpoints** are git-tree snapshots, not inlined content. `stage`
  captures the worktree (`Snapshot.capture`) and later restores selected paths
  from a captured tree (`packages/core/src/session/revert.ts:60-96`); the snapshot
  service maintains a **per-project shadow git directory** under
  `~/.local/share/opencode/snapshot/<projectID>/<hash(worktree)>` and captures/
  restores via git tree objects (`packages/core/src/snapshot.ts:91-135`,
  `43-91`). Each assistant step records `snapshot.start` / `snapshot.end` tree ids
  and the `files` it touched (`packages/schema/src/session-message.ts:171-175`;
  set from `Step.Started`/`Step.Ended` in `message-updater.ts:196-221`). Cost is
  git-dedup, not per-turn full copies.
- **Fork**: no first-class `fork`/branch operation was found on the v2
  `SessionV2.Service` (`session.ts:113-180`). Parent/child lineage exists
  (`parent_id`, below) but a copy-plus-lineage or shared-prefix fork primitive was
  not present in the v2 store at this commit (see Open questions).

## Subagents and nested sessions

- A subagent is a **first-class sibling session** with its own `sessionID` (its
  own event stream / aggregate), linked to its parent by `parent_id`. The `task`
  tool creates the child with `parentID: ctx.sessionID`
  (`packages/opencode/src/tool/task.ts:155-172`); the column and index are
  `session.parent_id` + `session_parent_idx`
  (`packages/core/src/session/sql.ts:31`, `64`), surfaced as `Info.parentID`
  (`packages/core/src/session/info.ts:19`, `packages/schema/src/session.ts:20`).
- The child **isolates its own transcript** (separate aggregate, separate
  `session_message` rows); it does not write into the parent's log. Parent-walk
  logic exists in the tooling (`task.ts:107-110` walks `current.parentID`).
- **Delete/cascade**: within the DB, `session` rows do **not** cascade on
  `parent_id` (only `project_id` cascades, `session/sql.ts:26-30`), so deleting a
  parent session row does not delete children — they would orphan with a dangling
  `parent_id` (matching T3's behavior). Nesting bounds live in the tool/agent
  layer, not the store schema (not enumerated here; see Open questions).

## Retention, deletion, and multi-host

- **Retention**: none found. No TTL, lifecycle sweep, or scheduled cleanup of the
  `event` table or projections. The log is retained indefinitely. There is a
  `time_archived` field on `session` (`packages/core/src/session/sql.ts:59`,
  surfaced as `Info.time.archived`) but no v2 archive/expiry operation was traced.
- **Deletion**: the v2 `SessionV2.Service` exposes **no delete**. Deletion is
  possible two ways: (1) the legacy `session.deleted` event projects to
  `db.delete(SessionTable)`, which **cascades** to `message`/`part`/
  `session_message`/`session_input`/`session_context_epoch`/`todo` via
  `onDelete: "cascade"` foreign keys (`packages/core/src/session/projector.ts:259-261`,
  `session/sql.ts:72-176`) — but the `event` rows for that aggregate remain;
  (2) `EventV2.remove(aggregateID)` transactionally deletes both `event_sequence`
  and `event` rows for the aggregate (`packages/core/src/event.ts:514-523`), which
  is the only path that physically erases the log. So "delete the projection" and
  "delete the log" are distinct operations.
- **Multi-host is a first-class path** here, unlike most of the corpus. Sessions
  synchronize across hosts by **replaying events**, gated by owner:
  - Each workspace runs a `syncWorkspaceLoop` that opens an SSE stream to a global
    endpoint and, on each `{type:"sync", syncEvent}` message, calls
    `events.replay(syncEvent, { publish: true, ownerID: space.id })`
    (`packages/opencode/src/control-plane/workspace.ts:395-409`); on connect it
    back-fills via a `syncHistory` fetch that `replay`s each missing event
    (`workspace.ts:345-363`).
  - The instance sync HTTP API exposes `replay` (accept a batch, `replayAll(...,
    { ownerID, strictOwner: true })`), `steal` (reassign a session's owning
    workspace via `session.setWorkspace`), and `history` (return `event` rows the
    caller is missing, computed from a per-aggregate high-water map)
    (`packages/opencode/src/server/routes/instance/httpapi/handlers/sync.ts:34-85`).
  - Ownership transfer uses `events.claim(sessionID, workspaceID)`
    (`workspace.ts:589`). The `strictOwner` replay guard prevents two hosts from
    diverging a shared aggregate (`packages/core/src/event.ts:254-261`).
  The append transaction is `behavior: "immediate"` per DB, and cross-host safety
  rests on **owner claims + idempotent, expected-seq replay**, not on a shared
  filesystem. This is a real distributed event-sync design layered on the same
  append-only log.

## Interop with foreign session stores

Not applicable as a session-store feature in the v2 core. OpenCode's own history
has a **self-interop / migration** story rather than a foreign-store one: the
legacy filesystem store ran numbered migrations that discovered old
project/session/message/part JSON layouts and rewrote them into the current
layout (`packages/opencode/src/storage/storage.ts:81-243`), and the v2 projector
ingests legacy v1 `session.*`/`message.*` durable events into the SQL projections
(`packages/core/src/session/projector.ts:215-330`,
`packages/schema/src/durable-event-manifest.ts:12-15`). No discovery/import of
*other products'* native session stores (Claude Code, Codex, etc.) was found in
the store layer.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* OpenCode's v2 store is a second strong
event-sourced datapoint alongside T3 Code, and it is architecturally close to our
target: **the durable session is an append-only `event` log keyed by
`(aggregate_id = sessionID, seq)`, and every session/message/summary/context view
is a rebuildable SQLite projection materialized by projectors that run inside the
same transaction as the append.** What it adds beyond T3:

- **Per-aggregate sequence as the only ordering + OCC key.** OpenCode keys and
  orders strictly per session (`event_sequence.seq` + unique `(aggregate_id,
  seq)`), with no global autoincrement. On normal publish OCC is implicit
  (`seq = latest + 1`), and on replay it becomes an **explicit expected-version
  precondition** with idempotent match-or-diverge semantics
  (`packages/core/src/event.ts:262-302`). This is exactly the expected-version
  contract our design wants, and it shows one store supporting both the
  single-writer fast path and the replicated/replay path.
- **Atomic append + projection + local `commit(seq)` hook** in one transaction
  (`event.ts:237-353`, `118-124`) is a clean way to keep denormalized read models
  (token/cost totals, input queue) consistent with the log by construction. We
  should keep the transactional-project step and consider an equivalent local
  commit hook for operational side-state.
- **Owner-scoped, idempotent multi-host replay** (`owner_id`, `claim`,
  `strictOwner`, SSE sync loop, `steal`, `history` back-fill) is the most
  transferable idea here and the piece T3 lacked. It demonstrates that an
  append-only per-aggregate log can be **synchronized across hosts by replaying
  events with an ownership guard**, no shared filesystem — directly relevant to a
  multi-host Session Store.
- **Durable events are the full-value boundaries; stream deltas are non-durable.**
  Only `*.ended` events (full text/reasoning/tool-input values) are replayable;
  `*.delta` fragments are live-only (`session-event.ts:209-210`, `448-477`). This
  keeps the log compact and replay deterministic — a good rule for our event
  taxonomy (persist settled facts, stream the in-between).
- **Explicit per-event `version` with versioned stored type**
  (`packages/schema/src/event.ts:118`, `packages/core/src/event.ts:343`) lets
  multiple event versions coexist in one log and decodes by versioned type. This
  is a cleaner evolution ratchet than T3's implicit additive-defaults approach and
  worth adopting.
- **Rewind as an appended marker re-applied at replay** (`revert.committed`
  truncating projections while the log keeps every event,
  `projector.ts:415-454`) validates our append-only rewind model.
- **Cautions**: (1) **No retention / log truncation / snapshotting** — the log
  grows unbounded and projection rebuild is a full per-aggregate replay; our
  design needs the snapshot/retention story theirs lacks. (2) **Two live models in
  one tree** (filesystem JSON vs event-sourced SQLite) is a migration-cost signal:
  a clean cutover and a legacy-ingest path (as their v1-event projector shows) are
  worth planning up front. (3) **Delete is ambiguous** — deleting the projection
  (cascade) leaves the log intact, and only `EventV2.remove` erases the log; we
  must decide deliberately whether "delete" means "forget the view" or "erase the
  facts," especially given the privacy implications of an indefinitely retained
  log. (4) **No child cascade** on parent-session delete (children orphan); our
  design should define subagent lifecycle on parent delete/rewind explicitly.

## Open questions

- **Snapshotting / log growth**: no truncation, compaction, or periodic snapshot
  of the `event` table was found; whether large installs ever bound the log or
  precompute projection snapshots (beyond `session_context_epoch`) is unresolved.
- **Session delete UX in v2**: the v2 `SessionV2.Service` exposes no `delete`;
  which layer (server handler, control plane) drives `session.deleted` /
  `EventV2.remove`, and whether it erases the log or only the projection in the
  shipping product, was not fully traced.
- **Fork/branch in v2**: parent/child lineage exists, but no copy-plus-lineage or
  shared-prefix fork operation was found on the v2 store; whether fork is a
  higher-layer feature (or not yet ported from legacy) is unresolved.
- **Subagent nesting bounds and cascade**: the depth/concurrency limits and the
  intended child lifecycle on parent delete/rewind/crash were not pinned down in
  the store layer.
- **Filesystem vs SQLite authority in the shipped binary**: `packages/opencode`
  still imports the legacy filesystem `Storage` in many paths (compaction, revert,
  share, message-v2) while `packages/server` uses `SessionV2`; exactly which store
  is authoritative for a given command in the default distribution was not
  exhaustively mapped.
- **`shell`/`skill`/`compact`/`wait` unavailable**: these return
  `OperationUnavailableError` in v2 at this commit (`session.ts:387-424`); whether
  that is a temporary migration state or a deliberate removal is unclear.
</content>
</invoke>

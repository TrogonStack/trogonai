# Codex CLI: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local shallow checkout of `openai/codex`
(`https://github.com/openai/codex.git`) at commit
`8d34c0667215f9ae4f8a11678e27752d8a4a120f` (committed 2026-07-23). Every
citation below was verified against this exact commit on 2026-07-23. Paths are
relative to the `codex-rs/` Rust workspace unless stated otherwise; citations
use `path:line` shorthand.

Authoritative anchors:

- `openai/codex` @ `8d34c0667215f9ae4f8a11678e27752d8a4a120f`
- `codex-rs/rollout/**` (rollout persistence + discovery), `codex-rs/state/**`
  (SQLite state runtime), `codex-rs/thread-store/**` (thread/lineage reader),
  `codex-rs/protocol/src/protocol.rs` (durable record types),
  `codex-rs/core/src/session/rollout_reconstruction.rs` (resume replay).

> Scope note. Codex has **two persistence tiers that both ship in the same
> binary and are kept in sync by design**:
>
> 1. **The rollout JSONL log** — the durable source of truth. One append-only
>    `.jsonl` file per thread under `~/.codex/sessions/YYYY/MM/DD/`
>    (`codex-rs/rollout/src/recorder.rs:1536-1555`). This is unambiguously
>    session-as-log.
> 2. **A derived SQLite `state` database** — a rebuildable index/projection of
>    the JSONL logs (thread metadata for listing, plus an optional per-item
>    "thread history" projection). It is backfilled and read-repaired from the
>    logs; the logs win on any discrepancy
>    (`codex-rs/rollout/src/state_db.rs:495-620`).
>
> The design is therefore closer to our event-sourced target than a first
> glance at "`.jsonl` files" suggests: the log is authoritative, and everything
> queryable (listing index, per-turn/per-item tables) is a projection with an
> explicit rebuild path. This dossier centers the log and documents the SQLite
> projection where it clarifies listing, resume, or consistency.

## The storage model

The durable session is an **append-only JSONL rollout file**. The recorder's
own doc comment states it plainly (`codex-rs/rollout/src/recorder.rs:75-82`):

```rust
/// Writes canonical session rollout items to JSONL.
///
/// Rollouts are recorded as JSONL and can be inspected with tools such as:
/// $ jq -C . ~/.codex/sessions/rollout-2025-05-07T17-24-21-...-....jsonl
```

Each line is a `RolloutLine` — a timestamp, an optional monotonic `ordinal`, and
a flattened, tagged `RolloutItem` payload
(`codex-rs/protocol/src/protocol.rs:3386-3393`):

```rust
pub struct RolloutLine {
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    #[serde(flatten)]
    pub item: RolloutItem,
}
```

`RolloutItem` is a closed, `snake_case`-tagged union
(`codex-rs/protocol/src/protocol.rs:3191-3206`):

```rust
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RolloutItem {
    SessionMeta(SessionMetaLine),
    ResponseItem(ResponseItem),
    InterAgentCommunication(InterAgentCommunication),
    InterAgentCommunicationMetadata { trigger_turn: bool },
    Compacted(CompactedItem),
    TurnContext(TurnContextItem),
    WorldState(WorldStateItem),
    EventMsg(EventMsg),
}
```

The first line of every file is always a `SessionMeta` record; the rest are the
turn-by-turn stream (model response items, turn-context snapshots, compaction
markers, world-state snapshots, and selected protocol events).

What is authoritative vs. derived:

- **Authoritative**: the per-thread rollout `.jsonl` file. It is the only thing
  written on the live path, appended item-by-item, and it is what resume reads.
- **Derived / rebuildable**: the SQLite `state` DB. `threads` is a listing
  index row per thread (`codex-rs/state/migrations/0001_threads.sql:1-25`); the
  optional `thread_history` projection (`thread_turns`, `thread_items`, and a
  `thread_history_projection_state` cursor) materializes per-turn/per-item rows
  out of the log (`codex-rs/state/thread_history_migrations/0001_thread_history.sql`).
  Both are backfilled from and read-repaired against the JSONL
  (`codex-rs/rollout/src/state_db.rs:495-620`); a divergence triggers a rescan
  of the file and an upsert, never the reverse.

Conceptual model: **session-as-log** at the durable tier (an append-only event
log of `RolloutItem`s per thread), with **session-as-row** (listing metadata)
and a projected **session-as-turns/items** read model layered on top in SQLite.

## Keying and identity

- **Store scope / on-disk layout**: rollouts live under
  `$CODEX_HOME/sessions/<year>/<MM>/<DD>/` with archived threads under
  `$CODEX_HOME/archived_sessions/` (`codex-rs/rollout/src/lib.rs:25-26`,
  `recorder.rs:1536-1555`). The path is time-sharded by *creation* date, not by
  project/cwd. There is one SQLite `state` DB per `$CODEX_HOME`
  (`codex-rs/rollout/src/state_db.rs:44-74`).
- **Filename encodes identity + ordering**: the file is
  `rollout-<YYYY-MM-DDThh-mm-ss>-<thread_id>.jsonl`
  (`recorder.rs:1553`). Colons are replaced with `-` for filesystem
  compatibility (`recorder.rs:1545-1548`). So the filename carries both a
  creation timestamp and the thread id, and the listing layer parses both back
  out (`parse_timestamp_uuid_from_filename`, used in `recorder.rs:1124`).
- **Id minting**: a `ThreadId` (and its derived `SessionId`) is a **UUIDv7**,
  minted client-side with `Uuid::now_v7()`
  (`codex-rs/protocol/src/thread_id.rs:21-25`, `session_id.rs:19-24`). The type
  doc says so explicitly: "Codex-generated thread IDs are UUIDv7, and some use
  cases rely on that" (`thread_id.rs:12-14`). `SessionId` is derived from the
  `ThreadId`'s uuid (`session_id.rs:54-57`). Because UUIDv7 is time-ordered, the
  id itself encodes creation ordering, and the filename timestamp prefix
  reinforces it.
- **Keying inside the record**: `SessionMeta` carries `session_id`, `id`
  (thread id), `forked_from_id`, `parent_thread_id`, `cwd`, `originator`,
  `source`, `history_mode`, and subagent fields
  (`protocol/src/protocol.rs:3063-3121`). So the durable key hierarchy is
  effectively `thread_id` (primary) with optional `forked_from_id` /
  `parent_thread_id` lineage pointers and a `cwd` for scoping.
- **Listing scope**: global within a `$CODEX_HOME`, then *filtered*. Listing
  accepts allowed `SessionSource`s, model-provider filters, and **`cwd_filters`**
  (`recorder.rs:295-307`). So a per-project view is a cwd filter over the global
  set, not a separate store. The SQLite `threads.cwd` column plus cwd sort
  indexes back this (`state/migrations/0027_threads_cwd_sort_indexes.sql`).
- **Relocation / rename**: identity is the immutable `thread_id` in the
  filename and `SessionMeta`; the working directory is data (`SessionMeta.cwd`,
  `TurnContextItem.cwd`), so moving the working tree does not change identity. A
  new `TurnContextItem` with the new `cwd` is appended on the next turn. cwd
  values are normalized for comparison at the SQLite boundary
  (`normalize_cwd_for_state_db`, `state_db.rs:593`).

## The store interface

There is no *pluggable* store trait exposed to third parties; the store is the
`codex-rollout` crate's public surface plus the `codex-state` SQLite runtime.
Reconstructed from the source, the effective contract is:

**Rollout log (authoritative), `RolloutRecorder` — `codex-rs/rollout/src/recorder.rs`:**

- `RolloutRecorder::new(params)` — open a recorder. `params` is either
  `Create { session_id, conversation_id, forked_from_id, parent_thread_id,
  source, thread_source, originator, base_instructions, dynamic_tools,
  history_mode, subagent_history_start_ordinal, ... }` or `Resume { path }`
  (`recorder.rs:90-112`, constructor at `recorder.rs:785-903`). Create defers
  file creation (writes the `SessionMeta` line lazily on first flush);
  Resume reopens the existing file for append and recovers the ordinal cursor
  (`recorder.rs:856-868`).
- `record_canonical_items(&[RolloutItem]) -> io::Result<()>` — queue items for
  the background writer (append). Non-blocking; sends `RolloutCmd::AddItems`
  (`recorder.rs:909-921`).
- `persist() -> io::Result<()>` — materialize the file and flush all buffered
  items; idempotent, retryable (`recorder.rs:927-...`, `RolloutCmd::Persist`).
- `flush()` — barrier that returns once buffered writes are on disk
  (`RolloutCmd::Flush`, `recorder.rs:1616-1621`).
- `shutdown() -> io::Result<()>` — drain then stop the writer task
  (`recorder.rs:1050-1072`).
- `get_rollout_history(path) -> InitialHistory` — full ordered read of a file
  into a `Resumed` history (`recorder.rs:1030-1045`).
- `list_threads(state_db, config, page_size, cursor, sort_key, sort_direction,
  allowed_sources, model_providers, cwd_filters, default_provider, search_term)
  -> ThreadsPage` — paginated listing (`recorder.rs:295-322`).
- Free function `append_rollout_item_to_path(path, &RolloutItem)` — append a
  single item to an *unloaded* thread's file (metadata updates), recovering the
  ordinal first (`recorder.rs:1819-1827`).

**SQLite state runtime (derived), `StateDbHandle = Arc<codex_state::StateRuntime>` — `codex-rs/rollout/src/state_db.rs`:**

- `init(config) -> Option<StateDbHandle>` / `try_init(config)` — open the
  SQLite runtime, run migrations, kick off rollout-metadata backfill
  (`state_db.rs:44-74`).
- `list_threads_db(...)` — list thread ids/metadata straight from SQLite for
  parity checks and fast listing (`state_db.rs:306`, `363`).
- `reconcile_rollout_items(...)` — "Reconcile rollout items into SQLite,
  falling back to scanning the rollout file" (`state_db.rs:495`).
- `read_repair_rollout_path(...)` — recompute a thread's metadata from its file
  and upsert if SQLite diverged (fast path = path/cwd/archived fixups; slow path
  = full rebuild from rollout contents) (`state_db.rs:574-620`).
- `ctx.upsert_thread(&ThreadMetadata)` — write one listing row
  (`state_db.rs:607`).

The reader/lineage side is the `codex-thread-store` crate
(`thread-store/src/local/**`): `LocalThreadStore` resolves a thread id to its
rollout path, follows fork `history_base` pointers to assemble a
`RolloutLineage` of physical segments, and reads projected turns/items
(`thread-store/src/local/rollout_lineage.rs:31-55`).

The key contract takeaway: **every write goes to the JSONL log; SQLite is only
ever written as a reconciled projection of that log**, and reads can be served
from either tier with the log as the tiebreaker.

## Write and append path (ordering, durability, concurrency, delivery)

- **Append, never rewrite.** The writer opens the file with
  `OpenOptions::new().read(true).append(true).create(true)`
  (`recorder.rs:1573-1577`) and each item is serialized to one line + `\n` and
  written (`JsonlWriter::write_line`, `recorder.rs:1896-1902`). There is no
  in-place mutation of prior lines on the normal path.
- **Single-writer background task.** `RolloutRecorder` owns a bounded
  `mpsc::channel::<RolloutCmd>(256)` feeding one spawned `rollout_writer` task
  that owns the file handle (`recorder.rs:872-896`, `1753-1783`). All appends
  for a live session funnel through that one task, so the model is
  **single-writer-per-session**. Callers never touch the file directly.
- **Ordering** is positional line order, reinforced by an explicit per-record
  `ordinal`. `ThreadHistoryMode::Paginated` sessions carry a monotonic `ordinal`
  starting at 0 and incremented per written record
  (`codex-rs/rollout/src/ordinal.rs:16-47`); `Legacy` sessions omit it
  (`ordinal: None`). On resume the ordinal cursor is recovered by
  reverse-scanning to the last record and taking `last.ordinal + 1`
  (`ordinal.rs:50-95`). So ordering is both physical (line order) and logical
  (dense per-thread ordinal), which is exactly what the SQLite projection cursor
  keys on.
- **Durability / atomicity.** Each write does `write_all` then `flush()` on the
  async file handle (`recorder.rs:1899-1900`), and on open the file is repaired
  to be newline-terminated so a torn final line cannot corrupt the next append
  (`ensure_rollout_is_newline_terminated`, `recorder.rs:1848-1861`). There is no
  temp-file-and-rename and no explicit `fsync`/`sync_all` on the hot path;
  durability rests on append-only writes + flush + newline healing. I/O failure
  enters a **recovery mode**: the writer drops the file handle but keeps the
  unwritten suffix buffered, reopens, and retries on the next barrier
  (`RolloutWriterState` doc + `enter_recovery_mode` + `write_pending_with_recovery`,
  `recorder.rs:1582-1674`, `1630-1650`).
- **Concurrency model**: single-writer-per-session via the writer task; there is
  no caller-supplied expected-version precondition on append (no optimistic
  concurrency token). Cross-process safety relies on one live recorder per
  thread; `append_rollout_item_to_path` for unloaded threads recovers the
  ordinal from the file head/tail before appending
  (`recorder.rs:1819-1846`), but there is no lock protocol beyond append
  semantics.
- **What is persisted (policy filter).** Not every in-memory item is durable.
  `is_persisted_rollout_item` gates writes (`codex-rs/rollout/src/policy.rs:9-21`):
  session-meta, compaction markers, turn-context, world-state, and
  inter-agent-communication are always persisted; response items are filtered by
  `should_persist_response_item` (messages, reasoning, tool/function calls and
  outputs, web-search, image-gen, compaction — yes; `AdditionalTools`,
  `CompactionTrigger`, `Other` — no) (`policy.rs:39-59`); protocol `EventMsg`s
  are filtered by `should_persist_event_msg` (`policy.rs:87-...`).
- **Delivery semantics** to the store are **best-effort with in-process retry**,
  not at-least-once across crashes: items sit in `pending_items` and are drained
  on flush/persist/shutdown; if the process dies with items still buffered and
  the file unwritten, those items are lost (nothing is journaled outside the file
  itself). There is no client-side dedup id on the store — dedup, where it
  matters, happens at resume/reconstruction by interpreting the log.

## Read and resume path

- **Resume is a full ordered read of the log.** `RolloutRecorder::resume`
  reopens the file for append (`recorder.rs:856`), and the transcript is loaded
  via `get_rollout_history(path)`, which calls `load_rollout_items` and returns
  `InitialHistory::Resumed(ResumedHistory { conversation_id, history:
  Arc<Vec<RolloutItem>>, rollout_path })` (`recorder.rs:1030-1045`). The entire
  file is parsed into an in-memory `Vec<RolloutItem>`.
- **`InitialHistory` is the resume contract**
  (`protocol/src/protocol.rs:2555-2561`): `New`, `Cleared`,
  `Resumed(ResumedHistory)`, or `Forked(Vec<RolloutItem>)`. Fork/lineage helpers
  hang off it (`forked_from_id`, `session_cwd`, `get_rollout_items`,
  `protocol.rs:2571-...`).
- **Reconstruction is a replay, not a raw load.** `Session::
  reconstruct_history_from_rollout` scans the loaded items **newest-to-oldest**,
  stopping once it finds the newest surviving replacement-history checkpoint
  (compaction) and the required resume metadata, then replays only the buffered
  surviving tail forward "to preserve exact history semantics"
  (`core/src/session/rollout_reconstruction.rs:113-186`). This is where
  compaction markers, `ThreadRolledBack` events, `TurnContext` baselines, and
  `WorldState` snapshots are folded into the model-visible history.
- **Log vs cache on resume**: resume reads the durable JSONL directly (not a
  SQLite cache). The SQLite `thread_history` projection exists for *listing and
  UI paging over turns/items*, tracked by
  `thread_history_projection_state(next_rollout_byte_offset,
  next_rollout_ordinal)` (`state/thread_history_migrations/0001_thread_history.sql`),
  not for reconstructing the model context on resume.
- **Bounds / pagination**: the model-visible transcript is bounded by compaction
  (below), not by a row cap on read. Turn/item paging for the UI is keyed on
  `rollout_ordinal` with unique per-page indexes
  (`idx_thread_turns_page`, `idx_thread_items_page`), and turn byte offsets are
  recorded so a UI can seek into the file
  (`thread_history_migrations/0003_turn_rollout_positions.sql`).

## Listing, summaries, and search

- **Listing is DB-first with a filesystem-scan fallback.**
  `list_threads_with_db_fallback` (`recorder.rs:424-...`) serves listings from
  the SQLite `threads` index when it can, and falls back to
  `page_from_filesystem_scan` (`recorder.rs:1139`, scan driver near
  `recorder.rs:1268-1312`) — a bounded, reverse-chronological walk of the
  `sessions/YYYY/MM/DD` tree — when the DB is unavailable or a filtered/uncached
  listing is requested. The scan is explicitly capped: `scan_page_size =
  page_size * 8` clamped to `[256, 2048]`, and it reports `num_scanned_files` and
  a `reached_scan_cap` flag (`recorder.rs:1268-1312`). This is the stated scale
  guard — the scan does bounded work and surfaces when it truncated.
- **Repair modes**: listings run either `ScanAndRepair` (scan the files and
  reconcile SQLite) or `StateDbOnly` (trust the DB), chosen per call site
  (`recorder.rs:286-290`, `320`, `352`). Relationship-filtered listings "treat
  persisted state as authoritative" (`state_db.rs:430`) — i.e. use the DB
  without a rescan.
- **Summary sidecar = the `threads` row.** The SQLite `threads` table is the
  denormalized read model: `rollout_path`, `created_at`, `updated_at`, `source`,
  `model_provider`, `cwd`, `title`, `sandbox_policy`, `approval_mode`,
  `tokens_used`, `has_user_event`, `archived`, `archived_at`, `git_sha`,
  `git_branch`, `git_origin_url` (`state/migrations/0001_threads.sql:1-19`), later
  extended with `first_user_message`, `preview`, `name`, `is_pinned`,
  `thread_source`, `history_mode`, agent path/nickname, and recency/visible sort
  indexes (migrations `0007`, `0032`, `0041`, `0043`, `0030`, `0040`, `0013`,
  `0022`, `0036`). It is kept consistent with the log by backfill +
  read-repair: any mismatch recomputes metadata from the file and upserts
  (`state_db.rs:574-620`).
- **Search**: there is a `search` submodule (`search_rollout_matches`,
  `search_rollout_paths`, `first_rollout_content_match_snippet`,
  `rollout/src/lib.rs:78-79`) plus a `search_term` listing parameter
  (`recorder.rs:307`). Search is over rollout content/paths (content scan +
  metadata columns), not a separate FTS/vector engine — no `CREATE VIRTUAL
  TABLE ... fts5` exists in the state migrations.

## Entry/message structure and versioning

- **Envelope**: every durable line is `RolloutLine { timestamp, ordinal?, item
  }` with the `item` flattened via a `type`/`payload` tag
  (`protocol/src/protocol.rs:3386-3393`, `3191-3206`). The serialized form uses a
  `RolloutLineRef` writer struct that skips `ordinal` when `None`
  (`recorder.rs:1867-1874`).
- **Session header**: `SessionMetaLine { #[serde(flatten)] meta: SessionMeta,
  git: Option<GitInfo> }` (`protocol.rs:3154-3160`), with a custom
  `Deserialize` that backfills `session_id` from `id` for older files
  (`protocol.rs:3162-3189`). `SessionMeta` is a large, additive struct
  (`protocol.rs:3063-3121`) — most new fields are `#[serde(default,
  skip_serializing_if = ...)]`, which is the primary evolution mechanism.
- **Payload shapes**: `ResponseItem` (the model-facing item; message, reasoning,
  tool/function call + output, web-search, image-gen, compaction variants —
  `policy.rs:39-59`), `TurnContextItem` (per-turn cwd, approval/sandbox/permission
  policy, model, effort, collaboration mode — `protocol.rs:3269-3313`),
  `CompactedItem` (the compaction marker; see below), `WorldStateItem`
  (`full` snapshot vs `patch`, `protocol.rs:3209-3224`), and `EventMsg`
  (protocol events like `TurnComplete`, `TurnAborted`, `ThreadRolledBack`).
- **Store interpretation**: the store *parses* items (it must, to filter by
  policy, to write session-meta first, to recover ordinals, and to reconstruct
  history), but it persists the full payload verbatim. Identity for dedup is not
  a store-level entry id; reconstruction interprets the *stream* (compaction
  checkpoints, rollback counters) to derive the surviving history.
- **Versioning**: two mechanisms. (1) **Additive serde defaults + legacy
  sniffing** — new fields default and are skipped when absent; the deserializer
  patches missing `session_id`, strips legacy "ghost snapshot" lines
  (`recorder.rs:1090-1111`), and rejects unknown `history_mode` values
  (`reject_unknown_thread_history_mode`, `recorder.rs:1075-1088`). (2) **A
  `history_mode` ratchet** — `ThreadHistoryMode::{Legacy, Paginated}` changes
  whether records carry ordinals and whether the paginated SQLite projection
  applies (`ordinal.rs:22-28`). (3) **SQLite schema migrations** are a
  forward-only numbered set (`state/migrations/0001..0043`, plus separate
  `logs_`, `goals_`, `memory_`, and `thread_history_` migrator sets,
  `state/src/migrations.rs:6-10`); an older binary tolerates a DB migrated by a
  newer one via `ignore_missing` (`migrations.rs:12-27`).

## Compaction and history management

- **Compaction is upstream of the store but leaves a durable marker in the
  log.** When context is compacted, Codex appends a `RolloutItem::Compacted`
  carrying `CompactedItem { message, replacement_history:
  Option<Vec<ResponseItem>>, window_number, first_window_id, previous_window_id,
  window_id }` (`protocol.rs:3226-3243`; written at
  `core/src/compact_remote.rs:281`, `compact_remote_v2.rs:302`). The comment on
  the compaction path notes it ensures "the live and persisted histories remain
  identical" (`core/src/compact.rs:77`).
- **The durable log keeps everything; the model-visible view shrinks.** The
  `replacement_history` field is the compacted (summarized) history that replaces
  the pre-compaction body for the model. On resume, reconstruction scans backward
  to the newest `Compacted` with a `replacement_history`, uses it as the base,
  and replays only the tail after it (`rollout_reconstruction.rs:156-186`) — so
  crossing a compaction boundary means "start from the summary, then apply
  everything appended since," while the original pre-compaction lines remain in
  the file.
- **Context windows are chained**: `window_number` (monotonic position),
  `first_window_id`, `previous_window_id`, `window_id` (UUIDv7s) form a window
  chain so reconstruction can identify the active window and detect legacy
  compactions lacking a window number (`rollout_reconstruction.rs:123-172`).
- **`WorldState` snapshots** (full baseline vs patch, `protocol.rs:3209-3224`)
  are the analogous mechanism for model-visible world-state diffing, replayed on
  resume (`rollout_reconstruction.rs:142`, `159`).

## Rewind, checkpoints, and fork

- **Rewind is an appended marker interpreted at replay.** A rollback is recorded
  as an `EventMsg::ThreadRolledBack { num_turns }` line; reconstruction, scanning
  in reverse, turns it into "skip the next N user-turn segments we finalize"
  (`rollout_reconstruction.rs:144-146`, `188-191`). Nothing is deleted from the
  file — the rolled-back turns remain durable and are simply excluded from the
  replayed model history. This is the append-marker-interpreted-at-replay pattern.
- **Fork is copy-plus-lineage with a shared-prefix pointer.** A forked thread
  gets its own new `thread_id` and a `SessionMeta.forked_from_id` pointing at the
  source (`protocol.rs:3068`), and resume distinguishes `InitialHistory::Forked`
  from `Resumed` (`protocol.rs:2560`). Physically, forks are stitched via
  `SessionMeta.history_base: Option<HistoryPosition>` — "Exclusive prefix of
  another paginated rollout inherited by this thread" (`protocol.rs:3107-3109`).
  The `codex-thread-store` `RolloutLineage` follows those `history_base` pointers
  to assemble the ordered physical segments of a logical forked history, guarding
  against cycles (`thread-store/src/local/rollout_lineage.rs:31-55`). So fork is
  a lineage of bounded rollout segments plus an inherited-prefix pointer, not a
  full copy in the common (paginated) case.
- **File-state / environment checkpoints**: there is no full working-tree
  snapshot store here. What is checkpointed durably per turn is *policy/context*
  state — `TurnContextItem` (cwd, approval/sandbox/permission profile, model,
  effort) and `WorldStateItem` — plus `GitInfo` (commit hash, branch, remote URL)
  captured into `SessionMeta` at session start (`recorder.rs:1791-1803`). Legacy
  "ghost snapshot" response items existed but are now stripped on load
  (`recorder.rs:1090-1111`).

## Subagents and nested sessions

- **A subagent is a first-class sibling thread with its own rollout file.** The
  spawn path creates a child thread whose `SessionMeta` carries
  `parent_thread_id`, plus `agent_nickname`, `agent_role`, and `agent_path`
  identifying the AgentControl-spawned sub-agent (`protocol.rs:3070`,
  `3080-3088`; spawn logic in `core/src/agent/control/spawn.rs`). The child gets
  its own `thread_id`, its own JSONL file, and its own SQLite row.
- **The durable parent-child link** is `SessionMeta.parent_thread_id`
  (`protocol.rs:3070`), surfaced in listing as `parent_thread_ids` on a
  `ThreadsPage` (`recorder.rs:1908-1917`) and stored via the
  `thread_spawn_edges` table (`state/migrations/0021_thread_spawn_edges.sql`).
- **Inherited vs isolated history**: a subagent can *inherit* a bounded prefix of
  the parent's context via `subagent_history_start_ordinal` — "First rollout
  ordinal that belongs to this subagent's own projected history. Earlier rollout
  records are inherited model context and stay out of child turn/item projection"
  (`protocol.rs:3110-3115`). So the child's own transcript is isolated from that
  ordinal onward, while earlier records are treated as inherited context. The
  ordinal cursor recovery specifically validates that a child's inherited prefix
  is complete before appending (`ordinal.rs:81-91`).
- **Cascade**: parent/child are separate files and separate rows; there is no
  automatic file cascade on parent delete. Lineage resolution guards against
  cycles and missing sources by erroring (`rollout_lineage.rs:42-52`), i.e. a
  missing parent produces a "malformed lineage" error rather than silent
  cascade.

## Retention, deletion, and multi-host

- **Retention**: none enforced by the store. Rollout files accumulate
  indefinitely under `sessions/YYYY/MM/DD`; no TTL or scheduled cleanup of JSONL
  or SQLite rows was found.
- **Archive vs delete**: the first-class lifecycle operation is **archive**, not
  delete. There is an `archived_sessions/` subtree
  (`ARCHIVED_SESSIONS_SUBDIR`, `lib.rs:26`), the `threads` row carries
  `archived`/`archived_at` (`0001_threads.sql:14-15`), listing supports
  `Active`/`Archived` filters (`ThreadListArchiveFilter`, `recorder.rs:280-284`),
  and read-repair reconciles the archived flag/timestamp with reality
  (`state_db.rs:595-601`). Deletion, where it happens, is a filesystem removal of
  the JSONL plus reconciliation of the SQLite row; there is no append-only
  tombstone marker for delete in the durable log.
- **Multi-host**: this is a **single-host, local-filesystem** design. The store
  assumes `$CODEX_HOME` is a local directory, one live recorder per thread, and
  best-effort append. There is no shared-filesystem coordination protocol, no
  remote writeback, and no crash-detection/lease mechanism across hosts. The
  SQLite runtime tolerates a *newer* binary having migrated the same DB
  (`migrations.rs:12-27`) but that is version tolerance, not multi-host
  concurrency. Cloud/remote Codex sessions are a separate product surface not in
  this local store.

## Interop with foreign session stores

`INTERACTIVE_SESSION_SOURCES` lists the session sources the local store treats
as first-class interactive threads: `Cli`, `VSCode`, and custom `"atlas"` /
`"chatgpt"` sources (`rollout/src/lib.rs:27-34`). These are Codex's *own*
frontends writing into the same rollout format, not foreign products' stores.
There is also an `external_agent_config_imports` migration
(`state/migrations/0038_external_agent_config_imports.sql`), which concerns
importing external *agent config*, not foreign session transcripts. No
discovery/import/resume of another product's native session store (Claude Code,
Gemini, etc.) was found in the store layer.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* Codex's durable session is **an
append-only per-thread JSONL event log (`RolloutLine` of tagged `RolloutItem`s)
that is the single source of truth, with a SQLite `state` DB acting as a purely
derived, rebuildable projection** (a listing index row per thread plus an
optional per-turn/per-item read model with an explicit
`next_rollout_byte_offset`/`next_rollout_ordinal` cursor). That is
architecturally the same CQRS shape our event-sourced Session Store targets,
implemented on plain files rather than a database log. Salient lessons:

- **A file log can still be event-sourced.** The pattern — authoritative
  append-only log + read-repaired SQL projections + a projection cursor — does
  not require a database as the log. But it *does* require the discipline Codex
  encodes: a dense per-record `ordinal`, byte-offset checkpoints for the
  projector, and a read-repair path that treats the log as the tiebreaker
  (`ordinal.rs`, `state_db.rs:574-620`, `thread_history_projection_state`). We
  should keep the ordinal/offset cursor idea even if our log lives in a DB.
- **Rewind and compaction as appended markers re-applied at replay** (`ThreadRolledBack`
  and `Compacted{replacement_history}`, `rollout_reconstruction.rs:144-186`)
  validate our append-only rewind/compaction model: the durable log never
  shrinks; the model-visible view is a reverse-scan fold that stops at the newest
  surviving checkpoint. Worth adopting the "newest replacement-history checkpoint
  wins, replay the tail forward" reconstruction rule.
- **Fork as a shared-prefix pointer (`history_base`) + lineage resolution**
  rather than a full copy (`rollout_lineage.rs`) is a cheap, log-friendly fork
  primitive we should mirror, together with cycle detection.
- **Subagents as first-class sibling logs with an inherited-prefix ordinal**
  (`subagent_history_start_ordinal`, `parent_thread_id`, `thread_spawn_edges`) is
  a clean way to isolate a child transcript while sharing inherited context —
  better than nesting child entries in the parent log.
- **UUIDv7 client-minted ids** give time-ordered identity without a server
  round-trip, and the filename encodes both timestamp and id for zero-DB
  discovery. Our design can keep server-authoritative ids but should note the
  value of time-ordered ids for listing.
- **Cautions**: (1) **Durability is best-effort** — items buffered in memory are
  lost on a hard crash before flush; there is no write-ahead journal outside the
  file, and no `fsync` on the hot path. Our store should decide its durability
  contract explicitly. (2) **No expected-version/OCC on append** — Codex relies
  on single-writer-per-thread, which does not generalize to multi-writer or
  multi-host; our design wants an expected-position precondition. (3) **No
  retention or log truncation** — logs grow unbounded and projection rebuild is a
  full file rescan; we need a snapshot/retention story. (4) **Single-host only** —
  there is no cross-host coordination; multi-host is out of scope for this store
  and would need to be designed in, not retrofitted.

## Open questions

- **Exact `fsync`/crash-consistency guarantees**: writes `flush()` the async
  handle but no `sync_all`/`fsync` was observed on the hot path; the precise
  durability guarantee under power loss (vs. process crash) was not pinned down.
- **When the `thread_history` projection is populated**: the projector schema and
  cursor exist (`thread_history_migrations/**`) and `runtime_thread_history_migrator`
  is present but marked `#[allow(dead_code)]` at this commit
  (`state/src/migrations.rs:46-49`), so which builds/flows actually populate
  `thread_turns`/`thread_items` (vs. keeping it dark) was not fully traced.
- **Delete semantics end-to-end**: archive is first-class, but exactly which UI
  path performs a hard JSONL delete, and how the SQLite row and any child threads
  are reconciled on that delete, was not exhaustively mapped.
- **Compaction v1 vs v2**: both `compact_remote.rs` and `compact_remote_v2.rs`
  write `CompactedItem`s; which is active by default and how `window_number`-less
  legacy compactions interact with v2 windows on resume was only partially
  traced (`rollout_reconstruction.rs:123-126`).
- **Search backing**: `search_rollout_matches` appears to scan content; whether
  any indexed acceleration exists beyond the `threads` metadata columns
  (there is no FTS5 virtual table) was not confirmed for large corpora.

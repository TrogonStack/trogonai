# LangGraph: how session (thread) state is stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local shallow checkout of `langchain-ai/langgraph`
(`https://github.com/langchain-ai/langgraph.git`) at commit
`31f90df3e6b0268fa77fd2d118a917d420b84a68` (committed 2026-07-21). Every
citation below was verified against this exact commit on 2026-07-23. Citations
use repo-relative `path:line` shorthand.

Authoritative anchors:

- `langchain-ai/langgraph` @ `31f90df3e6b0268fa77fd2d118a917d420b84a68`
- `libs/checkpoint/langgraph/checkpoint/base/__init__.py` (the `BaseCheckpointSaver`
  contract + `Checkpoint`/`CheckpointTuple`/`CheckpointMetadata` types)
- `libs/checkpoint/langgraph/checkpoint/base/id.py` (checkpoint id = UUID6)
- `libs/checkpoint-sqlite/langgraph/checkpoint/sqlite/__init__.py` (SQLite saver)
- `libs/checkpoint-postgres/langgraph/checkpoint/postgres/base.py` +
  `shallow.py` (Postgres saver, migrations, content-addressed channel blobs)
- `libs/checkpoint/langgraph/checkpoint/memory/__init__.py` (in-memory saver)

> Framing note. LangGraph is a **library**, not a CLI, so "a session" is a
> **thread** and the durable session state is a **checkpointer** (a pluggable
> `BaseCheckpointSaver`). Unlike the transcript-oriented products in this corpus,
> LangGraph does not store a message transcript per se — it stores **state
> snapshots of a graph's channels** plus **pending intermediate writes**, keyed
> by thread. A conversational message history is just one channel's value. This
> is the most explicitly *pluggable-store-interface* product in the study and the
> closest to a formal checkpoint/event model.

## The storage model

The durable unit is a **`Checkpoint`: a state snapshot at a point in time**
(`base/__init__.py:92-123`):

```python
class Checkpoint(TypedDict):
    v: int                    # format version, currently 1
    id: str                   # unique + monotonically increasing (sortable)
    ts: str                   # ISO 8601 timestamp
    channel_values: dict[str, Any]
    channel_versions: ChannelVersions            # channel -> monotonic version str
    versions_seen: dict[str, ChannelVersions]    # node -> channel -> version seen
    updated_channels: list[str] | None
```

Two durable record kinds make up a thread's state:

1. **Checkpoints** — immutable, id-addressed snapshots forming a **parent-linked
   chain** (each carries `parent_checkpoint_id`, `base/__init__.py:145`,
   `CheckpointMetadata.parents`, `:56-60`). A thread is the ordered chain of its
   checkpoints.
2. **Pending writes** — `PendingWrite = tuple[str, str, Any]` = `(task_id,
   channel, value)` (`base/__init__.py:31`), the intermediate outputs a task
   produced against a specific checkpoint, stored so an interrupted/failed
   superstep can resume without recomputation (`put_writes`, `:300-318`).

What is authoritative vs. derived:

- **Authoritative**: the checkpoint chain + pending writes, keyed by
  `(thread_id, checkpoint_ns, checkpoint_id)`. In the production Postgres saver
  the *channel values themselves* are stored **content-addressed by `(channel,
  version)`** in a separate `checkpoint_blobs` table with `ON CONFLICT DO
  NOTHING` — i.e. immutable, deduplicated, versioned value blobs
  (`postgres/base.py:57-65`, `131-135`). The `checkpoints` row then holds only the
  metadata + `channel_versions` *pointers*, and full `channel_values` are
  reconstructed by joining `checkpoint.channel_versions → checkpoint_blobs`
  (`SELECT_SQL`, `postgres/base.py:101-118`).
- **Derived / optional**: the "shallow" savers, which "ONLY store the most recent
  checkpoint and do NOT retain any history … with the exception of time travel"
  (`postgres/shallow.py:169-174`). A shallow saver is effectively a
  latest-state-only projection of the same model.

Conceptual model: **session-as-log-of-immutable-snapshots** (an append-only,
parent-linked checkpoint chain) with **content-addressed versioned channel
values** as the deduplicated value store and **per-checkpoint pending writes** as
the in-flight delta. It is materially closer to event-sourcing than the
transcript products: values are immutable and versioned, and the head state is a
fold over the chain.

## Keying and identity

- **Primary key = `thread_id`.** "The `thread_id` is the primary key used to
  store and retrieve checkpoints. Without it, the checkpointer cannot save state,
  resume from interrupts, or enable time-travel debugging"
  (`base/__init__.py:190-192`). It is client-supplied via
  `config["configurable"]["thread_id"]` (`base/__init__.py:186`).
- **Full key hierarchy** is `(thread_id, checkpoint_ns, checkpoint_id)` — the
  composite primary key of every backend's `checkpoints` table
  (`sqlite/__init__.py:150`, `postgres/base.py:55`). `checkpoint_ns` (namespace,
  default `''`) scopes checkpoints produced by nested/subgraph executions;
  `checkpoint_id` identifies the specific snapshot. Writes add `(task_id, idx)`
  to that key (`sqlite/__init__.py:161`, `postgres/base.py:75`).
- **Checkpoint id minting = UUID6.** `checkpoint["id"]` is a `uuid6()`
  (`base/__init__.py:18`; generator in `base/id.py`), chosen because it is "both
  unique and monotonically increasing, so can be used for sorting checkpoints
  from first to last" (`base/__init__.py:99-101`). So ids encode creation
  ordering (time-ordered UUID), and listing sorts by `checkpoint_id DESC`
  (`sqlite/__init__.py:240`, `338`).
- **Channel versions** are a separate monotonic sequence per channel:
  `get_next_version` defaults to integer increment (`current + 1`), and any
  override must be "monotonically increasing" (`base/__init__.py:692-711`). These
  versions are what `checkpoint_blobs` is keyed on.
- **Listing scope**: per-thread (and optionally per-namespace). `list`/`alist`
  take a `config` (thread) plus `filter`/`before`/`limit`
  (`base/__init__.py:253-275`). There is no cross-thread global enumeration in the
  saver contract — enumerating threads is the host application's concern (the
  LangGraph Platform server layer, not this OSS store).
- **Relocation / rename**: not applicable — there is no cwd/filesystem coupling.
  Identity is the caller's `thread_id`. Moving a thread's state is an explicit
  `copy_thread(source, target)` operation (`base/__init__.py:350-372`).

## The store interface

This is a **first-class pluggable interface**: `BaseCheckpointSaver[V]`
(`libs/checkpoint/langgraph/checkpoint/base/__init__.py:176-722`). Reproduced
verbatim (sync surface; every method has an `a`-prefixed async twin):

```python
class BaseCheckpointSaver(Generic[V]):
    serde: SerializerProtocol = JsonPlusSerializer()

    @property
    def config_specs(self) -> list: ...

    def get(self, config: RunnableConfig) -> Checkpoint | None: ...
    def get_tuple(self, config: RunnableConfig) -> CheckpointTuple | None:  # required
        raise NotImplementedError
    def list(self, config: RunnableConfig | None, *, filter=None,
             before=None, limit=None) -> Iterator[CheckpointTuple]:          # required
        raise NotImplementedError
    def put(self, config, checkpoint: Checkpoint, metadata: CheckpointMetadata,
            new_versions: ChannelVersions) -> RunnableConfig:                # required
        raise NotImplementedError
    def put_writes(self, config, writes: Sequence[tuple[str, Any]],
                   task_id: str, task_path: str = "") -> None:               # required
        raise NotImplementedError
    def delete_thread(self, thread_id: str) -> None: ...
    def delete_for_runs(self, run_ids: Sequence[str]) -> None: ...
    def copy_thread(self, source_thread_id: str, target_thread_id: str) -> None: ...
    def prune(self, thread_ids: Sequence[str], *, strategy="keep_latest") -> None: ...
    def get_delta_channel_history(self, *, config, channels) -> Mapping[str, DeltaChannelHistory]: ...
    def get_next_version(self, current: V | None, channel: None) -> V: ...
    def with_allowlist(self, extra_allowlist) -> BaseCheckpointSaver[V]: ...
```

Contract notes drawn from the docstrings:

- **Required to implement**: `get_tuple`, `list`, `put`, `put_writes` (and the
  async equivalents) — all `raise NotImplementedError` in the base
  (`:239-318`, `429-509`). `get`/`aget` are conveniences built on the tuple
  getters (`:227-237`, `417-427`).
- **`put`** stores one checkpoint and returns the updated config carrying the new
  `checkpoint_id`; `new_versions` are the channel versions as of this write
  (`:277-298`).
- **`put_writes`** stores intermediate `(channel, value)` writes for a `task_id`
  against the config's checkpoint; idempotent by `(…, task_id, idx)` (below).
- **`CheckpointTuple`** is the read shape: `(config, checkpoint, metadata,
  parent_config, pending_writes)` (`:139-146`).
- **Lifecycle ops**: `delete_thread`, `delete_for_runs`, `copy_thread`, `prune`
  (`keep_latest` vs `delete`) — all optional, several carrying explicit
  **`DeltaChannel` correctness warnings** that copies/prunes must preserve the
  ancestor chain back to the nearest `_DeltaSnapshot` or silently corrupt delta
  channels (`:320-415`, `540-580`).
- **Serialization** is itself pluggable via `SerializerProtocol`
  (`serde: SerializerProtocol = JsonPlusSerializer()`, `:209`), and the msgpack
  allowlist can be narrowed with `with_allowlist` (`:713-722`).

A **conformance test suite** (`libs/checkpoint-conformance`) exists to validate
third-party savers against this contract, underscoring that the interface is the
product's real extension point.

## Write and append path (ordering, durability, concurrency, delivery)

- **`put` = upsert one immutable snapshot.** SQLite: `INSERT OR REPLACE INTO
  checkpoints (…) VALUES (…)` keyed by `(thread_id, checkpoint_ns,
  checkpoint_id)`, with `parent_checkpoint_id` taken from the incoming config's
  `checkpoint_id` (`sqlite/__init__.py:424-436`). Postgres: `INSERT … ON CONFLICT
  … DO UPDATE SET checkpoint = EXCLUDED.checkpoint, metadata = EXCLUDED.metadata`
  (`postgres/base.py:137-144`). Because each checkpoint has a fresh UUID6, the
  normal path *appends a new row*; the upsert semantics matter only for
  re-writing the same id.
- **Channel values written content-addressed + deduplicated.** Postgres upserts
  each channel value into `checkpoint_blobs` keyed by `(thread_id, ns, channel,
  version)` with `ON CONFLICT DO NOTHING` (`postgres/base.py:131-135`), so a value
  is stored once per version and shared across every checkpoint that references
  that version. This is real cross-checkpoint dedup.
- **Ordering** is by `checkpoint_id` (monotonic UUID6): `get_tuple` for the
  latest does `ORDER BY checkpoint_id DESC LIMIT 1` (`sqlite/__init__.py:240`);
  `list` does `ORDER BY checkpoint_id DESC` (`:338`). The parent chain
  (`parent_checkpoint_id`) gives the logical order independent of wall-clock.
- **Durability / atomicity**: delegated to the backend. SQLite runs in WAL mode
  (`PRAGMA journal_mode=WAL`, `sqlite/__init__.py:141`) with a process lock around
  the cursor and a transaction per cursor context (`:168-182`). Postgres uses
  transactions/pipelines. The in-memory saver has no durability
  (`memory/__init__.py:38`).
- **Concurrency model**: the OSS savers do **not** implement optimistic
  concurrency or an expected-version precondition on `put` — `put` simply writes
  the checkpoint the Pregel loop computed. Safety against concurrent runs of the
  *same thread* is expected to be enforced above the saver (the LangGraph Platform
  serializes runs per thread); the store contract itself is last-write-wins on a
  given `checkpoint_id`.
- **`put_writes` delivery = idempotent, with a special-channel override.** For
  ordinary channels writes are `INSERT OR IGNORE` / `ON CONFLICT DO NOTHING`
  keyed by `(thread_id, ns, checkpoint_id, task_id, idx)` — **at-least-once with
  dedup by write position** (`sqlite/__init__.py:462-482`,
  `postgres/base.py:155-159`). For "special" channels in `WRITES_IDX_MAP` (e.g.
  the `RESUME` channel) it uses `INSERT OR REPLACE` / `DO UPDATE` so a re-sent
  resume value overwrites (`sqlite/__init__.py:462-466`,
  `postgres/base.py:146-153`). So writes are idempotent by construction, which is
  what lets a crashed superstep re-run safely.

## Read and resume path

- **Resume = fetch the latest checkpoint tuple for the thread.** With no
  `checkpoint_id` in config, `get_tuple` selects the newest checkpoint for
  `(thread_id, checkpoint_ns)` and attaches its `pending_writes`
  (`sqlite/__init__.py:239-266`). The Pregel loop resumes from that snapshot,
  applying pending writes so an interrupted step continues rather than restarts.
- **Time travel = fetch a specific checkpoint id.** With a `checkpoint_id` in
  config, `get_tuple` fetches that exact snapshot; `list(before=…, limit=…)`
  pages the chain (`base/__init__.py:253-275`). This is the "time-travel
  debugging" the docs reference (`:192`).
- **Reads reconstruct full state from the store.** Postgres reads join
  `checkpoint_blobs` on the checkpoint's `channel_versions` to rebuild
  `channel_values`, and aggregate `checkpoint_writes` into `pending_writes`, in
  one SELECT (`postgres/base.py:93-118`). SQLite stores the whole serialized
  checkpoint blob inline and loads writes separately (`sqlite/__init__.py:240`,
  `263`).
- **`DeltaChannel` reconstruction** walks the parent chain: for a delta channel,
  `channel_values` holds only a sentinel except at snapshot points, so
  `get_delta_channel_history` accumulates on-path `pending_writes` from ancestors
  until it hits the nearest ancestor whose `channel_values[ch]` is populated (a
  `_DeltaSnapshot`), returned as the `seed` (`base/__init__.py:582-649`). A
  snapshot fires every `snapshot_frequency` updates or after a system-wide
  superstep bound (default 5000) (`:78-82`). This is an explicit
  log-fold-with-periodic-snapshot mechanism.
- **Bounds / pagination**: `list` takes an explicit `limit` and a `before`
  cursor (`base/__init__.py:253-275`). There is no built-in transcript-size cap;
  history growth is bounded by `prune`/shallow savers, not by read limits.

## Listing, summaries, and search

- **Listing checkpoints** within a thread is a SQL scan ordered by
  `checkpoint_id DESC`, filterable by metadata (`sqlite/__init__.py:295-360`;
  `list` contract `base/__init__.py:253-275`). Metadata filtering matches against
  the JSON(B) `metadata` column (`CheckpointMetadata`: `source`, `step`,
  `parents`, `run_id`, …, `:38-86`).
- **No separate summary sidecar per thread** in the OSS store. The `metadata`
  column *is* the queryable read model (source/step/parents/run_id), maintained at
  write time by `put` (`get_checkpoint_metadata(config, metadata)`,
  `sqlite/__init__.py:421-423`). Per-thread indexes exist on `thread_id`
  (`checkpoints_thread_id_idx`, etc., `postgres/base.py:82-89`).
- **No FTS/vector search over checkpoints.** Semantic search is a *different*
  subsystem — the `BaseStore` / long-term memory store
  (`libs/checkpoint/langgraph/store/base/**`, with an embeddings/`embed` module)
  — which is orthogonal to the checkpointer and stores namespaced key-value
  "memories," not session transcripts. It is out of scope for session
  persistence.

## Entry/message structure and versioning

- **Checkpoint envelope**: the `Checkpoint` TypedDict itself (`v`, `id`, `ts`,
  `channel_values`, `channel_versions`, `versions_seen`, `updated_channels`)
  (`base/__init__.py:92-123`), stored serialized. `v` is the **format version,
  currently 1** (`:95-96`) — an explicit schema-version field on every snapshot.
- **Write envelope**: `checkpoint_writes` rows are `(thread_id, ns,
  checkpoint_id, task_id, task_path, idx, channel, type, blob)`
  (`postgres/base.py:66-76`, `90`). The store relies on `(task_id, idx)` for write
  identity/dedup.
- **Store interpretation**: values are **opaque to the store** — serialized via
  the pluggable `SerializerProtocol`. The default `JsonPlusSerializer` uses
  `ormsgpack` with a JSON fallback and an ext-hook allowlist for safe type
  reconstruction (`serde/jsonplus.py:30`, `83-125`); `dumps_typed` returns a
  `(type, bytes)` pair and the `type` column records which codec produced the
  blob. The store persists and returns bytes verbatim; it does not parse channel
  values.
- **Versioning of the format**: three layers. (1) The per-checkpoint `v` field
  (`:95-96`). (2) **Numbered backend migrations** — Postgres keeps an ordered
  `MIGRATIONS` list where "the position of the migration in the list is the
  version number," tracked in a `checkpoint_migrations(v)` table
  (`postgres/base.py:40-91`), including additive `ALTER TABLE … ADD COLUMN IF NOT
  EXISTS` (e.g. `task_path`, `:90`). This is a forward-only ratchet. (3) **Serde
  compatibility** — pre-msgpack checkpoints remain loadable, and the msgpack
  allowlist is versioned by `SAFE_MSGPACK_TYPES` (`serde/jsonplus.py:64-70`).

## Compaction and history management

- **Compaction/summarization of message history is an application concern, not a
  store concern.** LangGraph's store keeps whatever channel values the graph
  computes; if an app summarizes chat history, that summary is just the new value
  of a channel in the next checkpoint. The store neither triggers nor
  understands it.
- **History growth is managed by `prune` or shallow savers.** `prune(thread_ids,
  strategy="keep_latest" | "delete")` retains only the most recent checkpoint per
  namespace or removes all (`base/__init__.py:374-415`). Shallow savers
  structurally keep only the latest checkpoint (`postgres/shallow.py:169-174`).
- **`DeltaChannel` is the store-level compaction analogue**: instead of storing a
  full channel value per checkpoint, it stores periodic `_DeltaSnapshot` blobs
  plus on-path write deltas, and reconstructs by folding forward from the nearest
  snapshot (`base/__init__.py:63-86`, `582-649`). Crossing a "compaction boundary"
  (a snapshot point) on read just means the fold seeds from that snapshot rather
  than the chain root. The durable rows are not rewritten in place; snapshots are
  new blobs.

## Rewind, checkpoints, and fork

- **Every step is a checkpoint; rewind is native.** Because each superstep writes
  a parent-linked checkpoint, "rewind" is simply resuming from an earlier
  `checkpoint_id` (time travel, `base/__init__.py:192`). Nothing is destroyed — you
  select an older snapshot in the chain.
- **Fork is a first-class metadata concept.** `CheckpointMetadata.source` includes
  `"fork"` — "The checkpoint was created as a copy of another checkpoint"
  (`:41-48`), and `parents` maps namespace → parent checkpoint id (`:56-60`). A
  fork produces a new checkpoint whose `parent_checkpoint_id`/`parents` point at
  the branch point, sharing the (immutable, content-addressed) ancestor blobs.
  `copy_thread(source, target)` copies an entire thread's checkpoints + writes to
  a new thread id, and its docstring **requires copying the complete parent
  chain** so `DeltaChannel` state remains reconstructable (`:350-372`).
- **File-state/environment checkpoints**: not applicable — LangGraph checkpoints
  application *graph state* (channel values), not workspace files. There is no
  git-snapshot or filesystem checkpoint concept.

## Subagents and nested sessions

- **Nested/subgraph executions are namespaced within the same thread.**
  `checkpoint_ns` (default `''`) is part of the key precisely so a subgraph's
  checkpoints live under the parent thread but in a distinct namespace
  (`sqlite/__init__.py:144`, `150`; `postgres/base.py:49`, `55`).
  `CheckpointMetadata.parents` is explicitly a **map from checkpoint namespace to
  parent checkpoint id** (`base/__init__.py:56-60`), linking a child namespace's
  checkpoint to its parent.
- **Durable parent-child link** is therefore `(thread_id, checkpoint_ns)` +
  `parents[ns]`. The child shares the thread and its content-addressed blobs; it
  is isolated by namespace rather than by a separate thread/file.
- **Cascade**: `delete_thread(thread_id)` deletes *all* checkpoints and writes for
  the thread across namespaces — SQLite `DELETE FROM checkpoints WHERE thread_id =
  ?` (`sqlite/__init__.py:484-496`) — so nested namespaces cascade with the
  parent thread. (A fully separate subagent running under its *own* `thread_id`
  would be an independent thread with no automatic cascade.)

## Retention, deletion, and multi-host

- **Retention**: caller-owned. The store provides the *mechanisms* —
  `prune(strategy=…)`, shallow savers, `delete_for_runs(run_ids)` — but enforces
  no TTL or automatic lifecycle (`base/__init__.py:331-415`). `delete_for_runs`
  targets checkpoints by `run_id` and carries the `DeltaChannel` warning that
  deleting ancestor rows a live thread depends on will corrupt reconstruction
  (`:331-348`).
- **Deletion**: `delete_thread(thread_id)` removes all of a thread's rows across
  the `checkpoints`/`checkpoint_blobs`/`checkpoint_writes` tables (backend
  implementations delete from each) (`sqlite/__init__.py:484-496`). It is a hard
  delete, not a tombstone; there is no remote-first/local-cache split at this
  layer.
- **Multi-host**: **first-class via the backend.** Because the store is a shared
  database (Postgres in production, or SQLite for single-node), multiple hosts can
  read/write the same thread through the same Postgres instance. WAL mode + per-DB
  transactions provide the concurrency substrate (`sqlite/__init__.py:141`),
  content-addressed blob upserts are conflict-free
  (`postgres/base.py:131-135`), and idempotent `put_writes` tolerates retries
  (`:155-159`). The store does not itself implement cross-host leasing or
  single-writer arbitration — that is layered above (the Platform run queue) — but
  a shared Postgres checkpointer is explicitly the intended multi-host deployment,
  unlike the local-filesystem CLIs in this corpus.

## Interop with foreign session stores

Not applicable. LangGraph reads only its own checkpoint schema (across its own
saver backends and migration versions). There is no discovery/import/resume of a
foreign product's session store. (It can *migrate its own* pre-msgpack
checkpoints via serde compatibility, `serde/jsonplus.py:64-70`, and its own
schema via numbered migrations, `postgres/base.py:40-91`.)

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* LangGraph's durable session is **a
per-thread, parent-linked chain of immutable, id-addressed state snapshots
(`Checkpoint`) plus per-checkpoint pending writes, over a pluggable saver
interface, with channel values stored content-addressed by `(channel, version)`
and deduplicated across snapshots.** This is the most event-sourcing-adjacent
model in the study and the one whose *interface* most resembles what we want.
Transferable ideas:

- **A small, conformance-tested store interface.** `BaseCheckpointSaver`'s
  four required methods (`get_tuple`, `list`, `put`, `put_writes`) plus optional
  lifecycle ops, validated by a conformance suite, is a clean template for our own
  pluggable Session Store contract — separate the read/append core from the
  lifecycle (delete/copy/prune) extras.
- **Content-addressed, version-keyed value blobs with `ON CONFLICT DO NOTHING`**
  (`postgres/base.py:57-65`, `131-135`) is an excellent dedup strategy: store each
  distinct value once per version and let snapshots reference it. We should adopt
  value-level content addressing rather than inlining full state per event.
- **Idempotent writes keyed by `(task_id, idx)`** (`put_writes`,
  `sqlite/__init__.py:462-482`) give exactly the at-least-once-with-dedup delivery
  our design wants, and the special-channel `DO UPDATE` override
  (`postgres/base.py:146-153`) shows a principled exception (resume signals) to the
  otherwise append-only rule.
- **Monotonic, time-ordered ids (UUID6) as the sort key** (`base/id.py`,
  `base/__init__.py:99-101`) plus a **separate monotonic per-channel version**
  (`get_next_version`, `:692-711`) cleanly separate "ordering of snapshots" from
  "ordering of a value's revisions" — worth mirroring.
- **`DeltaChannel` = periodic-snapshot + delta-fold** (`:63-86`, `582-649`) is a
  concrete pattern for bounding log-replay cost: snapshot every N updates, store
  deltas between, fold forward from the nearest snapshot. This is exactly the
  snapshot/retention story the file-based products in this corpus lack, and we
  should design an equivalent from the start.
- **Namespaces (`checkpoint_ns`) for nested/subgraph state** under one thread key,
  with `parents[ns]` links and cascade on `delete_thread`, is a tidy alternative
  to separate child sessions when the child is truly part of the same run.
- **Cautions**: (1) **No expected-version/OCC in the OSS savers** — `put` is
  last-write-wins on an id, and single-writer-per-thread is assumed to be enforced
  above the store; our multi-host design should add an explicit expected-position
  precondition rather than rely on an upstream queue. (2) **Correctness coupling
  between prune/copy/delete and the delta chain** — the repeated `DeltaChannel`
  warnings (`:340-415`, `540-580`) show how snapshot-based history makes deletion
  dangerous: any retention/GC we build must be snapshot-chain-aware or it will
  silently corrupt reconstructed state. (3) **It is state-snapshot-oriented, not
  transcript-oriented** — a "message history" is just one channel's value, so if
  our store must also serve a first-class, queryable message transcript, that is
  an additional projection LangGraph does not provide at the store layer.

## Open questions

- **Thread enumeration / cross-thread listing**: the OSS saver contract has no
  "list all threads" method; how the LangGraph Platform server enumerates and
  scopes threads (per user/assistant) sits above this store and was not examined.
- **`v` format-version migrations**: the checkpoint `v` is documented as
  "currently 1"; how a future `v=2` would be migrated on read (vs. the SQL
  `MIGRATIONS` ratchet) was not traced.
- **Exact concurrency guarantees under two hosts writing the same thread**: the
  store relies on backend transactions and idempotent writes, but the precise
  outcome of two simultaneous `put`s minting sibling checkpoints (branch vs.
  clobber) was not exercised.
- **`checkpoint_blobs` GC**: content-addressed blobs are shared across
  checkpoints; whether/when orphaned `(channel, version)` blobs are reclaimed after
  `prune`/`delete_for_runs` was not confirmed.
- **Shallow-saver semantics vs. pending writes**: how the shallow savers handle
  pending writes and interrupts when only the latest checkpoint is retained was
  not fully traced.

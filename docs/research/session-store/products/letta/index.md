# Letta: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against a local clone of [letta-ai/letta](https://github.com/letta-ai/letta)
(formerly MemGPT) pinned at commit `ff19ffeafeb54bd2a7dc5d4a552f10191732a235`.
Letta is licensed Apache-2.0 (`LICENSE:1-5`). Authoritative anchors:
`letta/orm/agent.py` (the persistent "session" entity), `letta/orm/message.py`
and `letta/services/message_manager.py` (the durable message log),
`letta/orm/conversation.py`, `letta/orm/conversation_messages.py`, and
`letta/services/conversation_manager.py` (the newer relational
in-context-window model, in migration alongside the legacy JSON pointer),
`letta/orm/archive.py`, `letta/orm/passage.py`, and
`letta/services/archive_manager.py` (the archival/recall tier),
`letta/helpers/tpuf_client.py` (the external vector-search index),
`letta/services/memory_repo/` and `letta/services/block_manager_git.py` (an
opt-in, git-backed versioning system for core-memory blocks), and
`letta/orm/sqlalchemy_base.py` / `letta/server/db.py` (the generic ORM
CRUD/transaction layer). Letta runs as a server (FastAPI + SQLAlchemy over
Postgres or SQLite), not a local CLI, so "session" in this dossier means the
durable conversational state Letta's REST API exposes, not a terminal
session.

## The storage model

Letta does not have a first-class "Session" object. The durable, long-lived
entity is the **Agent** row itself (`letta/orm/agent.py:1-524`, table
`agents`) -- an agent is created once and persists indefinitely; there is no
separate session record that expires or gets swapped out. What most other
products call "resuming a session" is, in Letta, simply continuing to send
messages to the same `agent_id`.

Two durable SQL tables carry the conversational history:

- `messages` (`letta/orm/message.py:1-266`) -- one row per message, with a
  server-assigned monotonic `sequence_id` (`letta/orm/message.py`, `BigInteger`,
  unique) that is the authoritative ordering key, distinct from the row's
  string `id` and from `created_at`. Message rows are overwhelmingly
  append-only in practice, but they are not strictly immutable: `MessageManager`
  exposes `update_message_by_id_async` (`letta/services/message_manager.py:705`)
  and `_update_message_by_id_impl` (`letta/services/message_manager.py:794`),
  and `delete_message_by_id_async` (`letta/services/message_manager.py:822`)
  performs a genuine hard delete.
- `conversations` / `conversation_messages` (`letta/orm/conversation.py:1-77`,
  `letta/orm/conversation_messages.py:1-74`) -- a newer, explicitly
  in-progress relational model. The `ConversationMessage` docstring states it
  "replaces the `message_ids` JSON list on agents with proper relational
  modeling" of which messages are in-context for which conversation
  (`letta/orm/conversation_messages.py:16-21`).

Sitting on top of the message log is a piece of state that **cannot** be
reconstructed by replaying the log: the pointer that says which subset of
historical messages is currently "in context" for the LLM. In the legacy
model this is `Agent.message_ids`, a mutable JSON array column directly on
the agent row:

```python
# letta/orm/agent.py:71
message_ids: Mapped[Optional[List[str]]] = mapped_column(JSON, ...)
```

with an adjacent code comment calling this out as a known anti-pattern:

```python
# letta/orm/agent.py:69-70
# TODO: This should be a separate mapping table
# This is dangerously flexible with the JSON type
```

`AgentManager.reset_messages_async` makes the append-vs-pointer distinction
explicit in its own docstring: "Note: This only clears messages from the
agent's context, it does not delete them from the database"
(`letta/services/agent_manager.py:1671-1743`, quoted docstring at
`agent_manager.py:1686`). The implementation confirms it: it truncates
`agent.message_ids` down to `[system_message_id]`
(`letta/services/agent_manager.py:1711-1713`) -- the underlying `messages`
rows are untouched. The newer model replaces this single JSON array with a
per-row `ConversationMessage.in_context` boolean
(`letta/orm/conversation_messages.py`), which is the same kind of
non-replayable state, just modeled relationally instead of as a JSON blob.
`AgentManager` still exposes low-level pointer mutators --
`set_in_context_messages`/`_async` (`letta/services/agent_manager.py:1616,1622`),
`trim_older_in_context_messages` (`agent_manager.py:1627`),
`trim_all_in_context_messages_except_system` (`agent_manager.py:1634`),
`prepend_to_in_context_messages` / `append_to_in_context_messages` /
`_async` (`agent_manager.py:1642,1650,1658`) -- all of which mutate the
pointer, never the `messages` table.

A third tier, **archival memory**, is durable, permanent, and explicitly
separate from both of the above (see "Compaction" and "Rewind" below for
detail): `Archive` / `ArchivalPassage` rows (`letta/orm/archive.py:1-99`,
`letta/orm/passage.py:1-105`) created only by an explicit agent tool call,
never touched by context-window eviction.

Which parts are authoritative vs. derived:

- Authoritative / source of truth: `agents` row (including `message_ids`),
  `messages` rows, `conversations`/`conversation_messages` rows,
  `archival_passages` rows -- all in the primary SQL database (Postgres or
  SQLite, see "Keying and identity").
- Derived / rebuildable projections: the Turbopuffer vector index for
  messages and passages (`letta/helpers/tpuf_client.py`, see "Listing,
  summaries, and search" -- explicitly best-effort and can silently drift);
  the agent's compiled system prompt (rebuilt from `Block` core-memory rows
  on demand via `rebuild_system_prompt_async`, referenced at
  `letta/services/agent_manager.py:1724`); and, when git-backed memory is
  enabled for an agent, the `Block.value` column itself becomes a read cache
  of the git repository, not the source of truth (see "Rewind, checkpoints,
  and fork" -- `sync_blocks_from_git` docstring: "rebuild the PostgreSQL cache
  from git source of truth", `letta/services/block_manager_git.py:571`).

Best-fit conceptual model: **session-as-row-set with a mutable pointer**,
not a pure append-only log. The `messages` table is a durable, mostly-append
row set; the `agents.message_ids` array (legacy) or
`conversation_messages.in_context` flags (new) are mutable state layered on
top that determines what is "in the session" at any moment and is not
recoverable by replaying `messages` alone.

## Keying and identity

Every durable row is scoped to an **organization** via `OrganizationMixin`
(`letta/orm/mixins.py:1-99`), which is Letta's tenancy boundary -- not a
per-project/per-cwd scheme the way local-CLI tools key sessions. Within an
organization, the addressing hierarchy is:

- `Agent.id` -- the top-level identity a client interacts with (all message
  send/read operations take an `agent_id`).
- `Conversation.id` -- an optional, secondary scope for concurrent
  conversations *within* one agent (`letta/orm/conversation.py`, docstring
  "Conversations that can be created on an agent for concurrent messaging").
  `Conversation.agent_id` is a required FK with `ondelete="CASCADE"`
  (`letta/orm/conversation.py`).
- `Run.id` -- an execution-attempt record created per processing turn,
  optionally linked to a conversation via a nullable
  `conversation_id` FK with `ondelete="SET NULL"` (`letta/orm/run.py:22-57`).
  The `Run` docstring: "Runs are created when agents process messages and
  represent a conversation or processing session. Unlike Jobs, Runs are
  specifically tied to agent interactions and message processing"
  (`letta/orm/run.py:23-25`).
- `Message.id` / `sequence_id` -- the leaf entries.

IDs are server-minted, human-readable, prefixed UUIDs, not UUIDv7 and not
purely random: e.g. `Run.id` defaults to `f"run-{uuid.uuid4()}"`
(`letta/orm/run.py:37`), `Step.id` to `f"step-{uuid.uuid4()}"`
(`letta/orm/step.py:27`), `Group.id` to `f"group-{uuid.uuid4()}"`
(`letta/orm/group.py:21`), `BlockHistory.id` to
`f"block_hist-{uuid.uuid4()}"` (`letta/orm/block_history.py:25`). None of
these prefixes encode ordering; ordering is carried by separate columns
(`created_at`, and for messages the dedicated `sequence_id`).

Listing is organization-scoped, not global: `ConversationManager.list_conversations`
filters `ConversationModel.organization_id == actor.organization_id`
unconditionally and additionally by `agent_id` when provided
(`letta/services/conversation_manager.py:390-397`). There is no
cross-project/cross-org enumeration path in the code read.

There is no "relocation" or working-directory concept to reconcile -- Letta
is a server storing rows keyed by opaque IDs, not a local tool keyed by
filesystem path, so this part of research question 2 does not apply
(noted rather than answered, per the Method section).

`otid` ("offline threading ID", `letta/schemas/message.py`) is a
client-supplied-or-server-generated field documented as the idempotency/dedup
mechanism for retried requests, distinct from the row's own `id`/`sequence_id`.

## The store interface

Letta has no pluggable storage adapter/protocol for sessions -- the "store"
is the SQLAlchemy ORM layer plus a set of service-layer managers
(`MessageManager`, `ConversationManager`, `ArchiveManager`, `PassageManager`,
`AgentManager`) that are the de facto internal interface. Reconstructed
operation contract, with call sites:

**Generic CRUD** (`letta/orm/sqlalchemy_base.py:555-800`, base class for all
ORM models):
- `create_async` / `batch_create_async` -- insert, wrapped in the deadlock-
  retry decorator (see "Write and append path").
- `delete_async` -- **soft** delete (sets `is_deleted=True`); the default
  delete path for models that support it.
- `hard_delete_async` / `bulk_hard_delete_async` -- genuine SQL `DELETE`.
- `update_async` (`letta/orm/sqlalchemy_base.py:747-791`) -- full-row update;
  translates a SQLAlchemy `StaleDataError` into a domain
  `ConcurrentUpdateError` for models using optimistic-locking (see below).

**Messages** (`letta/services/message_manager.py`):
- `create_many_messages_async` (`message_manager.py:477-603`) -- the core
  write path; batch-inserts message rows and, if a `run_id` is supplied,
  guards against a race by checking the run exists before insert
  (`message_manager.py:548-569`).
- `update_message_by_id_async` / `_update_message_by_id_impl`
  (`message_manager.py:705`, `794`) -- in-place row mutation.
- `delete_message_by_id_async` (`message_manager.py:822`) and
  `delete_messages_by_ids_async` (`message_manager.py:1094`) -- hard delete.
- `list_messages` (`message_manager.py:895-1044`) -- the primary read path,
  cursor-paginated on `sequence_id` (`message_manager.py:1001-1024`), with
  three-way `conversation_id` filter semantics: unset means "no filter",
  `None` explicitly means "legacy/no-conversation messages", and a concrete
  id filters to that conversation (`message_manager.py:957-972`).
- `delete_all_messages_for_agent_async` (`message_manager.py:1045-1093`) --
  bulk delete for one agent.
- `search_messages_async` (`message_manager.py:1142-1261`) -- routes to
  Turbopuffer if configured, else falls back to a SQL query.
- `search_messages_org_async` (`message_manager.py:1262+`) -- org-wide search,
  Turbopuffer-only, **no SQL fallback**.

**Conversations** (`letta/services/conversation_manager.py`):
- `create_conversation` (`conversation_manager.py:50-104`).
- `fork_conversation` (`conversation_manager.py:105-174`) -- see "Rewind,
  checkpoints, and fork."
- `fork_default_conversation` (`conversation_manager.py:175-221`) -- bridges
  the legacy `agent.message_ids` array into the new relational model.
- `list_conversations` (`conversation_manager.py:338-518`) -- cursor-paginated
  listing, sortable by `created_at`, `last_message_at`, or
  `last_run_completion` (a joined aggregate over `runs`).
- `update_conversation` (`conversation_manager.py:519-555`),
  `delete_conversation` (`conversation_manager.py:556-609`) -- see "Retention,
  deletion, and multi-host."
- `add_messages_to_conversation` / `_add_messages_to_conversation_with_session`
  (`conversation_manager.py:719`, `683`), `update_in_context_messages`
  (`conversation_manager.py:752`), `list_conversation_messages`
  (`conversation_manager.py:801`).

**Archival memory** (`letta/services/archive_manager.py`,
`letta/services/passage_manager.py`):
- `create_archive_async` (`archive_manager.py:30-58`) -- binds a
  `vector_db_provider` (`NATIVE` or `TPUF`) to the archive at creation time.
- `get_or_create_default_archive_for_agent_async` (`archive_manager.py:502-572`)
  -- self-healing against a race via `IntegrityError` catch.
- `create_passage_in_archive_async` (`archive_manager.py:284-376`) -- the
  archival write path (embed, SQL-insert, best-effort TPUF dual-write).
- `create_passages_in_archive_async` (`archive_manager.py:377-463`) -- batch
  version.
- `delete_passage_from_archive_async` (`archive_manager.py:464-501`),
  `delete_archive_async` (`archive_manager.py:266-279`, hard delete).
- `get_or_set_vector_db_namespace_async` (`archive_manager.py:691-717`) --
  lazy Turbopuffer namespace creation, cached on the `Archive` row.
- `PassageManager.insert_passage` (`passage_manager.py:543`),
  `create_agent_passage_async` / `_passages_async`
  (`passage_manager.py:134`, `199`), `agent_passage_size_async`
  (`passage_manager.py:955`).

**Agent-level context pointer** (`letta/services/agent_manager.py`):
- `get_in_context_messages` (`agent_manager.py:1413`),
  `set_in_context_messages`/`_async` (`agent_manager.py:1616,1622`),
  `trim_older_in_context_messages` (`agent_manager.py:1627`),
  `append_to_in_context_messages`/`_async` (`agent_manager.py:1650,1658`),
  `reset_messages_async` (`agent_manager.py:1671-1743`),
  `delete_agent_async` (`agent_manager.py:1320-1397`).

## Write and append path (ordering, durability, concurrency, delivery)

**Ordering.** `Message.sequence_id` is the authoritative order/pagination
cursor (`letta/orm/message.py`, `BigInteger`, unique). On Postgres it is
generated server-side from a real sequence (`message_seq_id`, created and
backfilled in migration `alembic/versions/e991d2e3b428_add_monotonically_increasing_ids_to_.py`,
lines 1-40: `CREATE SEQUENCE message_seq_id START 1;` then a backfill query
ordered by `["created_at", "id"]`). SQLite has no equivalent server-side
sequence under concurrent insert, so Letta hand-rolls one: a singleton
`message_sequence` table incremented via two SQLAlchemy event listeners,
`set_sequence_id_for_sqlite_bulk` (`letta/orm/message.py:130`) and
`set_sequence_id_for_sqlite` (`letta/orm/message.py:204`), each doing an
atomic `UPDATE ... RETURNING` against that singleton row.

**Durability/atomicity.** All writes go through
`db_registry.async_session()` (`letta/server/db.py:70-116`), an
`asynccontextmanager` that commits on success, rolls back on any exception
(including `asyncio.CancelledError`, handled separately because it is a
`BaseException` and would otherwise skip rollback and leak an
"idle in transaction" connection -- comment at `letta/server/db.py:77-80`),
and retries transient `ConnectionError`s up to 3 times with exponential
backoff (`letta/server/db.py:84-116`) before raising a domain
`LettaServiceUnavailableError`. There is no temp-file-and-rename pattern --
durability is entirely the RDBMS transaction.

**Concurrency.** Two distinct mechanisms:
- Nearly every write in `letta/orm/sqlalchemy_base.py` is wrapped in a
  deadlock-retry-with-exponential-backoff decorator (constants referenced in
  that module) that catches DB-reported deadlocks and retries the operation.
- Optimistic-concurrency version checking (SQLAlchemy's `version_id_col`) is
  used on exactly one model in the whole ORM layer -- `Block`
  (`letta/orm/block.py:61`: `__mapper_args__: ClassVar[dict] =
  {"version_id_col": version}`) -- confirmed by grepping every file under
  `letta/orm/` for `version_id_col`, which returns only this one hit. When a
  stale `Block` write loses the race, `sqlalchemy_base.update_async` catches
  SQLAlchemy's `StaleDataError` and re-raises it as a domain
  `ConcurrentUpdateError(resource_type=class_name, resource_id=object_id)`
  (`letta/orm/sqlalchemy_base.py:747-791`). No other model in the tree --
  not `Agent`, not `Message`, not `Conversation`, not `Archive` -- has this
  guard. In particular, `Agent.message_ids` updates
  (`agent.update_async(...)`, e.g. `letta/services/agent_manager.py:1713`)
  are plain last-write-wins full-column overwrites with no version check:
  two concurrent turns racing to update the same agent's context pointer can
  silently clobber each other's context-window state (the underlying
  `messages` rows are never lost, only the pointer's view of "what's in
  context").
- Message writes carry an explicit precondition check rather than a version
  column: `create_many_messages_async` verifies the referenced `run_id`
  exists before inserting, specifically to close a race window
  (`letta/services/message_manager.py:548-569`).

**Delivery semantics to the SQL store**: effectively exactly-once per
successful transaction (single RDBMS commit), at-least-once at the
client-retry level, deduplicated via the client-or-server-generated `otid`
field (`letta/schemas/message.py`) -- this is Letta's documented
idempotency/dedup key for retried sends, not `sequence_id` or `id`.

**Delivery to the derived vector index is different and weaker**: message
embedding to Turbopuffer is backgrounded and fire-and-forget by default
(`_embed_messages_background`, `letta/services/message_manager.py:605-661`),
gated behind `strict_mode` if the caller wants to wait for/fail on it;
failures are logged and swallowed otherwise, so the SQL row and the vector
index can drift out of sync with no automatic reconciliation (see "Listing,
summaries, and search").

## Read and resume path

Because Letta is a server with one authoritative Postgres/SQLite database
(there is no local on-disk cache layer in the paths read), "resume" is
simply: the client sends a new message to an existing `agent_id`, and the
server loads `Agent.message_ids` (or, on the new model, the set of
`ConversationMessage` rows with `in_context=True` for the active
conversation) and reads the referenced `Message` rows to reconstruct the
LLM's context window. There is no separate "cached view" distinct from the
SQL rows.

- `list_messages` is the general-purpose paginated read, cursor-based on
  `sequence_id` with `after`/`before` semantics
  (`letta/services/message_manager.py:1001-1024`), not offset pagination, so
  it does not degrade as the table grows.
- Materialized eagerly on resume: the `agent.message_ids` array (or
  `in_context` conversation-message rows) and the referenced message rows --
  this is exactly the in-context window the model will see next turn.
- Lazily loaded: the full message history beyond the in-context window
  (queried on demand via `list_messages`), and archival passages (queried
  only when the agent explicitly calls the archival-search tool -- see
  below).
- No stated bound on total transcript size in the code paths read; the
  in-context window is bounded (see "Compaction"), but the durable
  `messages` table itself has no code-enforced cap.

## Listing, summaries, and search

**Listing.** `ConversationManager.list_conversations`
(`letta/services/conversation_manager.py:338-518`) is an indexed SQL query,
not a directory scan: `Conversation` has `ix_conversations_agent_id` and
`ix_conversations_org_agent` indexes (added in migration
`alembic/versions/27de0f58e076_add_conversations_tables_and_run_.py:1-45`).
Cursor pagination supports three sort keys -- `created_at`,
`last_message_at`, and `last_run_completion` (the last computed via an
outer join and `MAX(runs.completed_at)` grouped by conversation,
`conversation_manager.py:413-420`). No stated cost numbers were found in
the source (nothing to quote).

**Metadata sidecar.** `Conversation.summary` is a plain nullable `String`
column on the `Conversation` row itself (`letta/orm/conversation.py`), not a
separately maintained denormalized read-model table -- it is set at write
time by `compile_and_save_system_message_for_conversation`
(`letta/services/conversation_manager.py:222-310`, not fully read for its
summarization logic) and searched via a simple `.contains()` filter
(`conversation_manager.py:400-406`). This is the only "summary" artifact
found; there is no separate FTS index over conversation summaries.

**Search.** Two independent search paths, chosen per call:
- SQL fallback -- a direct query against `messages` (used when Turbopuffer
  is unavailable or unconfigured; routed inside
  `search_messages_async`, `letta/services/message_manager.py:1142-1261`).
- Turbopuffer -- an external hosted vector-search service
  (`letta/helpers/tpuf_client.py`, `TurbopufferClient` class,
  `tpuf_client.py:223+`), used for both archival-passage search
  (`query_passages`, `tpuf_client.py:909`) and message search
  (`query_messages_by_agent_id`/`_by_org_id`, `tpuf_client.py:1056,1233`),
  combining vector similarity and full-text search via reciprocal-rank
  fusion (`_reciprocal_rank_fusion`, `tpuf_client.py:1489`).

**Whether Turbopuffer is used at all is a server-wide, settings-gated
decision**, re-evaluated at each relevant call site:

```python
# letta/helpers/tpuf_client.py:208-220
def should_use_tpuf() -> bool:
    # We need OpenAI since we default to their embedding model
    return bool(settings.use_tpuf) and bool(settings.tpuf_api_key) and bool(model_settings.openai_api_key)

def should_use_tpuf_for_messages() -> bool:
    return should_use_tpuf() and bool(settings.embed_all_messages)
```

**How the index is bootstrapped and kept consistent** (the area the task
called out for extra depth): there is **no backfill/reindex code path**.
Every archival passage and every embedded message is indexed exactly once,
at write time:
- Archival passages: `create_passage_in_archive_async` synchronously
  generates the embedding, writes the SQL row first (authoritative), then
  best-effort dual-writes to Turbopuffer only if
  `Archive.vector_db_provider == TPUF`
  (`letta/services/archive_manager.py:349-368`); `vector_db_provider` is
  decided once, at archive-creation time, from the server-wide
  `should_use_tpuf()` gate (`archive_manager.py:30-58`) and then cached
  permanently on the `Archive` row.
- Messages: embedding is asynchronous and fire-and-forget by default
  (`_embed_messages_background`, `letta/services/message_manager.py:605-661`).

I grepped `archive_manager.py`, `message_manager.py`, and `tpuf_client.py`
for `backfill`/`reindex`/`re-index` and found no job or migration that
retroactively embeds pre-existing rows into Turbopuffer. The one related hit
is a still-open TODO acknowledging a gap in the opposite direction (a schema
field, not historical data): `"# TODO: Once existing TPUF namespaces are
backfilled with is_deleted attribute,"` (`letta/helpers/tpuf_client.py:1053`).
Practical implication (inference): if Turbopuffer is enabled for the first
time on a server that already has agents with history, or if a single
`Archive`'s `vector_db_provider` was set to `NATIVE` before a later
org-wide policy change to `TPUF`, vector search will never surface the
pre-existing rows -- only the SQL fallback (`search_messages_async`) or,
for org-wide message search, nothing at all, since
`search_messages_org_async` is Turbopuffer-only with no fallback
(`letta/services/message_manager.py:1262+`). Consistency is best-effort and
eventual, never transactionally tied to the SQL write.

## Entry/message structure and versioning

`Message` (`letta/orm/message.py:1-266`, table `messages`) carries: `id`
(string, uuid-prefixed), `sequence_id` (`BigInteger`, unique, monotonic --
the true order key), `created_at`, `role`, `content` (via
`MessageContentColumn`, a custom SQLAlchemy `TypeDecorator` over `JSON`:
`process_bind_param`/`process_result_value` call
`serialize_message_content`/`deserialize_message_content`,
`letta/orm/custom_columns.py:129-139`), `run_id` (FK to `runs`),
`conversation_id` (FK to `conversations`, nullable), `tool_call_id`, and
`otid`. The store parses and interprets this structure -- it is not an
opaque blob returned verbatim; `sequence_id` is what the store relies on for
ordering, and `otid` is what it relies on for dedup/idempotency
(`letta/schemas/message.py`).

Client-facing message *kinds* are enumerated separately in
`letta/schemas/letta_message.py` (`MessageType` enum, 11 variants, e.g.
`SystemMessage`, `UserMessage` subclasses of a common `LettaMessage` base) --
these are API-surface projections, distinct from the ORM row shape.

**Format evolution** is handled through a linear Alembic migration chain
(167 files under `alembic/versions/`, validated in CI per
`.github/workflows/alembic-validation.yml`), each declaring a `down_revision`
pointer to its predecessor. Concrete evidence of schema evolution driven by
real production needs, read directly from migration files:
- `alembic/versions/e991d2e3b428_add_monotonically_increasing_ids_to_.py:1-40`
  -- added `messages.sequence_id`, created a Postgres sequence
  (`message_seq_id`), and backfilled existing rows ordered by
  `["created_at", "id"]`; explicitly skipped for SQLite
  (`if not settings.letta_pg_uri_no_default: return`).
- `alembic/versions/27de0f58e076_add_conversations_tables_and_run_.py:1-45`
  -- added the entire `conversations`/`conversation_messages` schema and a
  `runs.conversation_id` column, i.e. the newer relational in-context model
  described above, dated `2026-01-01` in the migration header.
- `alembic/versions/95badb46fdf9_migrate_messages_to_the_orm.py` (name only,
  not opened in full) and two data-backfill migrations,
  `alembic/versions/9fa274fb0b83_backfill_hidden_for_subagent_role_tag.py`
  and `alembic/versions/8149a781ac1b_backfill_encrypted_columns_for_....py`
  (names only) -- further evidence that Letta treats in-place data backfill
  migrations, not just DDL, as a normal part of its evolution.
- `alembic/versions/068588268b02_add_vector_db_provider_to_archives_table.py`
  and `alembic/versions/f6cd5a1e519d_add_embedding_config_field_to_archives_.py`
  (names only) -- the archival vector-search feature was added onto an
  already-shipped `archives` table, consistent with `vector_db_provider`
  being a late, per-archive add rather than a day-one design choice.
- A dedicated tool-call-ID backfill was also found at the application layer,
  not in a migration: `backfill_missing_tool_call_ids`
  (`letta/services/message_manager.py:30-113`), whose docstring/log message
  explicitly ties it to "historical messages (oct 1-6, 2025 bug)"
  (`message_manager.py:113`) -- i.e., Letta has shipped at least one
  application-level self-healing pass to repair rows written during a
  known bug window, applied lazily on read (called from both
  `list_messages` and another read path, `message_manager.py:391,1040-1041`)
  rather than as a one-time migration.

Whether the migration ratchet is one-way or reversible: each Alembic
revision file conventionally supports `upgrade()`/`downgrade()`, but I found
no CI evidence (in the files read) that `downgrade()` is actually exercised
-- the GitHub Actions validation job name (`alembic-validation.yml`) suggests
forward-migration validation. Treat "reversible in principle, not verified
in practice" as inference, not a confirmed fact.

## Compaction and history management

Model-visible context shrinks via `Summarizer`
(`letta/services/summarizer/summarizer.py:36-895+`), which supports two
modes via a `SummarizationMode` enum: `STATIC_MESSAGE_BUFFER` and
`PARTIAL_EVICT_MESSAGE_BUFFER`, dispatched from `summarize()`
(`summarizer.py:75-123`). The eviction path,
`_partial_evict_buffer_summarization` (`summarizer.py:136-243`), operates on
an in-memory Python list of messages; its output is committed back by
rewriting the agent's context pointer (the same `message_ids`/
`in_context` mechanism described above), not by deleting rows from
`messages`. This matches the `reset_messages_async` docstring pattern
exactly ("does not delete them from the database",
`letta/services/agent_manager.py:1686`): compaction is a context-window
concern layered over the durable log, and the log itself is untouched.
There is no explicit "compaction marker" appended to the log the way an
event-sourced design might record a synthetic boundary event -- the boundary
exists only implicitly, as whatever the pointer/`in_context` state was at
that moment, and is not itself a durable, replayable fact.

Resume/replay across a compaction boundary: a resumed agent simply sees
whatever `message_ids` (or `in_context` rows) currently say, which already
reflects the post-compaction state -- there is no notion of "replay from
before the compaction" through the API paths read.

## Rewind, checkpoints, and fork

Two independent mechanisms answer this section, at two different tiers of
state:

**Conversation forking** (message-log tier). `ConversationManager.fork_conversation`
(`letta/services/conversation_manager.py:105-174`) is a genuine
shared-prefix fork, not a copy: it creates a new `Conversation` row and a
new system message, then links the *same* underlying `Message` rows into
the new conversation via new `ConversationMessage` junction rows
(`message_ids_to_copy = source_message_ids[1:]`, linked, not duplicated).
`fork_default_conversation` (`conversation_manager.py:175-221`) is the
migration bridge that promotes an agent's legacy `message_ids` array into
this model the first time it is needed. Because messages can be
multiply-referenced this way, deletion is reference-aware:
`delete_conversation` (`conversation_manager.py:556-609`) soft-deletes the
`conversation_messages` junction rows for that conversation
(`conversation_manager.py:573-580`), then soft-deletes only the `Message`
rows that are **not** still referenced by any other non-deleted
conversation -- enforced with an explicit `NOT IN` subquery against
`conversation_messages` (`conversation_manager.py:582-597`, comment: "With
conversation forking, messages can be referenced by multiple conversations
via the conversation_messages junction table").

**Git-based memory-block checkpoints** (mutable-state tier, opt-in). This is
a second, independent versioning system layered over core-memory `Block`
rows, gated per-agent behind a tag (`GIT_MEMORY_ENABLED_TAG`,
`letta/services/block_manager_git.py:359-482`,
`GitEnabledBlockManager` class at `block_manager_git.py:30`). When enabled:
- Each memory block is rendered to a `{label}.md` file in a per-agent git
  repository (`letta/services/memory_repo/git_operations.py`,
  `GitOperations` class at `git_operations.py:49`), backed by an external
  `StorageBackend` (blob storage, not opened in full).
- Every mutation is committed via `GitOperations.commit`
  (`git_operations.py:351-414`), which acquires a **Redis distributed lock**
  scoped to the agent (`acquire_memory_repo_lock`,
  `git_operations.py:384-386`) before running `git reset --hard`, applying
  file changes, and `git commit` (`_commit_with_lock`,
  `git_operations.py:415-517`) -- this is the concurrency-control mechanism
  for this tier (a mutex per agent, not optimistic versioning).
- `BlockManagerGit.get_block_at_commit` (`block_manager_git.py:510-529`)
  reads a block's value **at a specific historical commit SHA** -- a true
  point-in-time rewind read, not destructive to later history.
- `get_block_history` (`block_manager_git.py:533-560`) lists the commit
  history for an agent's blocks (optionally filtered to one block/`label`).
- `sync_blocks_from_git` (`block_manager_git.py:564-596`) is an explicit
  "rebuild the PostgreSQL cache from git source of truth" operation
  (docstring, `block_manager_git.py:571`) -- confirming that once git-memory
  is enabled for an agent, the `Block.value` column in Postgres/SQLite
  becomes a derived cache, and the git repository (not the SQL row) is
  authoritative for that agent's core memory.
- `enable_git_memory_for_agent` (`block_manager_git.py:359-481`) is
  explicitly self-healing/idempotent: if the tag is already present but the
  repo is missing blocks or missing entirely, it backfills from whatever
  blocks currently exist in Postgres (`block_manager_git.py:379-442`,
  comment: "tags can be added via the agent update endpoint... we treat the
  tag as the source-of-truth 'desired state' and backfill the repo if
  missing").
- `disable_git_memory_for_agent` (`block_manager_git.py:485-506`) only
  removes the tag; it explicitly "keeps the git repo for historical
  reference" (docstring, `block_manager_git.py:492`) -- disabling is not
  destructive to the checkpoint history.

Separately, and without the git system, `Block` (core memory) is versioned
via SQLAlchemy's optimistic `version_id_col` (`letta/orm/block.py:61`) and
linked to a `block_history` table via `current_history_entry_id`
(`letta/orm/block.py`, FK column) -- `BlockHistory`
(`letta/orm/block_history.py:12-49`) stores a full snapshot per historical
state (`description`, `label`, `value`, `limit`, `metadata_` all copied,
plus `actor_type`/`actor_id` for attribution and a per-block monotonic
`sequence_number`, `letta/orm/block_history.py:46-48`), cascade-deleted with
its `Block` (`ondelete="CASCADE"`, `letta/orm/block_history.py:40-44`). This
looks like the always-on undo/redo trail (docstring: "Stores a single
historical state of a Block for undo/redo functionality",
`letta/orm/block_history.py:13`), distinct from and simpler than the opt-in
git system.

## Subagents and nested sessions

Letta's "subagent" concept is the **sleeptime agent** -- a background
memory-management agent linked to a "main" agent via a `Group` row
(`letta/orm/group.py:1-43`), with `Group.manager_type` set to
`ManagerType.sleeptime` or `ManagerType.voice_sleeptime`. Sleeptime agents
are first-class `Agent` rows (siblings), not entries nested inside the main
agent's transcript, and each maintains its own independent `messages`
history -- there is no shared-transcript nesting.

The durable parent-child link is the `groups`/`groups_agents` join plus
`Group.manager_agent_id` (FK to `agents.id`, `ondelete="RESTRICT"`,
`letta/orm/group.py:24`). `RESTRICT` on that FK means the database itself
will refuse to delete a manager agent while a group still references it as
manager -- deletion has to go through the application-level cleanup path
below rather than a raw `DELETE`.

`AgentManager.delete_agent_async` (`letta/services/agent_manager.py:1320-1397`)
is the actual cascade/cleanup code path, handling both delete directions:

```python
# letta/services/agent_manager.py:1340-1341
# Handle case where we're deleting a sleeptime agent (not the main agent)
# In this case, we need to clean up the group and the main agent's enable_sleeptime flag
```

- Deleting a sleeptime/voice-sleeptime agent directly: finds its
  `Group` (`agent_manager.py:1344-1351`), deletes that group, and clears
  `enable_sleeptime` on the main agent it belonged to
  (`agent_manager.py:1353-1362, 1387-1390`).
- Deleting the main agent: if it has a `multi_agent_group` of manager type
  `sleeptime`/`voice_sleeptime`, every participant sleeptime agent is loaded
  and added to the deletion set alongside the main agent, and the group is
  deleted too (`agent_manager.py:1365-1377`). All deletions in the batch
  commit inside one `try`/`except`, rolling back together on failure
  (`agent_manager.py:1379-1394`).

This is explicit application-level cascade logic, not a database `ON DELETE
CASCADE` -- the FK from group to manager agent is `RESTRICT` precisely
because the app needs to run this cleanup first. Nesting is not depth-bounded
in the code read (a group has exactly one manager and a flat list of
participant agent IDs -- `Group.agent_ids`, `letta/orm/group.py:36` -- not a
recursive tree).

## Retention, deletion, and multi-host

**Retention/TTL.** No scheduled retention or TTL-cleanup job was found:
`letta/jobs/scheduler.py` contains no retention/expiry logic (grepped for
`TTL`/`expire`/`retention`/`cleanup`; the only match was an unrelated log
line about scheduler shutdown, `letta/jobs/scheduler.py:228`). Deletion is
entirely caller-driven (an explicit API call), never store-initiated.

**Delete cascade behavior**, established from `letta/orm/mixins.py:1-99`
and per-model overrides:
- `AgentMixin.agent_id` FKs (used by `messages`, `conversations`,
  `files_agents`, etc.) are `ondelete="CASCADE"`
  (`letta/orm/mixins.py:40`) -- deleting an agent hard-cascades all of its
  messages and conversations at the database level, in addition to the
  application-level sleeptime-agent cleanup above.
- `ArchiveMixin.archive_id` FK (used by `archival_passages`) is also
  `ondelete="CASCADE"` (`letta/orm/mixins.py:80`) -- passages cascade with
  their *archive*, not with any one agent.
- The `archives_agents` junction table has `ondelete="CASCADE"` on **both**
  FK directions (`letta/orm/archives_agents.py:23-24`), but that only
  removes the attachment row linking an agent to an archive -- it never
  deletes the `Archive` itself. No code path calls
  `delete_archive_async` (`letta/services/archive_manager.py:266-279`, a
  hard delete) from within `delete_agent_async`. Practical consequence:
  **archival memory outlives agent deletion** by default; an archive (and
  its passages) becomes an unreferenced-but-still-live row set once its last
  agent is deleted, unless something explicitly calls `delete_archive_async`.
- `Conversation` deletion is reference-counted rather than a blanket
  cascade, described in detail above (`conversation_manager.py:556-609`):
  soft-delete the conversation and its own message associations, but only
  soft-delete `Message` rows not still referenced by another live
  conversation.
- Isolated per-conversation `Block`s are hard-deleted on conversation
  delete (`Block` has no soft-delete support, per the comment at
  `conversation_manager.py:602-603`), after first removing their junction
  rows (`conversation_manager.py:605-609`).

**Multi-host behavior.** Letta's engine/session setup
(`letta/server/db.py:1-146`) is a single shared async SQLAlchemy engine per
process (`create_async_engine`, `letta/server/db.py:58`), pooled with
`AsyncAdaptedQueuePool` (configurable pool size/overflow/timeout/recycle) or
`NullPool` if pooling is disabled (`letta/server/db.py:24-41`) -- this is the
standard "many stateless app servers, one shared Postgres" topology; nothing
in the code assumes a shared filesystem or does its own crash detection.
Statement caching is explicitly disabled for asyncpg
(`statement_cache_size: 0`, `prepared_statement_cache_size: 0`,
`letta/server/db.py:48-49`) and each connection gets a UUID-suffixed
prepared-statement name (`letta/server/db.py:47`) -- both of these are
classic PgBouncer/transaction-pooling-mode compatibility settings, i.e.
Letta's design assumes it may run behind a connection pooler in front of
Postgres, consistent with a genuinely multi-host deployment rather than a
single-box assumption (inference from the settings shape; no explicit
"PgBouncer" comment was found to cite directly). The `db_registry.async_session()`
context manager retries transient `ConnectionError`s (`letta/server/db.py:84-116`)
specifically to smooth over exactly the kind of blip multi-host/pooled
deployments produce. The Redis-backed lock used by the git-memory commit
path (`git_operations.py:384-386`) is the one place I found an explicit
cross-process/cross-host mutual-exclusion primitive outside the RDBMS
transaction itself -- everything else relies on Postgres transactions and
(for `Block` only) optimistic version checks for multi-host safety.

## Interop with foreign session stores

Letta does not read or import other agent frameworks' native session
stores. It has its own portable export/import format instead -- the
"agent file" (`.af`), defined in `letta/schemas/agent_file.py:1-60+` and
implemented in `letta/services/agent_serialization_manager.py`
(not read in depth; out of the scope this dossier prioritized). This is
Letta-to-Letta portability (`ImportResult`/`MessageSchema` types at
`letta/schemas/agent_file.py:29-52`), not consumption of a foreign product's
transcript format, so it does not answer "does Letta read another product's
store" in the sense research question 12 asks -- noted as a gap rather than
answered affirmatively.

## What this implies for our Session Store (our inference)

Letta's durable "session" is best understood as a mostly-append-only
`messages` row set, permanently owned by a long-lived `Agent` entity, with a
thin and explicitly-acknowledged-as-fragile mutable pointer
(`message_ids`/`in_context`) layered on top that determines what the model
currently sees -- that pointer, not the message log, is the one piece of
state that cannot be reconstructed by replay, and Letta's own maintainers
are mid-migration away from its riskiest form (a JSON array with no ORM
relationship) toward a proper join table. This maps cleanly onto our
event-sourced Session Store's distinction between the durable log and
derived read models, with two caveats worth carrying forward: (1) Letta
demonstrates that "the pointer" needs its own concurrency story -- a bare
last-write-wins column update is exactly the failure mode our design should
avoid for any per-turn in-context-window projection; and (2) Letta's
archival/vector-search tier shows that a derived search index can be
allowed to silently and permanently miss historical data if it is enabled
after data already exists, with no reconciliation job -- if our design wants
"enable search later" to actually mean "search everything," it needs an
explicit backfill path that Letta's own codebase does not have.

## Open questions

- **Turbopuffer backfill**: confirmed absent in the code paths read
  (`letta/helpers/tpuf_client.py`, `letta/services/archive_manager.py`,
  `letta/services/message_manager.py`), but I did not check for an
  out-of-repo/ops-only backfill script (e.g. in a separate deployment repo)
  that might exist outside this checkout. Treat "no backfill exists" as
  "no backfill exists in `letta-ai/letta` at this commit," not as a claim
  about Letta's hosted-cloud operations.
- **`StorageBackend`** behind the git-memory system
  (`letta/services/memory_repo/git_operations.py:64`, constructor parameter)
  was referenced but its concrete implementation (S3? local disk? something
  else?) was not opened -- I cannot confirm what medium actually stores the
  per-agent git repositories at rest.
- **`agent_file.py` / `agent_serialization_manager.py` export-import
  semantics** (whether it's a snapshot-only export or supports true
  round-trip resume) were not investigated beyond the type definitions
  glimpsed at `letta/schemas/agent_file.py:1-60`; out of scope for this
  pass given the task's emphasis elsewhere.
- **Downgrade-path testing**: I could not confirm from the files read
  whether Alembic `downgrade()` functions are exercised in CI, only that an
  `alembic-validation.yml` workflow exists; whether Letta's migration
  ratchet is genuinely reversible in practice is unverified.
- **`Step` model and per-LLM-call metadata** (`letta/orm/step.py`) were
  read only partially (first ~50 lines); its relationship to `Run` and to
  the durability story for partial/streaming responses was not investigated
  in depth.
- **`Summarizer`**: only `summarize()` (lines 75-123) and
  `_partial_evict_buffer_summarization` (lines 136-243) were read in
  detail out of roughly 900 lines in `letta/services/summarizer/summarizer.py`;
  `_static_buffer_summarization` (starting line 244) and the LLM-driven
  summarization call itself were not examined.
- **Passage tag storage**: `ArchivalPassage` has both a JSON `tags` column
  and a `passage_tags` junction-table relationship
  (`letta/orm/passage.py`) described in the source as complementary/dual
  storage; I did not trace why both exist or which is authoritative.

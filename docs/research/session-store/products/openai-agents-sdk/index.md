# OpenAI Agents SDK (Python): how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot: local shallow checkout of `openai/openai-agents-python`
at commit `7b7587425a17676f5a713d346abec76db30e0eab` (committed 2026-08-04,
license MIT, package `openai-agents` version `0.19.2` per `pyproject.toml`).
Every citation below was verified against this exact commit on 2026-08-04.
Paths are relative to the repository root (`src/agents/...`, `docs/...`)
unless stated otherwise; citations use `path:line` shorthand.

Authoritative anchors:

- `openai/openai-agents-python` @ `7b7587425a17676f5a713d346abec76db30e0eab`
- `src/agents/memory/session.py` (the `Session` protocol/ABC)
- `src/agents/memory/sqlite_session.py` (default local backend)
- `src/agents/memory/session_settings.py`, `src/agents/memory/util.py`
- `src/agents/run_internal/session_persistence.py` (Runner-facing
  append/read/rewind/compaction orchestration)
- `src/agents/extensions/memory/**` (`SQLAlchemySession`, `RedisSession`,
  `MongoDBSession`, `DaprSession`, `AdvancedSQLiteSession`,
  `AsyncSQLiteSession`, `EncryptedSession`)
- `src/agents/extensions/experimental/codex/**` (subprocess wrapper around
  the `codex` CLI binary -- the sole discovered interop surface with Codex CLI)
- `docs/sessions/index.md`, `docs/ref/memory.md`

> Scope note. This dossier is written specifically to answer a comparison
> question already raised in this corpus: OpenAI ships **Codex CLI**
> (documented at
> [`../codex-cli/index.md`](../codex-cli/index.md)), whose durable record is
> an append-only JSONL rollout log with a derived SQLite projection, and it
> *separately* ships this Agents SDK with a `Session` abstraction whose
> reference backend is a plain SQLite table storing JSON blobs with no
> append-only guarantee at all. Both products live in the same GitHub org
> (`openai`) and the same commit history era, but -- as documented below --
> they share no code, no on-disk format, and no store-level interop path.
> The only bridge between them is a CLI/subprocess wrapper
> (`src/agents/extensions/experimental/codex/`) that shells out to the
> `codex` binary and tracks its opaque `thread_id` string; it does not read
> or write Codex's rollout files, and the Agents SDK's `Session` protocol is
> never used to store or serve Codex CLI data. Any explanation of *why*
> OpenAI maintains two incompatible session models is not attempted here;
> only what and where they differ is documented, and it is marked
> **[inference]** wherever this dossier goes beyond what the source shows.

## The storage model

The SDK's own docs describe the durable session as a conversation-item list,
not a log or event stream:

> "Sessions stores conversation history for a specific session, allowing
> agents to maintain context without requiring explicit manual memory
> management." (`docs/sessions/index.md:5`)

> "Before each run: The runner automatically retrieves the conversation
> history for the session and prepends it to the input items. After each
> run: All new items generated during the run (user input, assistant
> responses, tool calls, etc.) are automatically stored in the session."
> (`docs/sessions/index.md:66-67`)

There is no single canonical storage format -- the `Session` protocol
(`src/agents/memory/session.py:13-54`) is pluggable, and the SDK ships at
least nine concrete backends with materially different physical models:

- **`SQLiteSession`** (`src/agents/memory/sqlite_session.py:17`) -- a SQLite
  table of JSON-blob rows, one row per conversation item, WAL-mode, default
  `:memory:` unless a file path is given (`src/agents/memory/sqlite_session.py:30-33`).
- **`AsyncSQLiteSession`** -- same physical model via `aiosqlite`
  (`src/agents/extensions/memory/async_sqlite_session.py`, header confirmed;
  not read in full).
- **`AdvancedSQLiteSession`** (`src/agents/extensions/memory/advanced_sqlite_session.py:1`,
  class declared subclassing `SQLiteSession`) -- the same row-per-item table
  plus two additional tables, `message_structure` and `turn_usage`, that
  index/annotate the same rows for branching and usage analytics
  (`src/agents/extensions/memory/advanced_sqlite_session.py:43-160`).
- **`SQLAlchemySession`** (`src/agents/extensions/memory/sqlalchemy_session.py`)
  -- any SQLAlchemy-async-capable SQL database (Postgres, MySQL, SQLite via
  `aiosqlite`), real transactions, `busy_timeout` tuning
  (`src/agents/extensions/memory/sqlalchemy_session.py`, `_SQLITE_BUSY_TIMEOUT_MS = 5000`).
- **`RedisSession`** (`src/agents/extensions/memory/redis_session.py`) -- Redis
  keys/lists, headers read, full implementation not read line-by-line.
- **`MongoDBSession`** (`src/agents/extensions/memory/mongodb_session.py`) --
  two Mongo collections, `agent_sessions` and `agent_messages`, each message
  document carrying a monotonically increasing `seq` field for ordering
  across concurrent writers (`docs/sessions/index.md:453`).
- **`DaprSession`** (`src/agents/extensions/memory/dapr_session.py`) -- a Dapr
  state-store key/value entry, backend-agnostic (30+ possible physical
  stores behind Dapr), with ETag-based optimistic concurrency and optional
  TTL (`docs/sessions/index.md:391-421`).
- **`OpenAIConversationsSession`** (`src/agents/memory/openai_conversations_session.py:1-139`)
  -- no local storage at all; every item lives server-side in OpenAI's
  Conversations API. The class is a thin RPC client, not a local store.
- **`OpenAIResponsesCompactionSession`** (`src/agents/memory/openai_responses_compaction_session.py:1-534`)
  -- not a storage backend itself, a *decorator* that wraps any other
  `Session` and periodically replaces its contents via `responses.compact`.
- **`EncryptedSession`** (`src/agents/extensions/memory/encrypt_session.py:1-214`)
  -- also a decorator: wraps any other `Session`, storing Fernet-encrypted
  envelopes instead of plaintext JSON in whatever the underlying backend is.

Given this, the single most accurate general description is: **the durable
session is a conversation-item list (an ordered set of opaque JSON blobs)
whose physical representation is entirely backend-dependent, and none of the
built-in backends store an append-only event log** -- the two operations
that mutate history after the fact (`pop_item`, used for rewind and
corrections, and `run_compaction`, used for compaction) are implemented as
destructive deletes/rewrites in every backend read for this dossier, not as
appended tombstone/marker records. This is the central contrast with Codex
CLI's rollout log, which never deletes a line and instead appends
`Compacted`/`ThreadRolledBack` markers
(see [`../codex-cli/index.md`](../codex-cli/index.md)).

There is no authoritative/derived split inside a single backend the way
Codex CLI has (JSONL log authoritative, SQLite `state` db derived). Each
Agents-SDK backend's row set/document *is* the authoritative record; the
only backend with an explicit "derived" concept is `AdvancedSQLiteSession`'s
`message_structure` table, which is a same-database indirection table over
the shared `agent_messages` rows for branching, not a separate rebuildable
projection (see **Rewind, checkpoints, and fork** below).

Conceptual-model fit: **session-as-row-set / session-as-document**,
depending on backend -- never session-as-append-only-log in the built-in
backends. (The one exception found is the `examples/memory/file_session.py`
sample, which is explicitly a full-file JSON array rewrite on every write,
i.e. session-as-mutable-document, and is example code rather than a shipped
backend.)

## Keying and identity

- A session is addressed by a single opaque `session_id: str`, declared
  directly on the `Session` protocol (`src/agents/memory/session.py:21`) and
  on `SessionABC` (`src/agents/memory/session.py:67`). There is no
  project/workspace/cwd component in the protocol itself -- any such
  namespacing is left to the caller (e.g. by choosing `sessions_table`/
  `messages_table` names or a `db_path`, or by prefixing the `session_id`
  string).
- Session ids are **caller-supplied**, not minted by the SDK. Every
  constructor takes `session_id` as a required first argument (e.g.
  `SQLiteSession.__init__(self, session_id: str, ...)`,
  `src/agents/memory/sqlite_session.py:31-32`). There is no UUID generation,
  no ordering-encoding scheme, and no client-vs-server minting distinction --
  identity is whatever string the application chooses
  (`docs/sessions/index.md:513-519` recommends patterns like `"user_12345"`
  or `"thread_abc123"` but these are conventions, not enforced formats).
- `OpenAIConversationsSession` is the one exception: it can either mint a
  new server-side conversation (`conversation_id=None`, lazily created on
  first use) or resume an existing one by passing `conversation_id=...`
  (`src/agents/memory/openai_conversations_session.py`, `docs/sessions/index.md:236-237`).
  In that backend the durable identity lives entirely in OpenAI's Conversations
  API, not in the SDK.
- **No listing/enumeration API exists anywhere in the core protocol.** The
  `Session` Protocol and `SessionABC` expose only `get_items`, `add_items`,
  `pop_item`, `clear_session` (`src/agents/memory/session.py:24-54`,
  `:70-104`) -- there is no `list_sessions`/`list_session_ids` method, and a
  repo-wide search for such names in `src/agents/` returns no hits. Listing,
  if needed, is entirely up to the chosen backend's native tooling (e.g.
  querying the SQLite `agent_sessions` table directly, or the Mongo
  `agent_sessions` collection) -- it is not a protocol-level concept.
- Because `session_id` is a plain, uninterpreted string, there is no
  concept of relocation/rename reconciliation (no cwd, no worktree, no
  encoded path) -- nothing to reconcile at the identity layer. **[inference]**
  This is a natural consequence of the protocol treating identity purely as
  an opaque key handed to whatever backend, with no filesystem or
  project-path semantics baked in anywhere in `src/agents/memory/`.

## The store interface

The protocol is pluggable and is captured **verbatim** below, exactly as
defined in `src/agents/memory/session.py:13-104`:

```python
@runtime_checkable
class Session(Protocol):
    """Protocol for session implementations.

    Session stores conversation history for a specific session, allowing
    agents to maintain context without requiring explicit manual memory management.
    """

    session_id: str
    session_settings: SessionSettings | None = None

    async def get_items(self, limit: int | None = None) -> list[TResponseInputItem]:
        """Retrieve the conversation history for this session.

        Args:
            limit: Maximum number of items to retrieve. If None, retrieves all items.
                   When specified, returns the latest N items in chronological order.

        Returns:
            List of input items representing the conversation history
        """
        ...

    async def add_items(self, items: list[TResponseInputItem]) -> None:
        """Add new items to the conversation history.

        Args:
            items: List of input items to add to the history
        """
        ...

    async def pop_item(self) -> TResponseInputItem | None:
        """Remove and return the most recent item from the session.

        Returns:
            The most recent item if it exists, None if the session is empty
        """
        ...

    async def clear_session(self) -> None:
        """Clear all items for this session."""
        ...
```

(`src/agents/memory/session.py:13-54`)

`SessionABC` (`src/agents/memory/session.py:57-104`) is an identical
`abc.ABC`-based restatement of the same four abstract methods, "intended
for internal use and as a base class for concrete implementations" while
"third-party libraries should implement the `Session` protocol instead"
(`src/agents/memory/session.py:63-64`). All four methods are `async` and
**all four are required** -- there is no optional method in the base
contract.

One optional extension protocol exists, layered on top of `Session`:

```python
@runtime_checkable
class OpenAIResponsesCompactionAwareSession(Session, Protocol):
    """Protocol for session implementations that support responses compaction."""

    async def run_compaction(self, args: OpenAIResponsesCompactionArgs | None = None) -> None:
        """Run the compaction process for the session."""
        ...
```

(`src/agents/memory/session.py:131-137`), detected at runtime via
`is_openai_responses_compaction_aware_session()`
(`src/agents/memory/session.py:140-150`), which does a `getattr`/`callable`
duck-type check rather than an `isinstance` check on the `Protocol` (an
`isinstance` check against a `@runtime_checkable` `Protocol` only verifies
method *presence*, not signature -- the helper still relies on structural
typing, consistent with the rest of the module).

Two instance attributes are part of the protocol surface but not methods:
`session_id: str` (required) and `session_settings: SessionSettings | None = None`
(optional, defaults to `None` at the protocol level;
`src/agents/memory/session.py:21-22`). `SessionSettings`
(`src/agents/memory/session_settings.py:30-64`) is a pydantic dataclass with
a single documented field today, `limit: int | None = None`
("Maximum number of items to retrieve. If `None`, retrieves all items.",
`src/agents/memory/session_settings.py:38-39`), plus a `.resolve(override)`
method that overlays non-`None` fields from a per-call override onto the
session's default settings (`src/agents/memory/session_settings.py:41-60`) --
used by `RunConfig.session_settings` to override a session's default
`limit` on a single `Runner.run` call (`docs/sessions/index.md:110-133`).
`resolve_session_limit(explicit_limit, settings)`
(`src/agents/memory/session_settings.py:18-27`) picks an explicit
per-call `limit` first, else falls back to the session's own settings, else
`None` (unbounded).

Call sites confirming when each method fires (all in
`src/agents/run_internal/session_persistence.py`, the Runner-facing
orchestration layer -- see next two sections for detail):

| Operation | Caller | path:line |
|---|---|---|
| `get_items` | `prepare_input_with_session` (pre-run history fetch) | `src/agents/run_internal/session_persistence.py:189,191` |
| `add_items` | `save_result_to_session` (post-turn append) | `src/agents/run_internal/session_persistence.py:452` |
| `pop_item` | `_rewind_session_tail_suffix` (retry/rewind path) | `src/agents/run_internal/session_persistence.py:531,555` and `:761` (helper, read but not quoted here) |
| `clear_session` + `add_items` | `OpenAIResponsesCompactionSession.run_compaction` (destructive replace) | `src/agents/memory/openai_responses_compaction_session.py` (method body, full file read) |

## Write and append path (ordering, durability, concurrency, delivery)

**Ordering.** In the reference `SQLiteSession`, ordering is the SQLite
`INTEGER PRIMARY KEY AUTOINCREMENT` column `id` on the messages table
(`src/agents/memory/sqlite_session.py:162`), populated purely by insertion
order (`ORDER BY id ASC`/`DESC` in every read query,
`src/agents/memory/sqlite_session.py:237,252,271`). There is no
client-supplied sequence number or timestamp used for ordering -- the
`created_at` columns exist (`src/agents/memory/sqlite_session.py:153,165`)
but are not read back anywhere in this file. `MongoDBSession` instead
attaches "an atomic sequence counter" (`seq`) per message document
specifically because Mongo has no auto-increment primary key
(`docs/sessions/index.md:209,453`) -- the same ordering *guarantee*, a
different *mechanism*, because the backend lacks a native monotonic key.

**Durability/atomicity -- `SQLiteSession`.** `add_items` wraps
`_insert_items` + `conn.commit()` in a `try/except` that explicitly calls
`conn.rollback()` on any exception, with a comment spelling out why:

> "`_locked_connection()` does not manage transactions; roll back
> explicitly so a failure partway through the insert never leaves a partial
> mutation or an open transaction on this cached connection. An open write
> transaction would hold the SQLite write lock for the lifetime of the
> connection and block every later writer."
> (`src/agents/memory/sqlite_session.py:295-299`)

`_insert_items` itself does three statements inside the (manually managed)
transaction: `INSERT OR IGNORE` the session row, `executemany` the batch of
message rows, then `UPDATE ... SET updated_at = CURRENT_TIMESTAMP`
(`src/agents/memory/sqlite_session.py:181-204`) -- all items in one
`add_items` call are appended as a single batch/transaction, not one
transaction per item.

**Concurrency -- `SQLiteSession`.** Concurrency is handled by an
**in-process** lock, not a SQLite-level primitive: a per-resolved-file-path
`threading.RLock`, shared across all `SQLiteSession` instances pointed at
the same file via class-level `_file_locks`/`_file_lock_counts` dictionaries
guarded by `_file_locks_guard` (`src/agents/memory/sqlite_session.py:26-28,91-116`).
Every DB operation runs inside `_locked_connection()`
(`src/agents/memory/sqlite_session.py:117-121`), which just acquires that
lock and yields a connection -- so within one Python process, writers to the
same file are fully serialized. `PRAGMA journal_mode=WAL` is set on every
connection opened (`src/agents/memory/sqlite_session.py:76,82,138`), but
**no `PRAGMA busy_timeout` is set anywhere in `sqlite_session.py`** (confirmed
by reading the file in full -- the string `busy_timeout` does not appear).
This means: for **two separate processes** writing the same SQLite file
concurrently, `SQLiteSession` has no application-level defense against
`SQLITE_BUSY` beyond whatever behavior WAL mode's default (effectively zero)
busy handler gives it -- cross-process contention could raise immediately
rather than retry. By contrast, `SQLAlchemySession` explicitly sets
`_SQLITE_BUSY_TIMEOUT_MS = 5000` and layers a bounded exponential-backoff
retry loop on top (`_SQLITE_LOCK_RETRY_DELAYS = (0.05, 0.1, 0.2, 0.4, 0.8)`,
`src/agents/extensions/memory/sqlalchemy_session.py`) specifically to
tolerate "database is locked" errors -- a materially more robust concurrency
story than the base backend, for the SQLite dialect. `DaprSession` is the
only backend offering true optimistic concurrency control: writes carry an
ETag precondition against the underlying state store, with a
`consistency=DAPR_CONSISTENCY_STRONG` option available for stronger
read-after-write guarantees (`docs/sessions/index.md:405-419`; source file
`src/agents/extensions/memory/dapr_session.py` header/API read, not read in
full).

**Delivery semantics / idempotence.** Above the backend layer, the Runner
orchestration in `src/agents/run_internal/session_persistence.py` treats
`add_items` as at-least-once and builds its own client-side dedup on top,
because retries (e.g. after a network error mid-turn) could otherwise
double-append. `save_result_to_session`
(`src/agents/run_internal/session_persistence.py:350-492`) tracks
`run_state._current_turn_persisted_item_count` to avoid re-sending
already-persisted items on retry (`:367,376,449,455`), and separately
computes a **content fingerprint** for every candidate item via
`fingerprint_input_item`/`_fingerprint_or_repr`
(`src/agents/run_internal/session_persistence.py:411-431`,
`src/agents/run_internal/items.py:334-369`) before calling
`deduplicate_input_items_preferring_latest`
(`src/agents/run_internal/session_persistence.py:423`). `fingerprint_input_item`
strips internal metadata and (optionally) the `id` field, then returns
`json.dumps(payload, sort_keys=True, default=str)`
(`src/agents/run_internal/items.py:334-369`) -- a normalized-JSON string, not
a hash. A related helper, `digest_input_item`
(`src/agents/run_internal/items.py:372-391`), additionally SHA-256-hashes
that fingerprint string (`hashlib.sha256(fingerprint.encode("utf-8")).hexdigest()`,
`src/agents/run_internal/items.py:391`) for "durable occurrence tracking."
**There is no store-assigned entry id used for identity** -- identity for
dedup/rewind purposes is entirely **content-fingerprint-based**, computed
by the Runner layer, not by any backend.

## Read and resume path

`prepare_input_with_session`
(`src/agents/run_internal/session_persistence.py:157-290`) is the sole
resume entry point used by the Runner. On every call it does a **full or
limit-bounded `get_items` read** -- not a cursor/offset read -- then merges it
with the new turn's input:

```python
if resolved_settings.limit is not None:
    history = await session.get_items(limit=resolved_settings.limit)
else:
    history = await session.get_items()
```
(`src/agents/run_internal/session_persistence.py:188-191`)

There is no separate "cache" read on resume -- the session's `get_items` call
*is* the read path; nothing is read from a local filesystem cache first
(confirmed: no cache/memoization wrapper exists around `get_items` in this
file). The docs describe the same two-step model narratively:

> "1. Session history (retrieved from `session.get_items(...)`) 2. New turn
> input" (`docs/sessions/index.md:76-77`)

**Pagination/size bound.** `get_items(limit=...)` is the only bound offered
by the protocol, and it is a **tail-window count**, not a byte/offset
pagination cursor -- "retrieves the latest N items" per the protocol
docstring (`src/agents/memory/session.py:28-29`). In `SQLiteSession`, when
`session_limit > 0`, the implementation does a `SELECT ... ORDER BY id DESC
LIMIT ?` window, decodes rows, and **doubles the window and retries** if
fewer than `session_limit` valid (non-corrupt) items came back, to guarantee
"limit counts valid conversation items"
(`src/agents/memory/sqlite_session.py:243-263`, comment at `:244-245`).
Corrupt/non-JSON rows are silently skipped during decode
(`src/agents/memory/sqlite_session.py:218-227`, `except (json.JSONDecodeError, TypeError): continue`).
When `limit is None`, it is a single unbounded `SELECT ... ORDER BY id ASC`
(`src/agents/memory/sqlite_session.py:231-241`) -- the entire history is
materialized into memory in one call; there is no streaming/incremental
read.

**`RunConfig.session_input_callback`** (`SessionInputCallback`,
`src/agents/memory/util.py:8-11`) lets a caller intercept the merge of
`history` and `new_input` immediately before the model call -- "Use this
when you need custom pruning, reordering, or selective inclusion of
history without changing how the session stores items"
(`docs/sessions/index.md:108`). Critically, this callback affects only the
*model-visible* input for that turn; it does not change what gets persisted
-- `prepare_input_with_session` separately tracks which combined-output items
came from `new_input` vs. `history` (via reference/frequency maps,
`src/agents/run_internal/session_persistence.py:226-263`) specifically "so
retries and custom merge strategies do not accidentally re-persist old
history as fresh input" (`src/agents/run_internal/session_persistence.py:173-178`).
This is the one mechanism in the SDK that lets an application-level caller
bound what the *model* sees without bounding what the *store* holds -- i.e.
model-context trimming and store growth are explicitly decoupled here.

## Listing, summaries, and search

There is **no listing/summary/search subsystem in the core `Session`
protocol** (see **Keying and identity** above -- no `list_sessions` method
exists anywhere in `src/agents/memory/` or `src/agents/run_internal/`).
Backend-specific facilities exist only in the two SQLite variants:

- `AdvancedSQLiteSession.find_turns_by_content(...)` does a plain SQL `LIKE`
  substring scan over the `message_structure`/`agent_messages` tables
  (`src/agents/extensions/memory/advanced_sqlite_session.py`, method read in
  the 1090-1165 range) -- there is no full-text-search (FTS) index and no
  vector index; it is a linear `LIKE` query.
- `AdvancedSQLiteSession.list_branches(...)`
  (`src/agents/extensions/memory/advanced_sqlite_session.py`, 808-960 range)
  enumerates branch metadata from the `message_structure` table -- this is
  branch listing within one session, not cross-session listing.

No backend maintains a separate denormalized "summary" read-model at write
time analogous to Codex CLI's SQLite `state` projection
(see [`../codex-cli/index.md`](../codex-cli/index.md)). **[inference]**
The absence of any cross-session listing primitive in the base protocol
suggests session enumeration is treated entirely as an application/hosting
concern outside the SDK's scope -- every backend that supports it does so by
exposing its own native storage medium (SQL table, Mongo collection) for the
caller to query directly, not through a protocol-level API.

## Entry/message structure and versioning

**Entry type.** The item type stored and returned by every backend is
`TResponseInputItem`, which is a type alias, not an SDK-defined class:

```python
TResponseInputItem = ResponseInputItemParam
```
confirmed at `src/agents/items.py:76` -- `ResponseInputItemParam` is an
OpenAI Python SDK (`openai` package) type, i.e. the **wire shape of OpenAI's
Responses API input items**, not an Agents-SDK-specific envelope. There is
no Agents-SDK-level wrapper (no `{type, timestamp, id}` outer envelope
added by the store) -- every backend persists and returns these items
directly.

**Opaque vs. parsed.** `SQLiteSession` treats items as fully opaque blobs:
`json.dumps(item)` on write (`src/agents/memory/sqlite_session.py:189`),
`json.loads(message_data)` on read with no schema validation
(`src/agents/memory/sqlite_session.py:222`) -- the store never inspects
field contents except to catch decode failures. The one place the *Runner*
(not the store) inspects item shape is the content-fingerprinting/dedup
logic described above, and `OpenAIConversationsSession`'s
`_sanitize_openai_conversation_item`/`_is_unpersistable_for_openai_conversation`
helpers (`src/agents/run_internal/session_persistence.py:697-727`, read),
which strip or reject certain items before sending them to OpenAI's server
because that particular backend's server enforces its own item-shape rules.
So: **opaque at the local-backend layer, lightly parsed at the Runner
orchestration layer for identity/compat reasons, and inspected by the
server for the `OpenAIConversationsSession` backend specifically.**

**Versioning.** No schema-version field was found anywhere in
`src/agents/memory/` or `src/agents/run_internal/session_persistence.py` --
neither an item-level `schema_version` nor a store-format version stamp.
Because the entry format is simply "whatever `ResponseInputItemParam` is in
the currently installed `openai` package version," format evolution is
implicitly tied to the `openai` SDK's own versioning, not to anything the
Agents SDK's session layer manages. This is stated as a **gap** (see **Open
questions**) rather than a confirmed absence of any version-compat handling
elsewhere in the `openai` package, which was out of scope for this
checkout.

## Compaction and history management

There is **no compaction in the store/protocol layer** -- `Session`,
`SessionABC`, and every plain backend (`SQLiteSession`, `RedisSession`,
`MongoDBSession`, `SQLAlchemySession`, `DaprSession`,
`OpenAIConversationsSession`) hold every item indefinitely; nothing trims
them. Growth is bounded only by two things:

1. **Retrieval-time limiting** -- `SessionSettings.limit`/
   `get_items(limit=...)` bounds what is *read into the model context* per
   turn, not what is stored (`src/agents/memory/session_settings.py:38-39`,
   `docs/sessions/index.md:110-133`). The underlying row/document count is
   unaffected.
2. **`OpenAIResponsesCompactionSession`** -- an explicit opt-in decorator
   (`src/agents/memory/openai_responses_compaction_session.py:1-534`) that
   wraps another `Session` and can trim it. This is the only place actual
   compaction logic lives, and it is emphatically **not part of the store
   layer** -- it is a separate class layered on top, matching the research
   brief's framing.

`OpenAIResponsesCompactionSession` mechanics: `DEFAULT_COMPACTION_THRESHOLD = 10`
candidate items trigger auto-compaction after a turn by default (checked via
`should_trigger_compaction`, overridable per-instance,
`docs/sessions/index.md:277,298`). `select_compaction_candidate_items`
excludes user messages and items that are themselves prior compaction
summaries from the candidate set (full file read). `run_compaction(args)`
calls OpenAI's `responses.compact` API to get a compacted summary, then
performs a **destructive replace of the underlying session**: it calls the
underlying session's `clear_session()` and then `add_items()` with the new
(compacted) item set, with compensating restore-on-failure logic if the
compact call itself fails partway (full file read,
`src/agents/memory/openai_responses_compaction_session.py`). This is
triggered from the Runner via `save_result_to_session`:

```python
if response_id and is_openai_responses_compaction_aware_session(session):
    ...
    await session.run_compaction(compaction_args)
```
(`src/agents/run_internal/session_persistence.py:457-490`)

The docs explicitly warn this blocks streaming completion: "Compaction
clears and rewrites the session history, so the SDK waits for compaction to
finish before considering the run complete... `run.stream_events()` can
stay open for a few seconds after the last output token if compaction is
heavy" (`docs/sessions/index.md:284-285`).

**Contrast with Codex CLI:** Codex's compaction leaves a `Compacted` marker
appended to the still-intact rollout log -- the original lines are never
deleted (see [`../codex-cli/index.md`](../codex-cli/index.md)). The Agents
SDK's only compaction mechanism is the opposite: a full destructive
`clear_session()` + `add_items()` cycle that leaves no trace of the
pre-compaction items in the store once it completes successfully. There is
no append-only marker anywhere in this SDK's compaction path.

## Rewind, checkpoints, and fork

**Rewind.** Implemented via repeated calls to `pop_item()`, which is
**destructive row/item deletion**, not an appended marker. In the reference
backend:

```python
cursor = conn.execute(
    f"""
    DELETE FROM {self.messages_table}
    WHERE id = (
        SELECT id FROM {self.messages_table}
        WHERE session_id = ?
        ORDER BY id DESC
        LIMIT 1
    )
    RETURNING message_data
    """,
    (self.session_id,),
)
```
(`src/agents/memory/sqlite_session.py:315-326`) -- an atomic
`DELETE ... RETURNING` in one statement, looped to skip past corrupt JSON
rows (`src/agents/memory/sqlite_session.py:332-354`). The Runner-level
`rewind_session_items`/`_rewind_session_tail_suffix`
(`src/agents/run_internal/session_persistence.py:519-621`, `:761` for the
suffix helper) calls `pop_item()` repeatedly to undo a specific
content-fingerprinted suffix of recently-added items when "a conversation
retry is needed, so we do not accumulate duplicate inputs on lock errors"
(`src/agents/run_internal/session_persistence.py:525-526`). This is
explicitly **best-effort**: it matches the current tail against expected
fingerprints and *skips* the rewind entirely with a warning if the tail
doesn't match (`:560-563`), and `wait_for_session_cleanup`
(`src/agents/run_internal/session_persistence.py:624-661`) polls
`get_items(limit=window)` up to 5 times to confirm the rewound items are
actually gone, rather than assuming a strong read-after-write guarantee --
direct evidence that the store's consistency semantics under retry are not
fully trusted even by the SDK's own orchestration code. `pop_item` also
doubles as an explicit user-facing correction primitive:
"`pop_item` is particularly useful when you want to undo or modify the last
item in a conversation" (`docs/sessions/index.md:165-166`).

**Checkpoints.** `RunState.to_json()`/`RunState.from_json()`
(`src/agents/run_state.py`, `to_json` at line 728, `from_string` at 1105,
`from_json` at 1146 per grep of `class RunState`) is an **application-managed,
out-of-band checkpoint** for interrupted/human-in-the-loop runs -- explicitly
separate from the `Session` protocol. The docs show the pattern:

```python
result = await Runner.run(agent, "Delete temporary files...", session=session)
if result.interruptions:
    state = result.to_state()
    for interruption in result.interruptions:
        state.approve(interruption)
    result = await Runner.run(agent, state, session=session)
```
(`docs/sessions/index.md:53-59`) -- the `state` object round-trips through
the caller's own persistence (e.g. serialized to a file, per
`examples/memory/file_hitl_example.py`, partially read), not through
`session.add_items`/`get_items`. This is **not file-state or diff-based
checkpointing** in the sense Codex CLI's environment snapshots are; it is a
serialized run-state object for resuming a paused agent loop.

**Fork/branching.** Only `AdvancedSQLiteSession` implements branching, via
`create_branch_from_turn`/`create_branch_from_content`/`switch_to_branch`/
`delete_branch`/`list_branches`
(`src/agents/extensions/memory/advanced_sqlite_session.py`, methods present
in the 808-1283 range read). The mechanism is a **shared-row-by-reference
fork**: `_copy_messages_to_new_branch` (same file) copies entries into the
`message_structure` indirection table for the new branch while the
underlying `agent_messages.message_id` rows are shared/reused rather than
duplicated -- conceptually the closest thing in this SDK to Codex CLI's
`history_base` shared-prefix pointer fork, but implemented as SQL rows in
one database rather than file lineage across files. A `_generation` counter
provides optimistic-concurrency protection between branch-pointer updates
and a concurrent `clear_session()` call (same file, 1229-1283 range read).
No other backend (SQLite base, Redis, Mongo, SQLAlchemy, Dapr,
OpenAIConversations) has any fork/branch concept at all.

## Subagents and nested sessions

This SDK has **two distinct "subagent" mechanisms with opposite durability
stories**:

**Handoffs** -- the new agent takes over the *same* `Runner.run` loop and
the *same* session; there is no separate child session at all. The
handoff-history mapping comment states this directly:

> "The mapped history is the exact model input. New items stay unchanged
> for session history." (`src/agents/handoffs/history.py:151-152`, inside
> `nest_handoff_history`, `src/agents/handoffs/history.py:83`)

So a handoff's conversation is durable exactly to the extent the parent
run's session is durable -- it is one continuous append stream from the
store's point of view, with `HandoffCallItem`/`HandoffOutputItem`
(`src/agents/items.py`, read at lines 270-310) persisted as ordinary items
carrying `source_agent`/`target_agent` references, not as a separate
storage record.

**Agents-as-tools** (`Agent.as_tool(...)`,
`src/agents/agent.py:575-597`) is architecturally different: it spawns a
fully separate nested `Runner.run(...)`/`Runner.run_streamed(...)` call
(`src/agents/agent.py:941-953` for the non-streaming path, `~859-871` for
the streaming path) with its **own, independent `session` parameter that
defaults to `None`**:

```python
def as_tool(
    self,
    tool_name: str | None,
    tool_description: str | None,
    ...
    session: Session | None = None,
    ...
) -> FunctionTool:
```
(`src/agents/agent.py:575-597`, `session` param at `:590`)

```python
run_result = await Runner.run(
    starting_agent=cast(Agent[Any], self),
    input=resume_state or resolved_input,
    ...
    session=session,
)
```
(`src/agents/agent.py:941-953`)

Because `session=None` is the default and it is threaded straight through
to the nested `Runner.run` call, **a nested agent-as-tool run has no
durable session at all unless the caller explicitly constructs and passes
one.** By default its conversation state is purely runtime/ephemeral: only
the nested run's *final output string* (or the `custom_output_extractor`'s
result) round-trips back into the parent's session, as an ordinary
tool-call-output item -- the nested run's own intermediate steps are never
persisted anywhere unless the caller wires a `session=` explicitly.

**On parent failure:** because there is no separate child session record by
default (agents-as-tools) and no separate session at all (handoffs), there
is nothing SDK-managed to cascade/orphan/reconcile. If the caller *did*
supply an explicit `session=` for a nested agent-as-tool run, that session
is an ordinary, independently-addressed `Session` instance the caller owns
-- the SDK does not establish or track any parent-child link between the
parent's session and that child session; a parent failure has no special
effect on it beyond whatever partial items were already `add_items`'d before
the failure (subject to the same best-effort rewind semantics described
above, if the caller's own retry path calls `rewind_session_items`).
**[inference]** No code path was found that deletes, orphans, or reconciles
a nested session on parent crash -- the absence appears to be because the
SDK does not model a parent-child session relationship as a first-class
concept at all, only as an implementation detail of whatever `session=`
value a caller happens to pass into `as_tool(...)`.

There is also `max_concurrent_subagents: int | None` in
`src/agents/extensions/experimental/hosted_multi_agent/model.py` (grepped,
not read in full) -- this is a runtime concurrency-limiting config field for
an experimental hosted-multi-agent extension, not a storage/session concept;
noted here for completeness but not further analyzed (see **Open
questions**).

## Retention, deletion, and multi-host

**Retention/deletion** is entirely backend-specific; the protocol itself
offers only `clear_session()` (wipe everything for one `session_id`) with no
TTL/lifecycle concept at the protocol level. Backend behaviors observed:

- `SQLiteSession.clear_session()` issues two `DELETE` statements (messages,
  then the session row) inside the file's serialized lock, no soft-delete
  (`src/agents/memory/sqlite_session.py:359-374`).
- `DaprSession` supports a `ttl=...` constructor option "to let the backing
  state store expire old session data automatically when the store supports
  TTL" (`docs/sessions/index.md:418`) -- the only backend with native
  TTL-based retention.
- `EncryptedSession` layers its own `ttl` on top of any backend: encrypted
  envelopes carry an expiry, and expired entries are silently skipped on
  decrypt rather than actively purged (`src/agents/extensions/memory/encrypt_session.py:1-214`,
  full file read) -- this is application-level silent-expiry, not
  storage-level deletion.
- `OpenAIConversationsSession` has no local retention concept -- retention is
  whatever OpenAI's Conversations API enforces server-side; out of scope for
  this checkout.

**Multi-host.** No backend in this SDK is designed around a single-host
assumption analogous to Codex CLI's design
(see [`../codex-cli/index.md`](../codex-cli/index.md)) -- quite the
opposite: `RedisSession`, `MongoDBSession`, `SQLAlchemySession` (against a
networked SQL server), and `DaprSession` are all explicitly positioned as
multi-process/multi-host-safe by their docs ("shared memory across
workers/services," `docs/sessions/index.md:207`; "multi-process storage,"
`:209`; "cloud-native deployments," `:210`). `SQLiteSession` itself is the
one backend that is **not** advertised as multi-host-safe -- its
concurrency story (in-process `RLock`, no `busy_timeout`, see **Write and
append path**) is explicitly single-machine/single-file-oriented, and the
docs position it as "Local development and simple apps"
(`docs/sessions/index.md:205`). So: **multi-host support is a first-class,
per-backend design choice in this SDK, not a workaround** -- callers pick a
backend precisely on this axis, per the backend-selection table in
`docs/sessions/index.md:203-214`.

## Interop with foreign session stores

**Checked directly, not assumed.** A repository-wide case-insensitive
search for "codex" across `src/`, `docs/`, and `pyproject.toml` turns up
exactly one relevant hit outside of test files and unrelated model-name
strings (e.g. `"gpt-5.2-codex"` as a model identifier): the experimental
package `src/agents/extensions/experimental/codex/`.

This package is a **subprocess/CLI wrapper around the actual `codex`
binary**, not a shared storage layer. `CodexExec.run`
(`src/agents/extensions/experimental/codex/exec.py`) builds a command line:

```python
# Build the CLI args for `codex exec --experimental-json`.
command_args: list[str] = ["exec", "--experimental-json"]
...
command_args.extend(["resume", args.thread_id])
```
(`src/agents/extensions/experimental/codex/exec.py:62-63,108`) and spawns it
via `asyncio.create_subprocess_exec(...)`
(`src/agents/extensions/experimental/codex/exec.py:119`) -- i.e. it drives
Codex CLI as an external process over its `--experimental-json` protocol,
the same interface a human or script would use from a shell. `Thread`
(`src/agents/extensions/experimental/codex/thread.py:1-215`) wraps this
subprocess and captures a `ThreadStartedEvent`
(`src/agents/extensions/experimental/codex/events.py:14-15`,
`class ThreadStartedEvent(_DictLike): thread_id: str`) on first run
(`src/agents/extensions/experimental/codex/thread.py:155-158`), then uses
that same `thread_id` string on subsequent calls to `resume` that Codex CLI
thread, per the `["resume", args.thread_id]` argument above.

The item vocabulary this extension deals with --
`CommandExecutionItem`, `FileChangeItem`, `McpToolCallItem`,
`AgentMessageItem`, `ReasoningItem`
(`src/agents/extensions/experimental/codex/items.py`, lines 1-80 read) -- is
**structurally distinct** from `TResponseInputItem`/`ResponseInputItemParam`
used by the `Session` protocol; there is no shared type between the two
vocabularies. `codex_tool.py`
(`src/agents/extensions/experimental/codex/codex_tool.py`, lines 1-80 and
grep of `thread_id` usage) exposes this whole thing as a regular Agents-SDK
tool: `CodexToolResult(thread_id=...)` is returned as an ordinary function
tool output, which the calling agent's own `Session` then persists as a
normal opaque tool-call-output item -- the `thread_id` string is the *only*
thing that crosses from Codex's world into the Agents SDK's session, and it
crosses as an opaque string value inside an ordinary tool-output item, not
as a shared record format.

**Conclusion, directly stated:** there is **no shared code, no shared
on-disk/wire format, and no store-level interop path** between the Agents
SDK's `Session` store and Codex CLI's JSONL rollout store. The Agents SDK
never reads or writes a Codex rollout file, and Codex CLI has no awareness
of the Agents SDK's `Session` protocol. The only connection is a
process-boundary integration: the experimental `codex` extension shells out
to the `codex` binary and tracks its `thread_id`, entirely at the CLI/RPC
level, never at the storage layer. This is a positive finding checked by
direct code inspection (full read of the six files in
`src/agents/extensions/experimental/codex/` plus a repo-wide grep), not an
assumption.

No other foreign-store interop (e.g. reading LangChain, LlamaIndex, or any
other framework's session format) was found anywhere in `src/agents/`.

## What this implies for our Session Store (our inference)

**[inference]** In this SDK, "a stored session" is whatever a chosen
`Session` backend's `get_items`/`add_items`/`pop_item`/`clear_session`
implementation happens to persist -- the protocol defines an operational
contract, not a data model, and every shipped backend implements that
contract as a **mutable row-set or document**, not an append-only log. Two
operations that would be natural append-only-log candidates -- rewind and
compaction -- are both implemented destructively (row deletion via
`pop_item`, full clear-and-replace via `run_compaction`) in every backend
examined. There is no built-in backend in this SDK that is "close to an
append-only log with derived projections" the way this corpus's Codex CLI
dossier found Codex CLI's rollout+state-db design to be
(see [`../codex-cli/index.md`](../codex-cli/index.md)) -- the two products,
from the same vendor, sit at opposite ends of the append-only-vs-mutable
spectrum this research program cares about. For our own event-sourced
Session Store, the useful takeaways from this SDK are less about its
storage physics (which we should not imitate -- destructive rewind and
destructive compaction directly conflict with an event-sourced design) and
more about two narrower ideas worth stealing on their own merits: (1) a
narrow, uniform four-method store contract that many physically different
backends can implement without leaking backend-specific concerns into the
Runner, and (2) explicit decoupling of "what the model sees this turn"
(`session_input_callback`, `SessionSettings.limit`) from "what the store
holds," which is a useful separation of concerns regardless of whether the
store itself is a log or a document.

## Open questions

- `docs/ref/realtime/session.md` and `docs/ref/responses_websocket_session.md`
  exist in the tree (2 and 3 lines respectively per a `wc -l` check) but were
  **not read** in this pass -- it is not verified whether either describes a
  session-adjacent concept relevant to this dossier (e.g. a realtime/
  websocket-specific session notion distinct from the `Session` protocol
  covered here). Flagged rather than assumed to be irrelevant.
- The `hosted_multi_agent` experimental extension's `max_concurrent_subagents`
  field (`src/agents/extensions/experimental/hosted_multi_agent/model.py`) was
  only grepped, not read in full -- whether this experimental extension has
  any deeper session-durability implication beyond a concurrency-limit
  config field is not verified.
- `RedisSession`, `MongoDBSession`, and `DaprSession` were characterized from
  their headers, the `docs/sessions/index.md` narrative, and partial reads,
  not full line-by-line reads the way `sqlite_session.py`,
  `sqlalchemy_session.py`, `openai_conversations_session.py`,
  `openai_responses_compaction_session.py`, `encrypt_session.py`, and the
  `codex` extension package were. Their exact retry/backoff semantics under
  contention (beyond what `docs/sessions/index.md` states narratively) are
  not independently confirmed from source in this dossier.
- No schema-version field was found in the item format or any backend's
  schema (see **Entry/message structure and versioning**), but whether the
  `openai` package's own `ResponseInputItemParam` type carries any
  version-compat handling internally was out of scope for this checkout and
  was not investigated.
- Whether any first-party tooling outside this SDK (e.g. a companion CLI or
  admin tool) provides session listing/search across backends was not
  investigated -- the finding above is limited to what `src/agents/` itself
  exposes.
- `examples/memory/file_session.py` and `examples/memory/file_hitl_example.py`
  were read only partially (first ~40-50 lines each); they are example code,
  not shipped backends, and were used only to confirm the `RunState`
  checkpoint pattern and to note the existence of a mutable-JSON-document
  example backend -- not analyzed exhaustively.

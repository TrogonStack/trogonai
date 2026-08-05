# Google ADK (Agent Development Kit, Python): how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Apache-2.0. Source:
`google/adk-python`, pinned at commit `cbedafd9e4c18d462dc571e1bb079177a496ef51`. All
`path:line` citations below are repo-root-relative to that clone (e.g.
`src/google/adk/sessions/session.py:39`), not to this docs repo.

- `src/google/adk/sessions/session.py` (the `Session` pydantic model)
- `src/google/adk/sessions/base_session_service.py` (the pluggable contract,
  `BaseSessionService`)
- `src/google/adk/sessions/in_memory_session_service.py`,
  `src/google/adk/sessions/database_session_service.py`,
  `src/google/adk/sessions/sqlite_session_service.py`,
  `src/google/adk/sessions/vertex_ai_session_service.py` (the four shipped
  backends)
- `src/google/adk/sessions/schemas/v0.py`, `src/google/adk/sessions/schemas/v1.py`,
  `src/google/adk/sessions/schemas/shared.py` (SQL schema generations)
- `src/google/adk/sessions/migration/` (schema migration tooling and its README)
- `src/google/adk/sessions/state.py`, `src/google/adk/sessions/_session_util.py`
  (state scoping)
- `src/google/adk/events/event.py`, `src/google/adk/events/event_actions.py`,
  `src/google/adk/events/_rewind_events.py` (the entry type and rewind/compaction
  markers)
- `src/google/adk/apps/compaction.py` (compaction policy, operates on the
  session's event list, calls back into `BaseSessionService.append_event`)
- `src/google/adk/runners.py` (the only caller of `rewind_before_invocation_id`)
- `src/google/adk/tools/agent_tool.py`, `src/google/adk/agents/context.py`
  (the two distinct subagent storage models)

ADK is a framework with a pluggable store abstraction, not a single
application, so the interface itself is the primary finding. Where a
conclusion here would differ from an accepted record in this repo's own ADR
index, note that only `BaseSessionService` and its four shipped
implementations were examined; no ADK server/UI code (e.g. `adk web`) was
read beyond what these files import.

## The storage model

The durable session is a **mutable, four-part relational/document record**,
not an append-only log at the storage layer -- although the runtime always
mutates it by appending, never rewriting. The `Session` pydantic model is the
literal contract:

```python
class Session(BaseModel):
  id: str
  app_name: str
  user_id: str
  state: dict[str, Any] = Field(default_factory=dict)
  events: list[Event] = Field(default_factory=list)
  last_update_time: float = 0.0
  _storage_update_marker: str | None = PrivateAttr(default=None)
```

(`src/google/adk/sessions/session.py:28-73`, field names verbatim). `state` is
the *merged* view returned to callers (app + user + session scopes folded
together, see "Entry/message structure and versioning" below); `events` is the
ordered transcript; `last_update_time` and the private
`_storage_update_marker` are concurrency-control fields, not domain data
(`src/google/adk/sessions/session.py:64-73`).

What is authoritative differs **by backend**, which is the core divergence
this dossier documents:

- **`InMemorySessionService`**: a session is a `Session` object living in a
  nested dict `dict[app_name][user_id][session_id] -> Session`
  (`src/google/adk/sessions/in_memory_session_service.py:71`). There is no
  external representation at all; the object graph *is* the store. Explicitly
  documented as unsuitable for production
  (`src/google/adk/sessions/in_memory_session_service.py:64-66`: "It is not
  suitable for multi-threaded production environments. Use it for testing and
  development only.").
- **`DatabaseSessionService`** (SQLAlchemy, any dialect): four/five tables --
  `sessions`, `events`, `app_states`, `user_states`, `adk_internal_metadata`
  (`src/google/adk/sessions/schemas/v1.py:55-288`). The session row and the
  event rows are both authoritative; `events` rows are individually inserted
  (append), while the `sessions.state` column is periodically **overwritten in
  place** by an UPDATE (`src/google/adk/sessions/database_session_service.py:931-932`,
  `.../932: storage_session.state.update(...)`).
- **`SqliteSessionService`** (hand-rolled `aiosqlite`, distinct from
  `DatabaseSessionService`): same four-table shape, created via raw SQL
  strings (`src/google/adk/sessions/sqlite_session_service.py:48-96`), with
  state columns updated via SQLite's `json_patch()` function
  (`src/google/adk/sessions/sqlite_session_service.py:552-596`), which is an
  atomic partial-document merge rather than a full overwrite.
- **`VertexAiSessionService`**: the Vertex AI Agent Engine Sessions API is the
  store; ADK holds no local copy. The `raw_event` field
  (`src/google/adk/sessions/vertex_ai_session_service.py:463-467`) is ADK's own
  full-fidelity envelope smuggled into the remote API's `event_metadata`/
  `custom_metadata` extension points, so the "real" record ADK relies on for
  fidelity is itself a derived/optional field of the remote schema, with a
  documented fallback path when the SDK rejects it
  (`src/google/adk/sessions/vertex_ai_session_service.py:488-494`).

State and events are **not the same kind of record**. `events` is
append-typed (new `StorageEvent`/`Event` rows are only ever inserted, never
edited -- see "Write and append path"). `state` (app, user, and session scopes)
is a **mutable folded document**, updated via full-value overwrite
(SQLAlchemy backends) or `json_patch` (SQLite backend) every time a
`state_delta` lands. There is no way to recover a prior `state` value once
overwritten except by replaying the `state_delta`s recorded on past `events`
rows -- which makes `events` the only true append-only source of truth, and
`state` a **rebuildable-in-principle but not-actually-rebuilt** projection: no
backend reconstructs `state` from `events` on read; it always reads the
folded column/attribute directly.

Conceptual model: **session-as-document (state) plus session-as-log
(events), both under one `Session` id**, with the log being the only part
that is genuinely append-only across all four backends.

## Keying and identity

A session's key is the triple `(app_name, user_id, session_id)`. This triple
is the primary key of the `sessions` table in both SQL backends
(`src/google/adk/sessions/schemas/v1.py:75-85`,
`src/google/adk/sessions/sqlite_session_service.py:66-76`) and the three
nesting levels of the in-memory dict
(`src/google/adk/sessions/in_memory_session_service.py:71`). There is no
single opaque "session id" that stands alone; `app_name` and `user_id` are
load-bearing parts of identity, not decoration.

`session_id` minting: **client-supplied or server-generated UUID4**, never
ordering-encoding. `create_session`'s `session_id` parameter is optional
(`src/google/adk/sessions/base_session_service.py:60-80`); when absent, all
three local backends call `platform_uuid.new_uuid()`
(`src/google/adk/sessions/in_memory_session_service.py:135`,
`src/google/adk/sessions/database_session_service.py:590`,
`src/google/adk/sessions/sqlite_session_service.py:188`), which by default
returns `str(uuid.uuid4())` (`src/google/adk/platform/uuid.py:24`) -- a
context-var-overridable provider, but the shipped default is a random UUID4
with no timestamp or lexical ordering. `VertexAiSessionService` instead lets
the remote Agent Engine API assign the id and extracts it from the response
resource name (`src/google/adk/sessions/vertex_ai_session_service.py:208-209`).
A user-supplied `session_id` is validated as URL-path-safe
(`^[A-Za-z0-9_-]+$`) only in the Vertex backend
(`src/google/adk/sessions/vertex_ai_session_service.py:51,77-85`); the other
backends accept any non-empty string (after `.strip()`).

Listing is scoped by `(app_name, user_id)`, with `user_id` optional to widen
to all users of that app (`src/google/adk/sessions/base_session_service.py:94-96`:
"Lists all the sessions for a user... If not provided, lists all sessions for
all users."). There is no cross-`app_name` enumeration in any backend; every
`list_sessions` implementation filters on `app_name` first
(`src/google/adk/sessions/database_session_service.py:745-749`,
`src/google/adk/sessions/sqlite_session_service.py:326-337`).

Relocation/rename: **not a concept in ADK.** There is no workspace/cwd
component in the key, and no rename or move operation exists in
`BaseSessionService`. The Vertex backend supports one identity-adjacent
operation: a full resource name (`reasoningEngines/.../sessions/<id>`) can be
passed in place of a bare id and is normalized down to the short id, with the
reasoning-engine segment checked for a mismatch
(`src/google/adk/sessions/vertex_ai_session_service.py:54-74`).

## The store interface

ADK exposes a genuinely pluggable abstract base class,
`google.adk.sessions.base_session_service.BaseSessionService`
(`src/google/adk/sessions/base_session_service.py:54-210`). This is captured
**verbatim** below, method by method, because per the research brief this is
the centerpiece of the dossier.

### Abstract methods (every backend must implement)

```python
@abc.abstractmethod
async def create_session(
    self,
    *,
    app_name: str,
    user_id: str,
    state: Optional[dict[str, Any]] = None,
    session_id: Optional[str] = None,
) -> Session:
  """Creates a new session."""

@abc.abstractmethod
async def get_session(
    self,
    *,
    app_name: str,
    user_id: str,
    session_id: str,
    config: Optional[GetSessionConfig] = None,
) -> Optional[Session]:
  """Gets a session."""

@abc.abstractmethod
async def list_sessions(
    self, *, app_name: str, user_id: Optional[str] = None
) -> ListSessionsResponse:
  """Lists all the sessions for a user."""

@abc.abstractmethod
async def delete_session(
    self, *, app_name: str, user_id: str, session_id: str
) -> None:
  """Deletes a session."""
```

(`src/google/adk/sessions/base_session_service.py:60-112`, signatures and
docstrings verbatim.)

### Concrete (non-abstract) methods, overridable but shipped with a default

```python
async def get_user_state(
    self, *, app_name: str, user_id: str
) -> dict[str, Any]:
  """... Raises NotImplementedError when the concrete BaseSessionService
  implementation does not support reading user state independently of a
  session. ..."""
  raise NotImplementedError(...)

async def append_event(self, session: Session, event: Event) -> Event:
  """Appends an event to a session object."""
  if event.partial:
    return event
  self._apply_temp_state(session, event)
  event = self._trim_temp_delta_state(event)
  self._update_session_state(session, event)
  session.events.append(event)
  return event

async def flush(self) -> None:
  """Flushes any buffered events.
  For non-buffering implementations, this can be a no-op."""
  pass
```

(`src/google/adk/sessions/base_session_service.py:114-172`, bodies verbatim.)
`get_user_state` is optional and explicitly may raise `NotImplementedError`;
callers are told to fall back to `list_sessions` + `get_session` per-session
merge (`src/google/adk/sessions/base_session_service.py:141-152`).
`append_event`'s **default body only mutates the in-memory `Session` object
passed in -- it does not persist anything.** Every concrete backend overrides
`append_event` to add the actual write path, then (with the sole exception of
`VertexAiSessionService`, see below) still calls
`super().append_event(...)` to keep the caller's in-memory object consistent
(`src/google/adk/sessions/database_session_service.py:956-957`,
`src/google/adk/sessions/sqlite_session_service.py:487-488`,
`src/google/adk/sessions/vertex_ai_session_service.py:392-393`). `flush()` has
no overrides anywhere in the sessions package -- grep confirms it is defined
exactly once, in the base class
(`src/google/adk/sessions/base_session_service.py:167`) -- so it is a
guaranteed no-op today; no shipped backend buffers writes.

### Supporting types (verbatim)

```python
class GetSessionConfig(BaseModel):
  """The configuration of getting a session.
  Attributes:
    num_recent_events: ... if None, the filter is not applied; if greater
      than 0, returns at most given number of recent events; if 0, no
      events are returned.
    after_timestamp: ... if None, the filter is not applied; otherwise,
      returns events with timestamp >= the given time.
  """
  num_recent_events: Optional[int] = None
  after_timestamp: Optional[float] = None

class ListSessionsResponse(BaseModel):
  """The response of listing sessions.
  The events and states are not set within each Session object."""
  sessions: list[Session] = Field(default_factory=list)
```

(`src/google/adk/sessions/base_session_service.py:29-51`.) Note the
`ListSessionsResponse` docstring's claim that "states are not set" is
**inaccurate for three of the four backends**: `InMemorySessionService`,
`DatabaseSessionService`, and `SqliteSessionService` all call their internal
`_merge_state`/merge helper inside `list_sessions` and populate `state` on
every returned `Session`, clearing only `events`
(`src/google/adk/sessions/in_memory_session_service.py:274-283`,
`src/google/adk/sessions/database_session_service.py:784-794`,
`src/google/adk/sessions/sqlite_session_service.py:357-371`). Only
`VertexAiSessionService.list_sessions` matches the docstring's stated
"no events" behavior for `state` in that it can leave `state` empty when the
API returns none, but even there `state` is populated when present
(`src/google/adk/sessions/vertex_ai_session_service.py:322-329`). This is a
doc/implementation mismatch in the upstream project, not a store-model
divergence, but it matters for any caller relying on the docstring for a
"list is metadata-only" contract.

### Operation contract summary

| Operation | Abstract? | Inputs | When invoked |
| --- | --- | --- | --- |
| `create_session` | yes | `app_name, user_id, state?, session_id?` | Runner-side "get or create" at invocation start; direct API/CLI calls. |
| `get_session` | yes | `app_name, user_id, session_id, config?` | Resume; every `Runner.run_async` invocation reloads or receives a session. |
| `list_sessions` | yes | `app_name, user_id?` | Session pickers, `adk web`/CLI listing. |
| `delete_session` | yes | `app_name, user_id, session_id` | Explicit retirement only; no automatic caller found in this package. |
| `get_user_state` | no (may raise `NotImplementedError`) | `app_name, user_id` | Optional fast-path read of user-scoped state without loading a session. |
| `append_event` | no (base is in-memory-only; every backend overrides) | `session: Session, event: Event` | Every turn/tool-call/compaction/rewind event; the sole write path onto a session's transcript and state. |
| `flush` | no (no-op default, unoverridden) | none | Declared for buffering implementations; no such implementation ships. |

## Write and append path

- **Append, not rewrite, for events; overwrite-or-patch for state.** Every
  backend's `append_event` inserts one new event row/object and separately
  mutates the state document (see "The storage model"). No backend rewrites
  or removes prior event rows during append.
- **Ordering.** There is no explicit monotonic sequence number field on
  `Event` -- ordering is positional (`events.append(event)` /
  insertion order) for `InMemorySessionService`
  (`src/google/adk/sessions/in_memory_session_service.py:352,164`), and
  **timestamp, with an id tiebreak**, for both SQL-backed services on read:
  `.order_by(schema.StorageEvent.timestamp.desc(), schema.StorageEvent.id.desc())`
  (`src/google/adk/sessions/database_session_service.py:697-699`) and the
  equivalent `ORDER BY timestamp DESC, id DESC`
  (`src/google/adk/sessions/sqlite_session_service.py:285`). The database
  comment explains why the tiebreak exists: "Without it the database is free
  to return tied events in a different order on every read, so a replayed
  conversation shuffles and `num_recent_events` truncates at an arbitrary
  point in the tie" (`src/google/adk/sessions/database_session_service.py:693-696`).
  `Event.timestamp` is a client-generated `float` via `platform_time.get_time()`
  at construction (`src/google/adk/events/event.py:155`), not a
  server-assigned monotonic counter, so **clock skew across writers can
  reorder events** unless ids happen to tiebreak consistently (ids are UUID4,
  so the tiebreak is arbitrary-but-stable, not causally meaningful).
- **Durability/atomicity.** `DatabaseSessionService` wraps each append in one
  SQLAlchemy transaction (`sql_session.commit()` at
  `src/google/adk/sessions/database_session_service.py:951`) with guaranteed
  rollback on any exception via an `asynccontextmanager`
  (`src/google/adk/sessions/database_session_service.py:411-429`).
  `SqliteSessionService` similarly batches the state upsert(s) and the event
  insert into one connection before a single `db.commit()`
  (`src/google/adk/sessions/sqlite_session_service.py:456-482`). Neither does
  torn-write detection or repair; durability is delegated entirely to the
  underlying database engine's transaction guarantees.
- **Concurrency model differs sharply by backend -- this is the most
  consequential divergence in this dossier:**
  - `InMemorySessionService`: no locking at all. Concurrent `append_event`
    calls on the same process race on plain Python dict/list mutation;
    the class docstring says outright it "is not suitable for
    multi-threaded production environments"
    (`src/google/adk/sessions/in_memory_session_service.py:64-66`).
  - `DatabaseSessionService`: **two independent concurrency layers.** First,
    an in-process `asyncio.Lock` per `(app_name, user_id, session_id)` key
    serializes concurrent `append_event` calls **within one process**
    (`src/google/adk/sessions/database_session_service.py:446-476`,
    `_with_session_lock`). Second, and only for MySQL/MariaDB/PostgreSQL
    dialects, `SELECT ... FOR UPDATE` row-level locking is used on the
    session row and (conditionally) on the app/user state rows
    (`src/google/adk/sessions/database_session_service.py:431-436,865-866,881,895`;
    `_supports_row_level_locking` explicitly excludes SQLite). On top of
    that, an **optimistic-concurrency staleness check** compares a
    `_storage_update_marker` (a microsecond-precision ISO timestamp string,
    `get_update_marker()`, `src/google/adk/sessions/schemas/v1.py:141-146`)
    captured at load time against the current DB row, raising
    `ValueError(_STALE_SESSION_ERROR_MESSAGE)` on mismatch
    (`src/google/adk/sessions/database_session_service.py:904-924`,
    message at `:75-78`: "The session has been modified in storage since it
    was loaded. Please reload the session before appending more events.").
    A marker-less (e.g. manually constructed) `Session` falls back to a
    timestamp comparison plus a query for whether the DB's latest event id
    still matches the in-memory tail
    (`_session_matches_storage_revision`,
    `src/google/adk/sessions/database_session_service.py:540-571`).
  - `SqliteSessionService`: **no per-process lock**, and a **cruder
    staleness check** than `DatabaseSessionService` -- it compares only
    `storage_update_time > session.last_update_time` and raises a bare
    `ValueError` (not a typed exception) if the session looks stale
    (`src/google/adk/sessions/sqlite_session_service.py:405-420`). Its state
    upserts use SQLite's `json_patch()` inside `ON CONFLICT ... DO UPDATE`,
    which is atomic per-statement but has no equivalent to
    `DatabaseSessionService`'s row-level lock or savepoint-guarded
    concurrent-insert handling.
  - `VertexAiSessionService`: concurrency is delegated entirely to the
    remote Agent Engine API; ADK adds no local locking, no staleness check,
    and no expected-version precondition on its own `append_event` call
    (`src/google/adk/sessions/vertex_ai_session_service.py:390-495`).
  So: **optimistic concurrency with an expected-version precondition exists
  only in `DatabaseSessionService`**, and is materially weaker in
  `SqliteSessionService`, and **entirely absent** in the in-memory and Vertex
  backends. A store abstraction that is "the same interface" across these
  four backends hides a real behavioral cliff on concurrent append.
- **Delivery semantics.** No backend implements retry, idempotence keying, or
  at-least-once redelivery around `append_event` itself; a raised exception
  (stale-session `ValueError`, `SessionNotFoundError`, a DB error) propagates
  to the caller with no automatic resend. `Event.id` (UUID4, assigned in
  `Event.model_post_init`, `src/google/adk/events/event.py:282-286`) is a
  primary-key column in both SQL schemas
  (`src/google/adk/sessions/schemas/v1.py:180-182`), so a client-side retry
  that reuses the same `Event` object would collide on the id rather than
  duplicate -- an accidental dedup property, not a designed idempotence
  mechanism.
- **`partial` events are never persisted.** Every backend's `append_event`
  (and the base class's) checks `if event.partial: return event` and returns
  without writing (`src/google/adk/sessions/base_session_service.py:156-157`,
  and the same guard re-implemented in each backend, e.g.
  `src/google/adk/sessions/in_memory_session_service.py:323-324`,
  `src/google/adk/sessions/sqlite_session_service.py:394-395`). Streaming/
  partial model output is therefore transient-only in the interface's own
  terms, never a durability concern.

## Read and resume path

- **Resume is a full ordered read of the durable store on every
  `get_session` call**, not a cached/local-first read -- there is no
  filesystem or process-local cache layer anywhere in this package; the
  in-memory backend's "cache" is simply the entire store.
- **Pagination/bounding**: `GetSessionConfig.num_recent_events` and
  `after_timestamp` are the only bounding controls
  (`src/google/adk/sessions/base_session_service.py:29-43`). `num_recent_events`
  is a `LIMIT`-style tail bound (SQL backends push it into the query, e.g.
  `stmt.limit(config.num_recent_events)`,
  `src/google/adk/sessions/database_session_service.py:701-702`); passing `0`
  is special-cased to **skip the events query entirely**
  (`src/google/adk/sessions/database_session_service.py:678-680`: "Existence/
  metadata-only read; skip the events query entirely."). There is no
  cursor/offset pagination -- a caller cannot page through a transcript in
  chunks; it is "all events", "last N", or "events after timestamp T".
- **Eager materialization.** Every backend returns the full requested event
  list and the full merged state in one call; nothing is lazily loaded within
  a `Session` object once returned. `InMemorySessionService` slices the
  Python list in-process (`src/google/adk/sessions/in_memory_session_service.py:204-219`);
  the SQL backends run one query for events and separate `sql_session.get`
  calls (single-row primary-key lookups, not table scans) for app/user state
  (`src/google/adk/sessions/database_session_service.py:709-715`,
  `src/google/adk/sessions/sqlite_session_service.py:297-299`).
- **`VertexAiSessionService.get_session`** fetches the session resource and
  the event list **in parallel** via `asyncio.gather`
  (`src/google/adk/sessions/vertex_ai_session_service.py:257-263`), and
  deliberately does not try to reconcile clock skew between the two calls --
  the comment explains why: "Preserve the entire event stream that Vertex
  returns rather than trying to discard events written milliseconds after
  the session resource was updated. Clock skew between those writes can
  otherwise drop tool_result events and permanently break the replayed
  conversation." (`src/google/adk/sessions/vertex_ai_session_service.py:285-288`).
- **State merge on read** (all three local backends): session-scoped state is
  read from its own column/attribute, then app-scoped and user-scoped state
  are merged in under `app:`/`user:` prefixes
  (`src/google/adk/sessions/database_session_service.py:245-256`, `_merge_state`;
  identical logic re-implemented in
  `src/google/adk/sessions/sqlite_session_service.py:630-641` and
  `src/google/adk/sessions/in_memory_session_service.py:224-246`). This merge
  happens on every read, not once at write time -- there is no denormalized
  "final state" cached anywhere.

## Listing, summaries, and search

- **Listing is a direct table/dict scan filtered by `(app_name, user_id?)`**,
  not an index or search subsystem. `DatabaseSessionService.list_sessions`
  issues one `SELECT` on `sessions` filtered by `app_name` (and `user_id` if
  given), a state lookup, and a full result-set iteration to build responses
  (`src/google/adk/sessions/database_session_service.py:737-795`).
  `SqliteSessionService` is equivalent
  (`src/google/adk/sessions/sqlite_session_service.py:320-372`). No stated
  cost numbers or scale limits appear anywhere in this package -- no
  pagination parameters exist on `list_sessions` at all
  (`src/google/adk/sessions/base_session_service.py:93-96`), so cost is
  whatever a full per-app (optionally per-user) table scan costs on the
  underlying engine.
- **No metadata sidecar.** There is no separate summary/preview record
  maintained at write time in any backend; `list_sessions` derives its
  response by reading the same `sessions`/`app_states`/`user_states` rows
  used elsewhere, with `events` explicitly zeroed
  (`src/google/adk/sessions/in_memory_session_service.py:274-277`,
  `.../281-283`). See "The store interface" above for the docstring/
  implementation mismatch on whether `state` is included.
- **No search subsystem of any kind was found** -- no FTS, no vector index, no
  content index. Session content is only reachable through
  `get_session`/`list_sessions`'s exact-key or `app_name`/`user_id`-scoped
  reads.

## Entry/message structure and versioning

The unit of the durable log is `Event` (`src/google/adk/events/event.py:91`),
a `pydantic.BaseModel` subclass of `LlmResponse`
(`src/google/adk/events/event.py:29,91`), with `extra='ignore'` and a
camelCase alias generator for wire compatibility
(`src/google/adk/events/event.py:98-104`). Fields relevant to session storage
(verbatim names, `src/google/adk/events/event.py:107-155`):

```python
class Event(LlmResponse):
  invocation_id: str = ''
  author: str = ''
  actions: EventActions = Field(default_factory=EventActions)
  output: Any | None = None
  node_info: NodeInfo = Field(default_factory=NodeInfo)
  long_running_tool_ids: set[str] | None = None
  branch: str | None = None
  isolation_scope: str | None = None
  id: str = ''
  timestamp: float = Field(default_factory=lambda: platform_time.get_time())
```

`id` is never client-assigned before append -- it self-generates in
`model_post_init` via `Event.new_id()` (`str(uuid.uuid4())` through the
platform indirection) if empty
(`src/google/adk/events/event.py:282-286,315-317`). `branch` is the
dot-separated ancestor chain used to hide sibling sub-agent conversations from
each other (`src/google/adk/events/event.py:127-135`: "Branch is used when
multiple sub-agent shouldn't see their peer agents' conversation history.");
its hierarchical grammar (`name@run_id` segments) is implemented in
`_BranchPath` (`src/google/adk/events/_branch_path.py:20-151`). `isolation_scope`
is a second, narrower filter explicitly marked internal-and-unstable ("DO NOT
USE THIS FIELD DIRECTLY... may change without notice",
`src/google/adk/events/event.py:146-149`), currently used to scope Task-API
delegate agents to only their own function-call id's events.

`EventActions` (`src/google/adk/events/event_actions.py:78-202`) is the
mutation/annotation envelope carried by every event -- this is what makes an
`Event` more than a transcript line:

```python
class EventActions(BaseModel):
  skip_summarization: Optional[bool] = None
  state_delta: dict[str, Any] = Field(default_factory=dict)
  artifact_delta: dict[str, int] = Field(default_factory=dict)
  transfer_to_agent: Optional[str] = None
  escalate: Optional[bool] = None
  requested_auth_configs: dict[str, AuthConfig] = Field(default_factory=dict)
  requested_tool_confirmations: dict[str, ToolConfirmation] = Field(default_factory=dict)
  compaction: Optional[EventCompaction] = None
  end_of_agent: Optional[bool] = None
  agent_state: Optional[dict[str, Any]] = None
  rewind_before_invocation_id: Optional[str] = None
  route: Optional[Union[bool, int, str, list[Union[bool, int, str]]]] = None
  render_ui_widgets: Optional[list[UiWidget]] = None
  set_model_response: Optional[Any] = None
```

(`src/google/adk/events/event_actions.py:88-202`, field names/types verbatim.)
`state_delta` is the only field the store interface itself interprets (see
next paragraph); every other field is opaque to `BaseSessionService` and
passed through verbatim. `compaction` and `rewind_before_invocation_id` are
markers interpreted at replay time by application code, not by the store --
see "Compaction and history management" and "Rewind, checkpoints, and fork".

**State scoping is prefix-based and interpreted by the store, not opaque.**
`State` (`src/google/adk/sessions/state.py:61-136`) declares three prefixes:

```python
class State:
  APP_PREFIX = "app:"
  USER_PREFIX = "user:"
  TEMP_PREFIX = "temp:"
```

(`src/google/adk/sessions/state.py:64-66`.) `_session_util.extract_state_delta`
buckets a flat `state_delta` dict into `{"app": ..., "user": ..., "session":
...}` by stripping these prefixes, and **silently drops `temp:`-prefixed
keys** from the persisted buckets (`src/google/adk/sessions/_session_util.py:41-58`:
the loop only routes into `app`, `user`, or the `else` `session` bucket for
non-`temp:` keys). This is confirmed at the call site:
`BaseSessionService.append_event` applies `temp:` deltas to the in-memory
`Session.state` *before* trimming them from the event
(`_apply_temp_state`, `src/google/adk/sessions/base_session_service.py:174-186`),
then strips them from the event before it is persisted
(`_trim_temp_delta_state`, `src/google/adk/sessions/base_session_service.py:187-202`,
docstring: "This prevents temp-scoped state from being persisted, while the
in-memory session state... retains the values for the duration of the current
invocation.").

So, durability by scope:

| Scope | Persisted? | Shared across sessions? | Storage location |
| --- | --- | --- | --- |
| unprefixed (session) | yes | no -- one session only | `sessions.state` column/attribute |
| `app:` | yes | yes -- all sessions of that `app_name` | `app_states` table/dict, keyed by `app_name` alone |
| `user:` | yes | yes -- all sessions of that `(app_name, user_id)` | `user_states` table/dict, keyed by `(app_name, user_id)` |
| `temp:` | **no** | no -- process/invocation-local only | never leaves the in-memory `Session.state`/event; explicitly trimmed before persistence |

`get_user_state` (`src/google/adk/sessions/base_session_service.py:114-152`)
exists specifically so a caller can read `user:`-scoped state **without**
first loading any session, described as avoiding "an expensive
`list_sessions` call just to access user-scoped data"
(`src/google/adk/sessions/base_session_service.py:125-128`) -- a direct
acknowledgment that `list_sessions` is the fallback path for reading shared
state, and that fallback is a full scan (see "Listing" above).

**Versioning and evolution.** Two independent version axes exist:

1. **SQL schema version**, tracked in the `adk_internal_metadata` table
   (`src/google/adk/sessions/schemas/v1.py:55-67`, `StorageMetadata`).
   `_schema_check_utils.get_db_schema_version_from_connection` inspects the
   live database: if `adk_internal_metadata` exists, it trusts that row; if
   not, it sniffs the `events` table's columns -- presence of an `actions`
   column with no `event_data` column means the legacy v0 (pickle) schema
   (`src/google/adk/sessions/migration/_schema_check_utils.py:70-89`). This is
   detect-then-branch, not a hard version gate: `DatabaseSessionService`
   keeps parallel v0/v1 SQLAlchemy model classes
   (`_SchemaClasses`, `src/google/adk/sessions/database_session_service.py:259-276`)
   and every read/write path branches on `self._db_schema_version` to select
   which model class to bind, so **v0 databases keep working without
   migration**, at reduced fidelity (v0 used Python `pickle` for
   `EventActions`, v1 uses JSON -- `src/google/adk/sessions/schemas/v0.py:14-24`).
2. **Wire schema of `Event`/`Session` themselves**, which is bare pydantic
   `model_dump`/`model_validate` with `exclude_none=True`
   (e.g. `src/google/adk/sessions/schemas/v1.py:233`,
   `src/google/adk/sessions/sqlite_session_service.py:468`) -- additive fields
   with defaults are the only evolution mechanism visible; there is no
   explicit per-`Event` schema-version field.

**Migration is a one-way, external, dump-and-reload ratchet**, not an
in-place `ALTER TABLE`. `migration_runner.upgrade(source_db_url,
dest_db_url, ...)` refuses in-place migration outright
(`src/google/adk/sessions/migration/migration_runner.py:76-80`: "In-place
migration is not supported... migrations always read from a source and write
to a destination.") and chains migration steps through **temporary SQLite
files** for multi-hop upgrades, even between two non-SQLite databases
(`src/google/adk/sessions/migration/migration_runner.py:110-118`, docstring at
`:56-58`). The only registered step today is v0 (pickle) → v1 (JSON)
(`MIGRATIONS = {SCHEMA_VERSION_0_PICKLE: (SCHEMA_VERSION_1_JSON,
migrate_from_sqlalchemy_pickle.migrate)}`,
`src/google/adk/sessions/migration/migration_runner.py:34-39`). That migration
uses a **restricted unpickler** with an explicit allowlist of ~35 safe
classes (`_ALLOWED_PICKLE_GLOBALS`,
`src/google/adk/sessions/migration/migrate_from_sqlalchemy_pickle.py:41-100`)
and only falls back to the unrestricted `pickle.loads` if the caller
explicitly opts in with `allow_unsafe_unpickling=True`
(`src/google/adk/sessions/migration/migrate_from_sqlalchemy_pickle.py:120-126`),
i.e. legacy pickle payloads are treated as untrusted input during migration. A
second, SQLite-specific script
(`src/google/adk/sessions/migration/migrate_from_sqlalchemy_sqlite.py`)
migrates a SQLAlchemy-backed SQLite database to `SqliteSessionService`'s
hand-rolled schema, a **third schema lineage** distinct from both
`schemas/v0.py` and `schemas/v1.py`. The migration process document
(`src/google/adk/sessions/migration/README.md`) formalizes the deprecation
policy: a new schema version must keep `DatabaseSessionService` "backward-
compatible with the previous schema for a few releases (at least 2)"
before the old branch is removed (`.../README.md:106-109,123-129`).

## Compaction and history management

Compaction is an **application/runtime concern that writes back through the
same `append_event` store call**, not a store-level operation -- the store has
no compaction API of its own. `src/google/adk/apps/compaction.py` implements
two policies, both of which produce an ordinary `Event` whose
`actions.compaction` is an `EventCompaction` marker
(`start_timestamp, end_timestamp, compacted_content`,
`src/google/adk/events/event_actions.py:58-75`) and then call
`session_service.append_event(session=session, event=compaction_event)`
(`src/google/adk/apps/compaction.py:421-422`) exactly like any other event.
The durable event list is **never shortened or rewritten** by compaction --
the compaction event is appended alongside the events it summarizes, and
"which events are still live" is a pure function computed at read time.

The two policies:

- **Token-threshold compaction** (`_run_compaction_for_token_threshold_config`,
  `src/google/adk/apps/compaction.py:372-426`): triggers once the latest
  observed/estimated prompt token count exceeds `config.token_threshold`,
  selects the oldest events beyond `event_retention_size`, and seeds the
  new summary with the **previous compaction's own summary content** so
  summaries chain (`src/google/adk/apps/compaction.py:275-289`).
- **Sliding-window compaction** (`_run_compaction_for_sliding_window`,
  `src/google/adk/apps/compaction.py:448-649`): triggers on an invocation
  count (`compaction_interval`), with an `overlap_size` of prior invocations
  re-included in each new summary for continuity -- documented with a worked
  4-invocation example in the function's own docstring
  (`src/google/adk/apps/compaction.py:489-527`).

Both policies route through `_longest_self_contained_prefix`
(`src/google/adk/apps/compaction.py:312-333`), which refuses to compact past
an "open" function-call/tool-confirmation/auth-request obligation that has no
matching response yet -- compaction cannot split a call from its response.

**Rewind interacts with compaction through one shared function,
`_apply_rewinds`** (`src/google/adk/events/_rewind_events.py:22-55`), which
both the LLM prompt-content builder and both compaction policies call before
doing anything else (`src/google/adk/apps/compaction.py:394,544`, with an
explicit comment at `:390-393` and `:540-543` that this must stay consistent
across both call sites "otherwise rewound content can leak back into prompts
through a compaction summary"). This is the resume/replay behavior that
crosses both a rewind boundary and a compaction boundary: **the durable event
list is never edited; every reader must independently fold both markers
before building a prompt or a summary.**

## Rewind, checkpoints, and fork

**Rewind is an appended marker, never a destructive edit** -- the single
strongest confirmation of the append-only-log character of `events` in this
codebase. `Runner.rewind_async`
(`src/google/adk/runners.py:1329-1378`) locates the target invocation's first
event by linear scan, computes a `state_delta` that reverses every state
change made since that point (`_compute_state_delta_for_rewind`,
`src/google/adk/runners.py:1380-1405`, walking forward and diffing against
current state, setting reversed keys to `None` to signal removal) and an
equivalent artifact delta, then constructs and appends one new event:

```python
rewind_event = Event(
    invocation_id=new_invocation_context_id(),
    author='user',
    actions=EventActions(
        rewind_before_invocation_id=rewind_before_invocation_id,
        state_delta=state_delta,
        artifact_delta=artifact_delta,
    ),
)
await self.session_service.append_event(session=session, event=rewind_event)
```

(`src/google/adk/runners.py:1366-1378`, verbatim). No event is deleted, no
`Event` is mutated in place -- `_apply_rewinds`
(`src/google/adk/events/_rewind_events.py:22-55`) is a pure backward-scan
projection that any reader must apply to compute "which events are live."
`rewind_before_invocation_id` on `EventActions` is documented as being "only
set for rewind event" (`src/google/adk/events/event_actions.py:192-193`).

**"Checkpoints" in ADK are not storage checkpoints but workflow-resume
state.** `EventActions.agent_state` is documented as "The agent state at the
current event, used for checkpoint and resume... should only be set by ADK
workflow" (`src/google/adk/events/event_actions.py:166-168`) -- this is
in-band workflow-node state riding on the ordinary event stream, not a
separate checkpoint artifact or file. No distinct checkpoint file/table
exists in the sessions package.

**Fork does not exist as a session-store operation.** No `fork`/`branch`/
`clone` verb appears on `BaseSessionService` or any backend. The closest
concept is the `branch` string field on `Event`
(`src/google/adk/events/event.py:127-135`) and `isolation_scope`
(`:136-149`), which are **filters over one shared event list**, not separate
storage: a sub-agent's events live in the *same* session's `events` array,
tagged with a longer `branch` path so readers can hide sibling agents'
history from each other. There is no copy-on-fork and no lineage record
beyond that string tag.

## Subagents and nested sessions

ADK ships **two structurally different subagent storage models**, and which
one a given sub-agent uses depends entirely on how it is attached -- this is
the divergence the research brief specifically asked to be documented.

**1. In-session branch scoping** (`sub_agents=[...]`, and single-turn
`AgentTool` usage via `tool_context.run_node`): the sub-agent's events are
appended to the **same** `Session.events` list as the parent, distinguished
only by a longer `branch` path built with
`_BranchPath.create_sub_branch(base_branch, name=self.agent.name,
run_id=fc_id)` (`src/google/adk/tools/agent_tool.py:385-390`, in
`_SingleTurnAgentTool.run_async`) and executed via
`tool_context.run_node(self.agent, node_input=node_input,
override_branch=tool_branch, use_sub_branch=False)`
(`src/google/adk/tools/agent_tool.py:392-398`). `run_node`
(`src/google/adk/agents/context.py:424-479`) documents this explicitly: "The
dynamically executed node becomes a child run of the current node in the
workflow" -- it is a child *run*, not a child *session*. There is no separate
`Session` object, no separate store row, and no parent-delete cascade concern
because there is nothing separate to cascade to: deleting the parent session
deletes these child events too, trivially, because they were never anything
but rows/entries in the parent's own event list. Nesting depth is bounded
only by however deep `branch` paths can recurse (dot-separated segments,
`src/google/adk/events/_branch_path.py:20-42`), i.e. not bounded by the store.

**2. Fully separate, throwaway session** (multi-turn `AgentTool` usage, ADK's
documented-but-discouraged direct wrapping path): `AgentTool.run_async`
constructs a **brand-new `Runner` with a brand-new
`InMemorySessionService()`** for every single tool call
(`src/google/adk/tools/agent_tool.py:225-271`, `runner = Runner(...,
session_service=InMemorySessionService(), ...)`), creates a fresh session in
it (`runner.session_service.create_session(...)`,
`src/google/adk/tools/agent_tool.py:284-288`), runs the sub-agent to
completion, forwards only `state_delta` back into the parent's
`tool_context.state` (`src/google/adk/tools/agent_tool.py:299-301`), and then
calls `await runner.close()` (`:310`). **The child session and its entire
event transcript are never persisted anywhere beyond that in-memory
`Runner`'s lifetime** -- they are discarded as soon as the tool call returns.
There is no durable parent-child link at all in this path: the parent
session's `events` gains only the tool-call/tool-result pair produced by the
*parent's* agent framework around the call, not the child's own transcript.
The `AgentTool` class docstring itself flags this path as discouraged in
favor of the single-turn/branch model: "Direct usage of `AgentTool` is
discouraged. See the single-turn mode guide for details."
(`src/google/adk/tools/agent_tool.py:125-126`).

So: on **parent delete**, model (1) cascades trivially (it was never
separate); model (2) has nothing to cascade because nothing was durable in
the first place. On **parent rewind**, model (1)'s sub-agent events are
subject to the same `rewind_before_invocation_id` fold as any other event in
the shared list; model (2) is entirely out of scope of rewind because its
events never entered the durable store. On **crash mid-subagent-call**,
model (1) leaves whatever partial events had already been appended to the
shared session (consistent with the per-event durability story above); model
(2) loses the entire child run, since it lived only in an in-memory service
that is never flushed to durable storage -- a crash there is indistinguishable
from a normal tool-call failure from the parent session's point of view.

## Retention, deletion, and multi-host

- **No TTL or scheduled cleanup lives in this package** for the in-memory,
  database, or SQLite backends -- `delete_session` is the only removal path,
  and nothing calls it automatically anywhere searched in
  `src/google/adk/sessions/` or `src/google/adk/runners.py`.
  `VertexAiSessionService.create_session` does accept caller-supplied `ttl`
  or `expire_time` keyword arguments passed straight through to the remote
  API's session-create config
  (`src/google/adk/sessions/vertex_ai_session_service.py:179-200`), which
  makes retention a remote-service feature for that one backend, not an ADK
  concern.
- **Delete behavior differs by backend in exactly the way the storage model
  predicts.** `DatabaseSessionService.delete_session` issues one `DELETE`
  against `sessions` filtered by the full key
  (`src/google/adk/sessions/database_session_service.py:797-810`); cascading
  removal of that session's `events` rows is enforced by the schema's
  `ForeignKeyConstraint(..., ondelete="CASCADE")`
  (`src/google/adk/sessions/schemas/v1.py:208-213`) plus the SQLAlchemy
  relationship's own `cascade="all, delete-orphan"`
  (`src/google/adk/sessions/schemas/v1.py:98-103`) -- belt-and-suspenders,
  ORM-level and DB-level cascade both present. `SqliteSessionService` relies
  on the same `ON DELETE CASCADE` declared in its raw schema SQL
  (`src/google/adk/sessions/sqlite_session_service.py:87-90`), enabled only
  because `PRAGMA foreign_keys = ON` is set on every connection
  (`src/google/adk/sessions/sqlite_session_service.py:46,498`; SQLite ignores
  `ON DELETE CASCADE` without this pragma). `InMemorySessionService.delete_session`
  is a plain dict `.pop()` (`src/google/adk/sessions/in_memory_session_service.py:313`).
  `VertexAiSessionService.delete_session` additionally **enforces ownership
  before deleting** by fetching the session first and comparing `user_id`,
  with a code comment explaining why: "Enforce ownership: delete_session
  otherwise ignores user_id entirely."
  (`src/google/adk/sessions/vertex_ai_session_service.py:346-359`) -- the
  remote API's delete-by-name call has no `user_id` parameter of its own, so
  ADK adds the check client-side.
- **App-scoped and user-scoped state are never deleted by
  `delete_session`.** Deleting one session removes only that session's row
  and its events; `app_states`/`user_states` rows persist and remain visible
  to any other session sharing that `app_name`/`(app_name, user_id)` -- an
  intentional consequence of the scoping model (see "Entry/message
  structure and versioning"), but a real orphan-accumulation risk with no
  visible cleanup path in this package.
- **Multi-host is not a first-class path for the local backends.**
  `InMemorySessionService` is explicitly single-process
  (`src/google/adk/sessions/in_memory_session_service.py:64-66`).
  `SqliteSessionService` opens a fresh `aiosqlite.connect()` per operation
  (`src/google/adk/sessions/sqlite_session_service.py:492-502`) with no
  cross-process lock beyond whatever SQLite's own file locking provides --
  no explicit multi-host handling is present. `DatabaseSessionService` is the
  one backend designed for shared, multi-writer access, via its
  connection-pooled `AsyncEngine`, per-dialect row-level locking, and
  optimistic-concurrency marker (see "Write and append path") -- this is
  effectively ADK's answer to multi-host, achieved by pushing the problem
  onto a real database rather than solving it in the session-store layer
  itself. `VertexAiSessionService` is multi-host by construction, since the
  Agent Engine service itself is the single shared backend.

## Interop with foreign session stores

None found. No code in `src/google/adk/sessions/` reads, discovers, imports,
or resumes another product's native session store. The `migration/`
directory's scripts migrate **ADK's own** prior schema generations
(SQLAlchemy-pickle-v0, SQLAlchemy-JSON-v1, and the separate hand-rolled
SQLite schema) into current ADK schemas -- this is intra-product schema
migration, not interop with a foreign product's session format.

## What this implies for our Session Store (our inference)

**Inference.** ADK's "stored session" is not one durable design but a
contract (`BaseSessionService`) that four backends satisfy with materially
different guarantees underneath -- closest to session-as-document (mutable
`state`, keyed by `(app_name, user_id, session_id)`) with an append-only
`events` list riding alongside it, rather than a pure event-sourced log with
state as a derived projection. State is written *directly*, not derived from
folding `events`; only `events` is genuinely append-only, and even that
guarantee is enforced by each backend independently rather than being a
property of the interface itself. The interface's true unifying value is
narrower than it looks: it guarantees the same four abstract *methods* exist
everywhere, but explicitly does **not** guarantee the same *concurrency*,
*durability*, or *subagent nesting* semantics across backends -- those vary
by backend in ways a caller must know about to write correct code (e.g. the
different `ValueError` vs typed-exception staleness signal between
`DatabaseSessionService` and `SqliteSessionService`, or the entirely-absent
child-session durability in multi-turn `AgentTool`).

Three points worth carrying into our own Session Store design:

- **A verbatim, testable store contract is valuable precisely because it
  is thin.** `BaseSessionService` is four abstract methods plus two optional
  ones. Keeping the mandatory surface this small is what let ADK ship four
  backends with wildly different internals (in-memory dict, two independent
  SQL schema generations, a hand-rolled SQLite path, and a remote API) behind
  one interface without the interface itself needing to encode locking or
  durability policy. Our own interface boundary should resist the temptation
  to standardize concurrency semantics in the abstract contract -- that
  belongs to each backend's own documented guarantees, made explicit rather
  than implied by a shared method signature.
- **State-as-document plus events-as-log, within one aggregate id, is a
  real, shipped pattern worth naming precisely because it is *not* what our
  ADR direction favors.** ADK's `state` column is mutated directly and is
  the one thing in this codebase that is *not* rebuildable from the event
  log (no backend fold-replays `events` to reconstruct `state`; it always
  reads the live column). If our Session Store commits to state-as-projection,
  ADK is a concrete example of the alternative and its cost: state can drift
  from what the event history would replay to, silently, because nothing
  ever checks the two against each other.
- **Rewind-as-appended-marker plus a single shared fold function
  (`_apply_rewinds`) that every reader must call is a clean, cheap pattern**
  worth adopting directly: it keeps the log append-only, makes "what's live"
  a pure function of the log rather than a stored flag, and -- critically --
  ADK's own code comments flag the fragility of this approach: two call
  sites (prompt-building and compaction) must independently agree to call
  the same fold function, and a divergence between them was explicitly
  called out as a correctness risk in the source comments
  (`src/google/adk/apps/compaction.py:390-393,540-543`). Our design should
  make that fold a single, mandatory step in the read path rather than
  something every consumer must remember to call.

## Open questions

- No `adk web`/server code, CLI (`adk migrate session` other than the
  `migration_runner`/README references), or authentication/authorization
  layer around session access was read; this dossier is scoped to
  `src/google/adk/sessions/` and its direct dependents in `events/`,
  `apps/compaction.py`, and `runners.py`.
  `src/google/adk/sessions/migration/README.md:115-121` references an `adk
  migrate session` CLI command and a `cli_tools_click.py`, neither of which
  was located or read; how that CLI wires to `migration_runner.upgrade` is
  unverified.
- Whether any first-party or common third-party `BaseSessionService`
  implementations exist beyond the four in this package (e.g. Redis,
  Firestore) was not investigated; the scope note in the task restricted
  research to in-memory, database/SQL, and Vertex AI.
- The exact wire schema and versioning of the Vertex AI Agent Engine
  Sessions API itself (server-side) is outside this repository and was not
  examined; only ADK's client-side encoding/decoding
  (`_from_api_event`, `raw_event` fallback) was read.
  `src/google/adk/sessions/vertex_ai_session_service.py:558-573` prioritizes
  `raw_event` when present and falls back to legacy top-level fields -- how
  long that legacy path must be kept, and whether the remote API itself
  enforces any schema version, is unknown from this repo alone.
  `docs/upgrading_from_1_22_0.md`, referenced from
  `src/google/adk/sessions/schemas/v0.py:19-20`, was not read; it may contain
  additional migration guidance for users on the v0 schema.
  `src/google/adk/sessions/migration/README.md` is itself the process
  document for *adding* a new schema version, not evidence that a v2 schema
  exists yet -- none does at this commit.
- Whether `DatabaseSessionService`'s in-process `asyncio.Lock` per session
  key (`_with_session_lock`) is intended as the *only* safety net for
  single-process multi-task concurrency, or whether row-level locking alone
  was meant to be sufficient across all supported dialects, is not stated in
  comments; SQLite's exclusion from row-level locking
  (`_supports_row_level_locking`,
  `src/google/adk/sessions/database_session_service.py:431-436`) combined
  with the process-local lock suggests SQLite concurrency safety depends
  entirely on single-process usage, but this was not confirmed by a test or
  doc.
  Whether `SqliteSessionService`'s bare `ValueError` staleness check
  (versus `DatabaseSessionService`'s `_STALE_SESSION_ERROR_MESSAGE`
  constant and marker-based check) is an intentional simplification or an
  oversight is not stated anywhere in the source.
- Full-text of `docs/upgrading_from_1_22_0.md` and any migration guide for
  the SQLite-specific schema lineage were not located in this clone under
  the paths searched; if they exist elsewhere in the repository, they were
  not consulted.

# OpenHands: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against these authoritative anchors:

- `OpenHands/software-agent-sdk` at commit `973c35134f0be00f3ff65b9552b4b304433a74e2`
  (PRIMARY anchor: the SDK, tools, and agent-server all live here).
- `OpenHands/OpenHands` at commit `866512a485c88fbeb34579cd9155a629ae42ed2f`
  (SECONDARY anchor: the web app/frontend, checked only to confirm it has no
  independent transcript persistence).

Citations are `path:line` relative to the corresponding checkout root unless
otherwise noted (e.g. `openhands-sdk/openhands/sdk/conversation/state.py:315`
resolves under the `software-agent-sdk` checkout).

## The storage model

A conversation's durable state is split across two different kinds of files
under one persistence directory:

```text
{persistence_dir}/
  base_state.json          # single mutable document, rewritten wholesale
  events/
    event-00000-{id}.json  # one immutable file per event, never rewritten
    event-00001-{id}.json
    ...
    .eventlog.lock
```

`persistence_const.py` is the single source of truth for these two file
concerns:

```python
BASE_STATE = "base_state.json"
EVENTS_DIR = "events"
# Accept 5+ digits: the writer pads to a 5-digit minimum but does not cap width.
EVENT_NAME_RE = re.compile(
    r"^event-(?P<idx>\d{5,})-(?P<event_id>[0-9a-fA-F\-]{8,})\.json$"
)
EVENT_FILE_PATTERN = "event-{idx:05d}-{event_id}.json"
```
(`openhands-sdk/openhands/sdk/conversation/persistence_const.py:1-11`)

`events/` is append-only and authoritative for conversation history: every
`EventLog.append()` call writes a brand-new file and never edits or deletes
an existing one (`openhands-sdk/openhands/sdk/conversation/event_store.py:184-227`).
`base_state.json` is a separate, also-authoritative document for everything
that is not itself an event: the agent snapshot, workspace, `leaf_event_id`
HEAD pointer, stats, secret registry, and tags
(`openhands-sdk/openhands/sdk/conversation/state.py:82-230`). It is rewritten
in full on every autosaved field mutation via `ConversationState.__setattr__`,
which calls `_save_base_state()`:

```python
def _save_base_state(self, fs: FileStore) -> None:
    payload = self.model_dump_json(exclude_none=True, context=context)
    if self._write_guard is None:
        fs.write(BASE_STATE, payload)
    else:
        with self._write_guard():
            fs.write(BASE_STATE, payload)
```
(`openhands-sdk/openhands/sdk/conversation/state.py:421-442`, autosave trigger
at `state.py:581-634`)

Neither file is a pure cache of the other: `base_state.json` cannot be
rebuilt from `events/` alone (agent config, secrets, and tags have no event
representation), and `events/` cannot be reconstructed from `base_state.json`
(it holds only the current HEAD pointer, not history). The one genuine,
non-persisted cache is the in-memory `View` (`ConversationState._view`,
`state.py:240`): "Cached projection of `_events` for the *active branch*,
lazily updated on read. Derived state -- never persisted."
(`state.py:236-239`). It is rebuilt via `View.from_events()` on cold load,
fork, navigation, and error recovery (`state.py:383-395`).

The closest conceptual fit is **session-as-directory**: a directory holding
one mutable document (for non-event state) plus an append-only, per-entry
event log (for history) -- not a single log, not a single document, and not
event-sourcing in the strict sense, since `base_state.json` carries data that
is not derivable by replaying `events/`.

## Keying and identity

- A conversation is addressed by a `ConversationID` (a UUID); its persistence
  directory is `str(Path(persistence_base_dir) / conversation_id.hex)`
  (`openhands-sdk/openhands/sdk/conversation/base.py:315-330`).
- IDs are server/client-generated `uuid4()`, not time-ordered:
  `StartConversationRequest.conversation_id: UUID | None` -- "If not provided,
  a random UUID will be generated" (`openhands-sdk/openhands/sdk/conversation/request.py`,
  re-verified this session at the field's docstring). Unlike some peer
  products, the ID scheme encodes no ordering or location information.
- Sub-agent Task-tool conversations mint their own fresh `conversation_id`
  via `uuid.uuid4()` inside `TaskManager._generate_ids()`, alongside a
  process-local `task_id` string (`f"task_{task_number:08x}"`, derived from
  `len(self._tasks) + 1`) that is never itself persisted anywhere -- it is a
  purely in-memory, per-`TaskManager`-instance counter
  (`openhands-tools/openhands/tools/task/manager.py:148-153`).
- Listing at the agent-server layer is a full scan of persisted conversation
  metadata at startup, not scoped per-project inherently (each conversation
  record does carry its own `workspace`, but discovery is a flat catalog
  scan -- see "Listing" below).
- No relocation/rename reconciliation logic (moved working directory,
  worktree change) was found anywhere in the reviewed source -- see Open
  questions.

## The store interface

There is no publicly exported, pluggable "session store" protocol type in
the RESEARCH_PROMPT sense. The effective interface is reconstructed from two
layers:

**SDK layer -- `FileStore` (pluggable byte-store abstraction)**, the
substrate both `base_state.json` and `events/` are written through
(`openhands-sdk/openhands/sdk/io/base.py`, full class read this session):
an abstract contract with `write(path, contents)`, `read(path) -> str`,
`list(path) -> list[str]`, `delete(path)`, `exists(path) -> bool`,
`get_absolute_path(path) -> str`, and `lock(path, timeout) -> context manager`.
Two concrete implementations: `LocalFileStore` (plain `open()`/`write()`,
**no** atomic temp-file-and-rename for regular writes, `FileLock`-based
locking, and an explicit docstring warning that flock "does NOT work
reliably on NFS mounts or network filesystems" -- `openhands-sdk/openhands/sdk/io/local.py`,
full read this session) and `InMemoryFileStore` (test/ephemeral, backed by
`MemoryLRUCache`, `openhands-sdk/openhands/sdk/io/memory.py`).

**SDK layer -- `EventLog` (the append-only log)**
(`openhands-sdk/openhands/sdk/conversation/event_store.py`, full read this
session): `append(event)`, `__getitem__(idx)` / `_get_single_item`,
`__iter__`, `__len__`, `__contains__`, `get_index(event_id) -> int`,
`get_id(idx) -> EventID`, `path_to_root(leaf_id, limit=None) -> list[Event]`.
There is **no** `clear`/`delete`/`prune` method -- a dedicated test asserts
this directly: `test_event_log_clear_functionality` in
`tests/sdk/conversation/test_event_store.py` (full file read this session)
contains `assert not hasattr(log, "clear")`.

**SDK layer -- `ConversationState` (the open-or-create factory + mutation
surface)** (`openhands-sdk/openhands/sdk/conversation/state.py`):
`ConversationState.create(id, agent, workspace, persistence_dir, ...)` is the
single entry point for both fresh creation and resume (`state.py:445-578`);
`append_event(event)` is "the single storage chokepoint: stamp parent_id,
append, advance HEAD" (`state.py:315-334`); `view` (property, incremental,
`state.py:336-381`) and `rebuild_view()` (full replay,
`state.py:383-395`) expose the model-visible projection;
`get_unmatched_actions(events)` (`state.py:661-701`) is a static helper for
pending-confirmation reconciliation.

**Agent-server layer -- conversation lifecycle** (`openhands-agent-server/openhands/agent_server/conversation_service.py`,
multiple ranges read in full this session): `start_conversation`,
`interrupt_conversation`, `resume_conversation`, `update_conversation`,
`fork_conversation`, `delete_conversation`, `search_conversations`,
`count_conversations`. **Event query surface**
(`openhands-agent-server/openhands/agent_server/event_service.py:1-260,537-627`):
`search_events`, `count_events`, `_count_events_sync`. Every operation above
carries a repo `path:line`, so this reconstruction, though not an exported
type, is a complete call-site-verified contract.

## Write and append path (ordering, durability, concurrency, delivery)

- **Ordering**: positional, encoded directly in the filename
  (`event-{idx:05d}-{event_id}.json`) -- there is no independent sequence
  number field on the `Event` model itself; order is purely "which index did
  the writer assign at append time"
  (`openhands-sdk/openhands/sdk/conversation/persistence_const.py:10`).
- **Durability**: each event append acquires `self._fs.lock(self._lock_path,
  timeout=LOCK_TIMEOUT_SECONDS)` (30s) around a read-check-write critical
  section, then writes the event to its own new, never-reused file
  (`openhands-sdk/openhands/sdk/conversation/event_store.py:184-227`).
  Regular `FileStore.write()` calls (both for events and for
  `base_state.json`) are **not** temp-file-and-rename atomic in
  `LocalFileStore` -- that atomic-write pattern exists only in the unrelated
  settings/secrets persistence module
  (`openhands-agent-server/openhands/agent_server/persistence/store.py:184-235`,
  `_atomic_write_json`), which this repo's own docstring frames as mirroring
  "OpenHands app-server's FileSettingsStore" -- a different subsystem from
  conversation/event persistence, confirmed by direct read this session.
- **Concurrency**: multi-writer is explicitly supported via a lock plus a
  disk-resync-before-append check:

  ```python
  with self._fs.lock(self._lock_path, timeout=LOCK_TIMEOUT_SECONDS):
      disk_length = self._count_events_on_disk()
      if disk_length > self._length:
          self._sync_from_disk(disk_length)
      ...
  ```
  (`event_store.py:194-199`). But `_count_events_on_disk()` does a full
  `self._fs.list(self._dir)` directory scan **on every single append**
  (`event_store.py:235-249`), with no fast path -- this is the same
  operation flagged in `OpenHands/software-agent-sdk#3906` as an O(N²)
  cost across a whole conversation, with a measured 33× slowdown at N=2000
  events (7408ms vs 222ms). Reading the code at the pinned commit confirms
  this cost is still present; there is no cached count, mtime check, or
  length pointer file that would avoid the listdir.
- A stale in-memory index (e.g. from another process's concurrent write) is
  recovered lazily: `_get_single_item` catches the resulting `KeyError`,
  logs "Stale EventLog index... rebuilding from disk," calls
  `_scan_and_build_index()`, and retries once (`event_store.py:150-159`).
- **Delivery semantics / idempotence**: `append()` raises `ValueError` if an
  event with the same ID already exists, and raises `ValueError` if an
  explicit non-root `parent_id` does not exist in the log
  (`event_store.py:201-215`). This is a hard, fail-fast guard, not a
  silent-dedup at-least-once contract -- a duplicate append is a bug
  surfaced immediately, not swallowed.
- `base_state.json` writes batch multiple field mutations into a single I/O
  operation via a context-manager depth counter: `with state:` increments
  `_save_depth`; mutations inside the block set `_dirty = True` instead of
  writing immediately; `__exit__` flushes once when `_save_depth` returns to
  zero (`state.py:728-749`, mechanism declared at `state.py:256-257`).

## Read and resume path

`ConversationState.create()` is the single resume/create entry point
(`state.py:445-578`). It attempts `file_store.read(BASE_STATE)` first:

- **Resume path** (`base_state.json` found): deserializes the JSON into a
  `ConversationState`, verifies the requested `id` matches the persisted
  one, re-attaches `_fs` and a fresh `EventLog(file_store, dir_path=EVENTS_DIR)`
  (which eagerly re-scans the `events/` directory listing -- not file
  contents -- to rebuild the id↔index mapping,
  `event_store.py:49-57,282-333`), then calls `state.rebuild_view()`
  ("Cold-load: rebuild the cached view with full property enforcement --
  persisted events may come from an older code version or be corrupted",
  `state.py:535-538`), then verifies the runtime agent is compatible with
  the persisted one via `agent.verify(state.agent, events=state._events)`
  (`state.py:541`).
- **Fresh path** (no `base_state.json`): constructs a new `ConversationState`,
  attaches a fresh empty `EventLog`, and immediately calls
  `_save_base_state()` to write the initial snapshot (`state.py:561-578`).
- There is no separate local cache read before the durable store on resume --
  `create()` reads `base_state.json` and the `events/` directory directly.
  `rebuild_view()` replays the full **active branch** (`path_to_root(leaf)`,
  which excludes abandoned/forked-away branches, not the entire event log)
  -- so cold resume cost scales with the active branch length, with no
  page/cursor bound. A bounded-tail read is available separately via
  `active_branch(limit=...)`, "kept O(limit)" by walking back from the leaf
  (`state.py:295-302`), but this is not what cold resume uses.
- Individual event file contents are read and parsed lazily, one at a time,
  and memoized: `EventLog._get_single_item` reads the file, calls
  `Event.model_validate_json(txt)`, and stores the result in
  `self._event_cache: dict[int, Event]` (`event_store.py:140-165`). The
  *index* (which idx maps to which event id) is built eagerly at
  `EventLog.__init__` time from a directory listing, but event *bodies* are
  not read until requested.

## Listing, summaries, and search

- Conversation listing (`search_conversations`/`count_conversations`,
  `conversation_service.py`, ranges read in full this session) is an
  in-memory linear filter/sort/paginate over a catalog populated by a full
  scan of persisted conversation metadata at startup
  (`_load_catalog_sync`) -- there is no index, database, or paginated
  storage-side query; the cost is proportional to the total conversation
  count at request time.
- Event listing/search (`search_events`/`count_events`,
  `openhands-agent-server/openhands/agent_server/event_service.py:537-627`)
  is likewise a linear scan over the `EventLog`, reading each event's
  payload to test `kind`/`source`/body-substring/timestamp-range filters.
  Only a length-based fast path exists when no filters are supplied -- there
  is no full-text, vector, or other separate search index anywhere in the
  SDK or agent-server persistence layer.
- `sub_conversation_ids` (the reverse parent→children pointer) is explicitly
  a computed field, not a stored one: "IDs of conversations naming this one
  as their parent. Derived from the server catalog; empty on webhook
  payloads" (`openhands-agent-server/openhands/agent_server/models.py:245-252`).
  It is recomputed by `_children_index()`/`_children_of()` via a full linear
  scan of the catalog on every call -- not cached -- "because the catalog is
  mutated from several places and a cache could go stale" (comment
  confirmed earlier this session against `conversation_service.py`).

## Entry/message structure and versioning

- The base persisted unit is an `Event` (`openhands-sdk/openhands/sdk/event/base.py`,
  full read this session), with an `LLMConvertibleEvent` subtype exposing
  `to_llm_message()` for the subset of events the LLM actually sees.
  Concrete event kinds relevant to condensation
  (`openhands-sdk/openhands/sdk/event/condenser.py`, full read this
  session): `Condensation` (`forgotten_event_ids: set[EventID]`,
  `summary: str | None`, `summary_offset: int | None`,
  `llm_response_id: EventID`), `CondensationRequest` (a plain marker event),
  and `CondensationSummaryEvent` (`summary: str`, generated dynamically, not
  itself a file on disk -- see Compaction below).
- Entries are **not** opaque to the store: each event file is written via
  `event.model_dump_json(exclude_none=True)` and read back via
  `Event.model_validate_json(txt)` (`event_store.py:163-164,217`), so the
  store must know the discriminated `Event` type hierarchy to parse events
  at all -- it is a typed, parsed record, not a blob.
- Identity/dedup relies on the event's own `id` field, checked against
  `EventLog`'s in-memory `_id_to_idx` map before every append
  (`event_store.py:201-206`).
- No `schema_version` field or migration entry point was found on
  `ConversationState` or `Event` (contrast with the unrelated
  `PersistedSettings.schema_version` / `from_persisted()` migration pattern
  used for settings/secrets, `openhands-agent-server/openhands/agent_server/persistence/models.py:113,281-308`,
  confirmed this session to be a *different* subsystem from
  conversation/event storage). The one observed forward/backward-compat
  mechanism for the event file format is a **more permissive filename
  regex**, not a version field: `EVENT_NAME_RE` accepts `\d{5,}` (five or
  more digits) rather than exactly five, with the comment "the writer pads
  to a 5-digit minimum but does not cap width"
  (`persistence_const.py:6-8`) -- this is the fix for the historical
  100,000-event bug described below.
- Legacy (pre-tree-feature) events without a `parent_id` are still readable:
  `EventLog._effective_parent_id` falls back to treating the previous index
  as the implicit parent, "so old conversations load unbranched with no
  disk rewrite" (`event_store.py:91-104`).

## Compaction and history management

`CondenserBase.condense(view, agent_llm) -> View | Condensation`
(`openhands-sdk/openhands/sdk/context/condenser/base.py:16-52`, full read
this session) is the abstract contract. Its concrete `RollingCondenser`
subclass drives the actual policy:

```python
def condense(self, view: View, agent_llm=None) -> View | Condensation:
    request = self.condensation_requirement(view, agent_llm=agent_llm)
    if request is not None:
        try:
            return self.get_condensation(view, agent_llm=agent_llm)
        except NoCondensationAvailableException as e:
            if request == CondensationRequirement.SOFT:
                return view
            elif request == CondensationRequirement.HARD:
                ... hard_context_reset(...) or re-raise
    else:
        return view
```
(`context/condenser/base.py:159-198`)

The critical retention fact: **`Condensation` is itself an ordinary,
durably-persisted `Event`** -- it goes through the exact same
`EventLog.append()` path as any action or observation, written to its own
new `event-{idx:05d}-{id}.json` file. It never deletes or rewrites any prior
event file. `Condensation.apply(events)` operates purely on an in-memory
`list[LLMConvertibleEvent]`:

```python
def apply(self, events: list[LLMConvertibleEvent]) -> list[LLMConvertibleEvent]:
    output = [e for e in events if e.id not in self.forgotten_event_ids]
    if self.has_summary_metadata:
        output.insert(self.summary_offset, self.summary_event)
    return output
```
(`openhands-sdk/openhands/sdk/event/condenser.py:83-96`)

`View.append_event()` invokes this `apply()` only when replaying a
`Condensation` event into the transient `View` (`openhands-sdk/openhands/sdk/context/view/view.py:111-140`).
The `CondensationSummaryEvent` shown to the LLM is synthesized on the fly
from the `Condensation.summary` string field via a `@property` with a
deterministic id (`f"{self.id}-summary"`) -- "these events are not intended
to be stored alongside regular events" (`event/condenser.py:52-70`) -- so
even the summary text is not written as its own separate file; it is
re-derived from the persisted `Condensation` event every time the view is
rebuilt.

Net effect on the durable record: **`events/` never shrinks from
condensation.** Only the model-visible `View` shrinks. Growth of the
durable directory is unbounded absent something outside this code path.
This is confirmed as a real, user-facing problem by the issue tracker:

- `OpenHands/software-agent-sdk#3926` -- a confirmed, since-fixed silent
  data-loss/corruption bug at exactly 100,000 events, caused directly by
  unbounded append-only growth colliding with a fixed-5-digit filename
  regex (the writer zero-pads to 5 digits but does not cap width past
  99,999; the old reader regex matched exactly 5 digits, so the 100,000th
  event's 6-digit index silently failed to match, and the gap-detection
  logic -- "if n not in by_idx: ... break" (`event_store.py:308-317`) --
  truncates the log at the gap rather than raising). At the pinned commit,
  this specific bug is already fixed (the regex reads `\d{5,}`,
  `persistence_const.py:8`); the issue is cited here as first-hand,
  primary-source evidence of what unbounded growth breaks, not as a live
  defect in this commit.
- `OpenHands/software-agent-sdk#3906` -- the O(N²) `_count_events_on_disk()`
  listdir-per-append cost described above, with a measured 33× slowdown at
  N=2000 events. Reading the code at the pinned commit confirms this cost
  is **still present** (unfixed) here.
- `OpenHands/software-agent-sdk#1824` ("Proposal: don't use full events
  history in the OH ecosystem") -- a maintainer's own first-hand account that
  1,000+ events cause CLI slowdowns/crashes, and that conversations have been
  observed to reach roughly 30,000 events via the SDK.

Default condenser thresholds (`openhands-sdk/openhands/sdk/context/condenser/llm_summarizing_condenser.py`,
full read this session): `LLMSummarizingCondenser` defaults to
`max_size=240` events before condensing, `keep_first=2`,
`minimum_progress=0.1` (condensation must remove at least 10% of events or
it errors). A separate `default_condenser()` factory
(`max_size=80`, `keep_first=4`) is used for both the default top-level agent
and every sub-agent spawned via the registry, confirmed again this session
at `openhands-sdk/openhands/sdk/subagent/registry.py:271-275`: when
`agent_def.condenser is None`, `condenser = default_condenser(llm.model_copy(...))`
-- "Sub-agents get a summarizing condenser by default (parity with the
top-level agent) so deep runs auto-compact instead of erroring on context
overflow" (`openhands-sdk/openhands/sdk/subagent/registry.py:253-256`).

## Rewind, checkpoints, and fork

Events form a tree, not a strict line: each event carries an optional
`parent_id`; `ROOT_PARENT_ID` is an explicit sentinel for a new root; legacy
pre-tree events fall back to implicit linear chaining
(`event_store.py:91-104`, quoted above). `ConversationState.leaf_event_id` is
the movable HEAD -- "the parent of the next appended event... Moving it
re-roots the active branch" (`state.py:176-183`). `_resolve_active_leaf()`
resolves an unset leaf by walking backward from the tail, skipping trailing
non-tree bookkeeping events (`ConversationStateUpdateEvent`,
`ConversationErrorEvent`) so a server restart mid-write cannot strand
history (`state.py:263-293`, referencing bug `#4057` in its own comment).

`navigate_to()` and `fork()` (`openhands-sdk/openhands/sdk/conversation/impl/local_conversation.py:679-858`,
full read this session) are the two retroactive operations:

- **`navigate_to()`** re-roots HEAD in place without copying anything: all
  branches stay on disk; appending after navigating creates a sibling
  branch; events on the abandoned branch remain in the log but drop out of
  `state.view` on the next rebuild.
- **`fork()`** is deep-copy plus lineage metadata, not a shared-prefix
  reference or an identity rewrite: it deep-copies the agent via a JSON
  round-trip (avoiding thread-lock pickling issues per upstream issues
  #2917/#3443), holds the state lock during the read to avoid a torn read,
  and supports either a full-log copy or a `from_event_id`-scoped
  branch-slice copy via `path_to_root`, then calls `rebuild_view()` on the
  new conversation. Lineage is recorded as plain fields --
  `forked_from_conversation_id`, `forked_from_event_id` -- on
  `StoredConversation`/`ConversationInfo`
  (`openhands-agent-server/openhands/agent_server/models.py:96-110,224-237`),
  not as a shared-storage pointer: the forked conversation gets its own
  independent copy of the (possibly branch-sliced) event files under its
  own conversation id/directory.

No file-content or workspace/environment checkpoint tied to individual turns
was found in the reviewed conversation/event persistence source -- see Open
questions.

## Subagents and nested sessions

Two independent, non-overlapping delegation mechanisms exist.

**(a) SDK Task tool** -- `openhands-tools/openhands/tools/task/manager.py`
(full read this session), `definition.py`, `impl.py`. `TaskManager.start_task()`
creates (or resumes) a fully independent `LocalConversation` per task, with
its own fresh `conversation_id` (`_generate_ids()`, `openhands-tools/openhands/tools/task/manager.py:148-153`)
and its own persistence directory: when the parent conversation itself
persists, sub-agent conversations live under
`Path(parent_persistence_dir) / "subagents"`
(`_SUBAGENTS_DIR: Final[str] = "subagents"`, `openhands-tools/openhands/tools/task/manager.py:46,127-133`);
when the parent has no `persistence_dir`, they fall back to
`tempfile.mkdtemp(prefix="openhands_tasks_")` (`openhands-tools/openhands/tools/task/manager.py:135-137`). Both
freshly-created (`_create_task`, `openhands-tools/openhands/tools/task/manager.py:236-285`) and resumed
(`_resume_task`, `openhands-tools/openhands/tools/task/manager.py:201-234`) sub-agent conversations are
constructed with `delete_on_close=True` (`openhands-tools/openhands/tools/task/manager.py:219,314`) -- but this
flag is inert at the `LocalConversation` layer (verified earlier this
session by reading `LocalConversation.close()` in full: it never
references `self.delete_on_close`; only `RemoteConversation` acts on it),
so setting it here has no independent deletion effect through this code
path.

The durable parent-child link is thin by design:

- The `Task` record (`id`, `status`, `conversation_id`, `result`/`error`,
  the live `conversation` object) lives only in
  `TaskManager._tasks: dict[str, Task]` (`openhands-tools/openhands/tools/task/manager.py:103`) -- in-memory,
  scoped to that `TaskManager` instance. `task_id` itself
  (`f"task_{task_number:08x}"`) is a sequential in-process counter, not a
  stable durable identity that survives a process restart on its own.
- The only durable trace inside the *parent's* own event log is a
  `TaskObservation` (`task_id`, `subagent`, `status`, result-or-error text)
  produced by `TaskExecutor.__call__` (`openhands-tools/openhands/tools/task/impl.py:26-66`)
  and appended through the normal tool-call action/observation cycle.
- On completion (success, non-finished stop, or exception), `_run_task()`
  calls `_evict_task()`, which pauses and closes the sub-agent's
  `LocalConversation` and replaces the in-memory record with
  `task.model_copy(update={"conversation": None})` (`openhands-tools/openhands/tools/task/manager.py:155-160,346-377`).
  The live conversation object is dropped from memory; its on-disk
  directory under `subagents/` is **not** deleted at this point.
- `TaskManager.close()` only removes the sub-agents directory when the
  *parent itself has no persistence*: "Only clean up when using a temp dir
  (parent had no persistence). When the parent persists, subagent data
  lives under its directory" (`openhands-tools/openhands/tools/task/manager.py:446-459`) -- meaning when the
  parent does persist, sub-agent directories under `<parent>/subagents/`
  are never deleted by any code path reviewed here.
- Resuming a task (`resume="task_..."`) re-opens the *same* `conversation_id`
  under the parent's `subagents/` directory, round-tripping through the
  ordinary `ConversationState.create()` resume path (`openhands-tools/openhands/tools/task/manager.py:201-220`).
- On a sub-agent crash, `_run_task()`'s `except Exception` records
  `task.error` and reports it back to the parent via
  `TaskObservation(is_error=True)` (`openhands-tools/openhands/tools/task/manager.py:370-372`); whatever the
  sub-agent had already durably appended to its own `events/` remains on
  disk untouched -- there is no rollback.
- Sub-agent LLM usage is folded into the parent's own stats (not the parent's
  conversation transcript) by key: `parent.conversation_stats.usage_to_metrics[f"task:{task.id}"] = ...`
  (`openhands-tools/openhands/tools/task/manager.py:436-444`).

**(b) Agent-server `parent_conversation_id`** -- a separate, coarser
mechanism linking two independent, first-class, top-level conversations
(not an SDK Task). `_ConversationInfoBase.parent_conversation_id`
(`openhands-agent-server/openhands/agent_server/models.py:238-244`) is
client-supplied and validated at creation: `InvalidParentConversation` is
raised if it is "unknown, self-referential, or in a different workspace"
(`conversation_service.py`, verified earlier this session), the workspace
check comparing resolved `working_dir` paths via `_same_workspace()`. The
reverse pointer, `sub_conversation_ids`, is explicitly derived (not stored)
as described in Listing above. Deleting a parent **orphans, not cascades**:
"Children are orphaned, not cascaded: `parent_conversation_id` is left
dangling, like `forked_from_conversation_id` on source delete" (verbatim
comment confirmed earlier this session in `conversation_service.py` around
the delete-conversation handler). No bound on nesting depth for
`parent_conversation_id` chains was found -- see Open questions.

## Retention, deletion, and multi-host

No TTL, lifecycle policy, or scheduled cleanup job for conversation/event
data exists anywhere in the reviewed SDK or agent-server source. Retention
is either explicit (an operator or client calls `delete_conversation`) or
incidental (the Task-tool temp-dir cleanup described above, which only
fires when the *parent* has no persistence). `EventLog` itself exposes no
delete/prune/clear operation at all, confirmed by direct test assertion
(`tests/sdk/conversation/test_event_store.py`, full file read this
session): `assert not hasattr(log, "clear")`.

`delete_conversation` removes the target conversation's own catalog
entry/directory and orphans (does not cascade-delete) any children pointing
at it via `parent_conversation_id`, per the quote above.

Multi-host/crash-detection is handled by a dedicated lease mechanism,
`ConversationLease` (`openhands-agent-server/openhands/agent_server/conversation_lease.py`,
full 282-line file read this session): an `owner_lease.json` payload
(TTL, monotonic `generation`, optional `owner_host`/`owner_pid`) guarded by
a `.owner_lease.lock` `FileLock`. Taking over an existing lease requires
either the TTL to have expired, or the previous owner's PID to be confirmed
dead via `os.kill(pid, 0)` -- and that PID check is only trusted when
`owner_host` matches the current host, "since cross-host PID checks are
meaningless." Every disk write during the lease's lifetime goes through
`guarded_write()`, which re-asserts ownership and raises
`ConversationOwnershipLostError` if the generation has gone stale under it.
This is a first-class, if filesystem-bound, crash-handover design -- not a
distributed lock service. The underlying filesystem assumption is explicit
and admittedly narrow: both `LocalFileStore`'s own docstring and
`EventLog`'s own docstring warn, near-verbatim, that flock-based locking
"does NOT work reliably on NFS mounts or network filesystems"
(`openhands-sdk/openhands/sdk/io/local.py`, `event_store.py:37-40`).

## Interop with foreign session stores

The only foreign-store-adjacent surface confirmed by direct read this
session is the frontend, which has **no** independent transcript
persistence of its own: `openhands-app/src/utils/conversation-local-storage.ts`
(342 lines, full read) stores only UI-preference state in browser
`localStorage` -- selected tab, unpinned tabs, conversation mode, a draft
message, and Files-tab view-mode toggles -- explicitly scoped away from real
conversations by `shouldSkipPersistence()`, which skips both empty ids and
temporary `"task-{uuid}"` placeholder ids used during conversation
initialization (`conversation-local-storage.ts:139-150`). This is UI state,
not a session store, and not interop with any foreign product.

Whether the ACP (Agent Client Protocol) subprocess integration (which drives
foreign coding-agent backends such as codex-acp, claude-agent-acp, and
gemini-cli as subprocesses) ever reads or imports one of those backends' own
native session-transcript files, as opposed to treating their credentials
purely as opaque secrets, was described in an earlier phase of this research
but was **not re-verified against source in this session's final pass** --
treat that specific claim as unconfirmed rather than established; see Open
questions.

## What this implies for our Session Store (our inference)

- OpenHands sits close to, but is not, pure event sourcing: the events
  directory is a genuine append-only log, but `base_state.json` carries
  data that cannot be reconstructed by replaying that log (agent
  configuration, secrets, tags, the HEAD pointer as a practical shortcut).
  For our Session Store, this is a useful cautionary boundary -- if we want
  a strict claim that "the log is the only durable truth," we need to keep
  auditing that no field silently becomes log-independent state the way it
  has here.
- The condenser design -- a compaction step is recorded as one more ordinary,
  immutable event, never a rewrite or deletion of prior history -- is a
  pattern worth adopting outright: it means a crash or torn write during
  compaction cannot corrupt history (compaction either fully lands as a new
  append, or it doesn't happen at all), at the cost of leaving storage
  growth completely unbounded unless something external prunes it. OpenHands
  does not itself bound this; the issue tracker (`#3926`, `#3906`, `#1824`)
  shows that punting retention entirely to ops has produced real,
  user-visible pain (crashes, corruption, and multi-thousand-x slower reads)
  at scale.
- The two coexisting delegation mechanisms -- a cheap, ephemeral SDK Task
  primitive whose only durable trace in the parent is a summary observation
  event, versus a heavier, durably-linked, orphan-on-delete
  `parent_conversation_id` relationship between two first-class
  conversations -- is a workable reference model for us: it demonstrates
  that a single product can offer both a lightweight subagent primitive and
  a first-class nested-conversation primitive without conflating their
  storage or lifecycle semantics.
- The still-present O(N) `_count_events_on_disk()` directory listing on
  every single append (`#3906`, confirmed live at this pinned commit) is a
  direct warning against implementing an append-time concurrency check as an
  unindexed full scan: it is correct, but its cost is invisible in
  development and only surfaces catastrophically at scale. Our own
  append-path concurrency check should use a bounded, O(1)-ish signal (a
  versioned position pointer or compare-and-swap primitive) rather than a
  listdir.

## Open questions

- Is there any schema-version field or explicit migration entry point for
  the `ConversationState`/`Event` JSON format itself, comparable to
  `PersistedSettings.schema_version`/`from_persisted()`? None was found in
  `state.py` or `event/base.py`; the only observed forward/backward-compat
  mechanism for the event file format is the widened filename regex
  (`\d{5,}` instead of a hard 5-digit cap), not a versioned migration.
- Is there any external, ops-level retention/TTL/lifecycle-cleanup process
  for conversation directories outside the SDK/agent-server source reviewed
  here? The code itself implements none.
- Is nesting depth bounded for chained `parent_conversation_id` links
  (parent-of-parent-of-parent...)? Not addressed anywhere in the reviewed
  source.
- Does the ACP subprocess integration ever read or import a foreign agent's
  own native session-transcript store (e.g. a Codex rollout file or a
  Claude Code session file) rather than treating it purely as an opaque
  credential/secret store? An earlier pass of this research concluded "no,"
  but that conclusion was not re-verified against source in this session's
  final pass and should be treated as unconfirmed.
- Is the `subagents/` directory under a persisting parent ever
  garbage-collected by anything (a background job, a separate CLI command,
  an ops process) given that `TaskManager.close()` explicitly skips cleanup
  whenever the parent itself persists?
- Are file-content or workspace-state checkpoints (diffs, snapshots) tied to
  individual turns anywhere in the codebase outside the event/state
  persistence layer covered here? None were found in the paths reviewed.
- How are relocations (moved working directory, renamed workspace)
  reconciled to a conversation's identity, if at all? No such mechanism was
  found.

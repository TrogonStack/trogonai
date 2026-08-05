# AWS Strands Agents: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Source: local clone of
`strands-agents/harness-sdk` (formerly `sdk-python`; ships both a Python SDK
under `strands-py/` and a TypeScript SDK under `strands-ts/`), pinned at
commit `23541039fa1fef14bbfd738d54aface5ffefd625`, Apache-2.0. All citations
below are repo-root-relative paths within that clone. This dossier covers the
**Python SDK** (`strands-py/`) only; `strands-ts/` has a parallel
`src/session/` implementation that was not read for this dossier (see Open
questions). Version-sensitive claims were checked against these anchors:

- `strands-py/src/strands/session/session_manager.py` -- abstract
  `SessionManager` (hook-driven lifecycle interface).
- `strands-py/src/strands/session/session_repository.py` -- abstract
  `SessionRepository` (CRUD interface).
- `strands-py/src/strands/session/repository_session_manager.py` -- the one
  concrete `SessionManager` shipped, generic over any `SessionRepository`.
- `strands-py/src/strands/session/file_session_manager.py` and
  `strands-py/src/strands/session/s3_session_manager.py` -- the two shipped
  `SessionRepository` implementations.
- `strands-py/src/strands/types/session.py` -- the `Session`, `SessionAgent`,
  `SessionMessage` data models.
- `strands-py/src/strands/multiagent/{base,graph,swarm}.py` -- multi-agent
  orchestrator state persistence.
- `strands-py/src/strands/experimental/checkpoint/checkpoint.py` and
  `strands-py/src/strands/types/_snapshot.py` -- two adjacent, non-store
  persistence mechanisms that matter for the versioning and fork questions.
- `team/designs/0014-storage.md` -- an internal, dated (2026-06-29), "Proposed"
  design record for a future unified storage primitive that would replace the
  interface studied here.

## The storage model

There is no log file anywhere in this store. The durable session is a
**row-set of small, independent JSON documents**, one file (file backend) or
one S3 object (S3 backend) per session/agent/message/multi-agent record, with
no envelope tying them together beyond the path/key hierarchy itself.

`SessionManager`'s docstring states the intent directly: "A session manager is
in charge of persisting the conversation and state of an agent across its
interaction. Changes made to the agents conversation, state, or other
attributes should be persisted immediately after they are changed."
(`strands-py/src/strands/session/session_manager.py:34-37`).

Both shipped backends document the same layout in their class docstrings
(`strands-py/src/strands/session/file_session_manager.py:31-43`,
`strands-py/src/strands/session/s3_session_manager.py:34-46`):

```text
/<sessions_dir>/
└── session_<session_id>/
    ├── session.json                # Session metadata
    └── agents/
        └── agent_<agent_id>/
            ├── agent.json          # Agent metadata
            └── messages/
                ├── message_<id1>.json
                └── message_<id2>.json
```

This docstring is incomplete on both files: `create_session` on both backends
also creates a `multi_agents/` directory/prefix
(`strands-py/src/strands/session/file_session_manager.py:172-173`,
inferred equivalently for S3 since `_get_multi_agent_path` builds
`multi_agents/multi_agent_<id>/` keys at
`strands-py/src/strands/session/s3_session_manager.py:353-357`), which the
docstring never mentions.

Everything under a session directory/prefix is **authoritative** -- there is
no derived cache, summary, index, or search structure of any kind in this
store. Every read returns exactly what was last written, verbatim
(`_read_file` / `_read_s3_object` simply deserialize JSON:
`strands-py/src/strands/session/file_session_manager.py:119-131`,
`strands-py/src/strands/session/s3_session_manager.py:142-154`). Ordering
within an agent's messages is **positional**, carried by an integer
`message_id` that doubles as part of the filename/key
(`message_<id>.json`), not by a separate append-only sequence counter.

Conceptual model: **session-as-row-set** -- a session is a directory (or key
prefix) of independent whole-object documents addressed by a fixed path
scheme, closer to a tiny per-session key-value store than to a transcript or
a log. This is a deliberate design consequence, not an oversight: because
each conversation turn's user/assistant/tool messages are each written as
their own whole file (see Write and append path), the store never needs a
true "append" primitive, which sidesteps the one operation an object store
cannot do natively.

## Keying and identity

`session_id` and `agent_id` are **caller-supplied strings**, not minted by the
store. `RepositorySessionManager.__init__` takes `session_id` as a
constructor argument and creates the session if one does not already exist
under that id -- an idempotent create-or-attach, not a fresh mint per call
(`strands-py/src/strands/session/repository_session_manager.py:31-59`).

Both ids are validated identically, by a shared helper that rejects anything
containing a path separator:

```python
# strands-py/src/strands/_identifier.py:14-30
def validate(id_: str, type_: Identifier) -> str:
    if os.path.basename(id_) != id_:
        raise ValueError(f"{type_.value}_id={id_} | id cannot contain path separators")
    return id_
```

`Identifier` has exactly two members, `AGENT` and `SESSION`
(`strands-py/src/strands/_identifier.py:7-11`). Multi-agent ids reuse the
`AGENT` variant -- there is no third `MULTI_AGENT` identifier kind
(`strands-py/src/strands/session/file_session_manager.py:298`,
`strands-py/src/strands/session/s3_session_manager.py:356`).

Key hierarchy (file backend):
`<storage_dir>/session_<session_id>/agents/agent_<agent_id>/messages/message_<id>.json`
and `<storage_dir>/session_<session_id>/multi_agents/multi_agent_<id>/multi_agent.json`
(`strands-py/src/strands/session/file_session_manager.py:74-117,295-299`).
Key hierarchy (S3 backend) is the identical shape as a flat key with `/`
separators and an optional caller-supplied `prefix`
(`strands-py/src/strands/session/s3_session_manager.py:93-140,353-357`).

`message_id` is an integer **position index**, assigned client-side inside
`RepositorySessionManager`, not by the repository:

```python
# strands-py/src/strands/session/repository_session_manager.py:77-86
latest_agent_message = self._latest_agent_message[agent.agent_id]
if latest_agent_message:
    next_index = latest_agent_message.message_id + 1
else:
    next_index = 0
session_message = SessionMessage.from_message(message, next_index)
self._latest_agent_message[agent.agent_id] = session_message
self.session_repository.create_message(self.session_id, agent.agent_id, session_message)
```

**No listing or enumeration operation exists anywhere** in the interface or
either shipped implementation: neither `SessionManager` nor
`SessionRepository` declares a `list_sessions`/`list_agents` method, and a
repo-wide search of `strands-py/src/strands/session/*.py` for
`list_sessions` returns no matches. A caller must already know the
`session_id` (and `agent_id`) to address anything; there is no picker, no
directory scan surfaced through the SDK, and no cross-session query. (See
Listing, summaries, and search.)

There is no workspace/cwd binding, and no relocation/rename concept: the
`Session` dataclass carries only `session_id`, `session_type`, `created_at`,
`updated_at` (`strands-py/src/strands/types/session.py:196-212`) -- no
working-directory or origin field exists to reconcile if a caller's working
directory moves. The id is a bare opaque string used verbatim as a directory
name / key prefix.

## The store interface

Strands is interface-first: there are two separate abstract contracts, layered.
`SessionManager` is a **hook-driven lifecycle interface** invoked by the
agent's hook registry; `SessionRepository` is the **CRUD interface** it calls
into. `RepositorySessionManager` is the one shipped `SessionManager`,
generic over any `SessionRepository`; `FileSessionManager` and
`S3SessionManager` are `SessionRepository` implementations that also inherit
`RepositorySessionManager`, so each is usable directly as a
`session_manager=` argument.

### `SessionManager` (abstract, `strands-py/src/strands/session/session_manager.py`)

| Method | Signature | Required? | Invoked by |
| --- | --- | --- | --- |
| `register_hooks` | `(self, registry: HookRegistry, **kwargs) -> None` | Concrete (not abstract) | Called once by the agent's hook system when the manager is attached; wires every callback below (`:40-62`). |
| `redact_latest_message` | `(self, redact_message: Message, agent: "Agent", **kwargs) -> None` | **Abstract** | Not hook-wired; called directly by guardrail/redaction code paths (`:65-72`). |
| `append_message` | `(self, message: Message, agent: "Agent", **kwargs) -> None` | **Abstract** | `MessageAddedEvent` (`:46`). |
| `sync_agent` | `(self, agent: "Agent", **kwargs) -> None` | **Abstract** | `MessageAddedEvent` (`:49`) and `AfterInvocationEvent` (`:52`). |
| `initialize` | `(self, agent: "Agent", **kwargs) -> None` | **Abstract** | `AgentInitializedEvent` (`:43`). |
| `sync_multi_agent` | `(self, source: "MultiAgentBase", **kwargs) -> None` | Optional -- base impl raises `NotImplementedError` (`:109-113`) | `AfterNodeCallEvent` (`:55`) and `AfterMultiAgentInvocationEvent` (`:56`). |
| `initialize_multi_agent` | `(self, source: "MultiAgentBase", **kwargs) -> None` | Optional -- base impl raises `NotImplementedError` (`:126-130`) | `MultiAgentInitializedEvent` (`:54`). |
| `initialize_bidi_agent` | `(self, agent: "BidiAgent", **kwargs) -> None` | Optional -- raises `NotImplementedError` (`:139-143`) | `BidiAgentInitializedEvent` (`:59`). |
| `append_bidi_message` | `(self, message: Message, agent: "BidiAgent", **kwargs) -> None` | Optional -- raises `NotImplementedError` (`:153-157`) | `BidiMessageAddedEvent` (`:60`). |
| `sync_bidi_agent` | `(self, agent: "BidiAgent", **kwargs) -> None` | Optional -- raises `NotImplementedError` (`:166-170`) | `BidiMessageAddedEvent` (`:61`) and `BidiAfterInvocationEvent` (`:62`). |

`BidiAgent` lives under `strands.experimental.bidi` -- the bidi hooks are an
experimental, parallel lifecycle for (presumably voice/streaming) agents that
have no conversation manager, hence no compaction/`removed_message_count`
concept (`strands-py/src/strands/types/session.py:159-164`).

### `SessionRepository` (abstract, `strands-py/src/strands/session/session_repository.py`)

```python
class SessionRepository(ABC):
    @abstractmethod
    def create_session(self, session: Session, **kwargs: Any) -> Session: ...          # :16-17
    @abstractmethod
    def read_session(self, session_id: str, **kwargs: Any) -> Session | None: ...       # :20-21
    @abstractmethod
    def create_agent(self, session_id: str, session_agent: SessionAgent, **kwargs: Any) -> None: ...   # :24-25
    @abstractmethod
    def read_agent(self, session_id: str, agent_id: str, **kwargs: Any) -> SessionAgent | None: ...    # :28-29
    @abstractmethod
    def update_agent(self, session_id: str, session_agent: SessionAgent, **kwargs: Any) -> None: ...   # :32-33
    @abstractmethod
    def create_message(self, session_id: str, agent_id: str, session_message: SessionMessage, **kwargs: Any) -> None: ...  # :36-37
    @abstractmethod
    def read_message(self, session_id: str, agent_id: str, message_id: int, **kwargs: Any) -> SessionMessage | None: ...   # :40-41
    @abstractmethod
    def update_message(self, session_id: str, agent_id: str, session_message: SessionMessage, **kwargs: Any) -> None: ...  # :44-48
    @abstractmethod
    def list_messages(self, session_id: str, agent_id: str, limit: int | None = None, offset: int = 0, **kwargs: Any) -> list[SessionMessage]: ...  # :51-54

    def create_multi_agent(self, session_id: str, multi_agent: "MultiAgentBase", **kwargs: Any) -> None:
        raise NotImplementedError("MultiAgent is not implemented for this repository")   # :56-58
    def read_multi_agent(self, session_id: str, multi_agent_id: str, **kwargs: Any) -> dict[str, Any] | None:
        raise NotImplementedError("MultiAgent is not implemented for this repository")   # :60-62
    def update_multi_agent(self, session_id: str, multi_agent: "MultiAgentBase", **kwargs: Any) -> None:
        raise NotImplementedError("MultiAgent is not implemented for this repository")   # :64-66
```

`update_message`'s docstring is explicit about intent: "A message is usually
only updated when some content is redacted due to a guardrail."
(`strands-py/src/strands/session/session_repository.py:44-48`).

**There is no `delete_message`, `delete_agent`, or `delete_session` on this
ABC at all.** Both shipped backends implement a `delete_session` method
(`strands-py/src/strands/session/file_session_manager.py:191-197`,
`strands-py/src/strands/session/s3_session_manager.py:191-212`), but it is
not part of the abstract contract, and neither `SessionManager` nor
`RepositorySessionManager` ever calls it
(`repository_session_manager.py` has no reference to `delete_session`). A
custom `SessionRepository` (say, backed by DynamoDB) that only implements the
abstract methods would compile and run with no delete capability at all, and
nothing in the type system would catch that gap. Deletion is a
backend-specific convenience bolted onto the concrete classes, not a
guaranteed part of the pluggable interface -- an interface gap worth flagging
against Q11 (retention/deletion).

## Write and append path (ordering, durability, concurrency, delivery)

**Every write is a whole-object replace, never a byte-level append or a
partial update, on both backends.** "Appending a message" means writing an
entirely new, small JSON file/object at a new key; nothing in this codebase
ever opens an existing session/agent/message file and appends bytes to it.
This is why the object-store limitation ("S3 has no append") never surfaces
here -- the abstraction was shaped to avoid needing it. Concretely:

- `create_message` (append) writes one file/object per message, keyed by
  `message_<id>.json`. File backend:
  `strands-py/src/strands/session/file_session_manager.py:231-239`. S3
  backend: `strands-py/src/strands/session/s3_session_manager.py:241-246`.
- `update_agent` / `update_message` read the previous record, verify it
  exists (else raise `SessionException`), preserve `created_at` from the
  prior version, and overwrite the whole object with the new one. This logic
  is duplicated near-verbatim between the two backends: file
  (`strands-py/src/strands/session/file_session_manager.py:220-229,249-259`),
  S3 (`strands-py/src/strands/session/s3_session_manager.py:229-239,256-266`).
- `create_multi_agent` / `update_multi_agent` write one whole JSON blob per
  multi-agent id, same pattern on both backends
  (`strands-py/src/strands/session/file_session_manager.py:301-326`,
  `strands-py/src/strands/session/s3_session_manager.py:359-379`).

**Ordering** is the integer `message_id` position index, computed and cached
in the caller's process memory (`self._latest_agent_message`,
`strands-py/src/strands/session/repository_session_manager.py:63-64,78-86`),
not by any server-assigned sequence or timestamp. There is no monotonic
clock or expected-version precondition anywhere in the append path.

**Durability and atomicity per single write differ in mechanism but converge
on the same guarantee (no torn/partial object is ever observable):**

- File backend: `_write_file` writes to a `tempfile.mkstemp()` file in the
  same directory, then calls `os.replace(tmp_path, path)` -- an atomic rename
  on POSIX -- with cleanup of the temp file on any exception
  (`strands-py/src/strands/session/file_session_manager.py:133-161`). There
  is **no `os.fsync`** of the file descriptor or the containing directory
  anywhere in this path, so while a reader can never observe a half-written
  file, durability across an OS/power-loss crash immediately after
  `os.replace` is not guaranteed by this code (inference -- no fsync means
  the rename could still be sitting in page cache).
- S3 backend: `_write_s3_object` issues a single `put_object` call
  (`strands-py/src/strands/session/s3_session_manager.py:156-164`). S3
  guarantees per-object atomicity for a single PUT as a platform property (a
  concurrent GET returns either the whole old or whole new object, never a
  mix); the SDK code does not add, verify, or even comment on this guarantee
  -- it simply relies on it implicitly by issuing one PUT per logical write.

**Concurrency: neither backend has any locking, or compare-and-swap /
conditional-write precondition on any operation.** No file locks, no S3
`If-Match`/`If-None-Match` conditional headers, no version/ETag checks
anywhere in `file_session_manager.py` or `s3_session_manager.py`. Both
backends are symmetric here -- the S3 backend is not "less safe" than the
file backend; neither is safe against concurrent writers. Two consequences:

1. **`create_session`'s existence check is a check-then-act race on both
   backends, identically shaped.** File backend:
   `if os.path.exists(session_dir): raise SessionException(...)` followed by
   `os.makedirs(..., exist_ok=True)`
   (`strands-py/src/strands/session/file_session_manager.py:164-180`). S3
   backend: a `head_object` 404 probe followed by `put_object`, with no
   `IfNoneMatch` conditional write
   (`strands-py/src/strands/session/s3_session_manager.py:166-181`). Two
   processes racing to create the same `session_id` can both pass the
   existence check and both write `session.json`; the loser's write is
   silently overwritten with no error surfaced to either caller. This is one
   place the two backends deliver genuinely identical (mis)behavior.
2. **The `message_id` counter lives above the repository abstraction, in
   `RepositorySessionManager`'s process memory, so two independent manager
   instances attached to the same `session_id`/`agent_id` (e.g. two processes
   resuming the same session) will independently compute the same
   `next_index` from whatever they read at `initialize()` time, and can both
   `create_message` at the same key with different content** -- a genuine
   collision (not just a lost update of identical data), applying identically
   to both backends since the counter logic is shared, not
   backend-specific (`strands-py/src/strands/session/repository_session_manager.py:69-86`).

**Delivery semantics and idempotence.** There is no retry/at-least-once
wrapper visible in either backend (a raised `ClientError`/`OSError`
propagates as a `SessionException`, uncaught). The one idempotence-adjacent
mechanism is `Message.tracking_id` -- a durable, stable UUIDv4 assigned to a
message by the agent (not the store), distinct from the positional
`message_id`:

```python
# strands-py/src/strands/types/content.py:230-248
class Message(TypedDict):
    content: list[ContentBlock]
    role: Role
    tracking_id: NotRequired[str]      # durable UUID identity, survives session save/restore
    metadata: NotRequired[MessageMetadata]
```

```python
# strands-py/src/strands/types/content.py:263-275
def _ensure_tracking_id(message: Message) -> str:
    if not message.get("tracking_id"):
        message["tracking_id"] = _generate_tracking_id()
    return message["tracking_id"]
```

`tracking_id` is content identity (a UUID, stable across copy/redact/restore);
`message_id` is storage-position identity (an integer, reassigned if a
message is re-appended at a different offset). The store itself does not
dedup by `tracking_id` -- nothing in `file_session_manager.py` or
`s3_session_manager.py` reads or checks it; it is preserved as ordinary
message content and round-trips verbatim
(`strands-py/tests/strands/session/test_file_session_manager.py:218-236`,
test `test_message_durable_id_persists`, confirms the field survives a
create/read round-trip byte-for-byte).

## Read and resume path

Resume is a full, eager, ordered read; there is no cursor-based incremental
read, no cached "latest view" separate from the store itself. On
`AgentInitializedEvent`, `RepositorySessionManager.initialize`
(`strands-py/src/strands/session/repository_session_manager.py:169-243`):

1. Reads the `SessionAgent` record via `read_agent` (skipped entirely for a
   session known to be brand-new, `:180-184`).
2. If absent, treats this as a new agent: creates it, then writes every
   message currently in `agent.messages` as an individual
   `create_message` call with sequential indices (`:186-200`).
3. If present, restores `agent.state`, internal state (interrupt state,
   model state), and the conversation manager's own state via
   `restore_from_session`, which may hand back messages to prepend
   (`:201-216`).
4. Calls `list_messages(..., offset=agent.conversation_manager.removed_message_count)`
   -- the **only** place pagination/offset is used on the read path, and it
   is driven by the conversation manager's compaction bookkeeping, not by
   caller-supplied pagination (`:217-223`).
5. Unless the model is `stateful` (server-managed conversation, e.g. a
   Responses-API-style model that holds its own history), rebuilds
   `agent.messages` as `prepend_messages + [...loaded session messages...]`,
   then runs `_fix_broken_tool_use` to repair orphaned/mismatched
   `toolUse`/`toolResult` pairs left over from prior truncation
   (`:226-241`, referencing
   `https://github.com/strands-agents/harness-sdk/issues/859` in the code
   comment at `:240`).

`_fix_broken_tool_use` (`strands-py/src/strands/session/repository_session_manager.py:245-319`)
is itself evidence that this store's "resume" path has had to defensively
patch real-world corrupted histories: it drops a leading orphaned
`toolResult` with no preceding `toolUse` (`:262-269`), and for any assistant
message with `toolUse` blocks whose paired result message doesn't exactly
match by `toolUseId`, it rebuilds that result message from scratch, filling
gaps with synthesized error results (`:270-319`).

**Everything materialized on resume is eager** -- the entire (offset-adjusted)
message list is loaded into memory as part of `initialize`, not lazily
per-turn. There is no lazy-loading or byte-range mechanism anywhere in this
store (contrast with, e.g., a claim-check pattern for oversized tool results
-- none exists here; message bodies are stored inline, whole, every time).

`list_messages`'s `limit`/`offset` parameters exist on the interface and are
exercised by the compaction-offset call above, but there is no other
caller-facing pagination in the resume path itself -- a full resume always
reads the tail of the message list from the compaction offset forward, in
one call.

## Listing, summaries, and search

**None of the three exist.** There is no listing operation
(`list_sessions`/`list_agents`) anywhere in `SessionManager` or
`SessionRepository`; no metadata sidecar, summary, or read-model file is
written by either backend (no `session.json` field is a denormalized
summary of message content -- `Session` only carries `session_id`,
`session_type`, `created_at`, `updated_at`,
`strands-py/src/strands/types/session.py:196-202`); and there is no
full-text, vector, or any other search index. A caller who wants to enumerate
sessions must build that capability outside this SDK (e.g. by listing the
`storage_dir` directory or the S3 bucket's `session_` prefixes directly) --
nothing in `file_session_manager.py` or `s3_session_manager.py` exposes such
a helper.

## Entry and message structure and versioning

### `Session`, `SessionAgent`, `SessionMessage` (`strands-py/src/strands/types/session.py`)

```python
# :58-74
@dataclass
class SessionMessage:
    message: Message
    message_id: int
    redact_message: Message | None = None
    created_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
    updated_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())
```

`to_message()` returns `redact_message` in place of `message` when set
(`:86-94`) -- redaction is modeled as a **side-by-side replacement field**,
not an edit of the original content: the original `message` stays on disk,
`redact_message` is added alongside it, and reads transparently prefer the
redaction. Bytes values in either field are base64-encoded on write and
decoded on read via `encode_bytes_values`/`decode_bytes_values`
(`:28-55`).

```python
# :107-124
@dataclass
class SessionAgent:
    agent_id: str
    state: dict[str, Any]
    conversation_manager_state: dict[str, Any]
    _internal_state: dict[str, Any] = field(default_factory=dict)
    created_at: str = field(default_factory=...)
    updated_at: str = field(default_factory=...)
```

`_internal_state` carries `interrupt_state` and `model_state`
(`strands-py/src/strands/types/session.py:135-138`), restored via
`initialize_internal_state` (`:176-181`).

```python
# :195-202
@dataclass
class Session:
    session_id: str
    session_type: SessionType
    created_at: str = field(default_factory=...)
    updated_at: str = field(default_factory=...)
```

`SessionType` is a `str, Enum` with a **single member**, `AGENT`
(`strands-py/src/strands/types/session.py:18-25`) -- the enum's own docstring
anticipates growth ("As sessions are expanded to support new use cases like
multi-agent patterns, new types will be added here"), but at this commit only
one value exists, and nothing branches on it.

### Schema evolution: no version field on any durable session type

**None of `Session`, `SessionAgent`, or `SessionMessage` carries a
`schema_version` field, and `from_dict` on all three silently drops unknown
keys and defaults missing ones** (`strands-py/src/strands/types/session.py:96-100,166-170,204-207`
all filter `env.items()` down to `inspect.signature(cls).parameters` before
constructing). There is no migration function, no legacy-format sniffing, and
no version negotiation anywhere in `session/` or `types/session.py`. For a
product at this stage, that absence is itself the finding: forward
compatibility is handled entirely by "extra fields are ignored, missing
fields use dataclass defaults," which works for additive changes but has no
mechanism to reject or transform an incompatible shape.

The **only two schema-version fields in the whole persistence surface belong
to mechanisms that are explicitly not the session store**:

- `Checkpoint.schema_version` (always `"1.0"`,
  `strands-py/src/strands/experimental/checkpoint/checkpoint.py:40,45-57`),
  which `from_dict` uses to hard-reject a mismatched version by raising
  `CheckpointException` (`:73-78`). The module docstring is explicit that
  this is not conversation state: "It does **not** capture conversation
  state -- pair with a `SessionManager` for cross-process state continuity."
  (`:1-6`).
- `Snapshot.schema_version` (always `"1.0"`,
  `strands-py/src/strands/types/_snapshot.py:33,40-47`), whose `validate()`
  raises `SnapshotException` on any version other than `"1.0"`
  (`:54-66`). `Snapshot` is a separate, opt-in, in-memory export/import
  feature on `Agent` (`take_snapshot`/`load_snapshot`,
  `strands-py/src/strands/agent/agent.py:1543-1617`) -- it is the caller's
  responsibility to persist and restore the `Snapshot` object; the
  `SessionManager`/`SessionRepository` machinery is not involved at all.

So: the transient, explicitly-versioned mechanisms reject old data outright;
the durable session store has no version concept and silently tolerates
shape drift.

## Compaction and history management

Compaction is entirely a **conversation-manager concern layered on top of an
unbounded, never-pruned message store** -- the durable message files are
never deleted or rewritten by compaction on either backend.

`ConversationManager` tracks `removed_message_count`, "the messages that have
been removed from the agents messages array. These represent messages
provided by the user or LLM that have been removed, not messages included by
the conversation manager through something like summarization."
(`strands-py/src/strands/agent/conversation_manager/conversation_manager.py:78-82,94`).
`get_state`/`restore_from_session` persist and restore only this counter (and,
for `SummarizingConversationManager`, an optional summary message) as part of
`SessionAgent.conversation_manager_state`
(`strands-py/src/strands/agent/conversation_manager/conversation_manager.py:159-177`,
`strands-py/src/strands/agent/conversation_manager/summarizing_conversation_manager.py:85-100`).
`SummarizingConversationManager.reduce_context` increments
`removed_message_count` by the number of turns folded into a summary
(`strands-py/src/strands/agent/conversation_manager/summarizing_conversation_manager.py:188-191`);
`SlidingWindowConversationManager` does the same for trimmed turns
(`strands-py/src/strands/agent/conversation_manager/sliding_window_conversation_manager.py:206,270`).

On resume, `initialize` calls
`list_messages(..., offset=removed_message_count)`
(`strands-py/src/strands/session/repository_session_manager.py:219-223`) --
compaction is implemented purely as **a read-time offset into the full,
untouched message file list**. No `SessionRepository` implementation has a
`delete_message` method (confirmed above), so **the underlying
`message_<id>.json` files for every "compacted-away" turn remain on disk (or
in the S3 bucket) forever**, counted against nothing, cleaned up by nothing.
A session that summarizes 10,000 turns down to a 50-turn visible window still
holds 10,000 message files in storage. This is the single most consequential
"what bounds durable growth" finding in this dossier: **nothing bounds it**
(see Retention, deletion, and multi-host).

## Rewind, checkpoints, and fork

**No rewind, undo, or branch/fork operation exists in the session store
itself.** The two adjacent mechanisms that could be mistaken for one:

- **`Checkpoint`** (`strands-py/src/strands/experimental/checkpoint/checkpoint.py`)
  is a **mid-cycle pause marker**, not a history edit: "A `Checkpoint` is a
  pause-point marker emitted at agent cycle boundaries. It captures the
  position (which boundary fired) and the cycle index." (`:1-11`). It is
  emitted only on tool-use cycles ("A turn with no tool calls emits no
  checkpoint; use a `SessionManager` for durability of every turn.", `:25-26`)
  and is surfaced via `AgentResult.checkpoint`
  (`strands-py/src/strands/agent/agent_result.py:30-41`) for the caller to
  pass back on resume -- it is explicitly paired with, not a replacement for,
  the session store.
- **`Snapshot`** (`strands-py/src/strands/types/_snapshot.py`,
  `strands-py/src/strands/agent/agent.py:1543-1617`) is the closest thing to
  a fork primitive: `agent.take_snapshot(preset="session")` captures
  `messages`, `state`, `conversation_manager_state`, `interrupt_state`, and
  optionally `system_prompt`/`model_state` as one in-memory, versioned,
  JSON-serializable object; `agent.load_snapshot(snapshot)` restores all of
  it into a (potentially different) agent instance. This is genuinely
  copy-plus-restore -- a fresh agent loaded from a snapshot is fully
  independent of the source agent's `session_id` -- but it is **entirely
  outside `SessionManager`/`SessionRepository`**: nothing in
  `file_session_manager.py` or `s3_session_manager.py` knows about
  `Snapshot`, there is no lineage metadata recorded anywhere (no
  parent-snapshot pointer, no fork counter), and persisting the `Snapshot`
  object anywhere durable is entirely the caller's problem.

Redaction (`redact_latest_message` /
`SessionRepository.update_message`) is the one retroactive-looking operation
that *is* part of the store, and it is additive rather than destructive: the
original `message` field is preserved on disk; `redact_message` is written
alongside it and preferred by `to_message()`
(`strands-py/src/strands/types/session.py:86-94`).

## Subagents and nested sessions

**A child node in a multi-agent orchestration is never its own durable
session.** Both shipped multi-agent patterns -- `Graph` and `Swarm` -- actively
forbid a node's own `Agent` from carrying a session manager:

```python
# strands-py/src/strands/multiagent/graph.py:294-298
if isinstance(executor, Agent):
    # Check for session persistence
    if executor._session_manager is not None:
        raise ValueError("Session persistence is not supported for Graph agents yet.")
```

```python
# strands-py/src/strands/multiagent/swarm.py:539-541
if node._session_manager is not None:
    raise ValueError("Session persistence is not supported for Swarm agents yet.")
```

(Note: this SDK ships exactly two multi-agent orchestration patterns --
`Graph` and `Swarm`. A search of `strands-py/src/strands/multiagent/` for
"workflow" returns no matches; there is no third `Workflow` primitive at this
commit.)

The only durable artifact tied to multi-agent execution is the
**orchestrator's own state**, stored as a single whole-JSON blob at
`multi_agents/multi_agent_<id>/multi_agent.json`, via `MultiAgentBase`'s
`serialize_state`/`deserialize_state` (declared abstract-by-convention on the
base class -- the base implementation simply raises `NotImplementedError`,
`strands-py/src/strands/multiagent/base.py:329-335`) and persisted through
`SessionManager.sync_multi_agent`/`initialize_multi_agent`. Both `Graph` and
`Swarm` wire this to hooks fired **after every node completes** and **after
the whole orchestrator run**:

```python
# strands-py/src/strands/session/session_manager.py:54-56
registry.add_callback(MultiAgentInitializedEvent, lambda event: self.initialize_multi_agent(event.source))
registry.add_callback(AfterNodeCallEvent, lambda event: self.sync_multi_agent(event.source))
registry.add_callback(AfterMultiAgentInvocationEvent, lambda event: self.sync_multi_agent(event.source))
```

`Graph.serialize_state` / `Swarm.serialize_state`
(`strands-py/src/strands/multiagent/graph.py:1265-1287`,
`strands-py/src/strands/multiagent/swarm.py:978-1009`) embed a
`node_results` map keyed by node id, each value a `NodeResult.to_dict()`
(`strands-py/src/strands/multiagent/base.py:87-105`). For a node whose
executor is an `Agent`, that nested value is an `AgentResult.to_dict()`
(`strands-py/src/strands/agent/agent_result.py:120-131`):

```python
# strands-py/src/strands/agent/agent_result.py:120-131
def to_dict(self) -> dict[str, Any]:
    return {
        "type": "agent_result",
        "message": self.message,        # only the LAST message, not the full conversation
        "stop_reason": self.stop_reason,
        "checkpoint": self.checkpoint.to_dict() if self.checkpoint else None,
    }
```

**This is the durable parent-child link, and it is lossy by construction**: a
node's *final* message is embedded inside the parent orchestrator's single
JSON blob; the node's full internal conversation (every intermediate
user/assistant/tool-call turn it produced while executing) is never written
to durable storage anywhere, because the node's `Agent` is barred from having
a session manager at all. There is no nested session directory, no sibling
session, and no separate child transcript to cascade-delete, orphan, or
reconcile -- there is nothing durable to orphan in the first place beyond that
one final message per node.

Crash behavior follows directly from the sync timing: because
`sync_multi_agent` fires only on `AfterNodeCallEvent` (node *completion*) and
`AfterMultiAgentInvocationEvent` (run completion), a crash while a node is
**still executing** loses that node's entire in-flight work -- nothing about
it was ever synced. On restart, `deserialize_state`
(`strands-py/src/strands/multiagent/graph.py:1289-1316`,
`strands-py/src/strands/multiagent/swarm.py:1011-1029`) either resets all
nodes to re-execute from the beginning (if no `next_nodes_to_execute` was
persisted -- the terminal/fresh case) or resumes from the last-synced set of
ready-to-execute nodes; either way, resumption re-runs the interrupted node
from scratch rather than replaying any partial progress, because no partial
progress was ever durable.

## Retention, deletion, and multi-host

**No TTL, lifecycle policy, or scheduled cleanup exists anywhere in this
store.** A search of `strands-py/src/strands/session/*.py` and
`strands-py/src/strands/types/session.py` for retention/TTL/lifecycle/cleanup
terms returns no matches. The store retains everything it is given,
indefinitely, until an explicit `delete_session` call.

**Deletion is whole-session-only, is not part of the abstract contract**
(see The store interface), and is **not equally atomic on the two
backends** -- this is the one place file and S3 genuinely diverge in
behavior, not just in mechanism:

- File backend: `delete_session` is a single `shutil.rmtree(session_dir)`
  call (`strands-py/src/strands/session/file_session_manager.py:191-197`).
  From the caller's perspective this is effectively all-or-nothing for a
  local disk -- it either removes the whole tree or raises.
- S3 backend: `delete_session` pages through `list_objects_v2`, collects
  every key under the session prefix, then issues `delete_objects` in
  batches of up to 1000 keys in a loop
  (`strands-py/src/strands/session/s3_session_manager.py:191-212`). **This
  is not atomic**: if the process crashes or a batch call raises partway
  through the loop, the session is left with some keys deleted and others
  present, with no recorded resume point or partial-delete marker anywhere
  in the code.

There is no per-message or per-agent delete on either backend (confirmed
above), so deletion cannot be partial by design at that granularity -- only
by accident, in the S3 multi-batch case.

**Multi-host / multi-process behavior is not a designed-for path.** Nothing
in either backend detects a crashed writer, coordinates across hosts, or
handles a network filesystem specially. The file backend assumes a local (or
at least POSIX-semantics) filesystem: `os.replace` is atomic on a genuine
local filesystem but its atomicity on a network mount depends on that mount's
semantics, which the code does not check or document (inference). The S3
backend is inherently multi-host-capable as a side effect of being an HTTP
API, but the SDK does nothing to add coordination beyond what was described
under Write and append path -- no leader election, no lease, no lock object.

## What this implies for our Session Store (our inference)

Strands' interface is a useful negative example as much as a positive one.
Two things are worth taking directly, and three are worth treating as
warnings:

**Worth adopting:**

- **Design the store so it never needs a true append against a backend that
  can't append.** Strands sidesteps S3's lack of an append primitive
  entirely by keying one message to one whole object. If our event store
  ever needs an object-store-backed tier, the same move -- one event per
  object, keyed by sequence, rather than one growing log file -- removes the
  entire "how do you append to S3" problem instead of solving it.
- **Separate positional identity (storage key) from content identity
  (durable id).** `message_id` (position) versus `tracking_id` (UUID,
  content-stable across copy/restore) is a clean value-object split we
  should keep: our own stream position and our own event/entry id should
  never be conflated into one field.

**Worth treating as a warning:**

- **A pluggable interface with optional methods that raise
  `NotImplementedError` is a soft contract, not a hard one.** `sync_multi_agent`,
  `initialize_multi_agent`, and the three bidi methods are all "abstract in
  spirit, concrete in practice" -- a conformance test suite could still pass
  against a repository that silently can't do half of what the interface
  implies. Our Session Store's contract should keep genuinely optional
  capabilities out of the same interface as required ones, or gate them
  behind an explicit capability check rather than a raised exception.
- **No version field on the durable record is a real gap, not just an
  early-stage gap.** Strands versions its two *transient* mechanisms
  (`Checkpoint`, `Snapshot`) but not the actual durable `Session`/`SessionAgent`/
  `SessionMessage` types, and papers over drift by silently dropping unknown
  fields. Our Session Store should put the version field exactly where
  Strands put it in the wrong place: on the durable record, not the
  in-memory pause marker.
- **"Compaction" that only changes what is read, never what is stored, is
  not retention.** Strands' `removed_message_count` offset is a legitimate
  compaction technique for the model-visible window, but by itself it
  guarantees unbounded storage growth with no counterpart deletion
  mechanism. If our Session Store adopts an offset-based compaction view, it
  needs a paired retention/GC story from day one -- Strands does not have one
  at this commit, and it shows in the "delete is not part of the abstract
  interface" finding above.

Separately, `team/designs/0014-storage.md` (repo-root, dated 2026-06-29,
status "Proposed") is the Strands team's own acknowledgment of a version of
this same critique: it describes the current per-subsystem interface model
(including the `SessionManager`/`SessionRepository` split studied here,
referred to there as a "`SnapshotStorage` interface (6 methods)",
`team/designs/0014-storage.md:17`) as one that "requires per-subsystem
migration" and proposes collapsing session storage, memory, context, and
transcripts onto one four-method `put`/`get`/`delete`/`list` primitive
(`team/designs/0014-storage.md:93-133,191-200`). At the pinned commit this is
still a proposal -- the code in `strands-py/src/strands/session/` matches the
older, richer, per-subsystem interface documented above, not the unified one.
This is independent, primary-source confirmation that Strands' own team
considers the interface-first, five-separate-abstractions design (of which
sessions are one) a cost worth re-architecting, which is a useful signal
about how much weight to put on any one product's current interface shape
when designing ours.

## Open questions

- **The TypeScript SDK (`strands-ts/src/session/`) was not read for this
  dossier.** `strands-ts/test/integ/session-manager.test.node.ts` and
  `strands-ts/src/session/__tests__/session-manager.test.ts` exist and imply
  a parallel implementation; whether it shares the same on-disk/S3 layout,
  the same lack of append/CAS, and the same multi-agent restrictions as the
  Python SDK studied here is unverified.
- **Whether `os.replace`'s atomicity holds on the network filesystems some
  deployments might put `storage_dir` on** (NFS, SMB) was not verified from
  source; the code neither checks for nor documents this.
- **Whether S3's read-after-write / list-after-write consistency (a platform
  property, not something asserted in this SDK's code) is relied upon
  anywhere implicitly** -- e.g., whether a `list_messages` call immediately
  following a batch of `create_message` writes could ever observe a stale
  listing. Given S3's current strong consistency guarantees this is likely
  moot in practice, but the SDK code itself makes no assertion about it
  either way, so this is inference, not a sourced claim.
- **Whether the multi-agent session-persistence restriction
  ("Session persistence is not supported for Graph/Swarm agents yet.") is
  planned to be lifted**, and if so what the intended parent-child durability
  model would be, is not stated anywhere in the source read for this
  dossier -- the error strings' "yet" is suggestive but not a commitment.
- **Whether `team/designs/0014-storage.md`'s unified `Storage` proposal has
  since landed** post-commit is unknown; at the pinned commit it is
  explicitly "Proposed," not implemented, and the session code does not
  reflect it.
- **Whether any production deployment guidance exists for choosing S3 over
  file storage given the atomicity/consistency differences documented
  above** (e.g., official docs recommending against S3 for high-concurrency
  multi-writer scenarios) was not found in the source tree searched; if such
  guidance exists it likely lives in hosted documentation outside this
  repository clone.

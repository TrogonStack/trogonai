# Claude Agent SDK and Claude Code: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-07-23. Version-sensitive claims were checked
against these authoritative anchors:

- Claude Agent SDK documentation,
  [Persist sessions to external storage](https://code.claude.com/docs/en/agent-sdk/session-storage)
  (the `SessionStore` contract, dual-write behavior, delivery semantics, fork,
  compaction, retention).
- Claude Code documentation,
  [Manage sessions](https://code.claude.com/docs/en/sessions)
  (on-disk transcript location and encoding, `--continue`/`--resume`/`--from-pr`,
  the session picker, naming, branching, `/export`).
- Claude Code documentation,
  [Checkpointing](https://code.claude.com/docs/en/checkpointing)
  (rewind, file snapshots, what is and is not tracked).
- The exported types `SessionStore`, `SessionKey`, `SessionStoreEntry`, and
  `SessionSummaryEntry` from `@anthropic-ai/claude-agent-sdk` (TypeScript) and
  `claude_agent_sdk` (Python).
- Direct inspection of the local Claude Code store under `~/.claude/`
  (transcript JSONL layout, entry envelopes, subagent sidecars, file-history
  blobs, the running-session registry), retrieved 2026-07-23. On-disk examples
  below use synthetic keys and ids; only structure is reproduced.

> **Two surfaces, one model.** Claude Code owns the *authoritative* durable
> store: append-only JSONL transcripts on the local filesystem. The Agent SDK's
> `SessionStore` is a *pluggable mirror* of that same stream to an external
> backend. The SDK doc states it plainly: "The store is a mirror, not a
> replacement. The Claude Code subprocess always writes to local disk first;
> the SDK then forwards each batch to `append()`." This dossier documents the
> on-disk store first (the source of truth) and then the mirror contract, since
> the mirror's shape is derived from the on-disk one.

## The storage model

**Authoritative store (Claude Code, local):** an append-only JSONL transcript
per session. From the sessions doc: "By default, transcripts are stored as
JSONL at `~/.claude/projects/<project>/<session-id>.jsonl` ... Each line is a
JSON object for a message, tool use, or metadata entry. The entry format is
internal to Claude Code and changes between versions, so scripts that parse
these files directly can break on any release."

The on-disk tree observed under `~/.claude/`:

```text
~/.claude/
  projects/<projectKey>/<sessionId>.jsonl                     main transcript (append-only)
  projects/<projectKey>/<sessionId>/subagents/agent-<id>.jsonl        subagent transcript
  projects/<projectKey>/<sessionId>/subagents/agent-<id>.meta.json    subagent metadata sidecar
  projects/<projectKey>/<sessionId>/subagents/workflows/wf_<id>/agent-<id>.jsonl   workflow sub-session
  file-history/<sessionId>/<contentHash>@v<n>                 checkpoint file-content blobs
  sessions/<pid>.json                                         live running-session registry
  history.jsonl                                               global prompt-input history
```

- **Source of truth is the log, projections are derived.** The `.jsonl`
  transcript is authoritative and append-only. The running-session registry
  (`sessions/<pid>.json`), the AI-generated title, the message-chain view the
  model sees, the checkpoint file snapshots, and the session picker's listing
  are all derived from or maintained alongside the log, not the primary record.
- **The SDK mirror is not the source of truth during a run.** "By default, the
  SDK writes session transcripts to JSONL files under `~/.claude/projects/` on
  the local filesystem. A `SessionStore` adapter lets you mirror those
  transcripts to your own backend ... so a session created on one host can be
  resumed on another."
- **Stated motivations for the mirror:** multi-host deployments (serverless,
  autoscaled workers, CI runners with no shared filesystem), durability across
  container restarts, and compliance ("Keep transcripts in storage you already
  govern, with your own retention rules, encryption, and access controls.").
- **Conceptual model: session-as-log materialized as session-as-directory.**
  The durable session is an ordered, append-only log of JSON entries keyed by
  working directory + session UUID, with subagent transcripts nested under a
  per-session subdirectory and out-of-band file-content blobs stored
  separately. There is no server-side session resource; identity is the
  `sessionId` UUID that names the file.

## Keying and identity

The address of one transcript is `projectKey / sessionId / subpath`:

- **`projectKey`** is "your working directory path with non-alphanumeric
  characters replaced by `-`" (sessions doc). The SDK generalizes this to "a
  stable, filesystem-safe encoding of the working directory." So the absolute
  cwd is flattened into a single directory-name segment; the key literally
  encodes location on disk.
- **`sessionId`** is "the session UUID" (a random v4-shaped UUID observed on
  disk as the `.jsonl` filename). It is client-side (Claude Code) assigned, not
  server-assigned, and the scheme encodes neither ordering nor location;
  ordering comes from position in the log, and location comes from the
  `projectKey` directory it lives in.
- **`subpath`** is set "when the entry belongs to a subagent transcript or
  sidecar file rather than the main conversation ... it follows the on-disk
  layout, for example `subagents/agent-<id>`. When `subpath` is undefined the
  key refers to the main transcript."

Hierarchy is project -> session -> subpath. Every store operation is scoped to
one `projectKey`; there is no cross-project enumeration in the SDK contract.

**Scoping and relocation (Claude Code specifics):**

- Listing/resume is scoped to the current project directory. "session ID lookup
  is scoped to the current project directory and its git worktrees, so a
  session created elsewhere reports `No conversation found with session ID:
  <session-id>`." The picker can widen: `Ctrl+W` to "all worktrees of the
  repository," `Ctrl+A` to "every project on this machine."
- **Relocation is handled by moving storage, not by rewriting identity.** "From
  v2.1.169, moving a session with `/cd` relocates it to the new directory's
  project storage, so it appears in that directory's picker afterward." A moved
  session "stays out of the old directory's picker even after a crash or forced
  exit" (v2.1.196+). Because `projectKey` is derived from cwd, a directory move
  is reconciled by physically re-homing the transcript under the new
  `projectKey`.
- Worktrees of the same repo are treated as siblings for lookup: "Selecting a
  session from another worktree of the same repository resumes it in place."

## The store interface

Two contracts exist. First the **pluggable SDK interface** (verbatim exported
type), then the **reconstructed on-disk contract** that Claude Code itself uses
when no store is attached.

### SDK `SessionStore` (verbatim, pluggable)

TypeScript, as exported from `@anthropic-ai/claude-agent-sdk`:

```typescript
// Exported from @anthropic-ai/claude-agent-sdk as
// SessionStore, SessionKey, SessionStoreEntry, SessionSummaryEntry.

type SessionKey = {
  projectKey: string;
  sessionId: string;
  subpath?: string;
};

type SessionStore = {
  // Required
  append(key: SessionKey, entries: SessionStoreEntry[]): Promise<void>;
  load(key: SessionKey): Promise<SessionStoreEntry[] | null>;

  // Optional
  listSessions?(
    projectKey: string,
  ): Promise<Array<{ sessionId: string; mtime: number }>>;
  listSessionSummaries?(projectKey: string): Promise<SessionSummaryEntry[]>;
  delete?(key: SessionKey): Promise<void>;
  listSubkeys?(key: {
    projectKey: string;
    sessionId: string;
  }): Promise<string[]>;
};

type SessionSummaryEntry = {
  sessionId: string;
  mtime: number;
  data: Record<string, unknown>;
};
```

Python mirrors the same protocol (`claude_agent_sdk`), with snake_case fields,
`SessionStore` as a `Protocol`, and optional methods that are "omitted or raise
`NotImplementedError`":

```python
class SessionKey(TypedDict):
    project_key: str
    session_id: str
    subpath: NotRequired[str]

class SessionStore(Protocol):
    # Required
    async def append(self, key: SessionKey, entries: list[SessionStoreEntry]) -> None: ...
    async def load(self, key: SessionKey) -> list[SessionStoreEntry] | None: ...

    # Optional — omit or raise NotImplementedError
    async def list_sessions(self, project_key: str) -> list[SessionStoreListEntry]: ...
    async def list_session_summaries(self, project_key: str) -> list[SessionSummaryEntry]: ...
    async def delete(self, key: SessionKey) -> None: ...
    async def list_subkeys(self, key: SessionListSubkeysKey) -> list[str]: ...

class SessionSummaryEntry(TypedDict):
    session_id: str
    mtime: int
    data: dict[str, Any]
```

Method contract (from the doc's table):

| Method | Required | Called when |
| --- | --- | --- |
| `append` | Yes | "After each batch of transcript entries is written locally. Entries are JSON-safe objects, one per line in the local JSONL." |
| `load` | Yes | "Before the subprocess spawns when `resume` is set, and once per session when listing falls back from `listSessionSummaries`. Return `null` if the session is unknown." |
| `listSessions` | No | "By `listSessions({ sessionStore })` and by `query()`/`startup()` with `continue: true`. If undefined, `continue: true` throws, and `listSessions({ sessionStore })` throws unless `listSessionSummaries` is implemented." |
| `listSessionSummaries` | No | "By `listSessions({ sessionStore })` to read metadata for all sessions in one call. Maintain the summaries inside `append`. If undefined, listing falls back to `listSessions` plus a per-session `load`." |
| `delete` | No | "By `deleteSession({ sessionStore })`. Deleting the main key (no `subpath`) must cascade to all subkeys for that session and also remove the session's summary entry ... If undefined, deletion is a no-op, which suits append-only backends." |
| `listSubkeys` | No | "During resume, to discover subagent transcripts. If undefined, only the main transcript is restored." |

`importSessionToStore()` (TypeScript) migrates an existing local session into a
store; `InMemorySessionStore` ships in both SDKs for development and testing.

### Reconstructed on-disk contract (Claude Code, no store attached)

This is not an exported type; it is reconstructed from the on-disk layout and
documented behavior. Each operation names its effect against `~/.claude/`:

| Operation | Effect | Ordering / consistency |
| --- | --- | --- |
| append entry | Append one JSON line to `projects/<projectKey>/<sessionId>.jsonl` (or the subagent `.jsonl` for a child) | Positional line order is the total order; no sequence number field is required for ordering |
| read/resume | Read the full `.jsonl`, parse each line, rebuild the message chain via `parentUuid`/`uuid` links | Full ordered read; the model-visible view is the linked chain, not every line |
| list | Enumerate `.jsonl` files under the current `projectKey` directory (and, on widen, other project dirs) | Directory scan; per-project by default |
| checkpoint file state | On each user prompt, snapshot edited files; write a `file-history-snapshot` entry into the log and content blobs under `file-history/<sessionId>/` | Keyed to the prompt's `messageId`; keeps the 100 most recent checkpoints |
| rename / title | Set a name (`--name`, `/rename`) or an AI-generated title (`ai-title` entry) | In-log metadata entry; last-writer view |
| branch / fork | Copy the transcript to a new `sessionId`, switch the process to write to it | New identity; original left intact on disk |
| delete / retire | Sweep transcripts older than `cleanupPeriodDays` (default 30) | Time-based cleanup, not per-call delete |

The reader should come away knowing the complete operation set from either
contract without inferring it from the narrative below.

## Write and append path (ordering, durability, concurrency, delivery)

**On-disk (authoritative):**

- **Append per line.** A new turn/entry is a new JSON line appended to the
  session's `.jsonl`. "Sessions are saved continuously to local transcript
  files as you work." No full rewrite, no compare-and-swap on the transcript
  file itself.
- **Ordering** is positional (line order in the file). Each entry additionally
  carries a `timestamp` and a `uuid`, and message entries carry a `parentUuid`
  that threads the conversation chain independently of raw line order.
- **Concurrency is multi-writer with no lock.** "If you resume the same session
  in two terminals without forking, messages from both interleave into one
  transcript." Claude Code offers `/branch` / `--fork-session` as the safe
  alternative to divergent concurrent writers.
- **Durability/atomicity** of individual line writes is not documented; the
  format is declared internal and version-volatile. Treated as an open
  question (temp-file-and-rename vs. plain append, fsync, torn-line healing are
  not stated).

**SDK mirror (best-effort, at-least-once):**

- **Dual-write, local first.** "The Claude Code subprocess always writes to
  local disk first; the SDK then forwards each batch to `append()`." Combining
  `sessionStore` with `persistSession: false` throws, and combining it with
  file checkpointing (`enableFileCheckpointing`) throws, "since file-history
  backup blobs are written directly to local disk and are not mirrored to the
  store."
- **Retry and drop.** "If `append()` rejects, the SDK retries the batch up to
  two more times with a short backoff, for at most three attempts in total. A
  call that times out isn't retried, since the original call may still land. If
  the batch still fails, the error is logged, a `{ type: "system", subtype:
  "mirror_error" }` message is emitted into the iterator, the batch is dropped,
  and the query continues."
- **Idempotence is the adapter's job.** "Because a retried batch can re-deliver
  entries that already landed, deduplicate by `entry.uuid` in your `append()`
  implementation." There is no expected-position precondition on `append`, so
  no optimistic-concurrency surface at the store boundary.

## Read and resume path

- **On-disk resume** rebuilds the conversation from the full transcript.
  Third-party and doc descriptions agree the entire message history is
  deserialized and the linked chain restored; tool results remain in context.
  A resumed session also restores model, agent, permission mode (except `plan`
  and `bypassPermissions`), active goal, and non-expired scheduled tasks, per
  the sessions doc.
- **SDK resume reads the store, not the filesystem.** "`load` runs before the
  subprocess spawns when `resume` is set." Resume also calls `listSubkeys` to
  restore subagent transcripts; "without it, only the main transcript is
  materialized."
- **No entry-level pagination.** `load(key)` returns the entire transcript in
  one call; nothing in the contract bounds transcript size or offers a cursor.
- **The model-visible view is materialized lazily via the chain, not the raw
  log.** `getSessionMessages({ sessionStore })` "returns the linked message
  chain the agent would see on resume," which after compaction is far smaller
  than the raw entry count (see Compaction).

## Listing, summaries, and search

- **Picker listing is a directory scan** over the current `projectKey`, with
  widen-to-worktree (`Ctrl+W`) and widen-to-machine (`Ctrl+A`) modes. Each row
  shows name/title, time since last activity, git branch, and file size;
  `Ctrl+B` filters by branch, and pasting a PR URL finds the session that
  created it (`--from-pr`, backed by `pr-link` entries in the log).
- **AI-generated titles are a derived read model.** "If you don't name a
  session, Claude Code generates a session title for it: a short summary of your
  first prompt, written by a background request to the small/fast model,
  normally a Haiku-class model." On disk this surfaces as an `ai-title` control
  entry in the log.
- **Running-session registry** lives at `~/.claude/sessions/<pid>.json`, keyed
  by process id, holding `{pid, sessionId, cwd, name, nameSource, status, kind,
  entrypoint, startedAt, updatedAt, ...}`. This is the liveness/agent-view
  projection (`claude agents --json`), separate from the durable transcript.
- **SDK summaries** are a per-project read model. `listSessionSummaries`
  "read[s] metadata for all sessions in one call." The SDK exports a pure fold
  to maintain it: "Build the entries by calling the exported `foldSessionSummary`
  helper ... on each batch inside `append`." "Skip batches whose key has a
  `subpath`." "The fold never sets `mtime`: stamp it at persist time." `data`
  "is opaque SDK-owned state; persist it verbatim." "Concurrent `append` calls
  for the same session can race on the sidecar, so serialize the read-fold-write
  with a transaction, a compare-and-swap, or a per-session lock; the fold itself
  is pure."
- **Search** is not a separate indexed subsystem. The picker filters the
  scanned list in memory (name/title/first-prompt, branch, PR URL); there is no
  documented FTS/vector index over transcripts.

## Entry/message structure and versioning

Entries are line-delimited JSON. Two broad classes appear in the transcript.

**Conversation entries** (`type: "user"` / `"assistant"`) share an envelope
observed on disk:

```text
{ parentUuid, uuid, sessionId, type, timestamp, cwd, gitBranch,
  version, isSidechain, userType, entrypoint, message,
  // user adds:      promptId, permissionMode
  // assistant adds: requestId }
```

- `message` is the Anthropic API message payload. For assistant entries it
  carries `model, id, type, role, content, stop_reason, stop_sequence,
  stop_details, usage` plus internal fields (`diagnostics`, occasionally
  `container`, `context_management`); for user entries, `role` and `content`.
- The conversation is threaded by `parentUuid` -> `uuid` links (a chain, not
  just line order). `isSidechain` distinguishes subagent/sidechain turns from
  the main thread.

**Control / metadata entries** (top-level `type` tags, not part of the message
chain) observed on disk include: `last-prompt` (a `leafUuid` pointer to the
current chain tip), `mode` / `permission-mode`, `queue-operation`, `ai-title`,
`pr-link` (`{prNumber, prRepository, prUrl}`), `attachment`, `system`
(`subtype`, hook info, `toolUseID`), `agent-name`, `file-history-snapshot`, and
occasionally `custom-title` / `frame-link`.

- **Opaque to the store, interpreted by the SDK/CLI.** To an adapter,
  `SessionStoreEntry` is "a `{ type: string; ... }` object": "Treat them as
  opaque JSON-safe values: persist them in order and return them from `load` in
  the same order." "`load` must return entries that are deep-equal to what was
  appended; byte-equal serialization is not required, so backends like Postgres
  `jsonb` that reorder object keys are fine." The one field an adapter relies on
  is `uuid` (for dedup).
- **Versioning.** Each conversation entry carries a `version` field (the Claude
  Code version that wrote it). The store format "is internal to Claude Code and
  changes between versions, so scripts that parse these files directly can break
  on any release." There is no stable, documented schema-version ratchet for
  `SessionStoreEntry` beyond `{ type: string }`; evolution is by additive fields
  and version-sniffing, and it is one-way (old readers may not understand newer
  entries).

## Compaction and history management

- **Compaction shrinks the model-visible view, not the durable log.** "After
  auto-compaction, earlier turns are replaced by a summary, so a session whose
  store holds 503 raw entries may return 18 messages from `getSessionMessages`.
  For the full raw history, including pre-compaction turns and metadata entries,
  call `store.load(key)` directly."
- **Compaction is an upstream (SDK/CLI) concern, not a store concern.** The
  durable log keeps every raw entry; the summary is another appended entry. On
  disk a compaction boundary appears as a `user` entry with
  `isCompactSummary: true` and `isVisibleInTranscriptOnly: true`, chained into
  the thread via `parentUuid`; the pre-compaction turns remain in the file.
- **Targeted summarize.** `/rewind`'s "Summarize from here" / "Summarize up to
  here" and `/compact` compress a span into an AI summary. "In both cases the
  original messages are preserved in the session transcript, so Claude can
  reference the details if needed." Replay across a compaction boundary
  therefore sees the summary in the chain while the raw turns stay on disk.

## Rewind, checkpoints, and fork

- **Rewind is a non-destructive view operation over an append-only log.** The
  rewind menu offers "Restore code and conversation," "Restore conversation,"
  "Restore code," and the two summarize options. Restoring conversation moves
  the chain tip back; the raw entries remain in the transcript.
- **File checkpoints are content snapshots, not diffs, tied to prompts.** "Every
  user prompt creates a new checkpoint." "Claude Code keeps file snapshots for
  the 100 most recent checkpoints in a session. Discarding an older checkpoint
  deletes the snapshot files that no remaining checkpoint references, except
  each file's first snapshot." On disk this is a `file-history-snapshot` log
  entry — `{messageId, snapshot: {messageId, timestamp, trackedFileBackups},
  isSnapshotUpdate}` where `trackedFileBackups` maps each tracked path to
  `{backupFileName, version, backupTime}` — plus content blobs at
  `~/.claude/file-history/<sessionId>/<contentHash>@v<n>`. "Checkpoints are
  saved with the conversation, so a resumed session can still `/rewind` to
  them." These blobs "are written directly to local disk and are not mirrored
  to the store."
- **What checkpointing does not track:** bash-command file changes, subagent
  edits (except a foreground `context: fork` skill), external/concurrent-session
  edits, and symlinked/hard-linked paths. "Checkpoints complement but don't
  replace proper version control."
- **Fork rewrites identity, not a byte copy.** CLI `/branch` (or
  `--fork-session`) "creates a copy of the conversation so far and switches you
  into it, leaving the original intact"; both get their own session IDs and
  appear as separate picker rows (grouped when the picker finds duplicates).
  The SDK is explicit: "`forkSession({ sessionStore })` reads the source
  entries, rewrites every `sessionId` field and remaps message UUIDs, then
  appends the transformed entries under a new key. An adapter-level copy or
  `CopyObject` shortcut would produce a transcript that still references the old
  session ID, so the SDK does not use one." Lineage is the shared prefix content
  plus the new/old session IDs printed on branch.

## Subagents and nested sessions

- **Nested under the parent, as sibling files in a per-session directory.**
  Subagent transcripts are mirrored under `subpath: "subagents/agent-<id>"`; on
  disk they live at
  `projects/<projectKey>/<sessionId>/subagents/agent-<id>.jsonl` with a
  `agent-<id>.meta.json` sidecar (`{agentType, description, toolUseId}`). Deeper
  nesting is observed for workflow orchestration:
  `subagents/workflows/wf_<id>/agent-<id>.jsonl`.
- **Durable parent-child link** is the on-disk containment (child files live
  inside the parent session's directory) plus the `toolUseId` in the child's
  meta sidecar that ties the child to the parent tool call that spawned it. The
  child has its own isolated transcript, not entries inlined in the parent.
- **SDK surface:** "`listSubagents({ sessionStore })` requires the adapter to
  implement `listSubkeys`; `getSubagentMessages({ sessionStore })` uses it when
  available but falls back to the direct subpath when it is undefined."
- **Cascade on delete.** In the SDK, "Deleting the main key (no `subpath`) must
  cascade to all subkeys for that session." On disk, the subagent files are
  inside the session directory, so removing the session removes its children.
  Orphan/crash reconciliation of child sessions is not separately documented.

## Retention, deletion, and multi-host

- **Product owns retention; the store does not enforce it.** Locally, "Change
  the 30-day retention" via `cleanupPeriodDays`; local transcripts under
  `CLAUDE_CONFIG_DIR` "are swept independently by the `cleanupPeriodDays`
  setting." Writes can be suppressed with `CLAUDE_CODE_SKIP_PROMPT_HISTORY` or,
  per non-interactive run, `--no-session-persistence`.
- **SDK delete never happens on its own.** "The SDK never deletes from your
  store on its own. Retention is the adapter's responsibility: implement TTLs,
  S3 lifecycle policies, or scheduled cleanup." `delete` is optional; when
  undefined "deletion is a no-op, which suits append-only backends."
- **Multi-host is a first-class motivation for the mirror.** Local storage
  assumes a single filesystem; "Serverless functions, autoscaled workers, and
  CI runners don't share a filesystem. A shared store lets any replica resume
  any session." The `sessions/<pid>.json` registry is per-process/per-host
  liveness state, not a cross-host coordinator. Cross-host correctness of the
  mirror hinges on adapter-side ordering (see the S3 clock-skew caveat below).

## Reference adapters (examples/session-stores)

"Reference implementations. Not published to npm ... copy the `src/` file you
need into your project." Each "passes the ... conformance suite."

| Adapter | Backend client | Storage model |
| --- | --- | --- |
| `S3SessionStore` | `@aws-sdk/client-s3` | "One JSONL part file per `append()`; `load()` lists, sorts, and concatenates." |
| `RedisSessionStore` | `ioredis` | "`RPUSH`/`LRANGE` list per transcript, plus a sorted-set session index." |
| `PostgresSessionStore` | `pg` | "One row per entry in a `jsonb` table, ordered by `BIGSERIAL`." |

Conformance: TypeScript vendors `examples/session-stores/shared/conformance.ts`
and runs `runSessionStoreConformance(factory)` ("the factory must return a
fresh, isolated store on every call"); Python ships
`claude_agent_sdk.testing.run_session_store_conformance(MyStore)`. "Tests for
optional methods skip automatically when those methods are not implemented."
The S3 adapter's documented failure mode is instructive for our design:
part-file ordering uses the client wall clock, so "Multiple writer instances
with clock skew >1s may produce out-of-order `load()` results. Use NTP or a
single writer per session."

## Supported operations

TypeScript functions that accept `sessionStore` and operate against the store
instead of the local filesystem: `query()`, `startup()`, `listSessions()`,
`getSessionInfo()`, `getSessionMessages()`, `renameSession()`, `tagSession()`,
`deleteSession()`, `forkSession()`, `listSubagents()`, `getSubagentMessages()`.

Python: `session_store` in `ClaudeAgentOptions` for `query()`, plus store-backed
standalone functions (`list_sessions_from_store()`,
`get_session_info_from_store()`, `get_session_messages_from_store()`,
`list_subagents_from_store()`, `get_subagent_messages_from_store()`,
`rename_session_via_store()`, `tag_session_via_store()`,
`delete_session_via_store()`, `fork_session_via_store()`). "`startup()` has no
Python equivalent."

## What this implies for our Session Store (our inference)

Both surfaces converge on the same shape: **a stored session is an append-only,
per-key log of opaque JSON entries plus derived projections (a summary/read
model, a message-chain view, a file-content checkpoint store, and a liveness
registry) rebuilt from that log.** That is within one small step of an
event-sourced Session Store, and the SDK interface is already stream-shaped:

- `append(key, entries)` is an ordered append to one logical stream per
  `SessionKey`; `load(key)` is a full ordered read. The contract never asks for
  random access, mutation, or in-place rewrite. This is our event stream and
  full-replay read.
- Entries are opaque to the store by design (the host owns durability,
  ordering, listing, retention; the SDK owns message semantics). That matches an
  event envelope whose payload the store never inspects, with `uuid` as the
  idempotency key.
- Projections are explicit and rebuildable: `listSessionSummaries` maintained by
  a pure `foldSessionSummary` at append time is a classic projection, and its
  documented sidecar race (fix: transaction, CAS, or per-session lock) is the
  classic projection-update concurrency problem. The AI title, the message
  chain, and the picker list are likewise derived.
- Compaction, rewind, and fork are all expressed above the log without
  destroying it: compaction appends a summary marker (`isCompactSummary`),
  rewind moves a view pointer, fork rewrites identity into a new stream. An
  event-sourced backing models these as markers/new streams, not destructive
  edits — exactly our design goal.
- Delivery is at-least-once with client-generated `uuid`, so our backing needs
  idempotent append rather than exactly-once transport.

**Gaps we must close that this product leaves open.** There is no expected-
position precondition on `append` (no optimistic-concurrency surface), and
Claude Code explicitly tolerates multi-writer interleaving into one transcript.
An event-sourced store that wants single-writer-per-stream or optimistic
concurrency is *adding* a guarantee neither surface provides. `load` has no
cursor or size bound. `SessionStoreEntry` has no versioned schema. And file-
history blobs are deliberately *outside* the mirrored stream, so a faithful
event-sourced session that also wants checkpoint file state must model a second
content-addressed store alongside the event log.

## Open questions

- Durability/atomicity of the local JSONL append (temp-file-and-rename, fsync,
  torn-line healing) is undocumented; the format is declared internal and
  version-volatile.
- `SessionStoreEntry` has no versioned schema; how should a durable store handle
  entry-shape evolution across SDK/CLI versions it did not write?
- `mtime` must "share a clock source" between `listSessions` and the summary
  sidecar; the S3 adapter shows the failure mode (client wall-clock skew
  reorders parts). Who owns the clock in a multi-writer deployment?
- The store never sees which entries survive compaction; raw history grows
  unbounded unless the adapter imposes retention the SDK cannot observe. What
  listing/resume behavior is expected after adapter-side truncation?
- `load` returns the entire transcript in one call; nothing bounds transcript
  size or offers incremental reads for resume.
- Multi-writer interleaving is documented as *tolerated* for local resume in two
  terminals; there is no reconciliation or conflict model beyond "use `/branch`
  to fork." How should a multi-host mirror behave when two hosts append to the
  same key concurrently?

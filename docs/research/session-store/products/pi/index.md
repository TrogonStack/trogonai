# Pi: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of
[earendil-works/pi](https://github.com/earendil-works/pi) at commit
`a96fb984d8c8b065fc5d193309fc812a882adee0` (committed 2026-08-03 22:50:31
+0000, "chore: approve contributors from issue #7554"), MIT licensed
(`LICENSE`, copyright Mario Zechner). Retrieved and verified 2026-08-04. The
repository was previously named `pi-mono`; in-repo docs still link to
`github.com/earendil-works/pi-mono/blob/main/...`, which is the pre-rename
name of this same repository, not a separate product. Citations below are
`path:line` against the pinned commit unless otherwise noted.

Anchors used, by package:

- `@earendil-works/pi-coding-agent` (`packages/coding-agent/`) -- the shipped
  `pi` CLI/TUI. Its own hand-rolled session store:
  `src/core/session-manager.ts`, `src/core/messages.ts`,
  `src/core/compaction/{compaction.ts,branch-summarization.ts}`,
  `src/modes/interactive/components/session-selector.ts`,
  `docs/session-format.md`, `docs/sessions.md`, `docs/compaction.md`,
  `scripts/migrate-sessions.sh`.
- `@earendil-works/pi-agent-core` (`packages/agent/`) -- a separate, pluggable
  SDK-level session abstraction: `src/harness/types.ts`,
  `src/harness/session/{repository.ts,jsonl-repo.ts,memory-repo.ts,
  array-session-index.ts,search.ts,session.ts,keyed-operation-queue.ts}`,
  `docs/harness.md`, `docs/harness-v2.md`, `test/harness/{session.test.ts,
  session-backends.test.ts}`.
- `@earendil-works/pi-storage-sqlite-node` (`packages/storage/sqlite-node/`)
  -- a third, SQLite-backed implementation of the same pluggable interface:
  `src/sqlite/{repo.ts,search-backend.ts,migrations.ts,storage/*.ts}`.
- `@earendil-works/pi-client` / `@earendil-works/pi-protocol`
  (`packages/client/`, `packages/protocol/`) -- a wire-protocol client for
  attaching to a *live, running* session over a byte stream; not a storage
  layer, but relevant to the multi-process story.
- `@earendil-works/pi-server` (`packages/server/`) -- the counterpart server
  for the above (`src/sessions.ts`), not read in depth for this dossier.
- `@earendil-works/pi-ai` (`packages/ai/`) -- base `Message`/`Usage` types.

**The single most consequential finding, load-bearing for every section
below:** the monorepo ships **two independent, non-integrated session
storage implementations**, plus a third that exists but is wired into
neither shipped product. The `pi` CLI's `SessionManager`
(`packages/coding-agent/src/core/session-manager.ts`) does not use, import,
or know about the pluggable `SessionRepository`/`SessionStorage` interface
from `pi-agent-core`. This is not inference: `coding-agent/package.json:46`
depends on `@earendil-works/pi-agent-core`, but every import site in
`packages/coding-agent/src` pulls only agent-loop primitives
(`Agent`, `AgentMessage`, `AgentState`, `AgentTool`, `ThinkingLevel`,
`setDefaultStreamFn`, `StreamFn`) -- never `SessionRepository`,
`JsonlSessionRepository`, `InMemorySessionRepository`, or anything under
`harness/session`. The checked-in format spec
(`packages/coding-agent/docs/session-format.md`) documents the CLI's format
but, in at least one place (compaction's `retainedTail` field, see
"Entry/message structure and versioning" below), quietly describes a field
the CLI never writes and only the harness's implementation produces -- the
doc conflates the two systems. Everywhere below, "the CLI" and "the harness"
are called out explicitly because their answers to the same research
question frequently differ.

## The storage model

Both systems agree on the physical shape: **one JSONL file per session**,
first line a header, every subsequent line one JSON object with a `type`
tag, entries linked into a tree via `id`/`parentId` rather than read
top-to-bottom as a flat log. The checked-in spec states this directly:
"Sessions are stored as JSONL (JSON Lines) files. Each line is a JSON object
with a `type` field. Session entries form a tree structure via `id`/`parentId`
fields, enabling in-place branching without creating new files."
(`packages/coding-agent/docs/session-format.md:3`).

Source of truth vs. derived state:

- **The CLI** (`SessionManager`): `this.fileEntries: FileEntry[]` (header +
  entries, in on-disk order) is the authoritative record -- it is what
  `_rewriteFile()` and `_persist()` write
  (`packages/coding-agent/src/core/session-manager.ts:979-989,1015-1042`).
  `this.byId`, `this.labelsById`, `this.labelTimestampsById`, and
  `this.leafId` are rebuildable indexes/projections: `_buildIndex()`
  reconstructs all four from `fileEntries` in a single forward pass every
  time a file is loaded (`session-manager.ts:957-976`). Critically,
  `this.leafId` is *not* itself persisted anywhere in the header or as a
  distinct entry type in the CLI's format -- see "Read and resume path" below
  for what this implies.
- **The harness** (`pi-agent-core`): the JSONL file (or, for
  `InMemorySessionRepository`, an in-process array) is authoritative.
  `ArraySessionIndex` (`packages/agent/src/harness/session/
  array-session-index.ts`) is an explicit, named rebuildable projection --
  its own doc comment says so: "Ordered entries and derived projections for
  array-backed session storage" (`array-session-index.ts:57`). It derives
  `leafId`, a `labelsById` map, a session `name`, and running token/cost
  `stats` from a linear scan of the entries (`applyProjection`,
  `array-session-index.ts:24-55`), and is rebuilt wholesale on every load
  via `replace()` (`array-session-index.ts:82-99`). Unlike the CLI, the
  harness's `leafId` derivation also honors a first-class `LeafEntry`
  (`type: "leaf"`) when present -- see "Rewind, checkpoints, and fork".
- **The SQLite backend** (`pi-storage-sqlite-node`): genuinely different --
  here the database *is* the source of truth in the SQL sense (rows in a
  `sessions` table and a `session_entries` table,
  `packages/storage/sqlite-node/src/sqlite/storage/sessions.ts:4-10`), and a
  parallel `session_search_fts` virtual table is an explicitly
  trigger-maintained secondary index (`search-backend.ts:35-52`), not a
  separate rebuild-from-scratch cache. This is the only one of the three
  backends with real transactional durability (`db.transaction(...)`,
  `repo.ts:87-88`) and an actual indexed full-text search subsystem.

Conceptual model: none of RESEARCH_PROMPT's categories fits cleanly.
"Session-as-transcript" undersells it (there's no single linear order to
replay -- see below), and "session-as-log" undersells the tree. The best
description is **session-as-tree-of-entries, materialized as an append-only
JSONL log whose physical (on-disk) order is not the same thing as its
logical (parent-chain) order**. Every entry still has a place in on-disk
append order (line N), but the entries a reader must assemble to reconstruct
"the current conversation" are found by walking `parentId` pointers from a
leaf back to a root, not by reading the file start-to-finish. Two different
entries appended at physically adjacent lines can belong to two different
branches with no ancestor/descendant relationship to each other.

## Keying and identity

- **File path** encodes the CWD and is where identity effectively lives for
  the CLI, since there is no session-id-indexed lookup structure: `~/.pi/
  agent/sessions/--<encoded-cwd>--/<timestamp>_<uuid>.jsonl`
  (`packages/coding-agent/docs/session-format.md:5-11`). Both the CLI and
  the harness independently implement the *same* encoding scheme in two
  places:
  - CLI, `getDefaultSessionDirPath` -- `` `--${resolvedCwd.replace(/^[/\\]/, "").replace(/[/\\:]/g, "-")}--` ``
    (`packages/coding-agent/src/core/session-manager.ts:479-482`).
  - Harness, `encodeCwd` -- the identical regex, verbatim
    (`packages/agent/src/harness/session/jsonl-repo.ts:186-188`).

  Neither module imports the other's implementation; the scheme is
  duplicated by convention rather than shared code.
- **Session id**: both systems mint a `uuidv7()` (time-ordered, so
  lexicographic/creation order coincide) -- CLI's `createSessionId()`
  (`session-manager.ts:208-210`) and the harness's `createSessionId()`
  (`packages/agent/src/harness/session/repository.ts:14-16`), both from
  `@earendil-works/pi-ai`'s re-exported `uuidv7`. `assertValidSessionId()`
  constrains a caller-supplied id to
  `^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$` (`session-manager.ts:212-219`).
- **Entry id**: the two systems use *different* minting strategies for the
  same conceptual thing. CLI's `generateId()` takes the first 8 hex
  characters of a fresh `crypto.randomUUID()`, retried up to 100 times
  against the in-memory `byId` set before falling back to a full UUID
  (`session-manager.ts:221-227`). The harness's `Session.createEntryId()`
  instead takes the last 8 characters of a `uuidv7()`, same 100-try
  collision loop and full-UUID fallback (`packages/agent/src/harness/session/
  session.ts:229-235`). Both produce short, collision-checked but
  *not globally unique* ids -- uniqueness is only guaranteed within one
  session file's `byId`/index, confirmed independently by the harness's own
  append guard: `JsonlSessionBackend.appendEntry()` throws `SessionError("invalid_entry", "Entry ${entry.id} already exists")`
  if the id is already present in that session's index (`jsonl-repo.ts:270`).
- **Listing scope**: scoped-by-default with an explicit global escape hatch,
  on both sides.
  - CLI: `SessionManager.list(cwd, sessionDir?, onProgress?)` walks one
    directory and additionally filters by header `cwd` when the caller
    supplied a non-default `sessionDir`
    (`session-manager.ts:1637-1646`, `sessionCwdMatches` at `:630-632`).
    `SessionManager.listAll(onProgress?)` instead walks every subdirectory
    under the sessions root (`session-manager.ts:~1697` onward, confirmed
    call site using `buildSessionInfosWithConcurrency` over `allFiles`
    gathered across all `--<cwd>--` directories).
  - Harness: `JsonlSessionListOptions { cwd?: string }`
    (`packages/agent/src/harness/types.ts:579-581`). When `cwd` is omitted,
    `JsonlSessionBackend.listSessions()` calls `listSessionDirs()`, which
    enumerates every directory under the sessions root
    (`jsonl-repo.ts:220-234, 421-426`) -- i.e. cross-project by default
    unless a `cwd` is passed.
- **Relocation/rename reconciliation**: there is none, beyond a one-time
  historical bugfix script. `packages/coding-agent/scripts/
  migrate-sessions.sh` exists specifically to move session files that were
  (per its own comment) "saved to `~/.pi/agent/*.jsonl`" directly, "instead
  of `~/.pi/agent/sessions/<encoded-cwd>/`" due to "the bug in v0.30.0"
  (`migrate-sessions.sh:1-6`). It re-derives the target directory from the
  header's `cwd` field via `jq` and a shell port of the same encoding regex
  (`migrate-sessions.sh:41-62`), and is a manual, one-off operator tool, not
  something the CLI or harness runs automatically. Beyond that: if a
  project's working directory is literally renamed or moved, its encoded
  session directory does not move with it, and there is no reconciliation
  path found in source -- a session opened by explicit `--session <path>`
  still works, but `pi -c` / `pi -r` / `SessionManager.continueRecent()`
  scoped to the *new* cwd path will not find it (this is inference from the
  directory-derivation code; no explicit relocation-detection logic was
  found -- see Open questions).
- **Renaming the session's display name** (distinct from the file path) is
  implemented as a new tree entry, not a file or header rewrite. The
  interactive picker's rename callback opens a *fresh* `SessionManager` on
  the target path and calls `appendSessionInfo(next)`:
  ```ts
  renameSession: async (sessionFilePath: string, nextName: string | undefined) => {
      const next = (nextName ?? "").trim();
      if (!next) return;
      const mgr = SessionManager.open(sessionFilePath);
      mgr.appendSessionInfo(next);
  },
  ```
  (`packages/coding-agent/src/modes/interactive/interactive-mode.ts:5069-5075`).
  `appendSessionInfo()` appends a `SessionInfoEntry` as a child of *that
  manager's current leaf* (`session-manager.ts:1135-1147`), and
  `getSessionName()` resolves the display name by scanning entries in
  reverse for the most recent `session_info` entry, where "empty names
  explicitly clear the session title" (`session-manager.ts:1149-1160`).
  Renaming a session that is not the active one therefore appends to
  whatever the *last persisted* leaf was, independent of any in-memory
  navigation state of the process actually running that session.

## The store interface

There is no single interface; there are three implementations of one
pluggable interface (harness) plus one separate, non-pluggable concrete
class (CLI). Per RESEARCH_PROMPT's instruction, both are captured in full.

### 1. The pluggable interface (`@earendil-works/pi-agent-core`)

Two verbatim contracts. First, the repository-level contract -- how a caller
creates, opens, lists, deletes, or forks a session:

```ts
export interface SessionRepository<
    TMetadata extends SessionMetadata = SessionMetadata,
    TCreateOptions extends SessionCreateOptions = SessionCreateOptions,
    TListOptions = void,
> extends AsyncDisposable {
    create(options: TCreateOptions): Promise<Session<TMetadata>>;
    open(metadata: TMetadata): Promise<Session<TMetadata>>;
    list(options?: TListOptions): Promise<TMetadata[]>;
    delete(metadata: TMetadata): Promise<void>;
    fork(source: TMetadata, options: SessionForkOptions & TCreateOptions): Promise<Session<TMetadata>>;
}
```
(`packages/agent/src/harness/session/repository.ts:22-32`). All five methods
are required; there is no optional method on this interface. `AsyncDisposable`
requires an `[Symbol.asyncDispose]` implementation as well, used to drain
pending writes on shutdown (see "Write and append path").

Second, the per-session storage contract -- what an opened session can do
against its own entries once a repository has produced it:

```ts
export interface SessionStorage<TMetadata extends SessionMetadata = SessionMetadata> {
    readonly metadata: TMetadata;
    /** Rejects with `invalid_session` when a non-null active leaf does not reference a stored entry. */
    readHead(): Promise<SessionHead>;
    readEntry(id: string): Promise<SessionTreeEntry | undefined>;
    readEntries(options?: SessionEntryCursorOptions): Promise<readonly SessionTreeEntry[]>;
    appendEntry(entry: SessionTreeEntry): Promise<void>;
    findEntriesOnBranch(query: SessionBranchQuery & { start: string | null }): Promise<readonly SessionTreeEntry[]>;
    readPathToRootOrCompaction(leafId: string | null): Promise<readonly SessionTreeEntry[]>;
    getLabel(id: string): Promise<string | undefined>;
    getName(): Promise<string | undefined>;
    getStats(): Promise<SessionStats>;
}
```
(`packages/agent/src/harness/types.ts:558-571`). All nine members are
required (`metadata` is a required readonly property, not a method). None
are marked optional in the source.

Supporting types referenced by the contract, also verbatim:

```ts
export interface SessionMetadata { id: string; createdAt: string; }
export interface SessionCreateOptions { id?: string; }
export interface SessionForkOptions { entryId?: string; position?: "before" | "at"; id?: string; }
export type SessionForkSelection =
    | { kind: "all" }
    | { kind: "before_user_message"; entryId: string }
    | { kind: "through_entry"; entryId: string };
export interface SessionBranchQuery {
    start?: string | null;
    stopAtType?: SessionTreeEntry["type"];
    stopAtId?: string;
    type?: SessionTreeEntry["type"];
    customType?: string;
    order?: "newestFirst" | "oldestFirst";
    limit?: number;
}
export interface SessionEntryCursorOptions { afterEntrySeq?: number; limit?: number; }
export interface SessionHead { leafId: string | null; }
```
(`packages/agent/src/harness/types.ts:481-484, 501-503, 523-556, 493-497`).

Three concrete implementations of `SessionRepository` exist in the pinned
commit:

| Implementation | Package | Backing store | Concurrency primitive |
|---|---|---|---|
| `JsonlSessionRepository` / `JsonlSessionBackend` | `pi-agent-core` | one JSONL file per session, via an injected `FileSystem` capability | `KeyedOperationQueue<string>` keyed by session path (`jsonl-repo.ts:100`) |
| `InMemorySessionRepository` / `InMemorySessionBackend` | `pi-agent-core` | `Map<string, InMemorySessionState>` in process memory | `KeyedOperationQueue<string>` keyed by session id (`memory-repo.ts:29`) |
| `SqliteSessionRepository` / `SqliteSessionBackend` | `pi-storage-sqlite-node` | SQLite database (`sessions`, `session_entries` tables), via an injected `SqliteDatabaseFactory` | a simpler `SerialOperationQueue` -- one global tail, no per-key sharding, no concurrency cap (`packages/storage/sqlite-node/src/sqlite/repo.ts:48-61`), backed by real `db.transaction(...)` calls |

Additionally there is a generic, non-indexed search adapter usable with any
of the above:

```ts
export function createScanningSessionSearch<TMetadata extends SessionMetadata, TCreateOptions extends SessionCreateOptions, TListOptions>(
    source: Pick<SessionRepository<TMetadata, TCreateOptions, TListOptions>, "list" | "open">,
): SessionSearch<TMetadata>
```
(`packages/agent/src/harness/session/search.ts:42-48`) -- see "Listing,
summaries, and search" for what it actually does, and how the SQLite
package's `SqliteSessionSearch` differs by being a real index instead.

None of these three `SessionRepository` implementations, nor the harness's
`SessionStorage`/`Session` abstraction at all, are imported anywhere under
`packages/coding-agent/src` (confirmed by grep across the whole
`coding-agent` source tree; the only `@earendil-works/pi-agent-core` imports
there are `Agent`, `AgentMessage`, `AgentState`, `AgentTool`,
`ThinkingLevel`, `setDefaultStreamFn`, `StreamFn`). The shipped `pi` CLI does
not use the pluggable interface described above at all.

### 2. The CLI's own interface (reconstructed; `SessionManager`)

`packages/coding-agent/src/core/session-manager.ts` has no exported
interface type; `SessionManager` is a single concrete class instantiated
directly. The effective operational contract, reconstructed from its public
surface (spec's own summary at
`packages/coding-agent/docs/session-format.md:386-439` cross-checked
against the class body):

| Operation | Signature (reconstructed) | Notes |
|---|---|---|
| Create | `static create(cwd, sessionDir?, options?): SessionManager` (`session-manager.ts:1519-1522`) | Constructs and defers the actual file write (see write path) |
| Open | `static open(path, sessionDir?, cwdOverride?): SessionManager` (`:1530-1548`) | Loads and, if needed, migrates the file synchronously |
| Continue most recent | `static continueRecent(cwd, sessionDir?): SessionManager` (`:1557-1564`) | Scans directory mtimes via `findMostRecentSession` |
| In-memory | `static inMemory(cwd?, options?): SessionManager` (`:1568-1570`) | `persist = false`; never touches disk |
| Fork (whole file) | `static forkFrom(sourcePath, targetCwd, sessionDir?, options?): SessionManager` (`:1579-1630`) | Copies every non-header entry verbatim; see "Rewind, checkpoints, and fork" |
| List (scoped) | `static list(cwd, sessionDir?, onProgress?): Promise<SessionInfo[]>` (`:1637-1646`) | |
| List (global) | `static listAll(onProgress?): Promise<SessionInfo[]>` | Cross-project |
| Append message | `appendMessage(message): string` | Returns new entry id |
| Append model/thinking/compaction/custom/label/name | `appendModelChange`, `appendThinkingLevelChange`, `appendCompaction`, `appendCustomEntry`, `appendCustomMessageEntry`, `appendLabelChange`, `appendSessionInfo` | All funnel through `_appendEntry` → `_persist` |
| Move leaf (in-memory only) | `branch(entryId): void` (`:1360-1365`) | No persisted marker -- see below |
| Move leaf + summary | `branchWithSummary(entryId, summary, details?, fromHook?, usage?): string` (`:1381-1409`) | Appends a `BranchSummaryEntry` |
| Reset leaf | `resetLeaf(): void` (`:1372-1374`) | Sets `leafId = null` |
| Extract branch to new file | `createBranchedSession(leafId): string \| undefined` (`:1412-1512`) | See below |
| Tree read | `getTree(): SessionTreeNode[]`, `getChildren(parentId): SessionEntry[]`, `getBranch(fromId?)`, `getEntries()`, `getEntry(id)` | `getTree` and `getChildren` are O(n) scans, not indexed |
| Context build | `buildContextEntries()`, `buildSessionContext()` | Compaction-aware; see "Compaction" |
| Info | `getSessionName()`, `getHeader()`, `getCwd()`, `getSessionId()`, `getSessionFile()`, `isPersisted()` | |

There is **no `delete()` method on `SessionManager` at all.** Deletion is
implemented entirely one layer up, in the interactive TUI's session
selector -- see "Retention, deletion, and multi-host".

Ordering/consistency guarantees for the CLI's reconstructed interface: every
mutating call above is synchronous, runs on the Node.js single-threaded
event loop, and mutates `this.fileEntries`/`this.byId`/`this.leafId` in
place before (deferred) persistence -- there is no async gap in which a
second in-process caller could interleave a conflicting mutation against the
*same* `SessionManager` instance. There is, however, no protection at all
against two different `SessionManager` instances (in the same process or a
different one) writing to the same file path concurrently -- see "Write and
append path".

## Write and append path (ordering, durability, concurrency, delivery)

**Ordering.** Both systems order entries by position in the JSONL file,
which for the CLI is simply array-push order into `fileEntries`
(`session-manager.ts:1042-1046`, `_appendEntry`) and for the harness is
`appendEntry` on the storage backend after the in-process `appendTail`
promise resolves (`session.ts:240-249`). There is no separate sequence
number field on entries in either format -- physical line position *is* the
ordering key, and `SessionEntryCursorOptions.afterEntrySeq` in the harness
interface is explicitly described as "sequence" over array index
(`packages/agent/src/harness/types.ts:493-497`), confirmed by `ArraySessionIndex.readEntries()`
slicing its in-memory array by that index (`array-session-index.ts:112-116`)
-- i.e. it is a position into the already-loaded array, not a disk offset or
a durable per-entry counter.

**CLI durability/atomicity.** Three distinct write shapes exist:

1. *Deferred first write.* `_persist(entry)` checks whether any assistant
   message exists yet in `fileEntries`; if not, and the manager has never
   flushed, the entry is held only in memory (`this.flushed` stays `false`)
   -- nothing touches disk (`session-manager.ts:1015-1024`). The comment
   elsewhere in the class explains the intent is to avoid littering the
   sessions directory with aborted, reply-less sessions. If the manager
   *has* already flushed once (e.g. a reopened session with prior history),
   entries are appended immediately even without an assistant message yet
   (`:1018-1020`).
2. *First real flush.* Once an assistant message exists and the file has
   never been flushed, `_persist` does an **exclusive create**:
   `openSync(this.sessionFile, "wx")`, then writes every accumulated entry
   with `writeFileSync`, closes, and sets `flushed = true`
   (`session-manager.ts:1026-1034`). `"wx"` fails if the path already
   exists, so this path assumes the file does not yet exist on disk.
3. *Steady-state append.* Once flushed, every subsequent entry is one
   `appendFileSync(this.sessionFile, JSON.stringify(entry) + "\n")` call
   (`:1035-1036`).

None of these three paths call `fsync`, use a lock file, or write to a
temp path and rename. The **migration rewrite** path is the sharpest
contrast: `_rewriteFile()` opens the file with the truncating flag `"w"`
(not append, not a temp+rename swap) and rewrites every entry from scratch
(`session-manager.ts:979-989`), invoked synchronously from `_setSessionFile`
whenever `migrateToCurrentVersion()` reports a migration was applied
(`:895-919`). A process crash between `openSync(path, "w")` and the final
`closeSync` would leave a truncated, partially-rewritten file -- there is no
atomicity guard here (documented as an observation, not found stated as a
known risk in-repo; see Open questions).

**Harness durability/atomicity.** `JsonlSessionBackend.appendEntry()`
performs one `fs.appendFile(metadata.path, JSON.stringify(entry) + "\n")`
call through the injected, abstract `FileSystem` capability
(`jsonl-repo.ts:257-276`), then commits the entry into the in-memory
`ArraySessionIndex` only after that write succeeds. `createDocument()` (used
by both `create()` and `fork()`) instead builds the entire file content --
header plus every initial entry, joined by `\n` -- and issues **one**
`fs.writeFile()` call (`jsonl-repo.ts:354-382`). Whether the underlying
`FileSystem.writeFile`/`appendFile` implementation is itself atomic (temp
file + rename, fsync, etc.) is not specified by the `FileSystem` interface
itself (`packages/agent/src/harness/types.ts:291-341`, which only documents
"Create or overwrite a file, creating parent directories when supported")
and was not traced to a concrete Node filesystem adapter in this pass -- left
as an open question rather than asserted either way.

**Concurrency model -- the key nuance.** Both the CLI and the harness only
serialize writers that share the same in-process object graph; **neither
implements any cross-process lock** (no `flock`, no lock file, no
compare-and-swap on an expected file size/mtime found anywhere in either
tree).

- CLI: serialization is purely single-instance, single-thread -- there is no
  queue at all, just synchronous mutation of one `SessionManager`'s own
  state. Two `SessionManager` instances opened on the same path (same
  process or different processes) have no coordination whatsoever.
- Harness: `KeyedOperationQueue<TKey>` (`packages/agent/src/harness/session/
  keyed-operation-queue.ts`) provides two layers of in-process
  serialization:
  ```ts
  enqueue<T>(key: TKey, operation: () => Promise<T> | T): Promise<T> {
      const previous = this.tails.get(key) ?? Promise.resolve();
      const result = Promise.all([this.barrier, previous]).then(() => this.runOperation(operation));
      const tail = result.then(() => undefined, () => undefined);
      this.tails.set(key, tail);
      void tail.then(() => { if (this.tails.get(key) === tail) this.tails.delete(key); });
      return result;
  }
  enqueueBarrier<T>(operation: () => Promise<T> | T): Promise<T> {
      const result = Promise.all([this.barrier, ...this.tails.values()]).then(() => this.runOperation(operation));
      this.barrier = result.then(() => undefined, () => undefined);
      return result;
  }
  async drain(): Promise<void> {
      await Promise.all([this.barrier, ...this.tails.values()]);
  }
  ```
  (`keyed-operation-queue.ts:18-43`, exact). `enqueue` chains an operation
  behind both a global `barrier` and the given key's own tail -- i.e.
  per-key FIFO serialization, but every key-scoped operation also waits for
  any pending barrier. `enqueueBarrier` does the opposite: it waits for the
  barrier *and every currently-known key's tail*, then becomes the new
  barrier itself, so it acts as a full stop-the-world checkpoint relative
  to all in-flight per-key operations -- used by `JsonlSessionBackend.list()`
  (`jsonl-repo.ts:220`) so a directory listing waits for every pending
  append across every open session before reading directory contents.
  `runOperation` additionally gates on an optional global semaphore
  (`acquirePermit`/`releasePermit`, `:45-68`) bounding total concurrent
  operations; `JsonlSessionBackend` defaults this to
  `DEFAULT_MAX_CONCURRENT_OPERATIONS = 4` (`jsonl-repo.ts:44`). None of this
  is a lock in the OS sense -- it is pure in-process promise-chain
  serialization, scoped to one `KeyedOperationQueue` instance (i.e. one
  `JsonlSessionBackend`/repository instance, i.e. effectively one process).
  A second process opening the same JSONL file is completely uncoordinated
  with the first, exactly as with the CLI.
  In addition, `Session` itself (the per-session object handed back to
  callers) layers a *third*, per-instance serialization on top: every append
  method funnels through `enqueueAppend`, which chains onto a private
  `appendTail: Promise<void>` (`session.ts:157, 237-255`) -- so two calls on
  the *same* `Session` object are strictly ordered even before either one's
  promise reaches the backend's `KeyedOperationQueue`.
  The SQLite backend uses a fourth, simpler primitive,
  `SerialOperationQueue` -- a single global tail with no per-key sharding and
  no concurrency cap (`packages/storage/sqlite-node/src/sqlite/repo.ts:48-61`)
  -- but backs it with genuine SQLite transactions
  (`db.transaction(() => ...)`, `repo.ts:87-88`), so its durability story is
  categorically different (WAL mode, `PRAGMA synchronous=FULL`,
  `PRAGMA busy_timeout=5000`, set in `configureSqliteDatabase`,
  `repo.ts:35-39` and `search-backend.ts:19-23`) even though its in-process
  concurrency model is cruder than the JSONL backend's.

**Delivery semantics / idempotence.** The harness's `JsonlSessionBackend.
appendEntry()` explicitly rejects a duplicate id before writing: `if
(entries.has(entry.id)) throw new SessionError("invalid_entry", "Entry
${entry.id} already exists")` (`jsonl-repo.ts:270`) -- genuine client-side
dedup-by-entry-id, enforced against the in-memory index, not the file. The
CLI's `_appendEntry`/`_persist` path has **no equivalent check** -- nothing
in `session-manager.ts` guards against writing two entries with the same
`id` other than `generateId`'s own collision-avoidance loop at mint time
(`:221-227`); a caller that supplied its own colliding id (there is no public
API for that on the message/entry-append paths, but `newSession({id})` and
`forkFrom({id})` do accept caller-supplied session ids) would not be caught
here.

## Read and resume path

**Both systems fully materialize the file on open; there is no lazy/partial
load in either.**

- CLI: `SessionManager.open(path, ...)` first does an *optional* fast-path
  bounded header scan (`readSessionHeader`, capped at
  `MAX_SESSION_HEADER_SCAN_BYTES = 1024 * 1024` bytes,
  `session-manager.ts:494, 563-608`) purely to determine the session's `cwd`
  before constructing the object -- the exception handler for
  `SessionHeaderScanLimitError` explicitly documents the scan as "only a
  discovery optimization. A full load remains authoritative for legacy files
  with very large headers or prefixes." (`:1538-1541`). Regardless of that
  outcome, `_setSessionFile` always calls `loadEntriesFromFile(path)`
  (`:511-553`) to read the *entire* file into `this.fileEntries` -- a
  streaming line-reader over `readSync` chunks, but the *whole* file is
  buffered into one in-memory array before use. There is no cursor, no
  partial materialization, and no size bound on this second, authoritative
  read.
- Harness: `loadJsonlSession()` calls `fs.readTextFile(path)` (whole file at
  once) and splits on `\n` (`jsonl-repo.ts:159-174`); the resulting entries
  populate a fresh `ArraySessionIndex` in full (`jsonl-repo.ts:145-149`).
  `SessionEntryCursorOptions{afterEntrySeq, limit}` on `readEntries()` looks
  like pagination but is a slice over that already-fully-loaded in-memory
  array (`array-session-index.ts:112-116`) -- not a disk-level cursor.

**Resume reads the durable store directly; there is no separate local
cache checked first** on either side. The CLI's `_setSessionFile` reads the
target `.jsonl` file itself with no intermediate cache layer; the harness's
`JsonlSessionBackend` similarly loads straight from the injected
`FileSystem`, with the in-memory `entryIndexesByPath` map acting only as a
per-open-session cache to avoid re-reading on every operation, not as a
resume source distinct from the file (populated by `loadDocument`,
invalidated only implicitly by process lifetime).

**What is materialized eagerly vs. lazily.** Both systems materialize the
raw entries eagerly on open, but neither eagerly computes the "active
context" (the LLM-ready message list) at open time -- that is built lazily,
on demand, by walking from leaf to root:
- CLI: `getBranch(fromId?)` walks `parentId` pointers from a leaf backward
  (referenced throughout, e.g. `createBranchedSession` at `:1417`), and
  `buildContextEntries()`/`buildSessionContext()` are only invoked when the
  agent loop actually needs the next LLM request's message list.
- Harness: `Session.getBranch(fromId?)` calls
  `storage.readPathToRootOrCompaction(...)` (`session.ts:184-186`), and
  `buildContext()`/`buildContextEntries()` similarly derive the LLM view
  from that path on demand (`session.ts:201-207`, and the module-level
  `buildSessionContext`/`buildContextEntries` functions at
  `session.ts:94-150`).

**Active-branch (leaf) resolution on load -- asymmetry between the two
systems, one of this dossier's central findings.** The CLI's `_buildIndex()`
resolves the resumed leaf purely by *physical last line*: it iterates
`fileEntries` in on-disk order and unconditionally sets
`this.leafId = entry.id` for every non-header entry it visits
(`session-manager.ts:957-976`), so after a full reload the leaf is always
"whichever entry the file's last line encodes" -- i.e. append order, full
stop. The CLI's `branch(branchFromId)` -- the operation that moves the leaf
to an earlier point for `/tree` navigation -- is implemented as exactly one
line: `this.leafId = branchFromId;` (`session-manager.ts:1360-1365`), with
**no entry appended and nothing written to disk**. Consequence: if a CLI
process calls `branch()` to jump to an earlier point and then exits (or
crashes) before appending anything new, the file on disk is completely
unaffected -- a subsequent `SessionManager.open()` on that same file will
resolve the leaf back to the physically-last entry, silently undoing the
branch. The branch only "sticks" durably once a new entry is appended after
it, because that new entry becomes the new last physical line. The harness
does not have this gap: its `Session.moveTo(entryId, summary?)` calls a
private `setLeafId()` which itself appends a real, persisted `LeafEntry`
(`{ type: "leaf", targetId: string | null }`, `packages/agent/src/harness/types.ts:448-451`) via the
same `enqueueAppend` path as any other entry (`session.ts:257-264`), and
`ArraySessionIndex.append()`/`.replace()` both special-case this entry type
when deriving `leafId`: `` this.leafId = entry.type === "leaf" ? entry.targetId : entry.id; ``
(`array-session-index.ts:78, 92`). So in the harness, a bare leaf move with
no follow-up message is fully durable and survives a reload; in the CLI, it
is not durable until the next append. This is a real behavioral difference
between the two systems' definitions of "the current branch," not just an
implementation detail.

**Orphaned/broken tree entries are tolerated, not fatal**, on the CLI side:
`getTree()`'s doc comment states "Orphaned entries (broken parent chain) are
also returned as roots" (`session-manager.ts:1307-1309`), and the
implementation treats any entry whose `parentId` does not resolve to a
known node as an additional root rather than throwing
(`:1322-1330`). The harness's equivalent traversal is stricter and fails
loudly instead: `findEntriesOnBranch()` throws `SessionError("invalid_session", "Entry ${current.parentId} not found")`
when a `parentId` does not resolve (`array-session-index.ts:136-139`), as
does `readPathToRootOrCompaction()` (`:180-183`). This is another CLI/harness
divergence worth flagging: the same broken-chain condition is silently
tolerated in one system and a hard failure in the other.

## Listing, summaries, and search

**Enumeration is a directory scan on all three backends; there is no
persistent index of "which sessions exist" anywhere.**

- CLI: `SessionManager.list`/`listAll` call `readdirSync` over the target
  directory (or every directory under the sessions root for `listAll`) and
  then build a `SessionInfo` per `.jsonl` file found, bounded to
  `MAX_CONCURRENT_SESSION_INFO_LOADS = 10` files in flight at once
  (`session-manager.ts:769, 800`, `buildSessionInfosWithConcurrency`
  at `:771-810`).
- Harness `JsonlSessionBackend.list()`: routed through
  `enqueueBarrier` (so it waits out any in-flight writes first,
  `jsonl-repo.ts:220`), then lists either the one `cwd`-derived directory or
  every directory under the sessions root (`:222-234`), and calls
  `loadJsonlSessionMetadata` per file -- but that helper only reads the
  **first line** of each file (`fs.readTextLines(path, { maxLines: 1 })`,
  `jsonl-repo.ts:150-156`), i.e. the harness's list operation is
  header-only and cheap, unlike the CLI's.

**A per-file summary/read-model *is* built at listing time, but only in
memory, and only by the CLI -- the harness's list returns bare metadata with
no summary.** `buildSessionInfo(filePath)` (`session-manager.ts:687-758`)
streams each candidate file line-by-line with Node's `readline` (not the raw
`readSync` loop `loadEntriesFromFile` uses) and accumulates: header id/cwd/
`parentSessionPath`, the latest `session_info` name (including explicit
clears -- `name = entry.name?.trim() || undefined`, `:711-713`), a running
`messageCount`, the most recent user/assistant "activity" timestamp,
`firstMessage` (first user message's text), and **`allMessagesText`** -- the
concatenation of every user/assistant message's extracted text
(`:735-741`). This is a genuine per-file read-model, but it is rebuilt from
scratch on every `list()`/`listAll()` call -- nothing persists it to disk as
a sidecar, so its cost scales linearly with total session count and total
bytes across all session files on every picker open, bounded only by the
concurrency cap above. This matches the doc's framing of `/resume` as
letting the user "search by typing" (`packages/coding-agent/docs/
sessions.md:39-49`): the search is a client-side substring filter over this
in-memory `SessionInfo[]`, not a query against any index.

**The generic harness search adapter is the same "no index" story, made
explicit in its own doc comment**: "Searches canonical sessions directly and
therefore has no index to maintain."
(`packages/agent/src/harness/session/search.ts:16`). Its implementation is a
brute-force scan: for every session in `list()`, `open()` it, get every
entry, `JSON.stringify` it, and substring-match (case-insensitive) against
the query (`search.ts:24-39`).

**The one genuine indexed search subsystem in the whole monorepo lives in
the SQLite package, and is not wired into anything shipped.**
`packages/storage/sqlite-node/src/sqlite/search-backend.ts` maintains a
real SQLite FTS5 virtual table kept consistent via triggers on the
`session_entries` table:
```sql
CREATE VIRTUAL TABLE IF NOT EXISTS session_search_fts USING fts5(
  payload, content = 'session_entries', content_rowid = 'rowid',
  tokenize = 'trigram remove_diacritics 1'
);
CREATE TRIGGER IF NOT EXISTS session_search_fts_ai AFTER INSERT ON session_entries BEGIN
  INSERT INTO session_search_fts(rowid, payload) VALUES (new.rowid, new.payload);
END;
-- ...ad/au triggers mirror deletes/updates the same way
```
(`search-backend.ts:38-52`, elided only the delete/update trigger bodies,
which are structurally identical). Queries rank by BM25 and join back to
session/entry rows for metadata
(`bm25(session_search_fts) AS score`, `search-backend.ts:96-101`). This
package (`@earendil-works/pi-storage-sqlite-node`) is **not** a dependency
of `coding-agent`, `client`, or `server`'s `package.json` (checked directly)
-- it exists in the tree, implements the same `SessionRepository`/
`SessionSearch` interfaces as the JSONL/in-memory backends, but nothing in
the shipped product currently constructs or uses it.

## Entry/message structure and versioning

**Envelope.** Every entry (both systems) is `{ type: string; id: string;
parentId: string | null; timestamp: string } & <type-specific fields>` --
the CLI calls this `SessionEntryBase` (`session-manager.ts:44-49`), the
harness calls the identical shape `SessionTreeEntryBase`
(`packages/agent/src/harness/types.ts:375-380`). `parentId: null` marks a
root. Ordering/threading is entirely via this `parentId` pointer, not a
separate sequence field.

**Entry type unions differ between the two systems** -- same tag names for
the shared subset, but the harness's union is a strict superset with two
additions the CLI does not have:

- CLI (`SessionEntry`, `session-manager.ts:144-153`): `message`,
  `thinking_level_change`, `model_change`, `compaction`, `branch_summary`,
  `custom`, `custom_message`, `label`, `session_info`.
- Harness (`SessionTreeEntry`, `packages/agent/src/harness/types.ts:453-464`): the same nine, **plus**
  `active_tools_change` (`ActiveToolsChangeEntry`, `packages/agent/src/harness/types.ts:398-401`) and
  `leaf` (`LeafEntry`, `packages/agent/src/harness/types.ts:448-451`, discussed above). The harness's
  `SessionInfoEntry` also carries an explicit code comment marking it
  `// legacy name, kept for backwards compatibility` (`packages/agent/src/harness/types.ts:444`),
  signaling that even within the harness codebase this entry type is
  considered a holdover rather than a first-class current concept.

**`CompactionEntry` itself differs in field requiredness between the two
systems -- a genuine schema divergence under the same `type` tag, and the
clearest case in this research where the checked-in doc and the actual
shipped code disagree.**

- CLI's type (`session-manager.ts:69-79`):
  ```ts
  export interface CompactionEntry<T = unknown> extends SessionEntryBase {
      type: "compaction";
      summary: string;
      firstKeptEntryId: string;   // required
      tokensBefore: number;
      details?: T;
      usage?: Usage;
      fromHook?: boolean;
      // no retainedTail field exists on this type at all
  }
  ```
  Grepping the CLI's actual compaction implementation
  (`packages/coding-agent/src/core/compaction/compaction.ts`) for
  `retainedTail` returns **zero matches** -- the CLI never sets this field,
  because its own type doesn't have it.
- Harness's type (`packages/agent/src/harness/types.ts:403-412`):
  ```ts
  export interface CompactionEntry<T = unknown> extends SessionTreeEntryBase {
      type: "compaction";
      summary: string;
      firstKeptEntryId?: string;   // optional
      tokensBefore: number;
      retainedTail?: AgentMessage[];  // new, harness-only
      details?: T;
      usage?: Usage;
      fromHook?: boolean;
  }
  ```
- Yet the **checked-in spec document that is supposed to describe the CLI's
  own format** shows an example `CompactionEntry` with `retainedTail`
  populated and *no* `firstKeptEntryId` at all
  (`packages/coding-agent/docs/session-format.md:240`), and explicitly
  attributes the field to a different producer: "`retainedTail`: ...
  **Newer harness-generated compactions** include it so we can rebuild
  context from this checkpoint without walking older entries before the
  compaction entry." (`session-format.md:245`, emphasis added). Per this
  research's ground rule that code wins where doc and code disagree: the
  shipped `pi` CLI's own `compaction.ts` cannot and does not produce a
  `retainedTail`-bearing `CompactionEntry` -- that behavior belongs
  exclusively to the harness's `Session.appendCompaction()`
  (`packages/agent/src/harness/session/session.ts:317-340`, which accepts an
  optional `retainedTail` parameter and threads it straight onto the entry).
  The checked-in `session-format.md` conflates the two systems into one
  narrative even though, as established above, they are not integrated --
  this is worth recording as a finding in its own right, exactly as the
  research method anticipates specs can drift from code.

**Message-type hierarchy** (RESEARCH_PROMPT section 7's "quote the type
definitions"): base roles (`user`, `assistant`, `toolResult`) come from
`@earendil-works/pi-ai`'s `Message` union (not re-quoted here -- out of the
coding-agent package, only referenced); the coding-agent layer extends this
via TypeScript declaration merging into `AgentMessage`:
```ts
declare module "@earendil-works/pi-agent-core" {
    interface CustomAgentMessages {
        bashExecution: BashExecutionMessage;
        custom: CustomMessage;
        branchSummary: BranchSummaryMessage;
        compactionSummary: CompactionSummaryMessage;
    }
}
```
(`packages/coding-agent/src/core/messages.ts:69-77`), where e.g.
```ts
export interface CompactionSummaryMessage {
    role: "compactionSummary";
    summary: string;
    tokensBefore: number;
    timestamp: number;
}
```
(`packages/coding-agent/src/core/messages.ts:62-67`). `convertToLlm()` (`packages/coding-agent/src/core/messages.ts:148-195`) is the single
function that turns the full `AgentMessage[]` (including these
coding-agent-specific roles) into the base `Message[]` shape an LLM
provider actually receives -- every non-base role becomes a synthesized
`user` message wrapping prefixed/suffixed text (e.g.
`COMPACTION_SUMMARY_PREFIX`/`SUFFIX`, `packages/coding-agent/src/core/messages.ts:11-24`), and a
`bashExecution` message flagged `excludeFromContext` is dropped entirely
(`packages/coding-agent/src/core/messages.ts:152-156`).

**Format versioning is a header field, not a per-entry field, and lives only
in the CLI's world -- the harness has no version concept, only a hard-coded
constant.**
```ts
export interface SessionHeader {
    type: "session";
    version?: number; // v1 sessions don't have this
    id: string;
    timestamp: string;
    cwd: string;
    parentSession?: string;
}
export const CURRENT_SESSION_VERSION = 3;
```
(`session-manager.ts:30, 32-38`). Migration is **sequential, cascading, and
destructive-on-load** -- traced end to end:
1. `migrateToCurrentVersion(entries)` reads the header's `version` (default
   `1` if absent, matching the comment above), and does nothing if already
   `>= CURRENT_SESSION_VERSION`; otherwise runs `migrateV1ToV2` (if
   `version < 2`) then `migrateV2ToV3` (if `version < 3`) -- unconditionally
   both, not "either/or" (`session-manager.ts:277-291`).
2. `migrateV1ToV2` (`:230-257`): for every entry, bumps the header's
   `version` to `2`; for every non-header entry, mints a fresh 8-hex id and
   chains `parentId` to the previous entry's new id -- this is the point
   where the *tree* structure (`id`/`parentId`) is retrofitted onto what was
   previously an implicitly-linear array; and for `compaction` entries
   specifically, converts a legacy `firstKeptEntryIndex: number` (an array
   offset) into the new `firstKeptEntryId: string` by looking up
   `entries[firstKeptEntryIndex]` and using *its* freshly-minted id,
   deleting the old index field.
3. `migrateV2ToV3` (`:259-275`): bumps header `version` to `3`; rewrites any
   `message` entry whose nested `AgentMessage.role === "hookMessage"` to
   `role: "custom"` -- the "extensions unification" rename the checked-in doc
   describes at `session-format.md:25`.
4. `_setSessionFile` calls `migrateToCurrentVersion` on every load and, if
   it returned `true` (a migration was applied), immediately calls
   `_rewriteFile()` to persist the migrated entries back to the same path
   before doing anything else (`session-manager.ts:915-919`) -- i.e.
   migration is not lazy or on-demand; opening an old file rewrites it on
   disk on first touch, using the same non-atomic truncating write
   described in "Write and append path."

This is **one-way**: there is no `migrateV3ToV2` or downgrade path anywhere
in the source, and the header's `version` field is only ever written as
`CURRENT_SESSION_VERSION` (`newSession()`, `:930`) or bumped upward inside
the two migration functions -- never decremented. **A v3 file opened by an
older build** that still checks for `version === 2` (or lacks v3 awareness)
is not something this codebase can characterize by definition (that build no
longer exists in this repo); what is confirmed is that the *current* code's
own loader tolerates versions `1` through `3` inclusive and always upgrades
to `3` -- there is no branch that rejects a too-new version, but a version
number *greater than* `CURRENT_SESSION_VERSION` was not exercised by any
test found and its behavior is unconfirmed (see Open questions).

**By contrast, the harness's JSONL backend enforces exactly one version and
has no migration logic whatsoever**: `parseHeader()` throws unless
`header.version === 3` exactly --
```ts
if (header.type !== "session" || header.version !== 3) {
    throw invalidSession(
        path,
        header.type === "session" ? "unsupported session version" : "first line is not a valid session header",
    );
}
```
(`packages/agent/src/harness/session/jsonl-repo.ts:78-83`). A v1 or v2 CLI
session file, opened directly through `JsonlSessionRepository`, would fail
outright with `SessionError("invalid_session", "unsupported session
version")` rather than being auto-migrated -- migration is exclusively a
CLI-side, `session-manager.ts`-only behavior.

**The SQLite package versions its *schema*, not the session data**, via an
ordered, idempotent SQL migration list applied through a tracking table:
```ts
export async function loadMigrations(): Promise<SqliteMigration[]> {
    return [
        { id: "001_initial.sql", order: 1, sql: await loadMigrationSql("./migrations/001_initial.sql") },
        { id: "002_branch_tips.sql", order: 2, sql: await loadMigrationSql("./migrations/002_branch_tips.sql") },
    ];
}
```
(`packages/storage/sqlite-node/src/sqlite/migrations.ts:15-27`), tracked in a
`migrations(id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)` table
(`:29-34`). This is a conventional additive-migration model, structurally
unlike either the JSONL header-version scheme or the harness's fixed-v3
gate.

## Compaction and history management

Compaction is entirely an upstream (application-layer) concern that leaves a
marker entry in the durable log; the store itself does not truncate or
rewrite older entries. Both compaction and its sibling "branch
summarization" share one structured-summary format and one file-tracking
mechanism (`packages/coding-agent/docs/compaction.md:14-23`).

**Trigger.** Auto-compaction fires when
`contextTokens > contextWindow - reserveTokens`, `reserveTokens` defaulting
to 16384 and configurable via `~/.pi/agent/settings.json`
(`compaction.md:27-35`); `/compact [instructions]` triggers it manually.

**Mechanism** (`compaction.md:39-79`, cross-checked against
`packages/coding-agent/src/core/compaction/compaction.ts`): walk backward
from the newest message accumulating token estimates until
`keepRecentTokens` (default 20k) is reached; call the LLM to summarize
everything from the previous kept boundary (or session start) up to that
cut point, feeding the previous summary back in as iterative context when
one exists; append one new `CompactionEntry` recording `summary` and
`firstKeptEntryId` (the CLI always sets `firstKeptEntryId`; see the
required-vs-optional divergence above); the session then reloads its active
context using the summary in place of everything before
`firstKeptEntryId`. On a *second* compaction, the span to summarize starts
at the *previous* compaction's `firstKeptEntryId`, not at the earlier
compaction entry itself (falling back to "the entry after the previous
compaction" if that kept entry cannot be found on the current path) -- this
preserves messages that survived the first compaction by folding them into
the second pass too (`compaction.md:79`). `tokensBefore` is recomputed from
the freshly rebuilt session context immediately before the new entry is
written, so it reflects the actual pre-compaction context size being
replaced, not a stale estimate.

**Split turns.** A "turn" is one user message plus every assistant/tool
response until the next user message; compaction normally cuts at turn
boundaries. When a single turn alone exceeds `keepRecentTokens`, the cut
point instead lands mid-turn at an assistant message (a "split turn"), and
the tool generates and merges *two* summaries: one for prior history, one
for the early part of the oversized turn (`compaction.md:81-107`). Valid cut
points are user messages, assistant messages, `BashExecution` messages, and
custom messages (`custom_message`, `branch_summary`); **tool results are
never a valid cut point**, because they must stay attached to their
originating tool call (`compaction.md:109-117`).

**Artifact left in the log**: exactly one appended entry per compaction --
`CompactionEntry` -- never an external snapshot file and never an in-place
rewrite of older entries (those remain untouched in the JSONL file
indefinitely; they are merely excluded from the *active context* view, not
deleted). `details?: T` is extension point for arbitrary JSON; the CLI's
own default compaction populates it with `{ readFiles: string[];
modifiedFiles: string[] }`, tracked *cumulatively* across compactions by
reading the previous compaction's `details` and merging in new file
operations discovered in the newly-summarized span
(`compaction.md:179-185`).

**Resume/replay across a compaction boundary** is exactly the mechanism
already described under "The storage model"/"Read and resume path":
`defaultContextEntryTransform` (harness,
`packages/agent/src/harness/session/session.ts:61-92`) finds the single
most-recent `compaction` entry on the path and either (a) if
`retainedTail` is present, treats the compaction as a **self-contained
checkpoint** and includes only the compaction entry plus everything
appended after it, never re-walking anything earlier (`session.ts:74-79`);
or (b) if only `firstKeptEntryId` is present (the CLI's only mode), walks
forward from that id to the compaction entry, keeping everything in
between, then everything after (`session.ts:80-91`). The doc frames this
exactly the same way: "`retainedTail` is optional only so older sessions
that only store `firstKeptEntryId` continue to load correctly."
(`session-format.md:342`) -- i.e. `retainedTail` is presented as a forward
looking upgrade to the *same* mechanism, even though (per the versioning
section above) only the harness, not the CLI, currently produces it.

**Branch summarization** is the sibling mechanism triggered specifically by
`/tree` navigation away from a branch, not by context pressure: find the
deepest common ancestor of the old and new leaf positions, collect every
entry on the abandoned path back to that ancestor, summarize under a token
budget (newest-first), and append one `BranchSummaryEntry` at the new
navigation point (`compaction.md:150-177`, structure mirrors
`CompactionEntry` minus `firstKeptEntryId`/`retainedTail`, plus a `fromId`
back-pointer to the entry navigated away from). File-operation tracking
here is cumulative in the same way as compaction's.

## Rewind, checkpoints, and fork

**Rewind ("branch") is expressed purely as a leaf-pointer move, not a
destructive edit -- but the two systems disagree on whether that move is
itself durable, as already established under "Read and resume path."**
Nothing is ever deleted or mutated in place when navigating to an earlier
point; existing entries remain on disk regardless of which system is used.

There is **no file-state or environment checkpoint tied to turns** anywhere
in the session-storage layer itself -- no full-content/diff/hash file
snapshots were found associated with entries in either `session-manager.ts`
or the harness types. The closest thing is the `readFiles`/`modifiedFiles`
string-array tracking inside `CompactionEntry.details`/`BranchSummaryEntry.
details`, which records *paths*, not file content, diffs, or hashes
(`compaction.md:179-185`) -- this is bookkeeping for the summarization
prompt, not a restorable checkpoint of workspace state. (RESEARCH_PROMPT
section 9's file-state-checkpoint question therefore has essentially a "no"
answer for Pi at this commit; recorded as a gap rather than guessed.)

**Fork has three distinct implementations across the CLI alone, plus a
fourth (harness) shape -- this is one of the sharpest findings in the whole
dossier.**

1. **`/tree`** (in-place branch, same file): `SessionManager.branch()` /
   `branchWithSummary()` -- leaf pointer move only, covered above. Not really
   a "fork" by RESEARCH_PROMPT's definition (no new session identity), but
   the closest thing to "shared-prefix reference" semantics: nothing is
   copied, the shared history is simply the same physical file.
2. **`/clone`** and **user-initiated `/fork` from an earlier message**, both
   implemented by `createBranchedSession(leafId)`
   (`session-manager.ts:1412-1512`): extracts *only the root-to-leaf path*
   for the given `leafId` (via `getBranch`) into a **new** file. It strips
   `LabelEntry`s out of the copied path, then re-derives and re-appends
   them as a trailing sub-chain pointing at their (re-chained) targets, so
   labels survive the extraction without polluting the main path
   (`:~1420-1470`, per the file's own comment: "Filter out LabelEntry from
   path -- we'll recreate them from the resolved map. Because labels are
   real tree entries, later entries can be children of labels"). This is
   copy-plus-lineage in the sense that entry *content* is copied, but ids
   are **not** preserved verbatim for labels (they're re-derived), while
   non-label entries on the path keep their original ids and `parentId`
   chain intact. Like `newSession`, it only writes to disk immediately if
   the extracted path already contains an assistant message -- same
   deferred-write contract as ordinary sessions.
3. **`SessionManager.forkFrom(sourcePath, targetCwd, sessionDir?, options?)`**
   (`:1579-1630`) is a *completely different* operation from #2 despite the
   shared name "fork": it copies **every non-header entry from the entire
   source file, verbatim, in original order** -- not filtered by leaf or
   branch at all -- into a brand-new file, with a new session id, a new
   `cwd` (`targetCwd`), and a header `parentSession` field set to the
   *source file's path* (`:1607-1613`, `newHeader.parentSession =
   resolvedSourcePath`). This is used specifically for forking a session
   from one project directory into another (its own doc comment: "Fork a
   session from another project directory into the current project.
   Creates a new session in the target cwd with the full history from the
   source session.", `:1573-1577`). Lineage metadata recorded: exactly one
   field, the source file's absolute path, in the new header's
   `parentSession`. There is no reverse index from a source file to its
   forks -- lineage is discoverable only by scanning every session's header
   for a matching `parentSession` value (which is exactly what the
   interactive picker's "threaded" sort mode does, see below).
4. **Harness `fork()`** (`SessionRepository.fork(source, options)`):
   selection-based, not path-based like the CLI's `/fork`/`/clone`, and
   computed centrally by shared helpers rather than duplicated per backend:
   ```ts
   export function createSessionForkSelection(options: SessionForkOptions): SessionForkSelection {
       if (!options.entryId) return { kind: "all" };
       return (options.position ?? "before") === "at"
           ? { kind: "through_entry", entryId: options.entryId }
           : { kind: "before_user_message", entryId: options.entryId };
   }
   ```
   (`packages/agent/src/harness/session/repository.ts:51-56`) -- no
   `entryId` copies everything (closest analogue to `forkFrom`'s whole-file
   copy, but scoped to the *active path*, not the whole file's entries,
   since `readSessionEntriesForFork`'s `"all"` branch reads
   `source.readEntries()`, i.e. every entry ever appended to that session,
   matching `forkFrom`'s "copy everything" semantics rather than
   `createBranchedSession`'s "copy only the active path" semantics); an
   `entryId` with `position: "at"` copies the root-to-that-entry path
   inclusive (`readPathToRootOrCompaction(target.id)`); an `entryId` with
   `position: "before"` (the default) requires the target to be a `user`
   message and copies the path up to but excluding it
   (`readSessionEntriesForFork`, `repository.ts:59-71`, validated:
   `if (target.type !== "message" || target.message.role !== "user") throw new SessionError("invalid_fork_target", ...)`).
   `JsonlSessionBackend.fork()` (`jsonl-repo.ts:309-337`) reads the source
   under its own operation-queue key, computes the selection, then creates
   the new document under a fresh key -- two separate serialized operations
   chained by awaiting a promise, not one atomic cross-session
   transaction. Lineage: the new document's header `parentSession` defaults
   to the source's path unless the caller overrides it
   (`jsonl-repo.ts:330-333`).

The user-facing comparison table in the docs captures the CLI's three
navigation-affecting commands succinctly: `/tree` stays in the same file
with full-tree view and an optional branch summary; `/fork` creates a new
file via a user-message selector with no summary; `/clone` creates a new
file duplicating the current active branch, also with no summary
(`packages/coding-agent/docs/sessions.md:118-127`) -- all three are
distinct from `SessionManager.forkFrom`, which has no interactive command
at all and is reachable only via `pi --fork <path|id>` at CLI startup, per
the flags table (`sessions.md:9-16`).

## Subagents and nested sessions

No first-class subagent/child-session concept was found in Pi's core at
this commit. A targeted search across `packages/coding-agent/src` and
`packages/agent/src` for the literal string `subagent` returned **zero
matches** outside documentation. The only place the concept appears is:

- `packages/coding-agent/docs/extensions.md:2969`, a catalog table entry:
  `| \`subagent/\` | Spawn sub-agents | \`registerTool\`, \`exec\` |` --
  listing an *example extension*, not a built-in feature.
- The corresponding source lives under
  `packages/coding-agent/examples/extensions/subagent/` (`index.ts`,
  `agents.ts`, a `README.md`, and a handful of markdown agent-persona
  files under `agents/`) -- i.e. this is sample/reference code shipped to
  show extension authors *how they could* build subagent spawning on top of
  the public `registerTool`/`exec` extension API, not a shipped, in-product
  feature with its own session-storage semantics.

Consequently there is no durable parent-child link for subagents to
document, no inheritance-vs-isolation answer for a child transcript, and no
bounded-nesting or cascade/orphan/reconcile behavior to report -- this
section is a genuine "no position" per the task's instructions, not a gap
in research coverage. The one adjacent, *actually-implemented* parent-child
concept in the codebase is the session-header `parentSession` field used by
`forkFrom()`/harness `fork()` (see "Rewind, checkpoints, and fork" above),
which links two independent, full-fledged sessions (one per project/context
switch), not a subagent's nested execution within one parent turn -- this is
a different concept and should not be conflated with subagent support.

## Retention, deletion, and multi-host

**No TTL or lifecycle policy was found anywhere in the source.** Sessions
persist indefinitely until a human explicitly deletes them; there is no
scheduled cleanup, no automatic expiry, and no size-based eviction observed
in either `session-manager.ts` or the harness packages.

**Deletion is entirely a CLI/UI-layer concern; neither `SessionManager` nor
`SessionRepository` semantically "owns" retention beyond the repository's
own `delete()` primitive.** Concretely:

- `SessionManager` has **no `delete()` method** at all -- confirmed by
  reading the full class. Deletion logic lives one layer up, in
  `packages/coding-agent/src/modes/interactive/components/
  session-selector.ts` (not `session-picker.ts`, despite the module being
  colloquially "the session picker"). The actual function:
  ```ts
  async function deleteSessionFile(
      sessionPath: string,
  ): Promise<{ ok: boolean; method: "trash" | "unlink"; error?: string }> {
      const trashArgs = sessionPath.startsWith("-") ? ["--", sessionPath] : [sessionPath];
      const trashResult = spawnSync("trash", trashArgs, { encoding: "utf-8" });
      // ...
      if (trashResult.status === 0 || !existsSync(sessionPath)) {
          return { ok: true, method: "trash" };
      }
      try {
          await unlink(sessionPath);
          return { ok: true, method: "unlink" };
      } catch (err) { /* ... */ }
  }
  ```
  (`session-selector.ts:645-680`, elided only the error-hint string
  formatting). This confirms the checked-in spec's claim verbatim: "When
  available, pi uses the `trash` CLI to avoid permanent deletion."
  (`session-format.md:17`, `sessions.md:50`) -- `trash` is tried first and,
  only if it fails *and* the file still exists afterward, the code falls
  back to a hard `unlink`.
- Deletion is **blocked for the currently active session**:
  `startDeleteConfirmationForSelectedSession()` checks
  `isCurrentSessionPath(selected.session.path)` and, if true, calls
  `this.onError?.("Cannot delete the currently active session")` and
  returns without prompting for confirmation at all
  (`session-selector.ts:393-400`).
- Deletion requires an explicit confirmation step: pressing the delete key
  only calls `setConfirmingDeletePath(path)` (`:404`); the actual
  `deleteSessionFile` call only fires from a separate confirmed-delete
  handler (`onDeleteSession`, wired at `:830-833`), matching the
  documented UX ("delete with Ctrl+D, then confirm", `sessions.md:48`).
- Deletion has **no cascade to any other artifact** -- because there is no
  other artifact. No summary sidecar, no search index entry (the CLI has
  none to begin with), nothing else references the file by path except the
  in-memory `SessionInfo[]` arrays the picker itself maintains, which it
  updates by filtering out the deleted path locally after a successful
  delete (`session-selector.ts:833-844`).
- The harness's `JsonlSessionBackend.delete()` is the store-level analogue,
  and is a plain filesystem remove with no trash/undo semantics at all:
  `getFileSystemResultOrThrow(await this.fs.remove(metadata.path, { force: true }), ...)`,
  followed by clearing the in-memory `entryIndexesByPath`/
  `operationKeysByPath` entries for that path (`jsonl-repo.ts:297-307`).
  The `trash`-CLI-first behavior is exclusively a CLI/TUI-layer nicety;
  the pluggable store interface's own `delete()` contract makes no promise
  about recoverability.

**Session-list tree vs. entry tree -- an easily-confused pair of distinct
concepts, both present in this codebase, worth disambiguating explicitly.**
The interactive picker builds and displays a *tree of session files* (not
to be confused with the in-file entry tree discussed throughout this
dossier), keyed by each file's header `parentSessionPath` -- i.e. the fork
lineage recorded by `forkFrom`/harness `fork`:
`buildSessionTree()`/`flattenSessionTree()`
(`session-selector.ts:209-278`), selectable via a "threaded" sort mode
(default), toggled alongside "recent" and "relevance" (fuzzy) modes
(`session-selector.ts:985-986`). This is a second, independent tree
structure layered on top of everything described in "Rewind, checkpoints,
and fork" -- the *entry* tree lives inside one file; the *session* tree
spans multiple files via header back-references, reconstructed at list
time by scanning every session's `parentSessionPath`, with no persisted
index of the reverse direction.

**Multi-host / multi-process is not a first-class path anywhere in the
storage layer -- it is, at most, a workaround the operator must arrange
themselves (e.g. a shared filesystem mount), and the storage code does
nothing to detect or coordinate across hosts.** No network-filesystem
awareness, no crash-detection heuristic (e.g. a stale-lock check, a PID
file, an mtime-based staleness test), and no remote-writeback path were
found in `session-manager.ts` or the harness JSONL/SQLite backends. As
established under "Write and append path," even *same-host, same-file*
concurrent writers from two separate OS processes are unguarded -- there is
no OS-level lock anywhere in this codebase's session-storage layer.

The one place multi-*client* (not multi-host storage) concurrency is
handled explicitly is a completely different layer: `@earendil-works/
pi-client`'s `SessionLease`/`SessionHandle`
(`packages/client/src/session-handle.ts`), which lets multiple clients
attach to **one already-running, in-process** session over a wire protocol
(`@earendil-works/pi-protocol`) with `SessionLeaseMode = "shared" |
"exclusive"` (`session-handle.ts:12-16`). This is a live-process attach/
detach/lease negotiation for a session that is already open in one server
process (`@earendil-works/pi-server`'s `src/sessions.ts`, not read in depth
here) -- it says nothing about, and does not replace, the on-disk JSONL/
SQLite storage layer itself; it is a concurrency-control mechanism for
*viewing and steering* one live agent run from multiple UI clients, which
is a different problem from multi-host *storage* consistency. Recorded here
because it is the closest thing in the repo to a "multi-host" answer, but
flagged clearly as answering a different question than RESEARCH_PROMPT
section 11 asks.

**A note on an unshipped future design.** `packages/agent/docs/harness.md`
and `packages/agent/docs/harness-v2.md` are design documents (not source)
proposing a substantially different execution/storage model -- "lanes"
(named concurrent execution positions within one session), durable
"operations" with crash-recoverable boundaries, and (per `harness.md`,
not fully re-verified against `harness-v2.md`'s newer text) a planned v4
JSONL format interleaving harness-bookkeeping entries with session entries.
`harness-v2.md` is explicit that this is a **design document for
not-yet-shipped work**, not a description of current behavior: it opens
with "**Decision note.** This is the chosen design..." and states a
compatibility policy in its own words: "Old coding-agent v3 JSONL sessions
must open and restore idle. This is the only backward-compatibility
requirement. All other formats and APIs in `packages/agent/src/harness` and
`packages/storage/sqlite-node` (and their respective tests) may break. We do
not write migrations, schema versioning, or conversion paths for anything
else." (`packages/agent/docs/harness-v2.md`, the document's second
blockquote, immediately following its title). This directly confirms two
things checked independently in this pass: (a) the `SessionTreeEntry` union
actually present in `packages/agent/src/harness/types.ts` at this commit --
read in full -- contains no harness-entry types (`OperationStartedEntry`,
`HarnessEntryBase`, or similar), confirming the plan is not yet implemented
in `types.ts`; and (b) `packages/storage/sqlite-node` is named in the plan's
own compatibility policy as one of the pieces expected to change, consistent
with it being early/foundational work for that plan rather than a finished,
independent product feature. Nothing from either harness doc is otherwise
relied upon above as a description of current behavior.

## Interop with foreign session stores

Not applicable. No code path reading another product's native session
format (Claude Code, Codex CLI, etc.) was found in either the CLI or the
harness packages; `pi`'s own JSONL format is the only format its loaders
understand, gated by the strict version checks described above.

## What this implies for our Session Store (our inference)

Everything below is our synthesis, not a claim sourced from Pi's own docs.

Pi's most valuable lesson for our design is negative: **a tree-of-entries
model with a movable "leaf" pointer is a good fit for branch/rewind
semantics, but Pi's own two implementations disagree on whether the leaf
position is itself part of the durable log** -- the CLI treats it as
derived-from-append-order (i.e. leaf = last physical line, with `branch()`
a transient in-memory-only pointer move that a follow-up append is required
to make durable), while the harness's `LeafEntry` persists leaf moves as
first-class entries. For an event-sourced store, the harness's approach is
the sound one: **a "move active pointer" operation should itself be an
event**, not an inferred side effect of physical file order, precisely
because "the last thing that happened to be appended" is not a reliable
encoding of "the position a resuming client should restore to" once
non-linear navigation is possible. We should treat leaf/cursor moves as
first-class, appended, replayable events, exactly as the harness's
`LeafEntry` does, and avoid the CLI's implicit-by-append-order approach.

Second, Pi is a cautionary example of what happens when a pluggable storage
abstraction and a shipped product's actual persistence code are allowed to
diverge for long enough: the two implementations now disagree on required
fields for the same entry `type` tag (`CompactionEntry.firstKeptEntryId`),
on orphaned-entry tolerance (silently-repaired vs. hard failure), and on
whether format-version mismatches are auto-migrated or rejected outright --
and the checked-in documentation describes a blend of both as if it were
one coherent system. If our platform maintains both a "reference"
implementation and a "pluggable interface for embedders," the interface and
the reference implementation should either be the same code path, or the
divergence should be continuously tested (a cross-implementation
conformance test suite), not merely documented in prose that can silently
go stale.

Third, the near-total absence of any cross-process write coordination in
either of Pi's implementations (no file locks, no compare-and-swap, no
"expected version" precondition on append) is worth treating as a hazard to
design against deliberately, not something to reproduce by omission. All of
Pi's serialization guarantees are in-process promise chains
(`KeyedOperationQueue`, `SerialOperationQueue`, `Session.appendTail`) --
real for a single process, worthless the moment two processes (or two
hosts) touch the same file. Our event-sourced Session Store should bake an
expected-position precondition into its append primitive from the start
(optimistic concurrency keyed by last-known sequence/offset), specifically
because retrofitting it later, as this dossier shows, tends to produce
exactly the kind of two-tier "some paths get it, some don't" inconsistency
Pi currently has between its SQLite backend (real transactions) and its
JSONL backends (none).

Fourth, the deferred-first-write optimization in the CLI (`_persist`'s
`hasAssistant` gate) is a reasonable UX-driven idea -- don't litter storage
with sessions nobody actually used -- but as implemented it makes the
"is this session persisted yet" question stateful and process-local
(`this.flushed`), which is exactly the kind of implicit state an
event-sourced design should instead make an explicit, queryable fact (e.g.
a session lifecycle state transition that is itself an event, or a
first-class "materialize on N-th write" policy applied uniformly by the
store rather than folded into the append method's control flow).

Fifth, based on all of the above: what makes something "a stored session"
in Pi is, at minimum, one physical file (or one row-set) holding a
tree-shaped sequence of typed, `parentId`-linked entries whose *reachable
subset from a leaf* -- not the file's full contents -- defines "the current
conversation." Pi is closer to an append-only log with derived projections
than to a mutable document (nothing is ever rewritten in place except by
the version-migration path, which is itself best understood as "replaying
old entries and re-emitting them in the new schema," not as ad hoc mutation
of live data), but it falls short of a clean event-sourced design in two
respects our platform should not repeat: its "current position" state is
sometimes derivable only by convention (append order) rather than always
by an explicit event, and its two production storage implementations have
been allowed to drift into incompatible dialects of the same conceptual
format under one version-tag scheme.

## Open questions

- Whether a v3 (or, hypothetically, a future v4) session file opened by a
  strictly older build that only understands versions up to 2 fails
  gracefully, silently mis-reads the file, or crashes -- no such
  version-rejection branch (`version > CURRENT_SESSION_VERSION`) was found
  in `session-manager.ts`'s migration path, and no test exercising a
  too-new version was located in the two test files inspected
  (`packages/agent/test/harness/session.test.ts`,
  `session-backends.test.ts`) or in the CLI's own compaction tests
  referenced by `compaction.md`. This is a genuine gap, not an inferred
  answer.
- Whether the underlying `FileSystem.writeFile`/`appendFile`
  implementation(s) actually used in production (as opposed to the abstract
  capability interface at `packages/agent/src/harness/types.ts:291-341`)
  provide any atomicity (temp-file-and-rename, fsync) beneath the harness's
  JSONL backend. The interface itself makes no such guarantee, and no
  concrete Node.js-backed implementation of `FileSystem` was traced in this
  pass.
- Whether working-directory relocation (a project folder moved or renamed
  on disk) has any reconciliation path beyond the one historical
  `migrate-sessions.sh` bugfix script -- no generalized "cwd changed, find my
  old sessions" logic was found, but the absence of a feature is harder to
  prove exhaustively than its presence; flagged as unconfirmed rather than
  asserted as definitely absent.
- The exact runtime behavior of `packages/agent/docs/harness.md`'s and
  `harness-v2.md`'s planned "lanes"/durable-operation/v4-format design
  relative to what ships today was checked only to the extent of (a)
  confirming `harness-v2.md` explicitly labels itself a design decision
  document with an explicit compatibility policy for *future* breakage, and
  (b) confirming no harness-entry types from that plan exist in the current
  `packages/agent/src/harness/types.ts`. The full 2321-line `harness.md` and
  the "generator" variant referenced inside `harness-v2.md`
  (`harness-v2-generator.md`, said to be preserved "at commit `01eeafd1`")
  were not read to completion; nothing from either was used as a claim
  about current behavior above, but a reader wanting the complete planned
  design (as opposed to "is it shipped yet," which this dossier does
  answer: no) should read those documents directly.
- Whether `@earendil-works/pi-server`'s `src/sessions.ts` (353 lines,
  read only via `wc -l` and a name check in this pass) wires the
  `SessionLease`/`SessionHandle` protocol on the client side to any of the
  three `SessionRepository` implementations described above, to the
  harness's `AgentHarness` execution engine, or to some other mechanism
  entirely -- not traced in this pass; the multi-client leasing story in
  "Retention, deletion, and multi-host" above is therefore necessarily
  incomplete on the server side.
- Whether `packages/storage/sqlite-node`'s `SqliteSessionRepository` is
  actually exercised by anything at all in this monorepo (a dedicated test
  suite, an internal tool, a not-yet-merged integration) beyond existing as
  a standalone package with no consumer found in the three shipped
  `package.json` files checked (`coding-agent`, `client`, `server`) -- its
  purpose (foundational work for the `harness-v2.md` plan, a standalone
  offering for third-party embedders, or something else) is inference on
  our part, not something stated directly in source read during this pass.

# Void: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Void pinned at commit
`b3166e7ef2aefbdfeb139445fdf248a561b85d4d` (Apache-2.0).
Version-sensitive claims were checked against these
authoritative anchors:

- `src/vs/workbench/contrib/void/browser/chatThreadService.ts` (Void's own chat/thread service, ~1884 lines)
- `src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts` (message/entry types)
- `src/vs/workbench/contrib/void/common/storageKeys.ts` (storage key definitions)
- `src/vs/platform/storage/common/storage.ts` and `src/vs/platform/storage/electron-main/storageMain.ts` (upstream VS Code storage machinery Void reuses, not Void's own code)

## The storage model

Void is a VS Code fork. Its chat feature does have a durable, coherent
record -- but it is a thin layer built entirely on top of stock VS Code
machinery, not a bespoke store. Void's own contribution is: one TypeScript
service (`ChatThreadService`), one storage key, and one JSON blob.

The durable session record is a single `ChatThreads` value -- a
`{ [id: string]: ThreadType }` map holding **every thread the user has ever
had**, plus every message in each -- serialized with `JSON.stringify` and
written to one key in VS Code's built-in key-value `IStorageService`
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:415-423`):

```ts
private _storeAllThreads(threads: ChatThreads) {
    const serializedThreads = JSON.stringify(threads);
    this._storageService.store(
        THREAD_STORAGE_KEY,
        serializedThreads,
        StorageScope.APPLICATION,
        StorageTarget.USER
    );
}
```

`THREAD_STORAGE_KEY` is `'void.chatThreadStorageII'`
(`src/vs/workbench/contrib/void/common/storageKeys.ts:19`). `IStorageService`
is stock VS Code: on desktop it is backed by a SQLite database file named
`state.vscdb` (`src/vs/platform/storage/electron-main/storageMain.ts:285`,
`:361`), one key-value row per storage key. Void never touches SQLite, files,
or IndexedDB directly -- it calls the generic `.get()`/`.store()` API that
every VS Code extension/contribution uses for its own settings and UI state.

So: the source of truth is a **mutable JSON document under a single storage
key**, not an append-only log. Every mutation (new message, edit, delete,
checkpoint jump) reads the current in-memory `ThreadsState`, computes a new
whole `ChatThreads` object, and calls `_storeAllThreads()` with the entire
map -- confirmed at every call site
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:942`, `:1289`,
`:1646`, `:1659`, `:1675`, `:1696`). There is no derived index, cache, or
summary distinct from this blob -- the same object is both the record and the
thing the sidebar UI renders from directly.

Closest fit: **session-as-document** (a single mutable JSON document per
installation, keyed by thread id inside it), not session-as-log.

## Keying and identity

- A thread's id is a client-generated UUID via `generateUuid()`
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:211`, inside
  `newThreadObject()`). There is no server assignment; it is minted locally
  when a thread is created and does not encode ordering (plain UUID, not
  UUIDv7).
- `ThreadType` carries `createdAt` and `lastModified` ISO-string fields
  (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts` is
  message types; the thread shape itself is
  `src/vs/workbench/contrib/void/browser/chatThreadService.ts:114-144`).
  Ordering for the sidebar list is done by sorting on `lastModified`, not by
  id (`src/vs/workbench/contrib/void/browser/react/src/sidebar-tsx/SidebarThreadSelector.tsx:37-38`):
  `.sort((a, b) => (allThreads[a]?.lastModified ?? 0) > (allThreads[b]?.lastModified ?? 0) ? -1 : 1)`.
- Listing is **global, not scoped per project/workspace**. The storage scope
  is `StorageScope.APPLICATION`
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:406`, `:420`),
  which upstream VS Code documents as "scoped to all workspaces across all
  profiles" (`src/vs/platform/storage/common/storage.ts:225-228`). Every
  chat thread ever created in that VS Code installation lives in one blob and
  is enumerable regardless of which folder/workspace is currently open. There
  is no per-workspace or per-project partitioning of thread ids at all.
- `currentThreadId` is explicitly **not** persisted -- the comment at
  `src/vs/workbench/contrib/void/browser/chatThreadService.ts:308` says
  "allThreads is persisted, currentThread is not." On restart, `openNewThread()`
  runs (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1628-1648`):
  it looks for any existing empty thread and switches to it, otherwise mints a
  brand-new UUID thread. There is no "reopen where you left off" behavior for
  which thread was active; only the historical list survives, and the user
  must manually pick a thread to resume it via the sidebar selector.
- No relocation/rename reconciliation exists because there is nothing to
  reconcile against (no path/cwd component in the identity at all).

## The store interface

No pluggable interface -- this is a private module inside one workbench
contribution, called directly by `ChatThreadService`'s own methods and by
React UI code. Reconstructed effective operations, all in
`src/vs/workbench/contrib/void/browser/chatThreadService.ts`:

| Operation | Signature (reconstructed) | Location |
|---|---|---|
| Read all threads on boot | `_readAllThreads(): ChatThreads \| null` | `:405-413` |
| Write all threads (full rewrite) | `_storeAllThreads(threads: ChatThreads): void` | `:415-423` |
| Create thread (or reuse an empty one) | `openNewThread(): void` | `:1628-1648` |
| Switch active thread (not persisted) | `switchToThread(threadId: string): void` | `:1623-1625` |
| Delete a thread | `deleteThread(threadId: string): void` | `:1651-1661` |
| Duplicate a thread (new id, deep-cloned contents) | `duplicateThread(threadId: string): void` | `:1663-1677` |
| Append a message to a thread | `_addMessageToThread(threadId: string, message: ChatMessage): void` | `:1680-1697` |
| Replace one message in place | `_editMessageInThread(threadId, messageIdx, newMessage): void` | `:925-944` |
| Reset entire store (dev/debug) | `resetState(): void` / `dangerousSetState(newState): void` | `:384-392` |
| Rewind (checkpoint pointer, then truncate on next write) | `jumpToCheckpointBeforeMessageIdx(opts): void` | `:1080-...` (truncation at `:1272-1290`) |

Every one of these that mutates state ends by calling `_storeAllThreads()`
with the complete map (see call sites listed in "The storage model" above),
then firing `_onDidChangeCurrentThread` so the React sidebar re-renders. There
is no partial write, offset, or incremental append at the storage layer --
"append a message" is implemented as "compute a new whole thread map with one
more message in one thread's array, then persist the whole map."

## Write and append path (ordering, durability, concurrency, delivery)

- **Ordering**: positional -- messages are plain array elements
  (`ChatMessage[]` in `ThreadType.messages`,
  `src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts` for the
  `ChatMessage` union; array field at
  `src/vs/workbench/contrib/void/browser/chatThreadService.ts:119`). No
  sequence numbers, timestamps per message, or server-assigned positions --
  order is JS array order.
- **Durability/atomicity**: entirely inherited from VS Code's
  `IStorageService`, which is a SQLite-backed key-value store
  (`src/vs/platform/storage/electron-main/storageMain.ts:285`). Void does no
  temp-file-and-rename or explicit fsync of its own; whatever atomicity
  SQLite's transaction commit gives you is what you get. A crash mid-write
  would be governed by SQLite/VS Code's storage flush behavior, not by
  anything in Void's code.
- **Concurrency model**: effectively single-writer -- one renderer process
  reading and rewriting one in-memory `ThreadsState` object, with no
  optimistic-concurrency check (no expected-version precondition; last
  `_setState` call simply wins). This is fine for a single-user desktop editor
  but there is no compare-and-swap or lock around `_storeAllThreads`.
- **Delivery semantics**: best-effort, synchronous within the renderer;
  no retry/queue logic was found around `_storageService.store()`.
- **Idempotence**: none needed/observed -- mutations are local function calls,
  not messages that could be delivered twice.

## Read and resume path

On construction, `ChatThreadService` calls `_readAllThreads()`
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:334`), which
does a single synchronous `_storageService.get(THREAD_STORAGE_KEY,
StorageScope.APPLICATION)` and a full `JSON.parse` with a URI-reviving
reviver function (`:396-403`, `:405-413`). This is a **full ordered read of
the entire history in one shot** -- every thread and every message for the
whole installation is loaded into memory at startup, every time. There is no
cursor, no pagination, no lazy per-thread loading, and no size bound. What
gets read back is the raw record itself, not a cached/derived view -- Void
does not maintain any local cache distinct from this blob.

`openNewThread()` then runs unconditionally (`:343`), so "resume" for the
*active* editing session never happens automatically; the user resumes a
specific past thread only by clicking it in the sidebar
(`switchToThread`, `:1623-1625`), which simply repoints `currentThreadId` at
an already-loaded thread -- no additional storage read occurs at that point,
since everything was already pulled in at boot.

## Listing, summaries, and search

Listing is a direct iteration of the in-memory `allThreads` map, sorted by
`lastModified`
(`src/vs/workbench/contrib/void/browser/react/src/sidebar-tsx/SidebarThreadSelector.tsx:37-38`),
with an initial-page cap in the UI only (`numInitialThreads`, "Show N more..."
at `:42-73` of the same file) -- that is a rendering limit, not a storage
limit; all threads are already in memory regardless. There is no separate
metadata sidecar, no denormalized summary record, and no search index (full
text, vector, or otherwise) of any kind found in
`src/vs/workbench/contrib/void/` -- a targeted grep for `indexedDB`/`IndexedDB`
across that directory tree returned no matches.

## Entry/message structure and versioning

The full `ChatMessage` union is defined in
`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:50-69`:

```ts
export type ChatMessage =
    | {
        role: 'user';
        content: string;
        displayContent: string;
        selections: StagingSelectionItem[] | null;
        state: { stagingSelections: StagingSelectionItem[]; isBeingEdited: boolean; }
    } | {
        role: 'assistant';
        displayContent: string;
        reasoning: string;
        anthropicReasoning: AnthropicReasoning[] | null;
    }
    | ToolMessage<ToolName>
    | DecorativeCanceledTool
    | CheckpointEntry
```

`ToolMessage<T>` (`:11-28`) tags each tool call with `role: 'tool'`, an `id`,
`rawParams`, `mcpServerName`, and a discriminated `type` field walking through
its lifecycle (`invalid_params` → `tool_request` → `running_now` →
`tool_error`/`success`/`rejected`). `CheckpointEntry` (`:38-46`) has
`role: 'checkpoint'` and embeds, per file path, a `VoidFileSnapshot` --
defined in `src/vs/workbench/contrib/void/common/editCodeServiceTypes.ts:115-118`
as `{ snapshottedDiffAreaOfId: ...; entireFileCode: string }`. Note
`entireFileCode`: checkpoints store the **full file content**, not a diff.

Entries are not opaque to the store -- `ChatThreadService` reads and mutates
specific fields (`messages`, `state.currCheckpointIdx`, etc.) directly; there
is no black-box blob-passthrough.

Versioning is a manual, ad hoc convention, not a schema-version field on the
data itself. `src/vs/workbench/contrib/void/common/storageKeys.ts:6-19`
tracks it entirely through the **storage key name**:

```ts
// past values:
// 'void.chatThreadStorage'
// 'void.chatThreadStorageI' // 1.0.2
// 1.0.3
export const THREAD_STORAGE_KEY = 'void.chatThreadStorageII'
```

and a standing warning at
`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:49`:
"WARNING: changing this format is a big deal!!!!!! need to migrate old format
to new format on users' computers so people don't get errors." No migration
*code* implementing that warning was found anywhere under
`src/vs/workbench/contrib/void/` (a repo-wide grep for `migrat` in that tree
matched only an unrelated VSCodium doc link). In other words: the project has
bumped the key name twice as a de facto "start over" migration strategy
(old data under the old key becomes inaccessible, not converted), and
otherwise relies on the warning comment rather than enforced versioning.

## Compaction and history management

None found. No summarization, truncation-with-marker, or context-window
compaction logic exists in `chatThreadService.ts` or the LLM-message
conversion path -- a grep for `compact`/`summariz` across
`src/vs/workbench/contrib/void/browser/chatThreadService.ts` and
`convertToLLMMessageService.ts` returned nothing. Whatever context-window
management exists is presumably left to the model API itself; the durable
message array simply keeps growing.

## Rewind, checkpoints, and fork

- **Checkpoints**: each turn can append a `CheckpointEntry` message
  (`role: 'checkpoint'`, `type: 'user_edit' | 'tool_edit'`) carrying a full
  `entireFileCode` snapshot per touched file
  (`src/vs/workbench/contrib/void/common/chatThreadServiceTypes.ts:38-46`,
  `src/vs/workbench/contrib/void/common/editCodeServiceTypes.ts:115-118`).
  These are full-content snapshots, not diffs, and they live inline in the
  same `messages` array that gets wholesale-rewritten on every mutation --
  there is no separate snapshot store or deduplication observed.
- **Rewind ("jump to checkpoint")**: `jumpToCheckpointBeforeMessageIdx`
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1080` onward)
  moves a `currCheckpointIdx` pointer and re-applies prior file snapshots to
  the working tree; by itself this does **not** truncate the message array.
  Truncation is destructive and happens lazily, on the *next* user message: in
  `addUserMessageAndStreamResponse`
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1272-1290`),
  if `currCheckpointIdx !== null`, the code does
  `thread.messages.slice(0, checkpointIdx + 1)` and persists that truncated
  array -- an in-place, irreversible rewrite of history, not an appended
  branch marker. Anything after the checkpoint is permanently gone from the
  stored record once you type a new message.
- **Fork**: `duplicateThread(threadId)`
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1663-1677`) is
  the only fork-like operation -- a full `deepClone` of the thread object with
  a freshly minted `id`. It is copy-plus-new-identity with no lineage field
  recorded (no `parentThreadId`/`forkedFrom` anywhere in `ThreadType`); the
  two threads become entirely independent records with no durable link back
  to each other.

## Subagents and nested sessions

No concept found. `ChatThreads` is a flat `{ [id]: ThreadType }` map with no
parent/child relationship anywhere in `ThreadType`
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:114-154`).
Targeted greps for `subagent`, `childThread`, and `parentThread` across
`src/vs/workbench/contrib/void/` (excluding the React UI tree) returned no
matches. Tool calls (including MCP tool calls, tagged via `mcpServerName` on
`ToolMessage`) are recorded as ordinary entries in the same thread's message
array -- there is no sub-thread spun up for a tool or agent call, and
therefore no cascade-on-delete question to answer: `deleteThread` simply
removes one key from the flat map
(`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1651-1661`) with
nothing else to cascade to.

## Retention, deletion, and multi-host

- **Retention**: none. No TTL, lifecycle policy, or scheduled cleanup was
  found. Threads persist until a user manually calls `deleteThread`.
- **Deletion**: `delete newThreads[threadId]` followed by a full
  `_storeAllThreads` rewrite
  (`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1651-1661`) --
  a hard, immediate delete of that key from the in-memory map and the
  persisted blob. No soft-delete/tombstone, no cascade (nothing to cascade
  to, per above).
- **Growth bound**: none found. Every thread and every message (including
  full-file-content checkpoints) accumulates forever in one JSON value under
  one storage key, re-serialized and rewritten on every single mutation. This
  is architecturally the most notable risk surfaced by this investigation:
  there is no per-thread size limit, no total-thread cap, and no eviction --
  marked here as **inference**, since no explicit cap was found rather than
  proven never to exist by exhaustive testing.
- **Multi-host**: not applicable/not addressed. `StorageScope.APPLICATION` is
  a single local installation's storage; there is no remote writeback, shared
  filesystem handling, or cross-host reconciliation in this code path -- it is
  a single-process, single-machine, Electron-local design end to end.

## Interop with foreign session stores

Not applicable. No code was found that reads or imports session data from
other chat/agent products (Claude Code, Cursor, Continue, etc.) into Void's
thread store, and none was expected given Void's storage is entirely
internal to one VS Code installation's key-value store.

## What this implies for our Session Store (our inference)

Void is **not** independent evidence of an append-only, event-sourced session
design -- it is close to the opposite pole. The entire chat history for an
installation is one mutable JSON document, rewritten in full on every
mutation, with no positional dedup key, no expected-version precondition, and
no bound on size. It inherits durability entirely from VS Code's generic
key-value storage (itself SQLite-backed) rather than building anything
resembling a log. The one design choice worth carrying forward as a
cautionary data point: destructive, in-place history rewrite on
rewind-then-continue (checkpoint truncation,
`src/vs/workbench/contrib/void/browser/chatThreadService.ts:1272-1290`) is
exactly the failure mode an append-only log with branch markers is meant to
avoid -- Void's approach loses the "future" branch permanently the moment you
type past a rewind point, whereas an event-sourced design could retain it as
an orphaned but recoverable branch.

## Open questions

- Whether VS Code's `IStorageService` applies any internal row-size limit or
  compression for large values (e.g. this specific SQLite-backed store) was
  not verified beyond confirming the backing file name
  (`src/vs/platform/storage/electron-main/storageMain.ts:285`); if there is a
  practical ceiling, very active Void users could hit it.
- Whether the browser (non-Electron/web) build of Void backs
  `IStorageService` with IndexedDB instead of SQLite was not traced; only the
  Electron path (`storageMain.ts`) was confirmed. If Void ships a web/browser
  target, its physical storage substrate there is unconfirmed by this pass.
- No test file for `chatThreadService.ts` was located/reviewed in this pass;
  whether there is any test coverage asserting the migration warning's
  intent is unknown.

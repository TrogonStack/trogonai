# Continue: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against a local clone of
[continuedev/continue](https://github.com/continuedev/continue) pinned at
commit `5522c6f44ca0ac3528b37244818fbfa39b5af470` (committed 2026-07-20;
message "docs: remove Sign in link (login flow retired)"). Continue is
Apache-2.0. All `path:line` citations below are repo-root-relative paths at
that exact commit. The session subsystem spans:

- `core/util/history.ts` -- the `HistoryManager` singleton: the only code that
  reads or writes the on-disk session store.
- `core/util/paths.ts` -- path resolution (`getSessionFilePath`,
  `getSessionsFolderPath`, `getSessionsListPath`) and the global `~/.continue`
  directory layout.
- `core/index.d.ts` -- the `Session`, `BaseSessionMetadata`, `ChatHistoryItem`,
  and `ChatMessage` types.
- `core/util/conversationCompaction.ts` -- the VS Code/JetBrains ("core") path
  compaction implementation.
- `core/protocol/core.ts` -- the `ListHistoryOptions` type and the
  `history/*` IPC message surface.
- `core/core.ts:304-332` -- the IPC handlers that are the sole callers of
  `HistoryManager`'s public methods.
- `gui/src/redux/thunks/session.ts`, `gui/src/redux/slices/sessionSlice.ts` --
  the VS Code/JetBrains webview's session lifecycle (write triggers,
  in-memory truncation/rewind).
- `extensions/cli/src/session.ts`, `extensions/cli/src/services/
  ChatHistoryService.ts`, `extensions/cli/src/compaction.ts`,
  `extensions/cli/src/subagent/` -- the `cn` CLI, which reuses `HistoryManager`
  for its file format but layers a second, independent write/resume/compaction
  path on top of it.

Continue ships three clients (a VS Code/JetBrains extension pair sharing one
webview "gui", and a standalone terminal CLI called `cn`) that all read and
write the same `~/.continue/sessions/` directory through the same
`HistoryManager`, but the CLI and the GUI diverge sharply in write cadence,
resume mechanics, and compaction semantics on top of that shared format. Both
divergences are load-bearing findings below.

## The storage model

A session is one JSON file plus one entry in a separate JSON list file, both
under a single global (not per-project) directory:

```text
~/.continue/sessions/{sessionId}.json   # the full transcript, one per session
~/.continue/sessions/sessions.json      # a flat array of metadata for every session
```

(`~/.continue` is `CONTINUE_GLOBAL_DIR` if set,
`core/util/paths.ts:27-36`; the sessions folder is
`getSessionsFolderPath()`, `core/util/paths.ts:78-84`.)

There is no append-only log anywhere in this store. Both files are mutable
documents that get fully parsed, modified in memory, and fully rewritten on
every write:

- The per-session file (`{sessionId}.json`) holds the entire ordered
  `ChatHistoryItem[]` transcript for that session as a single JSON document.
  `HistoryManager.save()` writes it with a plain `fs.writeFileSync`, no
  temp-file-and-rename, no lock (`core/util/history.ts:131-134`).
- `sessions.json` is a flat JSON array of `BaseSessionMetadata` -- a
  denormalized summary of every session that exists -- read in full,
  mutated in memory, and rewritten in full on every save or delete
  (`core/util/history.ts:136-191`, `:69-84`).

**Neither file is authoritative over the other in the way an index over a log
would be.** The per-session file is the only place the actual conversation
content lives; `sessions.json` is a hand-maintained, independently-written
cache of a few fields (title, workspace, message count) whose only purpose is
to avoid parsing every session file to render a picker. Losing a session file
loses the conversation. Losing `sessions.json` loses only the ability to
enumerate sessions cheaply -- the CLI's own resume path (below) proves that
sessions remain independently resolvable by directory listing even without
it. But nothing in the codebase treats `sessions.json` as rebuildable: there
is no scan-and-rebuild function anywhere in the repository (confirmed by
grepping every reference to `sessions.json`, `getSessionsListPath`, and
`getSessionsFolderPath` outside test files -- the only matches are
`core/util/history.ts`, `core/util/paths.ts`, and
`extensions/cli/src/session.ts`). The two files are written by
non-transactional, non-atomic, independent `fs` calls in sequence, and
whichever one a given code path forgets to update first is the one that goes
stale. That divergence-with-no-repair-path is this product's central
storage-model fact, detailed under "The index-drift failure mode" below.

Conceptually this is **session-as-document, twice over**: one mutable
document per session, and one mutable document (`sessions.json`) that
denormalizes metadata from all of them. There is no event log, no positional
sequence number, and (confirmed by grepping the whole session-relevant
surface for `lock`, `atomicWrite`, `writeFileAtomic`, and `proper-lockfile`)
no locking or atomic-rename discipline of any kind protecting either file.

## Keying and identity

- A session's only identity is its `sessionId: string`
  (`core/index.d.ts:279-290`), minted as a random `uuidv4()` -- not UUIDv7,
  not time-ordered, not server-assigned. The GUI mints it in the `newSession`
  reducer (`gui/src/redux/slices/sessionSlice.ts:709`,
  `state.id = uuidv4()`); the CLI mints it in
  `SessionManager.getCurrentSession()` and `createSession()`
  (`extensions/cli/src/session.ts:96-98`, `:342`). Both are pure client-side
  IDs; the store never assigns or renumbers them.
- The session file's on-disk name is derived directly from the id:
  `{sessionsFolder}/{sessionId}.json` (`core/util/paths.ts:102-104`). There is
  no subpath, no per-workspace subdirectory, and no subagent-suffix scheme --
  every session for every project on the machine lives flat in one directory.
- `workspaceDirectory: string` is stored as a field on `Session` and
  `BaseSessionMetadata` (`core/index.d.ts:282`, `:296`), but it is **not**
  part of the key -- it is denormalized data, populated from
  `window.workspacePaths?.[0]` in the GUI
  (`gui/src/redux/thunks/session.ts:253`) or `process.cwd()` in the CLI
  (`extensions/cli/src/session.ts:103`, `:344`, `:567`).
- **Listing is global by default, not scoped.** `ListHistoryOptions` supports
  an optional `workspaceDirectory` filter (`core/protocol/core.ts:57-61`),
  and `HistoryManager.list()` implements it as a case-insensitive exact-string
  match (`core/util/history.ts:41-49`). But neither shipped client ever
  supplies it: the GUI's `refreshSessionMetadata` thunk calls
  `ideMessenger.request("history/list", { limit, offset })` with no
  `workspaceDirectory` key at all (`gui/src/redux/thunks/session.ts:41-45`),
  and the CLI's `listSessions()` calls `historyManager.list({ limit })`,
  same omission (`extensions/cli/src/session.ts:456`). The `cn ls` command
  (`extensions/cli/src/commands/ls.ts:34`, `:56`) shows every session on the
  machine, across every project, with no workspace filter applied anywhere in
  that call chain. The GUI's History page filters only by a client-side
  MiniSearch title index over the full `allSessionMetadata` array
  (`gui/src/components/History/index.tsx:69-116`), never by workspace. The
  only place `workspaceDirectory` filtering is actually exercised is the test
  suite (`core/util/history.test.ts:110-155`). Workspace scoping exists in
  the type and in `HistoryManager`, but is dead code on every production call
  path found.
- **Relocation/rename is unhandled.** No code anywhere touches a session's
  stored `workspaceDirectory` after creation; there is no listener for a
  workspace being moved or renamed, and no normalization (symlink resolution,
  trailing-slash handling) beyond `.toLowerCase()` in the comparison itself
  (`core/util/history.ts:43`). Since production listing never filters by
  workspace anyway, a moved workspace directory today has no visible effect --
  but if a caller ever did pass `workspaceDirectory` (as the tests do), a
  renamed or moved project folder would silently and permanently drop that
  project's sessions out of any workspace-scoped view; the session's file and
  its `sessions.json` entry remain intact and it stays visible in every
  *global* listing, so nothing is destroyed, but it becomes unreachable from
  the one filtered view the field exists to support.

## The store interface

There is no pluggable store interface or SDK type -- `HistoryManager`
(`core/util/history.ts:24-193`) is a concrete class instantiated once as a
module-level singleton (`core/util/history.ts:195-197`,
`const historyManager = new HistoryManager()`) and imported directly by
`core/core.ts`. It is exposed to the two IDE extensions and the webview only
through a fixed IPC message surface (`core/protocol/core.ts:63-76`); the CLI
imports the same class directly as a library (`core/util/history.js`) rather
than through IPC. Reconstructing the effective interface from both call
paths:

| Operation | Signature | Where | Behavior |
| --- | --- | --- | --- |
| `list` | `list(options: ListHistoryOptions): BaseSessionMetadata[]` | `core/util/history.ts:25-58` | Reads `sessions.json` in full, filters out legacy-format entries and (if malformed) fails silent to `[]`, reverses to newest-first, optionally filters by `workspaceDirectory` (case-insensitive exact match), then slices by `offset`/`limit`. Never touches the per-session files or checks they exist. |
| `load` | `load(sessionId: string): Session` | `:91-109` | Reads and `JSON.parse`s the single per-session file; on any error (missing file, bad JSON) logs and returns a synthesized empty `Session` with `history: []`, `title: NEW_SESSION_TITLE`, the given id -- never throws to the caller. |
| `save` | `save(session: Session): void` | `:111-192` | Full-document rewrite of the per-session file (with an explicit re-ordering of keys purely for readability), then a read-modify-write of `sessions.json`: update the matching metadata entry in place, or append a new one if the id isn't found. Recomputes `messageCount` from the full history on every call. Throws a decorated error if `sessions.json` is present but not valid JSON (unless it is empty/whitespace, which is treated as "no sessions yet" and reset to `[]`). |
| `delete` | `delete(sessionId: string): void` | `:60-85` | Throws if the per-session file doesn't exist; otherwise `fs.unlinkSync`s it first, then reads, filters, and rewrites `sessions.json`. If `sessions.json` is malformed, the filter step operates on `[]` (see index-drift below) and the rewrite silently replaces the whole index with an empty array. |
| `clearAll` | `clearAll(): void` | `:87-89` | `fs.rmSync` on the entire sessions folder, recursive and forced -- deletes every session file and the index together. The one operation where both files are guaranteed to change atomically from the caller's point of view (a single directory removal), though still not crash-atomic at the filesystem level. |

The IPC surface that wraps these (`core/protocol/core.ts:70-76`,
implemented at `core/core.ts:304-332`) is a 1:1 pass-through, with one
addition: `history/list` re-slices the result to `msg.data?.limit ?? 100`
*again* after `HistoryManager.list()` has already applied its own optional
limit (`core/core.ts:307-308`). Since the GUI's `refreshSessionMetadata`
thunk passes no `limit`, `HistoryManager.list()` returns every session
unbounded, and this second slice is what actually caps the GUI's session
picker at 100 sessions, silently and independent of the `HistoryManager`
contract itself.

`history/share` (`core/protocol/core.ts:74`, `core/core.ts:323-328`) is a
read-only export operation, not part of the store proper: it loads a session
and renders it to a timestamped Markdown file via
`core/util/historyUtils.ts:41-105` (`toMarkDown`/`shareSession`).

The CLI does not use this IPC surface at all -- it imports `historyManager`
directly (`extensions/cli/src/session.ts:12`) and additionally implements its
own `loadSession()` (resume-by-most-recent) that **bypasses `HistoryManager`
entirely**: see "Read and resume path" below.

## Write and append path (ordering, durability, concurrency, delivery)

There is no append operation. Every write is a full-document rewrite of the
per-session file, ordering is simply array order in memory, and the delivery
model is at-most-once, best-effort, fire-and-forget:

- **Ordering.** `ChatHistoryItem[]` order in the `Session.history` array *is*
  conversation order; there is no sequence number, timestamp, or monotonic
  id on individual entries (`core/index.d.ts:534-545` -- `ChatHistoryItem` has
  no id or ordinal field at all). Whoever holds the in-memory array and calls
  `save()` last wins.
- **Atomicity/durability.** `save()` calls `fs.writeFileSync` directly on the
  session file with no temp-file-then-rename and no `fsync` discipline
  (`core/util/history.ts:131-134`); a crash mid-write leaves a partially
  written, likely unparseable JSON file, which `load()` will catch and
  silently paper over with an empty session (`:91-109`) rather than surface
  as corruption. The same is true for `sessions.json`
  (`:178-181`, `:81-84`). No lock file, no `proper-lockfile`/`flock`
  dependency, and no compare-and-swap of any kind was found anywhere in the
  session write path (grepped for `lock`, `atomicWrite`, and
  `writeFileAtomic` across `core/util/history.ts`, `core/util/paths.ts`, and
  `extensions/cli/src/session.ts`: no hits).
- **Concurrency model.** Effectively single-writer-per-session by social
  convention only, not enforcement: nothing prevents the VS Code extension
  and a `cn` CLI process from writing the same `sessionId.json` (or the
  shared `sessions.json`) concurrently; the last `fs.writeFileSync` wins and
  the loser's in-memory state (and, for `sessions.json`, every entry not
  present in the winner's snapshot) is silently discarded. There is no
  optimistic-concurrency precondition (no expected-version, no ETag) on
  `save()`.
- **Delivery semantics and write cadence differ sharply between the two
  clients that share this store:**
  - The **VS Code/JetBrains GUI** saves the whole session once per completed
    LLM turn (`gui/src/redux/thunks/streamThunkWrapper.tsx:29-36`, guarded by
    `!state.session.isInEdit`) via `saveCurrentSession` →
    `dispatch(updateSession(...))` → `ideMessenger.request("history/save",
    session)` (`gui/src/redux/thunks/session.ts:70-82`, `:186-262`), plus on
    session-switch/new-session events (`gui/src/components/Layout.tsx:63-68`,
    `:94-100`) and on tab close (`gui/src/components/TabBar/TabBar.tsx:162`).
  - The **CLI** saves on *every single history mutation* -- every user
    message, every assistant delta, every tool-call-state update -- because
    `ChatHistoryService.setHistoryInternal()` calls `updateSessionHistory()`
    unconditionally unless explicitly told not to
    (`extensions/cli/src/services/ChatHistoryService.ts:44-71`, the `persist`
    option defaults to persisting), and `updateSessionHistory()` →
    `SessionManager.updateHistory()` → `saveSession()` →
    `historyManager.save(...)` (`extensions/cli/src/session.ts:116-120`,
    `:279-296`) is a full rewrite of the entire session file each time. A
    long tool-heavy turn can therefore rewrite the whole transcript file many
    times before the turn even completes.
  - Both share the same `historyManager.save()` call underneath, but the CLI
    additionally transforms the session before persisting it:
    `getSessionPersistenceSnapshot()` (`extensions/cli/src/session.ts:211-235`)
    strips every `system`-role message out of `history` and rewrites `user`
    messages' `content` into an `editorState` field before the write. The GUI
    performs no such transform -- it persists `session.history` as constructed
    in Redux, including whatever is in it. The on-disk shape of a session
    file therefore depends on which client last saved it.
  - There is no retry, no queue, and no acknowledgment of a failed save
    beyond a `console.log`/`logger.error` (`core/util/history.ts:100-101` for
    load errors is analogous; the CLI's `saveSession()` catches and logs but
    does not re-raise, `extensions/cli/src/session.ts:293-295`). A failed
    write is simply lost; the in-memory state moves on regardless.

### The index-drift failure mode

`sessions.json` and the per-session `.json` files are written by separate,
non-atomic `fs` calls in `save()` and `delete()`, and nothing in the codebase
reconciles them if a process is killed between the two writes, if the two
files disagree, or if `sessions.json` becomes malformed. There is no repair,
fsck, or rebuild-from-directory-scan function anywhere in the repository.
Four concrete divergence paths, each independently reachable from the code
read above:

1. **Orphan file, absent from the index.** `save()` writes the per-session
   file first (`core/util/history.ts:131-134`) and only afterward reads,
   updates, and rewrites `sessions.json` (`:136-191`). A crash, an
   `ENOSPC`, or a thrown validation error between those two steps (the code
   explicitly re-throws a decorated error if `sessions.json` fails to parse,
   `:182-190`) leaves a fully-written, fully-valid session file that
   `sessions.json` has never heard of. `HistoryManager.list()` -- the only
   thing the GUI's history picker and `cn ls` consult -- never scans the
   sessions directory, only `sessions.json` (`:26-30`), so this session is
   permanently invisible to both, forever, unless something calls `save()`
   on it again.
2. **Dangling index entry, absent file.** `delete()` unlinks the session
   file first (`core/util/history.ts:66`) and only then reads and rewrites
   `sessions.json` (`:69-84`). A crash between those two steps leaves a
   `sessions.json` entry pointing at a file that no longer exists. The GUI
   would still show that session's title in the picker; opening it calls
   `load()`, which catches the missing-file error and returns a synthesized
   *empty* session with the *requested* id but no history and the default
   title (`:91-109`) -- silently, with only a `console.log`, no user-facing
   error. The stale `sessions.json` entry is never cleaned up by this path;
   it persists until something else (e.g. a fresh `save()` under that same
   id) overwrites it.
3. **Whole-index wipeout on a corrupted `sessions.json`, triggered by any
   delete.** `list()` and `delete()` both parse `sessions.json` through
   `safeParseArray()` (`:12-22`), which catches any `JSON.parse` failure and
   returns `undefined` -- silently, with only `console.warn`, not surfaced to
   the caller. Both call sites then fall back with `?? []`
   (`:32`, `:71-75`). For `list()` this just means a corrupted index reads
   as "zero sessions" for that call. For `delete()` it is destructive: the
   filtered (now-empty) array is written straight back to disk
   (`:77-84`), replacing a merely-*malformed* `sessions.json` with a
   *validly empty* one -- permanently discarding every other session's index
   entry in the process of deleting one session, with no warning beyond a
   console log the user will never see. Every session file on disk survives
   this untouched; every one of them becomes invisible to `list()` until
   individually re-saved.
   `save()`, by contrast, does **not** use `safeParseArray` for this file -- it
   does a plain `JSON.parse` in its own try/catch and re-throws a
   user-facing error naming the file if the content is non-empty and
   unparseable (`:138-151`, `:182-190`). So the same corrupted-index
   condition is silent data loss through `delete()` and a loud, blocking
   error through `save()` -- the three methods on one `HistoryManager`
   disagree with each other about whether a broken index is a fatal
   condition.
4. **Resume bypasses the index entirely, exposing orphans the picker
   cannot see.** The CLI's own `--resume` path, `loadSession()`
   (`extensions/cli/src/session.ts:301-332`), does not call
   `historyManager.list()` or read `sessions.json` at all. It does a raw
   `fs.readdirSync` over the sessions folder, filters out `sessions.json`
   itself, sorts every remaining `.json` file by filesystem `mtime`, and
   loads the newest (`:309-321`). This means an orphan session file from
   failure mode (1) above -- invisible to `cn ls` and to the GUI picker -- is
   nonetheless the one `cn --resume` will pick up if it happens to be the
   most recently modified file, and a dangling index entry from failure mode
   (2) has no effect on this path at all, because it never consults the
   index. The two ways of finding "the session to work with" in this one
   product (index-backed listing versus raw-directory-scan resume) can and
   do disagree about which sessions exist.

No test in `core/util/history.test.ts` (224 lines, read in full) exercises
any of the above; the suite covers the happy path (create, list, load,
update, delete, workspace filtering, and a 100-session scale check) but never
a missing file, a malformed `sessions.json`, or a crash between the two
writes.

## Read and resume path

- **Standard load** (`HistoryManager.load`, `core/util/history.ts:91-109`) is
  a single synchronous full-file read and `JSON.parse` of the target
  session's `.json` file -- no cursor, no incremental read, no pagination of
  the transcript itself, and no size bound on it. The whole `ChatHistoryItem[]`
  array is materialized into memory in one call, eagerly, every time a
  session is opened.
- **The GUI's "resume last session"** (`loadLastSession`,
  `gui/src/redux/thunks/session.ts:140-170`) resumes by `lastSessionId`
  tracked purely in Redux state (`state.session.lastSessionId`, set in the
  `newSession` reducer, `gui/src/redux/slices/sessionSlice.ts:687`) -- an
  in-memory pointer with no persistence of its own. It calls `history/load`
  for that id, with one retry after a 1s delay on failure. There is a
  commented-out alternative in the same function that would have called
  `history/list` with `limit: 1` to find the actual most-recent session
  (`gui/src/redux/thunks/session.ts:145-150`, dead/disabled code) -- meaning
  the *shipped* GUI resume path depends on the webview process having
  survived with its Redux state intact, not on any durable "last session"
  record in the store.
- **The CLI's `--resume`** (`loadSession()`,
  `extensions/cli/src/session.ts:301-332`) is the opposite: it never
  consults any "last session id" state at all, durable or otherwise. It
  lists the sessions directory directly and picks the file with the newest
  `mtime`, bypassing `sessions.json` and `HistoryManager.list()` completely
  (see index-drift finding 4, above).
- **`cn ls` / the GUI history picker** (list, not resume) always loads
  through `HistoryManager.list()`, which is a full parse of `sessions.json`
  into memory followed by in-memory `Array.filter`/`slice` -- there is no
  streaming or partial read of the index file itself, though the index only
  holds metadata, not transcripts, so this is comparatively cheap. No
  numbers on index size or list latency are quoted anywhere in the source or
  comments (unlike, e.g., grok-build's documented ~12K-session cold-boot
  cost) -- Continue's own scale story is untested beyond the 100-session unit
  test (`core/util/history.test.ts:159-222`).
- Nothing here is materialized lazily: opening a session loads its entire
  history array; there is no "load metadata now, load transcript body on
  demand" split beyond the metadata/transcript file split itself.

## Listing, summaries, and search

- **Listing** is `HistoryManager.list()`: parse all of `sessions.json`,
  filter out legacy-format entries (see next section), reverse
  (`sessions.json` accumulates in creation order because new entries are
  only ever `push`ed, never reordered -- see `save()`,
  `core/util/history.ts:167-176` -- so reversing yields newest-first),
  optionally filter by `workspaceDirectory` (dead in production, see
  "Keying and identity"), then slice by `offset`/`limit`
  (`core/util/history.ts:25-58`). This is the entire "index"; there is no
  separate query engine, no SQL, no directory scan for the metadata path.
- **The metadata sidecar is `sessions.json` itself** -- `BaseSessionMetadata`
  (`core/index.d.ts:292-298`): `sessionId`, `title`, `dateCreated` (set once,
  at first `save()`, never updated -- `core/util/history.ts:171`),
  `workspaceDirectory`, and an optional `messageCount` (recomputed from the
  full history's assistant-role message count on every `save()`,
  `:154-156`, `:161`). It is written at write time (inline in `save()`), not
  computed lazily or rebuilt from the transcripts. It is a genuinely thin
  read model compared to other products in this corpus -- no tags, no fork
  lineage, no git metadata, no per-session token/cost totals in the index
  itself (`SessionUsage`/cost data lives only inside the per-session file,
  `core/index.d.ts:274-277`, `:289`).
  The CLI extends this in memory only, never on disk: `ExtendedSessionMetadata`
  (`extensions/cli/src/session.ts:22-26`) adds `firstUserMessage`, `isRemote`,
  and `remoteId`, computed by re-reading each session file
  (`getSessionMetadataWithPreview`, `:399-439`) at `cn ls` time -- an
  extra full read of every session file on every `cn ls` invocation, on top
  of the `sessions.json` read, specifically to extract a message preview that
  `sessions.json` does not carry.
- `isRemote`/`remoteId` on `ExtendedSessionMetadata` and the
  `getRemoteSessions()` function that would have populated them
  (`extensions/cli/src/session.ts:444-446`) are vestigial: the function's
  entire body is `return [];` with the comment "Remote sessions are no
  longer available (Hub integration removed)." -- added by commit
  `92b99cad9` ("feat: strip more hub config", 2026-03-23), which gutted a
  Continue Hub-backed remote-session feature that had been added five months
  earlier by commit `b12499cb0` ("feat: show remote sessions in cn ls
  (#7694)", 2025-09-19, which is also where `SessionMetadata` was renamed to
  today's `BaseSessionMetadata`, `core/index.d.ts` diff at that commit). The
  type fields and the merge-with-local-sessions code in `listSessions()`
  (`extensions/cli/src/session.ts:451-500`) remain in place; only the data
  source was severed.
- **Search** is not a separate indexed subsystem at all. The GUI's History
  page builds an in-memory MiniSearch index over session *titles only*,
  rebuilt from `allSessionMetadata` on every change
  (`gui/src/components/History/index.tsx:37-65`), combining exact, fuzzy, and
  prefix matches with priority weighting (`:69-107`). It indexes nothing
  from inside the transcripts and persists nothing -- it is rebuilt from
  scratch in the renderer process every time the metadata list changes.
  There is no FTS database, no vector index, and no bootstrap/incremental-
  update story to describe, because there is no persisted search index.

## Entry/message structure and versioning

- The entry type is `ChatHistoryItem` (`core/index.d.ts:534-545`):
  ```ts
  export interface ChatHistoryItem {
    message: ChatMessage;
    contextItems: ContextItemWithId[];
    editorState?: any;
    modifiers?: InputModifiers;
    promptLogs?: PromptLog[];
    toolCallStates?: ToolCallState[];
    isGatheringContext?: boolean;
    reasoning?: Reasoning;
    appliedRules?: RuleMetadata[];
    conversationSummary?: string;
  }
  ```
  `ChatMessage` is a discriminated union on `role`
  (`core/index.d.ts:440-445`): `UserChatMessage`, `AssistantChatMessage`,
  `ThinkingChatMessage`, `SystemChatMessage`, `ToolResultChatMessage`
  (`:374-438`), each carrying `content: MessageContent` (a string or
  `MessagePart[]` of text/image parts, `:342-354`) and an open
  `metadata?: Record<string, unknown>` bag. `ToolCallState`
  (`core/index.d.ts:516-525`) is the tool-execution envelope nested under an
  assistant `ChatHistoryItem`: `toolCallId`, the `ToolCall` itself,
  `status: ToolStatus` (a six-state enum, `:497-503`), `parsedArgs`, and an
  `output?: ContextItem[]`.
  There is no id, parent-pointer, or timestamp on `ChatHistoryItem` itself --
  the entry is not addressable independently of its array position, and the
  store cannot dedup or link entries across a chain beyond that position.
  Identity for the *store* is entirely at the session level
  (`sessionId`); nothing below that granularity has a persisted key.
- `Session.history` (the entry list) is opaque to `HistoryManager` in the
  sense that `save()`/`load()` never parse into individual entries -- they
  serialize/deserialize the whole array as one JSON blob. The one place the
  store *does* interpret an entry field is `save()`'s `messageCount`
  computation, which filters `session.history` on
  `item.message.role === "assistant"` to populate the metadata sidecar
  (`core/util/history.ts:154-156`).
- **Format evolution is real and directly evidenced, but entirely
  additive/optional-field, with no schema-version field anywhere on `Session`
  or `ChatHistoryItem`.** Three concrete episodes, oldest first:
  1. **The Python-era legacy format, still sniffed for today.** Continue's
     server was originally a Python process (`continuedev`); its session
     metadata model was a Pydantic `SessionInfo(ContinueBaseModel)` with a
     snake_case `session_id: str` field (first seen at commit `c25527926`,
     "feat: successfully loading past sessions", 2023-08-06, in
     `continuedev/src/continuedev/server/session_manager.py`, since removed
     from the repository). When the server was rewritten in TypeScript,
     `HistoryManager.list()` was given an explicit filter to skip any
     `sessions.json` entry matching that shape --
     `typeof session.session_id !== "string"` (`core/util/history.ts:36`,
     comment "Filter out old format") -- a check present as early as the
     first TypeScript `HistoryManager` (commit `7edfd3d65`, "history",
     2023-12-09) and unchanged in every subsequent rename of the surrounding
     types (`SessionInfo`→`SessionMetadata`→`BaseSessionMetadata`) through to
     today. **Old-format entries are silently dropped from `list()` output
     forever, not migrated.** Nothing rewrites `sessions.json` to purge them
     (they simply never pass the filter again on the next `list()` call
     either); nothing converts an old-format entry into a loadable
     `BaseSessionMetadata`. A user who still had Python-era `sessions.json`
     entries on upgrade would have found them permanently invisible, with no
     migration path found in the source.
  2. **A short-lived, then-removed file-checkpoint feature.** Commit
     `5a3206261` ("checkpoints working with undo", 2024-11-13) added
     `Checkpoint { [filepath: string]: string }` -- a raw filepath-to-full-
     file-content map, no diffing, hashing, or dedup -- as
     `Session.checkpoints?: Checkpoint[]`. Two weeks later, commit
     `438cba450` ("feat: update redux store schemas", 2024-11-27) moved it
     from a session-level array to a per-turn field:
     `ChatHistoryItem.checkpoint: Checkpoint` plus
     `isBeforeCheckpoint: boolean`. Two days after that, commit `4efd66137`
     ("feat: bugfixes on redux schema updates", 2024-11-29) stopped
     `HistoryManager.save()` from forwarding the (by-then legacy)
     session-level `checkpoints` field. Five months later, commit
     `ff8a63a9e` ("remove checkpoints", 2025-04-30) deleted the `Checkpoint`
     interface and both `ChatHistoryItem` fields outright. **As of the
     pinned commit, Continue has no checkpoint, undo, or rewind concept
     anywhere in its type definitions or store** -- this was built,
     relocated once, and fully retired inside a six-month window, with no
     replacement.
  3. **`MessageModes` grew from a two-value to a four-value enum in place,
     with no migration.** The same 2024-11-27 commit (`438cba450`)
     introduced `type MessageModes = "chat" | "edit"`; today's
     `core/index.d.ts:495` defines `MessageModes = "chat" | "agent" | "plan"
     | "background"`. `Session.mode` is optional
     (`core/index.d.ts:284-285`), so old sessions simply lack the field --
     evolution here is purely additive/optional, with no code anywhere
     translating an old `mode` value into a new one.
- Beyond these three episodes, the general evolution style is additive
  optional fields (`toolCallStates?`, `reasoning?`, `appliedRules?`,
  `conversationSummary?`, `mode?`, `chatModelTitle?`, `usage?` on `Session`
  and `ChatHistoryItem`, `core/index.d.ts:279-290`, `:534-545`) that
  `JSON.parse` simply leaves `undefined` on old data -- no `serde`-style
  explicit-default annotations exist in TypeScript/JSON, but the effect is
  the same: old session files continue to load, with newer optional fields
  absent. There is no store-format version number anywhere (`Session`,
  `BaseSessionMetadata`, and the `sessions.json` array itself all lack a
  `version`/`schemaVersion` key), so there is no way for the loader to know
  *which* format-evolution episode a given file predates other than the one
  ad hoc `session_id` sniff.

## Compaction and history management

Continue has **two independent compaction implementations that diverge on
the one thing that matters most for this research question: whether the
durable record is preserved.**

- **The core/GUI path** (`core/util/conversationCompaction.ts:19-112`,
  triggered manually by `conversation/compact`
  (`core/core.ts:622-642`) from the GUI's "compact" action
  (`gui/src/util/compactConversation.ts:10-43`)) is **non-destructive**: it
  loads the full session, generates a summary of history up to a chosen
  index via the current chat model, and writes that summary string into the
  *existing* `ChatHistoryItem.conversationSummary` field at that index
  (`conversationCompaction.ts:99-111`) -- then calls `historyManager.save()`
  on the *whole, unshortened* history array. Every message before and after
  the compaction point remains in the persisted JSON file, permanently.
  Compaction only affects what gets sent to the model: `constructMessages()`
  scans backward for the most recent `conversationSummary`, and if found,
  slices the array to everything *after* that index for the LLM prompt
  (`gui/src/redux/util/constructMessages.ts:47-58`) -- a read-time
  interpretation of an in-place marker, not a rewrite of the durable log.
  Re-compacting at an already-summarized index explicitly excludes the old
  summary from the search and is handled as "we're re-compacting"
  (`conversationCompaction.ts:29-30`, `:34-38`). Deleting a compaction from
  the GUI (`useDeleteCompaction`,
  `gui/src/util/compactConversation.ts:45-58`) clears the marker client-side
  and re-saves -- again a full-array rewrite, but the underlying messages
  were never gone.
- **The CLI path** (`extensions/cli/src/compaction.ts`,
  `extensions/cli/src/ui/hooks/useChat.compaction.ts`) is **destructive**.
  `compactChatHistory()` (`compaction.ts:53-167`) generates a summary the
  same way, but returns a `compactedHistory` that is *only* the (optional)
  system message plus one new assistant message carrying the summary
  (`compaction.ts:140-155`) -- every prior user/assistant/tool message is
  gone from that return value. Both the manual command
  (`useChat.compaction.ts:56-66`, `updateSessionHistory(result.compactedHistory)`)
  and auto-compaction (triggered by `shouldAutoCompact()`'s token-threshold
  check against context limit, `compaction.ts:266-315`, wired through
  `extensions/cli/src/stream/streamChatResponse.autoCompaction.ts`) persist
  this truncated array as a **full replacement** of `session.history` via
  `ChatHistoryService`/`updateSessionHistory` →
  `historyManager.save()`. The pre-compaction transcript is not written
  anywhere else first -- no snapshot artifact, no external file, nothing.
  Once the CLI auto-compacts or a user runs `/compact`, the discarded
  messages are unrecoverable from the on-disk session file.
- Both paths share the same `conversationSummary`-tagging convention and the
  same `findCompactionIndex`/`getHistoryForLLM` read-time-slicing helper
  shape (`extensions/cli/src/compaction.ts:174-238` mirrors
  `gui/src/redux/util/constructMessages.ts:47-58` and
  `gui/src/redux/slices/sessionSlice.ts`'s `findCompactionIndex`
  equivalent), but only the core/GUI path actually keeps the durable log
  intact; the CLI path collapses it. This is an unresolved internal
  inconsistency in the product, not a documented design choice -- no comment
  in either file acknowledges the other implementation.
- Compaction is entirely an upstream/prompt-construction concern from the
  store's point of view either way: `HistoryManager` has no compaction-aware
  method, no compaction marker type, and no snapshot format of its own. It
  only ever sees a `Session.history` array to fully persist, whatever shape
  the caller hands it.
- There is no resume/replay behavior that crosses "the compaction boundary"
  as a distinct concept in this codebase -- because the GUI path never
  actually removes anything, there is nothing to replay past; because the
  CLI path removes it destructively, there is nothing left to replay.

## Rewind, checkpoints, and fork

- **No fork exists.** Grepping the whole tree for fork/branch/clone-session
  vocabulary (`forkSession`, `sessionFork`, `branchSession`) returns nothing.
  There is no lineage field on `Session` or `BaseSessionMetadata` (no
  `parentSessionId`, no `forkedFrom`), and no code path creates a new session
  from an existing one's prefix.
- **Checkpoints existed and were removed** -- the full five-month history
  (added → relocated → forwarding dropped → deleted) is documented above
  under "Entry/message structure and versioning," episode 2. As of the
  pinned commit there is no file-state or environment checkpoint tied to
  turns anywhere in the type system or the store.
- **"Rewind" exists only as a destructive, in-place truncation of the
  history array, in both clients, with no appended marker and no
  possibility of un-rewinding:**
  - GUI: editing/resubmitting an earlier message
    (`submitEditorAndInitAtIndex`, `gui/src/redux/slices/sessionSlice.ts:357-410`)
    and the analogous `truncateHistoryToMessage` reducer
    (`:414-434`) both do `state.history = state.history.slice(0, index +
    1).concat({...new empty assistant message})` -- everything after the
    edited/target message is simply gone from the in-memory array, which the
    next `saveCurrentSession` will persist as the new, shorter, full
    contents of the session file.
  - CLI: `handleEditMessage()`
    (`extensions/cli/src/ui/hooks/useChat.ts:753-787`) computes
    `rewindedHistory = chatHistory.slice(0, messageIndex)` and immediately
    calls `updateSessionHistory(rewindedHistory)` -- a full rewrite of the
    session file to the truncated array -- before resubmitting the new
    message content from that point (`:762-786`; the UI's own label for this
    action is literally "Editing Message ... (will rewind to this point)",
    `extensions/cli/src/ui/EditMessageSelector.tsx:277`).
  - Neither implementation appends a marker or keeps the discarded tail
    anywhere; the store has no way to distinguish "this session was always
    this short" from "this session was truncated by an edit." There is no
    file-state or environment checkpoint captured alongside either
    operation (that entire concept was removed, per above), so a rewind in
    Continue undoes conversation history only, never any file edits the
    agent made along the way.

## Subagents and nested sessions

Only the CLI has a subagent concept; nothing under `core/` or `gui/`
references one.

- A subagent is invoked as a built-in tool
  (`extensions/cli/src/tools/subagent.ts:15-115`) whose `run()` calls
  `executeSubAgent()` (`extensions/cli/src/subagent/executor.ts:58-213`).
  `executeSubAgent` builds a **brand-new, in-memory-only**
  `ChatHistoryItem[]` seeded with just the delegated prompt
  (`executor.ts:114-122`), and explicitly disables persistence for the
  duration: it monkey-patches `services.chatHistory.isReady` to return
  `false` for the call (`executor.ts:109-112`, comment "Temporarily disable
  ChatHistoryService to prevent it from interfering with child session"),
  restoring it in a `finally` block (`:190-193`). The subagent's own turns
  are **never passed to `HistoryManager.save()` or written to any file** --
  there is no child session file, no child entry in `sessions.json`, and no
  subagent-specific directory anywhere under `~/.continue/sessions`.
- The only durable trace of a subagent run is its **final text output**,
  captured from the last message in its throwaway history array
  (`executor.ts:164-179`) and returned as the tool-call's result string,
  which the parent's `ChatHistoryService.addToolResult()` then folds into
  the *parent's* own persisted `ChatHistoryItem.toolCallStates`
  (`extensions/cli/src/tools/subagent.ts:86-98`, `:103-113`). This matches
  the "entries in the parent transcript" model from the research taxonomy,
  but even more minimally than that phrase implies: it is one opaque string
  inside a tool-result, not a structured child-transcript reference.
- `parentSessionId` is threaded into `SubAgentExecutionOptions`
  (`executor.ts:14-19`) and is fetched from
  `chatHistoryService.getSessionId()` at the call site
  (`extensions/cli/src/tools/subagent.ts:75-84`), but it is **never read
  inside `executeSubAgent`'s body** (confirmed: the destructuring at
  `executor.ts:61` omits it, and no other reference to `options.parentSessionId`
  exists in the file). There is, in other words, no durable parent-child
  link recorded anywhere -- not even a pointer file -- despite the plumbing
  suggesting one was intended.
- **Nesting depth is not bounded by any code found.** The subagent's own
  `streamChatResponse()` call computes its tool list via the same
  `getRequestTools()`/`getAllAvailableTools()` path the top-level agent uses
  (`extensions/cli/src/stream/handleToolCalls.ts:172-189`), and nothing in
  `subagent.ts`, `subagent/executor.ts`, or `subagent/get-agents.ts` excludes
  the subagent tool itself from that list or tracks a recursion depth. This
  is not a firm claim of unbounded recursion -- it is what the depth-limiting
  code would look like if present, and none was found; treat it as an open
  question rather than a confirmed unbounded-nesting design.
- Since nothing about a subagent is persisted beyond the folded-in output
  string, there is nothing to cascade, orphan, or reconcile on parent
  delete/rewind/crash -- a subagent that is still running when its parent
  process exits simply stops with the process; there is no independent
  subagent session for the store to have opinions about.

## Retention, deletion, and multi-host

- **No retention policy of any kind exists.** Grepping `history.ts`,
  `paths.ts`, `session.ts`, and `conversationCompaction.ts` for
  `ttl`/`retention`/`cleanup`/`prune`/`gc` (session-scoped) returns nothing.
  Sessions live until a user explicitly deletes one (`history/delete`) or
  clears everything (`history/clear` → `clearAll()`,
  `core/util/history.ts:87-89`). There is no scheduled cleanup, no
  size-based eviction, and no age-based expiry.
- **Delete is local, in-place, and (per the index-drift finding above)
  order-dependent rather than transactional:** unlink the session file,
  then rewrite `sessions.json` to drop the matching entry
  (`core/util/history.ts:60-85`). There is no remote/writeback backend to be
  "remote-first" about -- Continue's session store has no server-side
  component; even the CLI's abortive "remote session" feature
  (`getRemoteSessions()`, now hard-coded to `[]`) was about Hub-hosted agent
  runs, not a durable-session backend for this store.
- **`clearAll()` is the one crash-safer path**, in the narrow sense that a
  single `fs.rmSync(sessionsFolder, {recursive: true, force: true})`
  removes both files' worth of state together rather than in two ordered
  writes -- though it is still not atomic at the filesystem level (a crash
  mid-`rmSync` on a large directory can leave a partial removal).
- **Multi-host/shared-filesystem support was not found and does not appear
  to have been designed for.** Every read and write in this store is a
  synchronous local `fs` call with no lock; there is no per-host suffixing,
  no network-filesystem detection, and no remote-writeback path for session
  data. This is an inference from absence rather than a documented
  non-goal: nothing in the source or in adjacent docs discusses multi-host
  session access, so treat "not supported" as "not found," not as a
  confirmed design decision.
- Multi-*process*-on-one-host is a real, reachable hazard rather than a
  theoretical one: the GUI (via the extension host) and a `cn` CLI process
  can both be pointed at the same `CONTINUE_GLOBAL_DIR` and both write
  `sessions.json` with no coordination whatsoever (see "Write and append
  path" above); no crash-detection registry (e.g. a live-process pid file,
  as seen in other products in this corpus) exists here.

## What this implies for our Session Store (our inference)

Continue's durable session is a **mutable JSON document per session, plus a
second, independently-mutable JSON document that denormalizes a picker's
worth of metadata about all of them** -- not an append-only log with derived
projections, and not even internally consistent about how to treat its own
index going stale. Read as a cautionary data point rather than a positive
model for our event-sourced Session Store, it argues for several things we
already lean toward:

- **A denormalized listing index that is not rebuildable is a liability,
  not just an incompleteness.** Continue's `sessions.json` cannot be
  regenerated from the per-session files that remain the actual source of
  truth for conversation content; there is no scan-and-rebuild function.
  Our design should ensure any read-model/projection over the session log is
  explicitly reconstructable from the log by construction (fold-from-events),
  precisely so that a "list index vs. transcript" divergence like Continue's
  is a non-event: the projection is always rebuildable, never a second
  independent source of truth that can silently disagree with the first.
- **Silent, inconsistent failure handling on a corrupted index is worse than
  either always failing loud or always being self-healing.** Continue's
  three `HistoryManager` methods (`list`, `save`, `delete`) each treat a
  malformed `sessions.json` differently -- two swallow it into an empty
  array (one of which then persists that emptiness, destructively), one
  throws a decorated error to the caller. A store built on an actual
  event-sourced foundation removes the entire failure category: there is no
  "index that can be malformed independently of the log," because the index
  *is* a projection of the log and is invalidated/rebuilt from it rather
  than hand-maintained by separate read-modify-write calls scattered across
  the write path.
- **"Same store, different write cadence" is a real operational hazard
  worth designing against explicitly.** Continue's GUI persists once per
  completed turn; its CLI persists on every single history mutation inside
  a turn. Both share one `HistoryManager`, but nothing in the interface
  enforces or documents an expected write granularity, so the two clients
  drifted into very different I/O profiles and (per the compaction finding)
  even different retention semantics on top of the identical store
  contract. Our store's write contract should make the append/commit
  granularity explicit and singular, not something each caller is free to
  reinvent.
- **Compaction should be a store-adjacent, explicitly-versioned decision,
  not something two call sites can implement in mutually-destructive ways.**
  Continue's own core/GUI path is a good instinct -- mark-and-slice-at-read-
  time, keep the durable record whole -- but the fact that a second,
  independently-written compaction implementation in the same product
  destructively discards history proves that "non-destructive compaction"
  has to be a property the store itself guarantees (e.g. compaction expressed
  only as an event that a fold-time projection can interpret, never as a
  caller-provided replacement array the store blindly persists), not a
  convention two different code paths happen to follow.
- **A subagent whose entire transcript is discarded except for one output
  string is the degenerate end of the "nested session" spectrum**, and
  Continue's implementation goes out of its way to *prevent* its own
  ChatHistoryService from touching a running subagent (the
  `isReady = () => false` monkey-patch), rather than routing subagent turns
  into a first-class child-session facility. This is a useful negative data
  point for ADR 0031's child-Session direction: it shows what happens when a
  product treats "subagent" purely as an ephemeral tool implementation
  detail rather than a session at all -- no cascade/orphan questions to
  answer, because there was never a durable child session to begin with.
  Where our design intentionally differs (making the subagent a first-class
  child Session) is precisely the gap this product declines to fill.

## Open questions

- No test, comment, or issue reference was found acknowledging the
  index-drift scenarios documented above (orphan file, dangling entry,
  index-wipe-on-corrupted-delete, resume-bypasses-index); it is unclear
  whether the Continue team is aware of them or whether they have been
  reported/observed in the wild.
- Whether the GUI's disabled "resume by history/list" code
  (`gui/src/redux/thunks/session.ts:145-150`) was removed deliberately in
  favor of the Redux `lastSessionId` approach, or is simply dead code left
  over from a refactor, could not be determined from the diff history
  available in this clone.
- Whether subagent nesting is actually reachable in practice (i.e. whether
  the subagent tool is filtered out of a subagent's own tool list by some
  mechanism not found in `get-agents.ts`/`handleToolCalls.ts`) is unresolved;
  this dossier treats it as an open question rather than a confirmed
  unbounded-recursion finding.
- No migration tooling, CLI flag, or documentation was found for converting
  a Python-era (`session_id` snake_case) `sessions.json` entry into the
  current format; whether any users still carry such entries, and whether
  Continue considers that data permanently lost, is unverified from source
  alone.
- Whether `CONTINUE_GLOBAL_DIR` is ever pointed at a network/shared
  filesystem in practice (e.g. a team dev-container setup), and what would
  happen to `sessions.json` under concurrent writers in that configuration,
  was not addressed anywhere in the source or in comments; this dossier's
  "not designed for multi-host" conclusion is an inference from absence, not
  a documented non-goal.
- Whether JetBrains' extension (not read in this pass -- only the shared
  `gui/` webview and `core/` were examined) introduces any additional write
  path into `HistoryManager` beyond what `core/core.ts` exposes was not
  checked; the IDE-specific host code outside `core/` and `gui/` was out of
  scope for this dossier.

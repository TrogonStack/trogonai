# Gemini CLI: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local shallow checkout of `google-gemini/gemini-cli`
(`https://github.com/google-gemini/gemini-cli.git`) at commit
`87f785192c34067e4e8f26bda16cf9ce24014d83` (committed 2026-07-23). Every
citation below was verified against this exact commit on 2026-07-23. Citations
use repo-relative `path:line` shorthand.

Authoritative anchors:

- `google-gemini/gemini-cli` @ `87f785192c34067e4e8f26bda16cf9ce24014d83`
- `packages/core/src/services/chatRecordingService.ts` +
  `chatRecordingTypes.ts` (the durable transcript log)
- `packages/core/src/config/storage.ts` + `packages/core/src/utils/paths.ts`
  (on-disk layout, project keying)
- `packages/cli/src/utils/sessionUtils.ts` + `sessions.ts` (listing)
- `packages/core/src/services/gitService.ts` +
  `packages/core/src/utils/checkpointUtils.ts` (file-state checkpoints)
- `packages/core/src/core/logger.ts` (legacy prompt log + `/chat save` checkpoints)

> Scope note. Gemini CLI keeps **three related on-disk artifacts**, only the
> first of which is the durable session transcript:
>
> 1. **Chat recording JSONL** — one append-only `session-*.jsonl` per session
>    under `<projectTempDir>/chats/`. This is the durable session-as-log and the
>    focus of this dossier (`chatRecordingService.ts`).
> 2. **Shadow-git file-state checkpoints** — a hidden git repo mirroring the
>    workspace, one commit per restorable tool call, referenced by hash from a
>    per-tool-call `checkpoint-*.json` (`gitService.ts`, `checkpointUtils.ts`).
> 3. **Legacy `Logger`** — a global `logs.json` cross-session prompt history and
>    `checkpoint-<tag>.json` full-history snapshots for `/chat save`/`/restore`
>    (`core/logger.ts`).

## The storage model

The durable session is an **append-only JSONL log** whose lines are interpreted
by a replay reducer at load time. The type comment is explicit: "Complete
conversation record stored in session files"
(`chatRecordingTypes.ts:90-104`):

```ts
export interface ConversationRecord {
  sessionId: string;
  projectHash: string;
  startTime: string;
  lastUpdated: string;
  messages: MessageRecord[];
  summary?: string;
  memoryScratchpad?: MemoryScratchpad;
  directories?: string[];
  kind?: 'main' | 'subagent';
}
```

But `ConversationRecord` is the *materialized projection*, not what is stored
line-by-line. On disk the file is a sequence of four line kinds, all appended,
never edited in place (`chatRecordingService.ts:550-576`, `168-346`):

- **An initial metadata line** — a `PartialMetadataRecord { sessionId,
  projectHash, startTime, lastUpdated, kind, directories, ... }`
  (`chatRecordingTypes.ts:131-140`), written once at session start
  (`chatRecordingService.ts:521-530`).
- **Message lines** — full `MessageRecord`s (`id`, `timestamp`, `type`,
  `content`, and for `gemini` messages `toolCalls`, `thoughts`, `tokens`,
  `model`) (`chatRecordingTypes.ts:44-87`). A message is *re-appended in full*
  whenever it changes (tokens arrive, tool results update), and the loader keeps
  the **last** occurrence per `id` (`chatRecordingService.ts:572-587`, `234`).
- **Metadata-update lines** — `{ "$set": { ...partial ConversationRecord } }`,
  merged into metadata at replay; a `$set` carrying a full `messages` array is a
  **checkpoint** that clears and rebuilds the message set
  (`chatRecordingService.ts:243-299`, `566-570`).
- **Rewind markers** — `{ "$rewindTo": "<messageId>" }`, interpreted at replay
  as "drop this message and everything after it"
  (`chatRecordingService.ts:76-78`, `172-202`, `855-875`).

So the on-disk format is an **event log with last-write-wins-by-id message
upserts plus `$set`/`$rewindTo`/checkpoint markers**; the in-memory
`ConversationRecord` (`cachedConversation`) and the `messagesMap` built by
`loadConversationRecord` are the derived projections
(`chatRecordingService.ts:157-352`).

Conceptual model: **session-as-log** (append-only JSONL, replayed) that
projects to a **session-as-document** (`ConversationRecord`). It sits between a
pure event log and a mutable document: writes are append-only, but the message
semantics are "latest full copy per id wins," not immutable events.

## Keying and identity

- **On-disk scope**: sessions live under
  `<projectTempDir>/chats/`, where `projectTempDir` is
  `~/.gemini/tmp/<projectShortId>/` (`storage.ts:154-156`, `181-185`). The
  global root is `~/.gemini` (`GEMINI_DIR`, `storage.ts:54-59`).
- **Project keying is two-headed.** The *directory* uses a
  **`projectShortId`** minted by a `ProjectRegistry` (`projects.json`) mapping
  the absolute project root to a short slug (`storage.ts:218-248`). The
  *record* stores `projectHash = sha256(projectRoot)` inside every
  `ConversationRecord` (`paths.ts:318-320`, `chatRecordingService.ts:415`). The
  short id superseded a legacy `sha256(projectRoot)` directory name, and
  `performMigration` moves the old hash dirs to the new slug dirs
  (`storage.ts:255-273`).
- **Filename encodes time + id**: a main session file is
  `session-<YYYY-MM-DDThh-mm>-<sessionId[0:8]>.jsonl`
  (`chatRecordingService.ts:490-509`; `SESSION_FILE_PREFIX = 'session-'`,
  `chatRecordingTypes.ts:12`). Colons in the timestamp are replaced with `-`.
  The 8-char id slice is what listing/deletion match on ("shortId").
- **Session id minting**: the session id is the runtime `context.promptId`
  (`chatRecordingService.ts:414`, `469`); message ids are `randomUUID()`
  (`chatRecordingService.ts:602`). There is no ordinal/sequence number — order
  is line order, and identity within a session is the message `id`.
- **Listing scope**: per-project. Listing scans one project's `chats/` dir;
  there is no global cross-project session enumeration in the picker
  (`sessionUtils.ts:445-447`).
- **Relocation / rename**: identity of a session is its file; the *project*
  directory is reconciled through the `ProjectRegistry` short-id +
  `performMigration`, so a project whose path scheme changed keeps its sessions
  by migrating the temp/history dirs (`storage.ts:255-273`). A moved *workspace
  root* produces a new short id (and thus a new chats dir); there is no
  automatic re-linking of a session file to a moved root beyond the registry.

## The store interface

There is no pluggable store abstraction; the store is the `ChatRecordingService`
class plus module-level load/delete helpers. Reconstructed contract
(`packages/core/src/services/chatRecordingService.ts`):

- `initialize(resumedSessionData?, kind?) -> Promise<void>` — open or resume.
  New session: mint the file path, `mkdir -p` the chats dir, append the initial
  metadata line, seed `cachedConversation` (`:418-535`). Resume: adopt the
  existing file, load it into `cachedConversation`, migrate a legacy `.json`
  document to `.jsonl`, and append a `$set` updating the session id (`:424-463`).
- `recordMessage({ model, type, content, displayContent?, id? }) -> string` —
  append a new/updated message line and bump `lastUpdated` via `$set`
  (`:610-641`). `recordSyntheticMessage(...)` wraps it (`:647-658`).
- `recordToolCalls(model, toolCalls[])` — attach/merge tool-call records onto the
  last `gemini` message and re-append it (`:699-768`).
- `recordThought(thought)` / `recordMessageTokens(usage)` — queue thoughts and
  fold token usage into the last `gemini` message (`:660-697`).
- `saveSummary(summary)` / `recordDirectories(dirs)` — `$set` metadata updates
  (`:770-786`).
- `rewindTo(messageId) -> ConversationRecord | null` — truncate the in-memory
  messages and append a `$rewindTo` marker (`:855-875`).
- `updateMessagesFromHistory(history)` — reconcile the recorded messages against
  the live model history (masking sync) and, if changed, append a checkpoint
  `$set: { messages }` (`:877-962`).
- `getConversation() / getConversationFilePath()` — read the cached projection
  (`:788-795`).
- `deleteSession(idOrBasename)` / `deleteCurrentSessionAsync()` /
  `deleteCurrentSessionIfNotResumableAsync()` — delete files (`:804-849`).

Module-level readers: `loadConversationRecord(filePath, options?)` — the replay
reducer that folds all line kinds into a `ConversationRecord` (with a
`metadataOnly` fast path and a `maxMessages` window) (`:133-400`); and
`parseLegacyRecordFallback` — parse a whole-file single-JSON legacy record
(`:965-1026`). The takeaway: **every mutation is an append; all read/rewind/
checkpoint semantics are applied by the loader replaying the log**.

## Write and append path (ordering, durability, concurrency, delivery)

- **Append, synchronous, one line at a time.** `appendRecord` does
  `fs.appendFileSync(file, JSON.stringify(record) + '\n')` after `mkdir -p` of
  the parent (`chatRecordingService.ts:550-555`). There is no batching, no
  background writer, no temp-file-and-rename, and no explicit `fsync`. Durability
  is the OS append plus process-synchronous write.
- **Ordering** is purely positional (line order). There is no sequence number or
  ordinal; the loader relies on iteration order to build the `messagesMap` and to
  apply `$rewindTo`/checkpoint markers relative to prior lines
  (`chatRecordingService.ts:168-346`).
- **Message identity + upsert.** Because a changing message is re-appended in
  full and the loader keeps the last occurrence for an `id`
  (`messagesMap.set(id, record)`, `:234`; in-memory upsert at `:572-587`), the
  effective semantics are **last-write-wins per message id**. That is how a
  `gemini` message accumulates tool calls, thoughts, and token counts across
  several appends.
- **Concurrency**: single-writer-per-session in practice — one
  `ChatRecordingService` owns the file for a live session. There is no file lock,
  no optimistic-concurrency token, and no expected-position precondition. Two
  processes appending to the same session file would interleave lines with no
  coordination.
- **Durability / failure handling**: writes are best-effort. On `ENOSPC` the
  service disables recording (`this.conversationFile = null`) and warns that "the
  conversation will continue but will not be saved to disk"
  (`chatRecordingService.ts:46-49`, `540-563`). A crash mid-write can leave a
  torn final line; the loader tolerates it by ignoring per-line parse errors
  (`:343-345`).
- **Delivery semantics**: best-effort, no idempotence id beyond the message `id`
  (which provides dedup on *read* via last-write-wins, not on write). Nothing is
  journaled outside the file.

## Read and resume path

- **Resume is a full ordered replay of the file.** `initialize` with
  `resumedSessionData` calls `loadConversationRecord(filePath)`, which streams the
  JSONL line-by-line via `readline` and folds it into `cachedConversation`
  (`chatRecordingService.ts:429-433`, `133-400`). The file is read start-to-end;
  `$rewindTo` and checkpoint (`$set.messages`) lines rewrite the accumulating
  message set during that pass.
- **Reads the durable store directly.** Resume reads the JSONL file itself; there
  is no separate cache or index consulted first. The in-memory
  `cachedConversation` is rebuilt from the file each time.
- **Legacy `.json` migration on resume.** If the resumed file ends in `.json`
  (an old whole-document format), the service rewrites it as `.jsonl` by
  appending the initial metadata line, every message, and any
  `memoryScratchpad`, then continues in append mode
  (`chatRecordingService.ts:436-460`). `parseLegacyRecordFallback` handles a file
  that is a single JSON object (`:965-1026`).
- **Bounds / pagination.** The loader supports `metadataOnly` (count messages and
  extract the first user message without materializing bodies) and `maxMessages`
  (keep only the last N messages by evicting the oldest map entry)
  (`chatRecordingTypes.ts:118-121`, `chatRecordingService.ts:233-241`).
  `MAX_HISTORY_MESSAGES = 50` and `MAX_TOOL_OUTPUT_SIZE = 50KB`
  (`chatRecordingTypes.ts:13-14`) bound what is surfaced/persisted for tool
  output. There is no hard cap on total file size.
- **What is materialized eagerly**: the full projection is built on load (or just
  counts/first-message under `metadataOnly`). There is no lazy per-turn loading
  from an index.

## Listing, summaries, and search

- **Listing is a directory scan.** `SessionSelector.listSessions` reads
  `<projectTempDir>/chats/`, keeps files starting with `session-` and ending in
  `.json`/`.jsonl`, and loads each with `metadataOnly` to build `SessionInfo`
  (`sessionUtils.ts:409-447`, `234-321`). **Subagent sessions are skipped** in
  the picker — "these are implementation details of a tool call"
  (`sessionUtils.ts:288-290`). Results are sorted by `startTime`
  (`sessions.ts:36-40`). Cost scales linearly with the number of session files;
  no stated scale numbers, no index.
- **Summary sidecar = the metadata lines in the file.** There is no separate
  index file. The denormalized summary fields (`summary`, `lastUpdated`,
  `directories`, `memoryScratchpad`, `kind`) live as `$set`/initial-metadata
  lines inside each session's JSONL and are folded by the loader
  (`chatRecordingService.ts:296-299`). A `summary` is generated lazily for the
  most-recent session before listing (`generateSummary(config)`,
  `sessions.ts:21`).
- **`memoryScratchpad`** is workflow metadata (`workflowSummary`, `toolSequence`,
  `touchedPaths`, `validationStatus`) written via `$set`; the loader tracks
  whether messages appended *after* it make it stale
  (`memoryScratchpadIsStale`) (`chatRecordingTypes.ts:33-39`,
  `chatRecordingService.ts:164-206`, `243-249`).
- **Search**: there is no FTS/vector search over transcripts. Listing extracts a
  `firstUserMessage` for display (`chatRecordingService.ts:215-231`); selection
  is by index, UUID, or short id (`sessions.ts:70-90`, `sessionUtils.ts:419-429`),
  not full-text query.

## Entry/message structure and versioning

- **Envelope**: each line is a bare JSON object discriminated *structurally* by
  the loader, not by a `type`/version tag on the line itself
  (`chatRecordingService.ts:76-97`): `$rewindTo` ⇒ rewind, `$set` ⇒ metadata
  update/checkpoint, `id` present ⇒ message, `sessionId`+`projectHash` ⇒ initial
  metadata.
- **Message payload**: `BaseMessageRecord { id, timestamp, content, displayContent? }`
  plus a `ConversationRecordExtra` union tagged by `type`
  (`'user' | 'info' | 'error' | 'warning'` or `'gemini'` with `toolCalls`,
  `thoughts`, `tokens`, `model`) (`chatRecordingTypes.ts:44-87`). `content` is a
  `@google/genai` `PartListUnion`. Tool calls are `ToolCallRecord`s with
  `status`, `result`, and UI display fields (`chatRecordingTypes.ts:54-67`).
- **Store interpretation**: the store fully parses and interprets lines (it must,
  to apply upserts/rewinds/checkpoints), and it enriches tool calls with registry
  metadata before writing (`chatRecordingService.ts:702-712`). The field it
  relies on for identity/dedup is the message `id`.
- **Versioning**: there is **no explicit schema-version field** on the session
  format. Evolution is handled by (a) additive optional fields on the
  interfaces; (b) **format sniffing** — legacy whole-file `.json` documents are
  detected and migrated to `.jsonl` on resume
  (`chatRecordingService.ts:436-460`, `965-1026`); and (c) `MemoryScratchpad`
  carrying its own `version: 1` literal (`chatRecordingTypes.ts:34`). The
  project-directory scheme has its own migration (`hash → shortId`,
  `storage.ts:255-273`, `config/storageMigration.ts`).

## Compaction and history management

- **Compaction is an upstream concern reflected as a checkpoint in the log.**
  When the model history is compressed/summarized, the live history is
  reconciled into the record via `updateMessagesFromHistory`, which appends a
  **checkpoint** `$set: { messages: newMessages }` replacing the message set
  (`chatRecordingService.ts:877-962`, `945-954`). On replay, a `$set.messages`
  line clears `messagesMap` and rebuilds it from the provided array
  (`:250-294`), so the post-compaction message set supersedes the earlier lines.
- **The durable file keeps everything; the projection shrinks.** Earlier message
  lines remain physically in the JSONL after a checkpoint; the loader simply
  rebuilds from the latest checkpoint forward. So crossing a compaction boundary
  on resume means "adopt the checkpointed message set, then apply any lines
  appended after it."
- A stored `summary` (from `saveSummary`) is the human-facing session summary,
  distinct from context compaction (`chatRecordingService.ts:770-777`).

## Rewind, checkpoints, and fork

- **Rewind is an appended marker interpreted at replay.** `rewindTo(messageId)`
  truncates the in-memory messages and appends `{ "$rewindTo": messageId }`
  (`chatRecordingService.ts:855-875`). On load, the reducer finds that id and
  deletes it plus everything after (or clears all if not found)
  (`:172-202`). The pre-rewind lines are not physically removed — this is the
  append-marker pattern, so the transcript file still contains the rewound turns.
- **File-state checkpoints are a shadow git repo, not inline content.** `/restore`
  (interactive `checkpointing`) uses `GitService`, which maintains a hidden
  "shadow" git repository mirroring the workspace with a dedicated gitconfig and
  ignore rules (`gitService.ts:52-176`). Before a restorable tool call runs,
  `processRestorableToolCalls` calls `createFileSnapshot(message)` to commit the
  current workspace state and records the `commitHash`
  (`checkpointUtils.ts:84-116`; snapshot at `gitService.ts:193-208`). Restore
  runs `git restore --source <hash> .` plus a clean of untracked files
  (`gitService.ts:213-217`). The per-tool-call metadata is a
  `checkpoint-*.json` (`ToolCallData { history, clientHistory, commitHash,
  toolCall, messageId }`, `checkpointUtils.ts:15-24`) named
  `<timestamp>-<fileName>-<toolName>` under `getProjectTempCheckpointsDir()`
  (`generateCheckpointFileName`, `checkpointUtils.ts:48-67`;
  `storage.ts:313-315`). So file snapshots are git-deduplicated commits, cheap by
  content addressing, keyed to a specific tool call and message id.
- **Legacy `/chat save <tag>` checkpoints** are a separate mechanism: the
  `Logger` writes a full `checkpoint-<tag>.json` snapshot of the conversation
  history to the project `.gemini` dir (`core/logger.ts:285-345`), independent of
  the shadow-git file checkpoints.
- **Fork**: there is no first-class fork/branch operation on the chat record. The
  closest primitive is resume (adopt an existing file and continue appending,
  updating the session id via `$set`, `chatRecordingService.ts:462-463`), which
  continues the same file rather than branching a new lineage.

## Subagents and nested sessions

- **A subagent is a nested session file under the parent.** When `kind ===
  'subagent'` and a `parentSessionId` is present, the chats dir becomes
  `<projectTempDir>/chats/<sanitizedParentSessionId>/`, and the child file is
  `<sanitizedSessionId>.jsonl` (no `session-` prefix, no timestamp)
  (`chatRecordingService.ts:475-509`). The child records its own workspace
  directories (`:512-519`).
- **Durable parent-child link** is *path-based* (the child lives in a directory
  named for the parent's session id) plus the `kind: 'subagent'` field on the
  record (`chatRecordingTypes.ts:103`). There is no id pointer field on the
  child record beyond the directory nesting.
- **Isolated transcript**: the child has its own JSONL and its own message
  stream; it does not write into the parent's file. The child is *excluded* from
  the resume picker (`sessionUtils.ts:288-290`).
- **Cascade on delete**: deletion is designed to cascade. `deleteStoredSession`
  derives an 8-char short id and "finds and deletes all associated files (parent
  and subagents)" (`chatRecordingService.ts:797-806`). Nesting is effectively one
  level (subagents under a parent id); deeper nesting bounds were not traced.

## Retention, deletion, and multi-host

- **Retention**: the product, not a store layer, owns cleanup. There is a
  `sessionCleanup` utility (`packages/cli/src/utils/sessionCleanup.ts`) and
  `deleteCurrentSessionIfNotResumableAsync`, which deletes a session on exit if
  it has no resumable content (no real user prompt, model response, or tool
  activity) (`chatRecordingService.ts:839-849`, `127-131`). No TTL/age-based
  expiry of transcripts was found in the recording service itself.
- **Deletion**: `deleteCurrentSessionAsync` unlinks the tracked JSONL and then
  delegates tool-output/log cleanup to `deleteSessionArtifactsAsync`
  (`chatRecordingService.ts:813-832`). `deleteStoredSession` cascades to parent +
  subagent files by short id (`:804-806`). Deletion is a filesystem unlink; there
  is no tombstone in an append-only sense.
- **Multi-host**: single-host, local-filesystem only. The store assumes a local
  `~/.gemini/tmp/<projectShortId>/` tree, synchronous appends, and one live
  writer per session. There is no shared-filesystem coordination, remote
  writeback, lease, or crash-detection protocol. (A separate `a2a-server`
  package has its own GCS persistence for the agent-to-agent server surface —
  `packages/a2a-server/src/persistence/gcs.ts` — but that is a different product
  surface, not the CLI's session store.)

## Interop with foreign session stores

Not applicable. Gemini CLI reads only its own session formats: current `.jsonl`
and legacy whole-file `.json` records (`chatRecordingService.ts:436-460`,
`965-1026`). No discovery/import/resume of another product's native session
store was found.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* Gemini CLI's durable session is **an
append-only per-session JSONL log whose lines are folded by a replay reducer
into a `ConversationRecord` projection**, with rewind and compaction expressed
as appended markers (`$rewindTo`, checkpoint `$set: {messages}`) rather than
in-place edits. That is directionally the same append-only-log-with-projection
shape our design targets, but implemented loosely — the message semantics are
"latest full copy per id wins," not immutable events. Lessons:

- **Markers-at-replay for rewind and compaction** (`$rewindTo`, checkpoint
  `$set.messages`) confirm the pattern is workable on a plain file: never edit
  prior lines; append a marker; let the reducer recompute the view
  (`chatRecordingService.ts:172-202`, `250-299`). We should keep this shape but
  add the rigor Gemini omits.
- **Structural discrimination + last-write-wins-by-id is fragile.** Lines carry
  no explicit type tag or schema version, and ordering has no sequence number, so
  correctness depends entirely on physical line order and on the loader's
  heuristics (`chatRecordingService.ts:76-97`). Our store should use explicit
  entry types, a monotonic sequence/ordinal, and immutable events instead of
  re-appended full-message upserts — the Codex/OpenCode ordinal approach is the
  contrast to follow.
- **File-state checkpoints via a content-addressed shadow git repo**
  (`gitService.ts`, `checkpointUtils.ts`) are a cheap, dedup'd way to snapshot the
  workspace per tool call and restore by commit hash — a good model for our
  environment-checkpoint story, keyed to a specific turn/message id.
- **Directory-based subagent nesting** (`chats/<parentId>/<childId>.jsonl`) is
  simple but couples identity to path; a first-class parent pointer (as in
  Codex/OpenCode) is more robust to relocation. We should prefer an explicit
  lineage field.
- **Project keying drift**: the record's `projectHash = sha256(root)` and the
  directory's `shortId` from a registry can diverge, and a moved root yields a new
  chats dir with no automatic session re-linking (`storage.ts:181-273`). Our
  design needs a stable session key independent of cwd, plus explicit relocation
  reconciliation.
- **Cautions**: (1) **Best-effort durability** — synchronous append with no
  fsync, silent disable on `ENOSPC`, torn-line tolerance on read
  (`chatRecordingService.ts:540-563`, `343-345`); a hard crash can lose the last
  write. (2) **No concurrency control** — single-writer assumption with no lock
  or expected-version; unsafe for multi-writer/multi-host. (3) **Linear-scan
  listing** with no index; scales poorly with many sessions. (4) **No log
  compaction/retention** of the JSONL itself — rewound and superseded lines
  persist in the file indefinitely, so the file grows even as the projection
  shrinks.

## Open questions

- **fsync / crash-consistency**: `appendFileSync` provides no durability barrier;
  the exact behavior under power loss (vs. clean process exit) was not tested.
- **Concurrent resume of the same session** from two processes: there is no lock;
  the resulting interleaving and last-write-wins outcome was not exercised.
- **Subagent nesting depth and cascade correctness**: deletion claims to remove
  "parent and subagents" by 8-char short id (`chatRecordingService.ts:797-806`);
  multi-level nesting and short-id collision behavior were not verified.
- **Relationship between the three artifacts on delete**: whether deleting a
  session also prunes its shadow-git commits and `checkpoint-*.json` files (vs.
  only tool outputs and logs) was not fully traced.
- **`logs.json` retention**: the legacy `Logger` prompt history
  (`core/logger.ts:15`) accumulates across sessions; its pruning/retention policy
  was not examined.

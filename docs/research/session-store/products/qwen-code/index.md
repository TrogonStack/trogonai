# Qwen Code: what diverged from Gemini CLI's session store

Part of Session Store Research.
Fork delta report; see [backlog](../../backlog.md) Wave 5 for why this is a delta rather than a dossier.
Qwen Code pinned at `06cc41ee3f50845c05f518d072e5175910b91f7e` (Apache-2.0), compared against the accepted [Gemini CLI dossier](../gemini-cli/index.md). Retrieved 2026-08-04.

## Summary of divergence

Qwen Code diverged substantially, more than a typical fork in this class. It
kept the shape "append-only JSONL per session, replayed on resume" but
rewrote almost everything under that shape: the storage root was renamed
(`~/.gemini` → `~/.qwen`) with no migration path for existing Gemini CLI
users; the on-disk record format was redesigned from Gemini's positional,
structurally-discriminated line kinds into an explicit `type`/`subtype`
tagged, `uuid`/`parentUuid` tree; project keying moved from an opaque
sha256-hash-plus-short-id-registry scheme to a human-readable
sanitized-cwd directory name; the shadow-git file-state checkpoint
mechanism was replaced by a file-copy-based history service; a
process-level write concurrency control was added where Gemini had none;
and the single directory-nested subagent model was replaced by three
different subagent/child-session mechanisms plus a first-class session
fork (`/branch`). None of this is a cosmetic rebrand -- each item changes
either on-disk layout, read/replay semantics, or what a session even *is*.

## What diverged

### Storage root renamed, no migration from Gemini CLI

- The global root is `.qwen` (`qwen: packages/core/src/utils/paths.ts:14`,
  `QWEN_DIR = '.qwen'`), resolved by `Storage.getGlobalQwenDir()`
  (`qwen: packages/core/src/config/storage.ts:183-193`), versus Gemini's
  `~/.gemini` (`GEMINI_DIR`) per the dossier's keying section.
- No migration of session/config data from a legacy `~/.gemini` tree was
  found. The only "legacy dir" logic in Qwen concerns a user-configured
  `QWEN_HOME` pointing somewhere other than the default `~/.qwen` -- it warns
  that "OAuth tokens, settings, memory, extensions, and skills are not
  auto-migrated" between two *Qwen* homes, not from Gemini
  (`qwen: packages/cli/src/config/settings.ts:659-690`,
  `detectQwenHomeRedirectWithoutMigration`). Targeted greps for `.gemini`,
  `GEMINI_DIR`, and `legacyDir`-style names in `packages/core/src` and
  `packages/cli/src` turned up nothing that reads or migrates a prior
  Gemini CLI installation's session data. A user arriving from Gemini CLI
  with sessions under `~/.gemini/tmp/<projectShortId>/chats/` gets a fresh,
  empty `~/.qwen` tree; their old sessions are not orphaned by breakage, they
  are simply never looked at.

### Durable record format redesigned: tree of tagged records, not positional line kinds

- Gemini's format (per the dossier) is untagged lines discriminated
  structurally by the loader (`$rewindTo` / `$set` / message-with-`id` /
  initial-metadata), with last-write-wins-by-message-`id` upserts and no
  sequence number (dossier, "Entry/message structure and versioning").
- Qwen's `ChatRecord` carries explicit `type: 'user' | 'assistant' |
  'tool_result' | 'system'` and an explicit `subtype` enumerating over a
  dozen record kinds (`chat_compression`, `slash_command`, `ui_telemetry`,
  `at_command`, `attribution_snapshot`, `notification`, `custom_title`,
  `parent_session`, `session_source`, `rewind`, `agent_bootstrap`,
  `file_history_snapshot`, `user_text_elements`, `session_artifact_event`,
  `session_artifact_snapshot`, `goal_state`, `goal_runtime`, ...)
  (`qwen: packages/core/src/services/chatRecordingService.ts:265-393`).
  Every record has `uuid` and `parentUuid` -- "Forms a tree structure via
  uuid/parentUuid for future conversation branching support"
  (`qwen: packages/core/src/services/chatRecordingService.ts:230-238`).
  Records are never re-appended/upserted; each is written once and is
  self-contained.

### Rewind reimplemented as branch re-rooting, not a positional truncation marker

- Gemini appends `{ "$rewindTo": messageId }`; the replay reducer finds that
  id and deletes it plus everything after it on load (dossier, "Rewind,
  checkpoints, and fork").
- Qwen's `rewindRecording` re-points the in-memory `lastRecordUuid` (the
  parent pointer the *next* appended record will use) back to the record
  before the target turn, appends a `system`/`rewind` record for the audit
  trail, and lets subsequent writes form a new branch off that point
  (`qwen: packages/core/src/services/chatRecordingService.ts:1686-1719`).
  The rewound records are not deleted; they remain in the file as an
  abandoned parentUuid branch that a tree-walk from the tail never visits.
  Functionally similar end state (old turns invisible on replay, physically
  retained) but the mechanism is graph re-rooting, not "scan forward from an
  id and drop lines."

### First-class fork (`/branch`), which Gemini's dossier says does not exist upstream

- Gemini: "there is no first-class fork/branch operation on the chat
  record... resume... continues the same file rather than branching a new
  lineage" (dossier, "Rewind, checkpoints, and fork").
- Qwen's `SessionService.forkSession` reads the full source transcript,
  reconstructs only the *active* branch (explicitly excluding abandoned
  post-rewind records -- "Rewind leaves old records in the JSONL as abandoned
  parentUuid branches; copying raw records would resurrect them"), strips
  `parent_session`/`session_source` records so the fork is attributed as a
  fresh top-level session, and writes the result to a new session file
  (`qwen: packages/core/src/services/sessionService.ts:1690-1740`).

### Project keying scheme changed, with a new collision-handling code path as a direct consequence

- Gemini keys the chats directory by an opaque `projectShortId` minted by a
  `ProjectRegistry` (`projects.json`), separately from a `sha256(projectRoot)`
  stored in the record, and migrates old hash-named directories to the new
  short-id scheme (dossier, "Keying and identity").
- Qwen has no equivalent registry. `Storage.getProjectDir()` derives the
  directory name directly from the project root via `sanitizeCwd`, which
  just replaces every non-alphanumeric character with a hyphen
  (`qwen: packages/core/src/config/storage.ts:346-349`,
  `qwen: packages/core/src/utils/paths.ts:385-389`; chats live at
  `<projectDir>/chats`, `qwen: packages/core/src/services/chatRecordingService.ts:768-769`).
  A separate `getProjectTempDir()` keyed by `sha256` still exists for
  temp/debug output (`qwen: packages/core/src/config/storage.ts:352-357`)
  but is not what backs `chats/`.
  Because `sanitizeCwd` is a lossy many-to-one mapping (e.g. two absolute
  paths that differ only in punctuation can sanitize to the same string),
  `SessionService.listSessions` has to defend against directory collisions
  that did not exist under Gemini's hash-based scheme: "Different projects
  may share the same chats directory due to path sanitization, so we need to
  filter by project hash and continue until we have enough items"
  (`qwen: packages/core/src/services/sessionService.ts:982-984`, filtered via
  `sessionBelongsToCurrentProject`). This is a new failure mode introduced by
  the rename, not present in the scheme it replaced.

### Session identity minting changed

- Gemini mints the session id from the runtime `context.promptId` (dossier,
  "Keying and identity").
- Qwen mints an independent `randomUUID()` as `Config.sessionId` at config
  construction (`qwen: packages/core/src/config/config.ts:2087`), and
  `ChatRecordingService` uses that (or a `binding`-supplied override) as the
  session id (`qwen: packages/core/src/services/chatRecordingService.ts:764-765`).
  Session identity is decoupled from the prompt-id concept entirely.

### File-state checkpointing swapped from a shadow-git repo to a file-copy history service

- Gemini's `/restore` checkpointing is a hidden shadow git repository
  (`GitService`) that commits the workspace before each restorable tool call
  and restores via `git restore --source <hash>` (dossier, "Rewind,
  checkpoints, and fork"; `gitService.ts`, `checkpointUtils.ts`).
- No `GitService`, `gitService.ts`, or `checkpointUtils.ts` equivalent exists
  in Qwen Code (targeted searches for `GitService`, `createFileSnapshot`,
  `restorableToolCall`, and `shadow git` in `packages/core/src` and
  `packages/cli/src` found nothing). Qwen instead has
  `FileHistoryService` (`qwen: packages/core/src/services/fileHistoryService.ts`),
  which makes per-file backup copies (via `copyFile`, hashed content,
  diffed with the `diff` npm package) under a `file-history` directory
  (`qwen: packages/core/src/services/fileHistoryService.ts:103-104`,
  `MAX_SNAPSHOTS = 100`, `FILE_HISTORY_DIR = 'file-history'`), keyed by
  `promptId` rather than by tool-call/commit hash, and recorded into the
  transcript itself as `file_history_snapshot` system records
  (`qwen: packages/core/src/services/chatRecordingService.ts:2041-2060`,
  `recordFileHistorySnapshot`/`recordFileHistorySnapshotBatch`). This is a
  different mechanism (file copies + diff, not content-addressed git
  commits), not a rename of the same one.
- Gemini's legacy `Logger` (`logs.json`, `/chat save <tag>` →
  `checkpoint-<tag>.json`) *does* survive in Qwen essentially unchanged in
  shape, just under `.qwen` instead of `.gemini`
  (`qwen: packages/core/src/core/logger.ts:491-654`,
  `_checkpointPath`/`saveCheckpoint`/`loadCheckpoint`/`checkpointExists`) --
  see "What did not diverge" below.

### Concurrency control added where Gemini's dossier flags none

- Gemini's dossier lists "No concurrency control -- single-writer assumption
  with no lock or expected-version; unsafe for multi-writer/multi-host" as a
  caution (dossier, "What this implies for our Session Store").
  Qwen added `SessionWriterLease`
  (`qwen: packages/core/src/services/session-writer-lease.ts`), a PID-based
  lock file with a schema version (`LOCK_SCHEMA_VERSION = 2`,
  `qwen: packages/core/src/services/session-writer-lease.ts:17`), transcript
  snapshot hashing to detect drift, and typed failure modes
  (`SessionTranscriptChangedError`, `SessionWriterLostError`,
  `SessionWriterUnavailableError`,
  `qwen: packages/core/src/services/chatRecordingService.ts:40-45`). Writes
  are serialized through a per-service `operationTail` promise chain and, when
  a lease is held, appended via `lease.appendJsonLine`
  (`qwen: packages/core/src/services/chatRecordingService.ts:943-971`) -- a
  materially different durability/concurrency story than Gemini's bare
  `fs.appendFileSync`.

### Subagent/child-session model replaced with three distinct mechanisms; no cascade delete

Gemini has one subagent mechanism: a child session file nested under
`chats/<parentId>/<childId>.jsonl`, whose deletion is designed to cascade --
`deleteStoredSession` "finds and deletes all associated files (parent and
subagents)" (dossier, "Subagents and nested sessions"). Qwen has three,
none of which nest a child file under the parent's directory, and none of
which is cascade-deleted with the parent:

1. **`create_sub_session`** spawns "a FRESH top-level sub-session (a sibling
   of the current session, its own transcript)"
   (`qwen: packages/core/src/tools/create-sub-session.ts:8-9`). It lives as
   an ordinary file in the same `chats/` directory as its parent, linked only
   by a soft `parent_session` system record
   (`ParentSessionRecordPayload { parentSessionId }`,
   `qwen: packages/core/src/services/chatRecordingService.ts:507-515`,
   written at `qwen: packages/core/src/tools/agent/agent.ts:3280`, `:4081`)
   -- a pointer, not directory nesting.
2. **Background subagents** get their own directory,
   `<projectDir>/subagents/<sessionId>/`, holding a canonical
   `agent-<id>.jsonl`, an `agent-<id>.meta.json` sidecar (agentType,
   description, parent ids, createdAt), and a transient
   `agent-<id>.jsonl.stream` for in-flight text
   (`qwen: packages/core/src/agents/agent-transcript.ts:9-20`,
   `getSubagentsRootDir`/`getSubagentSessionDir` at `:53-77`). This is the
   closest analog to Gemini's nested-child model, but the directory is
   `subagents/`, not `chats/<parentSessionId>/`, and it carries a metadata
   sidecar Gemini's model doesn't have.
3. **Inline sidechain records**: `ChatRecord` carries `isSidechain`,
   `agentId`, `agentColor`, `agentRunId`, `agentRound` fields
   (`qwen: packages/core/src/services/chatRecordingService.ts:361-374`), set
   from `packages/core/src/tools/agent/agent.ts` (e.g. `agentId`/`agentColor`
   at `:3261-3263`), letting some subagent activity live as tagged records
   interleaved in the *parent's own* transcript rather than a separate file
   at all.

**Cascade on delete does not exist for either of the divergent mechanisms.**
`SessionService.removeSession` / `removeSessionFiles` deletes the session's
own transcript, worktree sidecars, and file-history backups
(`qwen: packages/core/src/services/sessionService.ts:1352-1417`,
`removeWorktreeSidecars` at `:554-562`, `removeFileHistoryBackups` at
`:563-569`), but never references `subagents/` or a `create_sub_session`
child. Background-subagent transcripts are instead reaped independently by
an age/TTL housekeeping job (`cleanupOldSubagentTranscripts`, scheduled at
`qwen: packages/cli/src/utils/housekeeping/scheduler.ts:175-194`), not tied
to parent-session deletion at all. Deleting a session that spawned
`create_sub_session` children leaves those children on disk indefinitely as
ordinary independently-listed sessions (they are full top-level sessions, so
this is arguably correct for that mechanism, but it means "delete cascades
to children" -- true in Gemini -- is false in Qwen for every mechanism except
possibly the fully-nested rewind-branch case, which isn't a parent/child
relationship at all).

## What did not diverge

- **The storage medium is still append-only JSONL**, one file per session,
  read by streaming/parsing the file directly on resume -- same shape as
  Gemini's `session-*.jsonl`, just with a redesigned record schema (see
  above). Cosmetic-only in the sense that "it's still JSONL append" holds;
  substantive in every other respect covered above.
- **Listing is still per-project**, a directory scan/cursor walk over one
  project's `chats/` dir (`qwen: packages/core/src/services/sessionService.ts:943-1067`),
  matching Gemini's "no global cross-project session enumeration" behavior
  (dossier, "Listing, summaries, and search") -- Qwen added pagination
  (`cursor`/`size`) and an archive/active split on top, but the scope (one
  project) is unchanged.
- **Resume is still "read the durable store directly, no separate cache or
  index"** -- same principle as Gemini's, just reconstructing a tree instead
  of replaying line kinds.
- **The legacy `Logger` (`logs.json`, `/chat save`/`checkpoint-<tag>.json`)
  survives essentially unchanged in shape**
  (`qwen: packages/core/src/core/logger.ts:15-79`, `:491-654`) -- this is a
  genuine cosmetic rename (`.gemini` → `.qwen` root only): same file names,
  same tag-encoding scheme (`encodeTagName`/`decodeTagName`,
  `qwen: packages/core/src/core/logger.ts:46-69`), same API shape
  (`saveCheckpoint`/`loadCheckpoint`/`checkpointExists`/`deleteCheckpoint`).
  It does not change on-disk layout beyond the root rename and does not
  break reading existing data in place (there simply is no existing `.qwen`
  data for a new user, per the migration gap above).
- **No multi-host / shared-filesystem coordination was added.** The new
  `SessionWriterLease` is a local PID-based file lock, not a remote/shared
  coordination protocol -- same single-host assumption as Gemini's model, just
  with a lock where Gemini had none.

## What this adds to the corpus

Qwen Code is independent evidence, not a restatement of Gemini CLI's design.
It shares Gemini's original append-only-JSONL-with-projection lineage (the
file names, the `Storage`/`paths` module split, and the legacy `Logger` are
recognizably descended from the same code), but the actual session record
schema, project-keying scheme, concurrency model, file-checkpoint mechanism,
and subagent model have all been independently redesigned and now differ in
ways that matter for anyone building a store: an explicit tagged/tree
format instead of positional-line discrimination, a real write-lease instead
of none, and three competing subagent mechanisms instead of one consistent
one. A design survey should treat Qwen Code as its own data point on tree-
structured (`uuid`/`parentUuid`) session formats and on file-lease-based
single-writer enforcement, not as "Gemini CLI with a different folder name."

## Open questions

- **Whether upstream Gemini CLI has since added anything comparable** (a
  write lease, a fork/branch operation, tree-structured records) after the
  dossier's pinned commit (`87f785192c34067e4e8f26bda16cf9ce24014d83`,
  2026-07-23) could not be checked -- no local Gemini CLI clone was available
  for this delta, per instructions. Everything above is stated relative to
  that pinned dossier, not to Gemini CLI's current `main`.
- **Whether any code path reads a legacy `~/.gemini` tree as a fallback**
  (e.g. a first-run importer not wired into the modules searched) was not
  exhaustively ruled out -- the searches covered `packages/core/src` and
  `packages/cli/src` for `.gemini`, `GEMINI_DIR`, and `legacyDir`-shaped
  names and found nothing, but Qwen Code is a large multi-package monorepo
  (it also contains `packages/desktop`, `packages/vscode-ide-companion`,
  `packages/channels`, etc.) that was not fully swept.
- **Retention/TTL default for the background-subagent housekeeping job**
  (`qwen: packages/cli/src/utils/housekeeping/scheduler.ts:175-194`) was not
  traced to its configured cutoff value.
- **Collision probability/handling correctness of `sanitizeCwd`** beyond the
  defensive filter in `listSessions` -- e.g. whether two genuinely different,
  colliding project roots can have their sessions cross-contaminate outside
  the listing path (writes, not just reads) -- was not traced end to end.

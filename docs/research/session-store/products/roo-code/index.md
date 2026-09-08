# Roo Code: what diverged from Cline's session store

Part of Session Store Research.
Fork delta report; see [backlog](../../backlog.md) Wave 5 for why this is a delta rather than a dossier.
Roo Code pinned at `b867ec9145750d0ae1ff7f02d35406e9bf2a0b16` (Apache-2.0, committed
2026-05-15), compared against Cline at `5ec2d47b21b3a09aa7a094bfbbe0c7e8f7ddd3fa`
(Apache-2.0, committed 2026-08-03). Retrieved 2026-08-04.
Upstream reference: [Cline](../cline/index.md).

## Summary of divergence

Roo Code forked before Cline's `sdk/packages/core` rewrite existed, and never
adopted it: at this pin, Roo Code is still entirely on the architecture Cline's
dossier calls "Generation 1" -- two per-task flat files
(`api_conversation_history.json`, `ui_messages.json`), no database, full-file
rewrite on every save, one JSON file per concern. That alone is the headline
finding and it dates the fork to before Cline's SQLite/manifest/messages-file
generation. Within that shared generation-1 skeleton, however, Roo Code has
independently built real infrastructure Cline's generation 1 never had: a
crash-safe write path (temp-file-plus-rename with cross-process locking), a
`history_item.json` + `_index.json` indexing layer with filesystem-watch-driven
reconciliation, a non-destructive tag-and-filter compaction scheme, and a
recursive (not one-level) cascade delete for its parent/child task tree.
Checkpoints are a real second git repository per task (`git init` + `core.worktree`
+ ordinary commits), not Cline's private-ref-plus-`git stash create` mechanism
inside the user's own repo, and not Cline's docs' claimed "shadow repository"
either -- Roo Code is the product that actually does what Cline's docs describe.
Net: the transcript *format* did not diverge (same generation, same file
names), but nearly every mechanism built around that format did.

## What diverged

### 1. Checkpoints: a real second git repository, not refs-in-the-real-repo

Cline (current generation) stores checkpoints as private refs
(`refs/cline/checkpoints/{sessionId}/{runCount}`) created via `git stash create`
directly inside the user's own repository -- no second `.git` directory
(`cline: sdk/packages/core/src/hooks/checkpoint-hooks.ts`, cited in the Cline
dossier's Rewind/checkpoints section).

Roo Code instead creates an actual second git repository per task, with a
separate `.git` directory living under the extension's global storage and
`core.worktree` pointed at the real workspace:

- `roo: src/services/checkpoints/ShadowCheckpointService.ts:125` --
  `this.dotGitDir = path.join(this.checkpointsDir, ".git")`.
- `roo: src/services/checkpoints/ShadowCheckpointService.ts:175-186` --
  `git.init(...)`, `git.addConfig("core.worktree", this.workspaceDir)`,
  then an ordinary `git.commit("initial commit", { "--allow-empty": null })`.
- `roo: src/services/checkpoints/ShadowCheckpointService.ts:295-341` --
  `saveCheckpoint()` does `stageAll` (`git add . --ignore-errors`) followed by
  a normal `git.commit(message)` -- full commits via the `simple-git` package,
  not `git stash create`.
- `roo: src/services/checkpoints/RepoPerTaskCheckpointService.ts:6-15` -- the
  concrete class used in production wires the shadow repo's directory to
  `{shadowDir}/tasks/{taskId}/checkpoints`, where `shadowDir` is the
  extension's `globalStorageUri.fsPath`
  (`roo: src/core/checkpoints/index.ts:62-73`, `getCheckpointService()`
  passes `shadowDir: globalStorageDir`). So the shadow repo's own `.git`
  physically lives at
  `{globalStorage}/tasks/{taskId}/checkpoints/.git`, nested inside the same
  per-task directory as the transcript files.
- Restore is `git clean -f -d` + `git reset --hard <commitHash>`
  (`roo: src/services/checkpoints/ShadowCheckpointService.ts:344-372`) -- no
  safety-net stash/ref is taken first, unlike Cline's
  `beginWorktreeRestoreTransaction` (noted in the Cline dossier as a
  pre-restore safety net Cline takes that Roo Code has no counterpart for, as
  far as this pass found).

This means Roo Code is closer to what Cline's own user-facing docs describe
("Cline maintains a shadow Git repository separate from your project's actual
Git history") than Cline's actual current code is -- the Cline dossier flags
that doc passage as contradicted by Cline's source; it is an accurate
description of Roo Code's mechanism instead.

A secondary, apparently vestigial code path exists for a *shared-per-workspace*
shadow repo with one git branch per task (`roo-${taskId}`):
`roo: src/services/checkpoints/ShadowCheckpointService.ts:440-516`
(`workspaceRepoDir()`, `deleteTask()`, `deleteBranch()`). `deleteTask()` is
still called from the task-deletion path
(`roo: src/core/webview/ClineProvider.ts:1786`), but it targets
`workspaceRepoDir()` (`{globalStorage}/checkpoints/{sha256(workspaceDir).slice(0,8)}`),
a different path than the repo-per-task directory `RepoPerTaskCheckpointService`
actually uses for live checkpoints. It is plausible this is a best-effort
cleanup for an older layout that a prior Roo Code version used, now dead in
practice because the directory it targets is never created by the live
`RepoPerTaskCheckpointService` path -- not confirmed by tracing git blame; flagged
under Open questions rather than asserted.

### 2. Compaction: non-destructive in-band tagging, not a separate sidecar file

Cline's compaction writes a separate sidecar artifact
(`{sessionId}.compaction.json`) and leaves the messages file untouched
(Cline dossier, Compaction and history management).

Roo Code has no sidecar file. Its condense step rewrites the *same*
`api_conversation_history.json` in place, but non-destructively: it does not
delete the original messages, it tags them:

- `roo: src/core/condense/index.ts:445-474` -- `summarizeConversation()`
  mints a `condenseId = crypto.randomUUID()`, appends a new message with
  `isSummary: true, condenseId`, and tags every pre-existing message with
  `condenseParent: condenseId` (skipping any that already carry one).
- `roo: src/core/task/Task.ts:1686` -- `condenseContext()` calls
  `await this.overwriteApiConversationHistory(messages)`, and
  `roo: src/core/task/Task.ts:1016-1019` shows `overwriteApiConversationHistory`
  writes the *entire* returned array (original tagged messages + new summary)
  back to disk via `saveApiConversationHistory` -- a full-file rewrite of the
  one durable transcript, not a second file.
- `roo: src/core/condense/index.ts:602-682` -- read-side filtering
  (`condenseParent`/`truncationParent` pointing at a still-existing
  summary/marker) decides what actually gets sent to the model API; orphaned
  tags (e.g. after a rewind deletes the summary message itself) are cleaned up
  on the next pass.
- `roo: src/core/task-persistence/apiMessages.ts:26-37` -- the `ApiMessage`
  type itself carries `condenseId`/`condenseParent`/`truncationId`/
  `truncationParent`/`isTruncationMarker` fields, i.e. this is a persisted
  part of the message schema, not a runtime-only concept.

Both products claim "non-destructive" compaction, but the mechanism differs:
Cline keeps the full transcript in one file and shrinks the *model-visible*
view in a second, independently-versioned file; Roo Code keeps everything in
one file and filters at send-time using tags baked into the persisted
records. Roo Code additionally supports a separate, older sliding-window
*truncation* path alongside condensing (`truncationId`/`truncationParent`/
`isTruncationMarker` fields, used at
`roo: src/core/task/Task.ts:3797`, `roo: src/core/task/Task.ts:4024`) -- a
second context-shrinking mechanism with no clear Cline counterpart identified
in this pass.

### 3. A history-index layer Cline's generation 1 never built (and generation 2 built differently)

Cline gen 1 kept task history in extension global state, and its own code
contains an abandoned stub for migrating that into a per-task file
(`cline: apps/vscode/src/core/storage/state-migrations.ts:65-67`,
`migrateTaskHistoryToFile`, body is `// TODO migrate to sdk location`, per the
Cline dossier's Entry/message structure and versioning section -- never
implemented at Cline's pinned commit).

Roo Code implemented this migration and the file layout it stubs out never got
around Cline's own gen-1 code:

- `roo: src/core/task-persistence/TaskHistoryStore.ts:1-72` -- each task's
  `HistoryItem` is now its own file,
  `{globalStorage}/tasks/{taskId}/history_item.json`
  (`roo: src/shared/globalFileNames.ts:7`), with a single cache/index file
  `{globalStorage}/tasks/_index.json`
  (`roo: src/shared/globalFileNames.ts:8`) for fast startup listing.
- `roo: src/core/task-persistence/TaskHistoryStore.ts:325-360` --
  `migrateFromGlobalState()` walks a legacy globalState `taskHistory` array
  and writes a `history_item.json` for any entry whose task directory still
  exists on disk, then rewrites the index -- an idempotent one-time migration,
  explicitly the migration Cline's own stub never implemented.
  `roo: src/core/webview/ClineProvider.ts:172` wires
  `new TaskHistoryStore(...)` into the live provider, so this is active code,
  not a dead utility.
- `roo: src/core/task-persistence/TaskHistoryStore.ts:465-508` -- cross-window
  reactivity via `fs.watch` on the tasks directory (debounced reconcile), plus
  a 5-minute periodic reconciliation
  (`roo: src/core/task-persistence/TaskHistoryStore.ts:514-529`) as a
  defensive fallback where `fs.watch` is unreliable. Cline's dossier records
  no equivalent watch-based cross-instance mechanism for its gen-1 files; its
  gen-2 manifest/index staleness detection instead relies on lazy
  reconciliation at listing time (`reconcileDeadSessions`, PID-liveness based)
  -- a different mechanism solving a related but not identical problem
  (crash/staleness detection vs. cross-window index freshness).

This is convergent evolution on the general shape ("small index file plus
per-item source-of-truth plus reconciliation"), not a copy of Cline's
SQLite/manifest design -- no schema validation library (Zod or otherwise), no
database, and a different trigger (`fs.watch` + timer vs. lazy
listing-time PID check).

### 4. Write durability: temp-file-plus-rename and cross-process locking, where Cline gen 1 has neither

Cline's classic generation-1 writes (`saveTaskMetadata` and siblings) are
plain `fs.writeFile`/`writeFileSync` calls with no atomic rename, per the
Cline dossier's storage-model section.

Roo Code's equivalent writes for the *same-named* files
(`api_conversation_history.json`, `ui_messages.json`, `history_item.json`,
`_index.json`) all go through a shared helper that is meaningfully more
durable:

- `roo: src/utils/safeWriteJson.ts:33-79` -- acquires an inter-process
  advisory lock via `proper-lockfile` (stale-after-31s, exponential-backoff
  retries) before touching the file.
- `roo: src/utils/safeWriteJson.ts:86-115` -- writes to a temp file, renames
  any existing target to a backup path, then renames the temp file onto the
  target (`fs.rename` as the commit step) -- a real temp-write-then-rename
  pattern, with the old version preserved as a `.bak` file until cleanup
  succeeds.
- Used by `roo: src/core/task-persistence/apiMessages.ts:120`
  (`saveApiMessages`), `roo: src/core/task-persistence/taskMessages.ts:55`
  (`saveTaskMessages`), and `roo: src/core/task-persistence/TaskHistoryStore.ts:442`
  (`writeTaskFile`).

This closes (for these specific files) the torn-write risk the Cline dossier
calls out as a genuine gap in Cline's own messages/manifest files -- a
divergence in the safe direction, though it does not change the shared
finding below that both products still rewrite the *entire* file on every
save (no append-only log, no bounded per-write cost).

### 5. Cascade delete on parent task removal: recursive, not one level deep

Cline's cascade delete only looks one level down from a root session and
does not recurse into a deleted child's own children
(`cline: sdk/packages/core/src/session/services/persistence-service.ts:557-609`,
gated on `if (!row.isSubagent)`, per the Cline dossier's Subagents section,
which calls this "a latent orphan path" for graphs deeper than one level).

Roo Code's task-history deletion walks the full child tree recursively before
deleting anything:

- `roo: src/core/webview/ClineProvider.ts:1737-1763` --
  `deleteTaskWithId()`'s `collectChildIds()` recurses through
  `historyItem.childIds` at every depth, building the full set of descendant
  task ids before any deletion happens (not gated on the target task being a
  root -- it recurses from whichever id is passed in).
- `roo: src/core/webview/ClineProvider.ts:1774-1803` -- all collected ids are
  removed from the history store in one batch (`taskHistoryStore.deleteMany`),
  then each id's shadow-checkpoint repo and task directory are individually
  removed.

Roo Code's parent/child linkage is `rootTask`/`parentTask` object references
on the in-memory `Task` (`roo: src/core/task/Task.ts:148-149`) plus
`childIds: z.array(z.string()).optional()` on the persisted `HistoryItem`
(`roo: packages/types/src/history.ts:25`), a "boomerang task"/delegation model
(`delegateParentAndOpenChild`, `roo: src/core/webview/ClineProvider.ts:2780-2907`)
rather than Cline's deterministic-subagent-id model -- conceptually adjacent
(both are relational parent-child links on the task/session record, not
path-nesting) but the delegation semantics and id-minting are unrelated
enough that this is a genuine design difference, not a rename.

## What did not diverge

- **Transcript generation and file names.** Roo Code is squarely on the same
  generation Cline calls "Generation 1": `api_conversation_history.json`
  (model-facing) and `ui_messages.json` (display-facing) are both still
  actively read and written, not a fallback for old data
  (`roo: src/core/task-persistence/apiMessages.ts:109-121`,
  `roo: src/core/task-persistence/taskMessages.ts:52-56`, contrast with
  Cline's SDK generation where these same two file names are read-only
  fallback paths per the Cline dossier). Roo Code also still tolerates one
  file name older than either: `claude_messages.json`
  (`roo: src/core/task-persistence/apiMessages.ts:73-99`), a one-time
  read-and-delete fallback -- an extra rung on the same ladder, not a new
  generation.
- **Directory layout convention.** `{globalStoragePath}/tasks/{taskId}/` as
  the per-task root (`roo: src/utils/storage.ts:53-58`,
  `getTaskDirectoryPath`) is the same flat, non-nested layout Cline's
  generation 1 uses (`{extensionGlobalStoragePath}/tasks/{taskId}/` per the
  Cline dossier). This is inherited structure from before the fork, not an
  independent convergence -- say so plainly rather than counting it as
  evidence of anything.
- **No database.** Neither Cline generation 1 nor Roo Code has one. Checked:
  no `sqlite`/`better-sqlite3` dependency in Roo Code's `package.json`, and no
  source file constructs a SQL connection or table for session/task data (the
  only two source hits for the string "sqlite" are an unrelated exclude-glob
  list and an unrelated custom-instructions string, not session storage).
  This is not a divergence from Cline gen 1, but it is worth restating plainly
  because Cline's *current* generation (2) does have SQLite as its primary
  backend -- Roo Code simply never made that jump, consistent with the
  Summary above.
- **Unbounded growth / full-file rewrite cost.** Every write to
  `api_conversation_history.json` or `ui_messages.json` re-serializes the
  entire array (`roo: src/core/task-persistence/apiMessages.ts:120`,
  `roo: src/core/task-persistence/taskMessages.ts:55`), and reads parse the
  whole file (`roo: src/core/task-persistence/apiMessages.ts:50-53`). No
  pagination, cursor, or size cap was found -- the same shape of unbounded
  linear-cost growth the Cline dossier documents as a proven, user-visible
  failure mode (issue cline/cline#9011) for Cline's own gen-1 files. The
  atomic-write improvement (item 4 above) makes individual writes safer, not
  smaller or cheaper.
- **No explicit subagent/child nesting-depth cap.** Grepped
  `roo: src/core/task/Task.ts` and `roo: src/core/webview/ClineProvider.ts`
  for a maximum-depth constant/guard; none was found -- matching the Cline
  dossier's own "none found" conclusion for Cline. Absence confirmed in the
  same places searched in both trees, not merely assumed.

## What this adds to the corpus

Independent evidence, not a restatement -- but only for the mechanisms layered
around the shared generation-1 skeleton, not for the skeleton itself. Roo Code
answers "what does a long-lived fork of the *old* Cline format actually look
like once it grows its own infrastructure for years" -- a question Cline's own
repository cannot answer about itself, since Cline replaced that generation
with a structurally different one rather than hardening it in place. The
checkpoint mechanism in particular is useful corpus evidence in its own right:
it is the second real-git-repo-based design in this research (as opposed to
refs-in-the-real-repo), and it happens to be the one that actually matches
Cline's own (otherwise inaccurate-for-Cline) documentation. The
history-index layer and the non-destructive condense-by-tagging scheme are
both independently engineered solutions to problems Cline also has, built
without sharing code with Cline's gen-2 answers to the same problems -- useful
as a second data point on "index file plus per-item source of truth" and
"non-destructive compaction" as recurring shapes across unrelated
implementations, not as confirmation of Cline's specific design.

## Open questions

- Is `ShadowCheckpointService.deleteTask()`'s branch-based cleanup
  (`roo: src/services/checkpoints/ShadowCheckpointService.ts:450-469`,
  targeting `workspaceRepoDir()`) dead code left over from an earlier
  shared-shadow-repo-per-workspace design, or does some other code path still
  create checkpoints there? I did not find any call site that constructs a
  `workspaceRepoDir()`-rooted service for live checkpoint creation -- only
  `RepoPerTaskCheckpointService`, which uses a per-task directory instead --
  but did not trace git history to confirm this is vestigial rather than
  reachable through a path this pass missed.
  `roo: src/core/webview/ClineProvider.ts:1786` still calls it unconditionally
  on every task delete, so if it is dead it is at least harmless (best-effort,
  caught and logged).
- Does Roo Code's CLI surface (`apps/cli/src/lib/task-history`,
  `apps/cli/src/lib/storage`) share the same per-task file format and
  `TaskHistoryStore`, or does it have its own independent storage path? Not
  investigated in this pass -- flagged rather than assumed, since Cline's own
  CLI (`apps/cli`) does share its store with the VS Code extension via
  `@cline/core`, and it would be a real divergence if Roo Code's CLI does not.
  `apps/cli/src/lib/storage/history.ts` was located but not read.
  (roo, path only, not opened)
- Whether Roo Code's `git add . --ignore-errors` / full-worktree-commit
  checkpoint strategy has the same "cost scales with working-tree diff since
  last checkpoint" property the Cline dossier documents for `git stash
  create`, or whether committing the full worktree via a real branch/HEAD
  history (rather than dangling stash-created commits with no branch) grows
  the shadow repo's `.git` directory differently over a long task, was not
  measured or benchmarked in this pass.

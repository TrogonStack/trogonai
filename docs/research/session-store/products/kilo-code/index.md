# Kilo Code: what diverged from Cline's session store

Part of Session Store Research.
Fork delta report; see [backlog](../../backlog.md) Wave 5 for why this is a delta rather than a dossier.
Kilo Code pinned at `6ec20f23952b94517a106de366c23024a628e0b9` (MIT), compared against Roo Code at
`b867ec9145750d0ae1ff7f02d35406e9bf2a0b16` (Apache-2.0) and Cline at `5ec2d47b21b3a09aa7a094bfbbe0c7e8f7ddd3fa`
(Apache-2.0). Retrieved 2026-08-04. All three trees were read read-only at those pinned commits.
Upstream reference: [Cline](../cline/index.md), via [Roo Code](../roo-code/index.md).

## Summary of divergence

Kilo Code's session store diverged **completely** from Roo Code. The decision to replace it was Kilo's own,
but most of what replaced it was not authored by Kilo, so "Kilo's divergence" and "Kilo's design" are two
different claims throughout this report and are kept apart deliberately.
At this pinned commit, Kilo Code has replaced the entire Cline/Roo-lineage persistence layer
(per-task `api_conversation_history.json` + `ui_messages.json` flat files under VS Code global storage) with
a SQLite-backed session store (`SessionTable`/`MessageTable`/`PartTable`, Drizzle ORM,
`kilo: packages/core/src/session/sql.ts:22-99`) that is not authored by Kilo from scratch but **vendored
wholesale from OpenCode** (`sst`'s `@opencode-ai/core`, published under that exact package name inside Kilo's
own monorepo -- `kilo: packages/core/package.json:2`, confirmed by dense `// kilocode_change` patch markers
throughout that code and by merge commits like `28f2abe5e9 Merge remote-tracking branch 'origin/main' into
marius-kilocode/kilo-opencode-v1.17.9` in `kilo`'s own git history). Roo Code, by contrast, still runs the
classic Cline-style per-task JSON model nearly verbatim (`roo:
src/core/task-persistence/TaskHistoryStore.ts:23-25`, `roo: src/utils/storage.ts:53-58`). So for every concern
below, the correct three-way read is: Cline ≈ Roo (same storage medium and layout, though Roo did rewrite
cascade-delete semantics -- see the [Roo delta](../roo-code/index.md)) vs. Kilo (wholesale replacement sourced
from a fourth codebase, OpenCode, that is outside the Cline/Roo lineage entirely). The old Roo/Cline-style
format survives in Kilo only as a one-time import path for users upgrading from "legacy Kilo Code v5.x" (which
was itself an ordinary Roo Code fork -- confirmed by the legacy secret-storage key
`kilo: packages/kilo-vscode/src/legacy-migration/migration-service.ts:50`,
`const SECRET_KEY = "roo_cline_config_api_config"`).

## Divergence attribution

| Concern | Cline | Roo Code | Kilo Code | Attributed to |
| --- | --- | --- | --- | --- |
| Durable transcript | Two generations: legacy `api_conversation_history.json`+`ui_messages.json` (dead-write, read-only fallback), live SDK `{sessionId}.messages.json` (`cline: sdk/packages/core/src/services/session-data.ts:304-327`) | Classic dual files, still live: `api_conversation_history.json`+`ui_messages.json` per task dir | SQLite rows in `MessageTable`/`PartTable` (`kilo: packages/core/src/session/sql.ts:68-99`); classic files read only by the one-time legacy importer (`kilo: packages/kilo-vscode/src/legacy-migration/task-store.ts:6-7`) | **Kilo, original** -- total replacement, not present in Roo |
| Storage root / session id | `~/.cline/data/{sessions,tasks}` (VS Code global storage / SDK data dir), taskId = SDK sessionId | VS Code `globalStorage/tasks/{taskId}/` (`roo: src/utils/storage.ts:53-58`) | XDG data dir (`~/.local/share/kilo` etc.), `app = "kilo"` (`kilo: packages/core/src/global.ts:12,22-23`); session ids are `ses_` + descending id (`kilo: packages/core/src/session/schema.ts:12-20`), not VS Code taskIds | **Kilo, original** -- root relocated out of VS Code entirely, with an explicit migration path |
| SQLite session store | Yes, primary backend (`cline: sdk/packages/core/src/services/storage/sqlite-session-store.ts`) | No -- pure per-task JSON files, no DB | Yes, Drizzle/SQLite (`kilo: packages/core/src/database/database.ts:46,56-61`), db file literally named `kilo.db` | **Convergent, not shared** -- Kilo's SQLite code is unrelated to Cline's; Roo has none |
| Subagent/child model + cascade | Sibling sessions, relational `parentSessionId`, **one-level-only** cascade delete (`cline: sdk/packages/core/src/session/services/persistence-service.ts:557-609`) | Legacy `rootTaskId`/`parentTaskId` fields on `HistoryItem`, **plus** its own genuinely recursive cascade over `childIds` (`roo: src/core/webview/ClineProvider.ts:1747-1762`) | `parent_id` column + indexed query (`kilo: packages/core/src/session/sql.ts:31,64`); `remove()` recursively deletes all descendants, any depth (`kilo: packages/opencode/src/session/session.ts:670-704`); DB-level `onDelete: cascade` FK from `message`→`session` and `part`→`message` (`kilo: packages/core/src/session/sql.ts:75,89`) | **OpenCode, inherited**: the recursion carries no `kilocode_change` marker, so it arrives with the vendored core and is not evidence about Kilo's own design choices |
| Checkpoints | Private refs `refs/cline/checkpoints/{sessionId}/{runCount}` inside the user's own repo, via `git stash create` (`cline: sdk/packages/core/src/hooks/checkpoint-hooks.ts`) | Separate shadow git repo **per task**, `{shadowDir}/tasks/{taskId}/checkpoints` (`roo: src/services/checkpoints/RepoPerTaskCheckpointService.ts:10`), via `simple-git` with sanitized env (`roo: src/services/checkpoints/ShadowCheckpointService.ts:1-77`) | Shadow git dir keyed **per project+worktree hash** (not per task), `path.join(Global.Path.data, "snapshot", project.id, Hash.fast(worktree))` (`kilo: packages/opencode/src/snapshot/index.ts:116`), raw `git --git-dir/--work-tree` calls, plus Kilo-added 7-day retention/pruning, large-repo seeding/materialization, and cross-process locking (`kilo: packages/opencode/src/snapshot/index.ts:54-56,338-341,371-422`) | **Kilo, original** (base mechanism via OpenCode; retention/materialization/locking are Kilo's own patches) -- different from both Cline's and Roo's checkpoint designs |
| Compaction | Separate sidecar file, `{sessionId}.compaction.json`, full messages file left untouched (`cline: sdk/packages/core/src/session/models/session-compaction.ts:25-34`) | Own `condense` module (`roo: src/core/condense/`), not inspected in depth here | LLM-generated "anchored summary" persisted as a `type: "compaction"` message in the same `MessageTable` stream, referencing the prior compaction message rather than a separate file (`kilo: packages/core/src/session/compaction.ts:169,184-190`) | **Kilo, original** (via OpenCode) -- no sidecar file; summary lives in-line in the message table |
| Migrations | No live-format migration beyond a `// TODO` stub (`cline: apps/vscode/src/core/storage/state-migrations.ts:65-67`) | N/A (still the classic format) | ~30 Drizzle schema migrations evolving the session tables over time (`kilo: packages/core/src/database/migration/*.ts`, e.g. `20260312043431_session_message_cursor.ts`, `20260604172448_event_sourced_session_input.ts`) **plus** an explicit one-time importer for users arriving from "legacy Kilo Code v5.x" (a Roo Code fork) that reads the classic per-task files and Roo's own secret-storage key and writes sessions through the new SDK client (`kilo: packages/kilo-vscode/src/legacy-migration/migration-service.ts:1-6,50`, `kilo: packages/kilo-vscode/src/legacy-migration/task-store.ts`, `kilo: packages/kilo-vscode/src/legacy-migration/sessions/migrate.ts`) | **Kilo, original** -- substantial migration machinery with no Roo/Cline counterpart |

## What Kilo Code diverged on its own

None of the following exist in Roo Code's tree, so none is inherited Cline/Roo behavior. Where a
`// kilocode_change` marker is quoted, the code is also Kilo-authored rather than merely vendored; the next
section separates out what arrived pre-built with the OpenCode core.

- **Full backend replacement.** The `kilo-vscode` VS Code extension package
  (`kilo: packages/kilo-vscode/src`) contains no `core/task`, `core/task-persistence`, `core/checkpoints`, or
  `core/condense` directories at all -- the directories that exist in Roo at the equivalent paths
  (`roo: src/core/task-persistence`, `roo: src/core/checkpoints`, `roo: src/core/condense`) are simply absent.
  Instead, `kilo-vscode` is a thin client (`kilo: packages/kilo-vscode/src/KiloProvider.ts:9-16`, importing
  `@kilocode/sdk/v2/client` and `@opencode-ai/core/kilocode/cost/max-cost-nudge`) to a separate CLI/server
  process built on the vendored OpenCode core.
- **Storage root relocated out of VS Code.** Session data now lives under an XDG data directory
  (`kilo: packages/core/src/global.ts:12,22-23`, `app = "kilo"`) rather than the VS Code extension's global
  storage path Roo and classic Cline use. This is a real on-disk-layout change, not a cosmetic rename: it is
  precisely why the legacy-migration subsystem exists.
- **The SQLite DB filename itself was renamed with a compatibility read-path.** `kilo:
  packages/core/src/database/database.ts:56-61` computes `kilo-{channel}.db` as the target file but falls back
  to reading a pre-existing `opencode-{channel}.db` if the new name doesn't exist yet -- a concrete, small-scale
  instance of "renamed storage root, with migration for existing users," this time for users of upstream
  OpenCode rather than Roo/Cline.
- **Checkpoint retention and cross-process locking.** `kilo: packages/opencode/src/snapshot/index.ts:54-56`
  (`retention = 7 * 24 * 60 * 60 * 1000`, hourly `git gc --prune=7.days` loop at lines 888-893) and the
  cross-process `flock`-based locking around every snapshot mutation (`kilo:
  packages/opencode/src/snapshot/index.ts:215-219`, explicitly commented "serialize snapshot repositories
  across CLI and extension processes") -- neither Roo's nor Cline's checkpoint code has an equivalent retention
  policy or explicit multi-process lock; this is necessitated by Kilo's own CLI+extension split architecture.
- **In-line compaction summaries instead of a sidecar file.** `kilo: packages/core/src/session/compaction.ts`
  persists the LLM-generated summary as an ordinary message row (`type: "compaction"`) inside the same message
  stream the raw transcript lives in, rather than Cline's separate `.compaction.json` sidecar or Roo's
  `condense` module output.

## What arrived with the vendored core, not from Kilo

The bullets above are divergences *relative to Roo* -- they are absent from Roo's tree, so they are not
inherited Cline/Roo behavior. That is a weaker claim than Kilo having authored them, and the two must not be
conflated: most of this subsystem was written by OpenCode and merely vendored. The `// kilocode_change`
markers are the discriminator, and where they are absent the design decision is OpenCode's.

- **The recursive cascade delete is OpenCode's, not Kilo's.**
  `kilo: packages/opencode/src/session/session.ts:695-704` -- `remove()` fetches all direct children and calls
  itself on each, cascading through the full descendant tree at any depth. The recursion carries **no**
  `kilocode_change` marker (the nearest marker opens on the line after it, around `SandboxPolicy.dispose`), so
  it came in with the vendored core. Attributing it to Kilo would double-count OpenCode's evidence.
- **Roo already solved this independently, and better than its own upstream.** Roo recurses over `childIds`
  (`roo: src/core/webview/ClineProvider.ts:1747-1762`, `await collectChildIds(childId)` inside the child loop)
  where Cline stops at one level (`cline: sdk/packages/core/src/session/services/persistence-service.ts:566`,
  gated on `!row.isSubagent`). So the corpus's evidence that a one-level cascade gets corrected under real use
  comes from the **Roo** delta, not this one. Kilo's cascade is a third codebase's answer, arriving
  pre-built.

## What is inherited from Roo Code

None found. Kilo's persistence-layer code shares essentially no surface with Roo's
`task-persistence`/`checkpoints`/`condense` modules at this commit -- the entire subsystem was replaced rather
than modified. The only Roo-derived artifact still present is passive: Kilo's legacy importer reads Roo's
on-disk/secret-storage conventions (the `roo_cline_config_api_config` secret key, the
`api_conversation_history.json`/`ui_messages.json`/`history_item.json`/`_index.json` file set) purely as a
**source format to migrate away from** (`kilo: packages/kilo-vscode/src/legacy-migration/task-store.ts:6-7`,
`kilo: packages/kilo-vscode/src/legacy-migration/migration-service.ts:50`), not as a format Kilo's current
engine writes or reads going forward. This is Kilo's own historical footprint (a prior version of Kilo was a
Roo fork), not a live inherited behavior.

## What did not diverge

Nothing material was found. The one surface-level constant that survives unchanged is the **shape** of the
legacy fields Kilo's importer still knows how to parse -- `task`, `workspace`, `ts`, `mode`, `rootTaskId`,
`parentTaskId` (`kilo: packages/kilo-vscode/src/legacy-migration/task-store.ts:195-205`) -- which is a byte-for-
byte match to Roo/classic-Cline's `HistoryItem` shape. This is expected, not a finding: an importer must match
the format it imports. It says nothing about Kilo's live session store, which does not use this shape.

## What this adds to the corpus

Kilo Code is **independent evidence, but not about the Cline/Roo lineage** -- it is independent evidence about
a *different* upstream (OpenCode) that happens to have been grafted onto a Cline/Roo-descended product. For
the specific question this corpus wave asks ("what diverged from Cline's session store, with paths"), Kilo's
honest answer is: everything, because Kilo no longer runs a Cline/Roo-descended session store at all -- it runs
OpenCode's, with Kilo-specific patches (retention, locking, large-repo snapshot seeding, recursive cascade
delete already present in the base, XDG relocation, and the legacy-format importer). The corpus **already has**
an [OpenCode dossier](../opencode/index.md) (pinned at `62e4641235d7847dadc60da37cca8a023dd54fc1`), so this
cross-check is available now rather than hypothetical: Kilo's engine should be read against that dossier, not
against Cline, and the `// kilocode_change` markers are the mechanical discriminator between OpenCode's design
decisions and Kilo's own patches. Note the two are pinned at different upstream generations (Kilo's vendored
core tracks `v1.17.9`), so a divergence found by that comparison may be version skew rather than a Kilo patch;
the marker, not the diff, is what settles authorship. As a **fork-of-Roo** data point specifically, Kilo adds exactly
one useful fact: it demonstrates that a fork can abandon the entire inherited persistence architecture rather
than evolve it, which the "inherited vs. original" framing this wave is built around should be able to
represent.

## Open questions

- **Resolved during verification, recorded here because the original framing was wrong:** the `SessionV2`
  module under `kilo: packages/core/src/session/` is neither a parallel engine nor an in-progress rewrite. It is
  a *dependency of* the engine this report treats as authoritative -- `packages/opencode/src/session/session.ts`
  imports `SessionV2` directly, as do `packages/opencode/src/session/schema.ts`, the control-plane HTTP
  handlers (`packages/opencode/src/server/routes/instance/httpapi/handlers/control-plane.ts`),
  `packages/server/src/groups/message.ts`, and four test files under `packages/opencode/test/`. So both modules
  are live and there is no competition between them; `packages/core/src/session/sql.ts` is the schema the
  `packages/opencode` engine runs against, which is why this report cites both trees for one store.
- Whether compaction in Kilo's engine ever deletes or archives the pre-compaction raw message rows, or only
  ever appends a new summary message alongside them indefinitely -- I read `compaction.ts`'s summary-generation
  and anchoring logic but did not trace a corresponding deletion/archival path, so I cannot confirm whether
  Kilo's transcript is fully retained forever or eventually pruned post-compaction.
- Full provenance of `packages/core` as a fork of `sst/opencode` was inferred from the package name
  (`@opencode-ai/core`), the density of `// kilocode_change` comments, and one merge-commit message
  referencing `kilo-opencode-v1.17.9` in Kilo's git log -- I did not diff against an actual upstream OpenCode
  checkout (none was provided for this task) to confirm how much of the unmarked code is verbatim OpenCode
  versus independently rewritten by Kilo under the same file layout.
- Roo Code's own checkpoint/subtask code was read only far enough to attribute Kilo's divergence correctly
  (confirming Roo still has `ShadowCheckpointService`/`RepoPerTaskCheckpointService` and no SQLite/recursive-
  cascade equivalent); a full characterization of how Roo's checkpoint or subtask model itself diverges from
  Cline is explicitly out of scope here and is being produced separately (see backlog Wave 5, Roo Code row).

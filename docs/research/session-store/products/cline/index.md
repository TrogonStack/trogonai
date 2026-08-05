# Cline: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against these authoritative anchors:

- Repo [cline/cline](https://github.com/cline/cline), pinned commit `5ec2d47b21b3a09aa7a094bfbbe0c7e8f7ddd3fa`
  (committed 2026-08-03T20:01:37-07:00, `refactor(ui): extract agent prompt
  queue (#12791)`). Every `path:line` citation below was read against this
  exact commit.
- The repo is a monorepo, not the single-package extension repo that older
  third-party summaries (DeepWiki-style write-ups, blog posts) describe.
  Relevant subtrees: `apps/vscode` (the VS Code extension host), `apps/cli`,
  `apps/cline-hub`, `sdk/packages/core` (published as `@cline/core`, the new
  session/runtime engine), `sdk/packages/shared` (published as `@cline/shared`,
  shared types/paths/db helpers), `sdk/packages/llms` (published as
  `@cline/llms`, re-exports message types from `@cline/shared`).
- In-repo official user docs `docs/core-workflows/checkpoints.mdx` (same
  commit) -- cited below specifically because it is **contradicted** by the
  source it describes.
- GitHub issue [cline/cline#9011](https://github.com/cline/cline/issues/9011)
  (opened 2026-02-01, closed 2026-07-03) -- a secondary source, cited only to
  corroborate a growth-risk finding that is independently established from
  source below. Labeled as secondary throughout.

A note on method for this dossier specifically: pre-task hypotheses supplied
by the task brief (that `api_conversation_history.json`, `ui_messages.json`,
and `task_metadata.json` are the *current* dual-file format, and that
checkpoints are a "shadow git repository") both turn out to be **stale**
relative to this pinned commit. Cline has, since those third-party summaries
were written, grown a second, parallel persistence generation inside
`sdk/packages/core` that supersedes the classic VS Code extension's on-disk
format for anything created going forward. Both generations are documented
below; the dossier is explicit about which one is live.

## The storage model

Two persistence generations coexist on disk at this commit, serving different
purposes:

**Generation 1 -- the classic per-task flat files.** Written by the pre-SDK
VS Code extension, under the legacy VS Code global-storage root
(`{extensionGlobalStoragePath}/tasks/{taskId}/`, historically; the SDK-era code
also mirrors the same layout under `~/.cline/data/tasks/{taskId}/` -- see
Keying and identity). `apps/vscode/src/core/storage/disk.ts:1-40` defines
`GlobalFileNames`, an object naming the files: `apiConversationHistory:
"api_conversation_history.json"`, `uiMessages: "ui_messages.json"`,
`contextHistory: "context_history.json"`, `taskMetadata:
"task_metadata.json"`, plus `openRouterModels.json`, `openRouterGenerations`,
`mcpSettings`, `clineRules`. Each is a **plain JSON array/object, rewritten in
full on every save** -- there is no append-only log at this generation. For
example `saveTaskMetadata` (`apps/vscode/src/core/storage/disk.ts:238-251`,
approx.) does `fs.writeFile(filePath, JSON.stringify(metadata))` -- a full-file
overwrite, not an append.

At this pinned commit, **only `task_metadata.json` is still actively written**
by live code (`apps/vscode/src/core/context/context-tracking/
FileContextTracker.ts`, see Retention below). `api_conversation_history.json`
and `ui_messages.json` are read-only: they are consumed for one-time migration
and cross-version fallback, not written by any code path found in this
checkout (see "Interop" and "Read and resume path" -- `legacy-state-reader.ts`
and `sdk-session-history-loader.ts`). The task brief's premise that these are
today's live dual-write pair does not hold; that was true of an earlier
single-package version of Cline, before the `sdk/packages/core` engine
existed.

**Generation 2 -- the SDK session store (`@cline/core`).** This is the live
system for anything created today, used by `apps/vscode`, `apps/cli`, and
`apps/cline-hub` alike. A session ("task" in VS Code UI copy) is:

- A **row** in either a SQLite table (`sessions`, see The store interface) or
  a flat JSON index file (`sessions.index.json`) -- whichever backend adapter
  is active. This row is the fast-lookup/listing projection.
- A **manifest file**, `{sessionsDir}/{sessionId}/{sessionId}.json`, a single
  JSON object matching `SessionManifestSchema`
  (`sdk/packages/core/src/session/models/session-manifest.ts:6-28`), containing
  denormalized session config/status/metadata (including, per session, an
  embedded checkpoint history -- see Rewind/checkpoints).
- A **messages file**, `{sessionsDir}/{sessionId}/{sessionId}.messages.json`
  (path computed by `SessionArtifacts.sessionMessagesPath`,
  `sdk/packages/core/src/services/session-artifacts.ts:75-80`), a single JSON
  object `{version: 1, updated_at, agent, sessionId, taskType?, messages:
  StoredMessageWithMetadata[], system_prompt?}`
  (`buildMessagesFilePayload`, `sdk/packages/core/src/services/
  session-data.ts:304-327`) -- **the actual model-facing conversation
  transcript**. This is the closest thing to a "the durable session" answer:
  it is the only artifact that carries the full turn-by-turn content.
- An optional **compaction sidecar**,
  `{sessionsDir}/{sessionId}/{sessionId}.compaction.json`
  (`SessionArtifacts.sessionCompactionPath`, `session-artifacts.ts:82-87`),
  validated by `SessionCompactionStateSchema`
  (`sdk/packages/core/src/session/models/session-compaction.ts:25-34`).

None of these four artifacts is an append-only log. The messages file, the
manifest file, and the compaction sidecar are each **read in full and
rewritten in full** on every persist
(`SessionManifestStore.persistSessionMessages`,
`sdk/packages/core/src/session/stores/session-manifest-store.ts:157-176`;
`writeSessionManifest`, same file, lines 71-78). The SQLite/file index row is
mutated in place with `UPDATE`/`INSERT OR REPLACE`
(`sdk/packages/core/src/session/services/session-service.ts:33-72`,
`sdk/packages/core/src/session/services/file-session-service.ts:114-118`).
This is a **mutable-document model, not an event log**: there is no
sequence-numbered append stream anywhere in this generation either. (This
directly matters for the platform's event-sourced Session Store design -- see
"What this implies".)

Which is authoritative and which is derived, concretely:

- **Authoritative**: the messages file (model-facing transcript content), the
  manifest file (session config/status/metadata as of last write), and the
  SQLite/file-index row (status, lineage, path pointers) are each the sole
  source for their own slice of state -- none of the three is rebuildable from
  either of the other two if lost, because each holds data the others do not
  (e.g., only the row holds `statusLock`/PID; only the manifest holds
  `checkpoint.history`; only the messages file holds message content).
- **Derived at read time, never persisted**: the UI-facing `ClineMessage[]`
  transcript shown in the VS Code webview. It is computed from the persisted
  model-facing messages by `sdkMessagesToClineMessages()`
  (`apps/vscode/src/sdk/message-translator.ts:2133-2317`) on every
  history/resume load -- see Entry/message structure. This is the answer to
  the task brief's "dual representation" question: in the current
  architecture there is only **one** persisted transcript type; the second,
  UI-shaped type is a pure projection, not a second file.
- **Derived, self-healing, and explicitly documented as a fallback**: when
  listing sessions, a directory scan of manifest files
  (`listManifestHistoryRows`, `sdk/packages/core/src/runtime/host/
  history.ts:...`, see Listing) backstops the SQLite/file index if the index
  under-returns rows.

Best-fit conceptual model per the RESEARCH_PROMPT taxonomy: **session-as-row
plus session-as-document**. The row (SQLite or JSON index entry) is the
addressable, queryable, lock-carrying record; the manifest and messages files
are full-document sidecars keyed by the same id. There is no
session-as-append-only-log anywhere in this codebase at this commit -- every
persistence layer that could have been an event log (messages file,
manifest, compaction sidecar, index row) is instead read-modify-write-whole.

## Keying and identity

- **VS Code's "taskId" IS the SDK's "sessionId"** -- the same string is used as
  both identifiers. Confirmed by `createHistoryItemFromSession(sessionId,
  ...): HistoryItem { id: sessionId, ... }`
  (`apps/vscode/src/sdk/cline-session-factory.ts`, function
  `createHistoryItemFromSession`).
- **Root session ids** are minted as `${Date.now()}_${nanoid(5)}` when the
  caller does not supply one:
  `createRootSessionWithArtifacts()`
  (`sdk/packages/core/src/session/services/persistence-service.ts:104-110`):
  ```ts
  const providedId = input.sessionId.trim();
  const sessionId =
      providedId.length > 0 ? providedId : `${Date.now()}_${nanoid(5)}`;
  ```
  This scheme is time-prefixed (millisecond epoch), so directory/session names
  sort chronologically by string comparison. `sdk/packages/core/src/runtime/
  host/history.ts` corroborates this independently: session listing extracts a
  13+-digit recency token from session ids via a `/\d{13,}/g` regex
  (`extractSessionRecencyToken`) purely to sort listings, implying the authors
  rely on the epoch-prefix convention rather than trusting directory mtimes.
- **Subagent (child) session ids are deterministic, not random.**
  `makeSubSessionId(rootSessionId, agentId)`
  (`sdk/packages/core/src/session/models/session-graph.ts:9-17`):
  ```ts
  export function makeSubSessionId(rootSessionId: string, agentId: string): string {
      const root = sanitizeSessionToken(rootSessionId);
      const agent = sanitizeSessionToken(agentId);
      const joined = `${root}__${agent}`;
      return joined.length > 180 ? joined.slice(0, 180) : joined;
  }
  ```
  `sanitizeSessionToken` (same file, lines 5-7) replaces any character outside
  `[a-zA-Z0-9._-]` with `_`. Because the id is a pure function of
  `(rootSessionId, agentId)`, re-spawning the same named subagent under the
  same root **reuses the same child session id and row**
  (`TeamChildSessionManager.upsertSubagentSession`,
  `sdk/packages/core/src/session/team/team-child-session-manager.ts:132-160`,
  checks `existing = await this.adapter.getSession(sessionId)` before
  deciding insert vs. update). Team-task sub-sessions get a related but
  non-deterministic id: `makeTeamTaskSubSessionId` appends a `nanoid(6)`
  (`session-graph.ts:19-26`): `` `${root}__teamtask__${agent}__${nanoid(6)}` ``.
  Both id shapes are parsed back apart by `parseSubSessionId` /
  `parseTeamTaskSubSessionId` (`session-graph.ts:28-67`) wherever a
  sub-session's artifact directory needs to be resolved back to its root
  (`childArtifactFileStem`, `sdk/packages/core/src/services/
  session-artifacts.ts:34-58`).
- **Lineage is relational, not path-nested.** A child session's link to its
  parent is a set of plain columns/fields on its own `SessionRow` --
  `parentSessionId`, `parentAgentId`, `agentId`, `conversationId`,
  `isSubagent`
  (`sdk/packages/core/src/session/models/session-row.ts:6-34`) -- not a
  parent-relative directory path. All child sessions for a given generation
  still live in a **flat** `{sessionsDir}/{sessionId}/` directory next to
  their root (subagent artifact paths are computed relative to the *root's*
  directory: `childArtifactFileStem` resolves to `{rootSessionId}` and a
  `fileStem`, then `subagentArtifactPaths` joins that file stem inside
  `this.sessionArtifactsDir(rootSessionId)`,
  `session-artifacts.ts:132-144`) -- so a subagent's messages file physically
  lives inside its root session's directory as a sibling file
  (`{rootSessionId}/{agentId}.messages.json`), while the subagent's own
  manifest/row still exists as an independent session entity addressable by
  its own deterministic id.
- **Listing is per-machine/global, not scoped to a project by construction.**
  `resolveSessionDataDir()` (`sdk/packages/shared/src/storage/paths.ts`)
  resolves to a single flat directory, `~/.cline/data/sessions/`, shared
  across all cwds/workspaces; `cwd`/`workspaceRoot` are just columns on each
  row (`sdk/packages/core/src/session/models/session-row.ts:18-19`), not part
  of the storage path. Any project-scoping in the VS Code UI (e.g., only
  showing tasks for the current workspace) is a query-time filter over this
  flat row set, not a storage-layout property -- I did not read the specific
  VS Code UI filter call site to confirm whether such a filter exists at
  this commit; flagged under Open questions.
- **No relocation/rename reconciliation was found.** Because `cwd` is stored
  as a plain string column and no other product's session store references
  or mirrors that path, a moved-workspace scenario is not treated specially
  anywhere in the persistence code read for this dossier (contrast with
  other corpus entries, e.g. Grok Build's `RelocationJournal` -- Cline appears
  to have no equivalent).
- **Path resolution is explicitly unified** across the classic and SDK code:
  both `resolveDataDirFromEnv()`
  (`apps/vscode/src/shared/storage/storage-context.ts`, resolves
  `CLINE_DATA_DIR` → `${CLINE_DIR}/data` → `~/.cline/data`) and the SDK's
  `resolveClineDataDir()` (`sdk/packages/shared/src/storage/paths.ts`) share
  the same default and env-var precedence, with an explicit code comment
  referencing an internal fixed bug ticket ("ENG-2332") about a prior
  divergence between the two resolvers -- i.e., the two persistence
  generations deliberately live under the same root directory today.

## The store interface

Cline's SDK session layer **does** expose a pluggable, first-class adapter
interface. Reproduced verbatim from
`sdk/packages/core/src/types/session.ts:105-128`:

```ts
export interface SessionPersistenceAdapter {
	ensureSessionsDir(): string;
	upsertSession(row: SessionRow): Promise<void>;
	getSession(sessionId: string): Promise<SessionRow | undefined>;
	listSessions(options: {
		limit: number;
		parentSessionId?: string;
		status?: string;
	}): Promise<SessionRow[]>;
	updateSession(
		input: PersistedSessionUpdateInput,
	): Promise<{ updated: boolean; statusLock: number }>;
	deleteSession(sessionId: string, cascade: boolean): Promise<boolean>;
	enqueueSpawnRequest(input: {
		rootSessionId: string;
		parentAgentId: string;
		task?: string;
		systemPrompt?: string;
	}): Promise<void>;
	claimSpawnRequest(
		rootSessionId: string,
		parentAgentId: string,
	): Promise<string | undefined>;
}
```

All eight methods are required (no optional methods on this interface). The
companion input type, also verbatim (`sdk/packages/core/src/
types/session.ts:89-103`):

```ts
export interface PersistedSessionUpdateInput {
	sessionId: string;
	expectedStatusLock?: number;
	status?: SessionStatus;
	endedAt?: string | null;
	exitCode?: number | null;
	prompt?: string | null;
	metadata?: Record<string, unknown> | null;
	title?: string | null;
	parentSessionId?: string | null;
	parentAgentId?: string | null;
	agentId?: string | null;
	conversationId?: string | null;
	setRunning?: boolean;
}
```

Two concrete adapters implement this interface at this commit:

1. `LocalSessionPersistenceAdapter` (SQLite-backed), defined inline in
   `sdk/packages/core/src/session/services/session-service.ts:20-261`, wraps a
   `SqliteSessionStore` (`sdk/packages/core/src/services/storage/
   sqlite-session-store.ts`) and issues raw SQL (`INSERT OR REPLACE INTO
   sessions (...)`, `session-service.ts:35-40`) against a `sessions` table.
2. `FileSessionPersistenceAdapter` (flat-file-backed), defined inline in
   `sdk/packages/core/src/session/services/file-session-service.ts:50-268`,
   backed by a single JSON index file `sessions.index.json`
   (`{version: 1, sessions: Record<string, SessionRow>}`) plus a
   `subagent-spawn-queue.json` file for the spawn-request queue methods.

**Which one is actually used, resolved definitively:**
`createLocalBackend()` (`sdk/packages/core/src/runtime/host/host.ts:64-97`)
tries SQLite first and only falls back to the file adapter if SQLite
initialization throws:

```ts
function createLocalBackend(options: ClineCoreOptions): SessionBackend {
	try {
		const store = new SqliteSessionStore();
		store.init();
		return new CoreSessionService(store, { ... });
	} catch (error) {
		// Fallback to file-based session service if SQLite is unavailable.
		options.telemetry?.capture({
			event: "session_backend_fallback",
			properties: { requestedBackend: "sqlite", fallbackBackend: "file" },
		});
		...
		return new FileSessionService(undefined, { ... });
	}
}
```
(`host.ts:64-97`, comment and telemetry event name as in source). So: SQLite
is the primary backend; the JSON-index adapter is a resilience fallback for
environments where the native SQLite binding cannot load, not a
user-selectable alternative. VS Code specifically forces
`backendMode: "local"` (`apps/vscode/src/sdk/vscode-session-host.ts:126`),
which routes through `createLocalRuntimeHost` → `createLocalBackend` above --
i.e., VS Code never uses the "hub" or "remote" runtime-host modes that
`createRuntimeHost` (`host.ts:137-249`) also supports for other Cline
surfaces (CLI/hub daemon, enterprise remote).

The SQLite schema (`sessions` table), reconstructed from the two adapters'
raw SQL (`session-service.ts:35-40`, `sqlite-session-store.ts:86-91`):
`session_id, source, pid, started_at, ended_at, exit_code, status,
status_lock, interactive, provider, model, cwd, workspace_root, team_name,
enable_tools, enable_spawn, enable_teams, parent_session_id,
parent_agent_id, agent_id, conversation_id, is_subagent, prompt,
metadata_json, transcript_path, hook_path, messages_path, updated_at` (28
columns). The DB file itself: `sessionDbPath()` returns
`join(resolveDbDataDir(), "sessions.db")`
(`sdk/packages/core/src/services/storage/sqlite-session-store.ts:44-46`),
i.e. `~/.cline/data/db/sessions.db` by default. I did not read
`@cline/shared/db`'s `loadSqliteDb`/`ensureSessionSchema` implementation (a
different package than the ones the task pointed at), so the SQLite journal
mode (WAL vs. rollback journal, `busy_timeout`) is not confirmed -- flagged
under Open questions, and directly relevant to whether two concurrent VS
Code windows can safely write to the same DB file.

`UnifiedSessionPersistenceService`
(`sdk/packages/core/src/session/services/persistence-service.ts:42-618`) is
the class that actually orchestrates an adapter plus a
`SessionManifestStore` plus a `TeamChildSessionManager`; `CoreSessionService`
and `FileSessionService` both extend it, injecting their respective adapter.
This is the effective "session store" callers (`apps/vscode/src/sdk/*`)
interact with; its public surface includes `createRootSessionWithArtifacts`,
`updateSessionStatus`, `updateSession`, `persistSessionMessages`,
`readSessionCompactionState`/`persistSessionCompactionState`,
`listSessions`, `reconcileDeadSessions`, `deleteSession`, and the
`TeamChildSessionManager`-delegated subagent methods
(`upsertSubagentSession`, `applyStatusToRunningChildSessions`, etc.) -- see
Write/append path and Subagents below for their individual contracts.

## Write and append path (ordering, durability, concurrency, delivery)

**Commit shape: full rewrite, not append, for message content.**
`SessionManifestStore.persistSessionMessages()`
(`sdk/packages/core/src/session/stores/session-manifest-store.ts:157-176`)
serializes the *entire* messages array on every call:
```ts
const payload = buildMessagesFilePayload({ updatedAt: nowIso(), context, messages, systemPrompt });
const contents = `${JSON.stringify(payload, null, 2)}\n`;
mkdirSync(dirname(path), { recursive: true });
writeFileSync(path, contents, "utf8");
```
There is no positional/line-append anywhere in this path -- the whole
conversation is re-serialized and rewritten to the same path on each turn.

**Ordering** is array order within that JSON file, established purely by the
order messages are pushed into the in-memory array before the call; there is
no independent sequence number stamped on each entry by the store itself
(individual `MessageWithMetadata` entries carry an optional `ts` field, but
it is caller-supplied -- see Entry/message structure).

**Durability/atomicity is inconsistent across the three artifacts written
per session, and this is a genuine finding, not paraphrase:**
- The messages file and the manifest file are written with a **plain
  `writeFileSync`**, no temp-file-and-rename, no fsync:
  `SessionManifestStore.persistSessionMessages` (as above,
  `session-manifest-store.ts:174-175`) and
  `SessionManifestStore.writeSessionManifest`
  (`session-manifest-store.ts:71-78`, `writeFileSync(manifestPath,
  JSON.stringify(...))`). A process kill mid-write can leave either file
  truncated/corrupt on a POSIX filesystem that does not guarantee atomic
  `write()` for the buffer size involved.
- The compaction sidecar, by contrast, **is** written atomically:
  `persistSessionCompactionState` calls `writeFileAtomic`
  (`session-manifest-store.ts:243-251`), defined in
  `sdk/packages/core/src/session/stores/atomic-file.ts:23-53` -- open a
  `{path}.{pid}.{uuid}.tmp` file with the `wx` flag, `writeFile`, `sync()`
  (fsync), close, `rename()` to the final path, then a best-effort fsync of
  the parent directory (`fsyncBestEffort`, `atomic-file.ts:5-21`).
- The file-index adapter's own bookkeeping (`sessions.index.json`,
  `subagent-spawn-queue.json`) also uses a temp+rename pattern:
  `atomicWriteJson()` (`file-session-service.ts:44-48`, `writeFileSync` to
  `${path}.tmp` then `renameSync`) -- simpler than `atomic-file.ts` (no fsync
  call before rename), but still crash-safer than a bare `writeFileSync`.
- So: **the SQLite row (or file index) and the compaction sidecar have some
  torn-write protection; the manifest and -- critically -- the messages file,
  which is where actual conversation content lives, do not.**

**Three-way write with no cross-artifact transaction on session creation.**
`createRootSessionWithArtifacts()`
(`sdk/packages/core/src/session/services/persistence-service.ts:104-179`)
performs, in this order: (1) `adapter.upsertSession(...)` (row insert), (2)
`manifestStore.initializeMessagesFile(...)` (plain `writeFileSync` of an
empty messages payload), (3) `manifestStore.writeSessionManifest(...)`
(plain `writeFileSync` of the manifest). No code read in this dossier wraps
these three writes in a transaction, a write-ahead marker, or a rollback
path. If the process is killed between steps 1 and 2, the row exists with a
`messagesPath` pointing at a file that was never created; between 2 and 3,
the messages file exists but the manifest (which `readManifestFile` and
`listManifestHistoryRows` depend on for fallback listing) does not.
Reconciliation of this specific interleaving is not implemented anywhere I
found -- `reconcileDeadRunningSession` (below) only reconciles *status*
(dead PID → `failed`), not missing artifact files. This is left as an Open
question rather than asserted as a bug, since I did not find a reproduction
or issue confirming it happens in practice.

**Concurrency model: optimistic concurrency control on the row, single
apparent writer per session in normal operation.** `SessionRow.statusLock`
(`sdk/packages/core/src/session/models/session-row.ts:14`) is an integer
version stamp. Both adapters implement compare-and-swap semantics keyed on
it: the SQLite adapter's `updateSession` issues `UPDATE ... WHERE session_id
= ? AND status_lock = ?` (`session-service.ts:194-208`), returning
`updated: false` if the row's lock no longer matches; the file adapter does
the equivalent in-memory compare
(`file-session-service.ts:150-155`). Callers retry through
`withOccRetry(load, update, maxRetries)`
(`sdk/packages/core/src/services/session-data.ts:383-409`), with
`OCC_MAX_RETRIES = 4`
(`sdk/packages/core/src/session/services/persistence-service.ts:40`) used by
`updateSessionStatus` (`persistence-service.ts:181-215`) and `updateSession`
(`persistence-service.ts:217-285`, hand-rolled retry loop, same constant). No
evidence of multi-writer coordination beyond this per-row CAS was found; the
messages/manifest files themselves have no locking at all (see above), so
two concurrent writers to the *same* session (e.g. two VS Code windows
somehow sharing one taskId) could race on those files with no
compare-and-swap protection -- this is inferred from the absence of any
lock/version check in `persistSessionMessages`/`writeSessionManifest`, not
from a documented guarantee.

**Delivery semantics**: best-effort, at-most-once from the store's
perspective -- there is no idempotence key on message-file writes (a
crashed write is simply lost, not retried or deduplicated by id on next
attempt). Row-level writes get "effectively exactly-once under a stable PID"
via the OCC retry loop, since a failed CAS is retried against the freshly
re-read row rather than blindly reapplied.

## Read and resume path

**Resume prefers the live/current-generation transcript, then falls back
to legacy files.** `SdkSessionHistoryLoader.loadInitialMessages()`
(`apps/vscode/src/sdk/sdk-session-history-loader.ts`, full file, 46 lines):
tries `sessionHost.readLiveMessages?.(taskId) ?? sessionHost.readMessages(taskId)`
first; only if that returns empty does it fall back to
`getSavedApiConversationHistory(taskId)` (the classic
`api_conversation_history.json` reader from
`apps/vscode/src/core/storage/disk.ts`). This is the concrete evidence that
classic per-task files are a **fallback source for tasks created before the
SDK migration**, not a currently-maintained parallel store.

**No pagination, no cursor, no bound on transcript size at read time.**
`readPersistedMessagesFile(messagesPath)`
(`sdk/packages/core/src/runtime/host/runtime-host-support.ts:53-75`) reads
the *entire* messages file into memory with a single `readFile` + full
`JSON.parse`:
```ts
const raw = (await readFile(path, "utf8")).trim();
const parsed = JSON.parse(raw) as unknown;
if (Array.isArray(parsed)) return parsed as LlmsProviders.Message[];
if (parsed && typeof parsed === "object") {
	const messages = (parsed as { messages?: unknown }).messages;
	if (Array.isArray(messages)) return messages as LlmsProviders.Message[];
}
```
(defensively handles both a bare array and the `{messages: [...]}` envelope
shape, but always whole-file). There is no offset/limit parameter anywhere
in this function or its callers. Combined with the full-file-rewrite write
path above, this means both the read and write cost of a session scale
linearly with total transcript size, with no architectural cap -- directly
relevant to the Retention section below.

**Resumed sessions reconstruct the UI transcript by deterministic
replay/translation of the persisted model messages, not by reading a second
persisted UI-transcript file.** `sdkMessagesToClineMessages()`
(`apps/vscode/src/sdk/message-translator.ts:2133-2317`) walks the persisted
`SdkMessageWithMetrics[]` array and re-derives a `ClineMessage[]`: text
blocks become `say: "text"`/`say: "user_feedback"` rows, `tool_use`/
`tool_result` pairs are matched up and re-emitted via
`finalizePersistedToolUse`, and the transcript's **final** turn is
conditionally retagged into a synthetic completion row (`endFinalTurn()`,
lines 2175-2180, 2297-2299) -- gated on `finalTurnCompleted` (the caller-
supplied "did the last run end cleanly" flag, sourced from the session
row's terminal status) specifically so that a cancelled/failed/crashed
final turn is not mis-rendered as a successful completion. The function's
own comments document that this reconstruction is **lossy by design**:
"persisted transcripts carry no per-turn outcome... an earlier turn that
the user cancelled mid-response and then followed up on is indistinguishable
from one that ended cleanly -- retagging it would present an interrupted
response as a deliberate turn end" (comment above `endFinalTurn`,
`message-translator.ts:2167-2174`). Message `ts` values and ids are also
**re-minted** during this replay via a shared `MessageTranslatorState`/
`MessageIdMinter` rather than preserved verbatim from the original stream
(`state.nextTs()` calls throughout, e.g. lines 2252, 2269, 2308).

This closes the task brief's central "dual representation" question
directly: the classic architecture (pre-SDK) genuinely wrote two
independent files -- `api_conversation_history.json` (model-facing) and
`ui_messages.json` (display-facing) -- populated by two independent append
calls from the same in-process `Task` object as it streamed (that object no
longer exists in this checkout: `apps/vscode/src/sdk/task-proxy.ts:1-5`
explicitly states its `MessageStateHandler` "Mirrors the classic
`MessageStateHandlerEvents` from `src/core/task/message-state.ts`" -- past
tense, `origin/main` reference, i.e. describing code that has been removed
from this monorepo). In the **current** architecture there is exactly **one**
persisted transcript (`StoredMessageWithMetadata[]` in the messages file);
the UI-shaped `ClineMessage[]` is computed fresh on both the live-streaming
path (`message-translator.ts`'s event-driven functions, e.g.
`agentEventToMessages`) and the resume/history path
(`sdkMessagesToClineMessages`), sharing the same tool-name/shape mapping
logic by design (explicit comment: "Keep this in the live message
translator so history rendering and streaming rendering share the same SDK
tool → Cline UI mapping", `message-translator.ts:2130-2132`). The
model-facing array is therefore authoritative and the UI array is fully
derivable from it -- with the explicitly-documented caveat that the
derivation loses per-turn outcome fidelity for turns other than the final
one.

**What is materialized eagerly vs. lazily on listing/resume**: session row
lookup is eager (index/SQLite read); the manifest's title field is read
lazily and cheaply (`readSessionManifestTitle`,
`session-manifest-store.ts:95-123`, explicitly reads only the file's
`metadata.title` key off the event loop rather than the full
Zod-validated manifest, "the session-listing hot path needs nothing from
the manifest except the title... skip the full `SessionManifestSchema`
validation" per its own doc comment); full message content, cost/token
aggregates, and provider/model inference are all **lazy**, computed only
when a specific session's history is actually opened
(`hydrateSessionHistory()`, `sdk/packages/core/src/runtime/host/
history.ts`, infers missing title/provider/model/cost by reading the full
messages file on demand; `inferTitleFromMessages()` truncates to 50 chars).

## Listing, summaries, and search

Listing (`UnifiedSessionPersistenceService.listSessions(limit = 200)`,
`persistence-service.ts:510-535`) is a **query against the adapter**
(SQL `SELECT ... ORDER BY started_at DESC LIMIT ?` for SQLite,
`session-service.ts:99-109`, or an in-memory filter+sort+slice over the
whole `sessions.index.json` for the file adapter,
`file-session-service.ts:124-140`), **not** a directory scan, in the normal
path. Two self-healing/fallback behaviors sit around this:

- Every call to `listSessions()` first runs `reconcileDeadSessions(scanLimit)`
  (`persistence-service.ts:513`, `scanLimit = min(limit*5, 2000)`), which
  queries all `idle`/`running`/`pending` rows and, for each, checks whether
  its owning process is still alive via `process.kill(pid, 0)`
  (`isPidAlive`, `persistence-service.ts:424-437`) -- a session whose PID is
  dead is transitioned to `status: "failed"` with metadata
  `{terminal_marker: "failed_external_process_exit", terminal_marker_at,
  terminal_marker_pid, terminal_marker_source: "stale_session_reconciler"}`
  (`reconcileDeadRunningSession`, `persistence-service.ts:439-508`), and its
  manifest is rewritten to match (`buildManifestFromRow`,
  `sdk/packages/core/src/services/session-data.ts:350-381`), and a JSONL
  audit line is appended to a hooks log
  (`appendStaleSessionHookLog`, `session-manifest-store.ts:258-279`, writes
  to `${CLINE_HOOKS_LOG_PATH}` or `{hookLogDir}/hooks.jsonl`). So listing
  doubles as the crash-detection mechanism -- there is no separate daemon or
  watcher; staleness is discovered lazily, the next time anyone lists.
- A separate, directory-scan-based listing path,
  `listManifestHistoryRows(limit)`
  (`sdk/packages/core/src/runtime/host/history.ts`), enumerates
  `{sessionsDir}/{sessionId}/{sessionId}.json` manifest files directly off
  disk, Zod-validates each with `SessionManifestSchema.safeParse()`, and
  silently drops any that fail validation. Per-session recency is derived by
  regex-extracting a 13+-digit token from the session id
  (`extractSessionRecencyToken`, `/\d{13,}/g`), not from file mtimes. This
  path exists specifically as a fallback/cross-check for the adapter-backed
  listing (`listSessionHistoryFromBackend`, same file, calls
  `readPersistedMessagesFile` for read-only history rendering) -- evidence
  that the manifest files are treated as a resilient ground truth
  independent of whichever row-index backend is active, consistent with the
  three artifacts each being independently authoritative for their own
  fields (see The storage model).
- `listHostSessionRows()` (`history.ts`) filters subagents out of the
  default listing via `isRootSessionRecord()` (`!isSubagent &&
  !parentSessionId`) -- child sessions do not appear in the top-level task
  list by default.
- `shouldProjectLegacyRunningSessionAsIdle()` (`history.ts`) is a read-time
  display workaround: a session stuck in `running`+`interactive` state is
  projected as `idle` for display purposes without mutating the stored
  status -- a targeted patch for some known stuck-state class the comment
  does not fully explain; not investigated further here (Open questions).

No dedicated full-text/vector search subsystem was found in the source read
for this dossier; listing/search over session content, if any exists in the
VS Code webview, would be a client-side filter over the already-loaded
`HistoryItem[]`/`ClineMessage[]` in memory, not a store-side indexed search --
I did not exhaustively search the webview UI code for this, so treat as an
open question rather than a confirmed absence.

## Entry/message structure and versioning

Two distinct message types exist, precisely named, one persisted and one
derived:

**Model-facing, persisted type**: `MessageWithMetadata`, defined in
`sdk/packages/shared/src/llms/messages.ts:131-156`, re-exported from
`@cline/llms` (`sdk/packages/llms/src/providers/messages.ts:4-17`) and
aliased in the core package as `StoredMessageWithMetadata`
(`sdk/packages/core/src/types/session.ts:79`: `export type
StoredMessageWithMetadata = LlmsProviders.MessageWithMetadata;`). Verbatim:

```ts
export type MessageRole = "user" | "assistant";

export interface Message {
	role: MessageRole;
	content: string | ContentBlock[];
}

export interface MessageWithMetadata extends Message {
	id?: string;
	agent?: string;
	sessionId?: string;
	metadata?: Record<string, unknown>;
	modelInfo?: { id: string; provider: string; family?: string };
	metrics?: {
		inputTokens?: number;
		outputTokens?: number;
		cacheReadTokens?: number;
		cacheWriteTokens?: number;
		cost?: number;
	};
	ts?: number;
}
```
(`sdk/packages/shared/src/llms/messages.ts:12-156`). `ContentBlock` is a
tagged union of `TextContent`, `FileContent`, `ImageContent`,
`ToolUseContent`, `ToolResultContent`, `ThinkingContent`,
`RedactedThinkingContent` (same file, lines 17-116), each with a `type`
discriminant field and provider-agnostic shape (e.g. `ToolUseContent.id` +
`call_id?` to bridge Cline's internal id with a provider-native call id;
`ThinkingContent.signature`/`details`/`summary` for provider-specific
reasoning replay data). This is the **only** persisted message shape in the
current architecture (see The storage model / Read and resume path); the
messages file is `{version: 1, updated_at, agent, sessionId, taskType?,
messages: StoredMessageWithMetadata[], system_prompt?}`
(`buildMessagesFilePayload`, `sdk/packages/core/src/services/
session-data.ts:304-327`). Fields are normalized before every persist by
`normalizeStoredMessageModelMetadata()` (`session-data.ts:64-111`), which
migrates a legacy flat `providerId`/`modelId` shape into the nested
`modelInfo` object and mints a stable `id` via `nanoid()` if missing --
evidence of at least one prior on-disk shape for this same file that the
current code still tolerates on read.

**Display-facing, derived (not persisted in the current architecture)
type**: `ClineMessage`, defined in
`apps/vscode/src/shared/ExtensionMessage.ts:174-203`. Verbatim:

```ts
export interface ClineMessage {
	ts: number
	type: "ask" | "say"
	ask?: ClineAsk
	say?: ClineSay
	text?: string
	reasoning?: string
	images?: string[]
	files?: string[]
	partial?: boolean
	seq?: number
	epoch?: number
	commandCompleted?: boolean
	lastCheckpointHash?: string
	isCheckpointCheckedOut?: boolean
	isOperationOutsideWorkspace?: boolean
	conversationHistoryIndex?: number
	conversationHistoryDeletedRange?: [number, number]
	modelInfo?: ClineMessageModelInfo
}
```
`ClineAsk` and `ClineSay` (`ExtensionMessage.ts:205-262`) are large string
literal unions naming every distinct webview row kind (`"followup"`,
`"tool"`, `"completion_result"`, `"checkpoint_created"`, `"compaction"`,
`"subagent"`, etc.) -- this is fundamentally a **UI event-row** type, not a
conversational-turn type: a single model turn can expand into several
`ClineMessage` rows (one per tool call, one per reasoning block, etc.), and
conversely legacy-classic code persisted this type directly to
`ui_messages.json` (`apps/vscode/src/core/storage/disk.ts` /
`legacy-state-reader.ts`'s `readUiMessages()`, which filters out
`REMOVED_LEGACY_SAY_TYPES = new Set(["error_retry", "api_req_retried"])` on
read -- a small, explicit backward-incompatible-value migration). In the
current architecture `ClineMessage[]` is never written to disk on its own;
it is produced fresh by `message-translator.ts`'s live event-to-message
functions while a session is streaming, and by
`sdkMessagesToClineMessages()` (above) when a session's history is
opened/resumed. `seq`/`epoch` fields on `ClineMessage` are explicitly
documented (inline comments, `ExtensionMessage.ts:184-195`) as freshness/
identity fencing for the webview's convergent-replica message merge, "not
optional for classic/legacy" wording present, i.e. purely a runtime-replica
concern, unrelated to persistence.

**Versioning**: the manifest schema carries an explicit `version:
z.literal(1)` field (`SessionManifestSchema`,
`sdk/packages/core/src/session/models/session-manifest.ts:6-28`); the
messages-file payload and the compaction sidecar likewise both stamp
`version: 1` (`buildMessagesFilePayload`, `session-data.ts:309`;
`SessionCompactionStateSchema`, `sdk/packages/core/src/session/models/
session-compaction.ts:25-34`). All three are Zod schemas parsed on read
(`SessionManifestSchema.parse`/`.safeParse`,
`SessionCompactionStateSchema.parse`), so malformed or wrong-version files
fail closed (caught and treated as absent, not repaired). No `version: 2`
(or higher) branch, and no migration function keyed off this field, was
found anywhere in the checkout -- i.e. the format has not yet needed to
evolve since being introduced, or a future migration path simply does not
exist yet. Separately, `apps/vscode/src/core/storage/
state-migrations.ts:65-67` contains a function stub,
`migrateTaskHistoryToFile`, whose entire body is `// TODO migrate to sdk
location` -- i.e., a planned migration from the classic
`state/taskHistory.json` format into the new SDK session store is
acknowledged in a TODO but **not implemented** at this pinned commit. The
rest of `state-migrations.ts` (lines 1-63+) handles unrelated VS Code
workspace-state → global-state key migrations, not session-format
migrations, and was confirmed not to be the "entry/message format
versioning" mechanism the task brief's named anchor suggested it might be.

## Compaction and history management

A session's compaction state is a **separate sidecar artifact**, not an
in-place rewrite of the messages file. `SessionCompactionStateSchema`
(`sdk/packages/core/src/session/models/session-compaction.ts:25-34`):
```ts
export const SessionCompactionStateSchema = z.object({
	version: z.literal(1),
	updated_at: z.string().datetime(),
	conversation_id: z.string().min(1).optional(),
	source_message_count: z.number().int().nonnegative(),
	source_prefix_hash: z.string().min(1).optional(),
	source_last_message_key: z.string().min(1).optional(),
	messages: z.array(MessageWithMetadataSchema),
	system_prompt: z.string().optional(),
});
```
`persistSessionCompactionState`/`readSessionCompactionState`
(`UnifiedSessionPersistenceService`, `persistence-service.ts:326-341`,
delegating to `SessionManifestStore`, `session-manifest-store.ts:220-256`)
read/write this file at a path resolved from the manifest's
`compaction_path` field, falling back to the artifact-computed default path
(`resolveCompactionPath`, `session-manifest-store.ts:195-201`). The full
underlying messages file (`{sessionId}.messages.json`) is left untouched by
compaction; the compaction sidecar instead carries its own `messages` array
(the model-visible, shrunk view) plus a `source_prefix_hash`/
`source_last_message_key` used to detect whether the durable transcript has
since diverged from what was compacted (`sourcePrefixHash`/
`messageBoundaryKey`, `session-compaction.ts:86-113`, hashing role +
content + agent/session/metadata/modelInfo/metrics but explicitly
**excluding** `id`/`ts` from the hash -- the code comment explains this was
deliberate: hashing transport-identity fields "made projection fail for
semantically identical prefixes, so persistence was silently rejected every
turn", `session-compaction.ts:79-85` -- a concrete documented bug-and-fix in
the compaction consistency check itself).

This means: durable record (full messages file) persists in full; the
model-visible view shrinks via a parallel, independently-versioned sidecar.
Compaction is therefore an **upstream/session-runtime concern that leaves an
artifact in the store**, not a store-internal operation on the messages
file itself -- the store's role is limited to holding the sidecar and
exposing read/write/delete for it (`deleteSessionCompactionState`,
`persistence-service.ts:339-341`, `rm(path, {force: true})`).

I did not trace the runtime code that actually *decides when* to compact
(token-threshold triggers, manual `condense_task`/`summarize_task` UI
actions referenced by `ClineAsk`/`ClineSay` values like `"condense"`,
`"summarize_task"` in `ExtensionMessage.ts:220-221`) or how a resume
reconciles a compaction sidecar whose `source_prefix_hash` no longer matches
the current messages file -- flagged under Open questions.

## Rewind, checkpoints, and fork

**The official user-facing docs describe an architecture that does not
match the source at this commit.** `docs/core-workflows/checkpoints.mdx`
(same pinned commit) states under "How It Works": "Cline maintains a shadow
Git repository separate from your project's actual Git history. After each
tool use (file edits, commands, etc.), Cline commits the current state of
your files to this shadow repo." No `CheckpointTracker`/shadow-repository
class, no secondary `.git` directory creation, and no code implementing a
parallel repository was found anywhere in this checkout. This is a
significant, well-evidenced doc-vs-code discrepancy, not a matter of
interpretation -- the actual mechanism, detailed below, stores checkpoints
as **refs inside the user's own repository**.

**Actual mechanism**: `sdk/packages/core/src/hooks/checkpoint-hooks.ts`
(full file, 477 lines) and `sdk/packages/core/src/session/
checkpoint-restore.ts` (full file, 414 lines). Key facts:

- A checkpoint is a private git ref, `refs/cline/checkpoints/{sessionId}/
  {runCount}`, created directly inside the user's working repository -- not a
  separate repo, not a separate `.git` directory.
- The commit a checkpoint ref points at is produced via `git stash create`
  (captures tracked-file changes without touching the working tree or
  index), run through raw `execFile("git", [...])` calls -- **not** through
  the `simple-git` npm dependency that is listed in the package's
  dependencies but not used for this.
- Untracked files are folded in via a synthesized third parent commit:
  `createUntrackedParentCommit()` builds a commit object from a temporary
  index built with a scratch `GIT_INDEX_FILE` environment variable, so the
  final checkpoint commit can represent "tracked changes + untracked files"
  as a single addressable point even though `git stash create` alone only
  covers tracked state.
- `CHECKPOINT_STASH_MESSAGE_PREFIX = "cline checkpoint session="` is stamped
  into the stash commit message, used later to identify/verify checkpoint
  commits defensively (`resolveCheckpointKind()`,
  `checkpoint-restore.ts`, detects checkpoint "kind" -- `"stash"` vs.
  `"commit"` -- for pre-`kind`-field checkpoints via parent count + this
  message-prefix marker, i.e. a backward-compatibility shim for checkpoints
  created before the `kind` field existed).
- Checkpoints are **keyed by `runCount`**, an integer "user turn" counter
  that (per code comments) survives context compaction, via
  `getUserRunSpan` -- i.e., the checkpoint boundary concept is turn-based, not
  message-index-based, so it stays meaningful even after the messages array
  is compacted.
- Checkpoint creation is **gated**: hooks only fire on root (non-subagent)
  sessions, and only on the first iteration of a run
  (`createCheckpointHooks`, returns `beforeRun`/`beforeModel` `AgentHooks`
  callbacks with these gates inline) -- subagent/child sessions do not get
  their own checkpoint timeline.
- **Checkpoint history is stored inside the session's manifest metadata, not
  as separate files.** `CheckpointEntry {ref, createdAt, runCount, kind?:
  "stash"|"commit"}` and `CheckpointMetadata {latest, history}` are read/
  written via `readSessionMetadata`/`writeSessionMetadata` callbacks that
  operate on the session's `metadata` JSON blob
  (`checkpoint-hooks.ts`). This is independently corroborated by the type
  definitions in `sdk/packages/core/src/types/sessions.ts:33-44`:
  ```ts
  checkpoint?: {
  	latest?: { ref?: string; createdAt?: number; runCount?: number };
  	history?: Array<{ ref?: string; createdAt?: number; runCount?: number }>;
  };
  ```
  as a field of `SessionHistoryMetadata`. There is no `checkpoints/`
  directory of files anywhere in this codebase -- the task brief's
  hypothesis of a per-session checkpoints directory does not hold.
- **Restore** (`checkpoint-restore.ts`): `beginWorktreeRestoreTransaction(cwd)`
  first takes a safety-net stash + private ref
  (`refs/cline/restore-transactions/{transactionId}`) before any destructive
  operation. `applyCheckpointToWorktree(cwd, checkpoint)` then does `git
  reset --hard` to the checkpoint commit, conditionally `git clean -fd`, and
  for stash-kind checkpoints a `git stash apply` to layer back the
  originally-stashed working-tree state. `findCheckpointForRun()`/
  `trimMessagesToCheckpoint()`/`trimMessagesBeforeUserRun()` locate the
  checkpoint for a given run and trim the in-memory/persisted message array
  back to that point -- but explicitly **throw** if the target run has been
  folded into a compacted summary (i.e., you cannot restore to a point
  inside a compacted region; the compaction sidecar's boundary becomes a
  hard floor for how far back a checkpoint restore can reach).
- The VS Code gRPC handler for checkpoint restore delegates entirely to the
  SDK: `apps/vscode/src/core/controller/checkpoints/checkpointRestore.ts`
  (24 lines total) forwards straight to `controller.restoreCheckpoint`
  with **no legacy fallback branch** -- further confirming the old
  shadow-git `CheckpointTracker` class is gone from the live code path, not
  merely superseded-but-still-present.
- Cost model (as documented in code, not benchmarked by me): each checkpoint
  is one `git stash create` + up to one synthetic commit-tree operation
  against the user's real repository object database -- i.e., cost scales
  with the size of the working-tree diff since the last checkpoint (git's
  own object-store deduplication applies, since these are ordinary git
  objects), not with total conversation length. The official docs' own
  caveat -- "For very large repositories, checkpoints may use significant
  storage and slow down Cline as it commits file snapshots after each tool
  use" (`checkpoints.mdx`) -- is consistent with this being real git object
  writes against the real repo, even though the "shadow repository"
  framing around it is not accurate.

**Fork**: no distinct "fork a session" operation was found. Restoring to a
checkpoint mutates the working tree and trims the message array of the
*same* session in place; it does not create a new session id or a
copy-with-lineage. I did not find evidence of session branching/forking as
a first-class store operation anywhere in this codebase.

## Subagents and nested sessions

Subagents (and team-task sub-runs) are **first-class sibling sessions**,
linked to their parent by relational fields, not nested inside the parent's
storage:

- `SessionRow` carries `parentSessionId?`, `parentAgentId?`, `agentId?`,
  `conversationId?`, `isSubagent: boolean`
  (`sdk/packages/core/src/session/models/session-row.ts:24-28`) directly on
  every row, root or child alike.
- A subagent session's id is deterministic --
  `makeSubSessionId(rootSessionId, agentId)` (see Keying and identity) --
  so re-invoking the same named agent under the same root updates the
  existing child row rather than creating a new one
  (`TeamChildSessionManager.upsertSubagentSession`,
  `sdk/packages/core/src/session/team/team-child-session-manager.ts:132-160`,
  explicit `existing = await this.adapter.getSession(sessionId)` check
  before deciding insert vs. reuse).
- The child **inherits** (copies, at creation time -- not a live reference)
  several fields from the parent row: `provider`, `model`, `cwd`,
  `workspaceRoot`, `teamName`, `enableTools`, `enableSpawn`, `enableTeams`
  (`buildSubsessionRow`, `team-child-session-manager.ts:71-113`). It gets
  its **own** `messagesPath` -- a separate, isolated transcript file -- so
  subagent conversation content is not commingled with the root's messages
  file, even though the file itself lives as a sibling inside the root's
  session directory (`{rootSessionId}/{agentId}.messages.json`, per
  `subagentArtifactPaths`, `session-artifacts.ts:132-144`).
- Spawn requests are queued and claimed through the same
  `SessionPersistenceAdapter` interface (`enqueueSpawnRequest`/
  `claimSpawnRequest`), backed by a `subagent_spawn_queue` SQL table
  (`session-service.ts:223-260`) or the `subagent-spawn-queue.json` file
  (`file-session-service.ts:34-38, 231-267`) -- an at-least-once producer/
  consumer queue with a `consumed_at` marker for idempotent claiming, scoped
  per `(rootSessionId, parentAgentId)`.
- Status propagation cascades downward while running: on a status change to
  a terminal state (e.g. `"cancelled"`), the parent's
  `updateSessionStatus` explicitly propagates to children:
  `await this.teamChildren.applyStatusToRunningChildSessions(sessionId,
  "cancelled")` (`persistence-service.ts:205-211`).
- **On parent delete, children are explicitly cascade-deleted, not
  orphaned.** `UnifiedSessionPersistenceService.deleteSession()`
  (`persistence-service.ts:557-609`): for a non-subagent (root) session, it
  queries all rows with `parentSessionId === id` (limit 2000), deletes the
  parent row, deletes all matching child rows via
  `adapter.deleteSession(id, true)` (cascade flag), and then -- for every
  child -- deletes its checkpoint refs (`deleteCheckpointRefs(child.cwd,
  child.sessionId)`), its messages file, its compaction state, and its
  manifest file, finally removing the now-empty artifact directory
  (`removeSessionDirIfEmpty`). This is an explicit, code-level guarantee
  that Cline does not orphan subagent sessions on parent delete -- a direct
  contrast with at least one other product in this corpus (Grok Build,
  which the accepted dossier documents as orphaning children on parent
  delete).
- **The cascade is one level deep, and only from a root.** The child query
  sits inside `if (!row.isSubagent)` (`persistence-service.ts:566`), so
  deleting a session that is *itself* a subagent never looks for its own
  children, and the children it does find are deleted directly rather than
  recursed into. Cline is safe from orphaning today only because the
  parent-child graph is in practice one level deep; the guarantee is a
  property of how deep the graph happens to get, not of the delete
  algorithm. Read together with the absent depth cap noted below, this is a
  latent orphan path rather than a present bug, and it is the sharpest
  available evidence that a one-level cascade written against an assumed-flat
  graph is not the same thing as cascade semantics.
- Nesting depth: no explicit maximum-depth guard was found in the
  persistence layer itself (the task brief mentioned a `xai-grok-tools`-style
  "subagent depth cap" existing in another product; I did not find an
  equivalent constant/check in Cline's session-service/team code, though a
  cap could plausibly live in tool-definition/agent-loop code not read for
  this dossier -- flagged under Open questions rather than asserted absent).
- Team-task sub-sessions (as opposed to plain subagents) get a distinct,
  non-deterministic id shape (`makeTeamTaskSubSessionId`, trailing
  `nanoid(6)`) and are tagged `taskType: "team"` in their messages-file
  envelope (`resolveMessagesFileContext`,
  `sdk/packages/core/src/services/session-data.ts:279-302`, distinguishes
  `agent: "lead" | "subagent" | "teammate"`) -- teammates are a third
  category alongside root/subagent, gated separately in the message-file
  context resolver.

## Retention, deletion, and multi-host

**No growth bound was found for the messages file, and this is the
direct, source-confirmed mechanism behind the task brief's ">10MB / freezes
the extension" concern.** Two independent pieces of evidence:

1. In the still-active legacy write path, `FileContextTracker`
   (`apps/vscode/src/core/context/context-tracking/
   FileContextTracker.ts`, full file, 280 lines) appends a new entry to
   `metadata.files_in_context` on *every* file-read/edit/mention event
   during a task (`addFileToFileContextTracker()`), with **no cap, trim, or
   eviction of old entries**, then calls `saveTaskMetadata(taskId,
   metadata)` -- a full JSON re-serialize-and-rewrite of the entire
   `task_metadata.json` file
   (`apps/vscode/src/core/storage/disk.ts`) -- on every single such event.
   This is unbounded growth by construction: the file's size is
   monotonically non-decreasing across a task's lifetime, and every event
   pays the cost of rewriting the whole (growing) file.
2. In the current SDK messages file, both the write path
   (`persistSessionMessages`, full-file `JSON.stringify`+`writeFileSync` on
   every turn) and the read path (`readPersistedMessagesFile`, full-file
   `readFile`+`JSON.parse`) scale linearly with total transcript size, with
   no pagination, truncation, or size-based warning found anywhere in the
   store layer (see Write/append path and Read/resume path above).

**Corroborating secondary evidence (GitHub issue, not source -- cited only
to confirm this is user-visible, per the explicit task requirement):**
[cline/cline#9011](https://github.com/cline/cline/issues/9011), "When tasks
grow beyond ~5-10MB (measured by `api_conversation_history.json` +
`ui_messages.json`), clicking on them in RustRover/VS Code can cause the IDE
to become unresponsive or freeze indefinitely" (filed against Cline
v3.52.0, JetBrains plugin; opened 2026-02-01, closed 2026-07-03). The
reporter's own root-cause analysis (task size 10.5MB / 2016 UI messages /
590 API messages) matches the unbounded-full-file-JSON-parse mechanism
found in source above, though the specific files the reporter names
(`api_conversation_history.json`, `ui_messages.json`) are the **classic**
per-task files, not the SDK-era messages file -- i.e., this issue documents
the legacy-generation growth problem, and I have only source-level
(not issue-level) confirmation that the same unbounded-full-file pattern
also exists in the current SDK generation's messages file. A bot comment on
the issue additionally raises a **separate** contributing factor not
independently verified by me from source: a default 4 MiB gRPC message-size
limit in the extension's own webview/host IPC layer
(`src/standalone/protobus-service.ts` per the comment -- a path I did not
read, since it lives outside the areas the task pointed me at) --
mentioned here for completeness but not verified, and explicitly a
secondary-source claim.

**Retention/TTL/scheduled cleanup**: no time-based or size-based retention
policy (auto-archival, TTL, scheduled pruning) was found anywhere in the
persistence code read for this dossier. Deletion is entirely manual/
explicit, driven by `deleteSession()` (see Subagents, above, for its
cascade behavior) -- I found no cron-like or startup-triggered pruning job;
the only "automatic" lifecycle transition found is the crash-detection
reconciler (`reconcileDeadSessions`, marks stale `running` sessions
`failed` -- a status change, not a deletion).

**Delete cascade** (already detailed under Subagents): deleting a root
session deletes its row, its children's rows, and every artifact file
(messages, compaction state, manifest, checkpoint refs) for both the parent
and its children. There is no "no-op for append-only backends" case here
since nothing in this store is append-only.

**Multi-host / multi-process behavior**:
- VS Code explicitly forces `backendMode: "local"`
  (`apps/vscode/src/sdk/vscode-session-host.ts:126`) -- it never uses the
  SDK's "hub" (`HubRuntimeHost`) or "remote" (`RemoteRuntimeHost`) runtime
  modes that exist in `sdk/packages/core/src/runtime/host/host.ts:137-249`
  for other Cline surfaces. Those modes exist in the codebase (and `auto`
  mode will opportunistically discover/connect to a local hub daemon for
  other consumers, `host.ts:195-246`) but are not reachable from the VS Code
  extension at this commit.
- Crash detection is PID-liveness-based (`process.kill(pid, 0)`,
  `persistence-service.ts:424-437`), which only detects the crash of the
  process whose PID is recorded on the row -- it does not detect, e.g., a
  second process on a different machine sharing a network filesystem.
- Separately, the **classic** in-memory `StateManager` singleton
  (`apps/vscode/src/core/storage/StateManager.ts`, full file, 789 lines)
  documents its own, unrelated multi-instance caveat: it is an
  in-memory-cache-first store with a 500ms debounced disk-persistence
  timer (`PERSISTENCE_DELAY_MS = 500`), and its own comments explicitly
  state that other VS Code windows only observe another window's changes
  after a restart -- this concerns global extension settings/state (API
  keys, feature toggles), not session/task content, but is a genuine,
  source-documented multi-window staleness gap adjacent to the session
  store proper.
- The SQLite adapter's actual concurrency safety under concurrent writers
  (WAL mode? `busy_timeout`?) was **not confirmed** -- `loadSqliteDb`/
  `ensureSessionSchema` live in `@cline/shared/db`, a module I did not read
  in full for this dossier. Flagged under Open questions.

## Interop with foreign session stores

No evidence was found of Cline reading another *product's* native session
store (e.g., Claude Code, Codex, Aider). The only "foreign format" reading
found is **Cline reading its own prior generation's format**:
`apps/vscode/src/sdk/legacy-state-reader.ts` (full file, 309 lines) is
explicitly framed as replacing "classic `src/core/storage/disk.ts` reads
(see `origin/main`)... so the SDK adapter can surface tasks and settings
created before the SDK migration. All reads are non-throwing" -- i.e., this
is Cline importing/resuming its own earlier self, not a different agent
product. `readAllLegacyState()` (same file) is described in its own comment
as "the primary entry point for bootstrapping the SDK adapter from existing
on-disk data" -- a one-time/ongoing-fallback bootstrap, not a general
foreign-store import feature. This section is otherwise not applicable at
this commit as far as this research could establish.

## What this implies for our Session Store (our inference)

Cline's current architecture (the SDK generation) is best described as
**session-as-row plus session-as-document**, not an append-only event log:
every persisted artifact -- the SQLite/file-index row, the manifest, the
messages file, the compaction sidecar -- is a mutable document that gets
read in full and rewritten in full on each update, with no positional
append, no sequence-numbered event stream, and (for the two artifacts that
matter most, the messages file and the manifest) no atomic-write protection
at all. The one place Cline does something structurally closer to our
platform's event-sourced design is the row-level optimistic-concurrency
check (`statusLock` + `expectedStatusLock`, retried via `withOccRetry`) --
that pattern (an expected-version precondition on update, bounded retries)
is directly reusable vocabulary for a Session Store `append`/`update`
contract, even though Cline applies it to a whole-row replace rather than
an appended event.

The clearest structural lesson for us is the **dual-representation
resolution**: Cline used to persist two independently-authored transcripts
(model-facing, display-facing) and kept them in sync only because a single
long-lived in-process object streamed writes to both. That coupling was
fragile enough that the rewrite eliminated it entirely -- the current design
persists exactly one durable transcript (model-facing) and computes the
display transcript fresh, every time, from that one source, explicitly
documented as lossy for anything except the final turn. This is a strong
argument, independent of Cline's specific bugs, for our Session Store to
treat "the UI/display view" as a pure, versioned projection function over
the canonical event/message log rather than as a second thing that must be
kept consistent by discipline.

The clearest structural warning for us is the retention story: an
unbounded, full-file-rewrite-per-turn transcript format is a proven,
user-visible failure mode (issue #9011) even in a mature, widely-used
product -- reinforcing that our Session Store's append/read paths must be
genuinely incremental (bounded per-operation cost independent of total
session size), not merely "JSON, versioned, and hope it stays small."

## Open questions

- Does the SQLite backend (`@cline/shared/db`'s `loadSqliteDb`/
  `ensureSessionSchema`, not read for this dossier) enable WAL mode or set a
  busy timeout, and is concurrent access from two VS Code windows against
  the same `sessions.db` actually safe? The SQLite adapter's row-level CAS
  (`statusLock`) implies awareness of concurrent writers, but I found no
  direct evidence of the underlying SQLite connection's concurrency
  configuration.
- What happens, concretely, if a process is killed between the three writes
  in `createRootSessionWithArtifacts()` (row insert → messages file →
  manifest file), or mid-write to the messages/manifest file themselves
  (both plain `writeFileSync`, no atomic rename)? I found no reconciliation
  code for a missing/truncated messages or manifest file specifically (only
  for a dead-PID *status*), but also found no reproduction/issue confirming
  this occurs in practice -- left open rather than asserted as broken.
  Answering this would require reading `readManifestFile`'s and any related
  file's actual catch-and-recover behavior more exhaustively than time
  allowed here, plus exercising the crash scenario directly.
- What actually triggers compaction (token thresholds? explicit user action
  via the `"condense"`/`"summarize_task"` `ClineAsk` values?), and what
  happens on resume when a compaction sidecar's `source_prefix_hash` no
  longer matches the current messages file? `session-versioning-service.ts`
  (named in earlier planning as a file to read) was not reached in this
  research pass.
- Is there an explicit maximum subagent/team nesting depth enforced
  anywhere (tool-definition layer, agent-loop layer)? None was found in the
  session-persistence code specifically.
- Does `shouldProjectLegacyRunningSessionAsIdle()`
  (`sdk/packages/core/src/runtime/host/history.ts`) correspond to a known,
  named bug class, and is it still needed at this commit, or is it legacy
  defensive code for a since-fixed issue?
- Is there any project/workspace-scoped filtering of the session list in
  the VS Code UI, given that the underlying store (`~/.cline/data/
  sessions/`) is a single global flat namespace with `cwd` as just a row
  column? Not confirmed either way from the persistence-layer code read.
- Does Cline have any file-content-level deduplication or diff-based
  storage for the messages file (as opposed to full-content JSON), given
  how large tool-result blobs (file reads, command output) could
  contribute disproportionately to the ">10MB" growth pattern? Not
  addressed by any code read for this dossier -- `ContentBlock`'s
  `ToolResultContent.content` is stored as plain string/array content with
  no reference/hash-based sharing observed.
- Full verbatim confirmation of PR #11480 ("feat(sdk): cap tool output
  ingestion for bash and file reads", found via the issue-tracker search
  above) as a fix for the growth problem was not performed -- I confirmed
  only its title and that issue #9011 references related work via a linked
  Linear ticket (CLINE-1255); I did not read the PR's diff to confirm it
  actually caps messages-file growth versus something narrower (e.g., a
  single tool call's output size before it ever reaches the transcript).

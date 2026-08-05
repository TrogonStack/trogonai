# Crush: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against these authoritative anchors:

- Repo: `github.com/charmbracelet/crush`, cloned locally, pinned at commit
  `fcfad839bbeff6530249c5e77f872eee2c7cb90e` ("Merge pull request #3489 from
  charmbracelet/server-404s", committed 2026-08-03). `go.mod` declares
  `module github.com/charmbracelet/crush` / `go 1.26.5`.
- `internal/db/migrations/*.sql` -- 7 goose migrations, the schema ledger.
- `internal/db/models.go`, `internal/db/sessions.sql.go`, `internal/db/messages.sql.go` (and siblings `files.sql.go`, `read_files.sql.go`) -- sqlc-generated data-access layer.
- `internal/session/session.go`, `internal/message/message.go`, `internal/message/content.go`, `internal/history/file.go`, `internal/filetracker/service.go` -- the hand-written service layer that gives the generated rows their meaning.
- `internal/db/connect.go`, `internal/db/connect_ncruces.go`, `internal/db/connect_modernc.go`, `internal/db/datadirlock.go` -- connection lifecycle, pragmas, and locking.
- `internal/agent/agent.go`, `internal/agent/coordinator.go`, `internal/agent/tools/edit.go` -- compaction and subagent orchestration, and file-history write call sites.
- `internal/server/server.go`, `internal/proto/session.go`, `internal/cmd/session.go`, `internal/cmd/stats.go` -- the REST/CLI surfaces layered on top of the same SQLite file.

**License note (required):** Crush is licensed under **FSL-1.1-MIT**
(Functional Source License 1.1, MIT Future License), Copyright 2025-2026
Charmbracelet, Inc. (`LICENSE.md:1-9`). This is a source-available license,
**not** an OSI-approved open-source license, while the version stays inside
its window. `LICENSE.md:87-92` ("Grant of Future License") grants an
irrevocable MIT license effective "on the second anniversary of the date we
make the Software available," applied per released version -- the tail of the
file (`LICENSE.md:114-116` in this checkout) carries a standing MIT block
already covering an earlier, now-converted release window
("Copyright (c) 2025-03-21 - 2025-05-30 Kujtim Hoxha"). Anything cited from
this dossier as open-source precedent should carry this caveat: the code
inspected here is source-available under FSL terms today, converting to MIT
on a rolling two-year delay, not freely re-licensable at the time of this
snapshot.

## The storage model

Crush's durable session state is a **single SQLite database file per
project**, `crush.db`, opened at `filepath.Join(dataDir, "crush.db")`
(`internal/db/connect.go:93`). There is no external log format, no JSONL, and
no blob store: every session, message, and file version is a row in one of
four tables defined across 7 goose migrations
(`internal/db/migrations/20250424200609_initial.sql`,
`.../20250515105448_add_summary_message_id.sql`,
`.../20250624000000_add_created_at_indexes.sql`,
`.../20250627000000_add_provider_to_messages.sql`,
`.../20250810000000_add_is_summary_message.sql`,
`.../20250812000000_add_todos_to_sessions.sql`,
`.../20260127000000_add_read_files_table.sql`).

The initial migration defines the core schema
(`internal/db/migrations/20250424200609_initial.sql:1-98`):

```sql
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    parent_session_id TEXT,
    title TEXT NOT NULL,
    message_count INTEGER NOT NULL DEFAULT 0 CHECK (message_count >= 0),
    prompt_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (prompt_tokens >= 0),
    completion_tokens  INTEGER NOT NULL DEFAULT 0 CHECK (completion_tokens>= 0),
    cost REAL NOT NULL DEFAULT 0.0 CHECK (cost >= 0.0),
    updated_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
-- ...
CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE,
    UNIQUE(path, session_id, version)
);
-- ...
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    parts TEXT NOT NULL default '[]',
    model TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    FOREIGN KEY (session_id) REFERENCES sessions (id) ON DELETE CASCADE
);
```

(`internal/db/migrations/20250424200609_initial.sql:4-14` sessions,
`:24-34` files, `:47-57` messages.) A later migration adds a fourth table,
`read_files`, a freshness sidecar keyed by `(path, session_id)`
(`internal/db/migrations/20260127000000_add_read_files_table.sql:3-9`).

Three different mutability regimes coexist under one roof, and none of them
is a pure append-only log:

- **`sessions`** is a classic mutable document row: one row per session,
  updated in place (title, token/cost counters, `summary_message_id`,
  `todos`) via full-row `UPDATE` statements
  (`internal/db/sql/sessions.sql:43-53`, `:55-63`). Denormalized counters
  (`message_count`) are maintained by `AFTER INSERT`/`AFTER DELETE` triggers
  on `messages`, not recomputed at read time
  (`internal/db/migrations/20250424200609_initial.sql:68-82`).
- **`messages`** is row-per-message, but each row's `parts` column is a JSON
  array that gets **wholesale-overwritten** on every update
  (`internal/message/message.go:403-411`, the `UpdateMessage` call) -- this is
  a mutable-document-per-row model, not an append of deltas. The collection
  of message rows *is* ordered append-like (`ORDER BY created_at ASC` in
  `internal/db/sql/messages.sql`, referenced via `ListMessagesBySession` in
  `internal/db/querier.go:39`), so the session-as-transcript is a sequence of
  mutable rows rather than a sequence of immutable log entries.
- **`files`** is the one genuinely append-only structure: every edit inserts
  a *new* row with an incremented `version`, and the schema's
  `UNIQUE(path, session_id, version)` constraint
  (`internal/db/migrations/20250424200609_initial.sql:33`) plus the
  `history.Service` code path (`internal/history/file.go:58-135`) never
  update an existing file-version row in place.

Nothing here is a rebuildable projection in the fx/Zed sense of "derived from
an authoritative log": the `sessions`, `messages`, and `files` tables are all
independently authoritative for their own facts, cross-linked only by
`session_id` foreign keys. There is no separate index/search/cache layer
that could be dropped and rebuilt from a canonical log -- SQLite itself, via
its own B-tree indexes, is the only "index" in the system
(`CREATE INDEX ... idx_messages_session_id`,
`internal/db/migrations/20250424200609_initial.sql:59`, and similar).

Best-fit conceptual model: **session-as-row**, with a **message-collection**
of mutable per-message rows, and a wholly separate **file-version log**
keyed by path. It does not match session-as-transcript-file,
session-as-directory, or session-as-append-only-log cleanly; it is closest to
"session-as-row" extended with two child collections (messages, file
versions) that have different mutability characters from each other.

## Keying and identity

- **Top-level session ID**: `uuid.New().String()` (google/uuid v4), minted
  client-side in `internal/session/session.go:98` inside `service.Create`.
  The scheme carries no ordering information; `sessions.created_at` /
  `updated_at` (Unix-epoch integer columns) carry ordering instead.
- **Task/subagent session ID**: the *tool-call ID itself* is reused as the
  session's primary key -- `CreateTaskSession(ctx, toolCallID, parentSessionID, title)`
  sets `ID: toolCallID` directly (`internal/session/session.go:110-122`).
  `parent_session_id` is set to the caller's session ID
  (`internal/session/session.go:113`).
- **Title-generation session ID**: a derived string, `"title-" + parentSessionID`
  (`internal/session/session.go:124-136`) -- a deterministic, collidable
  composite key (calling `CreateTitleSession` twice for the same parent hits
  the `sessions.id` primary key and fails/upserts via `INSERT`, not observed
  guarded further in this file).
- **Composite agent-tool session key**: `CreateAgentToolSessionID(messageID, toolCallID)`
  returns `fmt.Sprintf("%s$$%s", messageID, toolCallID)`
  (`internal/session/session.go:351-353`), parsed back by
  `ParseAgentToolSessionID` (`:356-362`, splits on `"$$"`) and tested by
  `IsAgentToolSession` (`:365-368`). This is the ID actually passed as
  `agentToolSessionID` into `CreateTaskSession` from the coordinator
  (`internal/agent/coordinator.go:1404-1406`), so a running sub-agent's
  session ID **is** `toolCallID` (per `CreateTaskSession`'s own `ID: toolCallID`
  assignment) while the `$$`-joined string is a separate addressing scheme
  used to derive that `toolCallID`/`messageID` pairing -- both mechanisms sit
  side by side in the same file rather than being layered.
- **CLI short-ID / git-style addressing**: `session.HashID(id)` XXH3-hashes
  the UUID and hex-encodes it (`internal/session/session.go:27-32`).
  `resolveSessionID` (`internal/cmd/session.go:217-258`) first tries a direct
  `Get` by full ID, then falls back to an **O(n) scan of every session**
  returned by `List`, hashing each and matching on exact-or-prefix, with
  git-style ambiguity disambiguation printing all matches
  (`internal/cmd/session.go:229-256`) if more than one session shares a
  prefix.
- **Listing scope**: `ListSessions` is a plain `SELECT * FROM sessions WHERE
  parent_session_id is NULL ORDER BY updated_at DESC`
  (`internal/db/sql/sessions.sql:37-41`) -- scoped to whichever `crush.db` the
  process has open, i.e. implicitly scoped **per project**, since each
  project gets its own data directory (see below). There is no
  `workspace_id`/`project_id` column anywhere in the schema; the database
  file boundary *is* the project boundary. Subagent/task/title sessions
  (those with a non-null `parent_session_id`) are excluded from this and
  from every stats query (`internal/db/sql/stats.sql:1-9, 21-27`, all filter
  `WHERE parent_session_id IS NULL`).
- **Project scoping and relocation**: the data directory is resolved by
  `internal/config/load.go:556-559` via
  `fsext.LookupClosestBounded(workingDir, projectBoundary(workingDir), defaultDataDirectory)`
  (looks for an existing `.crush` bounded by the project root), falling back
  to creating `filepath.Join(workingDir, defaultDataDirectory)`
  (`defaultDataDirectory = ".crush"`, `internal/config/config.go:24`). There
  is no rename/relocation reconciliation logic found: if a project directory
  is physically moved as a whole, its `.crush/crush.db` moves with it and is
  found again at the new path; if a project is *re-created* at a new path
  without moving the old `.crush`, a fresh, empty `crush.db` is created there
  instead -- **inference**, not directly asserted by any code comment.
- **Cross-project enumeration**: the one place multiple projects' databases
  are read in a single operation is `crush stats`.
  `crawlForStats` walks a root directory looking for files literally named
  `crush.db` (`internal/cmd/stats.go:243-244, 261`), then
  `gatherStatsFromProjects` / `gatherStatsFromDBPaths`
  (`internal/cmd/stats.go:314, 339`) open each with `db.ConnectReadOnly`
  (no migrations run, `internal/db/connect.go:219-239`) and
  `mergeStats` (`internal/cmd/stats.go:388`) combines the read-only query
  results. This is an analytics-only aggregation path, not a session
  list/resume path -- session listing itself never spans more than one
  `crush.db`.

## The store interface

Crush has no pluggable session-store adapter/trait -- the store is internal.
Per the Method section's guidance for non-pluggable stores, the effective
interface is reconstructed at two layers.

**Layer 1 -- sqlc-generated `Querier`** (`internal/db/querier.go:11-49`),
reproduced verbatim (it is the actual Go interface implemented by
`*db.Queries`, `internal/db/querier.go:51`):

```go
type Querier interface {
    CreateFile(ctx context.Context, arg CreateFileParams) (File, error)
    CreateMessage(ctx context.Context, arg CreateMessageParams) (Message, error)
    CreateSession(ctx context.Context, arg CreateSessionParams) (Session, error)
    DeleteFile(ctx context.Context, id string) error
    DeleteMessage(ctx context.Context, id string) error
    DeleteSession(ctx context.Context, id string) error
    DeleteSessionFiles(ctx context.Context, sessionID string) error
    DeleteSessionMessages(ctx context.Context, sessionID string) error
    GetAverageResponseTime(ctx context.Context) (int64, error)
    GetFile(ctx context.Context, id string) (File, error)
    GetFileByPathAndSession(ctx context.Context, arg GetFileByPathAndSessionParams) (File, error)
    GetFileRead(ctx context.Context, arg GetFileReadParams) (ReadFile, error)
    GetHourDayHeatmap(ctx context.Context) ([]GetHourDayHeatmapRow, error)
    GetLastSession(ctx context.Context) (Session, error)
    GetMessage(ctx context.Context, id string) (Message, error)
    GetRecentActivity(ctx context.Context) ([]GetRecentActivityRow, error)
    GetSessionByID(ctx context.Context, id string) (Session, error)
    GetToolUsage(ctx context.Context) ([]GetToolUsageRow, error)
    GetTotalStats(ctx context.Context) (GetTotalStatsRow, error)
    GetUsageByDay(ctx context.Context) ([]GetUsageByDayRow, error)
    GetUsageByDayOfWeek(ctx context.Context) ([]GetUsageByDayOfWeekRow, error)
    GetUsageByHour(ctx context.Context) ([]GetUsageByHourRow, error)
    GetUsageByModel(ctx context.Context) ([]GetUsageByModelRow, error)
    ListAllUserMessages(ctx context.Context) ([]Message, error)
    ListFilesByPath(ctx context.Context, path string) ([]File, error)
    ListFilesBySession(ctx context.Context, sessionID string) ([]File, error)
    ListLatestSessionFiles(ctx context.Context, sessionID string) ([]File, error)
    ListMessagesBySession(ctx context.Context, sessionID string) ([]Message, error)
    ListNewFiles(ctx context.Context) ([]File, error)
    ListSessionReadFiles(ctx context.Context, sessionID string) ([]ReadFile, error)
    ListSessions(ctx context.Context) ([]Session, error)
    ListUserMessagesBySession(ctx context.Context, sessionID string) ([]Message, error)
    RecordFileRead(ctx context.Context, arg RecordFileReadParams) error
    RenameSession(ctx context.Context, arg RenameSessionParams) error
    UpdateMessage(ctx context.Context, arg UpdateMessageParams) error
    UpdateSession(ctx context.Context, arg UpdateSessionParams) (Session, error)
    UpdateSessionTitleAndUsage(ctx context.Context, arg UpdateSessionTitleAndUsageParams) error
}
```

Note `ListNewFiles` (`internal/db/querier.go:40`) queries a `WHERE is_new = 1`
predicate (`internal/db/sql/files.sql:58-62`, generated into
`internal/db/files.sql.go:244-249`) against a column that **does not exist**
in any of the 7 migrations -- see Open Questions.

**Layer 2 -- hand-written service interfaces**, which is what application
code (agent, tools, CLI, REST handlers) actually calls:

- `session.Service` (`internal/session/session.go:65-82`): `Create`,
  `CreateTitleSession`, `CreateTaskSession`, `Get`, `GetLast`, `List`, `Save`,
  `UpdateTitleAndUsage`, `Rename`, `Delete`, plus
  `CreateAgentToolSessionID`/`ParseAgentToolSessionID`/`IsAgentToolSession`.
- `message.Service` (`internal/message/message.go:46-68`): `Create`,
  `Update`, `Get`, `List`, `ListUserMessages`, `ListAllUserMessages`,
  `Delete`, `DeleteSessionMessages`, `Flush`, `FlushAll` -- with an explicit
  documented consistency contract (quoted in full under Write and append
  path below).
- `history.Service` (`internal/history/file.go:29-42`): `Create`,
  `CreateVersion`, `Get`, `GetByPathAndSession`, `ListBySession`,
  `ListLatestSessionFiles`, `Delete`, `DeleteSessionFiles`.
- `filetracker.Service` (`internal/filetracker/service.go:16-26`, thin
  wrapper over `read_files`): `RecordRead`, `LastReadTime`, `ListReadFiles`.

**Layer 3 -- external REST surface**, the one place a *different process* can
be a "caller" against the same store: `internal/server/server.go:171-202`
registers `GET/POST /v1/workspaces/{id}/sessions`,
`GET/PUT/DELETE /v1/workspaces/{id}/sessions/{sid}`, `.../history`,
`.../messages`, `.../messages/user`, `.../filetracker/files`, and an
`agent/sessions/{sid}` subresource (get, `/cancel`, `/prompts/queued`,
`/prompts/list`, `/prompts/clear`, `/summarize`, `/shell`). Transport is a
local Unix domain socket / Windows named pipe
(`maxUnixSocketPathLen = 104`, `internal/server/server.go:25`; `net.Listener`
field, `:97`; `listen(s.network, s.Addr)`, `:249`) -- not a remote/TCP path.
`internal/proto/session.go:1-37` defines the REST-layer `Session` DTO, with
two fields computed on read rather than persisted: `IsBusy` and
`AttachedClients` (`internal/proto/session.go:5-15`, doc comments explain
both are derived from in-memory workspace/coordinator state, not columns).

## Write and append path (ordering, durability, concurrency, delivery)

**Ordering.** `sessions.updated_at`/`created_at` and `messages.created_at`
are Unix-epoch integer columns set via SQLite's own `strftime('%s', 'now')`
(`internal/db/sql/sessions.sql:22-23`) -- server-assigned wall-clock
timestamps, not a monotonic sequence number. Message read order is
`ORDER BY created_at ASC` (implied by `ListMessagesBySession`, backing
`internal/message/message.go:456-469`); file-version order is `version`
ascending/descending depending on query (`internal/db/sql/files.sql`,
`ListFilesByPath` is read DESC per `internal/history/file.go:78`
comment "Files are ordered by version DESC, created_at DESC").

**Commit style differs by table.**

- `sessions`: full-row `UPDATE` (`internal/db/sql/sessions.sql:43-53`
  `UpdateSession`; `:55-63` `UpdateSessionTitleAndUsage`, which increments
  counters additively: `prompt_tokens = prompt_tokens + ?` etc., rather than
  overwriting them -- a partial compare-free increment, still not a CAS).
  `session.service.Save` documents itself as unsafe for concurrent
  read-modify-write of the whole row (`internal/session/session.go:223-224`
  comment: "safer than fetching, modifying, and saving the entire session")
  and both `UpdateTitleAndUsage` and `Rename` exist specifically to avoid
  that race for their narrower fields.
- `messages`: full-column overwrite of `parts` via `UpdateMessage`
  (`internal/message/message.go:403-409`), but writes are **coalesced**
  through an in-memory debounce buffer, not issued per keystroke of a
  streaming response. `message.Service`'s doc comment
  (`internal/message/message.go:31-45`) states the contract explicitly:

  ```go
  // Service is the public interface to the message store.
  //
  // [Service.Update] is eventually consistent: it accepts new state into
  // an in-memory buffer and writes it to SQLite plus publishes a
  // [pubsub.UpdatedEvent] on the next debounce tick (default
  // [defaultUpdateDebounce]) or on the next terminal-state update,
  // whichever comes first. Terminal-state updates — those that finish
  // the message, add or finish a tool call, or end a reasoning section —
  // flush synchronously before [Service.Update] returns.
  //
  // Callers that need stronger ordering (e.g. tests, shutdown,
  // session-switch reads) must use [Service.Flush] or [Service.FlushAll]
  // before reading via [Service.Get] / [Service.List]. Without an
  // explicit flush, a read can race the debounce timer and miss the
  // most recent in-memory state.
  ```

  `defaultUpdateDebounce = 33 * time.Millisecond`
  (`internal/message/message.go:21`). `shouldFlushNow`
  (`internal/message/message.go:418-446`) forces a synchronous flush when a
  message finishes, a tool call is added/finishes, or reasoning finishes --
  otherwise deltas coalesce for up to one debounce window. This is the
  closest thing in Crush to a write-behind cache in front of SQLite.
- `files`: pure insert-only, one new row per version, via `CreateFile`
  inside a transaction (`internal/history/file.go:93-131`). Never an
  `UPDATE` on an existing file-version row.
- `read_files`: upsert, `INSERT ... ON CONFLICT(path, session_id) DO UPDATE
  SET read_at = excluded.read_at` (`internal/db/sql/read_files.sql:1-11`) --
  a genuinely mutable single row per (session, path).

**Durability/atomicity.** Every connection sets, at open time
(`internal/db/connect.go:18-27`):

```go
pragmas = map[string]string{
    "foreign_keys":  "ON",
    "journal_mode":  "WAL",
    "page_size":     "4096",
    "temp_store":    "MEMORY",
    "cache_size":    "-8000",
    "synchronous":   "NORMAL",
    "secure_delete": "ON",
    "busy_timeout":  "30000",
}
```

Two build-tag-gated driver backends apply these identically but by different
mechanisms: `internal/db/connect_ncruces.go:23-43` (the `ncruces/go-sqlite3`,
CGO-free WASM-based driver, used for a narrower CPU-arch set) execs each
`PRAGMA name = value;` inside the connection-init callback and opens with DSN
`_txlock=immediate`; `internal/db/connect_modernc.go:20-46` (`modernc.org/sqlite`,
pure Go, used for the broader/default arch set per its build-tag list at
`internal/db/connect_modernc.go:1`) instead passes each pragma as a
`_pragma=name(value)` DSN query parameter, also with `_txlock=immediate`. Both
comments explain `_txlock=immediate` the same way: "Use BEGIN IMMEDIATE so
writers acquire the reserved lock up front, preventing deferred-to-writer
upgrade deadlocks." `foreign_keys: "ON"` matters directly for the cascade
question below -- SQLite ignores `FOREIGN KEY` clauses unless this pragma is
set per-connection, and Crush does set it on every connection, both driver
backends.

`conn.SetMaxOpenConns(1)` (`internal/db/connect.go:142`) serializes *all*
access through a single `database/sql` connection, with an explicit comment
citing a past incident: "allowing multiple pool connections to interleave
writes/checkpoints (especially under concurrent sub-agents) has caused
WAL/header desync resulting in SQLITE_NOTADB (26) on the next open"
(`internal/db/connect.go:137-141`). This single-connection choice is itself
the primary concurrency-control mechanism in the whole system -- it turns
SQLite's file-level write serialization into full statement-level
serialization within one process.

Cross-*process* concurrency for the same data directory has a second,
independent mechanism: an OS-level advisory `flock` on `{dataDir}/crush.lock`
(`internal/db/datadirlock.go:51-80`), acquired via `lock.TryFile`
non-blocking, deliberately never unlinked (`internal/db/datadirlock.go:73-78`
explains the flock-is-keyed-by-inode-not-path hazard this avoids). This lock
is **opt-in per `Connect` call** via `WithDataDirLock(true)`
(`internal/db/connect.go:69-76`), and the only call site enabling it is the
server/workspace-bootstrap path, `internal/backend/backend.go:426`:
`db.Connect(b.ctx, cfg.Config().Options.DataDirectory, db.WithDataDirLock(true))`.
Ordinary local single-process TUI/CLI usage does not take this lock -- it
relies solely on `SetMaxOpenConns(1)` plus SQLite's own file locking. An
escape hatch, `CRUSH_SKIP_DATADIR_LOCK`, bypasses acquisition entirely
(`internal/db/datadirlock.go:83-86`).

**Transactions.** Explicit `BeginTx`/`Commit`/`Rollback` wrapping is used in
exactly two places: `session.service.Delete`'s three-statement cascade
(`internal/session/session.go:138-169`, quoted under Subagents below) and
`history.service.createWithVersion`'s retry loop
(`internal/history/file.go:84-135`). Everything else (`message.Service`'s
writes, `session.Save`/`Rename`/`UpdateTitleAndUsage`) is a single SQL
statement relying on SQLite's own per-statement atomicity, with no
app-level transaction wrapper.

**Concurrency model / expected-version precondition.** There is **no
optimistic-concurrency/expected-version precondition** anywhere in the
session, message, or history service layers -- confirmed by an explicit grep
across `internal/session/*.go`, `internal/message/*.go`, `internal/history/*.go`
for `expected_version`/CAS/compare-and-swap patterns, with zero matches
outside the one UNIQUE-constraint-retry loop below. The closest thing to
optimistic concurrency is in `history.service.createWithVersion`
(`internal/history/file.go:84-135`):

```go
func (s *service) createWithVersion(ctx context.Context, sessionID, path, content string, version int64) (File, error) {
    const maxRetries = 3
    var file File
    var err error
    for attempt := range maxRetries {
        tx, txErr := s.db.BeginTx(ctx, nil)
        ...
        qtx := s.q.WithTx(tx)
        dbFile, txErr := qtx.CreateFile(ctx, db.CreateFileParams{
            ID: uuid.New().String(), SessionID: sessionID, Path: path,
            Content: content, Version: version,
        })
        if txErr != nil {
            tx.Rollback()
            if strings.Contains(txErr.Error(), "UNIQUE constraint failed") {
                if attempt < maxRetries-1 {
                    version++
                    continue
                }
            }
            return File{}, txErr
        }
        if txErr = tx.Commit(); txErr != nil { ... }
        file = s.fromDBItem(dbFile)
        s.Publish(pubsub.CreatedEvent, file)
        return file, nil
    }
    return file, err
}
```

This is retry-on-conflict via the `UNIQUE(path, session_id, version)`
constraint, auto-incrementing the version up to 3 attempts -- not a
caller-supplied expected-version precondition (the caller never states what
version it expects to be writing over).

**Delivery semantics.** Best-effort within the process: `message.Service`
documents itself as eventually consistent (quoted above); `pubsub.Broker`
events are fire-and-forget except for `PublishMustDeliver` used for terminal
message events (`internal/message/message.go:378-382`). There is no
client-supplied idempotence key / dedup-by-entry-id anywhere -- every write
path mints a fresh `uuid.New().String()` server-side
(`internal/message/message.go:177`, `internal/history/file.go:103`,
`internal/session/session.go:98`).

## Read and resume path

Resume always reads the durable SQLite file directly -- there is no separate
resume-time cache or filesystem snapshot. `session.service.Get` /
`session.service.List` (`internal/session/session.go:171-179, 252-263`) are
thin wrappers over `GetSessionByID`/`ListSessions`. Message history is
reconstructed by a **full ordered `SELECT`** of every row for the session,
`ListMessagesBySession` (backing `internal/message/message.go:456-469`),
with **no pagination, cursor, or offset** anywhere in the read path -- cost
scales linearly with the number of messages ever created in that session, no
stated bound found.

The agent-level read path, `getSessionMessages`
(`internal/agent/agent.go:1692-1711`), is the one place a *bound* is applied,
and it is applied **after** the full read, in memory:

```go
func (a *sessionAgent) getSessionMessages(ctx context.Context, session session.Session) ([]message.Message, error) {
    msgs, err := a.messages.List(ctx, session.ID)
    if err != nil {
        return nil, fmt.Errorf("failed to list messages: %w", err)
    }
    if session.SummaryMessageID != "" {
        summaryMsgIndex := -1
        for i, msg := range msgs {
            if msg.ID == session.SummaryMessageID {
                summaryMsgIndex = i
                break
            }
        }
        if summaryMsgIndex != -1 {
            msgs = msgs[summaryMsgIndex:]
            msgs[0].Role = message.User
        }
    }
    return msgs, nil
}
```

Because `message.Service.Update` buffers state in memory with up to a
33ms debounce (`internal/message/message.go:21`), a read immediately after a
write can race the debounce timer; the service's own doc comment
(`internal/message/message.go:41-45`) says callers needing a
guaranteed-fresh read (explicitly including "session-switch reads") must call
`Flush`/`FlushAll` first. `FlushAll`'s own doc comment
(`internal/message/message.go:280-284`) confirms it exists specifically for
"shutdown and session-switch paths."

There is no lazy/eager split beyond this: everything returned by `List` is
materialized eagerly into memory as `message.Message` structs
(`internal/message/message.go:456-469`); nothing is fetched lazily
per-field.

## Listing, summaries, and search

`session list` / the sessions picker reads the full `sessions` table for the
current project's `crush.db` (`WHERE parent_session_id is NULL ORDER BY
updated_at DESC`, `internal/db/sql/sessions.sql:37-41`) -- no pagination
found, cost bounded by the number of top-level sessions in that one
project's database, not globally.

There is a metadata sidecar, but it is **denormalized in place on the
`sessions` row itself**, not a separate projection table: `message_count` is
maintained by `AFTER INSERT`/`AFTER DELETE` triggers
(`internal/db/migrations/20250424200609_initial.sql:68-82`), while
`prompt_tokens`/`completion_tokens`/`cost`/`title`/`summary_message_id`/`todos`
are maintained by explicit application writes
(`UpdateSessionTitleAndUsage`, `internal/db/sql/sessions.sql:55-63`; `Save`,
`internal/session/session.go:191-221`). Consistency with the underlying
`messages`/`files` rows is maintained by the write paths themselves (the
trigger fires in the same transaction as the `INSERT`/`DELETE`); there is no
separate rebuild/reconciliation job found.

**No search subsystem exists.** No FTS table, no vector index, and no
external search service were found anywhere in `internal/db` or
`internal/message` (targeted greps for `fts`, `MATCH`, `embedding`, `vector`
all returned no relevant hits in the schema/service code). The only
"search"-like operation is the CLI's exact/prefix XXH3-hash match over an
in-memory list, `resolveSessionID` (`internal/cmd/session.go:217-258`) -- a
git-style short-ID resolver, not content search.

Cross-project aggregation is limited to the `crush stats` command
(`internal/cmd/stats.go:243-244, 261, 314, 339, 388`), which crawls the
filesystem for files literally named `crush.db`, opens each read-only via
`db.ConnectReadOnly` (no migrations, `internal/db/connect.go:219-239`), and
merges the analytics query results. This is the sole place more than one
project database is read in a single operation, and it is analytics-only --
it does not feed the session list/resume UI.

## Entry/message structure and versioning

The entry type is **not** in `internal/db` -- as flagged going in, it lives in
`internal/message/message.go` and `internal/message/content.go`, decoded out
of the `messages.parts` JSON TEXT column.

`ContentPart` is a closed interface with a marker method
(`internal/message/content.go:51-53`):

```go
type ContentPart interface {
    isPart()
}
```

Eight concrete implementations, with full field lists
(`internal/message/content.go:55-146`):

```go
type ReasoningContent struct {
    Thinking         string
    Signature        string
    ThoughtSignature string                              // Used for google
    ToolID           string                              // Used for openrouter google models
    ResponsesData    *openai.ResponsesReasoningMetadata
    StartedAt        int64
    FinishedAt       int64
}
type TextContent struct { Text string }
type ImageURLContent struct { URL, Detail string }
type BinaryContent struct { Path, MIMEType string; Data []byte }
type ToolCall struct {
    ID, Name, Input  string
    ProviderExecuted bool
    Finished         bool
}
type ToolResult struct {
    ToolCallID, Name, Content, Data, MIMEType, Metadata string
    IsError bool
}
type Finish struct {
    Reason  FinishReason
    Time    int64
    Message, Details string
}
// ShellCommand stores a bang-mode shell command and its output as a
// distinct content part so it can be reconstructed on session restore.
type ShellCommand struct {
    Command  string
    Output   string
    ExitCode int
}
```

The JSON envelope is a hand-rolled tagged union, not a generic
`serde`/reflection scheme
(`internal/message/message.go:519-535, 537-646`):

```go
type partType string

const (
    reasoningType    partType = "reasoning"
    textType         partType = "text"
    imageURLType     partType = "image_url"
    binaryType       partType = "binary"
    toolCallType     partType = "tool_call"
    toolResultType   partType = "tool_result"
    finishType       partType = "finish"
    shellCommandType partType = "shell_command"
)

type partWrapper struct {
    Type partType    `json:"type"`
    Data ContentPart `json:"data"`
}
```

`marshalParts` type-switches on the concrete Go type to pick the tag
(`internal/message/message.go:537-570`); `unmarshalParts` does a two-pass
decode -- first into `[]json.RawMessage`, then per-element into an untyped
`{Type, Data json.RawMessage}` struct, then a `switch wrapper.Type` to
unmarshal `Data` into the matching concrete struct
(`internal/message/message.go:572-646`). There is **no schema-version field**
anywhere in this envelope or in the `messages` row -- additive fields
(`provider`, `is_summary_message`) were added as plain `ALTER TABLE ... ADD
COLUMN` migrations instead
(`internal/db/migrations/20250627000000_add_provider_to_messages.sql`,
`internal/db/migrations/20250810000000_add_is_summary_message.sql`), and the
`Message` Go struct picks up new fields via new `sql.NullString`/`int64`
columns on the generated `db.Message` (`internal/db/models.go:21-32`), not
via any envelope version discriminator. If the JSON shape of an existing
`ContentPart` type ever changed incompatibly, there is no sniffing/migration
mechanism visible in this tree to reinterpret old rows -- this is flagged
under Open Questions.

**Storage-level format evolution** is entirely goose's migration ledger:
7 one-way `-- +goose Up` migrations, each also carrying a `-- +goose Down`
block, but the app only ever calls `goose.Up(conn, "migrations")`
(`internal/db/connect.go:163`) -- no code path found that invokes
`goose.Down`. Whether an edited-in-place migration file would be detected
(goose tracks applied version numbers in its own `goose_db_version` table,
which is a dependency's behavior, not code in this tree) was not verified
against goose's own source and is left as an inference boundary, unlike
Zed's `sqlez`, which strict-diffs migration text itself.

## Compaction and history management

Compaction is a **marker pattern that never deletes durable history** -- the
full session's `messages` rows are never truncated or rewritten by
summarization.

`sessionAgent.Summarize` (`internal/agent/agent.go:1332-1471`) creates a
brand-new message row flagged as a summary
(`internal/agent/agent.go:1372-1377`):

```go
summaryMessage, err := a.messages.Create(ctx, sessionID, message.CreateMessageParams{
    Role:             message.Assistant,
    Model:            largeModel.ModelCfg.Model,
    Provider:         largeModel.ModelCfg.Provider,
    IsSummaryMessage: true,
})
```

then, once the summary content finishes streaming, the session row's
`summary_message_id` pointer is set to that message's ID and the session is
saved (`internal/session/session.go:191-221` `Save`, invoked from
`internal/agent/agent.go` later in `Summarize`). `messages.is_summary_message`
(`internal/db/migrations/20250810000000_add_is_summary_message.sql`) and
`sessions.summary_message_id`
(`internal/db/migrations/20250515105448_add_summary_message_id.sql`) are the
two on-disk artifacts this leaves.

The read-side truncation is entirely `getSessionMessages`
(`internal/agent/agent.go:1692-1711`, quoted in full under Read and resume
path): it always lists the *complete* message history first, then, only if
`session.SummaryMessageID != ""`, slices the in-memory result down to
`msgs[summaryMsgIndex:]` and relabels that first (summary) message's `Role`
to `message.User` before handing it to the model as the new conversation
root. Any other reader of `messages.List` (REST `.../history`, `.../messages`,
the CLI, `crush stats`) sees the full, untruncated row set -- the compaction
boundary is a **model-context-builder concern**, applied at one specific
call site, not a store-level concern. This is functionally the same pattern
documented for Zed's `Message::Compaction` marker: durable history persists,
only the model-visible window shrinks, applied at exactly one read call site
rather than store-wide.

## Rewind, checkpoints, and fork

No session-level rewind/undo/branch verb exists. A targeted search across
`internal/` for `rewind`, `checkpoint`, `fork`/`Fork` turned up only UI ASCII
art and one WAL-checkpoint code comment (`internal/db/connect.go:139`,
referring to SQLite's own WAL-checkpoint mechanism, unrelated to
session/turn checkpoints) -- no session-retroactive-edit feature was found.

What Crush has instead is **per-file version history tied to tool calls**,
not a session-level checkpoint/restore mechanism. Every file-mutating tool
(`internal/agent/tools/edit.go`, `multiedit.go`, `write.go`,
`lsp_rename.go`, `lsp_replace_symbol.go`) calls into `history.Service` to
record full file content per edit. `commitFileChange`
(`internal/agent/tools/edit.go:246-269`) is the representative call site:

```go
func commitFileChange(edit editContext, sessionID, filePath, oldContent, newContent string) error {
    if err := os.WriteFile(filePath, []byte(newContent), 0o644); err != nil {
        return fmt.Errorf("failed to write file: %w", err)
    }
    file, err := edit.files.GetByPathAndSession(edit.ctx, filePath, sessionID)
    if err != nil {
        _, err = edit.files.Create(edit.ctx, sessionID, filePath, oldContent)
        if err != nil {
            return fmt.Errorf("error creating file history: %w", err)
        }
    }
    if file.Content != oldContent {
        // User manually changed the content; store an intermediate version.
        if _, err := edit.files.CreateVersion(edit.ctx, sessionID, filePath, oldContent); err != nil {
            slog.Error("Error creating file history version", "error", err)
        }
    }
    if _, err := edit.files.CreateVersion(edit.ctx, sessionID, filePath, newContent); err != nil {
        slog.Error("Error creating file history version", "error", err)
    }
    edit.filetracker.RecordRead(edit.ctx, sessionID, filePath)
    return nil
}
```

Each edit can write **one or two full-content rows** -- an intermediate
"old content" version whenever on-disk content has drifted from the last
recorded version (out-of-band user edits), plus always a new row for the
post-edit content. Content is stored **whole, not diffed, not
content-addressed, not deduplicated** -- `files.content TEXT NOT NULL`
(`internal/db/migrations/20250424200609_initial.sql:28`) holds the entire
file on every version. This is the same anti-pattern flagged in the fx
reference dossier (`previous_content` full-pre-image inlining): N edits to
one file cost roughly N (or 2N) full copies inside `crush.db`, unbounded by
anything but the file's own size times edit count.

Nothing reads an old version back onto disk automatically: `history.Service`
exposes only `Get`/`GetByPathAndSession`/`ListBySession`/
`ListLatestSessionFiles` as reads (`internal/history/file.go:29-42`), and a
grep of every caller of those methods
(`internal/workspace/app_workspace.go:294`, `internal/backend/session.go:91`,
`internal/agent/tools/edit.go:251`, `internal/agent/tools/write.go:143`)
shows them used only to *compare against* current on-disk content before
writing a new version -- no "restore file to version N" tool/command was
found. Versions are recorded but, in this codebase, apparently
write-only/diagnostic rather than restorable -- flagged under Open Questions
since a UI-only restore action (outside the greppable Go source, e.g. a TUI
key binding calling an undocumented path) cannot be ruled out from source
alone.

Fork (branch a session from a point) has no code path found anywhere.

## Subagents and nested sessions

Subagent/task sessions are **first-class sibling rows** in the same
`sessions` table, linked to their parent only by the un-typed
`parent_session_id TEXT` column
(`internal/db/migrations/20250424200609_initial.sql:6`) -- there is no
separate "nested session" table or embedded-transcript structure. A child
session's messages/files are isolated in their own rows (own `session_id`),
not inherited or merged into the parent's transcript.

**The central, surprising finding**: `parent_session_id` carries **no
foreign-key constraint anywhere** in any of the 7 migrations -- unlike
`files.session_id`, `messages.session_id`, and `read_files.session_id`,
which all declare `FOREIGN KEY (session_id) REFERENCES sessions (id) ON
DELETE CASCADE`
(`internal/db/migrations/20250424200609_initial.sql:32, 56`;
`internal/db/migrations/20260127000000_add_read_files_table.sql:7`).
`parent_session_id` is declared as a bare, unconstrained `TEXT` column
(`internal/db/migrations/20250424200609_initial.sql:6`) in the initial
migration and never gains a constraint in any later migration. Meanwhile,
`PRAGMA foreign_keys = "ON"` **is** set on every connection
(`internal/db/connect.go:19`, applied identically via both driver backends,
`internal/db/connect_ncruces.go:28-37`, `internal/db/connect_modernc.go:29-39`)
-- so the FKs that *do* exist (on `files`, `messages`, `read_files`) are
genuinely enforced by SQLite at runtime. The parent/child session
relationship itself, however, is simply never expressed in DDL, so there is
nothing for `foreign_keys=ON` to enforce on it.

Consistent with that, the application-level delete path does **not** cascade
to child sessions either. `session.service.Delete`
(`internal/session/session.go:138-169`):

```go
func (s *service) Delete(ctx context.Context, id string) error {
    tx, err := s.db.BeginTx(ctx, nil)
    ...
    qtx := s.q.WithTx(tx)
    dbSession, err := qtx.GetSessionByID(ctx, id)
    ...
    if err = qtx.DeleteSessionMessages(ctx, dbSession.ID); err != nil { ... }
    if err = qtx.DeleteSessionFiles(ctx, dbSession.ID); err != nil { ... }
    if err = qtx.DeleteSession(ctx, dbSession.ID); err != nil { ... }
    if err = tx.Commit(); err != nil { ... }
    ...
}
```

This deletes only the target session's own `messages` and `files` rows (via
their real `ON DELETE CASCADE` FKs firing as a backstop, plus the explicit
`DeleteSessionMessages`/`DeleteSessionFiles` calls before it) and the
session row itself -- there is no query anywhere for
`WHERE parent_session_id = ?` inside `Delete`, and no recursive walk of
children. **Net conclusion: deleting a parent session in Crush orphans any
child (task/title/agent-tool) sessions -- their rows, messages, and file
versions survive untouched, unreferenced, and undiscoverable through normal
listing** (since `ListSessions` filters `parent_session_id IS NULL`,
`internal/db/sql/sessions.sql:40`, an orphan with a now-dangling
`parent_session_id` is invisible to `session list` but still occupies rows
in `crush.db` indefinitely). No reconciliation/orphan-sweep job was found
anywhere in the codebase.

Nesting depth (subagent spawning further subagents) is **not hardcoded**.
`buildTools` decides whether a given agent's own tool set includes the task
tool that can spawn a further subagent, gated purely by config
(`internal/agent/coordinator.go:681-683`):

```go
func (c *coordinator) buildTools(ctx context.Context, agent config.Agent, isSubAgent bool) ([]fantasy.AgentTool, error) {
    var allTools []fantasy.AgentTool
    if slices.Contains(agent.AllowedTools, AgentToolName) {
        agentTool, err := c.agentTool(ctx)
        ...
    }
```

Whether recursion is actually bounded therefore depends entirely on whether
a given `agent.AllowedTools` config includes `AgentToolName` for sub-agents
-- there is no `MAX_SUBAGENT_DEPTH`-style constant found (unlike Zed's
hardcoded `u8 = 1`). `runSubAgent`
(`internal/agent/coordinator.go:1402-1408`) creates the child session via
`CreateAgentToolSessionID` + `CreateTaskSession`, and
`updateParentSessionCost` (`internal/agent/coordinator.go:1487-1503`) rolls
the child's `Cost` into the parent's via an unguarded
read-modify-write-and-save (`parentSession.Cost += childSession.Cost`,
`internal/agent/coordinator.go:1497`) -- not wrapped in the same transaction
as anything else, relying entirely on the single-connection serialization
(`SetMaxOpenConns(1)`) to avoid lost updates from truly concurrent
subagents, rather than an atomic `UPDATE ... SET cost = cost + ?`
(`UpdateSessionTitleAndUsage`, which *does* do the increment-in-SQL version,
`internal/db/sql/sessions.sql:55-63`, is not the function used for cost
rollup here). Flagged as a potential race under Open Questions.

On crash: nothing beyond normal SQLite crash recovery was found. There is no
special-cased "in-flight subagent" cleanup at startup; a killed process's
child sessions simply remain rows with whatever state they last flushed.

## Retention, deletion, and multi-host

**Retention/TTL**: none found. No scheduled cleanup job, no TTL column, no
lifecycle policy anywhere in `internal/`. Deletion is exclusively
user/CLI/REST-initiated (`sessionDeleteCmd`,
`DELETE /v1/workspaces/{id}/sessions/{sid}`).

**Delete cascade**: within one session, `session.service.Delete`
(`internal/session/session.go:138-169`, quoted above) deletes messages and
files for that session inside one transaction, then the session row --
real cascade for the session's own children (messages/files), but, as
established above, **no cascade to child sessions**.

**Multi-host / multi-process**: multi-host is **not a first-class remote
path**. The "server" mode (`internal/server`, `internal/backend`) is a
local-machine IPC surface over a Unix domain socket / named pipe
(`internal/server/server.go:25, 97, 249`), letting multiple *local* clients
(e.g. multiple TUI/CLI invocations) share one running process's connection
to one `crush.db`. There is no network-filesystem or remote-writeback
handling found -- the OS-level `flock` (`internal/db/datadirlock.go:51-80`)
that gates concurrent access assumes a single local filesystem and a single
host; it is a same-machine, cross-*process* guard (default off, opt-in only
for the server bootstrap path, `internal/backend/backend.go:426`), not a
distributed-lock or leader-election mechanism. Crash detection is limited to
the informational (non-authoritative) `dataDirOwnerInfo` JSON payload
written into `crush.lock` (`internal/db/datadirlock.go:26-33, 88-98`) -- the
comment at `internal/db/datadirlock.go:73-78` is explicit that "the
authoritative state of ownership is the operating system flock on the file
descriptor," not the JSON payload, and a stale lock file left by a crashed
process's still-open fd is reclaimed the moment the kernel closes that fd,
with no separate app-level liveness check.

## Interop with foreign session stores

None found. No code path in this tree reads, imports, or converts session
data from another product's native store (Claude Code, Amazon Q, opencode,
etc.) -- targeted greps for common competitor session-file names/formats
inside `internal/` returned no hits. This section is otherwise not
applicable.

## What this implies for our Session Store (our inference)

**Our inference**: a stored session in Crush is "a row in `sessions` plus
whatever rows in `messages`/`files`/`read_files` reference its `id`" -- there
is no single authoritative append-only log underneath; instead there are
three independently-authoritative, differently-mutable stores (a
mutable session document, a collection of mutable-per-row messages, and a
genuinely append-only per-path file-version log) unified only by foreign
keys and a shared connection. It sits closer to a classic mutable-row RDBMS
model than to an event-sourced design: there is no single ordered log a
projection could be rebuilt from, updates are last-write-wins with no
expected-version precondition anywhere, and the one place durable history is
deliberately preserved across a destructive-looking operation (compaction)
does so via an in-place marker + read-time slice rather than an append-only
event.

For our event-sourced Session Store, the two most transferable/cautionary
data points are: (1) the `parent_session_id`-without-FK finding shows that an
unenforced, DDL-invisible parent pointer silently produces permanent orphan
rows on delete -- our design should make the parent-child cascade/orphan
policy an explicit, enforced part of the schema (or an explicit, tested
reconciliation job) rather than an implicit convention; and (2) the
full-content, non-deduplicated file-version store is a concrete
cautionary example (alongside fx's `previous_content`) of unbounded storage
growth from turn-level file snapshots -- worth costing out explicitly if our
design considers storing file state per tool call.

## Open questions

- **`ListNewFiles`/`is_new` schema mismatch**: `internal/db/sql/files.sql:58-62`
  and generated `internal/db/files.sql.go:244-249` reference a
  `files.is_new` column that does not exist in any of the 7 migrations.
  Confirmed dormant, not a live bug: the app only ever constructs queries via
  `db.New(conn)` (`internal/db/db.go:20-22`), never `db.Prepare()`
  (`internal/db/db.go:24-138`, which eagerly prepares every statement
  including `ListNewFiles` at `internal/db/db.go:111-113` and would fail
  loudly on this column), and `ListNewFiles` itself is never called anywhere
  in application code. Left unresolved: why this column was ever generated
  without a corresponding migration, and whether it is stale
  work-in-progress or a since-abandoned feature.
- **`updateParentSessionCost` race**: `internal/agent/coordinator.go:1487-1503`
  does an unguarded read-modify-write-and-save of `parentSession.Cost`, not
  an atomic SQL increment. Whether this is actually racy in practice depends
  on invariants not fully traced here (e.g., whether the coordinator
  serializes subagent completions before calling this, beyond the blanket
  `SetMaxOpenConns(1)` DB-level serialization) -- not fully verified.
- **File-version restore**: versions are recorded (`internal/history/file.go`)
  and diffed-against-for-drift-detection (`internal/agent/tools/edit.go:258-263`),
  but no "restore file to version N" code path was found via grep of every
  caller of the history read methods. Cannot rule out a TUI-only interaction
  not visible to a source-level grep.
- **Goose migration-ratchet strictness**: whether goose (a third-party
  dependency, not vendored in this tree) detects an edited-in-place migration
  file the way Zed's own `sqlez` does was not verified against goose's
  source -- flagged as an inference boundary, not a checked claim.
- **Title-session ID collision**: `CreateTitleSession` mints a deterministic
  `"title-" + parentSessionID` key (`internal/session/session.go:124-136`)
  with no guard shown against calling it twice for the same parent within
  this file -- behavior on a second call (unique-constraint error vs. silent
  reuse) was not traced into `CreateSession`'s SQL (`INSERT`, no
  `ON CONFLICT`, `internal/db/sql/sessions.sql:1-24`), so a second call would
  presumably error; whether any caller guards against this was not verified.
- **Project relocation semantics**: whether moving/renaming a project
  directory (as opposed to moving it as a whole with `.crush` inside it) has
  any explicit reconciliation logic beyond `fsext.LookupClosestBounded`
  (`internal/config/load.go:556-559`) was not exhaustively traced through
  `internal/fsext`.

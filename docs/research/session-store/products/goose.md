# Goose: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of Goose (Block's open-source AI agent,
originally [block/goose](https://github.com/block/goose)) in the
`aaif-goose/goose` fork, at commit
`d17d65f2f30876e391395e1e8b5f2666320588c6` (committed 2026-07-23, working tree
clean). Every citation below was re-verified against this exact commit on
2026-07-23. Citations use repo-relative `path:line` shorthand; unless another
crate is named, paths are under `crates/goose/src/`.

Goose is a Rust agent that runs the model itself (via pluggable providers) and
persists all session state locally. Since schema v10+ the durable session store
is a **single SQLite database** (`sessions.db`), replacing an earlier
per-session JSONL layout that is now only read for one-time import. The store
lives in `session/session_manager.rs` (4238 lines: the `SessionStorage` type,
its schema, migrations, and every read/write call site), with satellites in
`session/chat_history_search.rs` (keyword search), `session/legacy.rs` (JSONL
import), `session/import_formats/` (foreign-format import), and
`session/extension_data.rs`.

## The storage model

The durable session is a **mutable row plus an ordered set of message rows in
SQLite**, not an append-only event log. The database is one file per install:
`{data_dir}/sessions/sessions.db` (`session_manager.rs:27-28`, `859-867`;
`data_dir` on macOS is `~/Library/Application Support/Block/goose`,
`config/paths.rs:19-37`, `52-54`). The pool opens in WAL mode with foreign keys
on and a 30s busy timeout (`session_manager.rs:849-856`).

The source of truth is three tables (`create_schema`,
`session_manager.rs:924-999`):

- **`sessions`** — one mutable row per session; the session "header" plus
  denormalized rollups (`session_manager.rs:926-956`):

  ```sql
  CREATE TABLE IF NOT EXISTS sessions (
      id TEXT PRIMARY KEY,
      name TEXT NOT NULL DEFAULT '',
      description TEXT NOT NULL DEFAULT '',
      user_set_name BOOLEAN DEFAULT FALSE,
      session_type TEXT NOT NULL DEFAULT 'user',
      working_dir TEXT NOT NULL,
      created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
      updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
      extension_data TEXT DEFAULT '{}',
      total_tokens INTEGER, input_tokens INTEGER, output_tokens INTEGER,
      cache_read_tokens INTEGER, cache_write_tokens INTEGER,
      accumulated_total_tokens INTEGER, accumulated_input_tokens INTEGER,
      accumulated_output_tokens INTEGER, accumulated_cache_read_tokens INTEGER,
      accumulated_cache_write_tokens INTEGER, accumulated_cost REAL,
      schedule_id TEXT, recipe_json TEXT, user_recipe_values_json TEXT,
      provider_name TEXT, model_config_json TEXT,
      goose_mode TEXT NOT NULL DEFAULT 'auto',
      archived_at TIMESTAMP, project_id TEXT, parent_session_id TEXT
  )
  ```

- **`messages`** — the transcript, one row per message, insertion-ordered by an
  autoincrement `id` and a `created_timestamp` (`session_manager.rs:962-978`):

  ```sql
  CREATE TABLE IF NOT EXISTS messages (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      message_id TEXT,
      session_id TEXT NOT NULL REFERENCES sessions(id),
      role TEXT NOT NULL,
      content_json TEXT NOT NULL,
      created_timestamp INTEGER NOT NULL,
      timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
      tokens INTEGER,
      metadata_json TEXT
  )
  ```

- **`usage_ledger`** — an **append-only** per-session ledger of token/cost
  deltas, `ON DELETE CASCADE` from `sessions` (`session_manager.rs:980-999`). It
  is the one genuinely append-structured table in the design, used to reconcile
  cumulative usage (including carried-forward rows for subagents).

Authoritative vs. derived:

- **Authoritative**: the `sessions` row and its `messages` rows. Both are
  mutated in place — the session row via `UPDATE` (`apply_update`,
  `session_manager.rs:1590-1725`); the transcript via `INSERT` for a new turn
  (`add_message`, `1765-1798`) but also via bulk `DELETE`+re-`INSERT`
  (`replace_conversation_inner`, `1800-1838`) and `DELETE` (`truncate_*`,
  `2344-2385`). There is no separate event stream from which these are folded.
- **Semi-derived rollups on the authoritative row**: `total_tokens`,
  `accumulated_*`, `accumulated_cost` on `sessions` are running aggregates
  maintained by `record_usage_metrics` alongside the append-only `usage_ledger`
  (`session_manager.rs:2078-2166`); `get_session_usage_totals` recomputes the
  authoritative total from the ledger + row (`2168-2250`).
- **Fully derived / rebuildable**: `message_count` and `last_message_at` are
  computed at read time by `COUNT(*)`/`MAX(created_timestamp)` over `messages`,
  never stored (`get_session`, `session_manager.rs:1574-1585`; list query
  `1906-1908`). `last_message_snippet` is hydrated on demand for list pages
  (`1996-1999`). Keyword search is a live SQL scan, not a maintained index (see
  Listing).
- **Unrelated side tables**: `provider_inventory_entries` /
  `provider_inventory_models` (`providers/inventory/mod.rs:1145-1165`) live in
  the same DB but hold provider/model inventory, not session data. Migration 10
  also created `threads`/`thread_messages` tables (`session_manager.rs:1348-1380`)
  that are dormant in the current session path.

Best-fit conceptual model: **session-as-row + session-as-transcript** (a mutable
document with an ordered, in-place-editable message list). It is explicitly
**not** session-as-log: retroactive operations rewrite or delete rows rather
than appending interpreted markers.

## Keying and identity

- **Store scope**: one `sessions.db` for the whole install; all sessions of all
  working directories share it and are separated by columns
  (`session_manager.rs:859-867`). No cwd/project path is encoded into the key.
- **Session id**: a `TEXT PRIMARY KEY` minted server-side (by the store) as
  `YYYYMMDD_N` — today's date followed by a per-day monotonic counter computed
  inside the `INSERT` (`create_session`, `session_manager.rs:1497-1540`):

  ```sql
  INSERT INTO sessions (id, ...) VALUES (
    ? || '_' || CAST(COALESCE((
        SELECT MAX(CAST(SUBSTR(id, 10) AS INTEGER))
        FROM sessions WHERE id LIKE ? || '_%'
    ), 0) + 1 AS TEXT), ...)
  ```

  So the id encodes **date + ordinal**, giving coarse chronological ordering but
  no location. It is not a UUID and not client-supplied on create.
- **Message id**: `message_id TEXT`, either carried on the `Message` or minted as
  `msg_{session_id}_{uuid_v4}` at insert (`add_message`, `session_manager.rs:1771-1774`).
  It is indexed but **not UNIQUE** (`idx_messages_message_id`, `1007`), so it is
  a convenience handle, not a dedup key. Intra-second ordering falls back to the
  autoincrement `id` (`get_conversation` `ORDER BY created_timestamp, id`,
  `1727-1733`).
- **Listing scope**: global by default, optionally narrowed by `working_dir`
  equality and/or `session_type` and/or keyword (`SessionListFilters`,
  `session_manager.rs:330-336`; query build `1861-1892`). Cross-"project"
  enumeration is the norm; `project_id` is just a nullable column, not a key
  prefix.
- **Relocation / rename**: identity is the `id`, so moving the working directory
  is reconciled by an `UPDATE sessions SET working_dir = ?` that leaves the id
  and transcript intact (ACP `on_update_working_dir`,
  `acp/server/manage_sessions.rs:29-33`). Title renames are an `UPDATE` of
  `name`/`user_set_name` (`SessionUpdateBuilder`, `session_manager.rs:212-228`).

## The store interface

There is **no pluggable store trait**. `SessionStorage` is a concrete type
wrapping a `sqlx` SQLite `Pool`, and `SessionManager` is a thin public facade
over it (`session_manager.rs:314-646`, `648-652`). The effective interface,
reconstructed from `SessionManager`/`SessionStorage` methods, is:

Session lifecycle:

- `create_session(working_dir, name, session_type, goose_mode) -> Session` —
  mints the `YYYYMMDD_N` id and inserts the row (`session_manager.rs:1497-1540`).
- `get_session(id, include_messages) -> Session` — load the row; optionally load
  and attach the full ordered conversation, else just compute count + last
  timestamp (`1542-1588`).
- `update(id) -> SessionUpdateBuilder` … `.apply()` — partial mutable update of
  any header field; builds a dynamic `UPDATE ... SET` and always bumps
  `updated_at` (`426-432`, `1590-1725`).
- `delete_session(id)` — existence check, then `DELETE messages`, `DELETE
  usage_ledger`, `DELETE sessions` in one tx (`2012-2043`).
- `copy_session(id, new_name) -> Session` and `import_session`/`export_session`
  (`2252-2342`).

Transcript I/O:

- `add_message(id, &Message)` — append one message row; bump `updated_at`
  (`1765-1798`).
- `replace_conversation(id, &Conversation)` — destructive full rewrite:
  `DELETE` all message rows for the session, then re-`INSERT` each
  (`1800-1847`).
- `get_conversation(id) -> Conversation` (internal) — ordered read of all rows
  (`1727-1763`).
- `truncate_conversation(id, timestamp)` — `DELETE ... created_timestamp >= ?`
  (`2344-2353`).
- `truncate_conversation_from_message(id, message_id)` — resolve the boundary
  row, then delete it and everything after (`2355-2385`).
- `update_message_metadata(id, message_id, f)` — read/modify/write a message's
  `metadata_json` (`2412-2452`).
- `update_tool_request_meta(id, message_id, tool_call_id, patch)` — in-place
  merge into a `ToolRequest.tool_meta` inside a stored message's `content_json`
  (`2459-2509`).

Listing / analytics / search:

- `list_sessions()` / `list_sessions_by_types(types)` / `list_all_sessions()`
  (`442-459`, `1952-2010`).
- `list_sessions_paged(SessionListPageQuery)` — keyset pagination by
  `(sort_timestamp, id)` returning a `next_cursor` (`450-455`, `1963-2005`).
- `get_insights()` — count + summed tokens over selected types (`2045-2076`).
- `get_session_usage_totals(id)` — recursive parent→child rollup (`2168-2250`).
- `record_usage_metrics(...)` — reconcile rollups + append a `usage_ledger` row
  (`2078-2166`).
- `search_chat_history(query, limit, dates, exclude, types)` — keyword LIKE scan
  (`599-618`, delegating to `chat_history_search.rs`).

Ordering/consistency contract: every mutation runs inside a `BEGIN IMMEDIATE`
transaction (e.g. `1505`, `1715`, `1767`, `1805`, `2014`), which takes SQLite's
write lock immediately so concurrent writers serialize (with the 30s busy
timeout). There is **no expected-version precondition** anywhere; there is no
CAS. The unit of a "read model" is the live SQL query, not a maintained
projection.

There is also a shared-process singleton: `SESSION_STORAGE`
(`LazyLock<Arc<SessionStorage>>`, `session_manager.rs:56-57`) backing
`SessionManager::instance()` (`400-404`), so within one process a single pool
and single lazy-init are reused.

## Write and append path (ordering, durability, concurrency, delivery)

- **New turn commit**: a single `INSERT INTO messages (...)` per message, with a
  sibling `UPDATE sessions SET updated_at = datetime('now')`, both in one
  `BEGIN IMMEDIATE` tx (`add_message`, `session_manager.rs:1765-1798`). This is
  the normal per-turn path; the agent loop calls it repeatedly as it streams
  user, assistant, tool-request, and tool-response messages
  (`agents/agent.rs:1619-2004`).
- **Ordering** is positional: `messages.id INTEGER PRIMARY KEY AUTOINCREMENT`
  gives global insertion order, and `created_timestamp` gives wall-clock order;
  reads sort by `created_timestamp, id` so same-second messages keep insertion
  order (`get_conversation`, `1730-1733`). There is no per-session sequence
  number or stream version.
- **Durability / atomicity**: SQLite WAL + `BEGIN IMMEDIATE` per operation
  (`849-854`, transactions throughout). Multi-statement operations (append,
  replace, delete, migrations, usage reconcile) are atomic within their tx. The
  first-run `create_schema` deliberately uses `BEGIN IMMEDIATE` + `IF NOT
  EXISTS` DDL + `INSERT OR IGNORE` on the version row to survive two processes
  racing to initialize the same file (`893-923` and its comment). There is no
  application-level temp-file-and-rename or torn-write healing beyond SQLite's
  own guarantees.
- **Concurrency**: **single-writer-at-a-time per database**, enforced by
  SQLite's write lock (immediate transactions), not by a per-session lock or an
  in-process command queue. Multiple sessions and multiple processes contend on
  the one DB file; writers block up to the 30s busy timeout.
- **Delivery / idempotence**: **best-effort, no store-level idempotence**.
  `add_message` unconditionally inserts; `message_id` is not unique, so a
  retried append would create a duplicate row. There is no dedup key and no
  at-least-once/exactly-once machinery at the store boundary. (The `usage_ledger`
  reconcile in `record_usage_metrics`, `2089-2124`, guards against double-counting
  *usage totals* via a `MAX(... - ledger_sum, 0)` clause, but that is an
  analytics safeguard, not transcript idempotence.)

## Read and resume path

- **Resume is a full ordered read of the durable store**, not a cursor replay or
  a cached view. `get_session(id, include_messages=true)` runs `SELECT role,
  content_json, created_timestamp, metadata_json, message_id FROM messages WHERE
  session_id = ? ORDER BY created_timestamp, id` and rebuilds a `Conversation`
  from every row (`get_conversation`, `session_manager.rs:1727-1763`). There is
  no local filesystem cache consulted first; the SQLite file *is* the store.
- **ACP load** (`acp/server/load_session.rs:203-262`) calls `get_session(id,
  true)`, then `replay_conversation_to_client` streams each stored message back
  to the client as ACP `SessionUpdate` chunks, filtered to
  `user_visible_messages()` (`load_session.rs:103-200`). Message provenance
  (`created`, `messageId`, `steer`) is re-emitted under `_meta.goose`
  (`load_session.rs:17-45`).
- **Agent context vs. UI view**: the model's context window is the
  `agent_visible` subset of the same rows, while the UI shows the `user_visible`
  subset (`MessageMetadata.user_visible` / `agent_visible`,
  `goose-provider-types/src/conversation/message.rs:661-675`). Both are the same
  durable rows read eagerly; the split is a metadata filter, not a second store.
- **Pagination / bounds**: the transcript read is unbounded (whole session).
  Only *listing* paginates (keyset cursor, below). The legacy JSONL importer caps
  a single legacy file at 50 MiB (`legacy.rs:11`, `42-44`), but the SQLite path
  imposes no per-session size cap.
- **Materialization**: on resume everything is eager — the full conversation is
  loaded into memory as a `Conversation`. `last_message_snippet` and per-message
  usage are the only things hydrated lazily/optionally.

## Listing, summaries, and search

- **Enumeration** is a single grouped SQL query over `sessions LEFT JOIN
  messages`, grouping by `s.id`, computing `message_count` and
  `last_message_timestamp`, and ordering by a `sort_timestamp = COALESCE(MAX(msg
  ts), updated_at)` then `id` (`list_sessions_matching`,
  `session_manager.rs:1849-1950`). Pagination is **keyset**: the cursor is
  `(sort_at, session_id)` and the query fetches `page_size + 1` rows to derive
  `next_cursor` (`list_sessions_paged`, `1963-2005`; cursor type `318-322`).
  Supporting indexes: `idx_sessions_updated`, `idx_sessions_type`,
  `idx_sessions_parent`, `idx_messages_session` (`1001-1020`). No scale numbers
  are quoted in-source; cost is a per-page grouped join, so it scales with rows
  matched, not total history.
- **Summaries / read model**: there is **no maintained sidecar**. Picker fields
  are recomputed each query: counts and last-activity via aggregates, and
  `last_message_snippet` hydrated on demand for the returned page
  (`last_message_snippet::hydrate_last_message_snippets`, `1996-1999`). The only
  denormalized-at-write values are the token rollups on the `sessions` row.
- **Search** is a **live keyword LIKE scan**, not FTS or vectors. Two variants:
  the list filter builds `instr(LOWER(json_extract(value,'$.text')), ?) > 0`
  clauses over `json_each(content_json)` (`message_keyword_clause`,
  `session_manager.rs:361-382`); `ChatHistorySearch` builds `LOWER(json_extract(
  content.value,'$.text')) LIKE ?` clauses and additionally respects
  `metadata_json.$.agentVisible` and per-content `annotations.audience` so hidden
  text is not recalled (`chat_history_search.rs:133-206`). Nothing is
  bootstrapped or kept in sync — the scan reads current rows every time.

## Entry/message structure and versioning

- **Stored entry** = one `messages` row. The store parses and interprets it
  partially: `role` and `created_timestamp` are first-class columns used for
  ordering and filtering; `content_json` and `metadata_json` are JSON blobs that
  the store nonetheless reaches *into* via `json_extract`/`json_each` for search
  and for the `update_tool_request_meta` patch (`session_manager.rs:2459-2509`).
  So it is neither fully opaque nor a normalized schema — JSON columns with
  targeted introspection.
- **Message type** (`goose-provider-types/src/conversation/message.rs:763-770`):

  ```rust
  pub struct Message {
      pub id: Option<String>,
      pub role: Role,               // User | Assistant
      pub created: i64,             // unix seconds (ms tolerated on read)
      pub content: Vec<MessageContent>,
      pub metadata: MessageMetadata,
  }
  ```

  `content` is a tagged union (`#[serde(tag = "type")]`) over `Text`, `Image`,
  `ToolRequest`, `ToolResponse`, `ToolConfirmationRequest`, `ActionRequired`,
  `FrontendToolRequest`, `Thinking`, `RedactedThinking`, `SystemNotification`
  (`message.rs:205-219`). `MessageMetadata` carries the visibility flags plus
  `steer`, `inference`, and an optional `usage` box (`message.rs:661-675`).
  `ToolRequest` carries `id`, the tool call, provider `metadata`, and a
  `tool_meta` bag used for LLM-generated titles/chain summaries that must survive
  reload (`message.rs:80-114`).
- **Chaining/threading**: there is no per-message parent pointer. Tool
  request/response pair by shared `id` (`ToolRequest.id` == `ToolResponse.id`).
  Ordering is entirely positional (`created_timestamp`, autoincrement `id`).
  Session-to-session lineage is the `parent_session_id` column, not a
  message-level link.
- **Timestamp normalization**: reads tolerate both second and millisecond epochs
  via a `MILLISECOND_TIMESTAMP_THRESHOLD` guard (`session_manager.rs:29`,
  `661-674`).
- **Format evolution** is handled two ways. (1) **Content-level migration at
  deserialization**: `deserialize_sanitized_content` drops pre-14.0
  `conversationCompacted` blocks, rewrites legacy `reasoning` → `thinking`, and
  sanitizes Unicode-tag exploits every time a message is read
  (`message.rs:22-71`). Most new fields are additive with serde defaults
  (`#[serde(default)]` across `Session` and `MessageMetadata`). (2)
  **Schema-level migration**: `CURRENT_SCHEMA_VERSION = 15`
  (`session_manager.rs:26`), tracked in a `schema_version` table; `run_migrations`
  applies each `v(current+1..=15)` in order inside one tx and records it
  (`1146-1203`), with `apply_migration` holding the DDL for each step
  (`1206-1495`). Migrations are **forward-only** (no down migrations; an unknown
  version `bail!`s, `1489-1491`) and defensively idempotent (each checks
  `pragma_table_info` before `ALTER`). A brand-new DB is created at v15 directly
  via `create_schema` (`893-1032`).

## Compaction and history management

- Compaction is an **upstream (agent-loop) concern that rewrites the durable
  transcript**, but non-destructively at the message level. When the context
  crosses the auto-compact threshold, `compact_messages` produces a new message
  list where the **original messages are retained but flipped to
  `agent_invisible`** (still `user_visible`), a summary message is appended as
  `agent_only`, plus a continuation message and optionally the preserved last
  user message (`context_mgmt/mod.rs:76-186`, esp. `139-173`). The agent loop
  then calls `session_manager.replace_conversation(...)`, i.e. `DELETE` all rows
  and re-`INSERT` the new list (`agents/agent.rs:1812-1826`;
  `replace_conversation_inner`, `session_manager.rs:1800-1838`).
- **Net effect on the durable record**: the full pre-compaction transcript
  survives on disk as `user_visible`/`agent_invisible` rows, so the human history
  is preserved and re-shown on resume, while the model only re-reads the summary
  + tail. This is a **soft, view-shrinking compaction implemented by rewriting
  the whole message set with new visibility flags** — not an appended marker
  interpreted at replay, and not a hard deletion. Usage of the summarization call
  is charged and flagged `is_compaction` in the `usage_ledger`
  (`message.rs:636-637`; `session_manager.rs:837`).
- **Resume across a compaction boundary**: because compaction has already
  rewritten the rows, resume just reads the current rows; the model context is
  reconstituted as the `agent_visible` subset (summary + retained tail), the UI
  shows the full `user_visible` set.

## Rewind, checkpoints, and fork

- **Rewind / undo is a destructive delete, not an appended marker.** ACP fork and
  edit paths call `truncate_conversation(session_id, timestamp)` (delete rows at
  or after a cutoff, `session_manager.rs:2344-2353`) or
  `truncate_conversation_from_message(session_id, message_id)` (resolve the
  boundary row, delete it and everything after in one tx, `2355-2385`). The
  removed turns are gone from the durable store.
- **No file-state or environment checkpoints.** There is no per-turn snapshot of
  the working tree, no git-ref checkpoint, and no diff/hash store. The only
  per-turn artifact is the `usage_ledger` row. (Contrast T3 Code's
  content-addressed git checkpoints.)
- **Fork is copy-plus-truncate into a brand-new session (physical copy, not a
  shared prefix).** ACP `handle_fork_session` calls `copy_session(source, "{name}
  (copy)")` then optionally `truncate_conversation(new, conversationBefore)`
  (`acp/server/fork_session.rs:25-37`). `copy_session` mints a fresh
  `YYYYMMDD_N` id, copies header fields (extension_data, recipe, provider,
  model, mode, project_id), and **re-inserts every source message into the new
  session** via `replace_conversation` (`session_manager.rs:2299-2342`). Lineage
  recorded: the copy is a wholly independent session; the fork does **not** set
  `parent_session_id` (that column is reserved for subagents), so the only link
  is the `" (copy)"` name convention. Cost is O(history) per fork.

## Subagents and nested sessions

- A subagent is a **first-class sibling session**, stored exactly like a
  top-level session in the same DB, not nested storage and not entries in the
  parent transcript. `create_subagent_session` calls `create_session(...,
  SessionType::SubAgent, ...)` with the parent's working dir, then sets
  `parent_session_id` to the parent's id (`agents/platform_extensions/summon.rs:503-531`).
- **Durable parent→child link**: the `parent_session_id` column (added in
  migration 15, indexed by `idx_sessions_parent`, `session_manager.rs:1445-1461`).
  The child has its **own isolated transcript** (its own `messages` rows); it
  does not inherit or share the parent's message rows.
- **Rollup traversal**: usage totals walk the tree with a recursive CTE
  (`WITH RECURSIVE tree ... JOIN tree ON s.parent_session_id = tree.id`) so a
  parent's totals include descendant subagents (`get_session_usage_totals`,
  `session_manager.rs:2186-2201`). This is the one place the parent/child link is
  read structurally.
- **Bounds / cascade**: nesting is not bounded by the schema (arbitrary depth via
  the column). On parent delete there is **no cascade to children**:
  `delete_session` deletes only that session's own messages, ledger, and row
  (`2012-2043`), leaving a subagent row with a dangling `parent_session_id`
  (orphan). Subagent listing is normally filtered out of the default picker,
  which lists only `User`/`Scheduled` types (`list_sessions`, `2007-2010`).

## Retention, deletion, and multi-host

- **Retention**: none. No TTL, lifecycle sweep, or scheduled cleanup of sessions,
  messages, or the usage ledger was found. History is kept indefinitely.
  `archived_at` exists as a soft-archive timestamp column
  (`session_manager.rs:953`, `298-301`) but there is no retention policy acting
  on it.
- **Delete** is a hard, immediate, single-session cascade *within* that session:
  `DELETE FROM messages`, `DELETE FROM usage_ledger` (also `ON DELETE CASCADE`,
  `984`), `DELETE FROM sessions`, in one `BEGIN IMMEDIATE` tx after an existence
  check (`delete_session`, `2012-2043`). It does **not** cascade to subagent
  child sessions (see above). Because messages live in the same DB, there is no
  remote-first or append-only no-op consideration.
- **Multi-host**: not a first-class path. The design assumes a single local
  SQLite file; concurrency safety is SQLite write-locking + a 30s busy timeout +
  WAL, and the explicit multi-process concern addressed in-source is only
  *first-run schema-init races on a shared file* (`create_schema` comment,
  `893-905`). There is no network-filesystem handling, no crash detection, and no
  remote writeback. Cross-host *sharing* exists but only as an export mechanism
  (see below), not as a shared store.

## Interop with foreign session stores

Goose both **imports foreign transcripts** and **exports/shares its own**, but
never treats a foreign store as a live backend.

- **Foreign import**: `session/import_formats/` converts other agents' on-disk
  session files into goose-native `Session` JSON, then feeds them through the
  normal `import_session` pipeline (create + `replace_conversation`). Supported:
  **Claude Code** (`.jsonl` under `~/.claude/projects/...`), **Codex** (`.jsonl`
  rollouts under `~/.codex/sessions/YYYY/MM/DD/...`), and **Pi** (`.jsonl` under
  `~/.pi/agent/sessions/...`) (`import_formats/mod.rs:1-24`;
  `claude_code.rs`, `codex.rs`, `pi.rs`). Import is a one-way, converting copy
  into goose's SQLite — read-only against the foreign file, and the result is a
  normal goose session.
- **Legacy self-import**: on first initialization of a fresh DB, goose scans the
  old per-session `*.jsonl` layout in the sessions folder and imports each into
  SQLite (`import_legacy`, `session_manager.rs:1034-1144`; JSONL reader
  `legacy.rs:13-108`, first line = metadata object, subsequent lines = messages).
  This is the migration from the pre-v10 JSONL store to the SQLite store.
- **Export / share**: `export_session` serializes a `Session` (with conversation)
  to pretty JSON (`session_manager.rs:2252-2255`); the optional `nostr`-gated
  `nostr_share` module publishes an encrypted session as a Nostr event (kind
  30278) with a deeplink for out-of-band sharing (`session/nostr_share.rs:1-40`).
  These are copies out, not a shared durable store.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* Goose is the corpus's clearest
**non-event-sourced** local store: a session is a **mutable SQLite row plus an
in-place-editable ordered message table**, where resume is a full ordered
`SELECT`, listing/summaries/search are **live queries** (nothing maintained), and
every retroactive operation (compaction, rewind, fork) works by **rewriting or
deleting rows** rather than appending interpreted markers. This is close to the
opposite of our event-sourced target, and it is instructive precisely as a
contrast:

- **Where it validates our direction**: the pain points our design avoids are
  visible here. Compaction and fork both go through a `DELETE`-all +
  re-`INSERT` (`replace_conversation`, `session_manager.rs:1800-1838`), so a
  crash mid-rewrite risks a torn transcript, fork is O(history) copy, and rewind
  destroys the pre-cutoff turns (`truncate_*`, `2344-2385`). An append-only log
  with derived projections gets crash-safe rewind, cheap shared-prefix forks, and
  intact history for free.
- **Ideas worth carrying over**: (1) The **`agent_visible`/`user_visible`
  visibility split** (`message.rs:661-675`) is a clean way to shrink the
  model-visible view while keeping the human transcript — our log can express the
  same with a projection filter instead of a metadata flag on a rewritten row.
  (2) The **append-only `usage_ledger` beside the mutable row** shows they already
  reach for an append log exactly where correctness of cumulative state matters;
  our design generalizes that to the whole session. (3) **Recomputing
  count/last-activity/snippet at query time** (`get_session`, list query) is a
  reminder that not every list field needs a maintained sidecar — cheap
  projections can stay lazy.
- **Cautions our design should heed**: (1) **No store-level idempotence** —
  `message_id` is non-unique and `add_message` blind-inserts (`1765-1798`); we
  want a dedup/idempotency key on append. (2) **No expected-version / OCC** —
  concurrency is only SQLite's write lock; the moment writers aren't one local
  process, that guarantee evaporates, so we need an explicit expected-position
  precondition. (3) **Fork/subagent lineage is thin** — fork records no
  `parent_session_id` at all and delete does not cascade to subagent children,
  leaving orphans (`2012-2043`); our lineage metadata and cascade/retention rules
  must be deliberate.

## Open questions

- **Concurrent transcript writes to one session**: two processes appending to the
  same `session_id` serialize on the SQLite write lock, but nothing orders their
  interleave or dedups a retried append. The intended single-writer-per-session
  discipline (if any) above the store was not pinned down.
- **`threads`/`thread_messages` tables** (migration 10, `session_manager.rs:1348-1380`)
  and the `thread_id` column appear to be a dormant or superseded feature; their
  current role, if any, in the session path was not traced.
- **`archived_at` lifecycle**: the column and builder exist
  (`298-301`, `953`) and list queries can carry it, but no code path that sets it
  or filters the default picker by it was located; whether archive is surfaced in
  a UI or is vestigial is unresolved.
- **Subagent orphan cleanup**: nothing reconciles a subagent whose parent was
  deleted; whether an out-of-band sweep exists (outside the session module) was
  not found.
- **Migration-8/ledger interaction on old DBs**: migration 15 (re)creates
  `usage_ledger` `IF NOT EXISTS` (`1463-1487`) while `create_schema` also creates
  it; the exact state of the ledger for DBs that upgraded through intermediate
  versions before v15 was not exercised.

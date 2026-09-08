# Amazon Q Developer CLI: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against a local clone of the Amazon Q Developer CLI (crate name `chat-cli`,
binary `q`; package repo root contains `crates/chat-cli`, `crates/agent`,
`crates/chat-cli-ui`, and several `amzn-*-client` crates) pinned at commit
`15cc8f3cd18c4272925ce1c7053268eedff1ea0a` ("Update README to include issue
reporting link (#3775)", 2026-04-23). Authoritative anchors:
`crates/chat-cli/src/database/mod.rs` (the single SQLite store and its
migration ratchet), `crates/chat-cli/src/database/sqlite_migrations/*.sql`
(the eight-step migration sequence), `crates/chat-cli/src/cli/chat/conversation.rs`
(`ConversationState`, the durable session record itself),
`crates/chat-cli/src/cli/chat/checkpoint.rs` (the shadow-git checkpoint
mechanism), `crates/chat-cli/src/cli/chat/message.rs` (entry/message types),
`crates/chat-cli/src/cli/chat/tools/delegate.rs` (the "Delegate" subagent
tool), and `crates/chat-cli/src/util/paths.rs` (on-disk path layout). The
workspace root `Cargo.toml:12` declares `license = "MIT OR Apache-2.0"`
(dual-licensed, not Apache-2.0-only; `crates/chat-cli/Cargo.toml:8` inherits
this via `license.workspace = true`), confirmed against the checked-in
`LICENSE.APACHE` and `LICENSE.MIT` files at the repo root.

The headline finding is confirmed, with one important refinement: the
durable chat session is a **single mutable JSON blob keyed by the absolute
working-directory path**, overwritten in full on every assistant turn, with
no append-only log anywhere in the content path. The refinement is that this
degenerate design is not merely passive (no history to retain) -- the code
actively, repeatedly, and irreversibly *destroys* history in place: a
10,000-entry soft cap enforced by draining the in-memory `VecDeque` before
every save, and a separate `/compact` command that drains history and
replaces it with an AI-generated summary, both of which are persisted back to
the single row on the very next turn. Nothing resembling Zed's in-place
compaction marker or fx's `compacted_summary` turn-with-full-history-retained
exists here.

## The storage model

The database module's own doc comments state the shape directly:

```rust
#[derive(Debug)]
pub enum Table {
    /// The state table contains persistent application state.
    State,
    /// The conversations tables contains user chat conversations.
    Conversations,
    /// The auth table contains SSO and Builder ID credentials.
    Auth,
}
```
(`crates/chat-cli/src/database/mod.rs:157-165`)

All three tables are simple two-column SQLite key/value tables:

- `conversations (key TEXT PRIMARY KEY, value TEXT)` --
  `crates/chat-cli/src/database/sqlite_migrations/007_conversations_table.sql:1-4`.
- `state (key TEXT PRIMARY KEY, value BLOB)` -- originally `value TEXT`
  (`004_state_table.sql:1-4`), migrated to `BLOB` in
  `006_make_state_blob.sql:1-6` (rename-old-table, recreate, copy, drop --
  SQLite's standard workaround for `ALTER COLUMN TYPE`, since SQLite has no
  native column-type-change statement).
- `auth_kv (key TEXT PRIMARY KEY, value TEXT)` --
  `005_auth_table.sql:3-6`, whose file comment states the reason for a
  separate table: "We create a separate auth_kv to ensure the data is not
  available in all the same places that the state is available in"
  (`005_auth_table.sql:1-2`). This is corroborated by `q settings`
  (`crates/chat-cli/src/cli/settings.rs:101`) calling
  `Database::get_all_entries` (`database/mod.rs:234-236`), which dumps the
  entire `state` table verbatim -- `auth_kv` needed to be a separate table
  precisely so that a config/settings dump could never leak credentials.

The one and only chat-session accessor pair is:

```rust
pub fn get_conversation_by_path(&mut self, path: impl AsRef<Path>)
    -> Result<Option<ConversationState>, DatabaseError> { ... }
pub fn set_conversation_by_path(&mut self, path: impl AsRef<Path>, state: &ConversationState)
    -> Result<usize, DatabaseError> { ... }
```
(`crates/chat-cli/src/database/mod.rs:385-411`)

`set_conversation_by_path` serializes the entire `ConversationState` to a
JSON string (`self.set_json_entry(Table::Conversations, path, state)`,
`crates/chat-cli/src/database/mod.rs:410`) and issues `INSERT OR REPLACE INTO conversations (key, value)
VALUES (?1, ?2)` (`crates/chat-cli/src/database/mod.rs:470-475`, the shared `set_entry` primitive used by
every table). There is no partial update, no append, and no versioned/CAS
write anywhere in this path -- every save is a full-document upsert keyed on
the caller-supplied path string, which is always `std::env::current_dir()`
at the two call sites (`crates/chat-cli/src/cli/chat/conversation.rs:420-421`,
`crates/chat-cli/src/cli/chat/mod.rs:720-723`).

Authoritative vs. derived: there is no derived layer at all. The row *is*
the session; there is no separate summary/index/cache table for
conversations (contrast `state`, which the `q settings` dump treats as a
flat, fully-authoritative key/value bag too -- nothing in this product is a
rebuildable projection of something else). The conceptual model is
**session-as-row**, specifically a mutable document store with exactly one
row per distinct working directory, not session-as-log and not
session-as-directory.

## Keying and identity

- The primary key of the `conversations` table is the **literal, absolute
  working-directory path string**, not a session id.
  `get_conversation_by_path`/`set_conversation_by_path` both take
  `impl AsRef<Path>`, convert to `&str` via `.to_str()` (returning `Ok(None)`
  / `Ok(0)` silently for non-UTF-8 paths -- `database/mod.rs:390-393,405-408`),
  and use that string directly as the SQL key. There is no hashing, no
  normalization, and no canonicalization visible in this path (e.g. no
  symlink resolution) -- two different string spellings of the same
  directory (a symlink vs. its target, or a trailing slash) would be
  different keys, though we did not find a test exercising this.
- `ConversationState.conversation_id: String` (`conversation.rs:109`) is a
  separate field carried *inside* the JSON blob. It is minted as
  `uuid::Uuid::new_v4().to_string()` unconditionally on every `q chat`
  process launch (`crates/chat-cli/src/cli/chat/mod.rs:301-302`, "Generated
  new conversation id"). When resuming succeeds, this freshly-minted id is
  simply discarded in favor of the loaded row's own `conversation_id` -- the
  `Some(mut cs)` branch at `crates/chat-cli/src/cli/chat/mod.rs:728-751` never touches `cs.conversation_id`,
  it only overwrites `tool_manager`, `agents`, `mcp_enabled`, and re-derives
  `context_manager`/tool invariants. So: the working-directory path is the
  *addressing* key (one row per directory), while `conversation_id` is an
  internal identity field with no addressing role of its own -- it is UUIDv4
  (random), not UUIDv7 or otherwise ordering-encoding.
- **Relocation/renames are not reconciled at all.** Because the key is the
  literal path string, moving or renaming the working directory produces a
  cache miss on `get_conversation_by_path` -- the CLI simply falls through to
  `ConversationState::new(...)` and starts a fresh conversation
  (`crates/chat-cli/src/cli/chat/mod.rs:752-764`). We found no migration, symlink-following, or
  path-history mechanism anywhere in `database/mod.rs` or `conversation.rs`.
  This is the flip side of Zed's `PathList`-based reconciliation
  (`../zed/index.md`): Amazon Q has no reconciliation layer at all, by
  omission rather than by a deliberate no-op decision we could find recorded.
- **Listing is not a supported operation.** There is no method on `Database`
  that enumerates the `conversations` table -- `all_entries` exists
  (`crates/chat-cli/src/database/mod.rs:504-520`) but is only ever called with `Table::State`
  (`crates/chat-cli/src/database/mod.rs:235`, via `get_all_entries`). A repo-wide grep for
  `Table::Conversations` turns up exactly three lines, all in
  `database/mod.rs` itself (the `Display` impl at `:171-172` and the two
  accessor bodies at `:395,410`) -- there is no third call site anywhere that
  lists, searches, or enumerates saved conversations across directories. One
  directory can hold at most one saved conversation, and there is no product
  surface (CLI subcommand or otherwise) for discovering what other
  directories have one.
- Resume is a boolean switch tied to the *directory you are currently in*,
  not to a chosen session id: `ChatArgs.resume: bool`
  (`crates/chat-cli/src/cli/chat/mod.rs:233`, "Resumes the previous
  conversation from this directory") is the only lever; there is no
  `--session <id>` flag or a session picker anywhere in `ChatArgs`
  (`crates/chat-cli/src/cli/chat/mod.rs:227-253`).

## The store interface

There is no pluggable store trait -- `Database` (`database/mod.rs:184-187`,
wrapping an `r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>`) is a single
concrete struct with ad hoc methods, reconstructed below.

| Operation | Signature / entry point | Notes |
| --- | --- | --- |
| Open/migrate | `Database::new()` (`database/mod.rs:190-231`) | Opens `data.sqlite3`, creates parent dir, chmods to `0600` on Unix (`:212-223`), then runs `migrate()`. In `cfg!(test)` (non-integration) builds, opens an in-memory DB instead (`:191-197`). |
| Load (get) | `get_conversation_by_path(&mut self, path)` (`:385-396`) | Single-row `SELECT`, JSON-deserialize; `Ok(None)` on miss or non-UTF-8 path. |
| Save (full overwrite) | `set_conversation_by_path(&mut self, path, state: &ConversationState)` (`:399-411`) | `INSERT OR REPLACE`, full JSON blob, no partial update. |
| Generic get/set (shared primitive) | `get_entry<T: FromSql>`/`set_entry` (`:460-475`) | Used by all three tables; `set_entry` is `INSERT OR REPLACE INTO {table} (key, value) VALUES (?1, ?2)`. |
| Generic get/set JSON (shared primitive) | `get_json_entry`/`set_json_entry` (`:477-495`) | Wraps the above with `serde_json::to_string`/`from_str`. |
| Delete | `delete_entry(&self, table, key)` (`:497-502`) | `DELETE FROM {table} WHERE key = ?1`; exists generically but is **never called with `Table::Conversations`** anywhere in the crate (grep-confirmed) -- there is no delete-a-conversation operation exposed. |
| Enumerate (state only) | `all_entries(&self, table)` / `get_all_entries()` (`:504-520`, `:234-236`) | Full-table scan; used only for `Table::State` (the `q settings` dump), never for `Table::Conversations`. |
| Secrets | `get_secret`/`set_secret`/`delete_secret` (`:413-427`) | Thin wrappers over `get_entry`/`set_entry`/`delete_entry` against `Table::Auth`. |
| Migrate | `Database::migrate(self)` (`:431-458`) | Runs pending steps from the `MIGRATIONS` const inside one `rusqlite::Transaction`, committed once at the end (`:432-455`). |

There is no read/write interface at all for the checkpoint or delegate
subsystems through `Database` -- those are separate, file-based stores
described under Rewind/Subagents below.

## Write and append path (ordering, durability, concurrency, delivery)

- **Unit of write is the whole conversation.** `ConversationState::
  push_assistant_message` (`conversation.rs:403-423`) appends one
  `HistoryEntry { user, assistant, request_metadata }` to the in-memory
  `VecDeque<HistoryEntry>` and, in the same call, immediately persists: `if
  let Ok(cwd) = std::env::current_dir() { os.database.
  set_conversation_by_path(cwd, self).ok(); }` (`:420-421`). Every turn is a
  full JSON re-serialization of the entire `ConversationState` (transcript,
  history, context manager, checkpoint manager, tangent state, everything --
  see Entry structure below), not an append of the one new entry.
- **Ordering** is whatever order `VecDeque::push_back` gives an in-process
  `Vec`-like structure -- there is no sequence number, no timestamp-based
  ordering key, and no expected-version precondition on the write. The
  `.ok()` on the save call (`:421`) means a failed write is silently
  swallowed; the in-memory turn is never rolled back and the user is never
  told persistence failed.
- **Durability.** `Database::new` opens the SQLite file with
  `r2d2_sqlite::SqliteConnectionManager::file(&path)` (`database/mod.rs:209`)
  with no `PRAGMA` statements anywhere in the crate -- a repo-wide grep for
  `journal_mode`, `PRAGMA`, `busy_timeout`, and `WAL` across
  `crates/chat-cli/src` returns zero hits. This means the connection runs
  under SQLite's compiled-in defaults: rollback-journal mode (not WAL) and
  `synchronous=FULL` (durable per-statement fsync on commit), but also a
  **default `busy_timeout` of 0** -- a second connection hitting a locked
  database fails immediately with `SQLITE_BUSY` rather than waiting (this is
  inference from SQLite's documented defaults combined with the absence of
  any override in this codebase, not a claim we verified by forcing a lock
  in this environment). There is no temp-file-and-rename pattern anywhere in
  the conversation write path -- the durability story is entirely "one SQLite
  `UPDATE`/`INSERT` statement, implicitly its own transaction." The only
  place an explicit `rusqlite::Transaction` appears in this crate is
  `Database::migrate` (`:432-455`).
- **Concurrency.** `Database` is `#[derive(Clone, Debug)]` (`:183`) with a
  cloneable `r2d2::Pool` inside, so multiple async tasks or threads inside
  one process share the pool and its underlying file lock. Across
  *processes* (see Subagents below -- the Delegate tool spawns a second `q
  chat` process against the same database file and, critically, the same
  working directory), there is no application-level optimistic-concurrency
  check of any kind: two processes racing to call `set_conversation_by_path`
  for the same `cwd` key produce ordinary last-write-wins `INSERT OR REPLACE`
  semantics, gated only by SQLite's own file locking (and, per the point
  above, a zero busy-timeout that can surface as an outright error rather
  than a wait).
- **Delivery semantics** are at-most-once, best-effort: the save call's
  `Result` is discarded with `.ok()` (`conversation.rs:421`), so there is no
  retry, no queue, and no idempotence key -- there is nothing to be idempotent
  *about*, since the unit of write is the full document, not a discrete
  appended entry.

## Read and resume path

- Resume is a single-row, single-shot `SELECT` plus JSON deserialize:
  `get_conversation_by_path` (`database/mod.rs:385-396`) is called once at
  session startup (`crates/chat-cli/src/cli/chat/mod.rs:720-723`), gated by
  the `resume: bool` CLI flag being set and by the loaded conversation
  actually having non-empty history (`crates/chat-cli/src/cli/chat/mod.rs:727`, `previous_conversation
  .filter(|cs| !cs.history().is_empty())` -- this guard exists specifically
  "to prevent edge case where user clears conversation then exits without
  chatting," per the adjacent comment at `crates/chat-cli/src/cli/chat/mod.rs:725-726`).
- There is no cursor, no incremental read, and no entry-level pagination --
  the entire `ConversationState` blob, including the full `history`
  `VecDeque` and the full `transcript` `VecDeque`, is materialized eagerly
  in one deserialization call. There is no lazy-loaded portion of a
  conversation in this product; it is loaded whole or not at all.
- After a successful load, several fields that were explicitly *not*
  serialized are reconstructed and reattached rather than read back:
  `cs.tool_manager = tool_manager;` and `cs.agents = agents;`
  (`crates/chat-cli/src/cli/chat/mod.rs:731,746`) -- both fields carry `#[serde(skip)]` on the struct
  definition (`conversation.rs:125-126,131-132`), so the persisted JSON never
  contains them; they must be freshly constructed on every process start and
  spliced into the resumed state. `cs.update_state(true).await` and
  `cs.enforce_tool_use_history_invariants()` (`crates/chat-cli/src/cli/chat/mod.rs:748-749`) then run a
  bounds-check pass immediately after load, before the first new user
  message is processed.
- On successful resume, the CLI synthesizes an implicit new turn rather than
  just silently continuing: `input = Some(input.unwrap_or("In a few words,
  summarize our conversation so far.".to_owned()))` (`crates/chat-cli/src/cli/chat/mod.rs:730`) -- i.e. the
  *default* resume behavior, absent an explicit prompt, is to ask the model
  to re-summarize the just-loaded history back to the user.

## Listing, summaries, and search

There is effectively nothing here to document beyond "absent." As
established under Keying and identity: no enumeration method exists over
`Table::Conversations`, no metadata sidecar is written alongside a
conversation row, and no search index -- full-text, fuzzy, or otherwise -- was
found anywhere in `crates/chat-cli/src` for chat history content. The only
thing resembling a "listing" surface in this whole crate is `q settings`
dumping the unrelated `state` key/value table (`cli/settings.rs:101`) and
`status_all_agents` (see Subagents below), which lists *delegate task*
status files from a workspace directory, not saved conversations.

## Entry/message structure and versioning

`ConversationState` (`crates/chat-cli/src/cli/chat/conversation.rs:106-152`)
is the entire durable unit. Full field list, in source order:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationState {
    conversation_id: String,
    next_message: Option<UserMessage>,
    history: VecDeque<HistoryEntry>,
    valid_history_range: (usize, usize),
    pub transcript: VecDeque<String>,
    pub tools: HashMap<ToolOrigin, Vec<Tool>>,
    pub context_manager: Option<ContextManager>,
    #[serde(skip)]
    pub tool_manager: ToolManager,
    context_message_length: Option<usize>,
    latest_summary: Option<(String, RequestMetadata)>,
    #[serde(skip)]
    pub agents: Agents,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_info: Option<ModelInfo>,
    #[serde(default)]
    pub file_line_tracker: HashMap<String, FileLineTracker>,
    pub checkpoint_manager: Option<CheckpointManager>,
    #[serde(default = "default_true")]
    pub mcp_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tangent_state: Option<ConversationCheckpoint>,
}
```
(`conversation.rs:106-152`)

Evolution is entirely additive `#[serde(default)]`/`skip_serializing_if`
fields on this one struct -- there is no `version` field on `ConversationState`
itself, no schema-version tag inside the JSON blob, and no migration function
that rewrites an old shape into a new one at load time. The comment on the
`model` field is explicit about this style: "Unused, kept only to maintain
deserialization backwards compatibility with <=v1.13.3" (`:133-134`) -- old
fields are kept around forever, rather than migrated away, purely so that
`serde` can still deserialize rows written by older client versions. This is
a materially different evolution strategy from the SQL-schema ratchet
(`MIGRATIONS` in `database/mod.rs:67-76`), which only ever touches table
*shape* (adding/renaming/dropping columns), never the JSON *payload* shape
inside the `value` column -- the two evolve independently and by different
mechanisms.

Each history entry:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    user: UserMessage,
    assistant: AssistantMessage,
    #[serde(default)]
    request_metadata: Option<RequestMetadata>,
}
```
(`conversation.rs:91-97`)

```rust
pub struct UserMessage {
    pub additional_context: String,
    pub env_context: UserEnvContext,
    pub content: UserMessageContent,
    pub timestamp: Option<DateTime<FixedOffset>>,
    pub images: Option<Vec<ImageBlock>>,
}

pub enum UserMessageContent {
    Prompt { prompt: String },
    CancelledToolUses { prompt: Option<String>, tool_use_results: Vec<ToolUseResult> },
    ToolUseResults { tool_use_results: Vec<ToolUseResult> },
}
```
(`crates/chat-cli/src/cli/chat/message.rs:53-76`)

```rust
pub enum AssistantMessage {
    Response { message_id: Option<String>, content: String },
    ToolUse { message_id: Option<String>, content: String, tool_uses: Vec<AssistantToolUse> },
}
```
(`message.rs:436-448`)

```rust
pub struct ToolUseResult {
    pub tool_use_id: String,
    pub content: Vec<ToolUseResultBlock>,
    pub status: ToolResultStatus,
}
```
(`message.rs:332-339`, further fields truncated by our read window past this
point -- see Open questions)

There is no explicit parent/uuid chaining between entries -- ordering is
purely positional (`VecDeque` index), and identity/dedup for a tool round
trip is by `tool_use_id`/`tool_use_results[].tool_use_id` string matching
inside `enforce_conversation_invariants` (`conversation.rs:1121-1217`, e.g.
the tool-name-repair loop at `:1219-1266`), not by any store-level id. The
entry is opaque to the SQLite store itself -- `Database` never parses
`ConversationState`'s internal fields; only the application layer
(de)serializes it via `serde_json`.

The separate, human-readable `transcript: VecDeque<String>`
(`conversation.rs:120`) is a parallel, denormalized log of the same
conversation in prose form (`> ` prefixed user lines, assistant text plus a
`[Tool uses: ...]` suffix -- `append_user_transcript`/
`append_assistant_transcript`, `conversation.rs:892-901`), capped
independently at `MAX_CONVERSATION_STATE_HISTORY_LEN` entries via
`append_transcript`'s `pop_front`-then-`push_back` (`:903-908`,
`crates/chat-cli/src/cli/chat/consts.rs:6`, `= 10000`). It is stored in the
very same JSON blob as `history`, not a separate sidecar, and is **not** what
gets sent to the backend (`as_sendable_conversation_state` builds its
request from `history`, not `transcript`).

## Compaction and history management

Two independent, both **destructive**, size-management mechanisms exist --
neither leaves the pre-shrink data recoverable in the durable record, which
is the sharpest contrast with every log-based product in this corpus.

1. **The 10,000-entry soft cap**, enforced on every turn before a request is
   even sent. `ConversationState::as_sendable_conversation_state`
   (`conversation.rs:508-537`) calls
   `self.enforce_conversation_invariants()` and then
   `self.history.drain(self.valid_history_range.1..); self.history.drain(..
   self.valid_history_range.0);` (`:515-517`) -- an in-place `VecDeque::drain`
   of the live struct, not a read-only view. The free function
   `enforce_conversation_invariants` (`:1121-1217`) computes the new lower
   bound only once `(history.len() * 2) > MAX_CONVERSATION_STATE_HISTORY_LEN
   - 6` (`:1134`, so effectively once history exceeds ~4997 entries), finding
   "the second oldest message from the user without tool results" as the new
   start (`:1135-1145`) and dropping everything before it. Because
   `push_assistant_message` (which saves via `set_conversation_by_path`) runs
   *after* this drain in the request/response cycle, the very next save
   persists the shrunk history -- the dropped turns are gone from
   `data.sqlite3` as well as from the model-visible window, not just from the
   model-visible window.
2. **`/compact`**, a user- or auto-triggered summarization documented as
   destructive in its own help text: "Clears the conversation history to
   free up space" and "Compaction will be automatically performed whenever
   the context window overflows" (`crates/chat-cli/src/cli/chat/cli/compact.rs:14-30`).
   The implementation, `ConversationState::replace_history_with_summary`:
   ```rust
   pub fn replace_history_with_summary(
       &mut self, summary: String, strategy: CompactStrategy, request_metadata: RequestMetadata,
   ) {
       self.history.drain(..(self.history.len().saturating_sub(strategy.messages_to_exclude)));
       self.latest_summary = Some((summary, request_metadata));
   }
   ```
   (`conversation.rs:732-741`) drains all but the last
   `strategy.messages_to_exclude` entries (default `0`,
   `cli/compact.rs:82-90`) and stores the AI-generated summary string in
   `latest_summary`, which is spliced back into the next backend request as
   context (`conversation.rs:684,811`). The dropped `HistoryEntry` values are
   not written anywhere else first -- no snapshot file, no marker entry kept
   alongside the summary. The next `push_assistant_message` save overwrites
   the one durable row with the now-summarized state.

There is no compaction *marker* stored in the durable record the way Zed
inserts a `Message::Compaction` variant into its message vector while keeping
prior messages, and no separate compacted-turn record the way fx's
`compacted_summary` kind coexists with a `removed_turn_count` while (per our
inference in that dossier) the durable array is not shortened. Here, the
durable array *is* shortened, in place, permanently.

## Rewind, checkpoints, and fork

Two distinct, unrelated mechanisms answer this section; neither is a
durable, replay-based rewind.

**Turn checkpoints (shadow git repo).** `CheckpointManager`
(`crates/chat-cli/src/cli/chat/checkpoint.rs:36-65`) is itself a field of
`ConversationState` (`pub checkpoint_manager: Option<CheckpointManager>`,
`conversation.rs:146`) and is therefore persisted inside the same JSON blob.
Its own doc comment: "Manages a shadow git repository for tracking and
restoring workspace changes" (`crates/chat-cli/src/cli/chat/checkpoint.rs:35`). `manual_init`
(`:103-146`) creates a **bare** git repo at
`~/.aws/amazonq/cli-checkouts/<conversation_id>`
(`get_shadow_repo_dir`, `crates/chat-cli/src/cli/chat/mod.rs:225-227`, using
`PathResolver::new(os).global().shadow_repo_dir()`,
`crates/chat-cli/src/util/paths.rs:279-281,62`) with the real working
directory as its `--work-tree`, then commits an "Initial state" and tags it
`0`. `create_checkpoint` (`crates/chat-cli/src/cli/chat/checkpoint.rs:148-204`) stages, commits (`git
commit --allow-empty --no-verify`), and tags subsequent states with a
`turn.tool` tag scheme (`get_previous_tag`, `:471-489`). Each `Checkpoint`
struct (`:74-82`) additionally embeds a **full clone of the conversation
history at that point** (`history_snapshot: VecDeque<HistoryEntry>`,
`:79`) -- so file-state and conversation-state checkpoints are recorded
together, but the mechanism is genuinely `git`, not the SQLite store: file
content lives in git blobs in the bare repo, and `CheckpointManager::restore`
(`:207-244`) does either `git reset --hard <tag>` or `git checkout <tag> --
.` against the real working tree, then calls
`conversation.restore_to_checkpoint(checkpoint)`
(`conversation.rs:910-922`), which does `self.history =
checkpoint.history_snapshot.clone();` -- a **full, destructive replace** of
the live history, not an append or a marker. The shadow repo is deleted on
`CheckpointManager::drop` (`crates/chat-cli/src/cli/chat/checkpoint.rs:349-364`, spawning an async or
thread-based `remove_dir_all`) and via an explicit `cleanup`
(`:334-339`) -- so file-state checkpoints do not outlive their owning
conversation's checkpoint manager being torn down, in addition to the
`history_snapshot` copies themselves being fully retained (a real, if
heavyweight, form of persistence, since every checkpoint's entire history is
duplicated in memory and thus in the next JSON save).

**Tangent mode.** A separate, lighter in-memory-only branch mechanism:
`enter_tangent_mode` (`conversation.rs:263-267`) snapshots `history`,
`next_message`, `transcript`, and `latest_summary` into a
`ConversationCheckpoint` struct (`:154-167`) stored in the
`tangent_state: Option<ConversationCheckpoint>` field, which **is**
serialized into the durable blob (no `#[serde(skip)]` on it, only
`skip_serializing_if = "Option::is_none"`, `:150`). `exit_tangent_mode`
(`:278-282`) and `exit_tangent_mode_with_tail`
(`:285-303`) restore the pre-tangent state via `restore_from_checkpoint`
(`:250-260`), the latter optionally preserving the tangent's final
entry. Because `tangent_state` is part of the saved JSON, a process crash
while inside tangent mode leaves the *tangent's* history as the live
`history` field with the pre-tangent history preserved, recoverably, inside
`tangent_state.main_history` -- the one place in this product where a prior
state is retained alongside a newer one rather than destroyed, though we
found no explicit crash-recovery code path that automatically re-surfaces an
abandoned `tangent_state` on the next resume; a user would have to run the
exit command again after resuming (see Open questions).

**There is no fork.** No copy-with-new-identity operation, no
shared-prefix/lineage mechanism, and no equivalent of Zed's clipboard
copy-thread or fx's `session recover` was found anywhere in this crate.

## Subagents and nested sessions

Amazon Q's nearest equivalent to a subagent is the experimental **Delegate**
tool (`crates/chat-cli/src/cli/chat/tools/delegate.rs`), gated by
`ExperimentManager::is_enabled(os, ExperimentName::Delegate)` (`:82-84`,
`crates/chat-cli/src/cli/experiment/experiment_manager.rs:16,114-116`). It
is **not** a nested session inside the parent's `ConversationState` and not a
first-class sibling row in `data.sqlite3` -- it is a wholly separate OS
process:

```rust
let mut cmd = tokio::process::Command::new("q");
cmd.args(["chat", "--non-interactive"]);
...
cmd.args(["--agent", agent, task]);
```
(`delegate.rs:341-346`)

Critically, this `Command` never calls `.current_dir(...)` -- the only
`current_dir()` call in the file (`:371`) merely *reads* the parent's cwd to
record it as a display field on `AgentExecution`, not to set the child's
working directory. A spawned `tokio::process::Command` inherits its
parent's working directory by default, so the delegated `q chat` process
runs against the **same** working-directory path, hence -- per Keying and
identity above -- the **same primary key** in the shared `conversations`
table as the parent session, and against the **same** `data.sqlite3` file
(the store is opened via a machine-global, not per-process, path:
`GlobalPaths::database_path_static`, `crates/chat-cli/src/util/paths.rs:327-332`,
`~/.local/share/amazon-q/data.sqlite3` on Linux XDG layout / platform
equivalent via `dirs::data_local_dir()`). Because the delegated process is
launched with `--non-interactive` and no `--resume` flag, it does **not**
load the parent's saved row (`resume_conversation` defaults to `false`,
`ChatArgs::resume` default per `#[derive(Default)]`, `crates/chat-cli/src/cli/chat/mod.rs:227-230`) -- it
starts a brand-new, empty `ConversationState` with its own freshly-minted
`conversation_id`, then, on its own first turn, calls
`set_conversation_by_path(cwd, self)` against that same shared key. **This
is our inference, not an observed runtime trace**: if the parent and the
delegated child are both live and both save while sharing a working
directory, the store has no concept of "this row belongs to conversation
X" -- it is last-write-wins by directory path, so a completing delegate task
can silently clobber the parent's saved conversation (or vice versa), with
no cascade, no orphan-detection, and no merge. We did not run this scenario
to confirm the race in practice; it is a structural inference from reading
`spawn_agent_process` against `set_conversation_by_path`'s key derivation.

Delegate task bookkeeping itself is entirely separate from
`ConversationState`/SQLite: `AgentExecution` (`delegate.rs:267-291`,
fields `agent`, `task`, `status: AgentStatus` [`Running`/`Completed`/
`Failed`], `launched_at`, `completed_at`, `pid`, `exit_code`, `output`,
`user_notified`, `summary`, `cwd`) is serialized as pretty JSON to
**one file per agent name** -- `agent_file_path`
(`:584-587`) is `<subagents_dir>/<agent>.json`, and `subagents_dir`
(`:589-591`, `crates/chat-cli/src/util/paths.rs:248-250,50`) resolves to
`<cwd>/.amazonq/.subagents/`, a **workspace-relative** directory, distinct
from the global `data.sqlite3`. Because the file is keyed by agent *name*
and fully overwritten (`save_agent_execution`, `delegate.rs:577-582`, plain
`os.fs.write`, no lock, no temp-and-rename), the doc comment's own stated
constraint is a durability fact, not a UX suggestion: "Only one task per
agent" (`:52`) -- launching a second task under the same agent name before
the first completes is explicitly rejected (`launch_agent`,
`:149-156`, checking `AgentStatus::Running`).

Nesting is unbounded in the sense that a delegated `q chat --non-interactive`
process could itself invoke Delegate again (no depth check was found), but
we did not trace whether the experiment flag or environment propagates to
make that practically possible.

There is no cascade/orphan/reconcile behavior for delegate tasks on parent
delete, rewind, or crash: `status_agent`/`status_all_agents`
(`:471-527`) detect a dead process only by `kill -0` on the recorded `pid`
(`is_process_alive`, `:530-546`) and only mark the execution `Failed` after
the fact -- there is no notification to, or cleanup by, whichever process
consumes this file if the *parent* (not the delegate) is the one that dies
or deletes its conversation.

## Retention, deletion, and multi-host

- **Retention/TTL**: no lifecycle policy, scheduled cleanup, or age-based
  expiry was found for the `conversations` table. A repo-wide grep confirms
  `Database::delete_entry` -- the generic delete primitive
  (`database/mod.rs:497-502`) -- is **never called with `Table::Conversations`**
  anywhere in the crate; there is no exposed "forget this conversation"
  operation. The only two paths that shrink or replace a conversation's
  content are the destructive cap and `/compact` described above, and both
  operate on content, never on the row's existence.
- **Deletion**: not supported for conversations at the store layer. (`auth_kv`
  secrets do have `delete_secret`/`delete_entry`, `crates/chat-cli/src/database/mod.rs:424-427`, and the
  shadow-git checkpoint directory is removed via `CheckpointManager::cleanup`/
  `Drop` -- `crates/chat-cli/src/cli/chat/checkpoint.rs:333-339,349-364` -- but neither of those is a
  conversation-row delete.)
- **Multi-host**: `data.sqlite3` is a single local file at a
  platform-local-data path (`dirs::data_local_dir()`,
  `crates/chat-cli/src/util/paths.rs:327-332`) -- there is no
  remote-writeback, no shared-filesystem assumption beyond ordinary local
  disk, and no cross-host sync of conversations found anywhere in this
  crate. Multi-host is out of scope by construction, similar to fx, but
  without fx's explicit permission/safety refusal logic -- Amazon Q simply
  assumes a private, local, single-user file (it does chmod the DB file to
  `0600` on Unix on every open, `database/mod.rs:212-223`, which is the one
  concrete safety measure present).
- **The `history` table is dead code at this commit.** Three of the eight
  migrations (`001_history_table.sql`, `002_drop_history_in_ssh_docker.sql`,
  `003_improved_history_timing.sql`) create and evolve a `history` table
  (`id, command, shell, pid, session_id, cwd, start_time, end_time,
  duration, hostname, exit_code` after all three migrations apply) that
  appears to be a **shell-command-history** feature, not a chat-session
  concept -- its column names (`command`, `shell`, `pid`, `exit_code`) are
  unrelated to `ConversationState`. A grep across the entire workspace (all
  nine crates, not just `chat-cli`) for `FROM history` / `INTO history`
  returns zero hits: **no code anywhere in this repository reads or writes
  this table**. It is migrated into existence on every fresh install but
  left permanently empty and unused as far as this commit's source is
  concerned -- flagged as a strong inference from an exhaustive grep, not a
  runtime trace, since dynamically-constructed SQL (e.g. via
  `format!("...{table}...")` with a non-`Table`-enum string) could in
  principle still target it; we found no such construction.

## Interop with foreign session stores

No evidence found. A targeted search for import/legacy-session/foreign-format
handling (`import_conversation`, `legacy conversation`, references to other
CLI agent products) returned nothing relevant to chat *sessions*. The one
"migration" concept that does exist, `PROFILE_MIGRATION_KEY`/
`get_has_migrated`/`set_has_migrated` (`database/mod.rs:64,340-347`,
consumed by `crates/chat-cli/src/cli/agent/legacy/mod.rs:30,102,227`), is a
one-time **config-format** migration (old "profile" config to the current
"agent" config schema), not a session-transcript importer, and is out of
scope for this section.

## What this implies for our Session Store (our inference)

- Amazon Q is the cleanest available confirmation that "session-as-mutable-
  document, keyed by location, with a hard size cap enforced by destructive
  drain" is a real, shipped design, not a strawman: `set_conversation_by_path`
  is a full-blob `INSERT OR REPLACE` on every single turn
  (`database/mod.rs:399-411`), and both of its size-bounding mechanisms (the
  10k-entry cap and `/compact`) mutate the one durable row in place with no
  tombstone, marker, or externally-retained pre-image. For our event-sourced
  Session Store this is the strongest available argument *against* choosing
  a document-overwrite model even for a "just make it work" v1: every
  size-bounding operation here is unrecoverable by construction, which is
  exactly the failure mode an append-only log with a derived, replaceable
  projection is meant to avoid.
- The path-as-primary-key design collapses two orthogonal concepts --
  *location* and *conversation identity* -- into one key. This directly
  produced the parent/delegate collision risk described above: a system
  that mints a real identity (`conversation_id`, UUIDv4) but doesn't use it
  as the storage key gets no protection from that identity at all. Our
  Session Store's stream/aggregate identity should be the actual session id,
  never a derived environmental value like a cwd path, precisely to avoid
  this class of same-location collision between concurrently-running
  sessions (parent and child/subagent, or two terminals in the same
  directory).
- The dual evolution strategy -- an explicit, ordered, transactional SQL
  migration ratchet for table *shape* (`MIGRATIONS`,
  `database/mod.rs:67-76,431-458`) versus purely additive
  `#[serde(default)]` fields with no version tag for the JSON *payload*
  shape inside `ConversationState` -- is a real-world precedent for treating
  "the envelope/table schema" and "the event payload schema" as separately
  versioned concerns, which lines up with the asymmetry we already noted
  from Zed's dossier (strict ratchet for identity-bearing structure, additive
  tolerance for payload fields).
- Tangent mode is a useful small data point on ergonomic, reversible
  exploration: it is implemented as a full in-memory snapshot-and-restore
  rather than an appended marker, and it is itself part of the persisted
  blob (so it partially survives a crash) -- but it also shows the downside
  of that approach, since we could not confirm any automatic recovery path
  that surfaces an abandoned tangent snapshot to the user on the next
  resume. Our design's equivalent (an explicit "diverged" branch) should
  make abandoned branches discoverable at resume time rather than silently
  present-but-unsurfaced inside a resumed document.
- The complete absence of a listing/enumeration operation over
  conversations -- not even a full-table scan exists in the source -- is a
  useful negative baseline for "how little session-store surface a shipping
  product can get away with": Amazon Q's UX substitutes "the directory you
  are standing in" for a picker entirely. Any product wanting cross-project
  session listing (which ours does) needs to build that deliberately; it is
  not a byproduct of even a working single-session store.

## Open questions

- Whether `set_conversation_by_path`'s path-as-key collision between a
  parent session and a same-directory Delegate subagent actually manifests
  as data loss in practice (our inference is structural, from reading
  `spawn_agent_process`, `ChatSession::new`'s resume branch, and
  `set_conversation_by_path`'s key derivation, not from an observed race).
- The exact default value and full field list of `ToolUseResultBlock` and
  the remainder of `ToolUseResult`'s surrounding types in
  `crates/chat-cli/src/cli/chat/message.rs` past line 339 -- our read window
  stopped there; content-block variants beyond `Json(Document)`/`Text(String)`
  (seen used at `message.rs:271-275`) were not independently confirmed
  against the type definition.
- Whether an abandoned `tangent_state` (process crashed or was killed while
  in tangent mode) is ever surfaced back to the user on the next `--resume`,
  or whether it silently remains dormant inside the persisted blob until a
  user manually re-triggers `/tangent` -- we found the snapshot/restore
  functions but no resume-time check for a non-`None` `tangent_state`.
  the field.
- Whether SQLite's default `busy_timeout=0` genuinely causes observable
  `SQLITE_BUSY` failures under concurrent writers in practice, versus being
  masked by r2d2's connection-pool behavior or by writes being rare enough
  in normal single-user CLI usage that contention essentially never occurs
  -- we verified only the absence of any `PRAGMA busy_timeout` in source, not
  a runtime reproduction.
- Whether any dynamically-constructed SQL elsewhere in the crate (outside
  `database/mod.rs`) ever targets the dead `history` table under a table
  name built by string formatting rather than the `Table` enum -- we did not
  find one, but a targeted grep for literal `history` table SQL is weaker
  evidence than the enum-based confirmation we have for `conversations`
  and `auth_kv`.
- Whether `agent`/`chat-cli-ui` (the two sibling crates we did not read in
  depth) contain any additional session-adjacent persistence -- this dossier
  is scoped to `crates/chat-cli`, the crate containing the anchor paths we
  were given; we did not audit `crates/agent` or `crates/chat-cli-ui` for
  overlapping state.

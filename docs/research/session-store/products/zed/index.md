# Zed: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-08-04. Version-sensitive claims were checked
against a local clone of
[zed-industries/zed](https://github.com/zed-industries/zed) pinned at commit
`4aad57fd1f002f9feeea2b7fb6229ccbcd576cb1` ("workspace: Return the created
workspace when opening a remote project (#62028)", Aug 3 2026). Authoritative
anchors: `crates/agent_ui/src/thread_metadata_store.rs` (sidebar metadata
store and its `sqlez::Domain` migrations), `crates/agent/src/db.rs` and
`crates/agent/src/thread_store.rs` (thread content store), `crates/agent/src/thread.rs`
(in-memory thread model and ACP boundary conversion), `crates/sqlez/src/{domain,migrations}.rs`
(the migration ratchet), `crates/remote/src/remote_identity.rs` and
`crates/util/src/path_list.rs` (identity/relocation), and
`agent_client_protocol::schema::v1` as `acp` (the wire schema Zed's session
identity is drawn from). Zed is licensed per-crate; every crate cited below is
`GPL-3.0-or-later` except `gpui`, `util`, and `collections`, which are
`Apache-2.0` (checked against each crate's `Cargo.toml` `license` field at this
commit). This dossier is orthogonal to, and cross-references rather than
re-derives, the existing ACP corpus at [../../../acp/index.md](../../../acp/index.md)
and [../../../acp/products/zed.md](../../../acp/products/zed.md), which covers
Zed as an ACP *client* wiring external agents; this dossier covers Zed's own
built-in agent panel as a session-storage system.

## The storage model

Zed's agent panel keeps thread data in **two structurally distinct SQLite
databases**, migrated by two different mechanisms, which is the single most
important fact to get right before anything else:

1. **`ThreadMetadataDb`** -- table `sidebar_threads`, living inside the single
   shared `db.sqlite` file that the whole Zed application (not just the agent
   panel) uses for workspace state, key-value settings, and everything else
   registered as a `sqlez::Domain`
   (`crates/agent_ui/src/thread_metadata_store.rs:1370`,
   `impl Domain for ThreadMetadataDb`). This is *metadata only*: title,
   timestamps, project paths, remote-connection scoping, archival state -- not
   message content. It is migrated by `sqlez`'s strict, stored-text-compared,
   one-way ratchet (see Entry/message structure and versioning).
2. **`ThreadsDatabase`** -- table `threads`, in its own file at
   `paths::data_dir().join("threads").join("threads.db")`
   (`crates/agent/src/db.rs:438-440`), holding the actual message content as a
   single compressed JSON blob per row. It is migrated by ad hoc,
   best-effort `ALTER TABLE ... ADD COLUMN` statements run at startup with
   errors swallowed if the column already exists
   (`crates/agent/src/db.rs:456-471`) -- there is no ratchet, no drift
   detection, and no migrations ledger for this database.

Both databases are row-based mutable-document stores, not append-only logs.
`ThreadsDatabase::save_thread_sync` (`crates/agent/src/db.rs:489`) does a full
upsert of one JSON+zstd blob per thread on every save; there is no per-turn
append record anywhere in the durable content path. `ThreadMetadataDb`'s
`sidebar_threads` rows are likewise fully replaced on each save
(`crates/agent_ui/src/thread_metadata_store.rs:741`, `save_internal`, and the
domain's `save`/`delete`/`list` SQL at `:1370` onward). The closest thing to
an append-only structure anywhere in this system is the `migrations` ledger
table that `sqlez::Connection::migrate` writes to
(`crates/sqlez/src/migrations.rs:46-60`) -- but that records *schema* history,
not application data.

Session identity, however, is drawn directly from the Agent Client Protocol:
`ThreadMetadata.session_id: Option<acp::SessionId>`
(`crates/agent_ui/src/thread_metadata_store.rs:311`) and
`ThreadsDatabase::load_thread`/`delete_thread` are keyed by `acp::SessionId`
(`crates/agent/src/db.rs:607`, `:671`). So identity is ACP-shaped, but content
is not: see "Entry/message structure and versioning" below for exactly where
the wire-format boundary sits. The best-fitting conceptual model is
**session-as-row across two independently-keyed tables** -- a metadata row
keyed loosely (thread id, optionally linked to a session id) for listing, and
a content row keyed strictly by ACP session id for the full document -- with
neither table backed by an append log.

## Keying and identity

- The canonical metadata-store key is `ThreadId`, a newtype over `uuid::Uuid`
  (`crates/agent_ui/src/thread_metadata_store.rs:34`,
  `pub struct ThreadId(uuid::Uuid);`). This exists independently of ACP: a
  thread can exist as a metadata row (e.g. a draft) before it has ever been
  assigned a session. Both of Zed's identifiers are random, not
  time-ordered: `ThreadId::new` is `Self(uuid::Uuid::new_v4())`
  (`crates/agent_ui/src/thread_metadata_store.rs:36-39`), and the two sites
  that mint a fresh `acp::SessionId` for Zed's own agent both wrap
  `uuid::Uuid::new_v4()` (`crates/agent/src/thread.rs:1380`,
  `crates/agent_ui/src/agent_panel.rs:3819`). Nothing in the key carries
  ordering, which is why every listing path has to sort on a stored timestamp
  column instead.
- `ThreadMetadata.session_id: Option<acp::SessionId>`
  (`:311`) links a metadata row to its ACP session once one exists; the
  content store (`ThreadsDatabase`) is keyed purely by `acp::SessionId`
  (`crates/agent/src/db.rs:607,671`). `ThreadMetadataStore` keeps an in-memory
  reverse index, `threads_by_session: HashMap<acp::SessionId, ThreadId>`
  (`:505`), to translate between the two keyspaces.
- Comment evidence confirms drafts are the reason `session_id` is optional:
  "Drafts may not have a session_id yet; only index by session" (grepped
  comment text in `thread_metadata_store.rs`), consistent with the
  `Option<acp::SessionId>` field.
- Beyond thread id and session id, `ThreadMetadataStore` maintains two more
  in-memory indexes for scoping: `threads_by_paths: HashMap<PathList,
  HashSet<ThreadId>>` and `threads_by_main_paths: HashMap<PathList,
  HashSet<ThreadId>>` (`:500-509`) -- i.e. listing is scoped per-project by
  literal worktree path set, not global. `entries_for_path`
  (`:621`) is the per-project listing entry point.
- `PathList` (`crates/util/src/path_list.rs`, Apache-2.0) is the path-identity
  primitive: an ordered list of absolute paths whose `PartialEq`/`Hash`
  compare only the *set* of paths (lexicographically sorted internally),
  ignoring display order (`path_list.rs:27-39`). This means two projects with
  the same folders opened in a different order are the same identity key;
  renaming or moving a folder is a *different* key with no automatic
  reconciliation baked into `PathList` itself -- reconciliation, where it
  exists, is bolted on elsewhere (next point).
- Live relocation reconciliation happens at the workspace layer, not inside
  the store: `crates/sidebar/src/sidebar.rs` subscribes to
  `ProjectEvent::WorktreePathsChanged { old_worktree_paths }`
  (`subscribe_to_workspace`, `sidebar.rs:992-1012`) and calls
  `move_entry_paths` (`sidebar.rs:1062`), which in turn calls
  `ThreadMetadataStore::change_worktree_paths`
  (`crates/agent_ui/src/thread_metadata_store.rs:978`, via
  `mutate_thread_paths` at `:1005`) to rewrite the path-list key for every
  thread matching the *old* path set -- this is a broad, all-matching-threads
  rewrite, not scoped to only the currently active/open threads. A narrower,
  retained/active-threads-specific reconciliation also exists at
  `crates/agent_ui/src/agent_panel.rs:4113` (`update_thread_work_dirs`). Both
  paths only fire while Zed is running and the affected project is open in a
  workspace at the moment of the rename/move; **we could not find any code
  path that reconciles a worktree rename that happened while Zed was not
  running** -- this is listed under Open Questions rather than asserted as a
  gap, since we did not find positive evidence either way for an
  offline-reconciliation attempt (e.g. a path-existence check at load time).
- Remote/multi-host identity is layered independently of path identity.
  `RemoteConnectionOptions` (`crates/remote/src/remote_client.rs:1320-1327`)
  is an enum over `Ssh`/`Wsl`/`Docker` (plus test-only `Mock`) variants
  carrying full connection detail (including runtime-only fields like SSH
  passwords or Docker env overrides). Matching a live connection against
  persisted thread metadata does **not** use `RemoteConnectionOptions`
  equality directly; it goes through `RemoteConnectionIdentity`
  (`crates/remote/src/remote_identity.rs:10-27`), a normalized projection
  (host/username/port for SSH, distro/user for WSL, container id/name/user for
  Docker) with an explicit doc comment: "so runtime-only fields like SSH
  nicknames or Docker environment overrides do not affect matching"
  (`remote_identity.rs:6-8`). `same_remote_connection_identity`
  (`:87-98`) is the comparison entry point, and its own test suite
  (`:107-198`) confirms password/nickname/upload-flag changes do not break
  identity while host/port/username changes do.
- Session ids: we did not find the exact minting call site for
  `acp::SessionId` inside this codebase within our reading budget (Zed is
  the ACP host generating ids for its own built-in agent, not just a client
  consuming ids from an external one) -- flagged under Open Questions rather
  than guessed.

## The store interface

There is no pluggable store trait for agent threads (unlike, say, a
`LanguageModel` provider trait elsewhere in Zed) -- the store is two internal
modules with ad hoc call sites. Reconstructed operation contract, split by
which of the two databases each call touches:

**`ThreadMetadataStore`** (`crates/agent_ui/src/thread_metadata_store.rs`,
wraps `ThreadMetadataDb`):

| Operation | Signature / entry point | Notes |
| --- | --- | --- |
| List (scoped) | `entries_for_path<'a>(...)` (`:621`) | In-memory, filtered by `PathList` key; no DB hit per call |
| Reload (bootstrap) | `fn reload(&mut self, cx) -> Shared<Task<()>>` (`:657`) | Full-table load into the in-memory `threads`/`threads_by_*` maps |
| Save (upsert) | `fn save_internal(&mut self, metadata: ThreadMetadata)` (`:741`) | Enqueues onto `pending_thread_ops_tx`, not a direct write |
| Archive / unarchive | `fn archive(...)` (`:858`), `fn unarchive(&mut self, thread_id, cx)` (`:873`) | Archival is a metadata-store concept tied to `ArchivedGitWorktree` |
| Relocate | `fn change_worktree_paths(...)` (`:978`), `fn mutate_thread_paths(...)` (`:1005`) | Broad rewrite across all matching threads |
| Delete | `fn delete(&mut self, thread_id, cx)` (`:1140`); `fn delete_all(...)` (`:1175`) | Enqueued the same way as save |
| Draft matching | `fn unarchived_draft_ids_matching(...)` (`:1165`) | For dedup/lookup of not-yet-sessioned drafts |
| Conversation event intake | `fn handle_conversation_event(...)` (`:1265`) | Where live thread events (title changes, etc.) become metadata writes |

Writes to `ThreadMetadataDb` are **not synchronous**: `save`/`delete` push a
`DbOperation::Upsert`/`Delete` (`:513-517`) onto an `async_channel`, consumed
by a background task (`_db_operations_task`, `:509`) that de-duplicates
pending operations per thread id before hitting SQLite -- an eventual-write,
coalesced/debounced design, not one write per call.

**`ThreadsDatabase`** (`crates/agent/src/db.rs:392`, wraps a single
`Arc<Mutex<Connection>>`, `:394` -- one physical connection, no pool):

| Operation | Signature / entry point | Notes |
| --- | --- | --- |
| Save (full overwrite) | `fn save_thread_sync(...)` (`:489`) | Full JSON+zstd blob upsert; called from async `Task` wrappers |
| List | `fn list_threads(&self) -> Task<Result<Vec<DbThreadMetadata>>>` (`:564`) | Full-table scan, no pagination |
| Load | `fn load_thread(&self, id: acp::SessionId) -> Task<Result<Option<DbThread>>>` (`:607`) | Single-row fetch + decompress + version-sniffed deserialize |
| Delete (cascading) | `fn delete_thread(&self, id: acp::SessionId) -> Task<Result<()>>` (`:671`) | Stack-based transitive walk over `parent_id` finds and deletes all transitive subagent children too |
| Delete all | `fn delete_threads(&self) -> Task<Result<()>>` (`:724`) | Wipes the whole table |

`ThreadStore` (`crates/agent/src/thread_store.rs`) is a thin GPUI-entity
wrapper over `ThreadsDatabase` that additionally filters subagent threads out
of the main listing: `if thread.parent_session_id.is_some() { continue; }`
inside `entries()` (`thread_store.rs:115,131`).

## Write and append path (ordering, durability, concurrency, delivery)

- **Trigger**: writes to `ThreadsDatabase` are reactive, not batched by turn.
  `crates/agent/src/agent.rs:820` registers `cx.observe(&thread_handle, ...)`
  on every `Thread` entity, and that observer calls `save_thread`
  (`crates/agent/src/agent.rs:1736`) on essentially every GPUI change notification -- i.e. any
  mutation to the in-memory `Thread` (new message, tool-call update, title
  change) can trigger a full-document re-save, not just message-boundary
  commits.
- **Ordering / durability**: there is no positional-append guarantee to
  reason about because the unit of write is "the whole document," not a line
  or row per turn. Durability is whatever a single SQLite `UPDATE`/`INSERT`
  gives you under `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=500;`
  (`crates/db/src/db.rs:130-131`, the initialize pragmas run on every opened
  connection). `cx.on_app_quit(Self::flush_threads_on_quit)`
  (`crates/agent/src/agent.rs:575`, implementation at `crates/agent/src/agent.rs:1795`) registers a final flush
  specifically to race-proof shutdown against the reactive/async save path --
  i.e. the authors were aware that reactive saves can lag behind quit and
  added an explicit drain-on-quit safeguard.
- **Concurrency**: single-writer-per-process via one shared
  `Arc<Mutex<Connection>>` (`crates/agent/src/db.rs:394`) -- no expected-version/CAS precondition
  anywhere in `save_thread_sync`; it is a last-write-wins full overwrite.
  Concurrent Zed processes against the same `db.sqlite`/`threads.db` are
  handled only by SQLite's own WAL + busy-timeout, not by any
  application-level optimistic-concurrency check.
  `ThreadMetadataDb` writes are similarly last-write-wins per the coalescing
  background task described above (last enqueued mutation per thread id wins
  within a debounce window).
- **Delivery semantics**: best-effort, at-most-once-per-save-attempt from the
  app's perspective -- there is no retry/backoff visible around
  `save_thread_sync` or the metadata store's background op-processing task
  beyond what `anyhow::Result` propagation and logging provide. There is no
  idempotence key on individual "entries" because there are no discrete
  entries at the storage layer -- the whole thread is the unit.

## Read and resume path

- Resume reads directly from `ThreadsDatabase::load_thread(id)`
  (`crates/agent/src/db.rs:607`): a single-row SQLite `SELECT`, zstd-decompress, then a
  version-sniffed `serde_json` deserialize (`DbThread::from_json`, `db.rs`
  around the `VERSION`/`from_json` block near `:189` onward) -- full ordered
  read of the entire message vector in one shot, not incremental and not
  cursor-based. There is no entry-level pagination or transcript-size bound
  found anywhere in the load path.
- Listing/metadata, by contrast, is **eagerly materialized and kept resident**
  rather than loaded lazily per view: `ThreadMetadataStore::reload`
  (`thread_metadata_store.rs:657`) does a full-table load into the in-memory
  `threads: HashMap<ThreadId, ThreadMetadata>` and its path/session indexes at
  startup, and `ThreadsDatabase::list_threads` (`crates/agent/src/db.rs:564`) similarly does a
  full-table scan with no pagination. Sidebar listing/searching afterward is
  pure in-memory work against this resident cache (see next section) -- no
  incremental disk reads for browsing.
- Full message content is loaded lazily, on demand, only when a specific
  thread is opened (`load_thread(id)`), not prefetched for every row the
  sidebar shows.

## Listing, summaries, and search

- The sidebar list view is backed by `ThreadMetadata` rows (title, timestamps,
  path scoping, archival flag), which is exactly the "metadata sidecar/summary
  maintained at write time" pattern -- `ThreadMetadataDb`/`sidebar_threads` is
  a read model, denormalized away from the full message content that lives in
  `ThreadsDatabase`.
- Filtering/search over the sidebar list is **in-memory fuzzy string
  matching**, not a separate indexed subsystem: `crates/sidebar/src/sidebar.rs`
  calls `fuzzy_match_positions(&query, ...)` against already-loaded thread
  titles (and terminal-session titles) at multiple call sites (grepped hits
  at `sidebar.rs:1851,1860,1867,1885,1895`). We did not find any FTS table,
  vector index, or external search service anywhere in `agent_ui`, `agent`,
  or `sidebar` for cross-thread content search -- search appears to be
  title-only, over the resident `ThreadMetadata` cache, with no persisted
  index to bootstrap or keep consistent. We treat this as a well-supported
  but not fully exhaustive finding (see Open Questions): we grepped the
  relevant files for FTS/fuzzy/index keywords rather than reading every
  sidebar/search file end-to-end.
- Cross-listing at scale: because both `list_threads` and metadata `reload`
  are full-table scans loaded once into memory and then diffed/patched by the
  background op-processing task, there is no stated cost number (no
  benchmark or scale comment found), unlike grok-build's explicit ~12K-session
  full-scan pain point.

## Entry/message structure and versioning

- The durable content type is `DbThread`
  (`crates/agent/src/db.rs`, struct beginning near `:54`): `title:
  SharedString`, `messages: Vec<Arc<DbMessage>>`, `updated_at: DateTime<Utc>`,
  plus a long tail of `#[serde(default)]` optional fields -- `detailed_summary`,
  `initial_project_snapshot`, `cumulative_token_usage`,
  `request_token_usage: HashMap<acp_thread::ClientUserMessageId,
  language_model::TokenUsage>`, `model`, `profile`, `subagent_context:
  Option<crate::SubagentContext>`, `speed`, `thinking_enabled`,
  `thinking_effort`, `draft_prompt: Option<Vec<acp::ContentBlock>>`,
  `ui_scroll_position`, `sandboxed_terminal_temp_dir`, `sandbox_grants`. The
  additive-`#[serde(default)]` pattern on nearly every field after the core
  three is the schema-evolution mechanism for this table: new fields default
  in for old rows rather than requiring a migration.
- `DbThread::VERSION = "0.3.0"` (`crates/agent/src/db.rs:189`) is checked explicitly in
  `from_json`: it matches on the JSON `"version"` field and, for any value
  other than the current constant, calls `Self::upgrade_from_agent_1(...)`
  against `crate::legacy_thread::SerializedThread` -- a one-way upgrade path
  from an older, structurally different schema (file not read in full; the
  call site alone confirms a legacy-format-sniffing upgrade exists).
  Structurally this is version-tagged additive evolution plus a one-shot
  legacy-format bridge, not a numbered migration ratchet -- quite different
  from the `sqlez::Domain` mechanism described next.
- The **metadata table**, by contrast, evolves through `sqlez`'s strict
  migration ratchet. `Connection::migrate`
  (`crates/sqlez/src/migrations.rs:37-104`) stores every applied migration's
  formatted SQL text in a `migrations (domain, step, migration)` table
  (`:45-50`) and, on every subsequent boot, re-diffs the compiled-in
  `Domain::MIGRATIONS` array against what's stored: if step *n*'s text
  matches, it's skipped (already applied); if it differs, the connection call
  panics (`anyhow::bail!`) unless `Domain::should_allow_migration_change`
  explicitly opts in for that step (`domain.rs:7-9`, `crates/sqlez/src/migrations.rs:66-89`).
  `ThreadMetadataDb`'s own `impl Domain` (`thread_metadata_store.rs:1370-1373`)
  does not override `should_allow_migration_change`, so it uses the strict
  default (`false`) -- **any edit to an already-shipped migration string is a
  hard startup failure**, not a silent skip. This is confirmed by the crate's
  own test suite (`sqlez/src/migrations.rs:311-346`,
  `changed_migration_fails`). Practically: an **old Zed binary opening a
  newer database** will simply not have run the newer migrations (they don't
  exist in its compiled `MIGRATIONS` array), so it operates against a schema
  ahead of what it expects -- we did not find explicit downgrade-safety
  handling for this case and list it under Open Questions. A **new Zed binary
  opening an older database** runs exactly the not-yet-applied migration
  steps in order, ratcheting the schema forward one time only; this direction
  is squarely what the mechanism is built for.
- Backfill migrations layered on top of the schema ratchet use one-shot KVP
  guards rather than schema versioning: `THREAD_REMOTE_CONNECTION_MIGRATION_KEY`
  and `THREAD_ID_MIGRATION_KEY`
  (`thread_metadata_store.rs:60-61`), read/written via `read_kvp`/`write_kvp`
  (`:203,262,280,294`) against `KeyValueStore`, which is itself a
  `sqlez::Domain` (`crates/db/src/kvp.rs:20-21`) sharing the very same
  `db.sqlite` file. This is a distinct mechanism from the schema-text ratchet:
  it guards a one-time *data backfill* (e.g. populating a newly-added remote
  identity column from existing rows), not a `CREATE TABLE`/`ALTER TABLE`
  step.
- Message structure inside `DbThread.messages`: the persisted `Message` enum
  (`crates/agent/src/thread.rs`) has `User`/`Agent`/`Resume`/`Compaction`
  variants (evidenced throughout, e.g. match arms at `thread.rs:2379,3966,4809`).
  `UserMessage`/`AgentMessage` and their content enums
  (`UserMessageContent`/`AgentMessageContent`) are Zed's **own internal**
  representation -- not literally serialized ACP wire types. The conversion
  to/from the ACP wire schema happens at two explicit boundary functions:
  `UserMessageContent::from_content_block(value: acp::ContentBlock, ...)`
  (`thread.rs:6717`) and `impl From<UserMessageContent> for acp::ContentBlock`
  (`thread.rs:6773`). So: **session identity is literally ACP** (`acp::SessionId`
  used as the storage key throughout both databases), but **persisted message
  content is Zed's internal model**, converted at these two named functions --
  the stored thread is not a serialized ACP session.

## Compaction and history management

- Compaction is a **store-visible, in-place marker**, not a destructive
  rewrite and not an external snapshot file. `Thread::compact`
  (`thread.rs:2526`) and the automatic path both eventually call
  `stream_compaction` (`thread.rs:3135`), which on success inserts
  `Arc::new(Message::Compaction(CompactionInfo::Summary(summary.into())))`
  directly into `self.messages` (`thread.rs:3216-3234`): for the automatic
  case it is `messages.insert(insertion_ix, compaction)`, for the manual case
  it is appended after a marker `UserMessage` carrying the triggering
  `ClientUserMessageId`. Prior messages are **not removed** from the vector --
  the full raw history remains in `self.messages` (and therefore in the next
  full-document save to `ThreadsDatabase`).
- The model-visible view shrinks only at *request-building* time, not at
  storage time: `latest_compaction_message_ix_before`
  (`thread.rs:4844-4847`, `rposition` over `Message::Compaction` variants)
  finds the most recent compaction marker before a given point, and the
  request-assembly helper around it (`thread.rs:4820-4839`) skips messages
  before that marker except for a small retained-user-messages budget
  (`retained_user_request_messages_before`,
  governed by `COMPACTION_RETAINED_USER_MESSAGES_BYTE_BUDGET`). So: **the
  durable record keeps everything; only the LLM-facing request view is
  truncated at replay time**, evaluated fresh from the marker position on
  every turn rather than persisted as a separate compacted artifact.
- Resume crossing a compaction boundary therefore requires no special
  handling: `load_thread` loads the full `messages` vector including every
  `Message::Compaction` marker ever inserted, and the same
  marker-position-scan logic re-derives the model-visible view on the next
  turn.
- This is a clean point of contrast with `truncate()` (next section), which
  *is* destructive -- compaction leaves everything in place; truncate removes
  it.

## Rewind, checkpoints, and fork

- **Rewind/truncate is destructive, not an appended marker.** `Thread::truncate`
  (`thread.rs:2359-2383`) finds the position of a target user message by id
  and then does `for message in self.messages.drain(position..)` -- a genuine
  in-memory `Vec::drain`, permanently discarding every message from that
  point forward (net of per-message token-usage bookkeeping cleanup). Because
  every subsequent save is a full-document overwrite (`save_thread_sync`),
  the very next save after a truncate **permanently erases** the truncated
  tail from `threads.db` -- there is no tombstone, no soft-delete, and no
  server-side ability to "un-rewind" once a save has landed.
- **File-state checkpoints do not survive a restart.** The client-rendering
  type `acp_thread::UserMessage` carries `checkpoint: Option<Checkpoint>`
  (`crates/acp_thread/src/acp_thread.rs:294-307`), where `Checkpoint` wraps a
  `GitStoreCheckpoint` (`project::git_store::GitStoreCheckpoint`,
  `crates/project/src/git_store.rs:321`, itself ultimately a
  `GitRepositoryCheckpoint { commit_sha: Oid }`,
  `crates/git/src/repository.rs:1278`). This `acp_thread::UserMessage` type
  is the **ephemeral, in-memory client-side rendering layer** -- we found no
  corresponding `checkpoint` field on the durable `agent::thread::UserMessage`
  / `Message::User` type that gets serialized into `DbThread`. Checkpoints
  are created/compared/restored via `GitStore::checkpoint`/`restore_checkpoint`/
  `compare_checkpoints` (`crates/project/src/git_store.rs:1880,1901,1928`, with local vs.
  remote-RPC dispatch, and lower-level git-backend equivalents at
  `crates/project/src/git_store.rs:9049,9072,9197`), all operating on live git repository state
  (a commit sha of a WIP stash-like commit), not on any row in either
  SQLite database. So: rewinding file state to a checkpoint works within a
  live session, but the checkpoint pointer itself is not persisted -- reopen
  the thread after a restart and the file-state rewind capability for past
  turns is gone even though the message text remains.
- **There is no first-class fork.** The closest equivalent is a manual
  clipboard copy/paste: `copy_thread_to_clipboard`
  (`crates/agent_ui/src/agent_panel.rs:3717`) serializes the current
  `DbThread` into a `SharedThread` (`crates/agent/src/db.rs:131-141`, `VERSION = "1.0.0"`,
  zstd+base64-encoded JSON via `to_bytes`/`from_bytes`), and
  `load_thread_from_clipboard` (`agent_panel.rs:3777`) reverses this via
  `SharedThread::to_db_thread` (`db.rs`, the `to_db_thread` impl). Critically,
  `to_db_thread` explicitly resets `subagent_context: None` and every other
  identity-adjacent field to its default, and only prefixes the title with a
  link emoji (`format!("🔗 {}", self.title)`) -- **no parent/lineage pointer,
  shared-prefix reference, or origin session id is recorded anywhere**. The
  pasted thread gets a brand-new `acp::SessionId` (minted wherever a new
  thread is normally created) with zero durable connection to its origin.
  This is explicitly not shared-prefix forking or copy-plus-lineage; it is
  copy-and-forget.

## Subagents and nested sessions

- A subagent is a **first-class sibling row** in `ThreadsDatabase`, not
  nested inside the parent's `messages` vector. `SubagentContext {
  parent_thread_id: acp::SessionId, depth: u8 }`
  (`thread.rs:143-149`) is attached via `DbThread.subagent_context:
  Option<crate::SubagentContext>` (`db.rs`, field list above), and
  `Thread::new_subagent` (`thread.rs:1299`) is the constructor call site.
- Nesting is bounded to one level: `MAX_SUBAGENT_DEPTH: u8 = 1`
  (`thread.rs:77`), checked at `thread.rs:2169` (`if self.depth() <
  MAX_SUBAGENT_DEPTH`) before allowing a further spawn -- so a subagent cannot
  itself spawn a grandchild subagent.
- Subagent threads are hidden from the normal sidebar listing:
  `ThreadStore::entries()` explicitly filters out any row whose
  `parent_session_id.is_some()` (`crates/agent/src/thread_store.rs:115,131`).
  They are real rows with their own full transcript (isolated, not merged
  into the parent's), just excluded from the top-level list view.
- Cascade on parent delete is genuine, not orphaning:
  `ThreadsDatabase::delete_thread` (`crates/agent/src/db.rs:671-716`) does a stack-based
  transitive walk over `parent_id` (`SELECT id FROM threads WHERE parent_id =
  ?`, looped via a `frontier` vector popped from the back, so depth-first in
  practice, collecting into `ids_to_delete`) and deletes every
  transitively-found child in the same locked-connection block as the parent
  itself. This is a clear point of contrast with stores that orphan child
  sessions on parent deletion. Note: the whole walk-and-delete sequence runs
  under one held `Mutex<Connection>` guard but we did not find it wrapped in
  an explicit SQL `BEGIN`/`COMMIT` -- see Open Questions for the crash-safety
  implication.
- We found no code handling parent rewind/crash propagating to a still-live
  subagent (e.g. cancellation) -- only the delete-cascade path was confirmed;
  this is listed under Open Questions rather than asserted as absent.

## Retention, deletion, and multi-host

- **Retention/TTL**: no lifecycle policy or scheduled-cleanup mechanism was
  found for ordinary threads -- deletion is user-driven (sidebar delete
  action) or explicit archival, not time-based expiry. The one retention-like
  mechanism present is tied to **git-worktree archival**, not thread age:
  `ArchivedGitWorktree` (`thread_metadata_store.rs:457-470`) records
  `worktree_path`, `main_repo_path`, and WIP commit hashes
  (`staged_commit_hash`/`unstaged_commit_hash`/`original_commit_hash`, per
  the struct's continuation) so that archiving a thread can also archive (and
  later restore) the git worktree it was operating in -- a space-reclamation
  feature bound to thread archival, not an independent TTL sweep.
- **Deletion cascade**: `ThreadMetadataStore::delete`/`delete_all`
  (`thread_metadata_store.rs:1140,1175`) removes the metadata row (async,
  through the same debounced op-channel as saves); `ThreadsDatabase::delete_thread`
  (`crates/agent/src/db.rs:671`) removes the content row plus all cascaded subagent children
  plus any `sandboxed_terminal_temp_dir` associated with a deleted row
  (cleanup call at `db.rs` following the delete loop). These are two
  separate delete calls against two separate databases -- we found no single
  transactional operation spanning both `db.sqlite` and `threads.db`, so a
  crash between the two delete calls could leave one store's row present
  without its counterpart (flagged under Open Questions).
- **Multi-host**: thread data is **local to the machine running the Zed
  client process** -- we found no remote-writeback path for agent thread
  content or metadata. `RemoteConnectionIdentity`
  (`remote/src/remote_identity.rs`) is the only multi-host-aware concept in
  this subsystem, and it exists purely to *scope which locally-stored threads
  a given remote connection's project should show* (matching normalized host
  identity against `ThreadMetadata`'s remote-connection field), not to
  replicate or fetch thread data from the remote host itself. In other
  words: SSH/WSL/Docker remote projects still store their agent threads on
  the **local** Zed client's SQLite files, keyed in part by which remote
  identity the project was opened under.
- **Release-channel isolation**: each release channel (Stable/Preview/
  Nightly/Dev) gets a wholly separate `db.sqlite` at
  `{db_dir}/0-{scope_name}/db.sqlite` (`db_path`, `crates/db/src/db.rs:164-167`,
  `scope_name` trait methods at `:144-157`) -- so "multi-host" inside a single
  machine also includes "multi release-channel," each with an independent
  copy of both databases.
- **Stateless mode**: if `ZED_STATELESS` is set, `open_db`
  (`crates/db/src/db.rs:174-180`) skips the on-disk path entirely and falls back to an
  in-memory-only connection -- thread data becomes fully ephemeral,
  process-lifetime-only, for that run.
- **Crash/concurrency handling**: `PRAGMA journal_mode=WAL; PRAGMA
  busy_timeout=500;` (`crates/db/src/db.rs:130-131`) plus a fallback path,
  `open_fallback_db` (`crates/db/src/db.rs:215-225`), reached on either of two
  triggers from `open_db` (`crates/db/src/db.rs:174-203`): `ZED_STATELESS` is
  set, or `open_main_db` returns `None` because the parent directory could not
  be created or the file could not be opened, which also sets the global
  `ALL_FILE_DB_FAILED` flag. The fallback opens an in-memory connection named
  `FALLBACK_MEMORY_DB` behind a `log::warn!`, and `.expect`s on failure, so a
  broken migration or initialization query panics rather than degrading
  further. The practical consequence for session storage is that an
  unopenable database does not stop the editor: threads are written to a
  memory database and silently lost at process exit.

## Interop with foreign session stores

- Zed's agent thread stores do not read any *other* product's session store
  format (no Claude Code, Codex, Cursor, etc. discovery/import code found in
  `agent`, `agent_ui`, or `acp_thread`). The dedicated ACP-corpus research
  already establishes Zed's role as an ACP *client* that spawns and talks to
  external agent processes over the protocol
  ([../../../acp/products/zed.md](../../../acp/products/zed.md)); that is a
  live-protocol integration, not a session-store import, and it is out of
  scope for the storage question this dossier answers.
- The one real interop feature is **self-interop across release channels**:
  `channels_with_threads` (`crates/agent_ui/src/thread_import.rs:66`) opens a
  raw `sqlez::connection::Connection` directly against another release
  channel's `db.sqlite` file to detect whether it has thread rows, and
  `import_threads_from_other_channels`/`_in`
  (`thread_import.rs:891,896`) plus
  `thread_metadata_store::list_thread_metadata_from_connection`
  (referenced at `thread_import.rs:955`) read that foreign channel's metadata
  read-only for an import UI flow (`AcpThreadImportOnboarding`/
  `CrossChannelImportOnboarding`, per file structure). This is Zed importing
  its **own** prior data across Stable/Preview/Nightly/Dev boundaries, not a
  foreign-product import.

## What this implies for our Session Store (our inference)

- Zed's split -- a strictly-migrated metadata table for listing versus an
  ad-hoc-migrated content table for the actual document -- is a real-world
  example of two different reliability bars applied to two parts of the same
  logical "session": the part that must never silently drift (schema
  identity/structure) got a text-compared ratchet with a hard-failure default;
  the part that changes shape often (message content, feature flags on the
  thread) got additive `#[serde(default)]` fields and swallowed-error
  `ALTER TABLE`. For our event-sourced Session Store, this argues for the
  same asymmetry rather than one migration story for everything: the event
  schema (envelope, ordering, aggregate/stream identity) deserves a strict
  ratchet; the payload/data-carrying fields on individual event types can
  reasonably tolerate additive evolution.
- Zed is a mutable-document store dressed in ACP's identity vocabulary, not
  an event-sourced system: full-document overwrite on every reactive change,
  no expected-version precondition on writes, no append-only backing log for
  content. Its compaction design is the interesting counter-example, though --
  compaction is implemented as an **in-place marker plus a scan-from-marker
  replay rule** even inside a document-store world, which is functionally
  identical to how our decider stack would fold from a snapshot/marker event
  forward. That validates marker-based compaction as a pattern independent of
  whether the underlying store is log-based or document-based.
- Truncate/rewind being genuinely destructive (`Vec::drain` followed by full
  overwrite) is a caution, not a pattern to copy: our event-sourced design
  should keep rewind as an appended fact (a recorded "rewound to X" event)
  precisely so that undo-of-undo and audit remain possible -- Zed's design
  shows what is lost when rewind is implemented as in-place mutation instead.
- Zed's subagent model (bounded to one level, first-class sibling row,
  genuine cascade-delete on parent removal, hidden from the top-level list by
  a `parent_session_id` filter rather than by physical separation) is a clean
  reference point for ADR 0031/0035's child-session direction: it argues for
  explicit parent/child linkage as a first-class fact plus list-time
  filtering, and for treating cascade-delete as a deliberate policy decision
  (Zed chose "delete descendants," not "orphan them" -- our design should make
  this choice explicitly rather than by omission).
- The complete absence of any content search index (title-only in-memory
  fuzzy match) is a useful negative data point: a metadata/read-model table
  by itself does not give you real search, and any product wanting
  cross-thread content search needs a deliberately-built and
  deliberately-kept-consistent index (FTS or otherwise) -- Zed simply hasn't
  built one for this feature yet.

## Open questions

- What happens when an **old** Zed binary opens a database that a **newer**
  Zed binary has already migrated forward? The ratchet mechanism
  (`sqlez::migrations.rs`) only checks the migrations the old binary knows
  about against what's recorded, and skips or fails based on text match for
  *those* steps -- we did not find explicit handling (e.g., a schema-version
  ceiling check) for the case where the stored ledger contains *later* steps
  the old binary's `MIGRATIONS` array doesn't include at all.
- Does any code path reconcile a worktree rename/move that happened while
  Zed was **not running** (i.e. at next-launch load time, rather than via
  the live `WorktreePathsChanged` event)? We found the live-event
  reconciliation path (`sidebar.rs::move_entry_paths`,
  `agent_panel.rs::update_thread_work_dirs`) but no evidence of an
  offline/startup reconciliation check.
- Is the metadata-delete and content-delete pair
  (`ThreadMetadataStore::delete` / `ThreadsDatabase::delete_thread`) ever
  made atomic across the two separate SQLite files, or is a crash between
  the two calls an accepted (if rare) inconsistency window? We found no
  cross-database transaction.
- Is `ThreadsDatabase::delete_thread`'s subagent cascade walk-and-delete
  sequence protected by an explicit SQL transaction, or only by holding the
  process-local `Mutex<Connection>` for the duration? We saw the mutex guard
  but no `BEGIN`/`COMMIT` in the read excerpt.
- We treated "no cross-thread content search index" as a well-supported
  finding based on keyword greps across `sidebar.rs`, `thread_metadata_store.rs`,
  and `thread_search_bar.rs`, but did not read every line of
  `crates/agent_ui/src/conversation_view/thread_search_bar.rs` end-to-end --
  it's possible that file implements something beyond in-conversation
  find-in-page that our search missed.
- We did not read `crates/agent/src/legacy_thread.rs` in full, so the exact
  field-level shape of the pre-`0.3.0` `SerializedThread` schema (and
  therefore precisely what `upgrade_from_agent_1` translates) is inferred
  from the call site, not confirmed against the legacy struct definition
  itself.
- We did not read `crates/agent_ui/src/threads_archive_view.rs` or
  `crates/agent_ui/src/thread_worktree_archive.rs` in depth, so the exact
  restore-flow mechanics for an `ArchivedGitWorktree` (how a restored
  worktree is reattached, what happens if the main repo path no longer
  exists beyond the doc-comment's stated failure mode) are not verified
  beyond the `ArchivedGitWorktree` struct's own field-level doc comments.
- Whether any deployment actually relies on the `open_fallback_db` in-memory
  path in practice, and therefore how often thread data is silently discarded
  at process exit, is not observable from source. The trigger conditions are
  confirmed; their real-world frequency is not.
- Whether the two independent `Uuid::new_v4()` mint sites for `acp::SessionId`
  (`crates/agent/src/thread.rs:1380`, `crates/agent_ui/src/agent_panel.rs:3819`)
  are meant to be the only two, or whether the second is a duplication of the
  first, is not stated anywhere we could find. Both produce UUIDv4, so the
  scheme is unambiguous even if the ownership of it is not.

# Hermes (Nous Research): how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of
[NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) at
commit `d9165d7a678d4105f42921a7fc1886df3804531b` (committed 2026-07-23), the
source of Nous Research's self-improving terminal/gateway agent ("Hermes"). Every
citation below was re-verified against this exact commit on 2026-07-23 (working
tree clean, `origin/main`). The session subsystem is not a pluggable adapter: it
is one concrete class, `SessionDB`, in the repo-root module `hermes_state.py`
(~442 KB, the single source of truth for persistence), with call sites in
`run_agent.py` (the agent write/flush path), `gateway/session.py` (the gateway
transcript queue), `hermes_cli/*` (slash commands, resume, listing), and
`tools/async_delegation.py` (durable subagent-completion outbox). Citations use
repo-relative `path:line` shorthand.

## The storage model

The durable session is a **single per-profile SQLite database**,
`{HERMES_HOME}/state.db` (`hermes_state.py:154`,
`DEFAULT_DB_PATH = get_hermes_home() / "state.db"`). A session is one **row** in
the `sessions` table (`hermes_state.py:999-1048`) plus an ordered set of **rows**
in the `messages` table (`hermes_state.py:1050-1072`), joined by
`messages.session_id → sessions.id`. The class docstring states it plainly:
"SQLite-backed session storage with FTS5 search. Thread-safe for the common
gateway pattern (multiple reader threads, single writer via WAL mode)"
(`hermes_state.py:1510-1515`).

This is best described as **session-as-row-set** (a relational record with a
child message table), not session-as-log and not session-as-directory. The
transcript is *append-dominant but mutable*: messages are almost always added by
`INSERT` (`append_message`, `hermes_state.py:5627`), but the durable record is
edited in place through three flag columns rather than being a pure immutable
log:

- `active INTEGER NOT NULL DEFAULT 1` — live vs. soft-deleted
  (`hermes_state.py:1069`).
- `compacted INTEGER NOT NULL DEFAULT 0` — summarized-away vs. normal
  (`hermes_state.py:1070`).

The two flags encode three durable states, spelled out in the code:
`active=1` (live), `active=0, compacted=0` ("the user took it back" — rewind/undo),
and `active=0, compacted=1` ("summarized away" — compaction-archived)
(`hermes_state.py:5941-5946`, `7151-7156`). Rewind and compaction therefore
mutate existing rows (`UPDATE ... SET active=0`) rather than appending markers, so
the on-disk row set is authoritative and the "log" is reconstructed by filtering
flags at read time.

What is authoritative vs. derived:

- **Authoritative**: the `sessions` and `messages` rows. Nothing is rebuilt from
  an external source of truth; the rows *are* the truth.
- **Derived / rebuildable**: the FTS5 indexes (`messages_fts`,
  `messages_fts_trigram`, `messages_fts_cjk`) are external-content indexes kept in
  sync by triggers and fully rebuildable from `messages` via the FTS5 `'rebuild'`
  command (`rebuild_fts`, `hermes_state.py:9570`; runtime auto-repair,
  `hermes_state.py:2083-2130`). The per-session denormalized counters on the
  `sessions` row (`message_count`, `tool_call_count`, token totals) are
  maintained at write time and are a projection of the message rows.
- **Sidecar / ephemeral**: gateway request dumps and per-session log files
  (`{session_id}.json`, `{session_id}.jsonl`, `request_dump_{session_id}_*.json`,
  `logs_dir/session_{sid}.json`) are debug artifacts the store deletes on session
  deletion (`_remove_session_files`, `hermes_state.py:8437-8461`;
  `run_agent.py:2768`), never the source of truth.

## Keying and identity

- **Store scope**: one SQLite file per Hermes home/profile. Profiles are
  separated by `HERMES_HOME` (`get_hermes_home`, `hermes_constants.py:106-131`);
  there is no cwd- or project-encoded path in the key. All sessions for a profile
  — CLI, gateway platforms, subagents, branches — live in the same `state.db` and
  are distinguished by columns, not directories.
- **Primary key**: `sessions.id TEXT PRIMARY KEY` (`hermes_state.py:1000`).
- **Id minting**: client-supplied, generated as
  `f"{timestamp_str}_{short_uuid}"` where `timestamp_str =
  strftime("%Y%m%d_%H%M%S")` and `short_uuid = uuid.uuid4().hex[:6]`
  (`cli.py:4208-4210`, `cli.py:7393`, branch path
  `hermes_cli/cli_commands_mixin.py:900-903`). The scheme encodes **wall-clock
  ordering** in the lexical prefix but is not a UUIDv7 and carries only 6 hex of
  entropy (~24 bits), so ordering is second-resolution and collisions within the
  same second are theoretically possible. Resume reuses the existing id verbatim
  (`cli.py:4204-4206`).
- **Secondary identity for gateway routing**: `source`, `user_id`,
  `session_key`, `chat_id`, `chat_type`, `thread_id`, `origin_json`
  (`hermes_state.py:1001-1008`) address a session by its external messaging
  origin (Telegram/Discord/Slack/etc.). `gateway_routing` (a separate table,
  `hermes_state.py:1101-1107`) maps a `(scope, session_key)` to a JSON routing
  entry, and `find_session_by_origin` / `find_latest_gateway_session_for_peer`
  (`hermes_state.py:3625`, `3724`) resolve a platform peer to its live session id.
- **Listing scope**: global within the profile DB. `list_sessions_rich`
  (`hermes_state.py:5171`) queries the whole `sessions` table with optional
  `source` / `cwd_prefix` filters; there is no per-directory enumeration. A
  read-only cross-profile aggregation path exists (open another profile's
  `state.db` `mode=ro`, `hermes_state.py:1574-1591`).
- **Relocation / rename reconciliation**: `cwd`, `git_branch`,
  `git_repo_root` are plain columns updated in place (`update_session_cwd`,
  `hermes_state.py:3865`); moving a working directory does not change identity
  because the key is not path-derived. Renames are `set_session_title`
  (`hermes_state.py:4909`), a metadata update, not an identity change.

## The store interface

There is **no pluggable store protocol**; `SessionDB` is a concrete class and
callers invoke its methods directly. The reconstructed operational contract (every
method opens its own cursor; all mutations go through `_execute_write`, which wraps
`BEGIN IMMEDIATE` + commit + jittered retry, `hermes_state.py:1997-2065`) is:

Session lifecycle:
- `create_session(session_id, source, **kwargs) -> str`
  (`hermes_state.py:3441`) — INSERT a `sessions` row (via `_insert_session_row`,
  `hermes_state.py:3294`, an UPSERT with `ON CONFLICT` merge).
- `ensure_session(...)` (`hermes_state.py:4587`) — idempotent
  create-or-touch used before the first append.
- `end_session(session_id, end_reason)` / `reopen_session(session_id)`
  (`hermes_state.py:3789`, `3807`) — set/clear `ended_at`, `end_reason`.
- `promote_to_session_reset(...)` (`hermes_state.py:3816`) — compression-continuation bookkeeping.
- `update_session_cwd / _meta / _model / _billing_route / set_session_title /
  set_session_archived` (`hermes_state.py:3865, 4244, 4272, 4286, 4909, 4937`) —
  in-place metadata mutation.

Message write:
- `append_message(session_id, role, content, ...) -> int`
  (`hermes_state.py:5627`) — INSERT one message row, return autoincrement id,
  bump session counters. The single normal write path.
- `replace_messages(session_id, messages, active_only=False)`
  (`hermes_state.py:5851`) — **destructive** DELETE-then-INSERT of the whole (or
  live-only) transcript, one transaction. Used by /retry, /undo, /compress.
- `archive_and_compact(session_id, compacted_messages) -> int`
  (`hermes_state.py:5913`) — **non-destructive** soft-archive
  (`active=0, compacted=1`) of live rows then INSERT the compacted set.
- `set_latest_user_api_content(...)` (`hermes_state.py:5965`) — backfill the
  `api_content` sidecar onto the newest active user row.

Message read:
- `get_messages(session_id, include_inactive=False, limit, offset)`
  (`hermes_state.py:5997`) — ordered by autoincrement `id` (insertion order).
- `get_messages_as_conversation(session_id, include_ancestors, include_inactive,
  repair_alternation)` (`hermes_state.py:6334`) — OpenAI role/content format for
  replay.
- `resolve_resume_session_id(session_id) -> str` (`hermes_state.py:6245`) —
  redirect a resume target forward across the compression-continuation chain.
- `get_resume_conversations / get_ancestor_display_prefix / get_conversation_root
  / _session_lineage_root_to_tip / get_compression_lineage`
  (`hermes_state.py:6512, 6561, 6602, 6616, 7942`) — lineage walks.

History mutation (retroactive):
- `rewind_to_message(session_id, target_message_id) -> dict`
  (`hermes_state.py:6656`) — soft-delete (`active=0`) every row with
  `id >= target`, bump `rewind_count`.
- `restore_rewound(session_id, since_message_id) -> int`
  (`hermes_state.py:6743`) — undo a rewind (flip back to `active=1`).
- `clear_messages(session_id)` (`hermes_state.py:8424`) — DELETE all rows for a session.

Listing / search / count:
- `list_sessions_rich(...) -> list` (`hermes_state.py:5171`),
  `search_sessions(...)` (`hermes_state.py:7794`),
  `search_sessions_by_id(...)` (`hermes_state.py:7747`),
  `search_messages(query, ...)` (`hermes_state.py:7049`, FTS5),
  `session_count / message_count` (`hermes_state.py:7834, 7892`),
  `distinct_session_cwds` (`hermes_state.py:5145`).

Deletion / retention:
- `delete_session(session_id, sessions_dir)` (`hermes_state.py:8463`),
  `delete_session_if_empty` (`hermes_state.py:8504`),
  `delete_sessions([...])` (`hermes_state.py:8546`),
  `delete_empty_sessions` (`hermes_state.py:8654`),
  `archive_sessions` (`hermes_state.py:8869`),
  `prune_sessions(older_than_days=90, ...)` (`hermes_state.py:8895`).

Import / export:
- `export_session / export_session_lineage / export_all`
  (`hermes_state.py:7987, 7995, 8015`) — dict / JSONL shapes.
- `import_sessions([...]) -> dict` (`hermes_state.py:8097`) — bounded restore
  (caps at `hermes_state.py:1545-1549`).

Consistency guarantees for all of the above: single writer connection guarded by a
Python `threading.Lock` plus SQLite `BEGIN IMMEDIATE` (`hermes_state.py:2018-2019`);
ordering by autoincrement `id`; no expected-version precondition on any operation.

## Write and append path (ordering, durability, concurrency, delivery)

- **Commit shape**: every new turn is an `INSERT` into `messages` plus an
  `UPDATE sessions SET message_count = message_count + 1` (and
  `tool_call_count`), inside one `_execute_write` transaction
  (`append_message`, `hermes_state.py:5709-5754`). Full rewrites
  (`replace_messages`) and compaction (`archive_and_compact`) are the only
  non-append writes and are single transactions.
- **Ordering**: positional by `messages.id INTEGER PRIMARY KEY AUTOINCREMENT`
  (`hermes_state.py:1051`). Reads order by `id`, not `timestamp`, deliberately —
  "Ordered by AUTOINCREMENT id (true insertion order) rather than timestamp — see
  c03acca50 for the WSL2 clock-regression rationale" (`hermes_state.py:6011-6012`).
  `timestamp REAL` is stored but advisory.
- **Durability / atomicity**: WAL journal mode with a DELETE-mode fallback for
  WAL-incompatible filesystems (`apply_wal_with_fallback`,
  `hermes_state.py:464-527`); on macOS a checkpoint barrier and
  `synchronous=FULL` are enforced (`hermes_state.py:505-514`). `BEGIN IMMEDIATE`
  takes the write lock at transaction start so contention surfaces immediately
  (`hermes_state.py:2007-2008`). A malformed/corrupt-FTS write triggers a one-shot
  in-place FTS rebuild and retry, preserving canonical message rows
  (`hermes_state.py:2049-2061`, `2083-2130`). An offline schema-repair path backs
  up and de-duplicates a malformed `sqlite_master` (`hermes_state.py:827-991`,
  invoked from `__init__` at `hermes_state.py:1616-1628`).
- **Concurrency**: designed as **single-writer-per-DB, multi-reader** ("single
  writer via WAL mode", `hermes_state.py:1514`). Multiple processes (gateway + CLI
  sessions + worktree agents) share one `state.db`, so contention is handled at
  the application level: SQLite timeout is kept at 1s and retries use random
  20–150 ms jitter over up to 15 attempts to break the convoy effect
  (`hermes_state.py:1517-1528`, `2036-2048`). There is **no optimistic-concurrency
  / expected-position precondition** anywhere; correctness under concurrent
  writers to the *same* session relies on the app keeping one logical writer per
  session.
- **Delivery semantics**: from the live agent, best-effort. The flush
  (`_flush_messages_to_session_db_unlocked`, `run_agent.py:1871-2096`) walks the
  in-memory conversation, skips already-persisted dicts, appends the rest, and
  swallows exceptions ("Session DB append_message failed: %s",
  `run_agent.py:2095-2096`). Idempotence/dedup is an **in-memory** intrinsic
  marker `_DB_PERSISTED_MARKER` stamped on each written dict
  (`run_agent.py:1878-1889, 1970-1976, 2089`), *not* a durable dedup key — there is
  no unique constraint on message content, so a re-flush after a process restart
  that lost the marker could duplicate rows. The gateway path adds a bounded
  **in-memory retry queue** per session (`_dirty_transcripts`,
  `_MAX_PENDING_PER_SESSION`; drops oldest on overflow) with retry-on-failure and
  FTS-rebuild recovery (`gateway/session.py:2600-2687`) — at-least-once with a
  bounded buffer, not a durable outbox.

## Read and resume path

- Resume reads the **durable store** directly; there is no separate local cache.
  `--resume {id}` reuses the id (`cli.py:4204-4206`), then the conversation is
  rebuilt by `get_messages_as_conversation(session_id, ...)`
  (`hermes_state.py:6334`), a single ordered `SELECT` over `active=1` message rows
  transformed into OpenAI role/content dicts.
- **Full ordered read**, not incremental cursor replay: the whole active
  transcript is materialized on resume (the agent then tracks
  `_last_flushed_db_idx` to know which tail is new going forward,
  `run_agent.py:2094`).
- Resume redirects forward across compression continuations:
  `resolve_resume_session_id` follows `get_compression_tip` and the
  `parent_session_id` chain (depth cap 32) to the descendant holding the live
  messages, so resuming a compressed parent lands on the continuation child
  (`hermes_state.py:6245-6332`).
- `repair_alternation=True` runs `repair_message_sequence` over the loaded list
  for live replay so a durable `user;user` gap doesn't re-trigger repair every
  request (`hermes_state.py:6349-6355`).
- Pagination/bounds: `get_messages` accepts `limit`/`offset`
  (`hermes_state.py:5997-6002`); import caps bound restore size
  (`hermes_state.py:1545-1549`), but there is no hard cap on live transcript size
  — growth is managed by compaction, below.

## Listing, summaries, and search

- **Listing** is a single SQL query with correlated subqueries, not N queries and
  not a directory scan (`list_sessions_rich`, "Uses a single query with correlated
  subqueries instead of N+2 queries", `hermes_state.py:5194`). Child sessions
  (subagent runs, compression continuations) are hidden by default; a recursive
  CTE walks compression-continuation edges so `LIMIT`/`OFFSET` still apply and one
  logical conversation surfaces as one row projected to its live tip
  (`hermes_state.py:5196-5213`). `compact_rows=True` omits the `system_prompt`
  blob from the SELECT to avoid copying tens of KB per row
  (`hermes_state.py:5221-5225`). No scale numbers are quoted in-source.
- **Summary sidecar**: there is no separate summary file — the `sessions` row
  *is* the denormalized read model. It carries `title`, `started_at`, `ended_at`,
  `message_count`, `tool_call_count`, token totals, cost fields, `cwd`,
  `git_branch`, `git_repo_root`, `model`, `profile_name`, `archived`,
  `rewind_count`, handoff state (`hermes_state.py:999-1048`). Counters are kept
  consistent by being updated in the same transaction as the message insert
  (`hermes_state.py:5741-5751`).
- **Search** is a first-class, separately-indexed FTS5 subsystem with three
  virtual tables: `messages_fts` (unicode61), `messages_fts_trigram` (substring /
  CJK), and `messages_fts_cjk` (a loadable `cjk_unicode61` bigram tokenizer from
  `~/.hermes/lib/libfts5_cjk.so`) (`hermes_state.py:1186-1360`). They are
  **external-content** indexes synced by INSERT/DELETE/UPDATE triggers on
  `messages`, so they never store the canonical text and are fully rebuildable
  (`rebuild_fts`, `hermes_state.py:9570`). Query routing chooses FTS5 / trigram /
  CJK / LIKE-scan by script and token length, with slow-query logging
  (`_describe_search_path`, `hermes_state.py:7097-7117`; `search_messages`
  instrumentation, `hermes_state.py:7060-7095`). Rewound rows
  (`active=0, compacted=0`) are excluded; compaction-archived rows
  (`active=0, compacted=1`) remain discoverable
  (`hermes_state.py:7151-7156`). Index maintenance is amortized: PASSIVE WAL
  checkpoint every 50 writes and FTS5 `optimize` every 1000 writes to stop
  segment fragmentation from lengthening write-lock holds
  (`hermes_state.py:1529-1540`, `2031-2034`).
- **FTS layout versioning is a separate ratchet** from the schema version (see
  next section): `fts_storage_version` in `state_meta`, advanced only on a fresh
  DB or explicit `hermes sessions optimize-storage`, with legacy DBs left on an
  inline layout until the user opts in (`hermes_state.py:156-165`).

## Entry/message structure and versioning

- **Message row shape** (`hermes_state.py:1050-1072`): envelope-ish flat columns —
  `id` (autoincrement, the ordering + identity field), `session_id`, `role`,
  `content` (TEXT; multimodal lists are JSON-encoded via `_encode_content`),
  `tool_call_id`, `tool_calls` (JSON), `tool_name`, `effect_disposition`,
  `timestamp REAL`, `token_count`, `finish_reason`, `reasoning`,
  `reasoning_content`, `reasoning_details` (JSON), `codex_reasoning_items` /
  `codex_message_items` (JSON — provider-specific reasoning payloads),
  `platform_message_id` (external platform id, e.g. Telegram update_id — distinct
  from the PK), `observed`, `active`, `compacted`, and `api_content` (the exact
  bytes sent to the API when they differ from `content`, a "byte-fidelity sidecar
  for prompt-cache-stable replay", `hermes_state.py:5660-5666`).
- **Store interpretation**: the entry is **not opaque** — the store parses and
  interprets it. It distinguishes message types by `role`/`tool_*`, JSON-encodes
  structured fields, scrubs lone surrogates sqlite3 cannot bind
  (`_scrub_surrogates`), and strips base64 images to a text summary before
  persisting multimodal tool results (`run_agent.py:2051-2064`). The identity /
  dedup field the store relies on is the autoincrement `id`; live-agent dedup
  additionally uses the in-memory `_DB_PERSISTED_MARKER`.
- **Chaining**: messages link to a session by `session_id` and to each other only
  by insertion order (`id`); there is no per-message parent/uuid pointer.
  Cross-session lineage is carried on the `sessions` row via `parent_session_id`
  (FK to `sessions.id`, `hermes_state.py:1013, 1047`) plus JSON markers in
  `model_config` (`_branched_from`, `_delegate_from`).
- **Schema evolution**: `SCHEMA_VERSION = 23` (`hermes_state.py:156`). Column
  additions are **declarative and self-healing**, not version-gated: on every
  startup `_reconcile_columns` diffs live `PRAGMA table_info` against `SCHEMA_SQL`
  and `ALTER TABLE ... ADD COLUMN`s any missing ones (the Beets/sqlite-utils
  pattern, `hermes_state.py:2812-2854`). The `schema_version` table is retained
  only for row-transforming data migrations (`hermes_state.py:2866-2867`).
  `schema_version` is advanced forward on open for all non-FTS migrations
  (`hermes_state.py:3182-3195`). This is effectively a **one-way forward ratchet**
  (no down-migrations); legacy pre-`active` DBs backfill `active=1`
  (`hermes_state.py:2909`).

## Compaction and history management

- Compaction shrinks the **model-visible** view while keeping the durable record.
  The preferred path, `archive_and_compact`, keeps **one session id for life**
  (#38763): it soft-archives the live turns (`UPDATE ... SET active=0, compacted=1`)
  and inserts the compacted summary set as fresh `active=1` rows, atomically
  (`hermes_state.py:5940-5961`). The live-context load filters `active=1`, so the
  model reloads only the compacted set; the archived pre-compaction turns stay on
  disk, stay in the FTS index (flipping `active` is a content-preserving UPDATE
  that doesn't fire the FTS delete trigger), and stay searchable and recoverable
  via `get_messages(..., include_inactive=True)` (`hermes_state.py:5926-5933`).
- **Compaction is an upstream (agent) concern** that calls the store: the
  summary payload (`compacted_messages`) is produced by
  `trajectory_compressor.py` and handed to `archive_and_compact`. The durable
  artifact it leaves is the flag flip plus the inserted summary rows — an in-place
  soft rewrite, not an external snapshot file or an appended marker line.
- An **older compaction mode still exists**: ending the current session and
  **forking a continuation child** linked by `parent_session_id` with
  `end_reason='compression'` (`resolve_resume_session_id`,
  `hermes_state.py:6245-6287`; lineage via `get_compression_lineage`,
  `hermes_state.py:7942`). The whole chain is one logical conversation for
  listing/export (`export_session_lineage`, `hermes_state.py:7995-8013`).
- Resume behavior across the boundary: normal resume just loads the `active=1`
  set (post-compaction). Crossing back requires `include_inactive=True`, which
  reads the archived rows. There is no fold-time reconstruction from a raw log,
  because there is no raw log — the archived rows are the history.
- `replace_messages` (`hermes_state.py:5851`) is the **destructive** alternative
  used by /retry, /undo, /compress: it DELETEs and reinserts, so it does not
  preserve pre-compaction history and is explicitly warned against for compaction.

## Rewind, checkpoints, and fork

- **Rewind** is a non-destructive, reversible soft-delete, not a destructive edit
  and not an appended marker: `rewind_to_message` flips every row with
  `id >= target` to `active=0`, keeps them on disk "for audit / forensic
  inspection", and bumps `sessions.rewind_count` (`hermes_state.py:6656-6725`).
  It is idempotent on the `active` flag and reversible via `restore_rewound`
  (flip back to `active=1`, `hermes_state.py:6743-6763`; noted as "not wired to a
  slash command in v1"). Rewound rows (`active=0, compacted=0`) are hidden from
  live load and search, distinguishing them from compaction-archived rows.
- **File-state / environment checkpoints**: there is no per-turn file-content
  checkpoint stored *in the session store*. A separate git-native shadow-repo
  checkpoint system lives under `~/.hermes/checkpoints/` with opportunistic
  auto-prune (`cli.py:4195-4198`), outside `state.db`; it is not tied to message
  rows and was not traced to a durable per-turn snapshot in the transcript.
- **Fork / branch** is **copy-plus-lineage**, not a shared-prefix reference. The
  `/branch` command mints a new session id, ends the parent with
  `end_reason='branched'`, creates the child with `parent_session_id` and a
  `model_config._branched_from` marker, then **copies every message row** into the
  new id (including the `api_content` sidecar so the branch's first turn replays
  the parent's exact wire bytes for cache warmth)
  (`hermes_cli/cli_commands_mixin.py:879-974`). Lineage metadata recorded:
  `parent_session_id` + `_branched_from`. Compression continuation is the same
  shape with `end_reason='compression'`; delegate subagents use `_delegate_from`.

## Subagents and nested sessions

- A subagent / delegate is a **first-class sibling session** in the same
  `state.db`, not nested storage and not entries in the parent transcript. It gets
  its own `sessions` row and its own `messages` rows, linked to the parent by
  `parent_session_id` plus a `model_config._delegate_from` marker
  (`tools/delegate_tool.py:1391-1429, 2839, 3016`). The child has its own isolated
  transcript.
- **Async delegation** additionally has a **durable outbox table**,
  `async_delegations` (`hermes_state.py:1116-1135`), which is the closest thing in
  Hermes to an event-sourced delivery log. It records `delegation_id` (PK),
  `origin_session`, `parent_session_id`, `state`, `dispatched_at`/`completed_at`,
  `event_json`/`result_json`, and a full **at-least-once delivery protocol**:
  `delivery_state` (pending/…), `delivery_attempts`, `delivered_at`,
  `delivery_claim` + `delivery_claimed_at` (a claim token for
  exactly-once-per-consumer delivery), and `owner_pid` + `owner_started_at` for
  crash detection (`tools/async_delegation.py:300-426`).
- **Crash reconciliation**: `recover_abandoned_delegations`
  (`tools/async_delegation.py:226-270`) scans `running`/`finalizing` rows, checks
  whether the owner pid is still alive (matching `owner_started_at` to defeat pid
  reuse), and reclassifies dead-owner delegations to `state='unknown'` with a
  synthetic "outcome unknown" result and `delivery_state='pending'`.
  `restore_undelivered_completions` (`tools/async_delegation.py:273-297`)
  re-enqueues durable pending completions on the next process, stamped `restored`
  (in-memory only) and gated on positive ownership so a new session can't adopt a
  dead session's results (#64484).
- **Nesting bound**: not enforced by the store schema (any session may have a
  `parent_session_id`); depth caps live in the delegation tool layer, not
  `SessionDB`.
- **Parent delete cascade**: `delete_session` cascade-deletes *delegate* children
  (`_delegate_from`) so they don't resurface as orphans, but **orphans**
  branch/compression children (`parent_session_id → NULL`) so they stay
  independently accessible (`hermes_state.py:8463-8495`). `prune_sessions`
  orphans out-of-window children rather than cascading
  (`hermes_state.py:8927-8929`).

## Retention, deletion, and multi-host

- **Retention is product-driven, not store-enforced.** There is no background TTL
  sweeper inside `SessionDB`; retention is `prune_sessions(older_than_days=90,
  ...)` (`hermes_state.py:8895`) invoked from the CLI (`hermes_cli/main.py:16027`),
  the web dashboard (`hermes_cli/web_server.py:11676`), and an internal caller at
  `hermes_state.py:9679`. It deletes only *ended* sessions matching a rich filter
  set (age, title/model/branch substrings, message/token/cost bounds, cwd prefix,
  archived tri-state). The `compression_locks` table has an `expires_at` TTL
  (`hermes_state.py:1113`, default 300 s) but that is a lock lease, not session
  retention.
- **Delete behavior**: `delete_session` DELETEs the `messages` then the
  `sessions` row in one transaction (FK-satisfying: cascade delegate children,
  orphan the rest), then best-effort removes on-disk sidecar files outside the
  transaction (`hermes_state.py:8463-8501`). `delete_session_if_empty` guards CLI
  churn (ported from gemini-cli#27770, `hermes_state.py:8504-8543`). Bulk delete is
  one transaction (`delete_sessions`, `hermes_state.py:8546`). `session_model_usage`
  rows cascade via `ON DELETE CASCADE` (`hermes_state.py:1075`).
- **Multi-host / multi-process**: the design assumes a **shared local
  filesystem**, one `state.db` opened by many processes. WAL is the fast path;
  network filesystems (NFS/SMB/FUSE) fall back to DELETE journal mode because WAL's
  shared-memory index needs coherent mmap + reliable POSIX locks that those mounts
  don't provide (`apply_wal_with_fallback`, `hermes_state.py:464-527`). There is a
  guard against a known SQLite WAL-reset corruption bug that refuses to enable WAL
  on fresh files on vulnerable builds (`hermes_state.py:496-498, 530-560`).
  Cross-*host* is not a first-class path: no remote writeback, no distributed
  coordination — remote/serverless deployment (Modal, Daytona, SSH) hibernates a
  single host's filesystem rather than sharing the DB across hosts. A separate
  `hermes_cli/active_sessions.py` tracks open sessions with lease ids and
  atomic-rename temp files (`active_sessions.py:162, 247`) for crash/liveness
  detection, analogous to a pid registry.

## Interop with foreign session stores

Not applicable. Hermes does not read other agent products' native session stores
to discover, import, or resume them. The only "Codex" / "Anthropic" references in
scope are OAuth credential import for provider auth (`hermes_cli/auth_commands.py`),
and provider-specific reasoning payloads persisted in Hermes's own message columns
(`codex_reasoning_items`, `codex_message_items`, `hermes_state.py:1065-1066`) — not
foreign-store ingestion. Hermes's own import/export
(`import_sessions`/`export_session`, `hermes_state.py:8097, 7987`) round-trips its
own JSON/JSONL dump format only.

## What this implies for our Session Store (our inference)

*(Our inference, clearly marked as such.)* Hermes is the corpus's clearest example
of **session-as-mutable-relational-record**, and the least event-sourced of the
products studied: the durable session is a SQLite row plus a child message table
in one per-profile `state.db`, and retroactive operations (rewind, compaction) are
implemented as **in-place soft-flag UPDATEs** on existing rows
(`active`/`compacted`) rather than as appended events interpreted at fold time.
The "log" is reconstructed by *filtering flags*, which is expressive enough for
rewind/undo/compaction-with-audit but gives up the properties our event-sourced
design is built on. Implications:

- **It validates the read-model-as-denormalized-row pattern**: the `sessions` row
  is the listing/summary projection, maintained transactionally with the message
  insert, and search is a cleanly-separate rebuildable FTS index synced by
  triggers — the same authoritative-vs-derived split our design draws, achieved
  without a separate projection store. The FTS layout ratchet
  (`fts_storage_version` tracked independently of the schema version) is a useful
  precedent for versioning a derived index apart from the log.
- **It shows the cost of not being append-only**: concurrency is single-writer by
  convention with no optimistic-concurrency / expected-position precondition, so
  correctness under concurrent same-session writers depends entirely on the app
  serializing them; dedup/idempotence is an **in-memory** marker with no durable
  key, so a crash mid-flush risks duplicate or lost rows (the gateway's bounded
  in-memory retry queue is a mitigation, not a durable outbox). An event store with
  OCC on expected version and idempotent append by event id closes exactly these
  gaps.
- **Fork is copy-plus-lineage** (full row copy + `parent_session_id`), not a
  shared-prefix reference — simple and cache-friendly but O(history) in storage per
  branch; it argues for our design keeping fork as lineage metadata over a shared
  event prefix rather than physical copy.
- **Subagents as first-class sibling sessions with a `parent_session_id` link**
  matches ADR 0031's child-Session direction, and the `async_delegations` table is
  a concrete, well-thought-out **durable outbox with at-least-once delivery,
  claims, and pid-based crash reconciliation** — the one genuinely event-log-shaped
  component here, and a good reference for how we express delegation-completion
  facts and their delivery lifecycle.
- **Retention is a blunt product-invoked `prune_sessions(older_than_days=90)`
  DELETE**, not a data-model-tied lifecycle policy — the same anti-pattern grok's
  mtime janitor showed; our design should tie retention to the log/projection model
  rather than a periodic bulk DELETE of "ended" rows.

## Open questions

- Message-write idempotence across a crash: the durable schema has no unique
  constraint or dedup key on `messages`, and dedup relies on the in-memory
  `_DB_PERSISTED_MARKER` (`run_agent.py:1878-1889`). Whether a process restart
  mid-flush can duplicate or drop rows in practice was not exercised here.
- The session id scheme (`YYYYMMDD_HHMMSS_` + 6 hex, `cli.py:4208-4210`) has only
  ~24 bits of entropy per second; the collision-handling behavior of
  `create_session`/`_insert_session_row`'s UPSERT on a same-second id clash was not
  fully traced.
- The relationship/overlap between the two compaction modes
  (`archive_and_compact` in-place vs. the fork-a-continuation-child mode) and which
  is the current default at what trigger was not pinned to a single dispatch site.
- The git-native checkpoint system under `~/.hermes/checkpoints/`
  (`cli.py:4195-4198`) was not traced to a durable per-turn file-state snapshot
  linked to message rows; whether rewind restores file state or only conversation
  state is unresolved.
- No down-migration / reversible-migration path was found; `schema_version`
  appears to be a strict forward ratchet, but the exact behavior when an older
  binary opens a newer `state.db` was not verified.
- Cross-host concurrent access to one `state.db` (as opposed to one host's
  many processes) is assumed unsupported; no test or code path was found that
  coordinates writers across hosts.

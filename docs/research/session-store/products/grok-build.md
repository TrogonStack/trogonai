# Grok Build: how session transcripts are stored and resumed

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot: local checkout of
[xai-org/grok-build](https://github.com/xai-org/grok-build) at commit
`a5727c5960452e7527a154b25cb5bf00cda0545e` (committed 2026-07-22), the public
Rust source of SpaceXAI's terminal AI coding agent (`grok` CLI/TUI, headless,
and ACP-embedded), synced periodically from the SpaceXAI monorepo. Every
citation below was re-verified against this exact commit on 2026-07-23 (working
tree clean, `origin/main` unchanged). The session subsystem spans
`xai-grok-shell` (persistence core), `xai-grok-workspace` (file state, foreign
sessions), `xai-grok-config` (paths), `xai-grok-shared` (identity),
`xai-sqlite-journal`, `xai-fast-worktree`, and `xai-grok-tools` (subagent
depth cap). Citations use crate-relative shorthand: every crate named here
resolves under `crates/codegen/`, e.g. `xai-grok-shell/src/...` is
`crates/codegen/xai-grok-shell/src/...` in the repo.

## The storage model

A session is a directory of files under the user's grok home (default
`~/.grok`), keyed by working directory and session id:

```text
{grok_home}/sessions/{encoded_cwd}/{session_id}/
```

Inside, one append-only JSONL file is the source of truth and everything
else is derived or sidecar state. The house comment on the rebuild path
states the philosophy outright: "Rebuild the derived `chat_history.jsonl`
cache from `updates.jsonl`, the durable source of truth, so a session
restores from its update stream alone"
(`xai-grok-shell/src/session/storage/mod.rs:126-127`). This is event
sourcing on the filesystem in all but name: an ordered event log per
session, with caches, summaries, and indexes as rebuildable projections.

## Keying and identity

- Session identity is `Info { id: acp::SessionId, cwd: String }`
  (`xai-grok-shared/src/session/info.rs:4-8`); every persistence call takes
  it, and `session_dir(info)` resolves the directory.
- Session ids are minted as UUIDv7 when the ACP client does not supply one:
  `acp::SessionId::new(uuid::Uuid::now_v7().to_string())`
  (`xai-grok-shell/src/agent/mvp_agent/acp_agent.rs:994-996`), so directory
  names are time-ordered.
- The cwd component is URL-encoded when the encoded form fits 255 bytes;
  longer cwds use a compact `{slug}-{blake3_hex16}` form plus a sibling
  `.cwd` file (written with `O_CREAT|O_EXCL` against TOCTOU races) that
  records the original cwd for reverse mapping
  (`xai-grok-config/src/paths.rs:108-187`).
- Sessions whose cwd moves (worktree relocations) are tracked by JSON
  relocation journals under `{grok_home}/relocations/{session_id}.json`; a
  `RelocationView` snapshot resolves each session id to its authoritative
  directory during listing and lookup
  (`xai-grok-shell/src/session/storage/relocation/view.rs:14-106`, built from
  the per-session journals loaded at `view.rs:188-208`). Note:
  this `RelocationJournal` is unrelated to the `xai-sqlite-journal` crate,
  which despite the name is only a SQLite journal-mode selection helper.

## Session directory contents

| File | Format | Role |
| --- | --- | --- |
| `updates.jsonl` (+ `.jsonl.lock`) | JSONL of `SessionUpdateEnvelope {timestamp, method, params}` | Source-of-truth append log of every ACP and xAI-extension session notification |
| `chat_history.jsonl` | JSONL of `ConversationItem` | Derived conversation cache for prompt building; rebuilt from `updates.jsonl` when empty or corrupt |
| `summary.json` (+ `.json.lock`) | Single JSON `Summary` | Denormalized metadata sidecar: titles, timestamps, model, fork lineage, git info, `session_kind`, `hidden`, `chat_format_version` |
| `rewind_points.jsonl` (+ lock) | JSONL of `RewindPoint` | Per-prompt file-state checkpoints holding full before/after file content |
| `compaction_checkpoints/{id}.json` | JSON `CompactionCheckpointFile` | Full compacted-history snapshot per compaction event |
| `compaction_requests/`, `recap_requests/` | JSON per request | Summarizer request/response artifacts kept for offline prompt iteration |
| `compaction/segment_NNN.md`, `INDEX.md` | Markdown | Verbatim history segments, written only in Segments compaction mode |
| `subagents/{subagent_id}/meta.json` (+ `output.json`) | JSON `SubagentMeta` | Parent-to-child link and status pointer; not the child transcript |
| `plan.json`, `plan_mode.json`, `signals.json`, `goal/state.json`, `announcement_state.json` | JSON per concern | Assorted per-session feature state |
| `events.jsonl` | JSONL, tagged `Event` enum | Append-only per-turn analytics log (`EVENT_SCHEMA_VERSION` "1.0", `xai-file-utils`) |

Adjacent, outside the per-session directory:

- `{grok_home}/sessions/session_search.sqlite`: full-text search index.
- `{grok_home}/active_sessions.json` (+ lock + tmp): registry of currently
  open TUI processes for crash detection.
- `~/.grok/worktrees.db`: SQLite worktree registry linking worktrees to
  session ids (`WorktreeKind::Session|Subagent|Fork|...`).

## Append path, ordering, and durability

- Appends to any per-session JSONL take an exclusive `fs2` lock on a
  sibling `<file>.jsonl.lock`. Before writing, the code heals a torn
  trailing line left by a crashed append: "jsonl file has a torn trailing
  line (previous append crashed mid-write?); terminating it before
  appending" (`xai-grok-shell/src/session/storage/jsonl/mod.rs:313`),
  so damage is bounded to one corrupt line that lenient readers skip.
- Full-file rewrites (the only way files shrink) go through a crash-atomic
  temp-file-then-rename path that bypasses the append lock: "a crash /
  `ENOSPC` mid-write can't truncate the existing file"
  (`jsonl/mod.rs:491-496`).
- Two readers exist for the same data: a lenient streaming reader for loads
  (warns and skips malformed lines) and a strict reader used only on the
  rewrite path, which aborts the whole rewrite and preserves the original
  file if any line fails to parse.
- Ordering is positional: line order in `updates.jsonl` is the event order.
  There is no per-entry sequence number and no optimistic-concurrency
  precondition; single-writer-per-session is assumed and enforced socially
  by the active-sessions registry plus file locks.

## Versioning of stored data

- `Summary.chat_format_version: u8` versions the conversation cache format
  (0 = legacy `ChatRequestMessage`, 1 = `ConversationItem`).
- Envelope reads branch on the presence of a `method` key to accept legacy
  pre-envelope lines: "Backwards compatibility: old format without envelope
  wrapper" (`storage/mod.rs:650`; a second legacy branch is at
  `storage/mod.rs:683`).
- Elsewhere evolution is additive serde: `#[serde(default)]` fields,
  `#[serde(other)] Unknown` catch-alls, and `FlexiblePath` (relative
  preferred, absolute accepted for older sessions). Rewind points have no
  format-version field at all.

## Listing, summaries, and search

- Listing is a filesystem scan, not an index: every list walks
  `{sessions_root}/{cwd}/{session}/summary.json` via `RelocationView`,
  deserializes `Summary`, filters hidden sessions, and sorts by last
  activity. The one fast path stats mtimes first and parses only the top N:
  "On a machine with ~12K sessions this reduces cold-boot workspace_list
  from ~3s to ~200ms" (`jsonl/mod.rs:196-197`). A Criterion bench models 3,000
  workspaces and 9,864 summaries.
- `Summary` is a wide denormalized struct (title and `title_is_manual`,
  created/updated/last-active timestamps, `current_model_id`, message
  counts, fork lineage, git root/remotes/branch/commit, `session_kind`,
  `hidden`, sandbox profile, reasoning effort). There is no tags field.
- Search is architecturally separate and genuinely indexed: SQLite FTS5 in
  `session_search.sqlite` (`meta` k/v, `session_docs`, content-synced
  `session_docs_fts`), bootstrapped on first search, incrementally updated
  by a debounced `notify_session_updated` hook on save/rename, deduped by
  `content_hash = blake3(title + \0 + content)`. Schema migration is a
  one-way ratchet (drop and rebuild only on upgrade) so concurrently
  running grok binary generations can share the file; every search
  re-verifies the on-disk bootstrap marker because a peer process "may wipe
  or downgrade it" (`search_fts.rs`, `search.rs`). A delta-indexing path
  keyed on a persisted byte offset into `updates.jsonl` is implemented but
  not yet wired; every reindex today rescans the file. An optional,
  default-off feature zstd-compresses the whole index to GCS and installs a
  remote copy on startup, with an acknowledged incomplete staleness check.
- Admin: rename patches `summary.json` under its lock and reindexes FTS;
  delete is remote-first when a writeback backend applies (HTTP 404 treated
  as success), then local `remove_dir_all` plus FTS eviction. The CLI
  deliberately locates delete targets by id with no cwd filter.

## Compaction

Compaction shrinks the model-visible view, never the durable log:

- `updates.jsonl` is append-only through compaction. Each compaction
  appends a lightweight `CompactionCheckpoint` marker line (checkpoint id,
  `prompt_index_at_compaction`, schema_version, file pointer) and writes
  the full compacted conversation to a separate
  `compaction_checkpoints/{checkpoint_id}.json`: "writes the compacted
  history to a separate file and records a `CompactionCheckpoint` marker in
  `updates.jsonl`" (`compaction.rs:2152-2153`).
- Only `chat_history.jsonl` shrinks: it is atomically rewritten with the
  compacted item list ("Replacing chat history (compaction)").
- Normal resume just loads the already-compacted `chat_history.jsonl`.
  Rewinding past a compaction boundary reconstructs from raw
  `updates.jsonl` plus the checkpoint file, which is also required to
  recover the historical `original_user_info`; if the checkpoint file is
  missing the rewind fails rather than proceeding with wrong data ("Cannot
  safely rewind past the compaction point.", `helpers/replay.rs`).
- Triggers: manual `/compact` (optionally with user guidance text), a
  threshold check against estimated tokens, a post-tool-call preflight
  overflow check, and a reactive trigger off model context-length errors.
  A two-pass mechanism speculatively summarizes ~95% of history in the
  background before the threshold is crossed (prefire), then a second pass
  folds in the tail; it is a latency optimization and changes no on-disk
  artifacts.
- Fork interplay: `Summary.inherited_prefix_len` marks the conversation
  prefix copied from the parent; "During compaction, items below this index
  are preserved as-is (the 'inherited prefix'). Only items after this
  boundary are summarized" (`persistence.rs:855-860`). If preserving the
  prefix would keep the fork over the auto-compact threshold, the prefix is
  released permanently (sticky `prefix_released`).

## Rewind and file-state checkpoints

- Every prompt boundary is implicitly a checkpoint: "Every prompt is a
  checkpoint, the list always contains `[0, 1, ..., N-1]` where N is the
  current prompt_index" (`acp_session_impl/rewind.rs:11-34`).
- Conversation rewind appends a `RewindMarker` to `updates.jsonl`; nothing
  is deleted. Replay applies dead-branch filtering: content before the
  rewind target survives, content between the rewound turn and the marker
  is dropped, content after the marker is included. The search indexer
  applies the same semantics.
- File-state checkpoints (`rewind_points.jsonl`) store the full literal
  text of every touched file, twice per prompt (before and after maps),
  with no diffing, hashing, dedup, or compression. The file "can be
  hundreds of MB" (`file_state.rs:382-383`), so resume skips it entirely
  and loads lazily on an actual rewind; the rewind picker scans metadata
  with a visitor that counts map entries without allocating content.
- Rewind is the only shrink path: full rewind truncates points at or after
  the target via atomic rewrite; conversation-only rewind folds later
  points into the prior one (earliest before-snapshot wins, latest
  after-snapshot wins) so file-undo history is preserved.
- A git-native checkpoint domain (`GitCheckpointStore`: HEAD SHA plus
  staged paths per prompt) exists behind a default-off env flag but is held
  only in memory and does not survive restart.

## Fork

`ForkSessionRequest` carries source session and cwd, optional new session
id, model, target prompt index, and session kind. Forking copies
`chat_history.jsonl`, `updates.jsonl`, and plan state to the new session
directory, and the child `Summary` records `parent_session_id`,
`forked_at`, `fork_context_source`, `fork_parent_prompt_id`, and
`inherited_prefix_len` (`session/fork.rs:15-46`,
`persistence.rs:819-860`). Fork is copy-plus-lineage, not a shared-prefix
reference.

## Subagent sessions

- A subagent gets its own full session directory under the standard
  cwd-keyed scheme (a sibling of the parent when they share a cwd), with
  its own `updates.jsonl`, `chat_history.jsonl`, and `summary.json`. It
  does not write into the parent's transcript.
- `Summary.session_kind` is stamped `"subagent"`, `"subagent_fork"`, or
  `"subagent_resume"`, and `is_hidden()` defaults to true for any
  `session_kind` starting with `"subagent"` (`persistence.rs:977-982`), so
  subagent sessions are excluded from listing, the roster, and FTS
  indexing.
- The durable parent-child link is
  `{parent_session_dir}/subagents/{subagent_id}/meta.json` (`SubagentMeta`:
  ids, cwd, worktree, model, status running/completed/failed/cancelled).
  Nesting is hard-capped at depth 1: "Subagents cannot spawn further
  subagents" (`xai-grok-tools/.../task/mod.rs:29-31`).
- Crash recovery on resume: replay scans the rewind-filtered timeline for
  `SubagentSpawned` without a matching `SubagentFinished`, unions that with
  on-disk `meta.json` entries still marked running, then either re-emits
  the real result, flips the meta to cancelled and emits a synthetic
  finish, or re-emits an already-terminal status
  (`reconcile_orphaned_subagents`, `subagent/mod.rs:2878-2985`).
- Deleting the parent removes only the parent's directory (including the
  `subagents/` pointers) and orphans the hidden child directories; rewind
  likewise leaves child directories and worktrees behind. No GC for
  orphaned subagent session directories was found (subagent worktrees do
  have age-based GC in the worktree registry).

## Crash detection, multi-process, and multi-host

- `~/.grok/active_sessions.json` tracks open TUI processes (session id,
  pid, cwd, opened_at) under an exclusive lock with atomic writes: "Clean
  exit removes the entry; crash leaves it behind"
  (`active_sessions.rs:1-3`); next launch partitions out entries whose pid
  is dead.
- The `xai-sqlite-journal` crate picks WAL on local filesystems and a
  TRUNCATE rollback journal on network filesystems (WAL's mmap'd `-shm`
  index "relies on coherent shared memory plus reliable POSIX locks,
  guarantees network filesystems do not provide"), applies a 5s busy
  timeout, and on network mounts renames each SQLite DB to a per-host
  sibling so no peer host can flip it back to WAL. All SQLite stores here
  are rebuildable indexes, never the transcript. A
  `GROK_SQLITE_JOURNAL_MODE` env var is the field kill-switch.
- Remote lanes exist but are secondary: a writeback backend for session
  data (remote-first delete and rename sync), a remote session registry
  merged into unified listing (local wins on collisions, source becomes
  "both"), and the optional GCS sync of the search index.

## Foreign session stores (reading other products)

`xai-grok-workspace/src/foreign_sessions/` does bounded, read-only,
metadata-only discovery of other agents' native session stores to populate
grok's session picker; it never converts foreign transcripts into its own
format:

- **Claude Code**: reads `$CLAUDE_CONFIG_DIR` or
  `~/.claude/projects/<sanitized-cwd>/<uuid>.jsonl`, reconstructing the
  sanitization (non-alphanumeric to `-`, 200-char cap) and scanning the
  repo's linked worktrees too. Titles fall back through `customTitle`,
  `aiTitle`, `lastPrompt`, `summary`, then first real user prompt, reading
  a bounded head window (up to 4 MiB) and 64 KiB tail; transcripts whose
  first line contains `"isSidechain":true` are excluded.
- **Codex** (CLI/VSCode/Atlas/ChatGPT): reads `~/.codex`'s
  `state_<generation>.sqlite` `threads` table with a schema-adaptive query
  (columns probed via `PRAGMA table_info`), falling back to rollout files
  `sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl(.zst)` whose first
  record is `session_meta`. Timestamps below 2020-01-01 in ms are treated
  as legacy seconds. zstd reads are bounded (window log cap, single frame,
  size caps) so crafted files fail closed.
- **Cursor**: enum and UI plumbing only; the scanner closures are
  hardcoded no-ops.
- All access goes through a capability-gated `ApprovedRoot` (openat-style
  relative opens, `O_NOFOLLOW`, canonical-root containment; reparse-point
  checks on Windows), and foreign SQLite opens are read-only with
  `PRAGMA query_only=ON` inside a rolled-back transaction, and only when
  the DB sits on a local (WAL-classified) filesystem. Scans are bounded
  (50 sessions per tool, 30-day age cap, per-call read budgets), and a
  truncated enumeration reports Incomplete rather than a possibly wrong
  "most recent" answer.
- "Resume" of a foreign session starts a new grok session and injects a
  `/resume-<tool> <native_id>` prompt handled by a bundled skill outside
  this crate; grok's store never imports the foreign transcript body.

## What this implies for our Session Store (our inference)

grok-build is the strongest industry corroboration yet that the durable
session is an append-only event log with derived projections:

- One ordered log per session (`updates.jsonl`) is the source of truth;
  the conversation cache, summary sidecar, and FTS index are all explicitly
  rebuildable projections of it. The design keeps the write path append-only
  even for retroactive semantics: rewind is an appended marker interpreted
  at replay (dead-branch filtering), and compaction is an appended marker
  plus an external snapshot, with replay across the boundary reconstructing
  from the log. These are stream-native solutions to the same problems our
  decider stack solves with events, snapshots, and fold-time interpretation.
- The summary sidecar maintained at write time for listing is the same
  pattern as the Claude Agent SDK's `listSessionSummaries` fold and our
  projections; grok additionally shows the failure mode of skipping a real
  index (full directory scans, mtime stat fast paths, ~12K-session scale
  pain) and of bolting search on as a separately-consistent SQLite index
  with ratcheted schema migrations.
- Concurrency control is advisory file locks plus a pid registry, with no
  expected-position OCC anywhere; multi-host is handled by giving up
  (per-host SQLite files on network mounts, rebuildable indexes only). An
  event-store-backed design gets OCC, multi-writer safety, and multi-host
  access natively, which is precisely the gap this filesystem design works
  around.
- Costs visible at scale here that our design should avoid: full-content
  file snapshots with no dedup (hundreds of MB per session, mitigated only
  by lazy loading), unbounded log growth with retention expressed as a
  blunt per-file mtime janitor (30-day TTL) rather than policy tied to the
  data model, orphaned child sessions after parent deletion, and derived
  caches whose invalidation is manual (`notify_session_updated`).
- Their subagent model (child session as a first-class sibling session
  with a lineage pointer and reconciliation of orphans at resume) matches
  ADR 0031's child-Session direction and argues for explicit parent/child
  lifecycle facts rather than nested storage.

## Open questions

- The stale doc comments claiming subagent transcripts nest under the
  parent directory suggest an abandoned earlier layout; only the pointer
  files nest today.
- No test was found covering the TTL janitor's whole-directory removal
  path, and no server-side retention story for the writeback backend was
  traced.
- `SessionStoreEntry`-style schema evolution is ad hoc (serde defaults,
  legacy-format sniffing); nothing versions `rewind_points.jsonl`.
- Whether parent shutdown/delete/rewind should cascade cancellation to
  still-running subagents is unresolved in the code (currently they run to
  completion independently).
- The delta FTS indexing path (persisted byte offset into `updates.jsonl`)
  is implemented but unwired; its enablement plan is unknown.

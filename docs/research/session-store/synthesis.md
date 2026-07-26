# Synthesis: what the industry means by a "stored session"

Part of Session Store Research.
Nine product dossiers, one question: when an agent product persists,
resumes, lists, and retires a session, what does it actually keep on disk
and how close is that shape to an append-only log with derived
projections? Purpose: extract the invariant core our own event-sourced
Session Store must model, and the axes where products deliberately
diverge. This synthesis is frozen as decision-time input: where a
conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

The through-line is the append-log-vs-mutable-record spectrum. At one end
sit [T3 Code](./products/t3code.md) and [OpenCode](./products/opencode.md)'s
v2 subsystem, whose durable session **is** an event table with rebuildable
SQL projections. At the other sits [Goose](./products/goose.md) and
[Hermes](./products/hermes-agent.md), whose durable session is a mutable
SQLite row that retroactive operations DELETE and re-INSERT or flip flags
on. The CLIs in between, [Claude Agent SDK](./products/claude-agent-sdk.md),
[Codex CLI](./products/codex-cli.md), [Gemini CLI](./products/gemini-cli.md),
and [Grok Build](./products/grok-build.md), converge on append-only JSONL
transcripts with derived read models bolted alongside (a SQLite index for
Codex CLI; JSON sidecars/registries for Claude Agent SDK), which is
directionally the same shape with looser discipline: no expected-version
precondition anywhere in the group, though Codex CLI's SQLite projection
carries a formal rebuild/read-repair cursor contract that the others
lack. [LangGraph](./products/langgraph.md)
is the odd one out structurally: its unit is a parent-linked chain of
immutable state snapshots, not a message transcript, and it is the second
cleanest event-sourcing analog in the corpus after T3 Code.

## Convergence

**1. Nobody stores model-visible context as the durable artifact; a
separate durable log or table always exists, and the model's view is
derived from it.** [Claude Agent SDK](./products/claude-agent-sdk.md): "a
session whose store holds 503 raw entries may return 18 messages from
`getSessionMessages`." [Codex CLI](./products/codex-cli.md): "the live and
persisted histories remain identical" even as `replacement_history`
replaces what the model re-reads. [Grok Build](./products/grok-build.md):
"Rebuild the derived `chat_history.jsonl` cache from `updates.jsonl`, the
durable source of truth." [T3 Code](./products/t3code.md) and
[OpenCode](./products/opencode.md) hold this as policy: "the only
'shrinking' is view-side ... bound what the UI holds, not what is stored."
Even [Goose](./products/goose.md),
the corpus's most mutable store, keeps pre-compaction turns as
`agent_invisible` rows rather than deleting them.

**2. JSONL append-only transcripts are the majority default for CLI
products, and the append discipline is remarkably specific.** [Claude Agent
SDK](./products/claude-agent-sdk.md), [Codex CLI](./products/codex-cli.md),
[Gemini CLI](./products/gemini-cli.md), and [Grok Build](./products/grok-build.md)
all write one line per event/entry to a per-session `.jsonl` file with no
in-place edits on the hot path. Two independently built torn-write defenses
converge on the same fix: Codex repairs the rollout file to be
newline-terminated on open ("the file is repaired to be newline-terminated
so a torn final line cannot corrupt the next append"), and Grok Build heals
"a torn trailing line (previous append crashed mid-write?)" before
appending, the same failure mode, the same remedy, arrived at separately.

**3. Rewind is implemented as an appended marker interpreted at replay, not
a destructive edit, everywhere except the pure-SQL stores.** Claude Agent
SDK's `/rewind` menu is "a non-destructive view operation over an
append-only log." Codex CLI's `ThreadRolledBack` event means "skip the next
N user-turn segments we finalize"; nothing is deleted. Gemini CLI's
`$rewindTo` line means the reducer "deletes it plus everything after" only
from the in-memory fold, not the file. Grok Build: "Every prompt is a
checkpoint, the list always contains [0, 1, ..., N-1]..."; separately, the
dossier notes nothing is deleted and replay applies dead-branch filtering.
T3 Code: "exactly
the append-marker-replayed pattern (as opposed to Hermes's in-place flag
mutation)." OpenCode's v2 `revert.commit` truncates only the *projection*;
"the underlying event rows are not deleted." The two exceptions prove the
axis: Goose's rewind is "a destructive delete" (`truncate_conversation`),
and Hermes's is an in-place flag flip (`active=0`).

**4. Compaction is universally an upstream/agent-loop concern that the
store merely records, never triggers or understands.** [LangGraph](./products/langgraph.md):
"the store neither triggers nor understands it," a summary is just the
next value of a channel. [Claude Agent SDK](./products/claude-agent-sdk.md):
compaction produces "another appended entry," an `isCompactSummary` marker.
[Codex CLI](./products/codex-cli.md) appends a `Compacted` item with
`replacement_history`. [OpenCode](./products/opencode.md): "Compaction is
upstream of the store... but leaves a durable marker in the log." Even
[Goose](./products/goose.md), which rewrites rows for compaction, treats the
summarization call itself as an agent-loop decision the store just
persists the result of.

**5. Fork/branch always mints a new identity; nobody reuses the source
session id.** [Claude Agent SDK](./products/claude-agent-sdk.md)'s
`forkSession` "rewrites every `sessionId` field and remaps message UUIDs...
An adapter-level copy... would produce a transcript that still references
the old session ID, so the SDK does not use one." [Codex CLI](./products/codex-cli.md)
mints a new `thread_id` and stitches lineage via `SessionMeta.forked_from_id`
plus `history_base`. [T3 Code](./products/t3code.md) requires a dedicated
`ThreadForkService` and produces a `thread.forked` event on a new stream.
[Grok Build](./products/grok-build.md): "Fork is copy-plus-lineage, not a
shared-prefix reference." [Goose](./products/goose.md) mints a fresh
`YYYYMMDD_N` id via `copy_session`. Only [LangGraph](./products/langgraph.md)
gets a genuinely cheap fork, because content-addressed channel blobs are
shared by reference across the copied chain.

**6. Subagents are (almost) always a sibling stream/session linked by a
parent pointer, never entries inlined in the parent's transcript.**
[Codex CLI](./products/codex-cli.md): `SessionMeta.parent_thread_id`, "a
first-class sibling thread." [T3 Code](./products/t3code.md): "each
subagent appears as its own thread: openable, inspectable mid-flight,
steerable, and resumable." [OpenCode](./products/opencode.md): "a
first-class sibling session," linked by `parent_id`. [Grok Build](./products/grok-build.md):
its own directory plus a `SubagentMeta` pointer file. [Goose](./products/goose.md):
its own row, linked by `parent_session_id`. [Hermes](./products/hermes-agent.md):
its own row plus a durable delivery outbox (`async_delegations`) for
crash-safe result reconciliation. The sole structural exception is
[Claude Agent SDK](./products/claude-agent-sdk.md), which nests subagent
`.jsonl` files physically *inside* the parent's session directory rather
than as a database-level sibling, still a separate transcript, just a
different addressing scheme.

**7. Cascade-on-delete for subagents is inconsistent and mostly unhandled,
and nobody has a clean answer.** [Codex CLI](./products/codex-cli.md): "a
missing parent produces a 'malformed lineage' error rather than silent
cascade." [Goose](./products/goose.md): "no cascade to children... leaving
a subagent row with a dangling `parent_session_id`." [OpenCode](./products/opencode.md):
"deleting a parent session row does not delete children, they would
orphan." [T3 Code](./products/t3code.md): "No cascade to child threads was
found... children keep their `parentThreadId` and would be orphaned."
[Grok Build](./products/grok-build.md): "No GC for orphaned subagent
session directories was found." This is a convergence in the sense that
every product that has subagents has the *same unresolved gap*.

**8. No product implements true optimistic-concurrency control at the
store's write boundary, and the exceptions that come closest are the two
purest event-sourced designs.** [Claude Agent SDK](./products/claude-agent-sdk.md):
"there is no expected-position precondition on `append`." [Goose](./products/goose.md):
"no expected-version precondition anywhere; there is no CAS." [Hermes](./products/hermes-agent.md):
"there is no optimistic-concurrency / expected-position precondition
anywhere." [Gemini CLI](./products/gemini-cli.md) and [Codex CLI](./products/codex-cli.md)
rely on a single-writer-per-session assumption with no lock. Only
[T3 Code](./products/t3code.md) (a unique `(aggregate_kind, stream_id,
stream_version)` index plus a single-writer command queue) and
[OpenCode](./products/opencode.md) (an explicit expected-seq check on
replay: "Sequence mismatch") give the write path any real
conflict-detection teeth; [LangGraph](./products/langgraph.md)'s unique
`(thread_id, checkpoint_id)` key is unrelated to conflict detection, its
own dossier flags "no expected-version/OCC in the OSS savers" and notes
`put` is last-write-wins on a given checkpoint id.

## Divergence

**A. Append-only log vs mutable row, the central axis.** Pure log:
[T3 Code](./products/t3code.md) ("unambiguously session-as-log
(event-sourced)"), [OpenCode](./products/opencode.md) v2 ("This is
unambiguously session-as-log"), [LangGraph](./products/langgraph.md)
(immutable, id-addressed, parent-linked snapshots). Log-shaped but looser:
[Claude Agent SDK](./products/claude-agent-sdk.md), [Codex CLI](./products/codex-cli.md),
[Gemini CLI](./products/gemini-cli.md), [Grok Build](./products/grok-build.md),
all JSONL, but looser in different ways: tolerated multi-writer
interleaving (Claude Agent SDK), message-level last-write-wins re-appends
(Gemini), no ordinal at all (legacy Codex), or a single-writer-per-session
assumption enforced only socially, via an exclusive per-append file lock
plus a pid registry rather than a store-level contract (Grok Build).
Mutable row: [Goose](./products/goose.md) ("a mutable row plus an
in-place-editable ordered message table... explicitly not session-as-log")
and [Hermes](./products/hermes-agent.md) ("session-as-mutable-relational-
record... the least event-sourced of the products studied"). **Our
service must decide: is the append-only guarantee enforced by the store
(reject any operation that isn't an append), or merely a convention the
caller can violate?** T3 Code and OpenCode enforce it structurally (no
delete/rewrite path exists below the projector); the JSONL products only
enforce it by omission (nothing offers an edit API, but nothing prevents
one either).

**B. Identity minting: client-random UUID vs client time-ordered UUIDv7 vs
server-assigned date+ordinal vs server-assigned monotonic sequence.**
Random, v4-shaped (observed on disk, not documented): [Claude Agent SDK](./products/claude-agent-sdk.md)
session id. Time-ordered UUIDv7/ULID-like, client-minted: [Codex CLI](./products/codex-cli.md)
("Codex-generated thread IDs are UUIDv7, and some use cases rely on that"),
[OpenCode](./products/opencode.md) (`ses_` ids pack timestamp + counter,
bit-inverted for descending sort), [LangGraph](./products/langgraph.md)
(checkpoint id is UUID6, "unique and monotonically increasing, so can be
used for sorting"). Time-ordered UUIDv7, server-assigned as a fallback:
[Grok Build](./products/grok-build.md) (session ids "are minted as UUIDv7
when the ACP client does not supply one," via `uuid::now_v7()`).
Server-assigned, human-legible, low-entropy:
[Goose](./products/goose.md) (`YYYYMMDD_N`, a per-day counter, "not a
UUID... but no location"), [Hermes](./products/hermes-agent.md) session id
(`{timestamp}_{6-hex}`, "~24 bits of entropy, second-resolution collisions
theoretically possible"). Pure sequence, no id semantics at all:
[T3 Code](./products/t3code.md) (`stream_version` per aggregate plus a
global `sequence`) and [OpenCode](./products/opencode.md) (per-aggregate
`seq`). **Divergence to resolve: do we want an id that is sortable by
construction (UUIDv7/ULID) or an id that is opaque and let a separate
sequence column carry order?** The two cleanest event-sourced designs
(T3 Code, OpenCode) use opaque ids for identity and a *separate* strictly
monotonic sequence for order; they do not conflate the two concerns the
way UUIDv7-as-directory-name products do.

**C. Scope of the store: per-project directory vs single global
database.** Directory-per-project, no cross-project store: [Claude Agent
SDK](./products/claude-agent-sdk.md) (`projectKey` flattens the cwd into
the path), [Codex CLI](./products/codex-cli.md) (time-sharded but global
within `$CODEX_HOME`, filtered by `cwd_filters`), [Gemini CLI](./products/gemini-cli.md)
(`projectShortId` directory). Single database, cwd as a plain filter
column: [Goose](./products/goose.md) ("no cwd/project path is encoded into
the key... project_id is just a nullable column"), [Hermes](./products/hermes-agent.md)
(one `state.db` per profile, `cwd` a plain column), [T3 Code](./products/t3code.md)
and [OpenCode](./products/opencode.md) (one DB, `project_id`/`workspace_id`
columns), [Grok Build](./products/grok-build.md) (cwd-encoded directory
path, but a remote registry merges cross-host listings). This directly
determines whether "move the working directory" is a relocation problem
(directory-keyed stores all have bespoke migration/registry code for this:
Claude Agent SDK's `/cd` relocation, Gemini CLI's `ProjectRegistry` plus
`performMigration`, Grok Build's `RelocationJournal`) or a cheap column
update (Goose, Hermes, and T3 Code just `UPDATE working_dir`/`project_id`
on a plain column; OpenCode is the middle case, still appending a
`session.next.moved` event that projects into `directory`/`path`/
`workspace_id` in the same transaction, so relocation stays cheap and
non-migratory without becoming a bare out-of-band UPDATE).

**D. Compaction's durable shape: in-place row rewrite vs external snapshot
file vs pure append marker.** Rewrite in place: [Goose](./products/goose.md)
(`DELETE all rows, re-INSERT` via `replace_conversation`) and
[Hermes](./products/hermes-agent.md) (`UPDATE active=0, compacted=1` then
insert new rows, "a content-preserving UPDATE"). External snapshot plus a
log marker: [Grok Build](./products/grok-build.md) (`CompactionCheckpoint`
marker in `updates.jsonl` plus a full separate `compaction_checkpoints/
{id}.json` file, required to rewind past the boundary, and rewind fails
closed if the file is missing). Pure append, no external file needed:
[Claude Agent SDK](./products/claude-agent-sdk.md) (`isCompactSummary`
entry in the same log), [Codex CLI](./products/codex-cli.md) (`Compacted`
item with `replacement_history` inline), [T3 Code](./products/t3code.md)
(no compaction of the log at all, it is unbounded and grows forever).
**Divergence to resolve: does a compaction boundary require a sidecar
artifact recoverable independently of the log (Grok Build's model, with an
explicit fail-closed on missing sidecar), or is a same-stream marker
sufficient?** The sidecar approach adds a second thing that can go missing;
the same-stream approach keeps one recovery story but grows the log
un-compactably.

**E. Retention: nobody enforces it at the store layer, but who is
*expected* to differs.** Explicitly the caller's job, store provides
mechanism only: [Claude Agent SDK](./products/claude-agent-sdk.md) ("The
SDK never deletes from your store on its own... TTLs, S3 lifecycle
policies... are the adapter's responsibility"), [LangGraph](./products/langgraph.md)
(`prune(strategy=)`, no automatic lifecycle). Product-owned sweep with a
concrete default: [Claude Agent SDK](./products/claude-agent-sdk.md)'s own
CLI (`cleanupPeriodDays`, default 30), [Gemini CLI](./products/gemini-cli.md)
(delete-on-exit-if-not-resumable), [Hermes](./products/hermes-agent.md)
(`prune_sessions(older_than_days=90)`, invoked, not scheduled). No
retention story at all, log grows forever: [T3 Code](./products/t3code.md)
("no retention or log-truncation/snapshotting, the log grows unbounded")
and [OpenCode](./products/opencode.md) ("none found... the log is retained
indefinitely"). **The two purest event-sourced designs are also the two
with zero retention story**, an event-sourced Session Store gets rewind
and audit for free but inherits an explicit obligation to design retention
deliberately, since nothing in the pattern forces it.

**F. Multi-host / multi-writer posture.** Single-host by design, no
coordination: [Codex CLI](./products/codex-cli.md), [Gemini CLI](./products/gemini-cli.md),
[Goose](./products/goose.md) (SQLite write-lock only), [T3 Code](./products/t3code.md)
("the database is never shared across hosts"). Single-host with
network-filesystem awareness: [Hermes](./products/hermes-agent.md) (WAL on
local disks, falls back to DELETE-mode journal on NFS/SMB/FUSE because "WAL's
shared-memory index needs coherent mmap... those mounts don't provide").
Multi-host as a first-class adapter concern, pushed above the core
interface: [Claude Agent SDK](./products/claude-agent-sdk.md) ("Serverless
functions, autoscaled workers, and CI runners don't share a filesystem. A
shared store lets any replica resume any session," with a documented
clock-skew failure mode in the reference S3 adapter). Multi-host avoided
rather than solved: [Grok Build](./products/grok-build.md) (per-host
SQLite files on network mounts, rebuildable indexes only, "concurrency
control is advisory file locks plus a pid registry... multi-host is
handled by giving up," with a remote registry merge as a secondary
best-effort lane). Multi-host as a first-class *designed* protocol:
[OpenCode](./products/opencode.md) (`events.replay` with an `ownerID` +
`strictOwner` guard, `events.claim` to transfer ownership, a per-aggregate
high-water history-fetch API, "a real distributed event-sync design
layered on the same append-only log").
Multi-host via a shared database instance: [LangGraph](./products/langgraph.md)
(Postgres backend, content-addressed blob upserts are conflict-free by
construction). **This is the widest-open axis**: only OpenCode has
actually solved cross-host replication of an event-sourced session on top
of an ownership-claim protocol; everyone else either assumes single-host or
punts the problem to the storage substrate.

**G. What "the store" persists vs what it treats as opaque.** Fully
opaque entries, store is a pure byte-transport: [Claude Agent SDK](./products/claude-agent-sdk.md)
(`SessionStoreEntry` is "a `{ type: string; ... }` object," treated as
opaque JSON by contract). Parsed and validated on every read/write:
[OpenCode](./products/opencode.md) (event `data` "decoded and validated
through Effect Schema on both append and read"), [T3 Code](./products/t3code.md)
(same, via Effect Schema, plus derived `actor_kind`). Partially parsed,
targeted introspection: [Goose](./products/goose.md) ("neither fully
opaque nor a normalized schema, JSON columns with targeted introspection"
via `json_extract`/`json_each`). **Divergence: does the Session Store
validate event payloads against a schema at the storage boundary, or does
it store bytes and leave validation to the caller?** The schema-validating
designs (T3 Code, OpenCode) get validated event shapes at the storage
boundary; T3 Code decodes/validates every event via Effect Schema on
append and read but carries no explicit per-event-type version field,
handling schema evolution additively (defaults for new fields,
pre-decoding transforms for shape changes) rather than by branching on a
version number. The opaque-byte designs (Claude Agent SDK) get an easier
storage contract but push all versioning discipline onto the consuming
application.

## Conceptual models in play

| Model | Exemplars |
| --- | --- |
| session-as-log (append-only, source of truth) | T3 Code, OpenCode (v2) |
| session-as-log-of-immutable-snapshots (event-sourcing-adjacent, unit is a state chain not a message stream) | LangGraph |
| session-as-log with looser discipline (append-only JSONL, no formal OCC contract; projector-contract rigor varies — Codex CLI's SQLite projection has an explicit rebuild/read-repair cursor) | Claude Agent SDK / Claude Code, Codex CLI, Gemini CLI, Grok Build |
| session-as-directory (path/filename encodes identity; scope encoding varies — Codex CLI's path is time-sharded only, with cwd scope applied as a query-time filter, not a path segment) | Claude Agent SDK, Codex CLI, Gemini CLI, Grok Build, OpenCode (legacy) |
| session-as-row (mutable, single record + child table) | Goose, Hermes |
| session-as-mutable-relational-record (flag-mutation instead of events) | Hermes (Goose achieves a similar visibility effect only via a full delete+re-insert rewrite of the message table, not an in-place flag mutation) |
| session-as-document (whole-file rewrite, last-write-wins per id) | Gemini CLI (legacy `.json`) |

These are not mutually exclusive; several products carry two labels at
once (e.g. Claude Agent SDK is both session-as-log and session-as-directory:
the log is the file, and the file's path is the addressing scheme).

## Comparison table

| Product | Durable session is a... | Source of truth | Keying / id scheme | Compaction artifact | Rewind/fork | Append-log closeness |
| --- | --- | --- | --- | --- | --- | --- |
| [Claude Agent SDK](./products/claude-agent-sdk.md) | append-only JSONL transcript | the `.jsonl` file (mirrored, not replaced, by the SDK's `SessionStore`) | client v4-shaped UUID (observed); path = `projectKey/sessionId/subpath` | in-log `isCompactSummary` entry, raw entries retained | rewind = view op over log; fork = `forkSession` rewrites ids into a new key | high, but no OCC precondition and tolerates multi-writer interleave |
| [Codex CLI](./products/codex-cli.md) | append-only JSONL rollout + derived SQLite index | `RolloutLine` log; SQLite is read-repaired from it | client UUIDv7 `thread_id`; filename = timestamp+id, time-sharded dir | in-log `Compacted` item with `replacement_history` | `ThreadRolledBack` marker replay; fork = new `thread_id` + `history_base` prefix pointer | high; explicit CQRS-shaped log+SQLite-projection design |
| [Gemini CLI](./products/gemini-cli.md) | append-only JSONL folded by a replay reducer | the `.jsonl` file; `ConversationRecord` is "the materialized projection, not what is stored line-by-line" | client `promptId`; path = `projectShortId/chats/session-<ts>-<id8>.jsonl` | checkpoint `$set:{messages}` line replaces message set | `$rewindTo` marker, non-destructive; no first-class fork (resume continues same file) | medium; message-level last-write-wins per id, not immutable events |
| [Goose](./products/goose.md) | mutable SQLite row + message table | the `sessions`/`messages` rows themselves | server date+counter `YYYYMMDD_N`; global DB, no path key | `replace_conversation`: DELETE all rows, re-INSERT with visibility flags | `truncate_conversation`, a destructive delete; fork = `copy_session`, full physical copy | low; explicitly "not session-as-log" |
| [Grok Build](./products/grok-build.md) | append-only JSONL (`updates.jsonl`) + derived caches/index | `updates.jsonl`; cache/summary/FTS all rebuildable | server UUIDv7 session id; path = `sessions/{encoded_cwd}/{id}/` | append marker (`CompactionCheckpoint`) plus external snapshot file, fails closed if missing | `RewindMarker` + dead-branch-filter replay; fork = copy files + lineage fields | high; "event sourcing on the filesystem in all but name" |
| [Hermes](./products/hermes-agent.md) | mutable SQLite row + message table | the `sessions`/`messages` rows | client `{timestamp}_{6hex}` id; global per-profile DB | `active=0, compacted=1` in-place flag flip, content-preserving | `rewind_to_message`: soft-delete via flag flip (reversible); fork = `/branch`, full row copy | low; "least event-sourced of the products studied" |
| [LangGraph](./products/langgraph.md) | parent-linked chain of immutable state snapshots | the `checkpoints` table; channel values content-addressed by `(channel, version)` | caller-supplied `thread_id`; checkpoint id = UUID6 | no store-level compaction; `prune`/shallow-saver drop history, `DeltaChannel` snapshots for large channels | rewind = select an older checkpoint (nothing destroyed); fork = new checkpoint sharing ancestor blobs, `copy_thread` | very high; "materially closer to event-sourcing than the transcript products" |
| [OpenCode](./products/opencode.md) | append-only SQLite event log (v2) / mutable JSON-per-path store (legacy) | the `event` table, keyed `(aggregate_id, seq)` | client-minted ULID-like `ses_`/`evt_` ids; per-aggregate `seq` | in-log `compaction.ended` event; model-visible view folds from latest compaction seq | `revert.commit` truncates only the projection, event rows kept; no first-class fork found | very high; "unambiguously session-as-log (event-sourced)" |
| [T3 Code](./products/t3code.md) | append-only SQLite event log | the `orchestration_events` table, keyed `(aggregate_kind, stream_id, stream_version)` | client-supplied `threadId`; server UUIDv4 `eventId`; global `sequence` + per-stream `stream_version` | none; log is never compacted, only view-side caps | `thread.reverted` event filters the projection, log kept; fork = new stream via `ThreadForkService`, O(history) copy | highest in corpus; "the corpus's cleanest event-sourced example" |

## Working definition

> A stored session is an ordered, addressable record of everything that
> happened in one agent run, durable enough to survive a crash, complete
> enough that the model-visible context and every read model (listing,
> search, summary) can be rebuilt from it alone.

Design decisions the evidence forces for our event-sourced Session Store,
each with the industry's answer where one exists:

1. **The append operation must be the only mutation primitive; rewind,
   compaction, and revert are new appended events, never edits or
   deletes.** Industry's answer: T3 Code and OpenCode enforce this
   structurally; every JSONL product does it by convention only; Goose and
   Hermes are the cautionary counterexamples — Goose via DELETE+re-INSERT
   (`replace_conversation`), Hermes via both a destructive DELETE+re-INSERT
   (`replace_messages`, used by /retry, /undo, /compress) and a
   non-destructive flag-flip (`archive_and_compact`) — both flagged in
   their own dossiers as crash-risk and history-loss hazards.

2. **Separate identity from order: an opaque event/session id for
   addressing, a strictly monotonic per-aggregate sequence for ordering.**
   Industry's answer: T3 Code (`stream_version` unique per
   `(aggregate_kind, stream_id)`) and OpenCode (`seq` unique per
   `aggregate_id`) both do this; time-ordered ids (UUIDv7/ULID, used by
   Codex CLI, Grok Build, OpenCode's `ses_`) are a convenience for
   directory/lexical sort, not a substitute for a real sequence.

3. **Require an expected-version precondition on every append (real
   optimistic concurrency), not just a single-writer assumption.**
   Industry's answer: only OpenCode (explicit "Sequence mismatch" check on
   replay) enforces a real caller-supplied precondition; T3 Code gets
   OCC-like protection only implicitly, via a single-writer command queue
   combined with a unique `(aggregate_kind, stream_id, stream_version)`
   index, with no caller-supplied expected-version check at all; the rest
   (Claude Agent SDK, Goose, Hermes, Gemini CLI) admit they have none and
   rely on tolerated multi-writer interleaving or a social single-writer
   convention.

4. **Compaction is upstream: the store persists an event carrying the
   summary/replacement content, it does not trigger, understand, or
   rewrite prior events.** Industry's answer: universal (Convergence #4).
   Decide whether the compaction event also needs an out-of-band recovery
   artifact for rewinding past it (Grok Build's fail-closed
   `compaction_checkpoints/{id}.json`) or whether an in-stream marker
   suffices (Claude Agent SDK, Codex CLI, T3 Code's absence of compaction
   entirely).

5. **Fork always mints a new stream/session id and rewrites identity
   fields in the copied prefix; it must never let a copy reference the
   source id.** Industry's answer: unanimous outside LangGraph (Claude
   Agent SDK's explicit rationale for not using `CopyObject`; Codex CLI's
   `history_base` prefix-pointer as the efficient variant of the same
   rule). Decide whether fork is a physical copy (T3 Code, Goose, Hermes:
   O(history) per fork) or a shared-prefix reference (Codex CLI's
   `history_base`, LangGraph's shared content-addressed blobs); the
   shared-prefix design is strictly cheaper and available to us because
   events are immutable.

6. **Subagents are sibling streams linked by a parent pointer, and cascade
   behavior on parent delete/rewind must be decided explicitly; the
   industry has not decided it.** Industry's answer: convergence on
   sibling-stream-plus-pointer (Convergence #6) but zero consistent answer
   on cascade (Convergence #7); every product either orphans children or
   doesn't say. This is a genuine gap we get to close rather than copy.

7. **Retention and log truncation are not solved by event-sourcing and
   must be designed deliberately, not deferred.** Industry's answer: the
   two purest event-sourced stores (T3 Code, OpenCode) have *no* retention
   story at all and grow unbounded; the JSONL products that do have
   retention treat it as an out-of-store caller responsibility (Claude
   Agent SDK's explicit "the SDK never deletes from your store on its own")
   with concrete but ad hoc defaults (30-90 days) enforced by a sweep, not
   a store primitive.

8. **Multi-host correctness needs an explicit ownership/claim protocol on
   the write path, not an assumption of a shared filesystem.** Industry's
   answer: only OpenCode has one (`events.claim`, `ownerID` +
   `strictOwner` replay guard, per-aggregate high-water history sync);
   everyone else either assumes single-host (Codex CLI, Gemini CLI, Goose,
   T3 Code, Hermes) or pushes the problem to the adapter/storage substrate
   (Claude Agent SDK's pluggable mirror, Grok Build's per-host SQLite
   files, LangGraph's shared Postgres).

9. **Event payloads should be schema-validated at the storage boundary,
   not treated as opaque bytes.** Industry's answer: split. T3 Code and
   OpenCode validate every event type through a schema library on both
   append and read — T3 Code via Effect Schema, though without an explicit
   per-event-type version field, handling evolution additively rather than
   by branching on a version number; Claude Agent SDK deliberately keeps
   entries opaque (`{type: string}`) to maximize adapter portability.
   Given we control both ends of our own store, the schema-validating
   design (T3 Code/OpenCode) buys us a much cheaper way to catch malformed
   events than the opaque design, which pushes all of that discipline onto
   the consuming application.

The one-line reading of the whole study: the industry has already proven
the event-sourced session store pattern is implementable and exercised in
real, evolving codebases at two independent shops (T3 Code, OpenCode) —
though for OpenCode specifically, the evidence comes from a private fork
mid-migration where which store (legacy filesystem vs. v2) is authoritative
in the shipped distribution remains an open question — and approximated it
everywhere else with append-only JSONL, which means our job is not to
invent the pattern but to close the two gaps nobody has closed yet:
subagent cascade semantics and retention on an unbounded log.
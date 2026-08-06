# Synthesis: what the industry means by a "stored session"

Part of Session Store Research.
Every product dossier, one question: when an agent product persists,
resumes, lists, and retires a session, what does it actually keep on disk
and how close is that shape to an append-only log with derived
projections? Purpose: extract the invariant core our own event-sourced
Session Store must model, and the axes where products deliberately
diverge. This synthesis is frozen as decision-time input: where a
conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

The through-line is the append-log-vs-mutable-record spectrum. At one end
sit [T3 Code](./products/t3code/index.md) and [OpenCode](./products/opencode/index.md)'s
v2 subsystem, whose durable session **is** an event table with rebuildable
SQL projections. At the other sits [Goose](./products/goose/index.md) and
[Hermes](./products/hermes-agent/index.md), whose durable session is a mutable
SQLite row that retroactive operations DELETE and re-INSERT or flip flags
on. The CLIs in between, [Claude Agent SDK](./products/claude-agent-sdk/index.md),
[Codex CLI](./products/codex-cli/index.md), [Gemini CLI](./products/gemini-cli/index.md),
and [Grok Build](./products/grok-build/index.md), converge on append-only JSONL
transcripts with derived read models bolted alongside (a SQLite index for
Codex CLI; JSON sidecars/registries for Claude Agent SDK), which is
directionally the same shape with looser discipline: no expected-version
precondition anywhere in the group, though Codex CLI's SQLite projection
carries a formal rebuild/read-repair cursor contract that the others
lack. [LangGraph](./products/langgraph/index.md)
is the odd one out structurally: its unit is a parent-linked chain of
immutable state snapshots, not a message transcript, and it is the second
cleanest event-sourcing analog in the corpus after T3 Code.
[IronClaw](./products/ironclaw/index.md) sits off the spectrum's midpoint rather
than along it: its durable session is neither one log nor one mutable row
but a **set of individually compare-and-swap-versioned records** at virtual
paths (one per message, one per summary artifact, one thread metadata
record holding a durable sequence counter), which makes it the only product
in the corpus with a real expected-version precondition on the write
boundary *and* a mutable history.

> IronClaw was researched and added after this synthesis was first frozen.
> Its evidence revised Convergence #8 (optimistic concurrency), Divergence
> A (the central axis), Divergence F (multi-host), and Design decisions 3
> and 6; those revisions are marked inline. Every other claim held.

## Convergence

**1. Nobody stores model-visible context as the durable artifact; a
separate durable log or table always exists, and the model's view is
derived from it.** [Claude Agent SDK](./products/claude-agent-sdk/index.md): "a
session whose store holds 503 raw entries may return 18 messages from
`getSessionMessages`." [Codex CLI](./products/codex-cli/index.md): "the live and
persisted histories remain identical" even as `replacement_history`
replaces what the model re-reads. [Grok Build](./products/grok-build/index.md):
"Rebuild the derived `chat_history.jsonl` cache from `updates.jsonl`, the
durable source of truth." [T3 Code](./products/t3code/index.md) and
[OpenCode](./products/opencode/index.md) hold this as policy: "the only
'shrinking' is view-side ... bound what the UI holds, not what is stored."
Even [Goose](./products/goose/index.md),
the corpus's most mutable store, keeps pre-compaction turns as
`agent_invisible` rows rather than deleting them.
[IronClaw](./products/ironclaw/index.md) holds the line in one method: summary
substitution happens only inside `load_context_window`, while
`list_thread_history` returns the original messages *and* the summary
artifacts that stand in for them, so only the assembled prompt is lossy.

**2. JSONL append-only transcripts are the majority default for CLI
products, and the append discipline is remarkably specific.** [Claude Agent
SDK](./products/claude-agent-sdk/index.md), [Codex CLI](./products/codex-cli/index.md),
[Gemini CLI](./products/gemini-cli/index.md), and [Grok Build](./products/grok-build/index.md)
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
[IronClaw](./products/ironclaw/index.md) is a third kind of exception, and a
useful one: it ships **no rewind at all** (no rewind, fork, branch, or
message edit exists in its transcript trait or design docs), and its single
retroactive operation is deliberately destructive: `redact_message` nulls
content, attachments, and the provider-call payload in place, leaving only a
`redaction_ref`. A product can therefore decline the entire rewind axis and
pay no visible architectural cost, which reframes rewind as a schedulable
product feature rather than a property the durable model must be shaped
around.

**4. Compaction is universally an upstream/agent-loop concern that the
store merely records, never triggers or understands.** [LangGraph](./products/langgraph/index.md):
"the store neither triggers nor understands it," a summary is just the
next value of a channel. [Claude Agent SDK](./products/claude-agent-sdk/index.md):
compaction produces "another appended entry," an `isCompactSummary` marker.
[Codex CLI](./products/codex-cli/index.md) appends a `Compacted` item with
`replacement_history`. [OpenCode](./products/opencode/index.md): "Compaction is
upstream of the store... but leaves a durable marker in the log." Even
[Goose](./products/goose/index.md), which rewrites rows for compaction, treats the
summarization call itself as an agent-loop decision the store just
persists the result of. The one partial counterexample is
[IronClaw](./products/ironclaw/index.md), where compaction is explicitly *not* the
loop's business: it is host-managed and treated as a security boundary,
"Host-managed compaction is a typed retention boundary", with secret-leak
scanning (`Block`/`Redact`/`Warn`) inside the compaction path, a rescan
before both inference and persistence, and a fail-closed on residual
matches. The store still only records the artifact, but *who decides to
compact* moves below the loop rather than above it.

**5. Fork/branch always mints a new identity; nobody reuses the source
session id.** [Claude Agent SDK](./products/claude-agent-sdk/index.md)'s
`forkSession` "rewrites every `sessionId` field and remaps message UUIDs...
An adapter-level copy... would produce a transcript that still references
the old session ID, so the SDK does not use one." [Codex CLI](./products/codex-cli/index.md)
mints a new `thread_id` and stitches lineage via `SessionMeta.forked_from_id`
plus `history_base`. [T3 Code](./products/t3code/index.md) requires a dedicated
`ThreadForkService` and produces a `thread.forked` event on a new stream.
[Grok Build](./products/grok-build/index.md): "Fork is copy-plus-lineage, not a
shared-prefix reference." [Goose](./products/goose/index.md) mints a fresh
`YYYYMMDD_N` id via `copy_session`. Only [LangGraph](./products/langgraph/index.md)
gets a genuinely cheap fork, because content-addressed channel blobs are
shared by reference across the copied chain.
[IronClaw](./products/ironclaw/index.md) abstains: the only `Fork` in the tree is a
subagent context-seed mode that is "enum variant reserved, unimplemented"
and denied at runtime if requested.

**6. Subagents are (almost) always a sibling stream/session linked by a
parent pointer, never entries inlined in the parent's transcript.**
[Codex CLI](./products/codex-cli/index.md): `SessionMeta.parent_thread_id`, "a
first-class sibling thread." [T3 Code](./products/t3code/index.md): "each
subagent appears as its own thread: openable, inspectable mid-flight,
steerable, and resumable." [OpenCode](./products/opencode/index.md): "a
first-class sibling session," linked by `parent_id`. [Grok Build](./products/grok-build/index.md):
its own directory plus a `SubagentMeta` pointer file. [Goose](./products/goose/index.md):
its own row, linked by `parent_session_id`. [Hermes](./products/hermes-agent/index.md):
its own row plus a durable delivery outbox (`async_delegations`) for
crash-safe result reconciliation. The sole structural exception is
[Claude Agent SDK](./products/claude-agent-sdk/index.md), which nests subagent
`.jsonl` files physically *inside* the parent's session directory rather
than as a database-level sibling, still a separate transcript, just a
different addressing scheme. [IronClaw](./products/ironclaw/index.md) agrees on
the sibling shape but moves the pointer: the child gets a fresh
`thread_id` under the parent's exact tenant/agent/project/owner scope, and
the lineage (`parent_run_id`, `subagent_depth`,
`spawn_tree_root_run_id`) lives on the **run** record, not the thread. The
transcript store does not know it is holding a child session at all, which
is the cleanest separation in the corpus and also the reason IronClaw has no
parent-to-child enumeration path.

**7. Cascade-on-delete for subagents is inconsistent and mostly unhandled,
and nobody has a clean answer.** [Codex CLI](./products/codex-cli/index.md): "a
missing parent produces a 'malformed lineage' error rather than silent
cascade." [Goose](./products/goose/index.md): "no cascade to children... leaving
a subagent row with a dangling `parent_session_id`." [OpenCode](./products/opencode/index.md):
"deleting a parent session row does not delete children, they would
orphan." [T3 Code](./products/t3code/index.md): "No cascade to child threads was
found... children keep their `parentThreadId` and would be orphaned."
[Grok Build](./products/grok-build/index.md): "No GC for orphaned subagent
session directories was found." This is a convergence in the sense that
every product that has subagents has the *same unresolved gap*.
[IronClaw](./products/ironclaw/index.md) is the first product in the corpus to
close *half* of it: a parent cancel that discards an already-finished child
writes a durable `SubagentResultTombstone { child_run_id, disposition:
"discarded_by_parent_cancel", terminal_status }`, so a discarded child is a
recorded fact rather than an orphan. Parent *delete* cascade is still
unspecified there, so the gap narrows rather than closes.

**8. Optimistic concurrency at the store's write boundary is rare, and the
one product that implements it fully does so *without* event sourcing.**
Revised after IronClaw. [Claude Agent SDK](./products/claude-agent-sdk/index.md):
"there is no expected-position precondition on `append`." [Goose](./products/goose/index.md):
"no expected-version precondition anywhere; there is no CAS." [Hermes](./products/hermes-agent/index.md):
"there is no optimistic-concurrency / expected-position precondition
anywhere." [Gemini CLI](./products/gemini-cli/index.md) and [Codex CLI](./products/codex-cli/index.md)
rely on a single-writer-per-session assumption with no lock. Only
[T3 Code](./products/t3code/index.md) (a unique `(aggregate_kind, stream_id,
stream_version)` index plus a single-writer command queue) and
[OpenCode](./products/opencode/index.md) (an explicit expected-seq check on
replay: "Sequence mismatch") give the write path any real
conflict-detection teeth; [LangGraph](./products/langgraph/index.md)'s unique
`(thread_id, checkpoint_id)` key is unrelated to conflict detection, its
own dossier flags "no expected-version/OCC in the OSS savers" and notes
`put` is last-write-wins on a given checkpoint id.
[IronClaw](./products/ironclaw/index.md) is the counterexample that forced this
convergence to be rewritten: every transcript write carries a
`CasExpectation` (`Absent` for creates, `Version(RecordVersion)` for
read-modify-write, `Any` reserved for admin backfills), the version token is
backend-minted and unforgeable ("Consumers obtain versions only by reading
existing entries — they cannot fabricate one"), and the retry policy is
explicit (`FILESYSTEM_CAS_RETRIES = 8`, then a hard error). Its substrate
contract makes CAS the floor rather than an optimization: "Stores must always
work with CAS (`put` + `CasExpectation::Version`) as the floor." Two caveats
keep this from being a free win. First, per-record CAS protects each record,
not an invariant across records, which is exactly why IronClaw still needs a
separate durable per-thread active-run lock and an atomic sequence
reservation; a per-aggregate expected-sequence append folds lost-update
safety and cross-record ordering into one mechanism, though run ownership
still has to be modeled as explicit lease transitions on the aggregate rather
than obtained for free. Second, CAS is a lost-update defense on a mutable
store, not an immutability guarantee.

## Divergence

**A. Append-only log vs mutable row, the central axis.** Pure log:
[T3 Code](./products/t3code/index.md) ("unambiguously session-as-log
(event-sourced)"), [OpenCode](./products/opencode/index.md) v2 ("This is
unambiguously session-as-log"), [LangGraph](./products/langgraph/index.md)
(immutable, id-addressed, parent-linked snapshots). Log-shaped but looser:
[Claude Agent SDK](./products/claude-agent-sdk/index.md), [Codex CLI](./products/codex-cli/index.md),
[Gemini CLI](./products/gemini-cli/index.md), [Grok Build](./products/grok-build/index.md),
all JSONL, but looser in different ways: tolerated multi-writer
interleaving (Claude Agent SDK), message-level last-write-wins re-appends
(Gemini), no ordinal at all (legacy Codex), or a single-writer-per-session
assumption enforced only socially, via an exclusive per-append file lock
plus a pid registry rather than a store-level contract (Grok Build).
Mutable row: [Goose](./products/goose/index.md) ("a mutable row plus an
in-place-editable ordered message table... explicitly not session-as-log")
and [Hermes](./products/hermes-agent/index.md) ("session-as-mutable-relational-
record... the least event-sourced of the products studied"). A third pole,
added after IronClaw: **CAS-versioned record per message**
([IronClaw](./products/ironclaw/index.md)), where history is mutable in place
(draft → finalize, status transitions, redaction) but every mutation
requires the reader's version token, and creates require
`CasExpectation::Absent`. That combination is worth naming because it
separates two properties the rest of the corpus conflates: append-only-ness
(immutability of what was written) and lost-update safety (no writer
silently clobbers another). IronClaw buys the second without the first.
**Our
service must decide: is the append-only guarantee enforced by the store
(reject any operation that isn't an append), or merely a convention the
caller can violate?** T3 Code and OpenCode enforce it structurally (no
delete/rewrite path exists below the projector); the JSONL products only
enforce it by omission (nothing offers an edit API, but nothing prevents
one either).

**B. Identity minting: client-random UUID vs client time-ordered UUIDv7 vs
server-assigned date+ordinal vs server-assigned monotonic sequence.**
Random, v4-shaped (observed on disk, not documented): [Claude Agent SDK](./products/claude-agent-sdk/index.md)
session id. Time-ordered UUIDv7/ULID-like, client-minted: [Codex CLI](./products/codex-cli/index.md)
("Codex-generated thread IDs are UUIDv7, and some use cases rely on that"),
[OpenCode](./products/opencode/index.md) (`ses_` ids pack timestamp + counter,
bit-inverted for descending sort), [LangGraph](./products/langgraph/index.md)
(checkpoint id is UUID6, "unique and monotonically increasing, so can be
used for sorting"). Time-ordered UUIDv7, server-assigned as a fallback:
[Grok Build](./products/grok-build/index.md) (session ids "are minted as UUIDv7
when the ACP client does not supply one," via `uuid::now_v7()`).
Server-assigned, human-legible, low-entropy:
[Goose](./products/goose/index.md) (`YYYYMMDD_N`, a per-day counter, "not a
UUID... but no location"), [Hermes](./products/hermes-agent/index.md) session id
(`{timestamp}_{6-hex}`, "~24 bits of entropy, second-resolution collisions
theoretically possible"). Pure sequence, no id semantics at all:
[T3 Code](./products/t3code/index.md) (`stream_version` per aggregate plus a
global `sequence`) and [OpenCode](./products/opencode/index.md) (per-aggregate
`seq`). **Divergence to resolve: do we want an id that is sortable by
construction (UUIDv7/ULID) or an id that is opaque and let a separate
sequence column carry order?** The two cleanest event-sourced designs
(T3 Code, OpenCode) use opaque ids for identity and a *separate* strictly
monotonic sequence for order; they do not conflate the two concerns the
way UUIDv7-as-directory-name products do.
[IronClaw](./products/ironclaw/index.md) lands on the same answer from a different
direction: `ThreadId` is a *validated string* (≤256 bytes, no path
separators, no control characters, no reserved `__ironclaw_` prefix, because
the id becomes a path segment) that the caller may supply and the service
mints as UUIDv4 only when absent, while order comes from a durable per-thread
`u64` sequence allocated either by an atomic path-local counter row or by a
CAS loop on the thread record. Its migration note is the best argument in the
corpus for keeping identity and order separate: switching an existing thread
onto the faster counter "would restart at 1 and collide with messages already
at sequences 1..N", so "No thread ever switches counters mid-stream."

**C. Scope of the store: per-project directory vs single global
database.** Directory-per-project, no cross-project store: [Claude Agent
SDK](./products/claude-agent-sdk/index.md) (`projectKey` flattens the cwd into
the path), [Codex CLI](./products/codex-cli/index.md) (time-sharded but global
within `$CODEX_HOME`, filtered by `cwd_filters`), [Gemini CLI](./products/gemini-cli/index.md)
(`projectShortId` directory). Single database, cwd as a plain filter
column: [Goose](./products/goose/index.md) ("no cwd/project path is encoded into
the key... project_id is just a nullable column"), [Hermes](./products/hermes-agent/index.md)
(one `state.db` per profile, `cwd` a plain column), [T3 Code](./products/t3code/index.md)
and [OpenCode](./products/opencode/index.md) (one DB, `project_id`/`workspace_id`
columns), [Grok Build](./products/grok-build/index.md) (cwd-encoded directory
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
[IronClaw](./products/ironclaw/index.md) dissolves the question instead of
answering it: no working directory, cwd hash, or worktree appears in the key
at all. The path prefix is a *logical* scope of tenant, agent, optional
project, owner, and mission over a virtual filesystem that is usually SQL rows,
so there is no relocation to reconcile, and scoping doubles as an
authorization boundary (listing "MUST scope the listing by `owner_user_id`
[...] otherwise a caller could enumerate threads owned by other users in the
same `(tenant, agent, project)` triple"; reads return the same
`UnknownThread` for absent and cross-scope threads "so callers cannot use the
response as an existence oracle"). The cost is that scope is baked into the
path: re-parenting a thread to a new project or owner has no supported
operation.

**D. Compaction's durable shape: in-place row rewrite vs external snapshot
file vs pure append marker.** Rewrite in place: [Goose](./products/goose/index.md)
(`DELETE all rows, re-INSERT` via `replace_conversation`) and
[Hermes](./products/hermes-agent/index.md) (`UPDATE active=0, compacted=1` then
insert new rows, "a content-preserving UPDATE"). External snapshot plus a
log marker: [Grok Build](./products/grok-build/index.md) (`CompactionCheckpoint`
marker in `updates.jsonl` plus a full separate `compaction_checkpoints/
{id}.json` file, required to rewind past the boundary, and rewind fails
closed if the file is missing). Pure append, no external file needed:
[Claude Agent SDK](./products/claude-agent-sdk/index.md) (`isCompactSummary`
entry in the same log), [Codex CLI](./products/codex-cli/index.md) (`Compacted`
item with `replacement_history` inline), [T3 Code](./products/t3code/index.md)
(no compaction of the log at all, it is unbounded and grows forever).
Sibling record in the same store, addressed by sequence range:
[IronClaw](./products/ironclaw/index.md) writes a `SummaryArtifact { start_sequence,
end_sequence, summary_kind, content, model_context_policy }` at
`summaries/<summary_id>.json` with `CasExpectation::Absent`, validates that
both range endpoints exist as real messages, and makes replay
idempotent-by-content (an identical re-compaction returns the existing
artifact; a different overlapping range is rejected with
`OverlappingSummaryRange`, and only `ReplaceRangeWhenSelected` summaries are
overlap-checked at all).
**Divergence to resolve: does a compaction boundary require a sidecar
artifact recoverable independently of the log (Grok Build's model, with an
explicit fail-closed on missing sidecar), or is a same-stream marker
sufficient?** The sidecar approach adds a second thing that can go missing;
the same-stream approach keeps one recovery story but grows the log
un-compactably.

**E. Retention: nobody enforces it at the store layer, but who is
*expected* to differs.** Explicitly the caller's job, store provides
mechanism only: [Claude Agent SDK](./products/claude-agent-sdk/index.md) ("The
SDK never deletes from your store on its own... TTLs, S3 lifecycle
policies... are the adapter's responsibility"), [LangGraph](./products/langgraph/index.md)
(`prune(strategy=)`, no automatic lifecycle). Product-owned sweep with a
concrete default: [Claude Agent SDK](./products/claude-agent-sdk/index.md)'s own
CLI (`cleanupPeriodDays`, default 30), [Gemini CLI](./products/gemini-cli/index.md)
(delete-on-exit-if-not-resumable), [Hermes](./products/hermes-agent/index.md)
(`prune_sessions(older_than_days=90)`, invoked, not scheduled). No
retention story at all, log grows forever: [T3 Code](./products/t3code/index.md)
("no retention or log-truncation/snapshotting, the log grows unbounded")
and [OpenCode](./products/opencode/index.md) ("none found... the log is retained
indefinitely"), and [IronClaw](./products/ironclaw/index.md), which has no
transcript TTL, lifecycle policy, or sweep, and bounds *reads* (page-size
caps, byte budgets, a 100k index-row ceiling) instead of stored history. Its
only retention rule lives on the run-lifecycle store, not the transcript:
released admission-reservation evidence is kept "only while the corresponding
terminal run remains within the bounded terminal-record retention window",
because "active capacity accounting must not scan unbounded released
history." IronClaw also supplies the corpus's clearest example of what
un-owned retention costs: its inbound-idempotency records live at
`/threads/idempotency/<sha256>.json`, *outside* any thread root, so
`delete_thread` leaves them behind pointing at a deleted message, with no
sweeper found. **Retention is unowned in the majority of the corpus, and
dedup state that lives outside the aggregate's key space is where the cost
shows up first**, an event-sourced Session Store gets rewind and audit for
free but inherits an explicit obligation to design retention deliberately,
since nothing in the pattern forces it.

**F. Multi-host / multi-writer posture.** Single-host by design, no
coordination: [Codex CLI](./products/codex-cli/index.md), [Gemini CLI](./products/gemini-cli/index.md),
[Goose](./products/goose/index.md) (SQLite write-lock only), [T3 Code](./products/t3code/index.md)
("the database is never shared across hosts"). Single-host with
network-filesystem awareness: [Hermes](./products/hermes-agent/index.md) (WAL on
local disks, falls back to DELETE-mode journal on NFS/SMB/FUSE because "WAL's
shared-memory index needs coherent mmap... those mounts don't provide").
Multi-host as a first-class adapter concern, pushed above the core
interface: [Claude Agent SDK](./products/claude-agent-sdk/index.md) ("Serverless
functions, autoscaled workers, and CI runners don't share a filesystem. A
shared store lets any replica resume any session," with a documented
clock-skew failure mode in the reference S3 adapter). Multi-host avoided
rather than solved: [Grok Build](./products/grok-build/index.md) (per-host
SQLite files on network mounts, rebuildable indexes only, "concurrency
control is advisory file locks plus a pid registry... multi-host is
handled by giving up," with a remote registry merge as a secondary
best-effort lane). Multi-host as a first-class *designed* protocol:
[OpenCode](./products/opencode/index.md) (`events.replay` with an `ownerID` +
`strictOwner` guard, `events.claim` to transfer ownership, a per-aggregate
high-water history-fetch API, "a real distributed event-sync design
layered on the same append-only log").
Multi-host via a shared database instance: [LangGraph](./products/langgraph/index.md)
(Postgres backend, content-addressed blob upserts are conflict-free by
construction). Multi-host as the *default* case, with the local filesystem
treated as the constrained one: [IronClaw](./products/ironclaw/index.md), added
after the first freeze. Nothing in its design assumes a shared filesystem:
backends include libSQL and Postgres, and correctness rests on primitives
that work across processes: CAS with backend-minted versions, atomic
path-local sequence reservation, runner leases with lease tokens and
heartbeats ("Heartbeats only renew metadata for matching, unexpired runner ID/
lease token"; liveness must use durable lease metadata rather than one event
per heartbeat), and a durable active-thread lock so crash detection is
lease-expiry reconciliation rather than a stale-PID heuristic. Its recovery
stance is the sharpest in the corpus, expiry being terminal rather than
resumable:
"Reborn does not automatically retry uncertain side-effecting work after a
lost lease — expiry is terminal, and the user resubmits explicitly."
**This axis is narrower than it was**: two products have now designed it
deliberately rather than assumed it away, and they chose different
mechanisms. OpenCode chose an ownership-claim replication protocol over an
append log, IronClaw a lease-plus-lock coordination plane over a CAS store;
everyone else either assumes single-host or punts the problem to the storage
substrate.

**G. What "the store" persists vs what it treats as opaque.** Fully
opaque entries, store is a pure byte-transport: [Claude Agent SDK](./products/claude-agent-sdk/index.md)
(`SessionStoreEntry` is "a `{ type: string; ... }` object," treated as
opaque JSON by contract). Parsed and validated on every read/write:
[OpenCode](./products/opencode/index.md) (event `data` "decoded and validated
through Effect Schema on both append and read"), [T3 Code](./products/t3code/index.md)
(same, via Effect Schema, plus derived `actor_kind`). Partially parsed,
targeted introspection: [Goose](./products/goose/index.md) ("neither fully
opaque nor a normalized schema, JSON columns with targeted introspection"
via `json_extract`/`json_each`). Typed at the domain layer, opaque at the
substrate layer, with a declared projection between them:
[IronClaw](./products/ironclaw/index.md), where the message is a strongly typed
record (`sequence`, `kind`, `status`, `turn_run_id`, `redaction_ref`) that the
domain store interprets for every query, while the storage backend is
forbidden to parse it: "Backends never look inside `body` for indexing;
everything queryable lives in `indexed`", which is what lets one contract be
served portably by libSQL, Postgres, local files, and HSM-backed mounts. That
split is worth stealing independently of the rest of its design.
**Divergence: does the Session Store
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
| session-as-log with looser discipline (append-only JSONL, no formal OCC contract; projector-contract rigor varies -- Codex CLI's SQLite projection has an explicit rebuild/read-repair cursor) | Claude Agent SDK / Claude Code, Codex CLI, Gemini CLI, Grok Build |
| session-as-directory (path/filename encodes identity; scope encoding varies -- Codex CLI's path is time-sharded only, with cwd scope applied as a query-time filter, not a path segment) | Claude Agent SDK, Codex CLI, Gemini CLI, Grok Build, OpenCode (legacy) |
| session-as-record-set (one CAS-versioned record per message/summary under a scoped virtual path; mutable in place but never without an expected-version precondition) | IronClaw |
| session-as-row (mutable, single record + child table) | Goose, Hermes |
| session-as-mutable-relational-record (flag-mutation instead of events) | Hermes (Goose achieves a similar visibility effect only via a full delete+re-insert rewrite of the message table, not an in-place flag mutation) |
| session-as-document (whole-file rewrite, last-write-wins per id) | Gemini CLI (legacy `.json`) |

These are not mutually exclusive; several products carry two labels at
once (e.g. Claude Agent SDK is both session-as-log and session-as-directory:
the log is the file, and the file's path is the addressing scheme).

## Comparison table

| Product | Durable session is a... | Source of truth | Keying / id scheme | Compaction artifact | Rewind/fork | Append-log closeness |
| --- | --- | --- | --- | --- | --- | --- |
| [Claude Agent SDK](./products/claude-agent-sdk/index.md) | append-only JSONL transcript | the `.jsonl` file (mirrored, not replaced, by the SDK's `SessionStore`) | client v4-shaped UUID (observed); path = `projectKey/sessionId/subpath` | in-log `isCompactSummary` entry, raw entries retained | rewind = view op over log; fork = `forkSession` rewrites ids into a new key | high, but no OCC precondition and tolerates multi-writer interleave |
| [Codex CLI](./products/codex-cli/index.md) | append-only JSONL rollout + derived SQLite index | `RolloutLine` log; SQLite is read-repaired from it | client UUIDv7 `thread_id`; filename = timestamp+id, time-sharded dir | in-log `Compacted` item with `replacement_history` | `ThreadRolledBack` marker replay; fork = new `thread_id` + `history_base` prefix pointer | high; explicit CQRS-shaped log+SQLite-projection design |
| [Gemini CLI](./products/gemini-cli/index.md) | append-only JSONL folded by a replay reducer | the `.jsonl` file; `ConversationRecord` is "the materialized projection, not what is stored line-by-line" | client `promptId`; path = `projectShortId/chats/session-<ts>-<id8>.jsonl` | checkpoint `$set:{messages}` line replaces message set | `$rewindTo` marker, non-destructive; no first-class fork (resume continues same file) | medium; message-level last-write-wins per id, not immutable events |
| [Goose](./products/goose/index.md) | mutable SQLite row + message table | the `sessions`/`messages` rows themselves | server date+counter `YYYYMMDD_N`; global DB, no path key | `replace_conversation`: DELETE all rows, re-INSERT with visibility flags | `truncate_conversation`, a destructive delete; fork = `copy_session`, full physical copy | low; explicitly "not session-as-log" |
| [Grok Build](./products/grok-build/index.md) | append-only JSONL (`updates.jsonl`) + derived caches/index | `updates.jsonl`; cache/summary/FTS all rebuildable | server UUIDv7 session id; path = `sessions/{encoded_cwd}/{id}/` | append marker (`CompactionCheckpoint`) plus external snapshot file, fails closed if missing | `RewindMarker` + dead-branch-filter replay; fork = copy files + lineage fields | high; "event sourcing on the filesystem in all but name" |
| [Hermes](./products/hermes-agent/index.md) | mutable SQLite row + message table | the `sessions`/`messages` rows | client `{timestamp}_{6hex}` id; global per-profile DB | `active=0, compacted=1` in-place flag flip, content-preserving | `rewind_to_message`: soft-delete via flag flip (reversible); fork = `/branch`, full row copy | low; "least event-sourced of the products studied" |
| [IronClaw](./products/ironclaw/index.md) *(added post-synthesis)* | set of CAS-versioned records under a scoped virtual path (thread.json + one file per message + one per summary) | the per-record JSON entries; thread/message/ordered indexes are declared rebuildable projections | caller-suppliable validated `ThreadId` (UUIDv4 when absent) + durable per-thread `u64` sequence; path = scope axes (tenant/agent/project/owner/mission) | sibling `SummaryArtifact` record over a `[start_sequence, end_sequence]` range with `ReplaceRangeWhenSelected`; messages untouched | none of either; only `redact_message`, a destructive in-place erase | low as a log, highest in corpus for write-boundary OCC (`CasExpectation::Version`, 8 retries) |
| [LangGraph](./products/langgraph/index.md) | parent-linked chain of immutable state snapshots | the `checkpoints` table; channel values content-addressed by `(channel, version)` | caller-supplied `thread_id`; checkpoint id = UUID6 | no store-level compaction; `prune`/shallow-saver drop history, `DeltaChannel` snapshots for large channels | rewind = select an older checkpoint (nothing destroyed); fork = new checkpoint sharing ancestor blobs, `copy_thread` | very high; "materially closer to event-sourcing than the transcript products" |
| [OpenCode](./products/opencode/index.md) | append-only SQLite event log (v2) / mutable JSON-per-path store (legacy) | the `event` table, keyed `(aggregate_id, seq)` | client-minted ULID-like `ses_`/`evt_` ids; per-aggregate `seq` | in-log `compaction.ended` event; model-visible view folds from latest compaction seq | `revert.commit` truncates only the projection, event rows kept; no first-class fork found | very high; "unambiguously session-as-log (event-sourced)" |
| [T3 Code](./products/t3code/index.md) | append-only SQLite event log | the `orchestration_events` table, keyed `(aggregate_kind, stream_id, stream_version)` | client-supplied `threadId`; server UUIDv4 `eventId`; global `sequence` + per-stream `stream_version` | none; log is never compacted, only view-side caps | `thread.reverted` event filters the projection, log kept; fork = new stream via `ThreadForkService`, O(history) copy | highest in corpus; "the corpus's cleanest event-sourced example" |

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
   Hermes are the cautionary counterexamples -- Goose via DELETE+re-INSERT
   (`replace_conversation`), Hermes via both a destructive DELETE+re-INSERT
   (`replace_messages`, used by /retry, /undo, /compress) and a
   non-destructive flag-flip (`archive_and_compact`) -- both flagged in
   their own dossiers as crash-risk and history-loss hazards. The
   requirement that most punishes this decision is redaction, and IronClaw
   shows the price of answering it destructively: it erases content in place
   and treats propagation to every derived copy as a hard obligation (the
   cached sidebar title is cleared non-best-effort; summary content becomes
   `[redacted]`). An append-only store must reach the same guarantee by
   indirection, with content referenced rather than inlined so that one
   payload delete or crypto-erase satisfies redaction across every event that
   points at it, and must apply IronClaw's non-best-effort rule to
   projections, since any projection holding a copy of content is a
   redaction liability.

2. **Separate identity from order: an opaque event/session id for
   addressing, a strictly monotonic per-aggregate sequence for ordering.**
   Industry's answer: T3 Code (`stream_version` unique per
   `(aggregate_kind, stream_id)`) and OpenCode (`seq` unique per
   `aggregate_id`) both do this; time-ordered ids (UUIDv7/ULID, used by
   Codex CLI, Grok Build, OpenCode's `ses_`) are a convenience for
   directory/lexical sort, not a substitute for a real sequence.

3. **Require an expected-version precondition on every append (real
   optimistic concurrency), not just a single-writer assumption.**
   Industry's answer, revised after IronClaw: IronClaw is the strongest
   precedent, and notably not an event-sourced one: every transcript write
   carries a `CasExpectation` against a backend-minted, unforgeable
   `RecordVersion`, with CAS declared the portable floor beneath an optional
   transaction API. OpenCode (explicit "Sequence mismatch" check on replay)
   enforces a real caller-supplied precondition on an append log; T3 Code
   gets OCC-like protection only implicitly, via a single-writer command
   queue combined with a unique `(aggregate_kind, stream_id,
   stream_version)` index, with no caller-supplied expected-version check at
   all; the rest (Claude Agent SDK, Goose, Hermes, Gemini CLI) admit they
   have none and rely on tolerated multi-writer interleaving or a social
   single-writer convention. The lesson to carry: per-record CAS is not
   equivalent to a per-aggregate expected-sequence append. IronClaw needs
   record CAS plus an atomic sequence reservation to get the lost-update and
   ordering guarantees one expected-version append on a decider aggregate
   gives us, which is an argument for our shape rather than against it. Its
   third mechanism, the active-run lock, is not something the append replaces:
   lease ownership, heartbeat renewal, expiry, and admission before any
   model or tool side effect have to become modeled transitions on the
   aggregate, with the append enforcing them rather than supplying them.

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
   sibling-stream-plus-pointer (Convergence #6) but almost no consistent
   answer on cascade (Convergence #7); every product either orphans children
   or doesn't say. Revised after IronClaw, which supplies the one partial
   answer worth copying: link lineage on the *run*, bound the tree with an
   atomic descendant reservation before anything is queued, and record a
   discard as a durable `SubagentResultTombstone` naming the disposition
   rather than dropping it. Transcript-level cascade on parent delete remains
   a genuine gap we get to close rather than copy.

7. **Retention and log truncation are not solved by event-sourcing and
   must be designed deliberately, not deferred. Dedup and idempotency state
   must live inside the aggregate's key space so retention and deletion
   cascade to it.** Industry's answer: the two purest event-sourced stores
   (T3 Code, OpenCode) have *no* retention story at all and grow unbounded,
   and IronClaw has none for transcripts either; the JSONL products that do
   have retention treat it as an out-of-store caller responsibility (Claude
   Agent SDK's explicit "the SDK never deletes from your store on its own")
   with concrete but ad hoc defaults (30-90 days) enforced by a sweep, not
   a store primitive. IronClaw supplies the concrete failure mode for the
   second half: its SHA-256 inbound-dedup records sit outside the thread
   root, survive `delete_thread`, and have no sweeper, so a redelivered
   pre-deletion event replays as an accepted message that no longer exists.
   Our dedup key belongs in the appended event, not in a sidecar.

8. **Multi-host correctness needs an explicit coordination protocol on the
   write path, not an assumption of a shared filesystem.** Industry's
   answer, revised after IronClaw: two products have designed one, with
   different mechanisms. OpenCode uses ownership-claim replication
   (`events.claim`, `ownerID` + `strictOwner` replay guard, per-aggregate
   high-water history sync). IronClaw uses a coordination plane instead:
   record-level CAS, atomic sequence reservation, runner leases with tokens
   and heartbeats, a durable active-run lock per thread scope, and terminal
   (never auto-retried) lease expiry. Everyone else either assumes
   single-host (Codex CLI, Gemini CLI, Goose, T3 Code, Hermes) or pushes the
   problem to the adapter/storage substrate (Claude Agent SDK's pluggable
   mirror, Grok Build's per-host SQLite files, LangGraph's shared Postgres).
   Worth noting that IronClaw's two mechanisms are separable from its storage
   model: the lease/lock plane would sit on top of an append log unchanged.

9. **Event payloads should be schema-validated at the storage boundary,
   not treated as opaque bytes.** Industry's answer: split. T3 Code and
   OpenCode validate every event type through a schema library on both
   append and read -- T3 Code via Effect Schema, though without an explicit
   per-event-type version field, handling evolution additively rather than
   by branching on a version number; Claude Agent SDK deliberately keeps
   entries opaque (`{type: string}`) to maximize adapter portability.
   Given we control both ends of our own store, the schema-validating
   design (T3 Code/OpenCode) buys us a much cheaper way to catch malformed
   events than the opaque design, which pushes all of that discipline onto
   the consuming application.

The one-line reading of the whole study: the industry has already proven
the event-sourced session store pattern is implementable and exercised in
real, evolving codebases at two independent shops (T3 Code, OpenCode) --
though for OpenCode specifically, the evidence comes from a private fork
mid-migration where which store (legacy filesystem vs. v2) is authoritative
in the shipped distribution remains an open question -- and approximated it
everywhere else with append-only JSONL, which means our job is not to
invent the pattern but to close the two gaps nobody has closed yet:
subagent cascade semantics and retention on an unbounded log.

Revised after IronClaw: those two gaps are now one and a half. IronClaw is
the one product that treats both as first-class design problems and gets
partway through each, from the opposite end of the spectrum. On cascade, it
bounds the subagent tree with an atomic descendant reservation taken before
any child is queued and records a discarded child result as a durable
`SubagentResultTombstone` naming the disposition, which is more than any
event-sourced product does, but it still leaves transcript-level cascade on
parent deletion unaddressed. On retention, it declares a typed redaction
boundary with a leak-scanning test rather than a growth policy, so
transcripts still grow without bound while lifecycle records get a bounded
terminal-retention window. IronClaw also demonstrates that the write-boundary
concurrency guarantee we want is achievable outside event sourcing, at the
cost of coordinating separate record, sequence, and lock mechanisms where an
expected-version append covers the lost-update and ordering half on its own.
That strengthens rather than weakens the case for our shape, and it means the
remaining novel work is transcript cascade on delete plus a real growth
policy.

## Stage-two results, not yet absorbed above

Everything above is frozen as decision-time input from the dossiers that
existed when it was written. The stage-two comparisons landed after it, and
this section records what they add without rewriting the frozen text around
it. Where the two disagree, the comparisons are the newer reading and the ADR
is authoritative over both.

**The design mostly survives contact with the evidence.** Across the numbered
recommendations in the comparisons read for this pass, 55 of them, there are 45
blast-radius statements.
Eight mention breaking in any form: four are "do not do X later" guardrails
against regressions we have not committed to (Cline on deterministic child ids,
Letta on relaxing optimistic concurrency for an `At` transition, OpenHands on a
second non-replayable authoritative store, Zed on a pre-session draft keyspace),
three are conditional on which answer we pick (Crush on a parent-cost rollup,
Pi on `SessionForked` crossing a `WorkspaceRef`, Cline on a claim-check
threshold), and exactly one asks for a change that breaks something today:
adopt an explicit schema-version marker and a written back-compat policy at the
`v1alpha1` to `v1` promotion (Google ADK, 10/12, with AWS Strands at 6/12
arriving at the same question from the opposite direction). The remaining
statements are additive, and most of those are documentation, Non-Goals, tests,
or CI. The two thinnest stores, SWE-agent at 3/12 and Aider at 4/12, yield zero
recommendations by explicit argument, which is the maturity rubric discarding
evidence rather than letting a weak store anchor a change.

**A tenth convergence, and the strongest single finding of the second stage: a
pluggable store interface systematically hides the guarantees callers assume it
provides.** Four products span the full maturity range and fail the same way.
Google ADK (10/12) has real expected-version optimistic concurrency in
`DatabaseSessionService`, materially weaker checking in `SqliteSessionService`,
and none at all in the in-memory and Vertex backends, so "the same interface"
conceals a behavioral cliff on concurrent append. Mastra (11/12) has four
adapters reaching four different atomicity conclusions from one abstract
interface. The OpenAI Agents SDK (5/12) has nine backends each re-deriving
identity, ordering, and concurrency independently (an autoincrement column, a
Mongo `seq`, a Dapr ETag), several imperfectly. Pi (7/12) has three
implementations of one interface that have already silently diverged on a single
field, with the checked-in documentation then describing harness-only behavior as
if it were the CLI's. This converts Pi's recommendation 2 from a nice-to-have
into the best-supported precondition in the corpus: any second `trogon-decider`
implementation must pass a shared conformance suite over every
`WRITE_PRECONDITION` class before it ships.

**Convergence 8 above survives every product added since and gets sharper.** Even
the two stores that do have optimistic concurrency put it in the wrong place.
Letta version-checks exactly one ORM model, `Block`, which holds
memory-configuration data, while the actual per-turn hot pointer
`Agent.message_ids` has no guard at all. Substrate-level `At(current_position)`
by default remains the corpus outlier, in our favor.

**Cascade-on-terminal is validated, and the rewind split is not merely
unvalidated but unattempted.** Zed is the only studied product with genuinely
transitive cascade, and it chose the same behavior
[ADR#0035](../../adr/0035-session-store-decider-aggregate.md) decision 6 does,
independently. Cline's stops one level deep and only from a root. Codex CLI,
Goose, OpenCode, and T3 Code orphan. Roo Code, queued as a presumed restatement
of Cline, recurses the full child-task tree and is the second real cascade in the
corpus. Nobody anywhere has an analog to invalidating a child whose dispatch
point a still-running parent has rewound away, so that half of decision 6 is
original design work rather than industry practice restated.

**Decision 7 gets no evidence either way, structurally.** A mutable-document
store never faces the problem, because replacing or deleting a whole document is
strictly easier than masking part of a keep-forever fact stream. This is why no
product validates `SessionHidden`, `RedactionApplied`, or `ArtifactErased`, and
it is a property of the sample rather than a weakness in the decision.

**The message payload is documented per product and not at all per provider.**
All but one dossier carries an entry-structure section, and the comparisons that
go further map the product's message type row by row against `CanonicalMessage`
and its seven-arm `ContentBlock` oneof. What no artifact covers is the provider
side: `ProviderBlock` exists to absorb blocks the typed arms cannot model, and
nothing in the corpus enumerates what would go through it, so whether seven arms
are the right seven is still open. The queued stage three in the
[backlog](./backlog.md) takes a provider rather than a product as its unit of
study for that reason.

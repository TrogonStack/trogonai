# Amazon Q Developer CLI compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Amazon Q Developer CLI](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 8/12**: evolution scars 2/3 (a real eight-step SQL migration
ratchet across three tables, `crates/chat-cli/src/database/mod.rs:67-76`, one
step of which is a genuine `ALTER COLUMN TYPE` workaround: `006_make_state_blob.sql:1-6` renames the old table, recreates it, copies rows,
and drops the original, because SQLite has no native column-type-change
statement, plus a JSON-payload back-compat comment naming a specific prior
version, `model: Option<String>` "kept only to maintain deserialization
backwards compatibility with <=v1.13.3" (`conversation.rs:133-134`), real
scarring, but confined to additive fields and one type-change workaround, not
a generational cutover), operational age 1/3 (no issue reports, corruption or
lock-contention fixes, or first-commit dates are cited anywhere in the
dossier; the only operational-age evidence is the version-pinned back-compat
comment above, and the dossier is explicit that its own `busy_timeout=0`
concurrency claim is "inference from SQLite's documented defaults... not a
claim we verified by forcing a lock in this environment", source-level
evidence only, not field-confirmed), exposure 2/3 (vendor-shipped by a major
cloud vendor as the `q` CLI / `chat-cli` crate, a real, adopted product, but
the dossier cites no user-scale numbers, and the product has zero multi-host
or network-filesystem handling by design: a single local `data.sqlite3`,
"multi-host is out of scope by construction", so exposure evidence is
vendor-identity only, not corroborated by usage-scale or cross-host
operational evidence), design independence 3/3 (no evidence anywhere in the
dossier of forked or inherited persistence code from another product; the one
"migration" hit, `PROFILE_MIGRATION_KEY`, is an internal config-format
migration from an old "profile" scheme to the current "agent" scheme, not an
imported foreign session store).

8/12 clears the corpus's thin-evidence line, but only barely, and unevenly:
treat the evolution-scars and design-independence findings as solid, and the
operational-age and exposure findings as directional rather than
field-confirmed. Where Amazon Q disagrees with a higher-scoring store in this
corpus (Cline, 10/12), Cline's answer is the default per the ADR's own
maturity rule; Amazon Q is cited here for what it is: the strongest available
confirmation that the *most* mutable-record end of the spectrum is a real,
vendor-shipped design, not a strawman.

## The one structural difference everything else follows from

Amazon Q's durable session is a single mutable JSON blob in a SQLite
`conversations (key TEXT PRIMARY KEY, value TEXT)` table, keyed by the
**literal absolute working-directory path string**, `INSERT OR REPLACE`d in
full on every assistant turn
(`set_conversation_by_path`, `crates/chat-cli/src/database/mod.rs:399-411`).
There is no append operation anywhere in the content path, no partial update,
and, critically, past what "mutable document" alone would predict, two
independent, both destructive, size-management mechanisms mutate that one row
*in place* with no tombstone, no marker, and no externally-retained pre-image:
a 10,000-entry soft cap that drains the in-memory history before every save
(`enforce_conversation_invariants`, `conversation.rs:1121-1217`), and
`/compact`, which drains all but the last N entries and replaces them with an
AI-generated summary (`replace_history_with_summary`, `conversation.rs:732-741`).

We commit at **fact granularity** on an **opaque, addressable identity**:
`UserMessageRecorded`, `AssistantMessageStarted`/`Completed`,
`ToolCallRequested`/`Started`/`Completed`/`Failed` are separate, durable events
on a session's own logical stream, addressed by an opaque `SessionId`
(`proto/trogonai/session/sessions/v1alpha1/events.proto`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1),
and no command ever purges or trims that stream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2, decision
7). Two orthogonal choices compound into Amazon Q's design, and both cut the
opposite way from ours:

1. **Mutability, not append.** Every save is a full-document overwrite, so
   there is nothing to "keep forever"; the destructive cap and `/compact`
   are not omissions of a retention policy, they are the *only* size-management
   mechanism a mutable-document store can have without an append-only log
   underneath it.
2. **Location as identity.** The primary key is `std::env::current_dir()`
   (`conversation.rs:420-421`, `crates/chat-cli/src/cli/chat/mod.rs:720-723`), not a minted session id.
   `ConversationState.conversation_id` (`conversation.rs:109`) is a real,
   internal UUIDv4 identity field that plays **no addressing role at all**, and a system that mints an identity but does not use it as the storage key
   gets no protection from that identity.

Both choices are the direct cause of the two most consequential findings
below: the destructive-shrink retention story, and the parent/delegate
same-directory collision risk (an inference, not an observed race, carried
forward from the dossier, not hardened here). Everything else in this
comparison (no enumeration, no delete, no relocation reconciliation, no
cascade) is a consequence of one or the other.

## Mapping

| Amazon Q | Ours | Verdict |
| --- | --- | --- |
| `conversations (key TEXT PRIMARY KEY, value TEXT)` SQLite row, `INSERT OR REPLACE`d whole (`database/mod.rs:399-411`) | Per-session logical stream on subject `session.sessions.events.<session_id>` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1), append-only (decision 2) | Central structural difference |
| Absolute cwd path string as primary key (`get_conversation_by_path`/`set_conversation_by_path`, `database/mod.rs:385-411`) | Opaque `SessionId` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Ours, decisively: avoids collapsing location into identity |
| `ConversationState.conversation_id` (UUIDv4, internal, no addressing role, `conversation.rs:109`) | `SessionId` (opaque, the actual addressing key) | Semantic mismatch: Amazon Q's "id" is not the key; ours is the key |
| 10,000-entry soft cap, drained in place before every save (`enforce_conversation_invariants`, `conversation.rs:1121-1217`) | No cap; keep-forever ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7), snapshot-bounded replay (facet 8) | Ours, decisively |
| `/compact` → `replace_history_with_summary`, drains all but `messages_to_exclude` (default `0`) entries, stores AI summary in `latest_summary` (`conversation.rs:732-741`) | `Compacted{covers_from, covers_through, summary_content}` (`compacted.proto`), an in-stream marker; covered events stay on the log ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 4, decision 7) | Ours, decisively |
| `transcript: VecDeque<String>`, a denormalized prose log capped independently at the same 10,000 entries, not sent to the backend (`conversation.rs:120`, `append_transcript`, `:903-908`) | No parallel denormalized log; `CanonicalMessage` is the single source (`message.proto`) | Ours, deliberately: nothing to drift out of sync |
| `CheckpointManager`, a shadow **bare git repo** at `~/.aws/amazonq/cli-checkouts/<conversation_id>`; each `Checkpoint` additionally embeds a **full clone of conversation history** (`history_snapshot: VecDeque<HistoryEntry>`, `crates/chat-cli/src/cli/chat/checkpoint.rs:74-82`) | `Checkpoint{reference, checkpoint_type, digest, checkpoint_id, covers_through, session_execution_plan_digest}` (`checkpoint.proto`), a claim-check reference, never inline history | Semantic mismatch: Amazon Q's "checkpoint" duplicates the entire transcript per cut; ours references, never inlines ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3, "four records with separate authority") |
| Tangent mode: `ConversationCheckpoint` snapshot/restore of `history`/`transcript`/`latest_summary`, persisted inline in `tangent_state` (`conversation.rs:154-167,263-303`) | No equivalent | Gap: see recommendation 3 |
| Delegate tool: a wholly separate OS process (`tokio::process::Command::new("q")`, `delegate.rs:341-346`), bookkept in one JSON file per agent name under `<cwd>/.amazonq/.subagents/` | `DelegationDispatched`/`ParentLinked`, a first-class linked session on its own stream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Ours, decisively |
| No cascade/orphan/reconcile behavior for Delegate on parent delete, rewind, or crash, a dead process is detected only by `kill -0` on a recorded `pid`, opportunistically | `ParentTerminated`/`SessionCancelled`, `ParentHistoryInvalidated`, a reconciler reacting to terminal markers on `session.sessions.events.>` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Ours, decisively: see Subagent cascade below |
| No enumeration over `conversations` anywhere in the crate (`Table::Conversations` appears in exactly 3 grep hits, all in `database/mod.rs` itself) | `list_sessions`/`get_session`, a rebuildable KV projection ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively |
| No delete/TTL/retention policy for conversations; `delete_entry` is never called with `Table::Conversations` anywhere in the crate | `SessionHidden` (visibility tombstone) + `RedactionApplied` (read-time mask) + `ArtifactErased` (out-of-band artifact-byte destruction) ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively |
| `resume: bool` CLI flag tied to the current directory; no `--session <id>` flag, no picker (`ChatArgs`, `crates/chat-cli/src/cli/chat/mod.rs:227-253`) | Opaque `SessionId` addressing + `list_sessions` projection ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively |
| Additive `#[serde(default)]`/`skip_serializing_if` JSON payload fields, no version tag, version-pinned back-compat comments (`conversation.rs:133-134`) | Additive-only protobuf evolution, "never a per-event version branch" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) | Equivalent strategy, independently arrived at |
| SQL `MIGRATIONS` ratchet for table *shape*, 8 steps, one requiring a rename-recreate-copy-drop workaround (`database/mod.rs:67-76`, `006_make_state_blob.sql`) | No SQL table shape exists; the protobuf wire schema is the only "shape," ratcheted `v1alpha1` → `v1` by a later decision ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) §1) | Trade-off: see below |
| `ToolUseResult{tool_use_id, content, status}`, opaque to the store, deduped by `tool_use_id` string matching in `enforce_conversation_invariants` (`conversation.rs:1121-1266`) | `ToolCallCompleted`/`ToolCallFailed`, keyed by `tool_execution_id`, first-terminal-outcome-wins fold ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2, decision 4) | Ours, decisively |
| No application-level optimistic concurrency of any kind; concurrent writers race under ordinary SQLite file locking with (inferred, unverified) `busy_timeout=0` | Per-command `WRITE_PRECONDITION` (`NoStream`/`At`/`Any`), server-enforced by JetStream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively |
| No redaction or erasure concept anywhere in the dossier | `RedactionApplied`/`ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively |
| Save's `Result` discarded with `.ok()` (`conversation.rs:421`); a failed persist is silent and the turn is never rolled back | Typed append failures (`WrongExpectedVersion` and friends) that a command boundary must surface, not swallow ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours in principle: see recommendation 1 for why this needs to be said explicitly, not just implied |
| Non-UTF-8 path returns `Ok(None)`/`Ok(0)` silently from the accessor pair (`database/mod.rs:390-393,405-408`) | `SessionId` is an opaque string with its own validation at the append boundary (`validate_session_event`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 3) | Ours, decisively: identity is never a filesystem artifact that can silently fail to encode |

## What we should consider changing

### 1. State explicitly, as an [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) obligation, that a command boundary must never discard an append failure

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2 defines a typed `WrongExpectedVersion` on
guard conflict and treats `append_stream` as the one write path, but nowhere
does the ADR say, in so many words, that a command handler's caller must
propagate that result as command failure rather than logging-and-continuing.
It is implicit in "the store" being append-only and typed; it is not yet a
stated obligation on the code that calls it.

**Evidence anchor.** Amazon Q, store maturity 8/12,
`conversation.rs:420-421`: `if let Ok(cwd) = std::env::current_dir() {
os.database.set_conversation_by_path(cwd, self).ok(); }`, the save's
`Result` is discarded with `.ok()`. The in-memory turn is never rolled back
and the user is never told persistence failed; the next process start simply
resumes from whatever was last actually written, silently dropping every turn
after the last successful save.

**Blast radius.** Additive: a clarifying obligation in [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2 or
the Consequences section, not a schema change.

**Why.** This is not a store-shape problem, it is a discipline problem the
store's design does not automatically prevent: our substrate makes an append
failure typed and observable (`WrongExpectedVersion`, a decode-failure
metric per facet 3), but nothing stops a future command-handler
implementation from doing exactly what Amazon Q's `conversation.rs:421` does: matching on the `Result` and discarding it, especially on the high-volume
`Any`-guarded path, where "it almost never fails" is true enough in practice
to make silent discarding tempting. Amazon Q is the concrete demonstration of
what that costs: a class of data loss that is invisible until a user notices
their history is shorter than they remember, with nothing in the log to
explain why.

**Cost.** None beyond writing the sentence and, ideally, a lint or review
checklist item; it becomes real cost only if an implementer has already
written the `.ok()`-shaped code this is meant to forbid.

### 2. Ratify workspace-binding immutability as an explicit Non-Goal, not an implicit proto comment

**The change.** `WorkspaceRef` (`workspace.proto`) and its comment on
`SessionStarted.workspace` (`session_started.proto:19-23`) already state "the
plan's working directory is immutable for the life of the session... changing
it requires a new session or a fork," but this lives only as a proto-file
comment, not as a decision the ADR's Non-Goals section names, unlike, for
example, mid-session model switching, which [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s Non-Goals explicitly
defers.

**Evidence anchor.** Amazon Q, store maturity 8/12: because the store's key
*is* the path, moving or renaming the working directory produces a silent
cache miss: `get_conversation_by_path` returns `None`, and the CLI falls
through to `ConversationState::new(...)`, starting a brand-new, empty
conversation with no warning, no orphan reference, and no migration path
(`crates/chat-cli/src/cli/chat/mod.rs:752-764`). The dossier is explicit that
this is "by omission rather than by a deliberate no-op decision we could find
recorded" (the dossier's [Keying and identity](./index.md#keying-and-identity) section).

**Blast radius.** Additive: a documentation clarification, not a schema
change; `WorkspaceRef` already has the shape this recommendation asks the
ADR to ratify.

**Why.** Our design does not have Amazon Q's specific failure mode, because
`WorkspaceRef` is a data field carried on `SessionStarted`, not the session's
addressing key; a relocated workspace cannot produce a cache-miss-to-fresh-
session silently, because nothing about session addressing depends on the
workspace's current location. But the underlying product question Amazon
Q's omission raises (what happens when the directory a session was bound to
moves on disk) is one our proto comments already answer informally
("requires a new session or a fork") without that answer being a Non-Goal the
ADR names. Amazon Q is the evidence for *why* this needs to be a decision
recorded on purpose: it is the demonstrated cost of leaving it undecided.
Recording it costs nothing and forecloses a future proposal to add silent
relocation-reconciliation logic on the strength of "well, some product does
it", and no product in this corpus does it, so Amazon Q is the clearest
evidence that *not deciding* is itself a bad default.

**Cost.** None; this is a documentation-only change that prevents a future,
more expensive one (an ad hoc relocation-reconciliation feature added without
having weighed it against fork/new-session first).

### 3. Consider whether a lightweight, non-forking divergence marker belongs in the catalog

**The change.** Amazon Q's tangent mode, where `enter_tangent_mode`
(`conversation.rs:263-267`) snapshots `history`, `next_message`,
`transcript`, and `latest_summary` into a `tangent_state` field that *is*
serialized into the durable blob, and `exit_tangent_mode`/
`exit_tangent_mode_with_tail` (`:278-303`) restore it, is a real, shipped
answer to "let me explore a side-question and come back," distinct from both
our `SessionForked` (a new, independent session identity, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
5) and `SessionRewound` (an ordinal boundary that invalidates nothing until a
new attempt starts, decision 2). Nothing in our 41-arm catalog names "this
stretch of the stream was an aside, resume the prior context after it."

**Evidence anchor.** Amazon Q, store maturity 8/12:
`crates/chat-cli/src/cli/chat/conversation.rs:154-167,263-303`.

**Blast radius.** Additive if scoped to new event types (for example
`TangentEntered`/`TangentExited`, correlated by `turn_id`) that the
model-visible-context projection ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) folds around; no
existing event shape changes.

**Why this is worth naming, and what to solve that Amazon Q did not.** The
same dossier that documents tangent mode also documents its own open
question: "we found no explicit crash-recovery code path that automatically
re-surfaces an abandoned `tangent_state` on the next resume... a user would
have to run the exit command again after resuming" (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork) section,
flagged there as inference from an absent code path, not a confirmed
runtime trace). If we build this, that gap is the one thing not to
reproduce: an abandoned divergence must be a discoverable fact at resume
time (a fact the projection can report, e.g. "session has an open tangent
started at turn N"), not a silently-present field inside a resumed
document that only a re-issued command clears.

**Cost.** A new pair of event types plus a decide/evolve rule for what
"abandoned" means (most plausibly: a tangent with no matching `TangentExited`
by the time a new `ExecutionAttemptStarted` begins), and a resume-time
projection change to surface it. This is real, if modest, work; the
recommendation is to decide whether the UX value is worth it, not that it
obviously is.

## What our design already does better

- **Opaque identity instead of a location-derived key.** `SessionId` is
  never the working directory, the workspace, or any other environmental
  value ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1). Amazon Q's literal-path key collapses
  *location* and *conversation identity* into one, which is the direct cause
  of its parent/delegate collision risk (see Subagent cascade below) and of
  its silent relocate-and-lose-history behavior (see recommendation 2).
- **Content is claim-checked, never inlined and duplicated.** `ArtifactRef`
  (`artifact.proto`) references bytes by digest; Amazon Q's shadow-git
  `CheckpointManager` embeds a **full clone of conversation history** in
  every `Checkpoint` struct (`crates/chat-cli/src/cli/chat/checkpoint.rs:79`), so "every checkpoint's
  entire history is duplicated in memory and thus in the next JSON save"
  (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork) section), the opposite of our `Checkpoint`, which is a
  reference plus a digest (`checkpoint.proto`), never inline history.
- **Real, server-enforced optimistic concurrency on invariant-bearing
  transitions.** `WRITE_PRECONDITION` (`NoStream`/`At`/`Any`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
  decision 2) is enforced by JetStream. Amazon Q has **no application-level
  concurrency control of any kind** for conversation writes: "no
  application-level optimistic-concurrency check... gated only by SQLite's
  own file locking" (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section), which is a strictly weaker
  position than even Cline's client-issued `statusLock` CAS
  ([Cline comparison](../cline/vs-session-events.md), item 1).
- **Subagents are first-class sessions, not a side file.** `DelegationDispatched`/
  `ParentLinked` make a delegated child a real, linked stream with its own
  lifecycle ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6). Amazon Q's Delegate tool is a wholly
  separate OS process whose bookkeeping (`AgentExecution`) lives in one
  plain-JSON file per agent name, entirely outside `data.sqlite3`, with zero
  connection to the parent's conversation store beyond an accidentally
  shared working-directory key.
- **Redaction and erasure are named, typed facts.** `RedactionApplied`/
  `ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) have no analogue anywhere in the
  Amazon Q dossier; there is no privacy or masking concept of any kind.
- **Listing is a real, rebuildable capability.** `list_sessions`/
  `get_session` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) exist because we chose to build them.
  Amazon Q shows, concretely, how little a shipping product can get away
  with instead: zero enumeration surface, substituting "the directory you
  are standing in" for a picker entirely.
- **Turn identity is a stamped fact, not positional inference.** `turn_id`
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) is carried on every conversational and tool event.
  Amazon Q's ordering is "purely positional (`VecDeque` index)"
  (the dossier's [Entry/message structure and versioning](./index.md#entrymessage-structure-and-versioning) section), with round-trip identity recovered only by
  `tool_use_id` string matching inside `enforce_conversation_invariants`.
- **Typed tool-outcome resolution instead of destructive history replace.**
  `ToolCallCompleted` vs. `ToolCallFailed` under first-terminal-outcome-wins
  ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 4) never touches prior events. Amazon Q's
  `CheckpointManager::restore` does `self.history =
  checkpoint.history_snapshot.clone()`, "a full, destructive replace of the
  live history, not an append or a marker" (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork) section), for the one
  operation in Amazon Q that most resembles our rewind.

## Trade-offs, not gaps

**Whole-document mutability versus per-fact append.** Amazon Q's model buys
genuine implementation simplicity: one SQL statement, one JSON blob, no fold,
no projection, no ordinal scheme. The cost is that every size-bounding
operation is unrecoverable by construction; there is no way, even in
principle, to "keep forever" inside this design without changing it into
something else. Our append-only model pays in fold complexity and per-fact
write volume (mitigated by the `Any`-precondition commuting-fact path, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
decision 2) to buy the opposite: nothing is ever destructively unrecoverable.
Neither is free; Amazon Q chose to spend its budget on simplicity, we chose
to spend ours on recoverability.

**A location-tied `resume` flag versus an id-addressed picker.** `q chat
--resume` needs no argument beyond "am I in the right directory": zero
friction, at the cost of one conversation per directory and no way to have
two independent sessions in the same place. Our opaque `SessionId` plus
`list_sessions` buys the reverse: any number of sessions per workspace,
addressable independently, at the cost of needing an actual picker surface
for a user to choose among them.

**Separately-versioned table shape versus a single wire schema.** Amazon
Q's SQL `MIGRATIONS` ratchet (`database/mod.rs:67-76`) and its additive JSON
payload evolution are two independent mechanisms for two independent
concerns (table shape vs. document shape) that happen to converge on "be
additive wherever possible." We have only one mechanism: protobuf wire
evolution, additive within `v1alpha1` and a deliberate breaking ratchet to
`v1` later ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) §1), because we have no SQL table shape to separately
version at all. This is not a gap on either side, just a consequence of one
store having two physical substrates (SQLite table plus JSON blob) and the
other having one (the event log itself).

## What not to copy

- **Path-as-primary-key, full stop.** Collapsing *location* and *conversation
  identity* into one key is the single most consequential anti-pattern in
  this dossier: it produces the relocate-and-silently-lose-history behavior
  (recommendation 2) and the parent/delegate same-directory collision risk
  (Subagent cascade, below, an inference from reading the code, not a
  reproduced race, and it should stay described that way).
- **Two independent, both-destructive size-management mechanisms with no
  tombstone.** The 10,000-entry drain and `/compact`'s default
  `messages_to_exclude: 0` both mutate the one durable row in place; "the
  dropped turns are gone from `data.sqlite3` as well as from the model-visible
  window, not just from the model-visible window" (the dossier's [Compaction and history management](./index.md#compaction-and-history-management) section). This
  is exactly the truncation strategy [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s own Alternatives section
  already rejects ("Truncating the session log (purge-only, or
  archive-then-purge)... is a logical deletion... forecloses audit and
  rewind past the truncation point"); Amazon Q is that rejected alternative,
  shipped and load-bearing in a real vendor CLI, not a hypothetical.
- **Discarding a persistence-write `Result`.** `.ok()` on the save call
  (`conversation.rs:421`) turns a failed write into invisible data loss. See
  recommendation 1.
- **Full history duplication inside a "checkpoint."** Embedding
  `history_snapshot: VecDeque<HistoryEntry>` in every `Checkpoint` struct
  means every checkpoint duplicates the entire transcript so far. Our
  `Checkpoint` is a reference plus a digest for exactly this reason.
- **A subagent mechanism with zero notification path in either direction.**
  Amazon Q's Delegate tool has no cascade, no orphan-detection, and no
  parent-to-child or child-to-parent signal at all beyond a `kill -0` PID
  check run opportunistically by whichever process happens to call
  `status_agent`. This is a weaker position than even Cline's one-level,
  synchronous-push cascade ([Cline comparison](../cline/vs-session-events.md),
  Subagent cascade section); do not treat "no cascade" as an acceptable
  fallback position merely because a vendor CLI ships it.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream, linked by facts on each side (`DelegationDispatched`/
`ParentLinked`), acyclic by construction, with terminal cascade driven by a
reconciler reacting to Session-level terminal markers, transitively, in
O(depth) round-trips. The question here is whether Amazon Q's evidence
validates, challenges, or refines that position.

**What Amazon Q does.** Its nearest equivalent to a subagent, the Delegate
tool, is not a nested session and not a sibling row in `data.sqlite3` at
all; it is a wholly separate OS process
(`tokio::process::Command::new("q")`, `delegate.rs:341-346`). Because that
command never calls `.current_dir(...)`, the delegated process inherits the
parent's working directory (`delegate.rs:371` only *reads* the cwd for
display), and, per the "location as identity" structural point above, that
means the delegate process shares the **same primary key** in the shared
`conversations` table as the parent (the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section). The delegated
process launches `--non-interactive` with no `--resume`, so it starts a
fresh, empty `ConversationState` rather than loading the parent's row, but
on its own first save it targets that same shared key. The dossier is
explicit and careful here: **this is a structural inference from reading
`spawn_agent_process` against `set_conversation_by_path`'s key derivation,
not an observed runtime race** (the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) and [Open questions](./index.md#open-questions) sections). Carried forward
unchanged: nothing in this comparison hardens that inference into a
confirmed bug. Separately, and independently of the collision risk, Delegate
task bookkeeping (`AgentExecution`, one JSON file per agent name under
`.amazonq/.subagents/`) has no cascade, orphan, or reconcile behavior at all
on parent delete, rewind, or crash: a dead process is discovered only by
`kill -0` on a recorded `pid`, run opportunistically, with no notification to
or from the parent.

**Does this validate, challenge, or refine decision 6?** It validates the
design and adds one refinement decision 6's text does not currently name.
On cascade *mechanics*: Amazon Q has none, which is a strictly weaker
position than Cline's one-level, in-practice-usually-sufficient cascade, so
Amazon Q sits at the bottom of this corpus's cascade-maturity ladder (no
coordination at all, below Cline's synchronous one-level push, below our
transitive reconciler), and is straightforward corroborating evidence that
"cascade" is a real, unsolved industry gap decision 6 is right to close
deliberately rather than leave implicit.

The more interesting evidence is the collision risk, and what it is actually
evidence *for*. It is not, on close reading, a test of decision 6's cascade
*semantics*; it is a test of the prerequisite decision 6 silently assumes:
that a parent and a delegated child are *addressably distinct* in storage to
begin with. Decision 6 discovers children "through the parent-to-children
lineage projection folded from `DelegationDispatched`" and links them by
`ParentLinked`/`operation_id`; none of that machinery has anything to say
about a parent and child that share *the same storage key*, because
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1 (each session, each subagent, and each fork is its own
logical stream, its own subject, addressed by an opaque `SessionId`) rules
that scenario out by construction before decision 6 ever runs. Amazon Q's
same-directory collision is what happens when a system mints a real identity
(`conversation_id`) but does not use it as the addressing key; it is
evidence for why decision 1's opaque-identity choice is a *load-bearing
prerequisite* for decision 6's cascade guarantee to mean anything at all,
not evidence that decision 6 itself needs to change. Where Amazon Q's answer
is worse than ours: it has no equivalent of decision 1 to rule the collision
out, so its Delegate feature inherits the collision risk as a byproduct of a
choice (path-as-key) made for an unrelated reason, long before subagents
existed as a feature.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
snapshot-bounded replay so resume cost is O(tail) even as the log grows
forever. The question is whether Amazon Q's evidence validates that design or
exposes a cost the ADR does not bound.

**What Amazon Q does.** It does not merely fail to bound growth (Cline's
failure mode); it forecloses unbounded growth from ever being possible in the
first place, by choosing the opposite strategy: destroy old facts to keep the
row small. Two independent mechanisms enforce this on every turn: the
10,000-entry cap (`enforce_conversation_invariants`, `conversation.rs:1121-1217`,
draining via `VecDeque::drain` before the next save) and `/compact`
(`replace_history_with_summary`, `conversation.rs:732-741`, default
`messages_to_exclude: 0`), and neither leaves the pre-shrink data recoverable
anywhere: no snapshot file, no marker entry beside the summary, no tombstone.
Separately, there is no delete/TTL policy either: `delete_entry` is never
called with `Table::Conversations` anywhere in the crate, so the *only*
size-management this store has is the destructive shrink; "keep everything"
is not a state this design can be in.

**Does this validate, challenge, or refine decision 7?** It validates
decision 7's Alternatives-section rejection of truncation-as-retention,
concretely, in a shipped vendor product, rather than only in the abstract.
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) already rejects "any design that removes an event from the log...
[because it] forecloses audit and rewind past the truncation point"; Amazon
Q is exactly that rejected alternative, not a hypothetical: past the 10,000-
entry boundary or a `/compact` call, a dropped turn is gone from
`data.sqlite3` itself, not merely from the model-visible window, with no
`RedactionApplied`-equivalent masking-with-recovery option and no
`SessionRewound`-equivalent "undo"; the data is simply gone. Decision 7's
`SessionHidden`/`RedactionApplied`/`ArtifactErased` triad gives us a
recoverable-underneath, masked-on-top story Amazon Q has no equivalent of at
all.

The honest caveat, matching this product's weaker maturity axis: unlike
Cline's `cline/cline#9011` (a field-reported, issue-tracker-corroborated
freeze), **we found no issue report or user complaint anywhere in the dossier
confirming that Amazon Q's destructive shrink has caused a user-visible data-
loss incident**; this section's evidence is source-level (what the code
provably does) rather than field-level (a confirmed complaint that it did
this to someone). Weight it accordingly: it is strong evidence that the
*mechanism* is real and load-bearing in a shipped vendor product, and weaker
evidence that it has caused a specific, documented user harm. That gap is
consistent with this product's own operational-age score (1/3) being the
weakest of its four maturity axes.

## Open questions for the ADR

1. Should [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) state explicitly, as a facet 2 obligation or a
   Consequences note, that a command boundary must never discard an append
   failure, following recommendation 1 above, and Amazon Q's `.ok()`
   anti-pattern as the concrete cost of leaving it unsaid?
2. Should workspace-binding immutability (already implied by
   `WorkspaceRef`'s comment on `SessionStarted.workspace`) be promoted to a
   named Non-Goal, so a future relocation-reconciliation feature has to be
   proposed against a recorded decision rather than an implicit default?
3. Does a lightweight, non-forking divergence/"tangent" marker belong in the
   catalog at all, and if it does, who is responsible for surfacing an
   abandoned one at resume time: the model-visible-context projection
   (decision 8), or a dedicated read model?
4. Amazon Q's `/compact` auto-triggers "whenever the context window
   overflows," entirely inside the agent loop, never the store
   (`cli/compact.rs:14-30`), a concrete existing precedent for the open
   question already raised in the [Cline comparison](../cline/vs-session-events.md)
   (open question 3: who guarantees `Compacted` markers are emitted often
   enough). Should the ADR name the agent loop as that owner explicitly,
   following this precedent, rather than leaving it implicit under decision 4?
